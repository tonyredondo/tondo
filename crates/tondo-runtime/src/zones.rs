//! The time zones of `std.time` and the names of months and days in each
//! language (standard library 14.3.5), which `std/datetime.to` and
//! `std/timetext.to` are written on. Zones come from the system's tz
//! database and, for one it lacks, from the copy jiff bundles;
//! `TONDO_ZONES=builtin` takes them all from that copy. A zone is known to
//! Tondo by its name, which the runtime keeps compiled.

use super::{ArrayHeader, Str, element, str_bytes, str_new};
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::{Offset, TimeZone, TimeZoneDatabase};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

fn text<'a>(s: *const Str) -> &'a str {
    std::str::from_utf8(unsafe { str_bytes(s) }).unwrap_or_default()
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

/// The offset a fixed zone's name writes: `+05:30`, `-03:00`, with seconds
/// when it has them.
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

/// The names of a locale, as `std/timetext.to` reads them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_time_names(locale: *const Str) -> *mut ArrayHeader {
    crate::http::string_array(&names(text(locale)).0)
}

/// The abbreviation of a zone at a time in seconds since 1970, as `CET`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_zone_abbreviation(name: *const Str, seconds: i64) -> *mut Str {
    let at = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    str_new(zone_or_utc(text(name)).to_offset_info(at).abbreviation().as_bytes())
}
