//! The civil calendar over Unix milliseconds, in UTC.
//!
//! Hinnant's `days_from_civil` and its inverse: well-known, exact for every date this can
//! represent, and — the property that decides it here — pure arithmetic with no table behind it,
//! so `beck replay` cannot disagree with the run it is replaying because a time-zone database was
//! updated in between.

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `time_format` — an instant as RFC 3339 in UTC, to the millisecond.
pub fn format(ms: i64) -> String {
    // Floor division, so an instant before 1970 formats as the second it is in rather than the one
    // after it. `-1` is 1969-12-31T23:59:59.999Z, not 1970-01-01T00:00:00.-001Z.
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// `time_parse` — the inverse, or why the text is not an instant.
///
/// The message is here rather than at either call site because there are now two of them: the
/// evaluator raises `TimeError.BadTime` with it, and so does a compiled program.
pub fn parse(s: &str) -> Result<i64, String> {
    millis(s).ok_or_else(|| format!("`{s}` is not an RFC 3339 instant in UTC"))
}

fn millis(s: &str) -> Option<i64> {
    // `YYYY-MM-DDTHH:MM:SS[.mmm]Z`, UTC only. An offset is refused rather than silently shifted:
    // accepting `+01:00` would mean accepting that two spellings of the same instant are two
    // values, and a log is not the place to discover that.
    let b = s.as_bytes();
    if b.len() < 20 || (b[10] != b'T' && b[10] != b' ') || *b.last()? != b'Z' {
        return None;
    }
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let milli = match b[19] {
        b'.' => {
            let frac: String = s[20..s.len() - 1].chars().take(3).collect();
            if frac.is_empty() || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            format!("{frac:0<3}").parse::<i64>().ok()?
        }
        b'Z' if s.len() == 20 => 0,
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec) * 1000 + milli)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_round_trips_through_its_text() {
        for ms in [
            0,
            1,
            -1,
            1_700_000_000_000,
            -2_208_988_800_000,
            253_402_300_799_000,
        ] {
            assert_eq!(parse(&format(ms)), Ok(ms), "{ms}");
        }
    }

    #[test]
    fn the_epoch_and_the_second_before_it_are_the_dates_they_are() {
        assert_eq!(format(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn what_is_not_an_instant_says_so_with_the_text_in_it() {
        for not in ["", "nope", "1970-01-01", "1970-01-01T00:00:00+01:00"] {
            let why = parse(not).expect_err(not);
            assert!(why.contains(not), "{why:?} should quote {not:?}");
        }
    }
}
