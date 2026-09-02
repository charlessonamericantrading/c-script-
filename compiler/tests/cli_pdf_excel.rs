// `pdf.build` (GRAMMAR.md §3.201) y `excel.build`/`excel.parse` (§3.202)
// contra el BINARIO real -- auditoría del 02/09/2026 (PLAN.md §9.17 ítem 7):
// eran los dos únicos builtins de documento shipeados sin NINGÚN test que
// pasara por el checker Y el runtime del binario de verdad. Sus tests de
// runtime existentes usan el harness `program_from()` que NO corre el
// checker, y sus ejemplos en GRAMMAR.md son `linkc:fragment` (no compilan
// aislados, usan los tipos pre-sembrados en contexto parcial) -- exactamente
// la clase de agujero por donde ya pasó el bug §3.204 (PdfBlock/ExcelCell
// tipaban pero el codegen no los conocía).
//
// `linkc test <programa>` como subproceso compila (checker incluido) y
// ejecuta los bloques `test "..."` en el intérprete real -- el mismo camino
// completo que recorre un usuario.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-pdf-excel-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let full = self.0.join(name);
        std::fs::write(&full, content).unwrap();
        full
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_linkc_test(program: &str, name: &str) -> (bool, String) {
    let temp = TempDir::new(name);
    let src = temp.write("app.link", program);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(&src)
        .output()
        .expect("no se pudo ejecutar linkc test");
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), combined)
}

/// `pdf.build` end-to-end: el programa compila con el checker real (los
/// tipos pre-sembrados `PdfBlock.Text`/`PdfBlock.Table` tipan como
/// cualquier ADT) y el test interno verifica que el resultado es un PDF de
/// verdad -- base64 de bytes que empiezan con `%PDF` es siempre `JVBERi`,
/// así que el assert distingue un PDF real de cualquier otro string sin
/// necesitar decodificar base64 desde `.link`.
#[test]
fn pdf_build_produces_a_real_pdf_through_the_full_compile_and_run_path() {
    let program = r#"
service Docs {
  rpc factura(cliente: String, total: String) -> String {
    pdf.build([
      PdfBlock.Text { content: "Factura", bold: true, size: 18 },
      PdfBlock.Text { content: "Cliente: " + cliente, bold: false, size: 12 },
      PdfBlock.Table { headers: ["Concepto", "Importe"], rows: [["Servicio", total]] },
      PdfBlock.Text { content: "Total: " + total, bold: true, size: 14 },
    ])
  }
}

test "pdf.build devuelve un PDF real en base64" {
  let b64 = Docs.factura("ACME", "120.50");
  assert(b64.length() > 100, "un PDF con texto y tabla no puede ser tan chico");
  assert(b64.startsWith("JVBERi"), "base64 de bytes '%PDF' -- si no empieza asi, no es un PDF");
}
"#;
    let (ok, output) = run_linkc_test(program, "pdf");
    assert!(ok, "linkc test debió pasar:\n{output}");
    assert!(output.contains("1 passed") || output.contains("ok"), "salida inesperada:\n{output}");
}

/// `excel.build` + `excel.parse` round-trip end-to-end: las 4 variantes de
/// celda con valor viajan a un `.xlsx` real y vuelven con los MISMOS
/// valores -- el `Decimal` exacto (no aproximado por el `f64` interno del
/// formato) y la fecha reconocible como fecha (el bug real de
/// `write_datetime` que el round-trip de §3.202 atrapó en su momento). El
/// `match` sobre `ExcelCell` de vuelta prueba además que el enum
/// pre-sembrado se puede CONSUMIR desde `.link`, no solo construir.
#[test]
fn excel_build_then_parse_round_trips_every_cell_variant_with_exact_values() {
    let program = r#"
service Export {
  rpc hoja() -> String {
    excel.build([ExcelSheet {
      name: "Movimientos",
      headers: ["Fecha", "Concepto", "Importe", "Conciliado"],
      rows: [
        [
          ExcelCell.Date { value: dateFromParts(2026, 9, 2, 10, 30, 0) },
          ExcelCell.Text { value: "Café con acentos: ñandú" },
          ExcelCell.Number { value: 1234.5678.toDecimal() },
          ExcelCell.Bool { value: true },
        ],
      ],
    }])
  }

  rpc importada(base64: String) -> ExcelSheet[] {
    excel.parse(base64)
  }

  rpc conceptoDeVuelta(base64: String) -> String {
    let hojas = excel.parse(base64);
    let fila = hojas[0].rows[0];
    match fila[1] {
      ExcelCell.Text { value } => value,
      _ => "NO-ES-TEXTO",
    }
  }

  rpc importeDeVuelta(base64: String) -> String {
    let hojas = excel.parse(base64);
    let fila = hojas[0].rows[0];
    match fila[2] {
      ExcelCell.Number { value } => value.toString(),
      _ => "NO-ES-NUMERO",
    }
  }

  rpc fechaEsFecha(base64: String) -> Bool {
    let hojas = excel.parse(base64);
    let fila = hojas[0].rows[0];
    match fila[0] {
      ExcelCell.Date { value } => true,
      _ => false,
    }
  }
}

test "excel.build produce un xlsx real y el round-trip conserva los valores" {
  let b64 = Export.hoja();
  assert(b64.startsWith("UEsDB"), "base64 de bytes 'PK..' (firma ZIP) -- si no, no es un xlsx");

  let hojas = Export.importada(b64);
  assert(hojas.length() == 1, "una hoja escrita, una hoja leida");
  assert(hojas[0].name == "Movimientos", "el nombre de la hoja sobrevive");
  assert(hojas[0].headers.length() == 4, "los 4 encabezados sobreviven");
  assert(hojas[0].rows.length() == 1, "una fila de datos");

  assert(Export.conceptoDeVuelta(b64) == "Café con acentos: ñandú", "el texto UTF-8 vuelve exacto");
  assert(Export.importeDeVuelta(b64) == "1234.5678", "el Decimal vuelve EXACTO, no aproximado por f64");
  assert(Export.fechaEsFecha(b64), "la fecha vuelve como Date, no degradada a Number");
}
"#;
    let (ok, output) = run_linkc_test(program, "excel");
    assert!(ok, "linkc test debió pasar:\n{output}");
}

/// El camino de error también por el binario completo: una fila con
/// distinta cantidad de columnas que `headers` es un error de RUNTIME
/// limpio que hace fallar el test interno nombrando el problema -- nunca
/// un panic del proceso ni un xlsx corrupto en silencio.
#[test]
fn a_row_with_mismatched_columns_fails_cleanly_not_silently() {
    let program = r#"
service Export {
  rpc rota() -> String {
    excel.build([ExcelSheet {
      name: "Mal",
      headers: ["A", "B"],
      rows: [[ExcelCell.Text { value: "solo una columna" }]],
    }])
  }
}

test "una fila desalineada revienta el test" {
  let x = Export.rota();
  assert(x.length() > 0, "no deberia llegar aca");
}
"#;
    let (ok, output) = run_linkc_test(program, "mismatch");
    assert!(!ok, "linkc test debió FALLAR por la fila desalineada:\n{output}");
    assert!(
        output.contains("column") || output.contains("fila") || output.contains("columna"),
        "el error debe nombrar el problema de columnas, no ser un panic genérico:\n{output}"
    );
}
