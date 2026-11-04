//! A kept version's time (the relay's UTC ISO 8601) in the person's own time zone.

/// Minutes to add to UTC for local time at Unix time `secs` (east of Greenwich is positive).
fn offset_minutes(secs: i64) -> i32 {
    #[cfg(not(target_arch = "wasm32"))]
    let minutes = {
        use chrono::{Local, Offset, TimeZone};
        Local.timestamp_opt(secs, 0).earliest().map_or(0, |t| t.offset().fix().local_minus_utc() / 60)
    };
    // The browser knows the zone: `getTimezoneOffset` is UTC minus local time, in minutes.
    #[cfg(target_arch = "wasm32")]
    let minutes = {
        let d = js_sys::Date::new_0();
        let offset = if d.set_time(secs as f64 * 1000.0).is_finite() { d.get_timezone_offset() } else { 0.0 };
        if offset.is_finite() { -(offset as i32) } else { 0 }
    };
    // Real zones are within a day of UTC.
    minutes.clamp(-24 * 60, 24 * 60)
}

/// `YYYY-MM-DDTHH:MM[:SS[.fff]](Z|±HH:MM)` as `YYYY-MM-DD HH:MM` in local time; empty when it
/// isn't such a time.
pub fn utc_time(iso: &str) -> String {
    utc_time_at(iso, offset_minutes)
}

pub(crate) fn utc_time_at(iso: &str, offset: impl Fn(i64) -> i32) -> String {
    let Some(utc) = parse(iso) else { return String::new() };
    let local = utc + i64::from(offset(utc)) * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let s = local.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", s / 3600, s / 60 % 60)
}

/// Seconds since the Unix epoch, UTC.
fn parse(iso: &str) -> Option<i64> {
    let iso = iso.trim();
    let digits = |s: &str| -> Option<u32> { if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None } };
    let num = |a: usize, b: usize| iso.get(a..b).and_then(digits);
    let (y, mo, d, h, mi) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?);
    let sep = |i: usize, c: &str| iso.get(i..i + 1) == Some(c);
    if !(sep(4, "-") && sep(7, "-") && (sep(10, "T") || sep(10, " ")) && sep(13, ":")) {
        return None;
    }
    if y == 0 || !(1..=12).contains(&mo) || d == 0 || d > days_in_month(y, mo) || h > 23 || mi > 59 {
        return None;
    }
    let mut rest = iso.get(16..)?;
    let mut s = 0;
    if let Some(r) = rest.strip_prefix(':') {
        s = r.get(0..2).and_then(digits).filter(|s| *s <= 59)?;
        rest = r.get(2..)?;
        if let Some(r) = rest.strip_prefix('.') {
            rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    let zone = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let hm: String = rest.get(1..)?.chars().filter(|c| *c != ':').collect();
            let (zh, zm) = (hm.get(0..2).and_then(digits)?, hm.get(2..4).and_then(digits)?);
            if hm.len() != 4 || zh > 18 || zm > 59 {
                return None;
            }
            sign * i64::from(zh * 60 + zm)
        }
    };
    let wall = days_from_civil(i64::from(y), mo, d) * 86_400 + i64::from(h * 3600 + mi * 60 + s);
    Some(wall - zone * 60)
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `z` days after 1970-01-01 (the inverse of [`days_from_civil`]).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip() {
        for days in [-800_000, -1, 0, 1, 19_000, 20_376, 20_738, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}: {y}-{m}-{d}");
        }
        assert_eq!(civil_from_days(20_738), (2026, 10, 12));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn a_utc_time_shows_in_local_time() {
        assert_eq!(utc_time_at("2026-10-29T21:20:05.123Z", |_| 4 * 60), "2026-10-30 01:20");
        assert_eq!(utc_time_at("2026-10-29T21:20:05Z", |_| 0), "2026-10-29 21:20");
        assert_eq!(utc_time_at("2026-10-12T02:05Z", |_| -4 * 60), "2026-10-11 22:05");
        assert_eq!(utc_time_at("2026-10-12T09:00:00+05:30", |_| 0), "2026-10-12 03:30");
        assert_eq!(utc_time_at("2028-02-29T12:00:00Z", |_| 0), "2028-02-29 12:00");
    }

    #[test]
    fn what_isnt_a_utc_time_is_empty() {
        for bad in ["bad", "", "2026-10-29", "2026-10-29T21:20:05", "2026-02-30T00:00Z", "2026-10-29T24:00Z", "2026-10-29T21:20:05+5"] {
            assert_eq!(utc_time_at(bad, |_| 0), "", "{bad}");
        }
    }
}
