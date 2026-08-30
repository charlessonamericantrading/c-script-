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
    err_path: PathBuf,
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
        let mut server = Serve { child, port, err_path: err_path.clone() };
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

    /// El log de stderr capturado desde que arrancó (`GRAMMAR.md §3.94`
    /// lo necesita: la advertencia de tabla-posiblemente-ajena va por acá,
    /// nunca cambia el status HTTP de ningún rpc).
    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err_path).unwrap_or_default()
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

/// GRAMMAR.md §3.102: `maxRow`/`minRow` -- caso real que los motiva
/// (IgnisLove, `bandit_rewards.link`, `getBestArm()`): `db.arms.all()[0]`
/// devuelve la fila de menor `id`, nunca la de mejor recompensa. Este test
/// confirma el `ORDER BY ... LIMIT 1` real contra Postgres, no solo SQLite
/// (ya cubierto en `runtime/mod.rs`).
#[test]
fn max_row_and_min_row_push_order_by_limit_1_to_real_sql_against_postgres() {
    const COLLECTION: &str = "arms_top_row";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("top-row");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Arm = {{ id: Int, name: String, avgRewardTenths: Int }}
db {{ {COLLECTION}: Arm[] }}
service Arms {{
  rpc create(name: String, avgRewardTenths: Int) -> Arm {{
    db.{COLLECTION}.insert(Arm {{ id: 0, name: name, avgRewardTenths: avgRewardTenths }})
  }}
  rpc getBestArm() -> Arm? {{ db.{COLLECTION}.maxRow(|a: Arm| {{ a.avgRewardTenths }}) }}
  rpc getWorstArm() -> Arm? {{ db.{COLLECTION}.minRow(|a: Arm| {{ a.avgRewardTenths }}) }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Arms/create", r#"{"name":"A","avgRewardTenths":10}"#);
    server.rpc("Arms/create", r#"{"name":"B","avgRewardTenths":95}"#);
    server.rpc("Arms/create", r#"{"name":"C","avgRewardTenths":40}"#);

    let best = server.rpc("Arms/getBestArm", "{}");
    assert_eq!(best["name"], serde_json::json!("B"), "{best}");
    let worst = server.rpc("Arms/getWorstArm", "{}");
    assert_eq!(worst["name"], serde_json::json!("A"), "{worst}");
}

/// GRAMMAR.md §3.108: `countWhere`/`findWhere` empujan a SQL real, contra
/// Postgres también (no solo SQLite, ya cubierto en `runtime/mod.rs`), los
/// cinco operadores relacionales además de `==` -- caso real que lo motiva:
/// `chat.link` de un adoptador real, `c.unreadCount > 0`.
#[test]
fn count_where_and_find_where_push_relational_operators_to_real_sql_against_postgres() {
    const COLLECTION: &str = "chats_relational_pushdown";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("relational-pushdown");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Chat = {{ id: Int, name: String, unreadCount: Int }}
db {{ {COLLECTION}: Chat[] }}
service Chats {{
  rpc add(name: String, unreadCount: Int) -> Chat {{
    db.{COLLECTION}.insert(Chat {{ id: 0, name: name, unreadCount: unreadCount }})
  }}
  rpc gt() -> Int {{ db.{COLLECTION}.countWhere(|c: Chat| {{ c.unreadCount > 0 }}) }}
  rpc gtRows() -> Chat[] {{ db.{COLLECTION}.findWhere(|c: Chat| {{ c.unreadCount > 0 }}) }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Chats/add", r#"{"name":"a","unreadCount":0}"#);
    server.rpc("Chats/add", r#"{"name":"b","unreadCount":3}"#);
    server.rpc("Chats/add", r#"{"name":"c","unreadCount":5}"#);

    let count = server.rpc("Chats/gt", "{}");
    assert_eq!(count, serde_json::json!(2), "{count}");
    let rows = server.rpc("Chats/gtRows", "{}");
    assert_eq!(rows.as_array().unwrap().len(), 2, "{rows}");
}

/// GRAMMAR.md §3.109: generaliza el test anterior a una conjunción `&&` de
/// varias hojas -- el caso real de "CRM" que lo motiva, `notifications.link`:
/// `n.userId == uid && !n.read`. Contra un Postgres real para confirmar que
/// el `AND` generado (dos placeholders `$1`/`$2` en Postgres, no solo uno)
/// bindea en el orden correcto y no corrompe el protocolo binario -- el
/// mismo tipo de bug que el fix de escritura de `Int` (§3.104) hubiera
/// dejado pasar si solo se hubiera probado contra SQLite.
#[test]
fn count_where_and_find_where_push_a_conjunction_of_leaves_to_real_sql_against_postgres() {
    const COLLECTION: &str = "notifications_conjunction_pushdown";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("conjunction-pushdown");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Notification = {{ id: Int, userId: Int, read: Bool }}
db {{ {COLLECTION}: Notification[] }}
service Notifications {{
  rpc add(userId: Int, read: Bool) -> Notification {{
    db.{COLLECTION}.insert(Notification {{ id: 0, userId: userId, read: read }})
  }}
  rpc unreadFor(userId: Int) -> Int {{
    db.{COLLECTION}.countWhere(|n: Notification| {{ n.userId == userId && !n.read }})
  }}
  rpc unreadRowsFor(userId: Int) -> Notification[] {{
    db.{COLLECTION}.findWhere(|n: Notification| {{ n.userId == userId && !n.read }})
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Notifications/add", r#"{"userId":1,"read":false}"#);
    server.rpc("Notifications/add", r#"{"userId":1,"read":true}"#);
    server.rpc("Notifications/add", r#"{"userId":2,"read":false}"#);

    let count = server.rpc("Notifications/unreadFor", r#"{"userId":1}"#);
    assert_eq!(count, serde_json::json!(1), "{count}");
    let rows = server.rpc("Notifications/unreadRowsFor", r#"{"userId":1}"#);
    assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
}

/// GRAMMAR.md §3.75, landmine del barrido de "límites honestos" (26/08/2026):
/// `upsert` con un `matchFn` pusheable (`|c: T| { c.campo == valor }`) ahora
/// usa el mismo `find_where_conjunction` que `findWhere`/`countWhere`/
/// `deleteWhere` -- este test confirma que el camino nuevo funciona de
/// verdad contra Postgres (backend distinto, generación de SQL distinta a
/// SQLite): un segundo `upsert` sobre el MISMO valor actualiza la fila
/// existente (mismo id), nunca inserta una fila nueva.
#[test]
fn upsert_with_a_pushable_match_fn_updates_the_same_row_against_real_postgres() {
    const COLLECTION: &str = "counters_upsert_pushdown";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("upsert-pushdown");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Counter = {{ id: Int, name: String, count: Int }}
type NewCounter = {{ name: String, count: Int }}
db {{ {COLLECTION}: Counter[] }}
service Counters {{
  rpc bump(name: String) -> Counter {{
    db.{COLLECTION}.upsert(
      |c: Counter| {{ c.name == name }},
      NewCounter {{ name: name, count: 1 }},
      |c: Counter| {{ NewCounter {{ name: c.name, count: c.count + 1 }} }}
    )
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let first = server.rpc("Counters/bump", r#"{"name":"clics"}"#);
    let second = server.rpc("Counters/bump", r#"{"name":"clics"}"#);
    assert_eq!(first["id"], second["id"], "misma fila, mismo id: {first:?} vs {second:?}");
    assert_eq!(second["count"], serde_json::json!(2), "{second:?}");

    let other = server.rpc("Counters/bump", r#"{"name":"otro"}"#);
    assert_ne!(other["id"], second["id"], "un name distinto sí inserta una fila nueva: {other:?}");
}

/// Bug real, encontrado por una auditoría multi-agente adversarial
/// (26/08/2026): `"campo" = ?` ligado a un parámetro NULL nunca es cierto en
/// SQL, así que un `upsert` con `matchFn = |c| { c.opcional == variable }`
/// donde `variable` resulta `null` insertaba una fila duplicada en vez de
/// actualizar la existente -- divergía del camino interpretado, que trata
/// `Value::Null == Value::Null` como `true`. Lado Postgres del mismo test
/// que ya existe contra SQLite (runtime/mod.rs).
#[test]
fn upsert_pushdown_matches_an_existing_null_valued_optional_field_against_real_postgres() {
    const COLLECTION: &str = "items_upsert_null_pushdown";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("upsert-null-pushdown");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, note: String? }}
type NewItem = {{ name: String, note: String? }}
db {{ {COLLECTION}: Item[] }}
service S {{
  rpc upsertByNote(name: String, note: String?) -> Item {{
    db.{COLLECTION}.upsert(
      |c: Item| {{ c.note == note }},
      NewItem {{ name: name, note: note }},
      |c: Item| {{ NewItem {{ name: name, note: note }} }}
    )
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let first = server.rpc("S/upsertByNote", r#"{"name":"first","note":null}"#);
    let second = server.rpc("S/upsertByNote", r#"{"name":"second","note":null}"#);
    assert_eq!(first["id"], second["id"], "el segundo upsert con note=null debe ACTUALIZAR la misma fila: {first:?} vs {second:?}");
    assert_eq!(second["name"], serde_json::json!("second"), "{second:?}");

    let third = server.rpc("S/upsertByNote", r#"{"name":"third","note":"real"}"#);
    assert_ne!(third["id"], second["id"], "un note real y distinto de null sigue insertando una fila nueva: {third:?}");
    let fourth = server.rpc("S/upsertByNote", r#"{"name":"fourth","note":"real"}"#);
    assert_eq!(third["id"], fourth["id"], "el mismo note real sigue actualizando la misma fila: {third:?} vs {fourth:?}");
}

/// GRAMMAR.md §3.154: `transaction { ... }` -- `BEGIN`/`COMMIT`/`ROLLBACK`
/// reales contra Postgres, no solo SQLite (backend distinto, mismo
/// `execute_ddl` pero otra implementación por debajo -- `client.batch_execute`
/// vía `with_reconnect`). Un `panic` a mitad del bloque tiene que deshacer
/// TODO lo que la transacción alcanzó a escribir; una transacción exitosa
/// tiene que confirmar de verdad.
#[test]
fn transaction_commits_and_rolls_back_for_real_against_postgres() {
    const COLLECTION: &str = "stock_transaction_pushdown";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("transaction-pushdown");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Stock = {{ id: Int, productId: Int, quantity: Int }}
db {{ {COLLECTION}: Stock[] }}
service Shop {{
  rpc seedStock(productId: Int, qty: Int) -> Stock {{
    db.{COLLECTION}.insert(Stock {{ id: 0, productId: productId, quantity: qty }})
  }}
  rpc reserve(productId: Int, qty: Int) -> Stock {{
    transaction {{
      let matches = db.{COLLECTION}.findWhere(|s: Stock| {{ s.productId == productId }});
      let s = matches[0];
      if s.quantity < qty {{
        panic("stock insuficiente");
      }} else {{
      }}
      db.{COLLECTION}.increment(s.id, |x: Stock| {{ x.quantity }}, 0 - qty)
    }}
  }}
  rpc stockFor(productId: Int) -> Int {{
    let matches = db.{COLLECTION}.findWhere(|s: Stock| {{ s.productId == productId }});
    matches[0].quantity
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Shop/seedStock", r#"{"productId":1,"qty":10}"#);

    let failed = server.try_rpc("Shop/reserve", r#"{"productId":1,"qty":999}"#);
    assert!(failed.is_err(), "stock insuficiente debe fallar: {failed:?}");
    let stock = server.rpc("Shop/stockFor", r#"{"productId":1}"#);
    assert_eq!(stock, serde_json::json!(10), "el ROLLBACK real debe dejar el stock intacto: {stock:?}");

    let reserved = server.rpc("Shop/reserve", r#"{"productId":1,"qty":4}"#);
    assert_eq!(reserved["quantity"], serde_json::json!(6), "{reserved:?}");
    let stock = server.rpc("Shop/stockFor", r#"{"productId":1}"#);
    assert_eq!(stock, serde_json::json!(6), "el COMMIT real debe reflejarse: 10 - 4 = 6");
}

/// GRAMMAR.md §3.105: `db.<c>.increment` es un `UPDATE campo = campo +
/// delta` atómico -- SIN ida y vuelta de lectura previa. La prueba real de
/// que esto arregla el lost-update reportado (IgnisLove, varios procesos
/// incrementando la MISMA fila a la vez) es exactamente esta: muchos
/// hilos, cada uno con su PROPIA conexión HTTP, incrementando el mismo
/// contador en simultáneo contra un Postgres real -- si `increment`
/// hiciera read-then-write (como el `upsert` con `updateFn` que este
/// método reemplaza en los `.link` reales), esta concurrencia perdería
/// incrementos con altísima probabilidad. Con el `UPDATE` atómico, el
/// total final tiene que ser EXACTO, siempre.
#[test]
fn increment_never_loses_an_update_under_real_concurrent_writers() {
    const COLLECTION: &str = "counters_concurrent_increment";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("concurrent-increment");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Counter = {{ id: Int, hits: Int }}
db {{ {COLLECTION}: Counter[] }}
service Counters {{
  rpc create() -> Counter {{ db.{COLLECTION}.insert(Counter {{ id: 0, hits: 0 }}) }}
  rpc bump(id: Int) -> Counter {{ db.{COLLECTION}.increment(id, |c: Counter| {{ c.hits }}, 1) }}
  rpc get(id: Int) -> Counter? {{ db.{COLLECTION}.find(id) }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let created = server.rpc("Counters/create", "{}");
    let id = created["id"].as_i64().expect("insert devuelve un id");

    const THREADS: usize = 20;
    const BUMPS_PER_THREAD: usize = 25;
    let port = server.port;
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(move || {
                for _ in 0..BUMPS_PER_THREAD {
                    let body = format!(r#"{{"id":{id}}}"#);
                    ureq::post(&format!("http://127.0.0.1:{port}/Counters/bump"))
                        .set("Content-Type", "application/json")
                        .send_string(&body)
                        .unwrap_or_else(|e| panic!("Counters/bump falló: {e}"));
                }
            });
        }
    });

    let result = server.rpc("Counters/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(
        result["hits"],
        serde_json::json!((THREADS * BUMPS_PER_THREAD) as i64),
        "increment tiene que ser atómico -- ni un solo +1 de {} debería perderse: {result}",
        THREADS * BUMPS_PER_THREAD
    );
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

const TIMESTAMP_TRUNCATE_AGGREGATE_PROGRAM: &str = r#"
type Sale = { id: Int, at: Timestamp, amount: Int }
type DayTotal = { key: Timestamp, value: Int }

db { COLLECTION: Sale[], }

service Sales {
  rpc create(at: Timestamp, amount: Int) -> Sale {
    db.COLLECTION.insert(Sale { id: 0, at: at, amount: amount })
  }

  rpc byDay() -> DayTotal[] {
    db.COLLECTION.sumBy(|s: Sale| { s.at.truncateToDay() }, |s: Sale| { s.amount })
  }

  rpc byMonth() -> DayTotal[] {
    db.COLLECTION.sumBy(|s: Sale| { s.at.truncateToMonth() }, |s: Sale| { s.amount })
  }
}
"#;

#[test]
fn sum_by_truncated_to_day_and_month_pushes_a_utc_date_trunc_to_real_postgres() {
    // GRAMMAR.md §3.157: cierra el límite que §3.65 dejaba abierto -- agrupar
    // por un Timestamp truncado. Este test importa especialmente contra
    // Postgres porque es el backend donde el truncado depende de verdad del
    // manejo de timezone (`date_trunc(unit, ts, 'UTC')`, el overload de 3
    // argumentos que NO depende del `TimeZone` de la sesión) -- SQLite no
    // tiene esa clase de bug posible, su `strftime('start of day', ...)`
    // siempre opera sobre el valor tal cual. También confirma que la `key`
    // agrupada viaja como STRING ISO-8601 en el JSON (GRAMMAR.md §3.31), no
    // como número -- `scalar_cell_to_value` no tenía brazo para `Timestamp`
    // antes de esta ronda (un bug real, encontrado en la propia verificación
    // manual antes de shippear, no solo la ausencia de la feature).
    const COLLECTION: &str = "sales_timestamp_trunc";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("timestamp-trunc-aggregate");
    let src = temp.write("app.link", &TIMESTAMP_TRUNCATE_AGGREGATE_PROGRAM.replace("COLLECTION", COLLECTION));
    let server = Serve::start(&src, &url);

    server.rpc("Sales/create", r#"{"at":"2026-03-15T10:30:00.000Z","amount":10}"#);
    server.rpc("Sales/create", r#"{"at":"2026-03-15T23:59:59.999Z","amount":5}"#);
    server.rpc("Sales/create", r#"{"at":"2026-03-20T00:00:00.000Z","amount":7}"#);
    server.rpc("Sales/create", r#"{"at":"2027-01-05T05:00:00.000Z","amount":3}"#);

    let by_day = server.rpc("Sales/byDay", "{}");
    let day_rows = by_day.as_array().unwrap();
    assert_eq!(day_rows.len(), 3, "3 dias distintos: {day_rows:?}");
    for row in day_rows {
        assert!(row["key"].is_string(), "la key Timestamp debe viajar como string ISO-8601: {row:?}");
    }
    let by_day_key: std::collections::HashMap<String, i64> =
        day_rows.iter().map(|r| (r["key"].as_str().unwrap().to_string(), r["value"].as_i64().unwrap())).collect();
    assert_eq!(by_day_key.get("2026-03-15T00:00:00.000Z"), Some(&15), "10 + 5 en el mismo dia: {by_day_key:?}");
    assert_eq!(by_day_key.get("2026-03-20T00:00:00.000Z"), Some(&7), "{by_day_key:?}");
    assert_eq!(by_day_key.get("2027-01-05T00:00:00.000Z"), Some(&3), "{by_day_key:?}");

    let by_month = server.rpc("Sales/byMonth", "{}");
    let month_rows = by_month.as_array().unwrap();
    assert_eq!(month_rows.len(), 2, "2 meses distintos: {month_rows:?}");
    let by_month_key: std::collections::HashMap<String, i64> =
        month_rows.iter().map(|r| (r["key"].as_str().unwrap().to_string(), r["value"].as_i64().unwrap())).collect();
    assert_eq!(by_month_key.get("2026-03-01T00:00:00.000Z"), Some(&22), "10 + 5 + 7 en marzo: {by_month_key:?}");
    assert_eq!(by_month_key.get("2027-01-01T00:00:00.000Z"), Some(&3), "{by_month_key:?}");
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

// ---- `id: Uuid` como PK alternativa (GRAMMAR.md §3.177) ----

fn uuid_pk_link_source(collection: &str) -> String {
    format!(
        r#"
type Lead = {{ id: Uuid, email: String, score: Int }}
type NewLead = {{ email: String, score: Int }}
db {{ {collection}: Lead[] }}
service Leads {{
  rpc create(email: String, score: Int) -> Lead {{ db.{collection}.insert(NewLead {{ email: email, score: score }}) }}
  rpc get(id: Uuid) -> Lead? {{ db.{collection}.find(id) }}
  rpc list() -> Lead[] {{ db.{collection}.all() }}
  rpc update(id: Uuid, patch: Patch<Lead>) -> Lead {{ db.{collection}.applyPatch(id, patch) }}
  rpc remove(id: Uuid) -> Bool {{ db.{collection}.delete(id) }}
}}
"#
    )
}

#[test]
fn uuid_pk_collection_supports_the_full_crud_cycle_against_a_fresh_postgres_table() {
    // GRAMMAR.md §3.177, camino NO adoptado: `linkc serve` crea la tabla
    // por su cuenta, con el tipo NATIVO `UUID` de Postgres para "id"
    // (`create_postgres_table_sql`) -- confirma que el cast `::uuid`
    // explícito que `Db::id_placeholder` agrega funciona de punta a punta
    // contra el driver Postgres real (`postgres` crate, sin el feature
    // `with-uuid-1`), no solo en la teoría del protocolo binario.
    const COLLECTION: &str = "leads_uuid_pk_fresh";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("uuid-pk-fresh");
    let src = temp.write("app.link", &uuid_pk_link_source(COLLECTION));
    let server = Serve::start(&src, &url);

    let created = server.rpc("Leads/create", r#"{"email":"a@example.com","score":3}"#);
    let id = created["id"].as_str().expect("insert devuelve un id Uuid real, no un error de decodificación").to_string();
    assert_eq!(id.len(), 36, "{created:?}");

    let fetched = server.rpc("Leads/get", &format!(r#"{{"id":"{id}"}}"#));
    assert_eq!(fetched["email"], "a@example.com", "find por el mismo uuid encuentra la fila real");

    let updated = server.rpc("Leads/update", &format!(r#"{{"id":"{id}","patch":{{"score":9}}}}"#));
    assert_eq!(updated["score"], 9);
    assert_eq!(updated["id"], id, "applyPatch nunca cambia el id");

    let listed = server.rpc("Leads/list", "{}");
    assert_eq!(listed.as_array().map(|a| a.len()), Some(1));

    let removed = server.rpc("Leads/remove", &format!(r#"{{"id":"{id}"}}"#));
    assert_eq!(removed, true);
    let gone = server.rpc("Leads/get", &format!(r#"{{"id":"{id}"}}"#));
    assert!(gone.is_null(), "borrada de verdad: {gone:?}");
}

#[test]
fn adopt_existing_uuid_pk_table_supports_the_full_crud_cycle_against_a_real_postgres_table() {
    // El caso REAL que motiva GRAMMAR.md §3.177 -- reporte de adopción de
    // iaacademy (vía skynet-43): tablas de producción YA EXISTENTES con
    // "id uuid DEFAULT gen_random_uuid()", que `linkc serve --adopt-existing`
    // nunca podía usar (rechazadas al conectar, GRAMMAR.md §3.36/§3.59).
    // Crea la tabla A MANO (simulando la que ya existía en producción,
    // generación de id por DEFAULT de Postgres incluida) y confirma que
    // c-script -- que SIEMPRE genera su propio id del lado de la
    // aplicación, nunca depende de ese DEFAULT -- puede leerla y escribirla
    // de punta a punta sin migrar nada.
    const COLLECTION: &str = "leads_uuid_pk_adopted";
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
                \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(), \
                \"email\" TEXT NOT NULL, \
                \"score\" BIGINT NOT NULL\
            )"
        ))
        .expect("crear la tabla preexistente con id uuid + DEFAULT gen_random_uuid(), como la produce otro backend");

    let temp = TempDir::new("uuid-pk-adopted");
    let src = temp.write("app.link", &uuid_pk_link_source(COLLECTION));
    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);

    let created = server.rpc("Leads/create", r#"{"email":"b@example.com","score":1}"#);
    let id = created["id"].as_str().expect("insert devuelve un id Uuid real contra la tabla adoptada").to_string();
    assert_eq!(id.len(), 36, "{created:?}");

    let fetched = server.rpc("Leads/get", &format!(r#"{{"id":"{id}"}}"#));
    assert_eq!(fetched["email"], "b@example.com");

    // Confirma en SQL crudo que el id que c-script generó -- NO el
    // DEFAULT de la columna, que nunca se ejercita en este camino -- es
    // el que de verdad quedó en la fila.
    let mut check_client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar de nuevo, por fuera de c-script");
    let row = check_client
        .query_one(&format!("SELECT \"id\"::text, \"email\" FROM \"{COLLECTION}\""), &[])
        .expect("leer la fila insertada con SQL crudo");
    let raw_id: String = row.get(0);
    assert_eq!(raw_id, id, "el id que insertó c-script es un uuid REAL de la columna nativa, legible con SQL común");

    let removed = server.rpc("Leads/remove", &format!(r#"{{"id":"{id}"}}"#));
    assert_eq!(removed, true);
}

#[test]
fn migrate_dry_run_reports_no_changes_for_an_existing_native_uuid_pk_table() {
    // Antes de GRAMMAR.md §3.177, esta misma tabla hacía que `migrate
    // --dry-run` reportara "¡ESTO FALLARÍA AL CONECTAR DE VERDAD!" (ver
    // `a_preexisting_table_with_a_non_integer_id_fails_at_connect_not_at_first_insert`,
    // que sigue siendo el comportamiento correcto para un .link que declara
    // 'id: Int' contra esta misma tabla -- acá el .link declara 'id: Uuid',
    // que SÍ es compatible).
    const COLLECTION: &str = "leads_uuid_pk_dry_run";
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
                \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(), \
                \"email\" TEXT NOT NULL, \
                \"score\" BIGINT NOT NULL\
            )"
        ))
        .expect("crear la tabla preexistente con id uuid a mano");

    let temp = TempDir::new("uuid-pk-dry-run");
    let src = temp.write("app.link", &uuid_pk_link_source(COLLECTION));

    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("migrate")
        .arg(&src)
        .arg("--db")
        .arg(&url)
        .arg("--dry-run")
        .output()
        .expect("ejecutar linkc migrate --dry-run");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(!report.contains("FALLARÍA"), "una PK Uuid contra una columna uuid nativa es compatible, no un rechazo: {report}");
    assert!(report.contains("Nada que migrar"), "{report}");
}

// ---- rate limiting DISTRIBUIDO vía Postgres (GRAMMAR.md §3.178) ----

const RATE_LIMIT_PROGRAM: &str = r#"
service Sys {
  @rate_limit("5/2s")
  rpc ping() -> String { "pong" }
}
"#;

/// Ventana larga a propósito -- para el test de concurrencia real (16
/// requests repartidas entre dos procesos, cada una compitiendo por el
/// LOCK de fila del mismo bucket): con una ventana corta, el tiempo real
/// que tarda una ráfaga serializada por lock contention en CI podría
/// acumular refill suficiente para admitir de más, un falso negativo que
/// no tiene nada que ver con si el bucket está de verdad compartido. Con
/// `5/60s` (refill ~0.083/s), ni varios segundos de ejecución real
/// alcanzan para sumar un token entero de más.
const RATE_LIMIT_PROGRAM_LONG_WINDOW: &str = r#"
service Sys {
  @rate_limit("5/60s")
  rpc ping() -> String { "pong" }
}
"#;

/// `POST <url>` -- devuelve el status HTTP real (200/429/...), sin
/// panickear ante un no-2xx (a diferencia de `Serve::rpc`, que sí lo hace
/// -- acá el punto es justamente poder ver un 429 real sin abortar el test).
fn post_status(url: &str, body: &str) -> u16 {
    match ureq::post(url).set("Content-Type", "application/json").send_string(body) {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(status, _)) => status,
        Err(e) => panic!("POST {url} falló de red: {e}"),
    }
}

#[test]
fn distributed_rate_limit_shares_one_bucket_across_two_real_server_instances() {
    // El punto entero de GRAMMAR.md §3.178: `@rate_limit("5/60s")` tiene
    // que limitar a 5 requests TOTAL entre las dos instancias, no 5 por
    // instancia (10 en total) -- que es exactamente lo que pasaría con el
    // `RateLimiter` en memoria de siempre, cada uno con su propio HashMap.
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    // Sin `reset_schema`: este programa no declara ningún `db {}`, nada
    // que dropear -- pero SÍ hace falta arrancar con la tabla interna de
    // rate limiting limpia, para que un bucket que otra corrida anterior
    // dejó a medio consumir no arranque este test con menos capacidad
    // disponible de la esperada.
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let _ = client.batch_execute("DROP TABLE IF EXISTS \"_linkc_internal_rate_limits\"");

    let temp_a = TempDir::new("rate-limit-distributed-a");
    let temp_b = TempDir::new("rate-limit-distributed-b");
    let link_a = temp_a.write("app.link", RATE_LIMIT_PROGRAM_LONG_WINDOW);
    let link_b = temp_b.write("app.link", RATE_LIMIT_PROGRAM_LONG_WINDOW);
    // Arrancan una atrás de otra: la primera crea la tabla interna (sin
    // --adopt-existing), la segunda la encuentra ya creada -- las dos
    // terminan con `distributed_rate_limit = true` de todos modos
    // (`postgres_table_exists`/`CREATE TABLE IF NOT EXISTS`, cualquiera
    // de los dos caminos).
    let server_a = Serve::start(&link_a, &url);
    let server_b = Serve::start(&link_b, &url);
    let url_a = format!("http://127.0.0.1:{}/Sys/ping", server_a.port);
    let url_b = format!("http://127.0.0.1:{}/Sys/ping", server_b.port);

    const REQUESTS_PER_SERVER: usize = 8; // 16 en total, contra una capacidad de 5
    let statuses = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..REQUESTS_PER_SERVER * 2)
            .map(|i| {
                let target = if i % 2 == 0 { &url_a } else { &url_b };
                scope.spawn(move || post_status(target, "{}"))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<u16>>()
    });

    let admitted = statuses.iter().filter(|&&s| s == 200).count();
    let rejected = statuses.iter().filter(|&&s| s == 429).count();
    assert_eq!(
        admitted, 5,
        "capacidad compartida entre las dos instancias: exactamente 5 admitidas, no 5 por instancia. statuses={statuses:?}\n\
         stderr A: {}\nstderr B: {}",
        server_a.stderr(), server_b.stderr()
    );
    assert_eq!(rejected, REQUESTS_PER_SERVER * 2 - 5, "el resto tiene que ser 429, nunca otro status: statuses={statuses:?}");
}

#[test]
fn distributed_rate_limit_refills_over_time_like_the_in_memory_bucket() {
    // Mismo algoritmo que `rate_limit::RateLimiter` (refill CONTINUO, no
    // por ventanas fijas que resetean de golpe) -- agotar el bucket y
    // esperar más que la ventana completa tiene que admitir de nuevo.
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let _ = client.batch_execute("DROP TABLE IF EXISTS \"_linkc_internal_rate_limits\"");

    let temp = TempDir::new("rate-limit-distributed-refill");
    let link = temp.write("app.link", RATE_LIMIT_PROGRAM);
    let server = Serve::start(&link, &url);
    let ping_url = format!("http://127.0.0.1:{}/Sys/ping", server.port);

    for _ in 0..5 {
        assert_eq!(post_status(&ping_url, "{}"), 200, "las primeras 5 (la capacidad completa) tienen que admitirse");
    }
    assert_eq!(post_status(&ping_url, "{}"), 429, "la 6ta, sin que pase tiempo, tiene que rechazarse");

    std::thread::sleep(std::time::Duration::from_millis(2200)); // > los 2s de la ventana completa
    assert_eq!(post_status(&ping_url, "{}"), 200, "después de refillear la ventana completa, vuelve a admitir");
}

#[test]
fn adopt_existing_falls_back_to_in_memory_rate_limiting_without_the_internal_table() {
    // `--adopt-existing` nunca ejecuta DDL, ni siquiera para la tabla
    // interna de rate limiting propia -- sin ella ya creada a mano, el
    // servidor tiene que arrancar y servir requests normalmente (el
    // `RateLimiter` en memoria de siempre, degradado en silencio salvo
    // por el `distributed_rate_limit = false` interno), nunca un fallo de
    // arranque por una tabla que ni siquiera es del usuario.
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let _ = client.batch_execute("DROP TABLE IF EXISTS \"_linkc_internal_rate_limits\"");

    let temp = TempDir::new("rate-limit-adopt-existing");
    let link = temp.write("app.link", RATE_LIMIT_PROGRAM);
    let server = Serve::start_with_args(&link, &url, &["--adopt-existing"]);
    let ping_url = format!("http://127.0.0.1:{}/Sys/ping", server.port);

    for _ in 0..5 {
        assert_eq!(post_status(&ping_url, "{}"), 200, "el limitador en memoria sigue funcionando sin la tabla interna");
    }
    assert_eq!(post_status(&ping_url, "{}"), 429, "y sigue rechazando al agotar la capacidad, solo que por-proceso, no compartido");
    let mut check_client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar de nuevo");
    let exists = check_client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = '_linkc_internal_rate_limits')",
            &[],
        )
        .map(|row| row.get::<_, bool>(0))
        .unwrap_or(false);
    assert!(!exists, "--adopt-existing nunca debe haber creado la tabla interna");
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

/// GRAMMAR.md §3.150: la instancia que RECIBE un cambio remoto (A, en el
/// test de arriba) es la que mide y reporta la latencia -- este test es la
/// contraparte que confirma que `GET /metrics` de esa instancia muestra un
/// evento real después de una propagación cross-instancia real.
#[test]
fn metrics_reports_notify_propagation_latency_after_a_real_cross_instance_write() {
    const COLLECTION: &str = "items_metrics_notify_latency";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("metrics-notify-latency");
    let src = temp.write("app.link", &PUSH_PROGRAM.replace("COLLECTION", COLLECTION));

    let instance_a = Serve::start(&src, &url);
    let instance_b = Serve::start(&src, &url);

    let mut watcher = StreamClient::connect(instance_a.port, "/Items/watchAll");
    instance_b.rpc("Items/create", r#"{"name":"desde-B"}"#);
    watcher.next_event().expect("la instancia A debió recibir el push de la instancia B");

    // Un margen chico: el drenado del canal remoto (que registra la
    // métrica) corre en el loop principal de A, no sincrónico con la
    // entrega al `stream` -- los dos pasan por el MISMO tick, pero sin una
    // señal explícita de "ya se registró la métrica" que esperar.
    std::thread::sleep(Duration::from_millis(200));

    let text = ureq::get(&format!("http://127.0.0.1:{}/metrics", instance_a.port))
        .call()
        .unwrap_or_else(|e| panic!("GET /metrics falló: {e}"))
        .into_string()
        .expect("leer el body");
    assert!(text.contains("linkc_notify_latency_seconds_count 1"), "body: {text}");
    assert!(text.contains("linkc_notify_latency_seconds_sum "), "body: {text}");
}

/// GRAMMAR.md §3.44: landmine encontrado en un barrido de "límites
/// honestos" -- un payload de NOTIFY de más de 8000 bytes se descarta PARA
/// SIEMPRE (nunca se reintenta, no lo arreglaría), con la única señal antes
/// de esta ronda un `eprintln!` que nadie lee corriendo desatendido. Este
/// test confirma que el conteo real -- no solo el mensaje de stderr, ya
/// probado en otro lado -- queda visible en `/metrics`, en la MISMA
/// instancia que escribió (el drop pasa en el envío, no en la recepción).
#[test]
fn metrics_reports_an_oversized_notify_payload_dropped_for_real() {
    const COLLECTION: &str = "items_metrics_oversized_notify";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("metrics-oversized-notify");
    let src = temp.write("app.link", &PUSH_PROGRAM.replace("COLLECTION", COLLECTION));
    let server = Serve::start(&src, &url);

    // Un "name" de 8200 caracteres empuja el payload entero del NOTIFY
    // (que envuelve la fila completa más instance/collection/sent_at_ms)
    // bien por encima del límite real de Postgres (8000 bytes) -- la
    // inserción en sí debe seguir funcionando normal, solo la propagación
    // remota se descarta.
    let huge_name = "x".repeat(8200);
    let created = server.rpc("Items/create", &serde_json::json!({"name": huge_name}).to_string());
    assert_eq!(created["name"], huge_name, "el insert local no debe verse afectado por el tamaño");

    let text = ureq::get(&format!("http://127.0.0.1:{}/metrics", server.port))
        .call()
        .unwrap_or_else(|e| panic!("GET /metrics falló: {e}"))
        .into_string()
        .expect("leer el body");
    assert!(text.contains(&format!("linkc_notify_oversized_dropped_total{{collection=\"{COLLECTION}\"}} 1")), "body: {text}");

    // Un segundo insert normal (nombre chico) no debe sumar al contador de
    // OTRA colección ni inflar este -- confirma que el conteo es real, no
    // un valor fijo.
    server.rpc("Items/create", r#"{"name":"chico"}"#);
    let text_after = ureq::get(&format!("http://127.0.0.1:{}/metrics", server.port))
        .call()
        .unwrap_or_else(|e| panic!("GET /metrics falló: {e}"))
        .into_string()
        .expect("leer el body");
    assert!(text_after.contains(&format!("linkc_notify_oversized_dropped_total{{collection=\"{COLLECTION}\"}} 1")), "body: {text_after}");
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
    //
    // `linkc introspect <url>` (sin filtro de tabla) escanea la base
    // ENTERA, no solo `{COLLECTION}` -- corriendo en paralelo con otros
    // tests de este archivo que crean SUS PROPIAS tablas en la misma base
    // (`cargo test` no serializa por default, CI tampoco fija
    // `--test-threads=1`), `generated` puede traer de arrastre otras
    // colecciones ajenas en su `db {{ ... }}` -- si alguna de esas tablas
    // no es apta para c-script (ej. un "id" `uuid`, GRAMMAR.md §3.59), el
    // `Serve::start` de ESTE test fallaría por una tabla que no tiene nada
    // que ver con lo que se está probando acá. Se extrae SOLO el bloque
    // `type LegacyCustomers = {{ ... }}` del output real de introspect (la
    // parte que de verdad se quiere verificar) y se arma un programa
    // mínimo propio alrededor, en vez de reusar el `db {{ ... }}` de
    // introspect tal cual.
    let type_start = generated.find("type LegacyCustomers").expect("el tipo generado tiene que estar en el output");
    let type_end = generated[type_start..].find('}').map(|i| type_start + i + 1).expect("el tipo generado tiene que cerrar con '}'");
    let legacy_customers_type = &generated[type_start..type_end];
    let full_program = format!(
        "{legacy_customers_type}\ndb {{ {COLLECTION}: LegacyCustomers[] }}\nservice Check {{\n  rpc list() -> LegacyCustomers[] {{ db.{COLLECTION}.all() }}\n}}\n"
    );
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

#[test]
fn introspect_emits_id_uuid_for_a_native_uuid_primary_key_with_no_warning() {
    // Reporte real de adopción (iaacademy, vía sesión skynet-43, 2026-08-29)
    // + GRAMMAR.md §3.177 (soporte real de `id: Uuid` como PK): una tabla
    // con `id uuid` PRIMARY KEY ahora es un mapeo EXACTO, sin advertencia --
    // antes de §3.177, generaba `id: Int` (v1.132.0 agregó una advertencia
    // ahí; esta ronda reemplaza ese placeholder por el tipo real, ahora que
    // c-script sabe adoptar esa forma de PK de punta a punta).
    const COLLECTION: &str = "legacy_leads";
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
                \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(), \
                \"email\" TEXT NOT NULL\
            )"
        ))
        .expect("crear la tabla con PK uuid a mano");

    let output =
        Command::new(env!("CARGO_BIN_EXE_linkc")).arg("introspect").arg(&url).output().expect("ejecutar linkc introspect");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let generated = String::from_utf8_lossy(&output.stdout).to_string();
    let warnings = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(generated.contains("id: Uuid,"), "{generated}");
    // `introspect` escanea TODA la base -- otros tests corriendo en
    // paralelo (mismo motivo que el comentario de GRAMMAR.md §4831 sobre
    // esta suite) pueden dejar SUS propias advertencias en el mismo
    // stderr, de tablas ajenas a `COLLECTION`. Lo que este test fija es
    // que la advertencia sobre "id" que existía ANTES de §3.177 ya no
    // aparece para ESTA tabla puntual -- no que el stderr entero esté vacío.
    let own_id_warning = warnings.lines().find(|line| line.contains(COLLECTION) && line.contains("\"id\""));
    assert!(own_id_warning.is_none(), "no debería haber ninguna advertencia sobre 'id' de '{COLLECTION}': {own_id_warning:?}");
}

#[test]
fn introspect_still_warns_when_the_id_primary_key_is_neither_integer_nor_native_uuid() {
    // Una PK "id" de un tipo que c-script no sabe adoptar como PK todavía
    // (ni entero ni uuid nativo -- acá, TEXT, el caso de un backend legacy
    // que guardaba un UUID como string plano) sigue emitiendo el placeholder
    // `id: Int` de siempre, con una advertencia -- distinto del caso `uuid`
    // nativo de arriba, que GRAMMAR.md §3.177 sí soporta de punta a punta.
    const COLLECTION: &str = "legacy_orders";
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
                \"id\" TEXT PRIMARY KEY, \
                \"total\" BIGINT NOT NULL\
            )"
        ))
        .expect("crear la tabla con PK text a mano");

    let output =
        Command::new(env!("CARGO_BIN_EXE_linkc")).arg("introspect").arg(&url).output().expect("ejecutar linkc introspect");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let generated = String::from_utf8_lossy(&output.stdout).to_string();
    let warnings = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(generated.contains("id: Int,"), "{generated}");
    assert!(
        warnings.contains(COLLECTION) && warnings.contains("\"id\"") && warnings.to_lowercase().contains("text"),
        "stderr: {warnings}"
    );
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

#[test]
fn adopt_existing_reads_correctly_after_a_real_drop_column_migration() {
    // Reporte real de skynet-43 (iaacademy, 29/08/2026): después de migrar
    // `id uuid` -> `id BIGSERIAL` a mano (ADD COLUMN id_seq + backfill +
    // RENAME de las dos columnas + ADD PRIMARY KEY) y por último DROP
    // COLUMN de la columna uuid vestigial, TODA query real contra la tabla
    // rompía con "error deserializing column N" -- `find`/`findWhere` por
    // igual, siempre, en la MISMA posición numérica sin importar el orden
    // de los campos en el `.link`. Hipótesis del reporte (sin confirmar
    // contra el código): un DROP COLUMN deja un hueco permanente en
    // `pg_attribute.attnum` (`attisdropped=true`, attnum nunca se
    // renumera) que en algún punto de `--adopt-existing` desalinearía
    // lecturas posteriores.
    //
    // Este test reproduce la MISMA secuencia real de migración (no solo
    // "una tabla con una columna de más", que ya cubre el test de arriba)
    // contra una tabla real, y confirma que `--adopt-existing` sigue
    // pudiendo leer/escribir después.
    const COLLECTION: &str = "leads_post_drop_column";
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
                \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(), \
                \"email\" TEXT NOT NULL, \
                \"status\" TEXT NOT NULL, \
                \"score\" BIGINT NOT NULL\
            )"
        ))
        .expect("crear la tabla legacy con id uuid, como la real de iaacademy antes de migrar");
    client
        .execute(
            &format!("INSERT INTO \"{COLLECTION}\" (email, status, score) VALUES ($1, $2, $3)"),
            &[&"a@example.com", &"new", &7i64],
        )
        .expect("sembrar una fila con la PK uuid original");

    // La MISMA secuencia que describe el reporte, paso a paso.
    client
        .batch_execute(&format!(
            "ALTER TABLE \"{COLLECTION}\" ADD COLUMN \"id_seq\" BIGSERIAL; \
             UPDATE \"{COLLECTION}\" SET \"id_seq\" = DEFAULT; \
             ALTER TABLE \"{COLLECTION}\" DROP CONSTRAINT \"{COLLECTION}_pkey\"; \
             ALTER TABLE \"{COLLECTION}\" RENAME COLUMN \"id\" TO \"id_uuid_legacy\"; \
             ALTER TABLE \"{COLLECTION}\" RENAME COLUMN \"id_seq\" TO \"id\"; \
             ALTER TABLE \"{COLLECTION}\" ADD PRIMARY KEY (\"id\"); \
             ALTER TABLE \"{COLLECTION}\" DROP COLUMN \"id_uuid_legacy\""
        ))
        .expect("correr la migracion real completa -- ADD/backfill/RENAME/ADD PK/DROP COLUMN");

    // Sembrar una SEGUNDA fila después de la migración, con la nueva PK
    // entera -- confirma que la tabla post-migración es usable en SQL
    // crudo antes de meter a c-script en la ecuación.
    let second_id: i64 = client
        .query_one(&format!("INSERT INTO \"{COLLECTION}\" (email, status, score) VALUES ($1, $2, $3) RETURNING id"), &[
            &"b@example.com",
            &"contacted",
            &9i64,
        ])
        .map(|row| row.get(0))
        .expect("insertar una segunda fila con SQL crudo tras la migración");

    let temp = TempDir::new("adopt-drop-column");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Lead = {{ id: Int, email: String, status: String, score: Int }}
type NewLead = {{ email: String, status: String, score: Int }}
db {{ {COLLECTION}: Lead[] }}
service Leads {{
  rpc list() -> Lead[] {{ db.{COLLECTION}.all() }}
  rpc get(id: Int) -> Lead? {{ db.{COLLECTION}.find(id) }}
  rpc create(email: String, status: String, score: Int) -> Lead {{ db.{COLLECTION}.insert(NewLead {{ email: email, status: status, score: score }}) }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);

    let listed = server.rpc("Leads/list", "{}");
    let rows = listed.as_array().unwrap_or_else(|| panic!("se esperaba una lista, llegó: {listed:?}"));
    assert_eq!(rows.len(), 2, "las dos filas (antes y después de migrar) tienen que leerse limpio: {listed:?}");

    let fetched = server.rpc("Leads/get", &format!(r#"{{"id":{second_id}}}"#));
    assert_eq!(fetched["email"], "b@example.com", "find por id tiene que funcionar después del DROP COLUMN: {fetched:?}");

    let created = server.rpc("Leads/create", r#"{"email":"c@example.com","status":"new","score":1}"#);
    assert_eq!(created["email"], "c@example.com", "insert también tiene que funcionar después del DROP COLUMN: {created:?}");
}

// ---- `String` contra `inet`/`uuid` NATIVOS de Postgres (GRAMMAR.md §3.179) ----

#[test]
fn adopt_existing_reads_and_writes_a_native_inet_column_mapped_to_string() {
    // La causa REAL del reporte de skynet-43 (iaacademy) -- no el DROP
    // COLUMN (ver test de arriba, que no reprodujo nada): una columna
    // `inet` NATIVA (`source_ip`, típica en una tabla de captación de
    // leads) mapeada a `String?` en el `.link`, tal como `linkc
    // introspect` ya recomienda ("revisado como String a mano") -- el
    // wire binario de `inet` no es texto UTF-8, así que leerla como
    // `String` rompía con un error de decodificación en runtime, aunque
    // compilara limpio.
    const COLLECTION: &str = "leads_inet_column";
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
                \"email\" TEXT NOT NULL, \
                \"source_ip\" INET, \
                \"user_agent\" TEXT\
            )"
        ))
        .expect("crear la tabla con una columna inet nativa, como la real de iaacademy");
    // El literal `inet` va EMBEBIDO en el SQL, no bindeado como parámetro:
    // el cliente `postgres` (crudo, sin el `Cell` propio de c-script que
    // esta ronda arregla) tampoco sabe bindear un `&str` contra una
    // columna `inet` -- ni siquiera con un cast explícito en el SQL, el
    // servidor sigue infiriendo el tipo del parámetro de la columna
    // destino (mismo motivo, documentado en GRAMMAR.md §3.177/§3.178,
    // por el que el propio `Cell::to_sql` de c-script necesitó un
    // decodificador binario a mano en vez de un cast). Un literal SQL
    // (`'203.0.113.7'`) no pasa por el bind de parámetros en absoluto --
    // Postgres lo parsea con su propio `inet_in()` de siempre, sin
    // involucrar al driver. Seguro acá: son constantes fijas del test,
    // nunca input externo.
    client
        .batch_execute(&format!(
            "INSERT INTO \"{COLLECTION}\" (email, source_ip, user_agent) VALUES ('a@example.com', '203.0.113.7', 'Mozilla/5.0')"
        ))
        .expect("sembrar una fila con source_ip real");
    client
        .batch_execute(&format!(
            "INSERT INTO \"{COLLECTION}\" (email, source_ip, user_agent) VALUES ('b@example.com', NULL, 'curl/8.0')"
        ))
        .expect("sembrar una fila con source_ip NULL -- el caso 'sin IP registrada'");

    let temp = TempDir::new("adopt-inet-column");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Lead = {{ id: Int, email: String, source_ip: String?, user_agent: String? }}
type NewLead = {{ email: String, source_ip: String?, user_agent: String? }}
db {{ {COLLECTION}: Lead[] }}
service Leads {{
  rpc list() -> Lead[] {{ db.{COLLECTION}.all() }}
  rpc create(email: String, source_ip: String?) -> Lead {{ db.{COLLECTION}.insert(NewLead {{ email: email, source_ip: source_ip, user_agent: null }}) }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Leads/list", "{}");
    let rows = listed.as_array().unwrap_or_else(|| panic!("se esperaba una lista, llegó: {listed:?}"));
    assert_eq!(rows.len(), 2, "{listed:?}");
    let with_ip = rows.iter().find(|r| r["email"] == "a@example.com").expect("la fila con IP");
    assert_eq!(with_ip["source_ip"], "203.0.113.7", "la IP tiene que decodificarse a su forma de texto real: {with_ip:?}");
    let without_ip = rows.iter().find(|r| r["email"] == "b@example.com").expect("la fila sin IP");
    assert_eq!(without_ip["source_ip"], serde_json::Value::Null, "NULL en inet sigue siendo NULL: {without_ip:?}");

    // Escritura: c-script tiene que poder ESCRIBIR un valor nuevo contra
    // la misma columna inet nativa, no solo leerla.
    let created = server.rpc("Leads/create", r#"{"email":"c@example.com","source_ip":"198.51.100.42"}"#);
    assert_eq!(created["source_ip"], "198.51.100.42", "{created:?}");
    // Confirma con SQL crudo que quedó guardada como inet real, no como texto.
    let raw_type: String = client
        .query_one("SELECT pg_typeof(source_ip)::text FROM \"leads_inet_column\" WHERE email = 'c@example.com'", &[])
        .map(|row| row.get(0))
        .expect("leer el tipo real de la columna con SQL crudo");
    assert_eq!(raw_type, "inet", "el insert tiene que haber escrito un valor inet real, no forzado el tipo de la columna");
}

// ---- `String` contra `json`/`jsonb` NATIVOS de Postgres (GRAMMAR.md §3.187) ----

/// Bug real de producción, confirmado en vivo (iaacademy, vía skynet-43,
/// 30/08/2026): una columna `jsonb` NATIVA adoptada, mapeada a `String?`
/// (la forma que GRAMMAR.md ya recomienda para JSON sin tipo propio
/// declarado), fallaba SIEMPRE al escribir -- "error deserializing column
/// N", la fila nunca se insertaba, CON o SIN valor (`null` fallaba
/// igual). ~2-3 min de 500 reales en un endpoint público de analíticas
/// antes de revertir a SQL crudo.
#[test]
fn adopt_existing_reads_and_writes_a_native_jsonb_column_mapped_to_string() {
    const COLLECTION: &str = "events_jsonb_column";
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
                \"kind\" TEXT NOT NULL, \
                \"properties\" JSONB\
            )"
        ))
        .expect("crear la tabla con una columna jsonb nativa, como la real de iaacademy");
    client
        .batch_execute(&format!(
            "INSERT INTO \"{COLLECTION}\" (kind, properties) VALUES ('click', '{{\"button\":\"cta\",\"n\":2}}'::jsonb)"
        ))
        .expect("sembrar una fila con properties real");
    client
        .batch_execute(&format!("INSERT INTO \"{COLLECTION}\" (kind, properties) VALUES ('pageview', NULL)"))
        .expect("sembrar una fila con properties NULL");

    let temp = TempDir::new("adopt-jsonb-column");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Event = {{ id: Int, kind: String, properties: String? }}
type NewEvent = {{ kind: String, properties: String? }}
db {{ {COLLECTION}: Event[] }}
service Events {{
  rpc list() -> Event[] {{ db.{COLLECTION}.all() }}
  rpc create(kind: String, properties: String?) -> Event {{ db.{COLLECTION}.insert(NewEvent {{ kind: kind, properties: properties }}) }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Events/list", "{}");
    let rows = listed.as_array().unwrap_or_else(|| panic!("se esperaba una lista, llegó: {listed:?}"));
    assert_eq!(rows.len(), 2, "{listed:?}");
    // Comparación SEMÁNTICA, no de texto exacto: `jsonb` (a diferencia de
    // `json`, ver el test hermano abajo) NO preserva el texto de entrada
    // tal cual -- reordena claves y normaliza espacios al reserializar
    // desde su árbol binario interno (comportamiento real y documentado
    // de Postgres, confirmado en CI: `{"button":"cta","n":2}` volvió como
    // `{"n": 2, "button": "cta"}`, mismo VALOR, texto distinto). El
    // contrato real de un roundtrip jsonb es equivalencia de VALOR, nunca
    // byte a byte.
    let click = rows.iter().find(|r| r["kind"] == "click").expect("la fila con properties");
    let click_props: serde_json::Value =
        serde_json::from_str(click["properties"].as_str().expect("properties es un string")).expect("properties es JSON válido");
    assert_eq!(click_props, serde_json::json!({"button": "cta", "n": 2}), "el jsonb tiene que decodificar al mismo VALOR JSON: {click:?}");
    let pageview = rows.iter().find(|r| r["kind"] == "pageview").expect("la fila sin properties");
    assert_eq!(pageview["properties"], serde_json::Value::Null, "NULL en jsonb sigue siendo NULL: {pageview:?}");

    // Escritura: el repro exacto de skynet-43 -- con contenido Y con null,
    // los dos tienen que funcionar (antes del fix, los dos fallaban igual).
    let created = server.rpc("Events/create", r#"{"kind":"purchase","properties":"{\"amount\":19.99}"}"#);
    let created_props: serde_json::Value =
        serde_json::from_str(created["properties"].as_str().expect("properties es un string")).expect("properties es JSON válido");
    assert_eq!(created_props, serde_json::json!({"amount": 19.99}), "{created:?}");
    let created_null = server.rpc("Events/create", r#"{"kind":"logout","properties":null}"#);
    assert_eq!(created_null["properties"], serde_json::Value::Null, "{created_null:?}");

    // Confirma con SQL crudo que quedó guardada como jsonb real, consultable.
    let raw: String = client
        .query_one("SELECT properties->>'amount' FROM \"events_jsonb_column\" WHERE kind = 'purchase'", &[])
        .map(|row| row.get(0))
        .expect("leer con un operador jsonb real -- falla si no se guardó como jsonb de verdad");
    assert_eq!(raw, "19.99", "el insert tiene que haber escrito un jsonb real, consultable con ->>: {raw}");
}

/// Mismo bug, la otra mitad: una columna `json` (no `jsonb`) -- formato
/// binario DISTINTO (texto UTF-8 crudo, sin el byte de versión que
/// `jsonb` antepone) -- confirma que el fix distingue los dos casos
/// correctamente, no solo uno de los dos.
#[test]
fn adopt_existing_reads_and_writes_a_native_json_column_mapped_to_string() {
    const COLLECTION: &str = "events_json_column";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!("CREATE TABLE \"{COLLECTION}\" (\"id\" BIGSERIAL PRIMARY KEY, \"payload\" JSON)"))
        .expect("crear la tabla con una columna json (no jsonb) nativa");
    client
        .batch_execute(&format!("INSERT INTO \"{COLLECTION}\" (payload) VALUES ('{{\"a\":1}}'::json)"))
        .expect("sembrar una fila con payload real");

    let temp = TempDir::new("adopt-json-column");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Event = {{ id: Int, payload: String? }}
type NewEvent = {{ payload: String? }}
db {{ {COLLECTION}: Event[] }}
service Events {{
  rpc list() -> Event[] {{ db.{COLLECTION}.all() }}
  rpc create(payload: String?) -> Event {{ db.{COLLECTION}.insert(NewEvent {{ payload: payload }}) }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Events/list", "{}");
    let rows = listed.as_array().unwrap_or_else(|| panic!("se esperaba una lista, llegó: {listed:?}"));
    assert_eq!(rows[0]["payload"], r#"{"a":1}"#, "{rows:?}");

    let created = server.rpc("Events/create", r#"{"payload":"{\"b\":2}"}"#);
    assert_eq!(created["payload"], r#"{"b":2}"#, "{created:?}");
    let raw_type: String = client
        .query_one("SELECT pg_typeof(payload)::text FROM \"events_json_column\" WHERE payload::text = '{\"b\":2}'", &[])
        .map(|row| row.get(0))
        .expect("leer el tipo real con SQL crudo");
    assert_eq!(raw_type, "json", "el insert tiene que haber escrito un valor json real: {raw_type}");
}

#[test]
fn adopt_existing_reads_and_writes_a_native_uuid_column_mapped_to_plain_string() {
    // La SEGUNDA mitad del mismo reporte: un campo declarado `String` (NO
    // `Uuid`, GRAMMAR.md §3.70/§3.177) mapeado contra una columna Postgres
    // NATIVA `uuid` -- el caso real de `posts`/`seo_pages` de iaacademy,
    // que conservaron una columna `uuid` legada como `String` en vez de
    // `Uuid` en su `.link`. Mismo problema de fondo que el `inet` de
    // arriba, mismo arreglo (`postgres_string_cell` prueba `PgUuidText`
    // antes de `PgInetText`).
    const COLLECTION: &str = "posts_uuid_string_column";
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
                \"title\" TEXT NOT NULL, \
                \"legacy_uuid\" UUID NOT NULL DEFAULT gen_random_uuid()\
            )"
        ))
        .expect("crear la tabla con una columna uuid nativa legada");
    client
        .execute(&format!("INSERT INTO \"{COLLECTION}\" (title) VALUES ($1)"), &[&"primer post"])
        .expect("sembrar una fila -- legacy_uuid se autogenera vía DEFAULT");

    let temp = TempDir::new("adopt-uuid-as-string");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Post = {{ id: Int, title: String, legacy_uuid: String }}
db {{ {COLLECTION}: Post[] }}
service Posts {{ rpc list() -> Post[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Posts/list", "{}");
    let rows = listed.as_array().unwrap_or_else(|| panic!("se esperaba una lista, llegó: {listed:?}"));
    assert_eq!(rows.len(), 1, "{listed:?}");
    let legacy_uuid = rows[0]["legacy_uuid"].as_str().unwrap_or_else(|| panic!("legacy_uuid tiene que ser un string: {listed:?}"));
    assert_eq!(legacy_uuid.len(), 36, "tiene que decodificar a la forma canónica de un uuid real: {legacy_uuid}");
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
type Factura = {{ id: Int, fecha_emision: Timestamp, created_at: Timestamp, updated_at: Timestamp }}
db {{ {COLLECTION}: Factura[] }}
service Facturas {{ rpc list() -> Factura[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Facturas/list", "{}");
    let rows = listed.as_array().expect("se esperaba una lista");
    assert_eq!(rows.len(), 1, "body: {listed:?}");
    assert_eq!(rows[0]["fecha_emision"], "2026-08-24T00:00:00.000Z", "date nativo: {rows:?}");
    assert_eq!(rows[0]["created_at"], "2026-08-24T14:30:00.000Z", "timestamptz nativo: {rows:?}");
    assert_eq!(rows[0]["updated_at"], "2026-08-24T14:30:00.000Z", "timestamp (sin tz) nativo: {rows:?}");
}

// GRAMMAR.md §3.182: la mitad de ESCRITURA del mismo problema que el test de
// arriba (solo lectura) -- bug real reportado por skynet-43/iaacademy en
// producción: `Cell::to_sql` no tenía ningún caso para una columna
// `timestamp`/`timestamptz`/`date` NATIVA, así que un `Cell::Int(millis)`
// caía al `i64::to_sql` genérico -- el MISMO ancho de 8 bytes que un
// `BIGINT` normal, así que Postgres lo aceptaba sin quejarse, pero
// interpretándolo como microsegundos-desde-2000 en vez de
// milisegundos-desde-1970: la fecha quedaba corrompida en SILENCIO (nunca
// un error), resolviendo siempre a enero del año 2000.
#[test]
fn a_timestamp_field_writes_correctly_against_a_native_postgres_date_and_timestamptz_column() {
    const COLLECTION: &str = "facturas_fecha_nativa_escritura";
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

    let temp = TempDir::new("native-timestamp-write");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Factura = {{ id: Int, fecha_emision: Timestamp, created_at: Timestamp, updated_at: Timestamp }}
type NewFactura = {{ fecha_emision: Timestamp, created_at: Timestamp, updated_at: Timestamp }}
db {{ {COLLECTION}: Factura[] }}
service Facturas {{
  rpc create(fecha_emision: Timestamp, created_at: Timestamp, updated_at: Timestamp) -> Factura {{
    db.{COLLECTION}.insert(NewFactura {{ fecha_emision: fecha_emision, created_at: created_at, updated_at: updated_at }})
  }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let created = server.rpc(
        "Facturas/create",
        r#"{"fecha_emision":"2026-08-24T00:00:00.000Z","created_at":"2026-08-24T14:30:00.000Z","updated_at":"2026-08-24T14:30:00.000Z"}"#,
    );
    // Primero, el propio c-script tiene que devolver de vuelta lo que
    // acaba de escribir -- necesario pero NO suficiente (si escritura y
    // lectura compartieran el mismo bug simétrico, esto pasaría igual con
    // la fecha corrompida en los dos sentidos, ver el chequeo con SQL
    // crudo más abajo, que es el que de verdad prueba algo).
    assert_eq!(created["fecha_emision"], "2026-08-24T00:00:00.000Z", "{created:?}");
    assert_eq!(created["created_at"], "2026-08-24T14:30:00.000Z", "{created:?}");
    assert_eq!(created["updated_at"], "2026-08-24T14:30:00.000Z", "{created:?}");

    // La prueba real: leer los bytes guardados con el cliente `postgres`
    // CRUDO, sin pasar por el `Cell`/decodificador propio de c-script en
    // absoluto -- si el bug estuviera presente, esto mostraría el año 2000,
    // no 2026, confirmando que la corrupción es real y no un artefacto de
    // que lectura y escritura compartan el mismo error compensándose entre sí.
    let row = client
        .query_one(
            &format!("SELECT fecha_emision::text, created_at::text, updated_at::text FROM \"{COLLECTION}\" WHERE id = 1"),
            &[],
        )
        .expect("leer la fila con SQL crudo");
    let fecha_emision: String = row.get(0);
    let created_at: String = row.get(1);
    let updated_at: String = row.get(2);
    assert_eq!(fecha_emision, "2026-08-24", "SQL crudo -- date: {fecha_emision}");
    assert!(created_at.starts_with("2026-08-24"), "SQL crudo -- timestamptz: {created_at}");
    assert!(updated_at.starts_with("2026-08-24"), "SQL crudo -- timestamp: {updated_at}");
}

// GRAMMAR.md §3.103: un campo `Float` decodifica correctamente contra una
// columna `numeric`/`decimal` NATIVA de Postgres, no solo contra
// `float4`/`float8` -- segundo bug real encontrado por MyFinance verificando
// EN SU PROPIO ESQUEMA el fix de fechas de §3.91: "Float no decodifica
// columnas numeric de Postgres -- error deserializing column 1". Cierto:
// `numeric` es un formato binario de precisión arbitraria, TOTALMENTE
// distinto de IEEE754 (`postgres-types` no implementa `FromSql<f64>` para
// él) -- y es justo el tipo que casi cualquier columna de DINERO real usa
// (nunca `float8`, por el error de redondeo binario que `numeric` evita).
#[test]
fn a_float_field_decodes_a_native_postgres_numeric_column() {
    const COLLECTION: &str = "facturas_numeric_nativo";
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
                \"subtotal\" numeric(12,2) NOT NULL, \
                \"descuento\" numeric(12,2) NOT NULL, \
                \"total\" numeric NOT NULL\
            )"
        ))
        .expect("crear la tabla legacy a mano, con columnas numeric NATIVAS de Postgres");
    // Sembrada con SQL crudo -- exactamente como llegan los datos reales de
    // un sistema de facturación/contabilidad ya en producción. Incluye un
    // valor negativo (descuento) y un entero exacto (total), no solo el caso
    // fácil de un positivo con decimales.
    client
        .execute(
            &format!(
                "INSERT INTO \"{COLLECTION}\" (subtotal, descuento, total) VALUES \
                 (1234.56, -78.90, 1000)"
            ),
            &[],
        )
        .expect("sembrar una fila con SQL crudo, columnas numeric nativas");

    let temp = TempDir::new("native-numeric");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Factura = {{ id: Int, subtotal: Float, descuento: Float, total: Float }}
db {{ {COLLECTION}: Factura[] }}
service Facturas {{ rpc list() -> Factura[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Facturas/list", "{}");
    let rows = listed.as_array().expect("se esperaba una lista");
    assert_eq!(rows.len(), 1, "body: {listed:?}");
    assert_eq!(rows[0]["subtotal"], serde_json::json!(1234.56), "numeric positivo con decimales: {rows:?}");
    assert_eq!(rows[0]["descuento"], serde_json::json!(-78.9), "numeric NEGATIVO: {rows:?}");
    assert_eq!(rows[0]["total"], serde_json::json!(1000.0), "numeric entero exacto, sin escala declarada: {rows:?}");
}

// GRAMMAR.md §3.184: caso real que motiva `Decimal` -- MyFinance tiene
// columnas `numeric(12,2)` (`subtotal`, `descuento`, `total`, etc.) YA
// EXISTENTES en producción. Este test adopta una tabla así (no generada por
// c-script) y confirma lectura Y escritura exactas -- la escritura se
// verifica con SQL crudo (`::text`), no con el propio decodificador de
// c-script, porque el punto es que la fila FÍSICA cambie bien, no solo que
// el programa "crea" que cambió.
#[test]
fn adopt_existing_reads_and_writes_a_native_postgres_numeric_column_as_decimal() {
    const COLLECTION: &str = "facturas_decimal_nativo";
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
                \"subtotal\" numeric(12,2) NOT NULL\
            )"
        ))
        .expect("crear la tabla legacy a mano, columna numeric(12,2) NATIVA -- el caso real de MyFinance");
    client
        .execute(&format!("INSERT INTO \"{COLLECTION}\" (subtotal) VALUES (1234.56), (-78.90)"), &[])
        .expect("sembrar filas con SQL crudo");

    let temp = TempDir::new("native-numeric-decimal");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Factura = {{ id: Int, subtotal: Decimal }}
db {{ {COLLECTION}: Factura[] }}
service Facturas {{
  rpc list() -> Factura[] {{ db.{COLLECTION}.all() }}
  rpc reprice(id: Int, p: Patch<Factura>) -> Factura {{ db.{COLLECTION}.applyPatch(id, p) }}
}}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Facturas/list", "{}");
    let rows = listed.as_array().expect("se esperaba una lista");
    assert_eq!(rows.len(), 2, "body: {listed:?}");
    let by_id: std::collections::HashMap<i64, String> =
        rows.iter().map(|r| (r["id"].as_i64().unwrap(), r["subtotal"].as_str().unwrap().to_string())).collect();
    assert_eq!(by_id.get(&1).map(String::as_str), Some("1234.5600"), "escalado a 4 decimales: {by_id:?}");
    assert_eq!(by_id.get(&2).map(String::as_str), Some("-78.9000"), "negativo: {by_id:?}");

    server.rpc("Facturas/reprice", r#"{"id":1,"p":{"subtotal":"999.9900"}}"#);
    let raw: String = client
        .query_one(&format!("SELECT subtotal::text FROM \"{COLLECTION}\" WHERE id = 1"), &[])
        .expect("leer con SQL crudo")
        .get(0);
    assert_eq!(raw, "999.99", "el valor físico en la columna numeric(12,2) adoptada, confirmado sin pasar por c-script: {raw}");
}

// GRAMMAR.md §3.184: la otra mitad -- una columna GENERADA por c-script
// mismo (no adoptada) tiene que salir como `NUMERIC(38,4)` real en el DDL,
// y el ciclo completo (create/find/multiplicación/sumBy/maxBy/minBy) tiene
// que dar el mismo resultado exacto que el test equivalente contra SQLite
// real (`runtime/mod.rs`).
#[test]
fn a_decimal_field_supports_the_full_crud_cycle_and_aggregation_against_a_freshly_generated_postgres_column() {
    const COLLECTION: &str = "line_items_decimal_generated";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("decimal-generated");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type LineItem = {{ id: Int, sku: String, unitPrice: Decimal, qty: Int }}
type NewLineItem = {{ sku: String, unitPrice: Decimal, qty: Int }}
db {{ {COLLECTION}: LineItem[] }}
service Items {{
  rpc create(sku: String, unitPrice: Decimal, qty: Int) -> LineItem {{
    db.{COLLECTION}.insert(NewLineItem {{ sku: sku, unitPrice: unitPrice, qty: qty }})
  }}
  rpc get(id: Int) -> LineItem? {{ db.{COLLECTION}.find(id) }}
  rpc lineTotal(id: Int) -> Decimal? {{
    match db.{COLLECTION}.find(id) {{
      item: LineItem => item.unitPrice * item.qty.toDecimal(),
      null => null,
    }}
  }}
  rpc totalBySku() -> {{key: String, value: Decimal}}[] {{ db.{COLLECTION}.sumBy(|i: LineItem| {{ i.sku }}, |i: LineItem| {{ i.unitPrice }}) }}
  rpc priciest() -> LineItem? {{ db.{COLLECTION}.maxRow(|i: LineItem| {{ i.unitPrice }}) }}
  rpc cheapest() -> LineItem? {{ db.{COLLECTION}.minRow(|i: LineItem| {{ i.unitPrice }}) }}
}}
"#
        ),
    );
    let server = Serve::start(&src, &url);

    let created = server.rpc("Items/create", r#"{"sku":"WIDGET","unitPrice":"19.9900","qty":3}"#);
    assert_eq!(created["unitPrice"], serde_json::json!("19.9900"), "{created}");
    let id = created["id"].as_i64().unwrap();

    let fetched = server.rpc("Items/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(fetched["unitPrice"], serde_json::json!("19.9900"), "round-trip exacto por Postgres real: {fetched}");

    let total = server.rpc("Items/lineTotal", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(total, serde_json::json!("59.9700"), "19.99 * 3 = 59.97 exacto: {total}");

    server.rpc("Items/create", r#"{"sku":"WIDGET","unitPrice":"0.0100","qty":1}"#);
    server.rpc("Items/create", r#"{"sku":"GADGET","unitPrice":"5.5000","qty":1}"#);

    let sums = server.rpc("Items/totalBySku", "{}");
    let mut by_key: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in sums.as_array().unwrap() {
        by_key.insert(row["key"].as_str().unwrap().to_string(), row["value"].as_str().unwrap().to_string());
    }
    assert_eq!(by_key.get("WIDGET").map(String::as_str), Some("20.0000"), "sumBy real contra Postgres: {by_key:?}");
    assert_eq!(by_key.get("GADGET").map(String::as_str), Some("5.5000"), "{by_key:?}");

    let priciest = server.rpc("Items/priciest", "{}");
    assert_eq!(priciest["sku"], serde_json::json!("WIDGET"), "maxRow real contra Postgres: {priciest}");
    assert_eq!(priciest["unitPrice"], serde_json::json!("19.9900"), "{priciest}");

    let cheapest = server.rpc("Items/cheapest", "{}");
    assert_eq!(cheapest["unitPrice"], serde_json::json!("0.0100"), "minRow real contra Postgres: {cheapest}");

    // Confirmar el DDL generado: `NUMERIC(38,4)` real, no un genérico.
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let row = client
        .query_one(
            "SELECT numeric_precision, numeric_scale FROM information_schema.columns \
             WHERE table_name = $1 AND column_name = 'unitPrice'",
            &[&COLLECTION],
        )
        .expect("leer information_schema");
    let precision: i32 = row.get(0);
    let scale: i32 = row.get(1);
    assert_eq!((precision, scale), (38, 4), "DDL generado tiene que ser NUMERIC(38,4) real");
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

#[test]
fn connecting_to_a_preexisting_table_with_zero_overlapping_columns_warns_but_still_connects() {
    // GRAMMAR.md §3.94: el caso real que lo motivó -- `telemetry.link`
    // estuvo a punto de chocar contra una tabla `events` real de otro
    // servicio. Este test confirma DOS cosas a la vez: que la advertencia
    // aparece por stderr cuando el schema no tiene NINGÚN nombre de columna
    // en común con la tabla preexistente, y que --a propósito-- el connect
    // sigue funcionando igual que antes (nunca bloquea): un intento anterior
    // de esta misma feature devolvía un error duro acá, lo que rompía
    // `two_different_link_files_declaring_disjoint_columns_of_the_same_table_can_read_each_others_rows_but_not_always_write`
    // -- ese test prueba que columnas disjuntas entre dos `.link` sobre la
    // MISMA tabla es un patrón SOPORTADO a propósito, no un bug.
    const COLLECTION: &str = "items_collision_warning";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("collision-warning");
    let link_a = temp.write(
        "a.link",
        &format!(
            r#"
type Item = {{ id: Int, orderTotal: Float }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create(orderTotal: Float) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, orderTotal: orderTotal }}) }} }}
"#
        ),
    );
    let server_a = Serve::start(&link_a, &url);
    server_a.rpc("Items/create", r#"{"orderTotal":9.5}"#);
    drop(server_a);

    // "b.link" declara la MISMA colección, pero sin NINGÚN nombre de
    // columna en común con "a.link" -- exactamente el shape de una
    // colisión de nombre accidental entre dos programas no relacionados.
    // `sessionId` opcional (`String?`) a propósito: la fila que "a.link" ya
    // insertó no tiene esa columna, así que la migración no destructiva la
    // agrega NULL -- si el campo fuera requerido, `all()` fallaría con el
    // guard de "fila con NULL en un campo requerido" (§9.1.1), un chequeo
    // real y deliberado, pero DISTINTO de lo que este test verifica.
    let link_b = temp.write(
        "b.link",
        &format!(
            r#"
type Item = {{ id: Int, sessionId: String? }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );
    let server_b = Serve::start(&link_b, &url);
    // Sigue funcionando -- la advertencia nunca bloquea el connect ni
    // ningún rpc.
    let listed = server_b.rpc("Items/list", "{}");
    assert!(listed.is_array(), "{listed:?}");

    let stderr = server_b.stderr();
    assert!(stderr.to_lowercase().contains("advertencia"), "esperaba una advertencia por stderr: {stderr}");
    assert!(stderr.contains(COLLECTION), "la advertencia debe nombrar la colección: {stderr}");
}

#[test]
fn an_evolving_table_that_shares_at_least_one_column_does_not_warn() {
    // Contraparte del test anterior: agregar UNA columna nueva a una tabla
    // que el programa YA venía usando (mismo escenario que
    // `a_new_field_is_added_to_an_existing_table_without_losing_rows`) tiene
    // overlap real con lo que la tabla ya tenía -- no debe disparar la
    // advertencia de "esto parece de otro programa".
    const COLLECTION: &str = "items_no_collision_warning";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("no-collision-warning");
    let link_v1 = temp.write(
        "v1.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create(name: String) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name }}) }} }}
"#
        ),
    );
    let server_v1 = Serve::start(&link_v1, &url);
    server_v1.rpc("Items/create", r#"{"name":"algo"}"#);
    drop(server_v1);

    let link_v2 = temp.write(
        "v2.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, note: String? }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );
    let server_v2 = Serve::start(&link_v2, &url);
    let listed = server_v2.rpc("Items/list", "{}");
    assert!(listed.is_array(), "{listed:?}");
    assert!(!server_v2.stderr().to_lowercase().contains("advertencia"), "'name' se comparte -- no debería avisar nada: {}", server_v2.stderr());
}

/// GRAMMAR.md §3.94: el landmine real que este test cierra -- antes de
/// esta ronda, DOS programas sin ninguna relación entre sí, que solo
/// coinciden en seguir la convención de nombre `createdAt` (§3.68) que el
/// propio lenguaje promueve, se veían como "relacionados" (un nombre en
/// común alcanzaba para suprimir la advertencia de §3.94) -- exactamente el
/// tipo de colisión accidental que la advertencia existe para atrapar,
/// pasando desapercibida solo porque los dos siguieron la misma convención
/// de campo de auditoría, no porque compartieran la tabla a propósito.
#[test]
fn sharing_only_a_generic_audit_field_name_like_created_at_still_warns() {
    const COLLECTION: &str = "items_generic_field_collision";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("generic-field-collision");
    let link_a = temp.write(
        "a.link",
        &format!(
            r#"
type Item = {{ id: Int, orderTotal: Float, createdAt: Timestamp = now() }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create(orderTotal: Float) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, orderTotal: orderTotal, createdAt: now() }}) }} }}
"#
        ),
    );
    let server_a = Serve::start(&link_a, &url);
    server_a.rpc("Items/create", r#"{"orderTotal":9.5}"#);
    drop(server_a);

    // "b.link" declara la MISMA colección, con el ÚNICO nombre en común
    // siendo "createdAt" -- el escenario real: dos servicios sin relación
    // que ambos siguen la convención `createdAt: Timestamp = now()` del
    // lenguaje. Antes de esta ronda, ese solo nombre alcanzaba para NO
    // avisar; ahora los campos de auditoría (createdAt/updatedAt/
    // deletedAt) se ignoran como evidencia de relación real.
    let link_b = temp.write(
        "b.link",
        &format!(
            r#"
type Item = {{ id: Int, sessionId: String?, createdAt: Timestamp = now() }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );
    let server_b = Serve::start(&link_b, &url);
    let listed = server_b.rpc("Items/list", "{}");
    assert!(listed.is_array(), "{listed:?}");

    let stderr = server_b.stderr();
    assert!(stderr.to_lowercase().contains("advertencia"), "compartir solo 'createdAt' no debe suprimir la advertencia: {stderr}");
    assert!(stderr.contains(COLLECTION), "{stderr}");
}

/// GRAMMAR.md §3.94: caso de borde del fix de arriba -- si el struct
/// declarado NO tiene NINGÚN campo fuera de la lista de auditoría
/// genérica (createdAt/updatedAt/deletedAt), no hay ningún nombre
/// "significativo" para comparar. En ese caso el código cae de vuelta a
/// considerar los genéricos como evidencia (mejor una señal débil que
/// ninguna) -- mismo comportamiento que antes de esta ronda, sin
/// regresión, para este caso específico.
#[test]
fn when_every_declared_field_is_a_generic_audit_field_it_still_falls_back_to_comparing_them() {
    const COLLECTION: &str = "items_only_audit_fields";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("only-audit-fields");
    let link_v1 = temp.write(
        "v1.link",
        &format!(
            r#"
type Item = {{ id: Int, createdAt: Timestamp = now() }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create() -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, createdAt: now() }}) }} }}
"#
        ),
    );
    let server_v1 = Serve::start(&link_v1, &url);
    server_v1.rpc("Items/create", "{}");
    drop(server_v1);

    // "updatedAt" nuevo declarado como OPCIONAL a propósito -- la fila que
    // "v1.link" ya insertó no la tiene, así que la migración no destructiva
    // la agrega NULL; si fuera requerida, `all()` fallaría con el guard de
    // "fila con NULL en un campo requerido" (un chequeo real y deliberado,
    // pero distinto de lo que este test verifica -- mismo motivo por el
    // que el test de arriba usa `sessionId: String?`).
    let link_v2 = temp.write(
        "v2.link",
        &format!(
            r#"
type Item = {{ id: Int, createdAt: Timestamp = now(), updatedAt: Timestamp? }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );
    let server_v2 = Serve::start(&link_v2, &url);
    let listed = server_v2.rpc("Items/list", "{}");
    assert!(listed.is_array(), "{listed:?}");
    assert!(
        !server_v2.stderr().to_lowercase().contains("advertencia"),
        "sin ningún campo significativo declarado, debe caer de vuelta a comparar los genéricos: {}",
        server_v2.stderr()
    );
}

/// GRAMMAR.md §3.96: `@check` crea una restricción `CHECK` real en
/// PostgreSQL, no solo del lado de la aplicación -- este test escribe SQL
/// crudo, sin pasar por `linkc serve`/`apply_field_validators` en absoluto,
/// exactamente el escenario ("otro programa inserta sin pasar por la
/// validación de la aplicación") que motivó el pedido.
#[test]
fn check_field_creates_a_real_postgres_check_constraint_that_rejects_raw_sql_too() {
    const COLLECTION: &str = "reviews_check_constraint";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("check-constraint");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Review = {{ id: Int, @check(range, 1, 5) rating: Int }}
db {{ {COLLECTION}: Review[] }}
service Reviews {{ rpc add(rating: Int) -> Review {{ db.{COLLECTION}.insert(Review {{ id: 0, rating: rating }}) }} }}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let ok = server.rpc("Reviews/add", r#"{"rating":3}"#);
    assert_eq!(ok["rating"], 3);
    drop(server);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let raw_insert = client.execute(&format!("INSERT INTO \"{COLLECTION}\" (rating) VALUES (99)"), &[]);
    let err = raw_insert.expect_err("un INSERT crudo que viola @check debe rechazarse a nivel de Postgres, sin pasar por Rust");
    // `postgres::Error`'s `Display` (`"{err}"`) SIEMPRE es el literal fijo
    // "db error" para cualquier error que vino del servidor (`Kind::Db` en
    // tokio-postgres, ver su propio `impl Display`) -- NUNCA incluye el
    // mensaje real, en ningún locale. El detalle real vive en
    // `.as_db_error()`. "check" aparece en el mensaje real sin importar el
    // idioma del servidor (queda como término técnico incluso en un
    // Postgres con `lc_messages` en español: "...viola la restricción
    // «check» «..._check»").
    let detail = err.as_db_error().map(|db| db.message().to_lowercase()).unwrap_or_default();
    assert!(detail.contains("check"), "detalle real del error: {detail:?} (err: {err})");
}

/// GRAMMAR.md §3.146: `@check(minLength, ...)` sobre `String` genera el
/// MISMO tipo de `CHECK` real en Postgres que `@check(range, ...)` sobre un
/// campo numérico -- este test es la contraparte de
/// `check_field_creates_a_real_postgres_check_constraint_that_rejects_raw_sql_too`
/// para la mitad nueva (texto, no número).
#[test]
fn check_min_length_creates_a_real_postgres_check_constraint_that_rejects_raw_sql_too() {
    const COLLECTION: &str = "posts_check_min_length_constraint";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("check-min-length-constraint");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Post = {{ id: Int, @check(minLength, 1) title: String }}
db {{ {COLLECTION}: Post[] }}
service Posts {{ rpc add(title: String) -> Post {{ db.{COLLECTION}.insert(Post {{ id: 0, title: title }}) }} }}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let ok = server.rpc("Posts/add", r#"{"title":"algo"}"#);
    assert_eq!(ok["title"], "algo");
    drop(server);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let raw_insert = client.execute(&format!("INSERT INTO \"{COLLECTION}\" (title) VALUES ('')"), &[]);
    let err = raw_insert.expect_err("un INSERT crudo con título vacío debe rechazarse a nivel de Postgres, sin pasar por Rust");
    let detail = err.as_db_error().map(|db| db.message().to_lowercase()).unwrap_or_default();
    assert!(detail.contains("check"), "detalle real del error: {detail:?} (err: {err})");
}

/// GRAMMAR.md §3.173: `@check(<expr>)` de nivel `type` -- una expresión
/// booleana comparando DOS campos entre sí, traducida a un `CHECK` real de
/// tabla en Postgres. Confirma tres cosas a la vez: el servidor real acepta
/// una fila válida, el servidor real rechaza una inválida con 400 (mismo
/// fix de clasificación por SQLSTATE que el resto de `@check`/`@unique`), y
/// -- lo que confirma que el CHECK vive de verdad en la base, no solo en la
/// aplicación -- un `INSERT` SQL crudo que viola el mismo constraint se
/// rechaza sin pasar por c-script en absoluto.
#[test]
fn a_type_level_check_constraint_is_enforced_for_real_against_postgres() {
    const COLLECTION: &str = "bookings_type_level_check";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("type-level-check-constraint");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
@check(endDay > startDay)
type Booking = {{ id: Int, startDay: Int, endDay: Int }}
db {{ {COLLECTION}: Booking[] }}
service Bookings {{
  rpc add(startDay: Int, endDay: Int) -> Booking {{
    db.{COLLECTION}.insert(Booking {{ id: 0, startDay: startDay, endDay: endDay }})
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    let ok = server.rpc("Bookings/add", r#"{"startDay":1,"endDay":5}"#);
    assert_eq!(ok["startDay"], 1);

    let failed = server.try_rpc("Bookings/add", r#"{"startDay":5,"endDay":1}"#);
    let msg = failed.expect_err("endDay <= startDay tiene que rechazarse");
    assert!(msg.contains("devolvió 400"), "el status real tiene que ser 400, no 500: {msg}");
    drop(server);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let raw_insert =
        client.execute(&format!("INSERT INTO \"{COLLECTION}\" (\"startDay\", \"endDay\") VALUES (9, 3)"), &[]);
    let err = raw_insert.expect_err("un INSERT crudo que viola @check(endDay > startDay) debe rechazarse a nivel de Postgres, sin pasar por Rust");
    let detail = err.as_db_error().map(|db| db.message().to_lowercase()).unwrap_or_default();
    assert!(detail.contains("check"), "detalle real del error: {detail:?} (err: {err})");
}

/// Bug real, encontrado verificando a mano `@unique` COMPUESTO (GRAMMAR.md
/// §3.155) contra Postgres real -- pero preexistente, afecta también al
/// `@unique` de un solo campo (§3.80) que ya estaba shippeado: una
/// violación de `@unique`/`@check` contra Postgres real daba **500**, no el
/// **400** que GRAMMAR.md documenta -- `postgres::Error::to_string()` para
/// un error del servidor es el literal fijo "db error" (ver los dos tests
/// de arriba), así que `is_unique_violation`/`is_check_violation`
/// (`runtime/db.rs`, buscan un substring en el mensaje) nunca matcheaban
/// nada real. Arreglado clasificando por SQLSTATE (`db_err.code()`,
/// `runtime/store.rs::describe_postgres_error`) -- el código NUNCA se
/// traduce, a diferencia del mensaje humano (este Postgres de test corre
/// en español: "llave duplicada viola restricción de unicidad...", no en
/// inglés -- confirmando de paso que el fix también es a prueba de locale,
/// no solo del bug de "db error" a secas).
#[test]
fn a_unique_violation_over_real_http_against_postgres_is_a_400_not_a_500() {
    const COLLECTION: &str = "products_unique_status_code";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("unique-status-code");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Product = {{ id: Int, profileId: Int, @unique slug: String }}
db {{ {COLLECTION}: Product[] }}
service Products {{
  rpc create(profileId: Int, slug: String) -> Product {{
    db.{COLLECTION}.insert(Product {{ id: 0, profileId: profileId, slug: slug }})
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Products/create", r#"{"profileId":1,"slug":"unique-slug"}"#);
    let failed = server.try_rpc("Products/create", r#"{"profileId":2,"slug":"unique-slug"}"#);
    let msg = failed.expect_err("un slug repetido debe rechazarse");
    assert!(msg.contains("devolvió 400"), "el status real tiene que ser 400, no 500: {msg}");
    assert!(msg.to_lowercase().contains("unique") || msg.contains("único"), "{msg}");
}

/// GRAMMAR.md §3.155: `@unique(campo1, campo2, ...)` a nivel de `type` --
/// un constraint COMPUESTO real contra Postgres. Confirma tres cosas a la
/// vez: el índice compuesto se crea de verdad, una violación real da 400
/// (mismo fix que el test de arriba), y -- lo que distingue "compuesto" de
/// "un solo campo" -- cambiar CUALQUIERA de los dos campos alcanza para
/// que la fila sea válida de nuevo.
#[test]
fn a_composite_unique_constraint_is_enforced_for_real_against_postgres() {
    const COLLECTION: &str = "products_composite_unique";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("composite-unique-postgres");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
@unique(profileId, slug)
type Product = {{ id: Int, profileId: Int, slug: String, name: String }}
db {{ {COLLECTION}: Product[] }}
service Products {{
  rpc create(profileId: Int, slug: String, name: String) -> Product {{
    db.{COLLECTION}.insert(Product {{ id: 0, profileId: profileId, slug: slug, name: name }})
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Products/create", r#"{"profileId":1,"slug":"foo","name":"A"}"#);

    // MISMO (profileId, slug): rechazado con 400 real.
    let failed = server.try_rpc("Products/create", r#"{"profileId":1,"slug":"foo","name":"B"}"#);
    let msg = failed.expect_err("el mismo (profileId, slug) debe rechazarse");
    assert!(msg.contains("devolvió 400"), "{msg}");

    // Distinto profileId, MISMO slug: el constraint es COMPUESTO, no dos
    // constraints de un solo campo -- esto tiene que aceptarse.
    let other_profile = server.rpc("Products/create", r#"{"profileId":2,"slug":"foo","name":"C"}"#);
    assert_eq!(other_profile["profileId"], serde_json::json!(2), "{other_profile:?}");

    // Mismo profileId, distinto slug: también válido por el mismo motivo.
    let other_slug = server.rpc("Products/create", r#"{"profileId":1,"slug":"bar","name":"D"}"#);
    assert_eq!(other_slug["slug"], serde_json::json!("bar"), "{other_slug:?}");
}

/// GRAMMAR.md §3.175: `linkc db inspect` contra un Postgres real -- filas
/// reales insertadas por un `linkc serve` real, `@softDelete` no filtrado
/// (mismo criterio que `db.tableStats()`), y una colección declarada que
/// nunca llegó a tener tabla física reportada como "no existe todavía".
///
/// Bug real en el DISEÑO de este mismo test, encontrado por CI (no en
/// desarrollo local, sin Postgres a mano en ese momento): la primera
/// versión declaraba las DOS colecciones en el MISMO `.link` que
/// `Serve::start` corría -- pero `linkc serve` crea la tabla física de
/// TODA colección declarada al conectar (`new_with_options`/
/// `connect_postgres_with_options`, GRAMMAR.md §3.17), sin importar si
/// algún `rpc` la usa. "Nunca la toca ningún rpc" NO es lo mismo que "la
/// tabla no existe" -- para lograr esto último de verdad hacen falta DOS
/// `.link` DISTINTOS contra la MISMA base: uno más chico que `Serve::start`
/// sirve de verdad (solo crea SU tabla), y uno más grande (con una
/// colección de más) que `linkc db inspect` -- que nunca ejecuta DDL --
/// usa para leer. Documentado acá para que nadie repita el mismo error de
/// diseño de test.
#[test]
fn db_inspect_reports_real_row_counts_against_postgres() {
    const COLLECTION: &str = "items_db_inspect";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    reset_schema(&url, "orders_db_inspect_never_created");

    let temp = TempDir::new("db-inspect");
    // Solo declara "items" -- lo único que `Serve::start` va a crear de
    // verdad al conectar.
    let served_link = temp.write(
        "served.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, @softDelete deletedAt: Timestamp? = null }}
db {{ {COLLECTION}: Item[] }}
service Items {{
  rpc add(name: String) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name }}) }}
  rpc remove(id: Int) -> Bool {{ db.{COLLECTION}.delete(id) }}
}}
"#
        ),
    );
    let server = Serve::start(&served_link, &url);
    server.rpc("Items/add", r#"{"name":"a"}"#);
    let created = server.rpc("Items/add", r#"{"name":"b"}"#);
    server.rpc("Items/remove", &format!(r#"{{"id":{}}}"#, created["id"]));
    drop(server);

    // Este SEGUNDO .link declara una colección de más -- `linkc db
    // inspect`, a diferencia de `linkc serve`, nunca ejecuta DDL, así que
    // inspeccionar CON este archivo contra la MISMA base no crea la tabla
    // que le falta; solo la reporta como ausente.
    let inspect_link = temp.write(
        "inspect.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, @softDelete deletedAt: Timestamp? = null }}
type NeverCreated = {{ id: Int, x: Int }}
db {{ {COLLECTION}: Item[], orders_db_inspect_never_created: NeverCreated[] }}
"#
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("inspect")
        .arg(&inspect_link)
        .env("LINK_DATABASE_URL", &url)
        .output()
        .expect("ejecutar linkc db inspect");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(COLLECTION) && stdout.contains("2 fila(s)"), "{stdout}");
    assert!(
        stdout.contains("orders_db_inspect_never_created") && stdout.contains("no existe todavía"),
        "una colección declarada pero nunca creada tiene que reportarse así, no como 0 filas: {stdout}"
    );
    assert!(stdout.contains("2 colección(es) declaradas, 1 sin crear todavía, 2 fila(s) en total"), "{stdout}");
}

// GRAMMAR.md §3.185: `linkc db export`/`linkc db import` -- siguiente pieza
// de la suite de administración de datos después de `db inspect`.

/// Round-trip completo contra Postgres real: exporta filas reales
/// (sembradas por un `linkc serve` real), resetea el esquema (simula un
/// target fresco reusando la misma base compartida de test) e importa de
/// vuelta -- confirma vía RPC real que el id y el Decimal sobreviven
/// exactos.
#[test]
fn db_export_and_import_round_trip_against_real_postgres() {
    const COLLECTION: &str = "items_export_import_pg";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("db-export-import-pg");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, price: Decimal }}
db {{ {COLLECTION}: Item[] }}
service Items {{
  rpc add(name: String, price: Decimal) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name, price: price }}) }}
  rpc get(id: Int) -> Item? {{ db.{COLLECTION}.find(id) }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Items/add", r#"{"name":"Widget","price":"19.9900"}"#);
    server.rpc("Items/add", r#"{"name":"Gadget","price":"5.5000"}"#);
    drop(server);

    let export_path = temp.write("export.json", "");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("export")
        .arg(&link)
        .arg(&export_path)
        .arg("--db")
        .arg(&url)
        .output()
        .expect("ejecutar linkc db export");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    // "Target fresco" -- reusa la misma base compartida de test, pero la
    // tabla se dropea y recrea antes de importar, simulando un entorno
    // nuevo sin necesitar una segunda base Postgres real.
    reset_schema(&url, COLLECTION);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("import")
        .arg(&link)
        .arg(&export_path)
        .arg("--db")
        .arg(&url)
        .output()
        .expect("ejecutar linkc db import");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&link, &url);
    let fetched = server.rpc("Items/get", r#"{"id":1}"#);
    assert_eq!(fetched["name"], serde_json::json!("Widget"), "id ORIGINAL preservado: {fetched}");
    assert_eq!(fetched["price"], serde_json::json!("19.9900"), "Decimal exacto tras el round-trip: {fetched}");
}

/// La propiedad de resync de secuencia SOLO se puede verificar contra una
/// secuencia Postgres real (`pg_get_serial_sequence`/`setval`) -- la
/// emulación `sqlite_sequence` de SQLite es un mecanismo DISTINTO,
/// verificado aparte contra SQLite real en `cli_db_import.rs`. Importa
/// filas con ids explícitos ALTOS, después crea una fila normal vía RPC
/// real (el camino de autoincremento real de `Db::call`) y confirma que
/// el id nuevo no choca con ninguno importado.
#[test]
fn db_import_resyncs_the_postgres_sequence_so_a_normal_insert_never_collides_with_an_imported_id() {
    const COLLECTION: &str = "items_import_resync_pg";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("db-import-resync-pg");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc add(name: String) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name }}) }} }}
"#
        ),
    );
    let export_path = temp.write(
        "export.json",
        &format!(r#"{{"linkc_version":"0","exported_at":"","collections":{{"{COLLECTION}":[{{"id":500,"name":"a"}},{{"id":510,"name":"b"}}]}}}}"#),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("import")
        .arg(&link)
        .arg(&export_path)
        .arg("--db")
        .arg(&url)
        .output()
        .expect("ejecutar linkc db import");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&link, &url);
    let created = server.rpc("Items/add", r#"{"name":"c"}"#);
    let new_id = created["id"].as_i64().expect("id");
    assert!(new_id > 510, "la secuencia resincronizada no debe chocar con ningún id importado: {created}");
}

/// Una colección con PK `Uuid` (GRAMMAR.md §3.177) no tiene ningún
/// concepto de secuencia -- confirma que el import funciona igual y que
/// `resync_id_sequence` se saltea sin error.
#[test]
fn db_import_a_uuid_pk_collection_against_real_postgres_needs_no_sequence_resync() {
    const COLLECTION: &str = "leads_import_uuid_pg";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("db-import-uuid-pg");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Lead = {{ id: Uuid, email: String }}
db {{ {COLLECTION}: Lead[] }}
service Leads {{ rpc get(id: Uuid) -> Lead? {{ db.{COLLECTION}.find(id) }} }}
"#
        ),
    );
    let export_path = temp.write(
        "export.json",
        &format!(
            r#"{{"linkc_version":"0","exported_at":"","collections":{{"{COLLECTION}":[{{"id":"6fe55062-2751-4ed0-b902-0820b15be183","email":"a@example.com"}}]}}}}"#
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("import")
        .arg(&link)
        .arg(&export_path)
        .arg("--db")
        .arg(&url)
        .output()
        .expect("ejecutar linkc db import");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&link, &url);
    let fetched = server.rpc("Leads/get", r#"{"id":"6fe55062-2751-4ed0-b902-0820b15be183"}"#);
    assert_eq!(fetched["email"], serde_json::json!("a@example.com"), "{fetched}");
}

/// GRAMMAR.md §3.174: `@unique(...) where <expr>` -- índice único compuesto
/// PARCIAL real contra Postgres. Caso motivador citado desde el schema
/// Drizzle de Glowapp: dos turnos con el mismo horario chocan SOLO si
/// ninguno está cancelado.
#[test]
fn a_conditional_composite_unique_constraint_is_enforced_for_real_against_postgres() {
    const COLLECTION: &str = "appointments_conditional_unique";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("conditional-composite-unique-postgres");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
@unique(userId, appointmentDate, startTime) where status != "cancelled"
type Appointment = {{ id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }}
db {{ {COLLECTION}: Appointment[] }}
service Appointments {{
  rpc book(userId: Int, appointmentDate: String, startTime: String, status: String) -> Appointment {{
    db.{COLLECTION}.insert(Appointment {{ id: 0, userId: userId, appointmentDate: appointmentDate, startTime: startTime, status: status }})
  }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Appointments/book", r#"{"userId":1,"appointmentDate":"2026-09-01","startTime":"10:00","status":"confirmed"}"#);

    // Mismo horario, todavía confirmado: rechazado con 400 real.
    let clash = server.try_rpc(
        "Appointments/book",
        r#"{"userId":1,"appointmentDate":"2026-09-01","startTime":"10:00","status":"confirmed"}"#,
    );
    let msg = clash.expect_err("el mismo horario, sin cancelar, debe rechazarse");
    assert!(msg.contains("devolvió 400"), "{msg}");

    // Mismo horario, pero cancelado: la fila existente queda AFUERA del
    // índice parcial -- reusar el horario tiene que aceptarse.
    let reused = server.rpc(
        "Appointments/book",
        r#"{"userId":1,"appointmentDate":"2026-09-01","startTime":"10:00","status":"cancelled"}"#,
    );
    assert_eq!(reused["status"], serde_json::json!("cancelled"), "{reused:?}");
}

/// GRAMMAR.md §3.149: `GET /metrics` sobre Postgres usa `pg_database_size`
/// (una función SQL distinta a la de SQLite, `PRAGMA page_count/page_size`)
/// -- este test es la contraparte real de
/// `metrics_reports_the_real_database_size_in_bytes` (`cli_metrics.rs`,
/// SQLite) para el otro backend.
#[test]
fn metrics_reports_a_real_database_size_on_postgres() {
    const COLLECTION: &str = "tasks_metrics_db_size";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("metrics-db-size");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Task = {{ id: Int, title: String }}
db {{ {COLLECTION}: Task[] }}
service Tasks {{ rpc add(title: String) -> Task {{ db.{COLLECTION}.insert(Task {{ id: 0, title: title }}) }} }}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Tasks/add", r#"{"title":"algo"}"#);

    let text = ureq::get(&format!("http://127.0.0.1:{}/metrics", server.port))
        .call()
        .unwrap_or_else(|e| panic!("GET /metrics falló: {e}"))
        .into_string()
        .expect("leer el body");
    let line = text.lines().find(|l| l.starts_with("linkc_db_size_bytes ")).unwrap_or_else(|| panic!("body: {text}"));
    let size: i64 = line.trim_start_matches("linkc_db_size_bytes ").trim().parse().expect("tamaño numérico");
    assert!(size > 0, "pg_database_size de una base real no puede ser 0: {size}");
}

/// GRAMMAR.md §3.151: `db.vacuum()` corre un `VACUUM` real contra Postgres
/// -- el riesgo real que este test cierra es que Postgres RECHAZA `VACUUM`
/// dentro de un bloque de transacción ("VACUUM cannot run inside a
/// transaction block"), así que hacía falta confirmar contra el motor de
/// verdad que el camino de ejecución (`batch_execute`, protocolo simple)
/// no envuelve el comando en una transacción implícita.
#[test]
fn db_vacuum_runs_for_real_against_postgres_without_a_transaction_block_error() {
    const COLLECTION: &str = "items_vacuum";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("db-vacuum");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
db {{ {COLLECTION}: Item[] }}
service Admin {{
  rpc add(name: String) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name }}) }}
  rpc doVacuum() -> Void {{ db.vacuum() }}
  rpc stats() -> Map<String, Int> {{ db.tableStats() }}
}}
"#
        ),
    );
    let server = Serve::start(&link, &url);
    server.rpc("Admin/add", r#"{"name":"x"}"#);
    let vacuum_result = server.rpc("Admin/doVacuum", "{}");
    assert_eq!(vacuum_result, serde_json::Value::Null, "body: {vacuum_result:?}");

    let stats = server.rpc("Admin/stats", "{}");
    assert_eq!(stats[COLLECTION], 1, "body: {stats:?}");
}

/// GRAMMAR.md §3.97: `linkc migrate --dry-run` sobre una colección cuya
/// tabla física NO EXISTE todavía tiene que mostrar el `CREATE TABLE`
/// exacto que `linkc serve` ejecutaría -- y, crucial, NO crear la tabla de
/// verdad (el punto entero de "dry-run").
#[test]
fn migrate_dry_run_shows_the_create_table_for_a_brand_new_collection_and_creates_nothing() {
    const COLLECTION: &str = "reviews_migrate_dry_run_new";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("migrate-dry-run-new");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Review = {{ id: Int, @check(range, 1, 5) rating: Int }}
db {{ {COLLECTION}: Review[] }}
service Reviews {{ rpc noop() -> Int {{ 1 }} }}
"#
        ),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("migrate")
        .arg(&link)
        .arg("--db")
        .arg(&url)
        .arg("--dry-run")
        .output()
        .expect("ejecutar linkc migrate");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("CREATE TABLE IF NOT EXISTS \"{COLLECTION}\"")), "{stdout}");
    assert!(stdout.contains("CHECK (\"rating\" >= 1 AND \"rating\" <= 5)"), "{stdout}");
    assert!(stdout.contains("tabla nueva"), "{stdout}");

    // El punto entero de "dry-run": nada de esto se aplicó de verdad.
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let exists = client
        .query_one("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)", &[&COLLECTION])
        .map(|row| row.get::<_, bool>(0))
        .unwrap();
    assert!(!exists, "'linkc migrate --dry-run' NO debe crear la tabla de verdad");
}

/// Como el test anterior, pero sobre una tabla que YA EXISTE y le falta una
/// columna -- el reporte tiene que mostrar el `ALTER TABLE ADD COLUMN`
/// exacto, y la columna NO debe existir de verdad después.
#[test]
fn migrate_dry_run_shows_the_alter_table_for_a_missing_column_and_adds_nothing() {
    const COLLECTION: &str = "reviews_migrate_dry_run_alter";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("migrate-dry-run-alter");
    let link_v1 = temp.write(
        "v1.link",
        &format!("type Review = {{ id: Int, rating: Int }}\ndb {{ {COLLECTION}: Review[] }}\nservice S {{ rpc noop() -> Int {{ 1 }} }}\n"),
    );
    // Crea la tabla de verdad con "rating" solamente, vía un connect real.
    let server = Serve::start(&link_v1, &url);
    drop(server);

    let link_v2 = temp.write(
        "v2.link",
        &format!(
            "type Review = {{ id: Int, rating: Int, comment: String? }}\ndb {{ {COLLECTION}: Review[] }}\nservice S {{ rpc noop() -> Int {{ 1 }} }}\n"
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("migrate")
        .arg(&link_v2)
        .arg("--db")
        .arg(&url)
        .arg("--dry-run")
        .output()
        .expect("ejecutar linkc migrate");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("ALTER TABLE \"{COLLECTION}\" ADD COLUMN IF NOT EXISTS \"comment\"")), "{stdout}");
    assert!(!stdout.contains("tabla nueva"), "la tabla ya existía, no es nueva: {stdout}");

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let exists = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = 'comment')",
            &[&COLLECTION],
        )
        .map(|row| row.get::<_, bool>(0))
        .unwrap();
    assert!(!exists, "'linkc migrate --dry-run' NO debe agregar la columna de verdad");
}

/// GRAMMAR.md §3.99: `linkc test --db <url-postgres>` corre los bloques
/// `test "..." { ... }` contra PostgreSQL real -- el caso real que lo
/// motiva es un bug de decodificación del wire binario de Postgres,
/// invisible corriendo contra SQLite `:memory:` (los dos backends emiten
/// SQL distinto para el mismo `.link`). Este test confirma que la fila que
/// un `test` insertó vía `db.<c>.insert` está de verdad en PostgreSQL
/// después -- no en un SQLite en memoria que se descartó al terminar.
#[test]
fn test_with_db_flag_runs_the_test_block_against_real_postgres() {
    const COLLECTION: &str = "reviews_test_against_postgres";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("test-against-postgres");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Review = {{ id: Int, rating: Int }}
db {{ {COLLECTION}: Review[] }}
test "insertar una reseña" {{
  let r = db.{COLLECTION}.insert(Review {{ id: 0, rating: 5 }});
  assert(r.rating == 5);
}}
"#
        ),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&link).arg("--db").arg(&url).output().expect("ejecutar linkc test");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed"), "{stdout}");

    // La fila tiene que existir de verdad en Postgres -- si esto corriera
    // contra SQLite :memory: (el bug que --db existe para evitar), esta
    // consulta directa a Postgres no encontraría nada.
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let count: i64 = client.query_one(&format!("SELECT COUNT(*) FROM \"{COLLECTION}\""), &[]).unwrap().get(0);
    assert_eq!(count, 1, "la fila insertada por el test tiene que existir de verdad en Postgres");
}

/// Límite honesto documentado en GRAMMAR.md §3.99: a diferencia de SQLite
/// `:memory:` (una conexión fresca y vacía por CADA test), `--db
/// <url-postgres>` comparte la MISMA conexión entre todos los tests de la
/// corrida -- sin aislamiento. Este test confirma ese comportamiento
/// explícitamente: lo que un test insertó, el SIGUIENTE test lo ve.
#[test]
fn test_with_db_flag_shares_state_across_tests_with_no_isolation() {
    const COLLECTION: &str = "reviews_test_shared_state";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("test-shared-state");
    let link = temp.write(
        "app.link",
        &format!(
            r#"
type Review = {{ id: Int, rating: Int }}
db {{ {COLLECTION}: Review[] }}
test "1 - insertar una reseña" {{
  db.{COLLECTION}.insert(Review {{ id: 0, rating: 5 }});
  assert(db.{COLLECTION}.count() == 1);
}}
test "2 - la reseña del test anterior sigue ahi" {{
  assert(db.{COLLECTION}.count() == 1);
}}
"#
        ),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&link).arg("--db").arg(&url).output().expect("ejecutar linkc test");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("2 passed"), "{stdout}");
}

/// GRAMMAR.md §3.100: `linkc doctor --db <url-postgres>` confirma
/// conectividad de solo lectura contra una base REAL -- este test cubre lo
/// que `cli_doctor.rs` no puede (esos tests solo prueban el camino de
/// error, con un puerto cerrado; nunca se conectan a un Postgres real). Doble
/// verificación: el chequeo reporta `[OK]`, Y la base sigue exactamente
/// igual después (ninguna tabla nueva) -- `doctor` nunca debe ejecutar DDL.
#[test]
fn doctor_reports_ok_connectivity_against_a_real_postgres_and_touches_no_schema() {
    const COLLECTION: &str = "reviews_doctor_connectivity";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("doctor-connectivity");
    let link = temp.write("app.link", &format!("type Review = {{ id: Int, rating: Int }}\ndb {{ {COLLECTION}: Review[] }}\n"));

    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("doctor").arg(&link).arg("--db").arg(&url).output().expect("ejecutar linkc doctor");
    assert!(out.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[OK]    conectividad a PostgreSQL"), "{stdout}");
    assert!(stdout.contains("0 error(es)"), "{stdout}");

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let exists = client
        .query_one("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)", &[&COLLECTION])
        .map(|row| row.get::<_, bool>(0))
        .unwrap();
    assert!(!exists, "'linkc doctor' NO debe crear ninguna tabla -- es solo diagnóstico de conectividad");
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

// ---- `linkc db shell` contra PostgreSQL real (GRAMMAR.md §3.189) ----
//
// `cli_db_shell.rs` ya prueba el subproceso real contra SQLite -- lo que
// SOLO Postgres puede probar es que `SET default_transaction_read_only = on`
// (`db_admin.rs::run_shell_postgres`) de verdad bloquea una escritura del
// lado del SERVIDOR (no un parser de palabras clave del cliente, que un
// `WITH ... AS (INSERT ...) SELECT ...` engañaría), y que columnas nativas
// no triviales (`numeric`/`uuid`/`jsonb`) salen legibles por
// `format_pg_cell`, no como el placeholder de tipo-no-soportado.

struct PgShellProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl PgShellProcess {
    fn start(link_path: &PathBuf, url: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("db")
            .arg("shell")
            .arg(link_path)
            .arg("--db")
            .arg(url)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("no se pudo iniciar 'linkc db shell' contra Postgres");
        let stdin = child.stdin.take().expect("stdin del proceso hijo");
        let stdout = BufReader::new(child.stdout.take().expect("stdout del proceso hijo"));
        PgShellProcess { child, stdin, stdout }
    }

    fn send(&mut self, sql: &str) {
        writeln!(self.stdin, "{sql}").expect("escribir la consulta al stdin del hijo");
        self.stdin.flush().expect("flush del stdin del hijo");
    }

    /// Mismo despegue del prompt (`"db> "`, sin salto de línea, pegado a la
    /// primera línea de cada respuesta) que `cli_db_shell.rs::ShellProcess::recv`.
    fn recv(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut first = true;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("leer una línea del stdout del hijo");
            assert_ne!(n, 0, "el proceso hijo cerró stdout antes de mandar '--fin--'");
            let mut content = line.trim_end_matches(['\r', '\n']).to_string();
            if first {
                content = content.strip_prefix("db> ").unwrap_or(&content).to_string();
                first = false;
            }
            if content == "--fin--" {
                return lines;
            }
            lines.push(content);
        }
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("esperar a que 'linkc db shell' termine");
        assert!(status.success(), "linkc db shell debería salir limpio (código 0) al ver EOF en stdin, salió con {status:?}");
    }
}

#[test]
fn db_shell_read_only_session_blocks_a_real_write_enforced_by_the_server_against_postgres() {
    const COLLECTION: &str = "items_db_shell_readonly";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut admin = postgres::Client::connect(&url, postgres::NoTls).expect("conectar como admin");
    admin
        .batch_execute(&format!("CREATE TABLE \"{COLLECTION}\" (\"id\" BIGSERIAL PRIMARY KEY, \"name\" TEXT NOT NULL)"))
        .expect("crear la tabla");
    admin.batch_execute(&format!("INSERT INTO \"{COLLECTION}\" (name) VALUES ('seed')")).expect("sembrar una fila");

    let temp = TempDir::new("shell-readonly-pg");
    let src = temp.write("app.link", &format!("type Item = {{ id: Int, name: String }}\ndb {{ {COLLECTION}: Item[] }}"));

    let mut shell = PgShellProcess::start(&src, &url);
    shell.send(&format!("INSERT INTO \"{COLLECTION}\" (name) VALUES ('hack')"));
    let joined = shell.recv().join("\n");
    assert!(joined.starts_with("error:"), "una escritura debe reportarse como error, no como resultado: {joined:?}");
    assert!(
        joined.to_lowercase().contains("read-only transaction"),
        "el mensaje tiene que venir del SERVIDOR (default_transaction_read_only), no de un parser local de palabras clave: {joined:?}"
    );

    // Confirmar que el rechazo fue real, no solo el texto del mensaje.
    let count: i64 = admin
        .query_one(&format!("SELECT count(*) FROM \"{COLLECTION}\""), &[])
        .expect("contar filas como admin")
        .get(0);
    assert_eq!(count, 1, "la fila 'hack' no debe existir de verdad -- el rechazo tiene que ser real: {joined:?}");

    // El rechazo no debe tumbar la sesión -- confirmar que sigue sirviendo.
    shell.send(&format!("SELECT count(*) FROM \"{COLLECTION}\""));
    let joined2 = shell.recv().join("\n");
    assert!(joined2.contains("1 fila(s)"), "el shell debe seguir respondiendo después del rechazo: {joined2:?}");

    shell.shutdown();
    admin.batch_execute(&format!("DROP TABLE IF EXISTS \"{COLLECTION}\"")).ok();
}

#[test]
fn db_shell_formats_native_numeric_uuid_and_jsonb_columns_as_legible_text_against_postgres() {
    const COLLECTION: &str = "items_db_shell_types";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut admin = postgres::Client::connect(&url, postgres::NoTls).expect("conectar como admin");
    admin
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\
                \"id\" BIGSERIAL PRIMARY KEY, \
                \"price\" NUMERIC NOT NULL, \
                \"external_id\" UUID NOT NULL, \
                \"properties\" JSONB NOT NULL\
            )"
        ))
        .expect("crear la tabla con columnas nativas no triviales");
    admin
        .batch_execute(&format!(
            "INSERT INTO \"{COLLECTION}\" (price, external_id, properties) VALUES \
             (19.99, '123e4567-e89b-12d3-a456-426614174000', '{{\"n\": 2}}'::jsonb)"
        ))
        .expect("sembrar una fila con tipos nativos no triviales");

    let temp = TempDir::new("shell-types-pg");
    let src = temp.write(
        "app.link",
        &format!("type Item = {{ id: Int, price: Decimal, externalId: String, properties: String }}\ndb {{ {COLLECTION}: Item[] }}"),
    );

    let mut shell = PgShellProcess::start(&src, &url);
    shell.send(&format!("SELECT price, external_id, properties FROM \"{COLLECTION}\""));
    let lines = shell.recv();
    let joined = lines.join("\n");
    assert!(!joined.contains("tipo no soportado"), "numeric/uuid/jsonb SÍ están cubiertos, no deberían caer al placeholder: {joined:?}");
    assert!(joined.contains("19.9900"), "NUMERIC(19.99) debe mostrarse como Decimal escalado exacto vía format_decimal: {joined:?}");
    assert!(
        joined.contains("123e4567-e89b-12d3-a456-426614174000"),
        "UUID debe mostrarse en su forma canónica de texto: {joined:?}"
    );
    // jsonb reordena/renormaliza al guardar (confirmado en la ronda que
    // arregló GRAMMAR.md §3.187) -- comparar como VALOR json, no como texto
    // exacto.
    let properties_cell = lines
        .iter()
        .find(|l| l.contains('{') && l.contains('}'))
        .unwrap_or_else(|| panic!("debe haber una celda con el jsonb sembrado: {lines:?}"));
    let start = properties_cell.find('{').unwrap();
    let end = properties_cell.rfind('}').unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&properties_cell[start..=end])
        .unwrap_or_else(|e| panic!("la celda jsonb debe ser JSON válido ({e}): {properties_cell:?}"));
    assert_eq!(parsed, serde_json::json!({"n": 2}));

    shell.shutdown();
    admin.batch_execute(&format!("DROP TABLE IF EXISTS \"{COLLECTION}\"")).ok();
}
