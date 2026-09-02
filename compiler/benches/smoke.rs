//! Benchmarks de humo (PLAN.md §9.18 Eje B ítem 1, GRAMMAR.md §3.217).
//!
//! Hasta esta ronda el proyecto no tenía NINGUNA medición de rendimiento
//! (§9.17 ítem 16 lo anotó; grep de `req/s`/`p99` en toda la documentación:
//! cero). Sin baseline, cada optimización del intérprete o del servidor es
//! una apuesta que no se puede verificar -- el mismo problema que
//! `docs_examples.rs` resuelve para la documentación, acá para el tiempo.
//!
//! Qué mide y por qué exactamente esto (no más):
//! - `check/users.link`: cargar + parsear + tipar el programa de referencia.
//!   Es lo que paga cada `linkc test <archivo>` (el camino rápido de "solo
//!   chequear", AGENTS.md) y cada iteración de un agente de IA.
//! - `rpc/create`: una escritura real (INSERT en SQLite `:memory:`) vía
//!   `invoke_rpc` -- el mismo punto de entrada que usa `linkc serve`, sin
//!   la capa HTTP. Mide intérprete + db.rs + serialización JSON.
//! - `rpc/list_100`: una lectura de 100 filas (`all()`), decodificación de
//!   fila a `Value` y de `Value` a JSON incluidas.
//! - `rpc/findWhere_pushdown`: el filtro `|n| n.pinned` empujado a SQL
//!   (GRAMMAR.md §3.95) -- mide que el pushdown de verdad evita cargar la
//!   tabla entera.
//! - `interp/while_1000`: un rpc PURO sin base ni I/O -- aísla el costo del
//!   árbol de evaluación (`eval_expr`), que es lo único que el Eje B ítem 6
//!   tocaría. Si este número no se mueve, el intérprete no es el cuello.
//!
//! Lo que NO mide a propósito: `linkc serve` bajo carga concurrente (un
//! pool de hilos/conexiones -- Eje B ítems 2 y 3 -- se mide con un cliente
//! externo contra un servidor real, no dentro de criterion; ese harness
//! llega con esa ronda, no antes). `criterion` es dependencia SOLO de
//! desarrollo -- no cambia el binario ni viola "cero dependencias" (§3.73).
//!
//! Correr: `cd compiler && cargo bench --bench smoke`. Comparar contra la
//! corrida anterior: criterion guarda `target/criterion/` y reporta el
//! cambio relativo solo. Los números de referencia de la máquina de
//! desarrollo están en GRAMMAR.md §3.217 -- son orientativos, no un SLA.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use linkc::ast::Program;
use linkc::checker::Checker;
use linkc::runtime::db::Db;
use linkc::runtime::invoke_rpc;
use std::path::Path;

/// Programa mínimo con una colección real y un rpc puro. Se tipa con el
/// checker de verdad (no el harness `program_from` de los tests unitarios,
/// que lo saltea -- `feedback_checker_skipping_test_harness`): un benchmark
/// sobre un programa que no compila mediría basura.
const BENCH_PROGRAM: &str = r#"
type Note = { id: Int, body: String, pinned: Bool }
type NewNote = { body: String, pinned: Bool }

db { notes: Note[] }

service Notes {
  rpc list() -> Note[] { db.notes.all() }
  rpc pinned() -> Note[] { db.notes.findWhere(|n: Note| { n.pinned }) }
  rpc create(body: String) -> Note {
    db.notes.insert(NewNote { body: body, pinned: false })
  }
  rpc sumTo(n: Int) -> Int {
    let mut i = 0;
    let mut acc = 0;
    while i < n {
      acc = acc + i;
      i = i + 1;
    }
    acc
  }
}
"#;

fn checked_program(src: &str) -> Program {
    let tokens = linkc::lexer::tokenize(src).expect("lexer");
    let program = linkc::parser::parse(tokens).expect("parser");
    Checker::check_program(&program).unwrap_or_else(|errs| panic!("el programa del benchmark no tipa: {errs:?}"));
    program
}

fn users_link_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link")
}

fn bench_check(c: &mut Criterion) {
    let path = users_link_path();
    // Verificación previa fuera del loop: si el programa de referencia no
    // carga, que falle acá con el error real, no dentro de criterion.
    let (program, _, item_files) = linkc::modules::load_program(&path).expect("users.link carga");
    Checker::check_program_with_files(&program, &item_files).expect("users.link tipa");

    c.bench_function("check/users.link", |b| {
        b.iter(|| {
            let (program, _, item_files) = linkc::modules::load_program(black_box(&path)).unwrap();
            Checker::check_program_with_files(&program, &item_files).unwrap();
        })
    });
}

fn bench_rpc(c: &mut Criterion) {
    let program = checked_program(BENCH_PROGRAM);

    c.bench_function("rpc/create", |b| {
        let db = Db::new(&program, Path::new(":memory:"));
        let args = serde_json::json!({ "body": "hola" });
        b.iter(|| invoke_rpc(&program, "Notes", "create", black_box(&args), &db).unwrap())
    });

    let mut group = c.benchmark_group("rpc/read");
    group.throughput(Throughput::Elements(100));
    let db = Db::new(&program, Path::new(":memory:"));
    for i in 0..100 {
        let args = serde_json::json!({ "body": format!("nota {i}") });
        invoke_rpc(&program, "Notes", "create", &args, &db).unwrap();
    }
    let no_args = serde_json::json!({});
    group.bench_function("list_100", |b| {
        b.iter(|| invoke_rpc(&program, "Notes", "list", black_box(&no_args), &db).unwrap())
    });
    group.bench_function("findWhere_pushdown", |b| {
        b.iter(|| invoke_rpc(&program, "Notes", "pinned", black_box(&no_args), &db).unwrap())
    });
    group.finish();
}

fn bench_interp(c: &mut Criterion) {
    let program = checked_program(BENCH_PROGRAM);
    let db = Db::new(&program, Path::new(":memory:"));
    let args = serde_json::json!({ "n": 1000 });
    c.bench_function("interp/while_1000", |b| {
        b.iter(|| invoke_rpc(&program, "Notes", "sumTo", black_box(&args), &db).unwrap())
    });
}

criterion_group!(benches, bench_check, bench_rpc, bench_interp);
criterion_main!(benches);
