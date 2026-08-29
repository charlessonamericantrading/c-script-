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

/// Cuántos días separan el epoch de POSTGRES (2000-01-01, el que usan sus
/// tipos binarios `date`/`timestamp`/`timestamptz`) del epoch de c-script
/// (1970-01-01, GRAMMAR.md §3.31) -- calculado con `days_from_civil`, no un
/// número mágico hardcodeado, así que queda claro DE DÓNDE sale (y un test
/// lo fija contra el valor público conocido, 10957).
const PG_EPOCH_DAYS_SINCE_UNIX_EPOCH: i64 = 10_957;

/// Milisegundos-desde-1970 (la representación interna de `Timestamp`,
/// GRAMMAR.md §3.31) a partir de microsegundos-desde-2000-01-01 -- la forma
/// binaria EXACTA en la que Postgres guarda `timestamp`/`timestamptz`
/// (GRAMMAR.md §3.91). `div_euclid`, no `/` cruda -- mismo motivo que
/// `format_iso8601_millis`: para un valor negativo (una fecha anterior al
/// año 2000), la división truncada redondearía hacia 0 en vez de hacia
/// abajo.
pub(crate) fn millis_from_pg_timestamp_micros(micros: i64) -> i64 {
    micros.div_euclid(1000) + PG_EPOCH_DAYS_SINCE_UNIX_EPOCH * MS_PER_DAY
}

/// Como `millis_from_pg_date_days`, para la forma binaria de un `date`
/// nativo de Postgres -- días (no microsegundos) desde 2000-01-01.
pub(crate) fn millis_from_pg_date_days(days: i32) -> i64 {
    (i64::from(days) + PG_EPOCH_DAYS_SINCE_UNIX_EPOCH) * MS_PER_DAY
}

/// Inversa EXACTA de `millis_from_pg_timestamp_micros` -- milisegundos
/// desde 1970 (la representación interna de `Timestamp`) a microsegundos
/// desde 2000-01-01 (la forma binaria que Postgres espera al ESCRIBIR
/// contra una columna `timestamp`/`timestamptz` nativa).
///
/// Bug real de adopción (iaacademy, vía skynet-43, 2026-08-29): antes de
/// esto, `Cell::to_sql` no tenía ningún caso para `ty == TIMESTAMP(TZ)` --
/// un `Cell::Int(millis)` caía al `_ => n.to_sql(ty, out)` genérico
/// (`i64::to_sql`, los mismos 8 bytes crudos que usa una columna `BIGINT`
/// normal). A diferencia del mismatch de `numeric` (§3.103, que Postgres
/// SÍ rechaza -- formato binario de ancho/forma distinta), acá los 8 bytes
/// de un `int8` y los de un `timestamptz` binario tienen el MISMO ancho --
/// el servidor los acepta sin quejarse, solo que interpretándolos como
/// microsegundos-desde-2000 en vez de milisegundos-desde-1970: silencioso,
/// nunca un error, la fecha guardada queda corrompida sin ningún aviso.
/// Confirmado con aritmética antes de escribir el fix (no solo el repro):
/// escribir el `Timestamp` crudo de 2026 contra una columna `timestamptz`
/// da como resultado una fecha de enero del año 2000 -- exactamente lo que
/// da interpretar esos milisegundos como si fueran microsegundos.
pub(crate) fn pg_timestamp_micros_from_millis(millis: i64) -> i64 {
    (millis - PG_EPOCH_DAYS_SINCE_UNIX_EPOCH * MS_PER_DAY) * 1000
}

/// Como `pg_timestamp_micros_from_millis`, para la forma binaria de un
/// `date` nativo -- trunca cualquier componente de hora (mismo criterio que
/// el propio Postgres al hacer `timestamp::date`: un `Timestamp` que no cae
/// exacto a medianoche UTC pierde esa parte al guardarse en una columna
/// `date`, no es un error). `div_euclid`, no `/` cruda, por el mismo motivo
/// de siempre: una fecha anterior a 2000 tiene que redondear hacia el día
/// de calendario correcto, no truncar hacia 0.
pub(crate) fn pg_date_days_from_millis(millis: i64) -> i32 {
    (millis.div_euclid(MS_PER_DAY) - PG_EPOCH_DAYS_SINCE_UNIX_EPOCH) as i32
}

/// Milisegundos desde epoch -> `(dateStamp, amzDate)` en las dos formas
/// fijas que AWS Signature V4 exige (GRAMMAR.md §3.110): `dateStamp` es
/// `YYYYMMDD` (usado en el credential scope y en la derivación de la
/// clave de firma), `amzDate` es `YYYYMMDDTHHMMSSZ` (el header/parámetro
/// `X-Amz-Date`) -- mismo cálculo de calendario que `format_iso8601_millis`
/// de arriba, solo que sin separadores. `div_euclid`/`rem_euclid` por el
/// mismo motivo de siempre (una fecha anterior a 1970 no debería pasar
/// nunca por acá en la práctica -- esto siempre parte de `SystemTime::now()`
/// -- pero la función es total de todas formas, sin panics posibles).
pub(crate) fn format_aws_sigv4_datetime(total_ms: i64) -> (String, String) {
    let days = total_ms.div_euclid(MS_PER_DAY);
    let ms_of_day = total_ms.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let hour = ms_of_day / MS_PER_HOUR;
    let min = (ms_of_day % MS_PER_HOUR) / MS_PER_MIN;
    let sec = (ms_of_day % MS_PER_MIN) / MS_PER_SEC;
    let date_stamp = format!("{y:04}{m:02}{d:02}");
    let amz_date = format!("{date_stamp}T{hour:02}{min:02}{sec:02}Z");
    (date_stamp, amz_date)
}

/// `dateFromParts(year, month, day, hour, minute, second) -> Timestamp`
/// (GRAMMAR.md §3.90): construye un `Timestamp` arbitrario a partir de sus
/// componentes de calendario -- cierra el límite que §3.31 dejaba abierto a
/// propósito ("un `Timestamp` solo llega de un `rpc` o de la base, nunca se
/// construye"). Reusa `parse_iso8601_millis` armando el string de forma fija
/// (milisegundos siempre `.000`) en vez de reimplementar la validación de
/// calendario -- un solo lugar decide qué fecha "existe de verdad".
///
/// Rangos validados ACÁ (antes de llegar a `parse_iso8601_millis`) para dar
/// un mensaje que nombra CUÁL campo está mal, en vez del `None` genérico
/// que ese parser da: `parse_iso8601_millis` solo detecta un día que no
/// existe DENTRO de un mes válido (30 de febrero), no un mes 13 o una hora
/// 25, que romperían el formato de ancho fijo antes de llegar a esa
/// comprobación.
pub(crate) fn date_from_parts(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Result<i64, String> {
    // Mismo límite de 4 dígitos que el parseo de un Timestamp que llega por
    // el wire (GRAMMAR.md §3.31) -- consistente en los dos sentidos.
    if !(0..=9999).contains(&year) {
        return Err(format!("dateFromParts: 'year' debe estar entre 0 y 9999, se recibió {year}"));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("dateFromParts: 'month' debe estar entre 1 y 12, se recibió {month}"));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("dateFromParts: 'day' debe estar entre 1 y 31, se recibió {day}"));
    }
    if !(0..=23).contains(&hour) {
        return Err(format!("dateFromParts: 'hour' debe estar entre 0 y 23, se recibió {hour}"));
    }
    if !(0..=59).contains(&minute) {
        return Err(format!("dateFromParts: 'minute' debe estar entre 0 y 59, se recibió {minute}"));
    }
    if !(0..=59).contains(&second) {
        return Err(format!("dateFromParts: 'second' debe estar entre 0 y 59, se recibió {second}"));
    }
    let s = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z");
    parse_iso8601_millis(&s)
        .ok_or_else(|| format!("dateFromParts: '{year:04}-{month:02}-{day:02}' no es una fecha de calendario que exista (ej. 30 de febrero)"))
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
    fn aws_sigv4_datetime_matches_the_official_aws_test_suite_dates() {
        // Fecha exacta del "get-vanilla" test case del aws4_testsuite oficial
        // de AWS (Mon, 09 Sep 2011 23:36:00 GMT) -- ver GRAMMAR.md §3.110.
        let ms = parse_iso8601_millis("2011-09-09T23:36:00.000Z").unwrap();
        assert_eq!(format_aws_sigv4_datetime(ms), ("20110909".to_string(), "20110909T233600Z".to_string()));
        // Fecha del worked example de "GET Object" con URL firmada de la
        // documentación oficial de AWS (2013-05-24T00:00:00Z).
        let ms = parse_iso8601_millis("2013-05-24T00:00:00.000Z").unwrap();
        assert_eq!(format_aws_sigv4_datetime(ms), ("20130524".to_string(), "20130524T000000Z".to_string()));
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

    // ---- decodificación de `date`/`timestamp` nativos de Postgres (GRAMMAR.md §3.91) ----

    #[test]
    fn pg_epoch_constant_matches_the_calendar_algorithm() {
        // El literal hardcodeado (10957, para no recalcularlo en cada
        // conversión) tiene que coincidir con lo que el propio algoritmo de
        // calendario da -- si alguien lo escribió mal a mano, este test lo
        // atrapa.
        assert_eq!(PG_EPOCH_DAYS_SINCE_UNIX_EPOCH, days_from_civil(2000, 1, 1));
    }

    #[test]
    fn pg_timestamp_micros_at_its_own_epoch_matches_the_known_unix_millis() {
        // 2000-01-01T00:00:00Z en milisegundos-desde-1970 es un valor
        // públicamente conocido (946684800000) -- ancla independiente,
        // aparte de la propia constante que se está probando.
        assert_eq!(millis_from_pg_timestamp_micros(0), 946_684_800_000);
        assert_eq!(format_iso8601_millis(millis_from_pg_timestamp_micros(0)), "2000-01-01T00:00:00.000Z");
    }

    #[test]
    fn pg_timestamp_micros_before_its_epoch_is_negative_and_correct() {
        // Un valor negativo de postgres (antes de su propio epoch) tiene
        // que seguir dando la fecha correcta, no redondear mal por usar
        // división truncada en vez de div_euclid.
        let one_day_before = -86_400_000_000i64; // -1 día en microsegundos
        assert_eq!(format_iso8601_millis(millis_from_pg_timestamp_micros(one_day_before)), "1999-12-31T00:00:00.000Z");
    }

    #[test]
    fn pg_timestamp_micros_truncates_to_millisecond_precision() {
        // c-script solo tiene precisión de milisegundos (GRAMMAR.md §3.31)
        // -- microsegundos de más se truncan, no se redondean ni fallan.
        let half_a_millisecond = 500; // microsegundos, relativo al epoch de Postgres
        assert_eq!(millis_from_pg_timestamp_micros(half_a_millisecond), 946_684_800_000);
    }

    #[test]
    fn pg_date_days_at_its_own_epoch_matches_the_known_unix_millis() {
        assert_eq!(millis_from_pg_date_days(0), 946_684_800_000);
        assert_eq!(format_iso8601_millis(millis_from_pg_date_days(0)), "2000-01-01T00:00:00.000Z");
    }

    #[test]
    fn pg_date_days_round_trips_a_real_calendar_date() {
        // 2026-08-24 -- días desde el epoch de Postgres calculados con el
        // MISMO algoritmo de calendario (independiente de la conversión que
        // se está probando).
        let days_since_pg_epoch = (days_from_civil(2026, 8, 24) - PG_EPOCH_DAYS_SINCE_UNIX_EPOCH) as i32;
        assert_eq!(format_iso8601_millis(millis_from_pg_date_days(days_since_pg_epoch)), "2026-08-24T00:00:00.000Z");
    }

    // ---- escritura contra `timestamp`/`timestamptz`/`date` nativos de
    // Postgres (GRAMMAR.md §3.182 -- inversa de las conversiones de arriba) ----

    #[test]
    fn pg_timestamp_micros_from_millis_is_the_exact_inverse_of_the_read_side_conversion() {
        // Repro real de skynet-43/iaacademy antes de este fix: escribir el
        // Timestamp crudo de 2026 contra `timestamptz` daba una fecha de
        // enero de 2000 -- exactamente lo que da interpretar milisegundos
        // como si fueran microsegundos. La ida y vuelta millis -> micros ->
        // millis tiene que ser EXACTA (sin pérdida) para cualquier
        // milisegundo real, no solo para el epoch.
        for iso in ["2000-01-01T00:00:00.000Z", "2026-08-29T12:34:56.789Z", "1999-12-31T23:59:59.999Z", "1969-01-01T00:00:00.000Z"] {
            let millis = parse_iso8601_millis(iso).unwrap();
            let micros = pg_timestamp_micros_from_millis(millis);
            assert_eq!(millis_from_pg_timestamp_micros(micros), millis, "ida y vuelta debe ser exacta para {iso}");
        }
    }

    #[test]
    fn pg_timestamp_micros_from_millis_matches_the_known_pg_epoch_anchor() {
        // Mismo ancla pública que `pg_timestamp_micros_at_its_own_epoch_matches_the_known_unix_millis`,
        // en la dirección inversa: 2000-01-01T00:00:00Z en millis-desde-1970
        // (946684800000, valor conocido) tiene que dar micros=0 exacto.
        assert_eq!(pg_timestamp_micros_from_millis(946_684_800_000), 0);
    }

    #[test]
    fn pg_timestamp_micros_from_millis_reproduces_the_exact_corruption_bug_when_absent() {
        // Documenta el bug real, no solo el fix: SIN esta conversión, el
        // wire binario tal cual (el i64 de millis mandado como si fueran
        // micros) resuelve a una fecha de enero de 2000 -- confirmado con
        // el mismo cálculo que motivó el fix, para que quede claro qué
        // exactamente estaba mal si alguien revierte esto sin querer.
        let millis_2026 = parse_iso8601_millis("2026-08-29T12:34:56.789Z").unwrap();
        let corrupted_if_sent_raw = format_iso8601_millis(millis_from_pg_timestamp_micros(millis_2026));
        assert!(corrupted_if_sent_raw.starts_with("2000-01-"), "confirma el bug documentado: {corrupted_if_sent_raw}");
    }

    #[test]
    fn pg_date_days_from_millis_is_the_exact_inverse_of_the_read_side_conversion_at_midnight() {
        for iso in ["2000-01-01T00:00:00.000Z", "2026-08-24T00:00:00.000Z", "1999-12-31T00:00:00.000Z"] {
            let millis = parse_iso8601_millis(iso).unwrap();
            let days = pg_date_days_from_millis(millis);
            assert_eq!(millis_from_pg_date_days(days), millis, "ida y vuelta debe ser exacta a medianoche para {iso}");
        }
    }

    #[test]
    fn pg_date_days_from_millis_truncates_a_time_of_day_component() {
        // Un Timestamp con hora distinta de medianoche pierde esa parte al
        // guardarse en una columna `date` -- mismo criterio que
        // `timestamp::date` del propio Postgres, no un error.
        let midnight = parse_iso8601_millis("2026-08-24T00:00:00.000Z").unwrap();
        let with_time = parse_iso8601_millis("2026-08-24T15:30:00.000Z").unwrap();
        assert_eq!(pg_date_days_from_millis(with_time), pg_date_days_from_millis(midnight));
    }

    // ---- `dateFromParts` (GRAMMAR.md §3.90) ----

    #[test]
    fn date_from_parts_matches_the_equivalent_iso8601_string() {
        let ms = date_from_parts(2026, 1, 1, 0, 0, 0).expect("1 de enero de 2026 existe");
        assert_eq!(ms, parse_iso8601_millis("2026-01-01T00:00:00.000Z").unwrap());
        assert_eq!(format_iso8601_millis(ms), "2026-01-01T00:00:00.000Z");
    }

    #[test]
    fn date_from_parts_supports_a_full_time_of_day() {
        let ms = date_from_parts(2026, 8, 24, 14, 30, 45).unwrap();
        assert_eq!(format_iso8601_millis(ms), "2026-08-24T14:30:45.000Z");
    }

    #[test]
    fn date_from_parts_rejects_a_day_that_does_not_exist() {
        let err = date_from_parts(2026, 2, 30, 0, 0, 0).expect_err("30 de febrero no existe");
        assert!(err.contains("2026-02-30"), "{err}");
    }

    #[test]
    fn date_from_parts_rejects_each_out_of_range_field_naming_it() {
        assert!(date_from_parts(2026, 0, 1, 0, 0, 0).unwrap_err().contains("month"));
        assert!(date_from_parts(2026, 13, 1, 0, 0, 0).unwrap_err().contains("month"));
        assert!(date_from_parts(2026, 1, 0, 0, 0, 0).unwrap_err().contains("day"));
        assert!(date_from_parts(2026, 1, 32, 0, 0, 0).unwrap_err().contains("day"));
        assert!(date_from_parts(2026, 1, 1, 24, 0, 0).unwrap_err().contains("hour"));
        assert!(date_from_parts(2026, 1, 1, 0, 60, 0).unwrap_err().contains("minute"));
        assert!(date_from_parts(2026, 1, 1, 0, 0, 60).unwrap_err().contains("second"));
        assert!(date_from_parts(-1, 1, 1, 0, 0, 0).unwrap_err().contains("year"));
        assert!(date_from_parts(10000, 1, 1, 0, 0, 0).unwrap_err().contains("year"));
    }

    #[test]
    fn date_from_parts_supports_dates_before_1970() {
        let ms = date_from_parts(1969, 12, 31, 23, 59, 59).unwrap();
        assert!(ms < 0, "una fecha antes del epoch debe dar milisegundos negativos");
        assert_eq!(format_iso8601_millis(ms), "1969-12-31T23:59:59.000Z");
    }

    /// El caso real que motiva esto (GRAMMAR.md §3.90): el límite de un
    /// trimestre se puede construir y comparar contra un Timestamp real.
    #[test]
    fn date_from_parts_builds_a_usable_quarter_boundary() {
        let q1_start = date_from_parts(2026, 1, 1, 0, 0, 0).unwrap();
        let q2_start = date_from_parts(2026, 4, 1, 0, 0, 0).unwrap();
        let inside_q1 = parse_iso8601_millis("2026-02-15T10:00:00.000Z").unwrap();
        assert!(inside_q1 >= q1_start && inside_q1 < q2_start, "15 de febrero debe caer dentro del Q1");
    }
}
