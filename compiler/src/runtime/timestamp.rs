// ISO-8601 (UTC, milisegundos, forma fija `YYYY-MM-DDTHH:mm:ss.sssZ`) <->
// milisegundos desde epoch, para `Type::Timestamp` (GRAMMAR.md §3.31) -- sin
// ninguna dependencia de fecha/hora nueva. El cálculo de calendario
// (año/mes/día <-> días desde epoch) es el algoritmo público de Howard
// Hinnant (http://howardhinnant.github.io/date_algorithms.html, dominio
// público / CC0, el mismo que usa libc++ para std::chrono::year_month_day)
// -- aritmética entera exacta, sin tabla de lookup, correcto en años
// bisiestos y en los límites de siglo (1900 no es bisiesto, 2000 sí) por
// construcción, no por casos especiales a mano. Mismo espíritu que el
// SHA-256 de lockfile.rs o el diff LCS de linkc test: un algoritmo chico y
// bien definido, no una razón para sumar una crate de calendario completa.

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_MIN: i64 = 60_000;
const MS_PER_SEC: i64 = 1_000;

/// Año/mes/día (calendario gregoriano proléptico) -> días desde
/// 1970-01-01, negativo para fechas anteriores. Puerto directo del
/// `days_from_civil` de Hinnant -- las dos ramas del cálculo de `era` (una
/// para `y >= 0`, otra para `y < 0`) SÍ importan acá, no son código muerto:
/// hasta un año tan "reciente" como 0000 ya cae del lado negativo (ver el
/// test de la frontera correspondiente).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inversa de `days_from_civil`: días desde 1970-01-01 -> (año, mes, día).
/// Puerto directo del `civil_from_days` de Hinnant.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = mp + if mp < 10 { 3 } else { -9 }; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// Milisegundos desde epoch -> string ISO-8601 UTC, forma fija exacta
/// `YYYY-MM-DDTHH:mm:ss.sssZ`. Total (nunca falla): cualquier `i64` es una
/// fecha válida en el calendario proléptico que usa Hinnant. `div_euclid`/
/// `rem_euclid`, no `/`/`%` crudos -- para un `total_ms` negativo (antes de
/// 1970), la división truncada de Rust redondearía hacia 0 en vez de hacia
/// el día anterior (ej. -1ms daría día 0 hora "-0.001" en vez de día -1
/// hora 23:59:59.999, que es lo correcto).
pub(crate) fn format_iso8601_millis(total_ms: i64) -> String {
    let days = total_ms.div_euclid(MS_PER_DAY);
    let ms_of_day = total_ms.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let hour = ms_of_day / MS_PER_HOUR;
    let min = (ms_of_day % MS_PER_HOUR) / MS_PER_MIN;
    let sec = (ms_of_day % MS_PER_MIN) / MS_PER_SEC;
    let ms = ms_of_day % MS_PER_SEC;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z")
}

/// Inversa de `format_iso8601_millis`. `None` si `s` no matchea EXACTAMENTE
/// la forma fija de 24 bytes (ancho fijo, 'Z' obligatorio -- sin offsets de
/// timezone, sin precisión variable, GRAMMAR.md §3.31 -- así que el año
/// queda limitado a 4 dígitos, 0000-9999, una restricción real de v0) o si
/// los campos no forman una fecha de calendario que existe de verdad (mes
/// 13, hora 25, o un día que no existe en ese mes -- 2026-02-30).
pub(crate) fn parse_iso8601_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24 {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'.' || b[23] != b'Z' {
        return None;
    }
    let digit = |i: usize| -> Option<i64> {
        let c = b[i];
        c.is_ascii_digit().then(|| i64::from(c - b'0'))
    };
    let num = |start: usize, len: usize| -> Option<i64> {
        (start..start + len).try_fold(0i64, |acc, i| Some(acc * 10 + digit(i)?))
    };
    let y = num(0, 4)?;
    let m = num(5, 2)?;
    let d = num(8, 2)?;
    let hour = num(11, 2)?;
    let min = num(14, 2)?;
    let sec = num(17, 2)?;
    let ms = num(20, 3)?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    // El propio algoritmo es el validador: un día que no existe (30 de
    // febrero) "se derrama" hacia el mes siguiente al calcular `days` --
    // convertir esos `days` de vuelta y comparar contra (y, m, d) original
    // detecta el derrame sin duplicar ninguna tabla de "días por mes" a mano.
    if civil_from_days(days) != (y, m, d) {
        return None;
    }
    Some(days * MS_PER_DAY + hour * MS_PER_HOUR + min * MS_PER_MIN + sec * MS_PER_SEC + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_round_trips() {
        assert_eq!(format_iso8601_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(parse_iso8601_millis("1970-01-01T00:00:00.000Z"), Some(0));
    }

    #[test]
    fn a_leap_day_round_trips() {
        let s = "2024-02-29T12:00:00.000Z";
        let ms = parse_iso8601_millis(s).expect("2024 es bisiesto, 29 de feb existe");
        assert_eq!(format_iso8601_millis(ms), s);
    }

    #[test]
    fn the_century_non_leap_boundary_is_rejected_and_accepted_correctly() {
        // 1900 NO es bisiesto (divisible por 100 pero no por 400) -- el bug
        // ingenuo clásico ("divisible por 4") lo aceptaría igual.
        assert_eq!(parse_iso8601_millis("1900-02-29T00:00:00.000Z"), None);
        assert!(parse_iso8601_millis("1900-02-28T00:00:00.000Z").is_some());
        // 2000 SÍ es bisiesto (divisible por 400) -- el otro lado del mismo bug.
        assert!(parse_iso8601_millis("2000-02-29T00:00:00.000Z").is_some());
    }

    #[test]
    fn a_pre_1970_negative_timestamp_round_trips() {
        let s = "1969-12-31T23:59:59.999Z";
        let ms = parse_iso8601_millis(s).unwrap();
        assert_eq!(ms, -1);
        assert_eq!(format_iso8601_millis(ms), s);
        assert_eq!(format_iso8601_millis(-1), s);
    }

    #[test]
    fn a_day_that_does_not_exist_is_rejected() {
        assert_eq!(parse_iso8601_millis("2026-02-30T00:00:00.000Z"), None); // feb no tiene 30
        assert_eq!(parse_iso8601_millis("2026-04-31T00:00:00.000Z"), None); // abril no tiene 31
        assert_eq!(parse_iso8601_millis("2026-13-01T00:00:00.000Z"), None); // mes 13
        assert_eq!(parse_iso8601_millis("2026-00-01T00:00:00.000Z"), None); // mes 0
    }

    #[test]
    fn wrong_shape_is_rejected_not_panicking() {
        for bad in [
            "",
            "2026-08-08",                    // sin hora
            "2026-08-08T10:00:00Z",          // sin milisegundos
            "2026-08-08T10:00:00.000+02:00", // offset de timezone, no 'Z'
            "26-08-08T10:00:00.000Z",        // año de 2 dígitos
            "not a timestamp at all!",
        ] {
            assert_eq!(parse_iso8601_millis(bad), None, "'{bad}' no debería parsear");
        }
    }
}
