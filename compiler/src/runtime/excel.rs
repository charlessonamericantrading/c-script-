// Generación y parsing real de bytes `.xlsx` (GRAMMAR.md §3.202) a partir
// de/hacia `ExcelSheetSpec`/`ExcelCellSpec` -- ver el comentario en
// `compiler/Cargo.toml` sobre por qué `rust_xlsxwriter`/`calamine` son la
// quinta y sexta excepción real a "cero dependencias nuevas" (una
// excepción CONJUNTA: comparten `zip` como dependencia transitiva).
// Escritura y lectura viven en el mismo módulo a propósito -- el diseño de
// `ExcelCellSpec` tiene que ser coherente entre las dos direcciones, no
// dos piezas independientes diseñadas por separado.

use calamine::{Data, Reader, Xlsx};
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq)]
pub enum ExcelCellSpec {
    Text(String),
    /// Ya escalado ×10.000 -- mismo repr que `Value::Decimal` (GRAMMAR.md
    /// §3.184). Nunca se expone un `f64` crudo a `.link`.
    Number(i128),
    /// Milisegundos desde epoch UTC -- mismo repr que `Value::Timestamp`.
    Date(i64),
    Bool(bool),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExcelSheetSpec {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<ExcelCellSpec>>,
}

/// `f64` (lo que `.xlsx` guarda internamente para cualquier celda numérica)
/// -> `Decimal` escalado, reusando la MISMA lógica de redondeo/rango que
/// `Float.toDecimal()` (`runtime/mod.rs::decimal_from_float`) -- no una
/// segunda copia de la fórmula de redondeo.
fn f64_to_decimal_scaled(f: f64) -> Result<i128, String> {
    match super::decimal_from_float(f) {
        Ok(super::Value::Decimal(n)) => Ok(n),
        Ok(_) => unreachable!("decimal_from_float siempre devuelve Value::Decimal"),
        Err(e) => Err(e.message),
    }
}

fn data_to_string(d: &Data) -> String {
    match d {
        Data::String(s) => s.clone(),
        Data::Empty => String::new(),
        other => other.to_string(),
    }
}

/// `calamine::Data` (9 variantes) -> `ExcelCellSpec` (5 variantes). Las
/// variantes sin equivalente directo (`Error`, `DurationIso`, y
/// `DateTimeIso` -- string ISO de otro formato que xlsx no suele producir,
/// `DateTime` es la variante real que xlsx usa) se representan como
/// `Text` con su forma de texto -- NUNCA se descartan en silencio, mismo
/// criterio "rechazar limpio o representar honesto" del resto del
/// proyecto.
fn data_to_cell_spec(d: &Data) -> Result<ExcelCellSpec, String> {
    Ok(match d {
        Data::Int(n) => {
            let scaled = (*n as i128)
                .checked_mul(super::DECIMAL_SCALE)
                .ok_or_else(|| format!("excel.parse: {n} no entra en el rango de Decimal"))?;
            ExcelCellSpec::Number(scaled)
        }
        Data::Float(f) => ExcelCellSpec::Number(f64_to_decimal_scaled(*f)?),
        Data::String(s) => ExcelCellSpec::Text(s.clone()),
        Data::Bool(b) => ExcelCellSpec::Bool(*b),
        Data::DateTime(edt) => {
            // Excel Data::DateTime reproduce A PROPÓSITO el bug histórico
            // de la fecha inexistente 1900-02-29 (documentado por la
            // propia crate) -- `to_ymd_hms_milli` ya da el resultado que
            // Excel real mostraría, sin código extra de este lado, y sin
            // necesitar el feature `chrono` de calamine.
            let (y, mo, d, h, mi, s, ms) = edt.to_ymd_hms_milli();
            let base_ms = super::timestamp::date_from_parts(
                i64::from(y),
                i64::from(mo),
                i64::from(d),
                i64::from(h),
                i64::from(mi),
                i64::from(s),
            )?;
            ExcelCellSpec::Date(base_ms + i64::from(ms))
        }
        Data::Empty => ExcelCellSpec::Empty,
        Data::DateTimeIso(_) | Data::DurationIso(_) | Data::Error(_) => ExcelCellSpec::Text(data_to_string(d)),
    })
}

pub fn build(sheets: &[ExcelSheetSpec]) -> Result<Vec<u8>, String> {
    for sheet in sheets {
        let expected = if !sheet.headers.is_empty() {
            sheet.headers.len()
        } else {
            sheet.rows.first().map(|r| r.len()).unwrap_or(0)
        };
        for (i, row) in sheet.rows.iter().enumerate() {
            if row.len() != expected {
                return Err(format!(
                    "excel.build: la fila {i} de la hoja '{}' tiene {} columna(s), se esperaban {expected}",
                    sheet.name,
                    row.len()
                ));
            }
        }
    }

    let mut workbook = Workbook::new();
    for sheet in sheets {
        let worksheet = workbook.add_worksheet();
        worksheet
            .set_name(&sheet.name)
            .map_err(|e| format!("excel.build: nombre de hoja '{}' inválido: {e}", sheet.name))?;

        let mut row_idx: u32 = 0;
        if !sheet.headers.is_empty() {
            for (col, h) in sheet.headers.iter().enumerate() {
                worksheet.write_string(row_idx, col as u16, h).map_err(|e| format!("excel.build: {e}"))?;
            }
            row_idx += 1;
        }
        for row in &sheet.rows {
            for (col, cell) in row.iter().enumerate() {
                let col = col as u16;
                match cell {
                    ExcelCellSpec::Text(s) => {
                        worksheet.write_string(row_idx, col, s).map_err(|e| format!("excel.build: {e}"))?;
                    }
                    ExcelCellSpec::Number(scaled) => {
                        let value = *scaled as f64 / super::DECIMAL_SCALE as f64;
                        worksheet.write_number(row_idx, col, value).map_err(|e| format!("excel.build: {e}"))?;
                    }
                    ExcelCellSpec::Date(ms) => {
                        let (y, m, d, h, mi, s, milli) = super::timestamp::ymd_hms_milli_from_millis(*ms);
                        let dt = ExcelDateTime::from_ymd(y as u16, m as u8, d as u8)
                            .and_then(|dt| dt.and_hms_milli(h as u16, mi as u8, s as u8, milli as u16))
                            .map_err(|e| format!("excel.build: fecha inválida: {e}"))?;
                        // SIN un formato de número explícito, la celda
                        // guarda el número de serie de Excel pero NO se
                        // distingue de un `Number` común al leerla de
                        // vuelta (ni con Excel real ni con `calamine`) --
                        // confirmado con un test de round-trip real que
                        // fallaba silenciosamente sin esto.
                        let date_format = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss.000");
                        worksheet
                            .write_with_format(row_idx, col, &dt, &date_format)
                            .map_err(|e| format!("excel.build: {e}"))?;
                    }
                    ExcelCellSpec::Bool(b) => {
                        worksheet.write_boolean(row_idx, col, *b).map_err(|e| format!("excel.build: {e}"))?;
                    }
                    ExcelCellSpec::Empty => {}
                }
            }
            row_idx += 1;
        }
    }

    workbook.save_to_buffer().map_err(|e| format!("excel.build: {e}"))
}

pub fn parse(bytes: &[u8]) -> Result<Vec<ExcelSheetSpec>, String> {
    let cursor = Cursor::new(bytes.to_vec());
    let mut workbook: Xlsx<_> =
        Xlsx::new(cursor).map_err(|e| format!("excel.parse: no se pudo abrir el archivo .xlsx: {e}"))?;

    let mut sheets = Vec::new();
    for (name, range) in workbook.worksheets() {
        let mut rows_iter = range.rows();
        // Siempre se trata la primera fila como encabezados -- límite v1
        // documentado (GRAMMAR.md §3.202): si el `.xlsx` no tiene
        // encabezados reales, esa primera fila de datos aparece como
        // `headers` en vez de en `rows`. Coincide con el caso real citado
        // (extractos bancarios, que siempre traen encabezados).
        let headers: Vec<String> = match rows_iter.next() {
            Some(first_row) => first_row.iter().map(data_to_string).collect(),
            None => Vec::new(),
        };
        let mut rows = Vec::new();
        for row in rows_iter {
            rows.push(row.iter().map(data_to_cell_spec).collect::<Result<Vec<_>, String>>()?);
        }
        sheets.push(ExcelSheetSpec { name, headers, rows });
    }
    Ok(sheets)
}
