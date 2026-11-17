//! XMLTV timestamps as plain unix seconds.
//!
//! The core never reads a clock: callers pass `now` wherever it matters.

/// Seconds since 1970-01-01T00:00:00Z.
pub type UnixTime = i64;

/// Seconds in a day, for retention arithmetic.
pub const DAY: i64 = 86_400;

/// How XMLTV times of one source are turned into [`UnixTime`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimePolicy {
    /// Offset east of UTC, in minutes, assumed when a timestamp has none.
    /// `0` treats such times as UTC.
    pub assumed_offset_minutes: i32,
    /// Per-source correction added to every parsed time, in minutes (the
    /// "EPG offset" setting of the reference UX). Positive moves programmes
    /// later.
    pub shift_minutes: i32,
}

impl TimePolicy {
    /// Times without an offset are UTC; no shift.
    pub const UTC: TimePolicy = TimePolicy {
        assumed_offset_minutes: 0,
        shift_minutes: 0,
    };
}

/// Parses an XMLTV time: `YYYYMMDD[hh[mm[ss]]]` followed by an optional
/// offset (`+hhmm`, `-hhmm`, `+hh:mm`, `+hh`, `Z`, `UTC`, `GMT`), with or
/// without a space in between. Returns `None` for anything else or for an
/// impossible calendar value.
pub fn parse_xmltv_time(s: &str, policy: TimePolicy) -> Option<UnixTime> {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if !matches!(digits, 8 | 10 | 12 | 14) {
        return None;
    }
    let num = |from: usize, len: usize| -> u32 {
        b[from..from + len]
            .iter()
            .fold(0, |acc, d| acc * 10 + u32::from(d - b'0'))
    };
    let year = i64::from(num(0, 4));
    let month = num(4, 2);
    let day = num(6, 2);
    let hour = if digits >= 10 { num(8, 2) } else { 0 };
    let minute = if digits >= 12 { num(10, 2) } else { 0 };
    // A leap second is folded into the preceding second.
    let second = if digits >= 14 { num(12, 2) } else { 0 };
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let second = second.min(59);
    let offset = match parse_offset(s[digits..].trim_start())? {
        Some(minutes) => minutes,
        None => i64::from(policy.assumed_offset_minutes),
    };
    let local = days_from_civil(year, month, day) * DAY
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    Some(local - offset * 60 + i64::from(policy.shift_minutes) * 60)
}

/// `Some(None)`: no offset present. `Some(Some(m))`: offset of `m` minutes
/// east of UTC. `None`: garbage.
fn parse_offset(s: &str) -> Option<Option<i64>> {
    if s.is_empty() {
        return Some(None);
    }
    if s.eq_ignore_ascii_case("z") || s.eq_ignore_ascii_case("utc") || s.eq_ignore_ascii_case("gmt")
    {
        return Some(Some(0));
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest: Vec<u8> = s[1..].bytes().filter(|&c| c != b':').collect();
    if !rest.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let (h, m) = match rest.len() {
        2 => (&rest[..2], &b"00"[..]),
        4 => (&rest[..2], &rest[2..]),
        _ => return None,
    };
    let two = |d: &[u8]| i64::from(d[0] - b'0') * 10 + i64::from(d[1] - b'0');
    let (h, m) = (two(h), two(m));
    if h > 14 || m > 59 {
        return None;
    }
    Some(Some(sign * (h * 60 + m)))
}

/// Formats as `YYYYMMDDhhmmss +0000` (UTC). Handy for writing fixtures.
pub fn format_xmltv_time(t: UnixTime) -> String {
    let days = t.div_euclid(DAY);
    let secs = t.rem_euclid(DAY);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since the epoch for a proleptic Gregorian date (H. Hinnant).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: TimePolicy = TimePolicy::UTC;
    // 2026-03-01T12:30:45Z
    const T: i64 = 1_772_368_245;

    #[test]
    fn full_form_with_offsets() {
        assert_eq!(parse_xmltv_time("20260301123045 +0000", UTC), Some(T));
        assert_eq!(parse_xmltv_time("20260301143045 +0200", UTC), Some(T));
        assert_eq!(parse_xmltv_time("20260301073045 -0500", UTC), Some(T));
        assert_eq!(parse_xmltv_time("20260301180045 +05:30", UTC), Some(T));
        assert_eq!(parse_xmltv_time("20260301143045+02", UTC), Some(T));
        assert_eq!(parse_xmltv_time("20260301123045Z", UTC), Some(T));
        assert_eq!(parse_xmltv_time("  20260301123045 UTC ", UTC), Some(T));
    }

    #[test]
    fn truncated_forms() {
        assert_eq!(parse_xmltv_time("202603011230 +0000", UTC), Some(T - 45));
        assert_eq!(parse_xmltv_time("2026030112", UTC), Some(T - 30 * 60 - 45));
        assert_eq!(
            parse_xmltv_time("20260301", UTC),
            Some(T - 12 * 3600 - 30 * 60 - 45)
        );
    }

    #[test]
    fn missing_offset_uses_policy_and_shift_applies_on_top() {
        assert_eq!(parse_xmltv_time("20260301123045", UTC), Some(T));
        let cet = TimePolicy {
            assumed_offset_minutes: 60,
            shift_minutes: 0,
        };
        assert_eq!(parse_xmltv_time("20260301133045", cet), Some(T));
        // An explicit offset beats the assumed one.
        assert_eq!(parse_xmltv_time("20260301123045 +0000", cet), Some(T));
        let shifted = TimePolicy {
            assumed_offset_minutes: 60,
            shift_minutes: -90,
        };
        assert_eq!(
            parse_xmltv_time("20260301133045", shifted),
            Some(T - 90 * 60)
        );
        assert_eq!(
            parse_xmltv_time("20260301123045 +0000", shifted),
            Some(T - 90 * 60)
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "2026",
            "202603011",
            "20261301000000 +0000",
            "20260230000000 +0000",
            "20260301250000 +0000",
            "20260301126000 +0000",
            "20260301120000 +2500",
            "20260301120000 +01x0",
            "20260301120000 EST",
            "abcd0301120000",
        ] {
            assert_eq!(parse_xmltv_time(s, UTC), None, "{s:?}");
        }
        assert!(parse_xmltv_time("20240229000000 +0000", UTC).is_some());
        assert!(parse_xmltv_time("20250229000000 +0000", UTC).is_none());
    }

    #[test]
    fn civil_round_trip() {
        for t in [0, T, -86_400 * 365 * 30 + 17, 4_102_444_800, 951_782_400] {
            let s = format_xmltv_time(t);
            assert_eq!(parse_xmltv_time(&s, UTC), Some(t), "{s}");
        }
        assert_eq!(format_xmltv_time(T), "20260301123045 +0000");
    }
}

#[cfg(test)]
mod seconds_regression {
    use super::*;
    #[test]
    fn validates_seconds_before_folding_only_the_leap_second() {
        let end = parse_xmltv_time("20260301123059 +0000", TimePolicy::UTC).unwrap();
        assert_eq!(
            parse_xmltv_time("20260301123060 +0000", TimePolicy::UTC),
            Some(end)
        );
        for seconds in [61, 99] {
            assert_eq!(
                parse_xmltv_time(&format!("202603011230{seconds} +0000"), TimePolicy::UTC),
                None
            );
        }
    }
}
