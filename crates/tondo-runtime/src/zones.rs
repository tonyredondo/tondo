//! Time zones, patterns and durations of `std.time` (standard library
//! 14.3.1, 14.3.5), which `std/datetime.to` is written on. Zones come from
//! the system's tz database and, for one it lacks, from the copy jiff
//! bundles; `TONDO_ZONES=builtin` takes them all from that copy. A zone
//! is known to Tondo by its name, which the runtime keeps compiled.

use super::pattern::{Directive, Piece, pieces};
use super::{ArrayHeader, Str, element, str_bytes, str_new};
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::{Offset, TimeZone, TimeZoneDatabase};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

fn text<'a>(s: *const Str) -> &'a str {
    std::str::from_utf8(unsafe { str_bytes(s) }).unwrap_or_default()
}

fn ints(values: &[i64]) -> *mut ArrayHeader {
    let array = super::tondo_rt_array_new(8, values.len());
    for (index, &value) in values.iter().enumerate() {
        unsafe { *(element(array, 8, index) as *mut i64) = value };
    }
    array
}

fn ints_of(array: *mut ArrayHeader) -> Vec<i64> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { (0..(*array).len).map(|i| *(element(array, 8, i) as *const i64)).collect() }
}

// ---------------------------------------------------------------------------
// Zones
// ---------------------------------------------------------------------------

/// Whether every zone comes from the copy bundled with the program.
static BUILTIN: LazyLock<bool> = LazyLock::new(|| std::env::var("TONDO_ZONES").is_ok_and(|v| v == "builtin"));
static SYSTEM: LazyLock<TimeZoneDatabase> = LazyLock::new(TimeZoneDatabase::from_env);
static BUNDLED: LazyLock<TimeZoneDatabase> = LazyLock::new(TimeZoneDatabase::bundled);
/// The zones found, by their names.
static ZONES: LazyLock<Mutex<HashMap<String, TimeZone>>> = LazyLock::new(Mutex::default);

/// A fixed offset's name: `+05:30`, `-03:00`, with seconds when it has
/// them.
fn offset_name(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let seconds = seconds.unsigned_abs();
    let (hours, minutes, rest) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match rest {
        0 => format!("{sign}{hours:02}:{minutes:02}"),
        _ => format!("{sign}{hours:02}:{minutes:02}:{rest:02}"),
    }
}

/// The offset a name of `offset_name` writes.
fn named_offset(name: &str) -> Option<i32> {
    let (sign, rest) = match name.as_bytes().first()? {
        b'+' => (1, &name[1..]),
        b'-' => (-1, &name[1..]),
        _ => return None,
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let number = |index: usize| parts.get(index).map_or(Some(0), |p| p.parse::<i32>().ok());
    let (hours, minutes, seconds) = (number(0)?, number(1)?, number(2)?);
    (hours < 26 && minutes < 60 && seconds < 60).then_some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// The zone of a name, from the cache or a database.
fn zone(name: &str) -> Option<TimeZone> {
    let mut zones = ZONES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = zones.get(name) {
        return Some(found.clone());
    }
    let found = if name.eq_ignore_ascii_case("UTC") {
        TimeZone::UTC
    } else if let Some(seconds) = named_offset(name) {
        TimeZone::fixed(Offset::from_seconds(seconds).ok()?)
    } else if *BUILTIN {
        BUNDLED.get(name).ok()?
    } else {
        SYSTEM.get(name).or_else(|_| BUNDLED.get(name)).ok()?
    };
    zones.insert(name.to_string(), found.clone());
    Some(found)
}

/// A zone a time is in, which the zone of its name gives: UTC when the
/// name is not a zone's, which `std.time` never makes.
fn zone_or_utc(name: &str) -> TimeZone {
    zone(name).unwrap_or(TimeZone::UTC)
}

/// `time.zone`: whether the name is a zone's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_zone_known(name: *const Str) -> u8 {
    zone(text(name)).is_some() as u8
}

/// `time.localZone`: the name of the zone `TZ` names, or `/etc/localtime`
/// is, or `UTC`; `Local` for a zone the system has without a name.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_zone_local() -> *mut Str {
    static LOCAL: LazyLock<String> = LazyLock::new(|| {
        let named = std::env::var("TZ").ok().map(|tz| tz.trim_start_matches(':').to_string());
        let linked = std::fs::read_link("/etc/localtime").ok().and_then(|target| {
            let target = target.to_str()?.to_string();
            Some(target[target.find("zoneinfo/")? + "zoneinfo/".len()..].to_string())
        });
        for name in named.iter().chain(&linked) {
            if !name.is_empty() && zone(name).is_some() {
                return name.clone();
            }
        }
        if named.is_none()
            && let Ok(data) = std::fs::read("/etc/localtime")
            && let Ok(local) = TimeZone::tzif("Local", &data)
        {
            ZONES.lock().unwrap_or_else(|e| e.into_inner()).insert("Local".into(), local);
            return "Local".into();
        }
        "UTC".into()
    });
    str_new(LOCAL.as_bytes())
}

/// The zone's offset from UTC at a time, in seconds since 1970.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_zone_offset(name: *const Str, seconds: i64) -> i64 {
    let at = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    zone_or_utc(text(name)).to_offset(at).seconds() as i64
}

/// The time, in seconds since 1970, when the zone's clocks show a date and
/// time: one they skip moves on by what they skip, and one they show twice
/// is the first. `i64::MIN` for a date that does not exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_zone_resolve(name: *const Str, fields: *mut ArrayHeader) -> i64 {
    let fields = ints_of(fields);
    let Some(civil) = civil(&fields) else { return i64::MIN };
    match zone_or_utc(text(name)).to_ambiguous_timestamp(civil).compatible() {
        Ok(at) => at.as_second(),
        Err(_) => i64::MIN,
    }
}

/// A date and time of the fields `[year, month, day, hour, minute, second,
/// nanosecond, ...]`.
fn civil(fields: &[i64]) -> Option<DateTime> {
    let field = |index: usize| fields.get(index).copied().unwrap_or(0);
    let small = |index: usize| i8::try_from(field(index)).ok();
    DateTime::new(
        i16::try_from(field(0)).ok()?,
        small(1)?,
        small(2)?,
        small(3)?,
        small(4)?,
        small(5)?,
        i32::try_from(field(6)).ok()?,
    )
    .ok()
}

// ---------------------------------------------------------------------------
// Names in each language
// ---------------------------------------------------------------------------

/// What `data/names.txt` has for a language: 12 months, 12 abbreviated,
/// 12 alone, 7 days from Sunday, 7 abbreviated, AM and PM.
struct Names(Vec<String>);

impl Names {
    fn month(&self, month: i64, form: usize) -> &str {
        &self.0[form * 12 + (month as usize).clamp(1, 12) - 1]
    }
    fn weekday(&self, day: usize, short: bool) -> &str {
        &self.0[36 + 7 * short as usize + day]
    }
    fn meridiem(&self, pm: bool) -> &str {
        &self.0[50 + pm as usize]
    }
}

static NAMES: LazyLock<HashMap<String, Names>> = LazyLock::new(|| {
    use std::io::Read;
    let packed = include_bytes!(concat!(env!("OUT_DIR"), "/names.txt.gz"));
    let mut table = String::new();
    let _ = flate2::read::GzDecoder::new(&packed[..]).read_to_string(&mut table);
    table
        .lines()
        .filter_map(|line| {
            let (tag, names) = line.split_once('\t')?;
            let names: Vec<String> = names.split_terminator('|').map(String::from).collect();
            (names.len() == 52).then(|| (tag.to_ascii_lowercase(), Names(names)))
        })
        .collect()
});

/// The names of a BCP 47 tag, of its language, or English's.
fn names(locale: &str) -> &'static Names {
    let tag = locale.replace('_', "-").to_ascii_lowercase();
    let language = tag.split('-').next().unwrap_or_default();
    NAMES.get(&tag).or_else(|| NAMES.get(language)).or_else(|| NAMES.get("en")).expect("the names of English")
}

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

/// `format`: the fields `[year, month, day, hour, minute, second,
/// nanosecond, offset]` as the pattern writes them, with the names of
/// `locale`; `zone` names the zone for `%Z`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_format(
    pattern: *const Str,
    locale: *const Str,
    fields: *mut ArrayHeader,
    zone: *const Str,
) -> *mut Str {
    use Directive as D;
    let fields = ints_of(fields);
    let field = |index: usize| fields.get(index).copied().unwrap_or(0);
    let names = names(text(locale));
    let (year, month, day, hour, offset) = (field(0), field(1), field(2), field(3), field(7));
    let date = Date::new(year as i16, month as i8, day as i8).ok();
    let since = civil(&fields).and_then(|c| c.to_zoned(TimeZone::UTC).ok()).map(|z| z.timestamp().as_second() - offset);
    let mut out = String::new();
    // A pattern the compiler did not see writes what it cannot read as is.
    let Ok(pieces) = pieces(text(pattern)) else {
        return str_new(unsafe { str_bytes(pattern) });
    };
    for piece in pieces {
        let (directive, padded) = match piece {
            Piece::Text(c) => {
                out.push(c);
                continue;
            }
            Piece::Directive(directive, padded) => (directive, padded),
        };
        let number = |out: &mut String, value: i64, width: usize, fill: char| {
            if padded {
                let digits = value.abs().to_string();
                if value < 0 {
                    out.push('-');
                }
                out.extend(std::iter::repeat_n(fill, width.saturating_sub(digits.len())));
                out.push_str(&digits);
            } else {
                out.push_str(&value.to_string());
            }
        };
        match directive {
            D::Year => number(&mut out, year, 4, '0'),
            D::ShortYear => number(&mut out, year.rem_euclid(100), 2, '0'),
            D::Month => number(&mut out, month, 2, '0'),
            D::MonthName => out.push_str(names.month(month, 0)),
            D::MonthShort => out.push_str(names.month(month, 1)),
            D::MonthAlone => out.push_str(names.month(month, 2)),
            D::Day => number(&mut out, day, 2, '0'),
            D::DaySpaced => number(&mut out, day, 2, ' '),
            D::DayOfYear => number(&mut out, date.map_or(0, |d| d.day_of_year() as i64), 3, '0'),
            D::Weekday | D::WeekdayShort => {
                let weekday = date.map_or(0, |d| d.weekday().to_sunday_zero_offset() as usize);
                out.push_str(names.weekday(weekday, directive == D::WeekdayShort));
            }
            D::Hour => number(&mut out, hour, 2, '0'),
            D::Hour12 => number(&mut out, (hour + 11) % 12 + 1, 2, '0'),
            D::Meridiem => out.push_str(names.meridiem(hour >= 12)),
            D::Minute => number(&mut out, field(4), 2, '0'),
            D::Second => number(&mut out, field(5), 2, '0'),
            D::Fraction(digits) => {
                let nanos = format!("{:09}", field(6));
                out.push_str(&nanos[..digits as usize]);
            }
            D::Offset | D::OffsetColon => {
                let name = offset_name(offset as i32);
                out.push_str(&if directive == D::Offset { name.replace(':', "") } else { name });
            }
            D::Zone => {
                let name = text(zone);
                let abbreviation = match since.and_then(|s| Timestamp::from_second(s).ok()) {
                    _ if name == "UTC" => "UTC".to_string(),
                    Some(at) => zone_or_utc(name).to_offset_info(at).abbreviation().to_string(),
                    None => String::new(),
                };
                out.push_str(&abbreviation);
            }
            D::Seconds => out.push_str(&since.unwrap_or(0).to_string()),
        }
    }
    str_new(out.as_bytes())
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// The years whose times since 1970 fit in a `Duration`.
const YEARS: std::ops::RangeInclusive<i64> = 1678..=2261;

/// `parseDateTime` and `parseZoned` without a pattern: ISO 8601 in RFC
/// 3339's extended form, `2026-10-22`, `2026-10-22T06:38:28Z` or
/// `2026-10-22 08:38:28.5+02:00`, as `tondo_rt_time_parse` gives it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_iso(input: *const Str) -> *mut ArrayHeader {
    match iso(text(input)) {
        Some(fields) => ints(&fields),
        None => ints(&[0]),
    }
}

fn iso(text: &str) -> Option<Vec<i64>> {
    let mut rest = text;
    let take = |count: usize, rest: &mut &str| -> Option<i64> {
        let digits = rest.get(..count)?;
        let value = digits.bytes().all(|b| b.is_ascii_digit()).then(|| digits.parse().unwrap_or(0))?;
        *rest = &rest[count..];
        Some(value)
    };
    let year = take(4, &mut rest)?;
    rest = rest.strip_prefix('-')?;
    let month = take(2, &mut rest)?;
    rest = rest.strip_prefix('-')?;
    let day = take(2, &mut rest)?;
    let (mut hour, mut minute, mut second, mut nanos, mut offset) = (0, 0, 0, 0, None);
    if !rest.is_empty() {
        rest = rest.strip_prefix(['T', 't', ' '])?;
        hour = take(2, &mut rest)?;
        rest = rest.strip_prefix(':')?;
        minute = take(2, &mut rest)?;
        if let Some(after) = rest.strip_prefix(':') {
            rest = after;
            second = take(2, &mut rest)?;
            if let Some(after) = rest.strip_prefix('.') {
                let count = after.bytes().take_while(u8::is_ascii_digit).count();
                if count == 0 {
                    return None;
                }
                let digits = format!("{:0<9}", &after[..count.min(9)]);
                nanos = digits.parse().ok()?;
                rest = &after[count..];
            }
        }
        if let Some(after) = rest.strip_prefix(['Z', 'z']) {
            rest = after;
            offset = Some(0);
        } else if let Some(sign) = rest.strip_prefix('+').map(|_| 1).or_else(|| rest.strip_prefix('-').map(|_| -1)) {
            rest = &rest[1..];
            let hours = take(2, &mut rest)?;
            let minutes = match rest.strip_prefix(':') {
                Some(after) => {
                    rest = after;
                    take(2, &mut rest)?
                }
                None if !rest.is_empty() => take(2, &mut rest)?,
                None => 0,
            };
            if hours > 23 || minutes > 59 {
                return None;
            }
            offset = Some(sign * (hours * 3600 + minutes * 60));
        }
    }
    if !rest.is_empty() || !YEARS.contains(&year) {
        return None;
    }
    let fields = [year, month, day, hour, minute, second, nanos];
    civil(&fields)?;
    let mut out = vec![1];
    out.extend(fields);
    out.extend([offset.is_some() as i64, offset.unwrap_or(0)]);
    Some(out)
}

/// `parseDateTime` and `parseZoned` with a pattern: `[1, year, month, day,
/// hour, minute, second, nanosecond, has offset, offset]`, or `[0]` when
/// the text is not of the pattern's form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_parse(
    input: *const Str,
    pattern: *const Str,
    locale: *const Str,
) -> *mut ArrayHeader {
    match parse(text(input), text(pattern), names(text(locale))) {
        Some(fields) => ints(&fields),
        None => ints(&[0]),
    }
}

fn parse(input: &str, pattern: &str, names: &Names) -> Option<Vec<i64>> {
    use Directive as D;
    let pieces = pieces(pattern).ok()?;
    super::pattern::parsing(&pieces).ok()?;
    let mut rest = input;
    let (mut year, mut month, mut day, mut day_of_year) = (None, None, None, None);
    let (mut hour, mut minute, mut second, mut nanos, mut pm) = (0, 0, 0, 0, None);
    let (mut offset, mut seconds) = (None, None);
    for piece in pieces {
        let directive = match piece {
            Piece::Text(c) => {
                rest = rest.strip_prefix(c)?;
                continue;
            }
            Piece::Directive(directive, _) => directive,
        };
        match directive {
            D::Year => year = Some(digits(&mut rest, 1, 4)?),
            D::ShortYear => {
                let short = digits(&mut rest, 1, 2)?;
                year = Some(if short < 69 { 2000 + short } else { 1900 + short });
            }
            D::Month => month = Some(digits(&mut rest, 1, 2)?),
            D::MonthName | D::MonthShort | D::MonthAlone => {
                let all: Vec<(&str, i64)> =
                    (0..36).map(|index| (names.0[index].as_str(), index as i64 % 12 + 1)).collect();
                month = Some(name(&mut rest, &all)?);
            }
            D::Day => day = Some(digits(&mut rest, 1, 2)?),
            D::DaySpaced => {
                rest = rest.trim_start_matches(' ');
                day = Some(digits(&mut rest, 1, 2)?);
            }
            D::DayOfYear => day_of_year = Some(digits(&mut rest, 1, 3)?),
            D::Weekday | D::WeekdayShort => {
                let all: Vec<(&str, i64)> = (36..50).map(|index| (names.0[index].as_str(), 0)).collect();
                name(&mut rest, &all)?;
            }
            D::Hour | D::Hour12 => hour = digits(&mut rest, 1, 2)?,
            D::Meridiem => pm = Some(name(&mut rest, &[(names.0[50].as_str(), 0), (names.0[51].as_str(), 1)])? == 1),
            D::Minute => minute = digits(&mut rest, 1, 2)?,
            D::Second => second = digits(&mut rest, 1, 2)?,
            D::Fraction(width) => {
                let before = rest.len();
                let value = digits(&mut rest, 1, width as usize)?;
                let read = before - rest.len();
                nanos = value * 10i64.pow(9 - read as u32);
            }
            D::Offset | D::OffsetColon => {
                if let Some(after) = rest.strip_prefix('Z') {
                    rest = after;
                    offset = Some(0);
                    continue;
                }
                let sign = match rest.as_bytes().first()? {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                rest = &rest[1..];
                let hours = digits(&mut rest, 2, 2)?;
                rest = rest.strip_prefix(':').unwrap_or(rest);
                let minutes = digits(&mut rest, 2, 2)?;
                offset = Some(sign * (hours * 3600 + minutes * 60));
            }
            D::Zone => {
                let all = [("UTC", 0), ("GMT", 0)];
                offset = Some(name(&mut rest, &all)?);
            }
            D::Seconds => {
                let negative = rest.starts_with('-');
                if negative {
                    rest = &rest[1..];
                }
                let value = digits(&mut rest, 1, 18)?;
                seconds = Some(if negative { -value } else { value });
            }
        }
    }
    if !rest.is_empty() {
        return None;
    }
    if let Some(seconds) = seconds {
        let at = Timestamp::from_second(seconds).ok()?.to_zoned(TimeZone::UTC).datetime();
        let fields = [at.year() as i64, at.month() as i64, at.day() as i64, at.hour() as i64, at.minute() as i64];
        return Some(vec![1, fields[0], fields[1], fields[2], fields[3], fields[4], at.second() as i64, nanos, 1, 0]);
    }
    if let Some(pm) = pm {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    let year = year?;
    let (month, day) = match day_of_year {
        Some(ordinal) => {
            let date = Date::new(year as i16, 1, 1).ok()?.with().day_of_year(ordinal as i16).build().ok()?;
            (date.month() as i64, date.day() as i64)
        }
        None => (month?, day?),
    };
    let fields = [year, month, day, hour, minute, second, nanos];
    civil(&fields)?;
    if !YEARS.contains(&year) {
        return None;
    }
    let mut out = vec![1];
    out.extend(fields);
    out.extend([offset.is_some() as i64, offset.unwrap_or(0)]);
    Some(out)
}

/// From `min` to `max` digits at the start of `rest`.
fn digits(rest: &mut &str, min: usize, max: usize) -> Option<i64> {
    let count = rest.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if count < min {
        return None;
    }
    let value = rest[..count].parse().ok()?;
    *rest = &rest[count..];
    Some(value)
}

/// The value of the longest name at the start of `rest`, whatever its case.
fn name(rest: &mut &str, names: &[(&str, i64)]) -> Option<i64> {
    let (found, value) = names
        .iter()
        .filter(|(name, _)| {
            !name.is_empty() && rest.get(..name.len()).is_some_and(|start| start.to_lowercase() == name.to_lowercase())
        })
        .max_by_key(|(name, _)| name.len())?;
    *rest = &rest[found.len()..];
    Some(*value)
}

// ---------------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------------

/// `time.parseDuration`: `[1, nanoseconds]`, or `[0]` when the text is not
/// a duration or it does not fit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_duration_parse(input: *const Str) -> *mut ArrayHeader {
    match duration(text(input)) {
        Some(nanos) => ints(&[1, nanos]),
        None => ints(&[0]),
    }
}

fn duration(text: &str) -> Option<i64> {
    let (negative, mut rest) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if rest == "0" {
        return Some(0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: i128 = 0;
    while !rest.is_empty() {
        let whole_end = rest.bytes().take_while(u8::is_ascii_digit).count();
        let whole = &rest[..whole_end];
        rest = &rest[whole_end..];
        let fraction = match rest.strip_prefix('.') {
            Some(after) => {
                let end = after.bytes().take_while(u8::is_ascii_digit).count();
                rest = &after[end..];
                &after[..end]
            }
            None => "",
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        let units: [(&str, i128); 7] = [
            ("ns", 1),
            ("us", 1_000),
            ("µs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ];
        let (unit, scale) = units.iter().filter(|(u, _)| rest.starts_with(u)).max_by_key(|(u, _)| u.len())?;
        rest = &rest[unit.len()..];
        let whole: i128 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
        let mut part = whole.checked_mul(*scale)?;
        let mut divisor = 1i128;
        for digit in fraction.bytes().take(18) {
            divisor *= 10;
            part += (digit - b'0') as i128 * scale / divisor;
        }
        total = total.checked_add(part)?;
    }
    let total = if negative { -total } else { total };
    i64::try_from(total).ok()
}
