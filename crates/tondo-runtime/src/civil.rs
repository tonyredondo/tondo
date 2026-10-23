//! Dates and times as RFC 3339 text and as seconds and nanoseconds since
//! the Unix epoch, for the formats that hold them as numbers.

/// The days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

/// The civil date of a count of days from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let of_cycle = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * of_cycle + of_cycle / 4 - of_cycle / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    let year = of_cycle + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `2026-10-23T08:00:00.5Z`, as `time.DateTime` shows itself: the
/// fraction without trailing zeros, in UTC.
pub(crate) fn format(seconds: i64, nanos: u32) -> String {
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let mut text = format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60);
    if nanos > 0 {
        text.push_str(format!(".{nanos:09}").trim_end_matches('0'));
    }
    text.push('Z');
    text
}

/// The seconds and nanoseconds of `2026-10-23`, `2026-10-23T08:00:00Z` or
/// `2026-10-23 08:00:00.5+02:00`; a time without an offset is UTC.
pub(crate) fn parse(text: &str) -> Option<(i64, u32)> {
    let bytes = text.as_bytes();
    let number = |from: usize, count: usize| -> Option<i64> {
        let digits = bytes.get(from..from + count)?;
        digits.iter().all(u8::is_ascii_digit).then(|| digits.iter().fold(0, |n, &d| n * 10 + (d - b'0') as i64))
    };
    let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    if bytes.len() == 10 {
        return Some((seconds, 0));
    }
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    seconds += hour * 3600 + minute * 60 + second;
    let mut at = 19;
    let mut nanos = 0u32;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let digits = &text[start..at];
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        nanos = format!("{digits:0<9}").parse().ok()?;
    }
    match bytes.get(at) {
        None | Some(b'Z' | b'z') if at + 1 >= bytes.len() => {}
        Some(sign @ (b'+' | b'-')) if bytes.len() == at + 6 && bytes[at + 3] == b':' => {
            let offset = number(at + 1, 2)? * 3600 + number(at + 4, 2)? * 60;
            seconds -= if *sign == b'+' { offset } else { -offset };
        }
        _ => return None,
    }
    Some((seconds, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some((0, 0)));
        assert_eq!(parse("2026-10-23T10:00:00.5+02:00"), Some((1_792_742_400, 500_000_000)));
        assert_eq!(format(1_792_742_400, 500_000_000), "2026-10-23T08:00:00.5Z");
        assert_eq!(format(-1, 0), "1969-12-31T23:59:59Z");
        assert_eq!(parse("2026-10-23"), Some((1_792_713_600, 0)));
        assert_eq!(parse("2026-13-01"), None);
    }
}
