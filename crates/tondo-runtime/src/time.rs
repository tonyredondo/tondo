//! `std.time` (standard library 14.3): the monotonic clock, the system
//! clock in UTC, and how durations and dates display.

use super::Builder;
use std::io::Write;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// `time.now()`: nanoseconds of the monotonic clock since the program began.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_time_now() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as i64
}

/// `time.unixTime()`: nanoseconds since 1970-01-01T00:00:00Z.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_time_unix() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i64,
        Err(before) => -(before.duration().as_nanos() as i64),
    }
}

/// `time.utc(sinceEpoch)`: year, month, day, hour, minute, second and
/// nanosecond into `out`, in the proleptic Gregorian calendar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_utc(nanos: i64, out: *mut i64) {
    const DAY: i64 = 86_400 * 1_000_000_000;
    let (days, within) = (nanos.div_euclid(DAY), nanos.rem_euclid(DAY));
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let second = within / 1_000_000_000;
    let fields = [year, month, day, second / 3600, second / 60 % 60, second % 60, within % 1_000_000_000];
    unsafe { std::ptr::copy_nonoverlapping(fields.as_ptr(), out, fields.len()) };
}

/// `time.utcNow()`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_utc_now(out: *mut i64) {
    unsafe { tondo_rt_time_utc(tondo_rt_time_unix(), out) }
}

/// Digits of a fraction without its trailing zeros.
fn fraction(value: u64, width: usize) -> String {
    let digits = format!("{value:0width$}");
    digits.trim_end_matches('0').to_string()
}

/// A `Duration` as Go writes it: `1h2m3.5s`, `1.5s`, `250ms`, `1.5µs`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_duration(b: *mut Builder, nanos: i64) {
    let out = unsafe { &mut (*b).0 };
    if nanos == 0 {
        out.extend_from_slice(b"0s");
        return;
    }
    if nanos < 0 {
        out.push(b'-');
    }
    let n = nanos.unsigned_abs();
    let decimal = |out: &mut Vec<u8>, whole: u64, rest: u64, width: usize, unit: &str| {
        let rest = fraction(rest, width);
        let point = if rest.is_empty() { "" } else { "." };
        write!(out, "{whole}{point}{rest}{unit}").unwrap();
    };
    match n {
        0..1_000 => decimal(out, n, 0, 1, "ns"),
        1_000..1_000_000 => decimal(out, n / 1_000, n % 1_000, 3, "µs"),
        1_000_000..1_000_000_000 => decimal(out, n / 1_000_000, n % 1_000_000, 6, "ms"),
        _ => {
            let seconds = n / 1_000_000_000;
            let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
            if hours > 0 {
                write!(out, "{hours}h").unwrap();
            }
            if hours > 0 || minutes > 0 {
                write!(out, "{minutes}m").unwrap();
            }
            decimal(out, seconds % 60, n % 1_000_000_000, 9, "s");
        }
    }
}

/// A `DateTime` in RFC 3339, UTC.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_datetime(b: *mut Builder, fields: *const i64) {
    let out = unsafe { &mut (*b).0 };
    let f = unsafe { std::slice::from_raw_parts(fields, 7) };
    write!(out, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", f[0], f[1], f[2], f[3], f[4], f[5]).unwrap();
    let rest = fraction(f[6].clamp(0, 999_999_999) as u64, 9);
    if !rest.is_empty() {
        write!(out, ".{rest}").unwrap();
    }
    out.push(b'Z');
}
