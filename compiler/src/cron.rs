//! Parseo de `@cron("5m")` / `@cron("0 4 * * *")` (GRAMMAR.md §3.159/§3.289)
//! -- tarea recurrente nativa dentro de `linkc serve`.
//!
//! Mismo motivo que `rate_limit.rs`/`cache.rs`/`idempotency.rs` para vivir
//! en un módulo aparte: el checker valida el FORMATO en compilación y el
//! servidor arma el scheduler real en runtime -- un solo parser evita que
//! las dos capas terminen de acuerdo en qué es "5m" por casualidad en vez
//! de por construcción (GRAMMAR.md §3.9).

use std::time::Duration;

/// Parsea "Ns"/"Nm"/"Nh"/"Nd" -- mismo formato que `--session-ttl`/
/// `@cache`, reimplementado acá a propósito (mismo criterio que el resto
/// de estos parsers chicos, ver el comentario de `cache::parse_ttl`).
pub fn parse_interval(raw: &str) -> Result<Duration, String> {
    let invalid = || format!("formato de @cron inválido: '{raw}' (se esperaba 'Ns', 'Nm', 'Nh' o 'Nd', ej. '5m' -- cada 5 minutos)");
    if raw.is_empty() {
        return Err(invalid());
    }
    let (num_str, unit) = raw.split_at(raw.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| invalid())?;
    if num == 0 {
        return Err(invalid());
    }
    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        "d" => Ok(Duration::from_secs(num * 86400)),
        _ => Err(invalid()),
    }
}

/// GRAMMAR.md §9.24 Fase 2 ítem F1: el `schedule` de `@cron` es O un
/// intervalo fijo (arriba) O una expresión cron real de 5 campos. Las dos
/// gramáticas nunca colisionan -- un intervalo (`"5m"`) nunca tiene un
/// espacio, una expresión cron real siempre tiene 4 (separando sus 5
/// campos) -- así que la presencia de un espacio es el único chequeo que
/// hace falta para elegir cuál parsear.
pub enum Schedule {
    Interval(Duration),
    Expression(CronExpr),
}

pub fn parse_schedule(raw: &str) -> Result<Schedule, String> {
    if raw.contains(' ') {
        parse_expression(raw).map(Schedule::Expression)
    } else {
        parse_interval(raw).map(Schedule::Interval)
    }
}

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_MIN: i64 = 60_000;

/// Una expresión cron real de 5 campos (`minuto hora día-mes mes
/// día-semana`), ya validada y precomputada a un `Vec<bool>` indexado por
/// valor -- así que evaluar "¿esta hora matchea?" en `next_run_after` es un
/// lookup, no volver a parsear rangos/pasos en cada minuto candidato.
pub struct CronExpr {
    minute: Vec<bool>,       // 60 entradas, índice = minuto (0-59)
    hour: Vec<bool>,         // 24 entradas, índice = hora (0-23)
    day_of_month: Vec<bool>, // 31 entradas, índice = día-1 (1-31)
    month: Vec<bool>,        // 12 entradas, índice = mes-1 (1-12)
    day_of_week: Vec<bool>,  // 7 entradas, índice = día (0=domingo..6=sábado)
    // Semántica ESTÁNDAR de cron (la misma que cron(8) de Vixie): si los DOS
    // campos día-mes/día-semana están restringidos (no `*`), un día matchea
    // si CUALQUIERA de los dos lo acepta (OR) -- no los dos a la vez (AND).
    // Si solo uno está restringido, ese manda solo; si ninguno, todo día
    // matchea. Guardado acá porque `day_of_month`/`day_of_week` ya son
    // `Vec<bool>` "todo true" cuando el campo es `*`, así que no hay forma
    // de distinguir "`*` de verdad" de "todos los valores listados a mano"
    // sin esta bandera aparte.
    dom_restricted: bool,
    dow_restricted: bool,
}

fn parse_field(spec: &str, min: u32, max: u32, field_name: &str) -> Result<Vec<bool>, String> {
    let mut allowed = vec![false; (max - min + 1) as usize];
    for part in spec.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s.parse().map_err(|_| format!("paso inválido en el campo {field_name}: '{s}'"))?;
                if step == 0 {
                    return Err(format!("paso inválido en el campo {field_name}: '{s}' -- tiene que ser mayor que 0"));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let lo: u32 = a.parse().map_err(|_| format!("rango inválido en el campo {field_name}: '{range_part}'"))?;
            let hi: u32 = b.parse().map_err(|_| format!("rango inválido en el campo {field_name}: '{range_part}'"))?;
            (lo, hi)
        } else {
            let v: u32 = range_part.parse().map_err(|_| format!("valor inválido en el campo {field_name}: '{range_part}'"))?;
            (v, v)
        };
        if lo > hi || lo < min || hi > max {
            return Err(format!("el campo {field_name} solo acepta valores entre {min} y {max}, se encontró '{range_part}'"));
        }
        let mut v = lo;
        while v <= hi {
            allowed[(v - min) as usize] = true;
            v += step;
        }
    }
    Ok(allowed)
}

fn parse_expression(raw: &str) -> Result<CronExpr, String> {
    let fields: Vec<&str> = raw.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "expresión cron inválida: '{raw}' -- se esperan exactamente 5 campos (minuto hora día-mes mes día-semana), se encontraron {} \
             (ej. '0 4 * * *' -- todos los días a las 4:00)",
            fields.len()
        ));
    }
    let minute = parse_field(fields[0], 0, 59, "de minuto")?;
    let hour = parse_field(fields[1], 0, 23, "de hora")?;
    let day_of_month = parse_field(fields[2], 1, 31, "de día del mes")?;
    let month = parse_field(fields[3], 1, 12, "de mes")?;
    // Dominio 0-7: tanto 0 como 7 significan domingo (las dos convenciones
    // reales que existen para este campo) -- se pliega acá, una sola vez,
    // en vez de que `next_run_after` tenga que acordarse de mirar dos
    // índices distintos para "domingo".
    let mut day_of_week = parse_field(fields[4], 0, 7, "de día de la semana")?;
    if day_of_week[7] {
        day_of_week[0] = true;
    }
    day_of_week.truncate(7);
    Ok(CronExpr {
        minute,
        hour,
        day_of_month,
        month,
        day_of_week,
        dom_restricted: fields[2] != "*",
        dow_restricted: fields[4] != "*",
    })
}

impl CronExpr {
    /// El próximo instante (milisegundos desde epoch, en punto de minuto
    /// exacto) ESTRICTAMENTE posterior a `now_ms` que matchea esta
    /// expresión. Avanza minuto a minuto -- barato (un lookup en 5 tablas
    /// chicas por candidato) y simple de probar contra vectores de
    /// referencia conocidos, sin necesitar aritmética de calendario más
    /// lista que sumar un minuto. Tope de 4 años de minutos para nunca
    /// colgarse en un horario imposible (ej. día 31 fijo en un mes sin
    /// día-de-semana de respaldo) -- devuelve `None` en ese caso extremo en
    /// vez de bloquear el hilo del scheduler para siempre.
    pub fn next_run_after(&self, now_ms: i64) -> Option<i64> {
        let start_min = now_ms.div_euclid(MS_PER_MIN) + 1;
        const MAX_MINUTES_AHEAD: i64 = 4 * 366 * 24 * 60;
        for offset in 0..MAX_MINUTES_AHEAD {
            let candidate_min = start_min + offset;
            let total_ms = candidate_min * MS_PER_MIN;
            let days = total_ms.div_euclid(MS_PER_DAY);
            let ms_of_day = total_ms.rem_euclid(MS_PER_DAY);
            let (y, m, d) = crate::runtime::timestamp::civil_from_days(days);
            let _ = y;
            let hour = (ms_of_day / MS_PER_HOUR) as usize;
            let minute = ((ms_of_day % MS_PER_HOUR) / MS_PER_MIN) as usize;
            // Domingo=0 -- 1970-01-01 (days=0) fue jueves (=4).
            let weekday = (days + 4).rem_euclid(7) as usize;
            let month_ok = self.month[(m - 1) as usize];
            if !month_ok {
                continue;
            }
            let day_ok = match (self.dom_restricted, self.dow_restricted) {
                (true, true) => self.day_of_month[(d - 1) as usize] || self.day_of_week[weekday],
                (true, false) => self.day_of_month[(d - 1) as usize],
                (false, true) => self.day_of_week[weekday],
                (false, false) => true,
            };
            if day_ok && self.hour[hour] && self.minute[minute] {
                return Some(total_ms);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_accepts_seconds_minutes_hours_days() {
        assert_eq!(parse_interval("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_interval("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_interval("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_interval("1d").unwrap(), Duration::from_secs(86400));
    }

    #[test]
    fn parse_interval_rejects_garbage() {
        assert!(parse_interval("").is_err());
        assert!(parse_interval("5").is_err());
        assert!(parse_interval("0m").is_err());
        assert!(parse_interval("5w").is_err());
        assert!(parse_interval("-5m").is_err());
    }

    // ---- expresión cron real (GRAMMAR.md §9.24 Fase 2 ítem F1) ----

    fn ms(iso: &str) -> i64 {
        crate::runtime::timestamp::parse_iso8601_millis(iso).unwrap()
    }
    fn iso(total_ms: i64) -> String {
        crate::runtime::timestamp::format_iso8601_millis(total_ms)
    }

    #[test]
    fn parse_schedule_detects_expression_vs_interval_by_the_presence_of_a_space() {
        assert!(matches!(parse_schedule("5m").unwrap(), Schedule::Interval(_)));
        assert!(matches!(parse_schedule("0 4 * * *").unwrap(), Schedule::Expression(_)));
    }

    // Vectores generados con `croniter` de Python (implementación de
    // referencia ampliamente usada) -- no inventados a mano, mismo criterio
    // que los vectores de PBKDF2 (GRAMMAR.md §3.284): comparar contra una
    // implementación de referencia real es lo único que prueba que esto
    // funciona, no solo que es consistente consigo mismo.
    #[test]
    fn daily_at_four_am_matches_croniter() {
        let expr = match parse_schedule("0 4 * * *").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T00:00:00.000Z")).unwrap()), "2026-09-08T04:00:00.000Z");
        // Arrancando JUSTO en el instante de un match, el próximo es el
        // SIGUIENTE -- estrictamente posterior, nunca el mismo instante.
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T04:00:00.000Z")).unwrap()), "2026-09-09T04:00:00.000Z");
    }

    #[test]
    fn step_expression_every_fifteen_minutes_matches_croniter() {
        let expr = match parse_schedule("*/15 * * * *").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T10:07:00.000Z")).unwrap()), "2026-09-08T10:15:00.000Z");
    }

    #[test]
    fn fixed_day_of_month_matches_croniter() {
        let expr = match parse_schedule("30 8 1 * *").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T00:00:00.000Z")).unwrap()), "2026-10-01T08:30:00.000Z");
    }

    #[test]
    fn day_of_week_only_matches_croniter() {
        let expr = match parse_schedule("0 0 * * 1").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T00:00:00.000Z")).unwrap()), "2026-09-14T00:00:00.000Z");
    }

    #[test]
    fn comma_list_of_days_of_month_matches_croniter() {
        let expr = match parse_schedule("0 0 1,15 * *").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T00:00:00.000Z")).unwrap()), "2026-09-15T00:00:00.000Z");
    }

    #[test]
    fn range_of_weekdays_matches_croniter() {
        let expr = match parse_schedule("0 9 * * 1-5").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        // 2026-09-11 es viernes -- matchea el MISMO día a las 9, no salta
        // al lunes siguiente.
        assert_eq!(iso(expr.next_run_after(ms("2026-09-11T00:00:00.000Z")).unwrap()), "2026-09-11T09:00:00.000Z");
    }

    #[test]
    fn february_29th_only_matches_a_real_leap_year_matches_croniter() {
        let expr = match parse_schedule("0 0 29 2 *").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        // 2026 y 2027 no son bisiestos -- tiene que saltar directo a 2028,
        // no fallar ni colgarse buscando un 29 de febrero que no existe.
        assert_eq!(iso(expr.next_run_after(ms("2026-01-01T00:00:00.000Z")).unwrap()), "2028-02-29T00:00:00.000Z");
    }

    #[test]
    fn day_of_month_and_day_of_week_combine_with_or_not_and() {
        // Semántica estándar de cron: con LOS DOS campos restringidos, un
        // día matchea si CUALQUIERA de los dos lo acepta. "1 de cualquier
        // mes O lunes" tiene que matchear un lunes que NO es el día 1.
        let expr = match parse_schedule("0 0 1 * 1").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        // 2026-09-08 es martes; el próximo día que matchea (1 del mes O
        // lunes) es el lunes 2026-09-14, antes que el 1 de octubre.
        assert_eq!(iso(expr.next_run_after(ms("2026-09-08T00:00:00.000Z")).unwrap()), "2026-09-14T00:00:00.000Z");
    }

    #[test]
    fn rejects_a_schedule_without_exactly_five_fields() {
        assert!(parse_schedule("0 4 * *").is_err());
        assert!(parse_schedule("0 4 * * * *").is_err());
    }

    #[test]
    fn rejects_an_out_of_range_field_value() {
        assert!(parse_schedule("60 4 * * *").is_err(), "minuto 60 no existe");
        assert!(parse_schedule("0 24 * * *").is_err(), "hora 24 no existe");
        assert!(parse_schedule("0 4 32 * *").is_err(), "día 32 no existe");
        assert!(parse_schedule("0 4 * 13 *").is_err(), "mes 13 no existe");
    }

    #[test]
    fn rejects_a_zero_step() {
        assert!(parse_schedule("*/0 * * * *").is_err());
    }

    #[test]
    fn day_of_week_seven_is_an_alias_for_sunday() {
        // Las dos convenciones reales de cron para domingo (0 y 7) tienen
        // que dar el MISMO resultado.
        let expr_0 = match parse_schedule("0 0 * * 0").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        let expr_7 = match parse_schedule("0 0 * * 7").unwrap() {
            Schedule::Expression(e) => e,
            _ => panic!(),
        };
        let after = ms("2026-09-08T00:00:00.000Z");
        assert_eq!(expr_0.next_run_after(after), expr_7.next_run_after(after));
    }
}
