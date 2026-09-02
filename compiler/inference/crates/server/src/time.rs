//! RFC3339 UTC timestamp formatting, hand-rolled to avoid pulling in a date
//! library for the one cosmetic `created_at` field Ollama's API includes.
//! Epoch-seconds -> civil calendar date uses Howard Hinnant's well-known
//! `civil_from_days` algorithm (public domain, widely used — not something
//! this project is claiming novel credit for, just implementing directly
//! instead of adding a dependency for it).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_rfc3339() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    unix_seconds_to_rfc3339(secs)
}

pub fn unix_seconds_to_rfc3339(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let (hour, minute, second) = (time_of_day / 3600, (time_of_day / 60) % 60, time_of_day % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as i64; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as i64; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_epoch_values() {
        assert_eq!(unix_seconds_to_rfc3339(0), "1970-01-01T00:00:00Z");
        // 2000-01-01T00:00:00Z (a well-known epoch value, Y2K).
        assert_eq!(unix_seconds_to_rfc3339(946_684_800), "2000-01-01T00:00:00Z");
        // Cross-checked against `date -u -d @1783425600` rather than hand
        // arithmetic — an earlier draft of this test had a hand-computed
        // date off by 4 days, exactly the kind of silent error this test
        // exists to catch.
        assert_eq!(unix_seconds_to_rfc3339(1_783_425_600), "2026-07-07T12:00:00Z");
    }
}
