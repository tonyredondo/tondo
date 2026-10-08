//! Pure Gregorian date, time and UTC values for the hosted `std.time` core.
//!
//! These kernels use no clocks, zones, locale or host handles. They establish
//! no native Tondo civil ABI or AOT lowering.

use std::fmt::{self, Write as _};

pub const NANOS_PER_DAY: i128 = 86_400_000_000_000;
pub const MAX_DATE_TEXT_BYTES: usize = 10;
pub const MAX_TIME_TEXT_BYTES: usize = 18;
pub const MAX_UTC_TEXT_BYTES: usize = 30;
const DAYS_BEFORE_MONTH: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
pub const ERROR_VARIANTS: &[&str] = &[
    "InvalidDate",
    "InvalidTime",
    "InvalidOffset",
    "InvalidZoneId",
    "ZoneUnavailable",
    "ZoneDataUnavailable",
    "NonexistentLocalTime",
    "AmbiguousLocalTime",
    "OutOfRange",
    "DomainMismatch",
    "Unavailable",
    "ResourceLimit",
];
pub const MONTH_POLICIES: &[&str] = &["Reject", "Clamp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CivilError {
    InvalidDate,
    InvalidTime,
    InvalidOffset,
    InvalidZoneId,
    ZoneUnavailable,
    ZoneDataUnavailable,
    NonexistentLocalTime,
    AmbiguousLocalTime,
    OutOfRange,
    DomainMismatch,
    Unavailable,
    ResourceLimit,
}

impl fmt::Display for CivilError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CivilError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonthPolicy {
    Reject,
    Clamp,
}

/// The finite pure calendar boundary; no zone or clock operations are registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CivilOperation {
    DateCreate,
    DateParse,
    DateYear,
    DateMonth,
    DateDay,
    DateDayOfWeek,
    DateDayOfYear,
    DateAddDays,
    DateAddMonths,
    DateAddYears,
    DateFormat,
    TimeCreate,
    TimeParse,
    TimeHour,
    TimeMinute,
    TimeSecond,
    TimeNanosecond,
    TimeFormat,
    UtcCreate,
    UtcParse,
    UtcDate,
    UtcTime,
    UtcAdd,
    UtcFormat,
}

impl CivilOperation {
    pub const ALL: [Self; 24] = [
        Self::DateCreate,
        Self::DateParse,
        Self::DateYear,
        Self::DateMonth,
        Self::DateDay,
        Self::DateDayOfWeek,
        Self::DateDayOfYear,
        Self::DateAddDays,
        Self::DateAddMonths,
        Self::DateAddYears,
        Self::DateFormat,
        Self::TimeCreate,
        Self::TimeParse,
        Self::TimeHour,
        Self::TimeMinute,
        Self::TimeSecond,
        Self::TimeNanosecond,
        Self::TimeFormat,
        Self::UtcCreate,
        Self::UtcParse,
        Self::UtcDate,
        Self::UtcTime,
        Self::UtcAdd,
        Self::UtcFormat,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::DateCreate => "std.time.Date.create",
            Self::DateParse => "std.time.Date.parse",
            Self::DateYear => "std.time.Date.year",
            Self::DateMonth => "std.time.Date.month",
            Self::DateDay => "std.time.Date.day",
            Self::DateDayOfWeek => "std.time.Date.dayOfWeek",
            Self::DateDayOfYear => "std.time.Date.dayOfYear",
            Self::DateAddDays => "std.time.Date.addDays",
            Self::DateAddMonths => "std.time.Date.addMonths",
            Self::DateAddYears => "std.time.Date.addYears",
            Self::DateFormat => "std.time.Date.format",
            Self::TimeCreate => "std.time.Time.create",
            Self::TimeParse => "std.time.Time.parse",
            Self::TimeHour => "std.time.Time.hour",
            Self::TimeMinute => "std.time.Time.minute",
            Self::TimeSecond => "std.time.Time.second",
            Self::TimeNanosecond => "std.time.Time.nanosecond",
            Self::TimeFormat => "std.time.Time.format",
            Self::UtcCreate => "std.time.UtcDateTime.create",
            Self::UtcParse => "std.time.UtcDateTime.parse",
            Self::UtcDate => "std.time.UtcDateTime.date",
            Self::UtcTime => "std.time.UtcDateTime.time",
            Self::UtcAdd => "std.time.UtcDateTime.add",
            Self::UtcFormat => "std.time.UtcDateTime.format",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }

    pub fn associated(owner: &str, member: &str) -> Option<Self> {
        Some(match (owner, member) {
            ("Date", "create") => Self::DateCreate,
            ("Date", "parse") => Self::DateParse,
            ("Time", "create") => Self::TimeCreate,
            ("Time", "parse") => Self::TimeParse,
            ("UtcDateTime", "create") => Self::UtcCreate,
            ("UtcDateTime", "parse") => Self::UtcParse,
            _ => return None,
        })
    }

    pub fn member(owner: &str, member: &str) -> Option<Self> {
        Some(match (owner, member) {
            ("Date", "year") => Self::DateYear,
            ("Date", "month") => Self::DateMonth,
            ("Date", "day") => Self::DateDay,
            ("Date", "dayOfWeek") => Self::DateDayOfWeek,
            ("Date", "dayOfYear") => Self::DateDayOfYear,
            ("Date", "addDays") => Self::DateAddDays,
            ("Date", "addMonths") => Self::DateAddMonths,
            ("Date", "addYears") => Self::DateAddYears,
            ("Date", "format") => Self::DateFormat,
            ("Time", "hour") => Self::TimeHour,
            ("Time", "minute") => Self::TimeMinute,
            ("Time", "second") => Self::TimeSecond,
            ("Time", "nanosecond") => Self::TimeNanosecond,
            ("Time", "format") => Self::TimeFormat,
            ("UtcDateTime", "date") => Self::UtcDate,
            ("UtcDateTime", "time") => Self::UtcTime,
            ("UtcDateTime", "add") => Self::UtcAdd,
            ("UtcDateTime", "format") => Self::UtcFormat,
            _ => return None,
        })
    }
}

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 => {
            if leap(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn digits(bytes: &[u8]) -> Option<i64> {
    bytes.iter().try_fold(0_i64, |number, byte| {
        byte.is_ascii_digit()
            .then(|| number * 10 + i64::from(byte - b'0'))
    })
}

fn year_start(year: u32) -> u32 {
    let prior = year - 1;
    365 * prior + prior / 4 - prior / 100 + prior / 400
}

fn render(value: impl fmt::Display, maximum: usize) -> String {
    let mut output = String::with_capacity(maximum);
    write!(output, "{value}").expect("writing to a String is infallible");
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    pub fn create(year: i64, month: i64, day: i64) -> Result<Self, CivilError> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=month_days(year, month)).contains(&day)
        {
            return Err(CivilError::InvalidDate);
        }
        Ok(Self {
            year: year as u16,
            month: month as u8,
            day: day as u8,
        })
    }
    pub fn parse(text: &str) -> Result<Self, CivilError> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(CivilError::InvalidDate);
        }
        Self::create(
            digits(&bytes[..4]).ok_or(CivilError::InvalidDate)?,
            digits(&bytes[5..7]).ok_or(CivilError::InvalidDate)?,
            digits(&bytes[8..]).ok_or(CivilError::InvalidDate)?,
        )
    }
    pub fn year(self) -> i64 {
        i64::from(self.year)
    }
    pub fn month(self) -> i64 {
        i64::from(self.month)
    }
    pub fn day(self) -> i64 {
        i64::from(self.day)
    }
    pub fn day_of_year(self) -> i64 {
        i64::from(DAYS_BEFORE_MONTH[usize::from(self.month - 1)])
            + self.day()
            + i64::from(self.month > 2 && leap(self.year()))
    }
    pub fn day_of_week(self) -> i64 {
        i64::from(self.ordinal() % 7) + 1
    }
    fn ordinal(self) -> u32 {
        year_start(u32::from(self.year)) + self.day_of_year() as u32 - 1
    }
    fn from_ordinal(ordinal: i128) -> Result<Self, CivilError> {
        if !(0..i128::from(year_start(10000))).contains(&ordinal) {
            return Err(CivilError::OutOfRange);
        }
        let ordinal = ordinal as u32;
        // At most fourteen comparisons for the finite normative year range.
        let (mut low, mut high) = (1, 10000);
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if year_start(middle) <= ordinal {
                low = middle;
            } else {
                high = middle;
            }
        }
        let mut remaining = ordinal - year_start(low);
        let mut month = 1_i64;
        while remaining >= month_days(i64::from(low), month) as u32 {
            remaining -= month_days(i64::from(low), month) as u32;
            month += 1;
        }
        Self::create(i64::from(low), month, i64::from(remaining) + 1)
    }
    pub fn add_days(self, amount: i64) -> Result<Self, CivilError> {
        Self::from_ordinal(i128::from(self.ordinal()) + i128::from(amount))
    }
    fn shift_months(self, amount: i128, policy: MonthPolicy) -> Result<Self, CivilError> {
        let index = i128::from(self.year() - 1) * 12 + i128::from(self.month() - 1) + amount;
        if !(0..9999 * 12).contains(&index) {
            return Err(CivilError::OutOfRange);
        }
        let year = (index / 12 + 1) as i64;
        let month = (index % 12 + 1) as i64;
        let last = month_days(year, month);
        let day = match policy {
            MonthPolicy::Reject => self.day(),
            MonthPolicy::Clamp => self.day().min(last),
        };
        Self::create(year, month, day)
    }
    pub fn add_months(self, amount: i64, policy: MonthPolicy) -> Result<Self, CivilError> {
        self.shift_months(i128::from(amount), policy)
    }
    pub fn add_years(self, amount: i64, policy: MonthPolicy) -> Result<Self, CivilError> {
        self.shift_months(i128::from(amount) * 12, policy)
    }
    pub fn format(self) -> String {
        render(self, MAX_DATE_TEXT_BYTES)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    nanos: u64,
}

impl Time {
    pub fn create(
        hour: i64,
        minute: i64,
        second: i64,
        nanosecond: i64,
    ) -> Result<Self, CivilError> {
        if !(0..24).contains(&hour)
            || !(0..60).contains(&minute)
            || !(0..60).contains(&second)
            || !(0..1_000_000_000).contains(&nanosecond)
        {
            return Err(CivilError::InvalidTime);
        }
        Ok(Self {
            nanos: ((hour * 3600 + minute * 60 + second) * 1_000_000_000 + nanosecond) as u64,
        })
    }
    pub fn parse(text: &str) -> Result<Self, CivilError> {
        let bytes = text.as_bytes();
        if !(8..=18).contains(&bytes.len())
            || bytes[2] != b':'
            || bytes[5] != b':'
            || (bytes.len() != 8 && (bytes.len() < 10 || bytes[8] != b'.'))
        {
            return Err(CivilError::InvalidTime);
        }
        let nanos = if bytes.len() == 8 {
            0
        } else {
            digits(&bytes[9..]).ok_or(CivilError::InvalidTime)?
                * 10_i64.pow((18 - bytes.len()) as u32)
        };
        Self::create(
            digits(&bytes[..2]).ok_or(CivilError::InvalidTime)?,
            digits(&bytes[3..5]).ok_or(CivilError::InvalidTime)?,
            digits(&bytes[6..8]).ok_or(CivilError::InvalidTime)?,
            nanos,
        )
    }
    pub fn hour(self) -> i64 {
        (self.nanos / 3_600_000_000_000) as i64
    }
    pub fn minute(self) -> i64 {
        (self.nanos / 60_000_000_000 % 60) as i64
    }
    pub fn second(self) -> i64 {
        (self.nanos / 1_000_000_000 % 60) as i64
    }
    pub fn nanosecond(self) -> i64 {
        (self.nanos % 1_000_000_000) as i64
    }
    pub fn format(self) -> String {
        render(self, MAX_TIME_TEXT_BYTES)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}",
            self.hour(),
            self.minute(),
            self.second()
        )?;
        let mut fraction = self.nanosecond();
        if fraction != 0 {
            let mut width = 9;
            while fraction % 10 == 0 {
                fraction /= 10;
                width -= 1;
            }
            write!(f, ".{fraction:0width$}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    date: Date,
    time: Time,
}

impl UtcDateTime {
    pub const fn create(date: Date, time: Time) -> Self {
        Self { date, time }
    }
    pub fn parse(text: &str) -> Result<Self, CivilError> {
        let bytes = text.as_bytes();
        if !(20..=30).contains(&bytes.len()) || bytes[10] != b'T' || bytes.last() != Some(&b'Z') {
            return Err(CivilError::InvalidDate);
        }
        // Byte-oriented validation before slicing prevents UTF-8 boundary panics.
        if !text.is_ascii() {
            return Err(CivilError::InvalidDate);
        }
        Ok(Self {
            date: Date::parse(&text[..10])?,
            time: Time::parse(&text[11..text.len() - 1])?,
        })
    }
    pub const fn date(self) -> Date {
        self.date
    }
    pub const fn time(self) -> Time {
        self.time
    }
    pub fn checked_add(self, nanoseconds: i64) -> Result<Self, CivilError> {
        let total = i128::from(self.time.nanos) + i128::from(nanoseconds);
        let date =
            Date::from_ordinal(i128::from(self.date.ordinal()) + total.div_euclid(NANOS_PER_DAY))?;
        let time = Time {
            nanos: total.rem_euclid(NANOS_PER_DAY) as u64,
        };
        Ok(Self { date, time })
    }
    pub fn format(self) -> String {
        render(self, MAX_UTC_TEXT_BYTES)
    }
}

impl fmt::Display for UtcDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}Z", self.date, self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_twenty_four_pure_operations_are_registered() {
        let mut names = std::collections::BTreeSet::new();
        let mut constructors = 0;
        for op in CivilOperation::ALL {
            assert!(names.insert(op.name()));
            assert_eq!(CivilOperation::from_name(op.name()), Some(op));
            let (owner, member) = op
                .name()
                .strip_prefix("std.time.")
                .unwrap()
                .split_once('.')
                .unwrap();
            if let Some(found) = CivilOperation::associated(owner, member) {
                assert_eq!(found, op);
                constructors += 1;
                assert_eq!(CivilOperation::member(owner, member), None);
            } else {
                assert_eq!(CivilOperation::member(owner, member), Some(op));
            }
        }
        assert_eq!(constructors, 6);
        for (owner, member) in [
            ("Date", "now"),
            ("DateTime", "create"),
            ("TimeZone", "id"),
            ("UtcDateTime", "inZone"),
            ("CivilClock", "now"),
            ("Time", "unknown"),
        ] {
            assert_eq!(CivilOperation::associated(owner, member), None);
            assert_eq!(CivilOperation::member(owner, member), None);
            assert_eq!(
                CivilOperation::from_name(&format!("std.time.{owner}.{member}")),
                None
            );
        }
    }

    #[test]
    fn gregorian_cycle_and_endpoints_roundtrip() {
        let start = Date::create(1600, 1, 1).unwrap();
        for delta in 0..146097 {
            let date = start.add_days(delta).unwrap();
            assert_eq!(date, Date::parse(&date.format()).unwrap());
            assert_eq!(
                date,
                Date::from_ordinal(i128::from(date.ordinal())).unwrap()
            );
            assert_eq!(
                date.day_of_week(),
                (start.day_of_week() - 1 + delta) % 7 + 1
            );
            assert!((1..=366).contains(&date.day_of_year()));
        }
        let first = Date::create(1, 1, 1).unwrap();
        let last = Date::create(9999, 12, 31).unwrap();
        assert_eq!(first.day_of_week(), 1);
        assert_eq!(first.add_days(-1), Err(CivilError::OutOfRange));
        assert_eq!(last.add_days(1), Err(CivilError::OutOfRange));
        for value in [i64::MIN, i64::MAX] {
            assert_eq!(first.add_days(value), Err(CivilError::OutOfRange));
            assert_eq!(
                first.add_months(value, MonthPolicy::Reject),
                Err(CivilError::OutOfRange)
            );
            assert_eq!(
                last.add_years(value, MonthPolicy::Clamp),
                Err(CivilError::OutOfRange)
            );
        }
    }

    #[test]
    fn month_policy_and_centuries_are_explicit() {
        for year in [100, 1900, 2100] {
            assert_eq!(Date::create(year, 2, 29), Err(CivilError::InvalidDate));
        }
        for year in [400, 1600, 2000, 2400] {
            assert!(Date::create(year, 2, 29).is_ok());
        }
        let date = Date::parse("2024-02-29").unwrap();
        assert_eq!(
            date.add_years(1, MonthPolicy::Reject),
            Err(CivilError::InvalidDate)
        );
        assert_eq!(
            date.add_years(1, MonthPolicy::Clamp).unwrap().format(),
            "2025-02-28"
        );
        let date = Date::parse("2024-01-31").unwrap();
        assert_eq!(
            date.add_months(1, MonthPolicy::Reject),
            Err(CivilError::InvalidDate)
        );
        assert_eq!(
            date.add_months(1, MonthPolicy::Clamp).unwrap().format(),
            "2024-02-29"
        );
    }

    #[test]
    fn strict_text_refuses_unicode_offsets_and_leap_seconds() {
        for text in [
            "",
            "0000-01-01",
            "10000-01-01",
            "2024-1-01",
            "2023-02-29",
            "2024/01/01",
            "2024-01-01\n",
            "２０２４-01-01",
            "2024-01-٠١",
        ] {
            assert_eq!(Date::parse(text), Err(CivilError::InvalidDate));
        }
        for text in [
            "",
            "24:00:00",
            "00:60:00",
            "23:59:60",
            "1:00:00",
            "12:00:00.",
            "12:00:00.1234567890",
            "12:00:00Z",
            "12:00:00.٠",
            "12:00:00\n",
        ] {
            assert_eq!(Time::parse(text), Err(CivilError::InvalidTime));
        }
        for text in [
            "2024-01-01T12:00:00",
            "2024-01-01t12:00:00Z",
            "2024-01-01T12:00:00z",
            "2024-01-01T12:00:00+00:00",
            "2024-01-01T12:00:00Z\n",
            "😀2024-01-01T00:00:00Z",
        ] {
            assert!(UtcDateTime::parse(text).is_err());
        }
        assert_eq!(
            Time::parse("01:02:03.123400000").unwrap().format(),
            "01:02:03.1234"
        );
        assert_eq!(
            Time::parse("01:02:03.000000000").unwrap().format(),
            "01:02:03"
        );
    }

    #[test]
    fn time_components_fraction_widths_and_constructor_bounds() {
        let value = Time::create(23, 59, 58, 999999999).unwrap();
        assert_eq!(
            (
                value.hour(),
                value.minute(),
                value.second(),
                value.nanosecond()
            ),
            (23, 59, 58, 999999999)
        );
        assert_eq!(value.format(), "23:59:58.999999999");
        for width in 1..=9 {
            let fraction = format!("{}1", "0".repeat(width - 1));
            let text = format!("01:02:03.{fraction}");
            let time = Time::parse(&text).unwrap();
            assert_eq!(time.nanosecond(), 10_i64.pow((9 - width) as u32));
            assert_eq!(time.format(), text);
        }
        for (hour, minute, second, nanos) in [
            (-1, 0, 0, 0),
            (24, 0, 0, 0),
            (0, -1, 0, 0),
            (0, 60, 0, 0),
            (0, 0, -1, 0),
            (0, 0, 60, 0),
            (0, 0, 0, -1),
            (0, 0, 0, 1_000_000_000),
            (i64::MIN, 0, 0, 0),
            (0, i64::MAX, 0, 0),
            (0, 0, i64::MAX, 0),
            (0, 0, 0, i64::MIN),
        ] {
            assert_eq!(
                Time::create(hour, minute, second, nanos),
                Err(CivilError::InvalidTime)
            );
        }
        let date = Date::parse("2026-10-08").unwrap();
        let utc = UtcDateTime::create(date, value);
        assert_eq!((utc.date(), utc.time()), (date, value));
        assert_eq!(utc.format(), "2026-10-08T23:59:58.999999999Z");
    }

    #[test]
    fn utc_checked_nanoseconds_carry_and_borrow_without_a_clock() {
        let utc = UtcDateTime::parse("2000-03-01T00:00:00Z").unwrap();
        assert_eq!(
            utc.checked_add(-1).unwrap().format(),
            "2000-02-29T23:59:59.999999999Z"
        );
        assert_eq!(
            utc.checked_add(1).unwrap().format(),
            "2000-03-01T00:00:00.000000001Z"
        );
        let first = UtcDateTime::parse("0001-01-01T00:00:00Z").unwrap();
        let last = UtcDateTime::parse("9999-12-31T23:59:59.999999999Z").unwrap();
        assert_eq!(first.checked_add(-1), Err(CivilError::OutOfRange));
        assert_eq!(last.checked_add(1), Err(CivilError::OutOfRange));
        assert_eq!(last.checked_add(0).unwrap(), last);
        for delta in [i64::MIN, -1, 0, 1, i64::MAX] {
            let moved = utc.checked_add(delta).unwrap();
            if let Some(reverse) = delta.checked_neg() {
                assert_eq!(moved.checked_add(reverse).unwrap(), utc);
            }
        }
    }
}
