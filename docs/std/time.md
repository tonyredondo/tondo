# std.time

Durations, the clock, dates and time zones. The three questions a program
asks of time are kept apart, each with its own type: how long something
took is an [`Instant`](#instant) and a [`Duration`](#duration), what the
date is in UTC is a [`DateTime`](#datetime), and what a clock on a wall
somewhere shows is a [`ZonedDateTime`](#zoneddatetime). Sleeping, and the
deadline a `within` block set, are here too.

- [How long](#how-long)
- [Dates](#dates)
- [Time zones](#time-zones)
- [What to reach for](#what-to-reach-for)

## How long

```tondo
import std.console
import std.time

fn main(): !time.TimeError {
    let started = time.now()                    // a clock that only goes forward
    time.sleep(time.milliseconds(20))
    let took = started.elapsed()
    console.println("{took.milliseconds() >= 20} {took.isLessThan(time.seconds(5))}")

    let timeout = time.minutes(1).multiply(3)
    console.println("{timeout.seconds()} s, {time.parseDuration("1m30s")?.seconds()} s, {time.hours(36)}")
}
```

```text output
true true
180 s, 90 s, 36h0m0s
```

A `Duration` is made by naming its unit, `time.seconds(30)`, so there is no
bare number of milliseconds to pass by mistake where seconds were meant.
`time.now()` is for measuring: it does not jump when the system's clock is
set.

## Dates

```tondo
import std.console
import std.time

fn main(): !time.TimeError {
    let release = time.parseDateTime("2026-11-12T09:30:00Z")?
    console.println("{release.year}-{release.month}-{release.day}, a {release.weekday()}, day {release.dayOfYear()}")

    let due = release.addMonths(1).addDays(3)
    console.println(due.format("%A %d %B %Y, %H:%M"))
    console.println(due.format("%A %d %B %Y", locale: "es"))
    console.println("{due.since(release).hours() / 24} days later")

    // The end of a month does not overflow into the next.
    let january = time.DateTime { year: 2027, month: 1, day: 31, hour: 0, minute: 0, second: 0, nanosecond: 0 }
    console.println(january.addMonths(1).format("%Y-%m-%d"))
}
```

```text output
2026-11-12, a Thursday, day 316
Tuesday 15 December 2026, 09:30
martes 15 diciembre 2026
33 days later
2027-02-28
```

A `DateTime` is a moment in UTC, with its parts as fields. `format` takes a
pattern of `strftime` and writes the names of days and months in a
language; `parseDateTime` reads RFC 3339.

## Time zones

```tondo
import std.console
import std.time

fn main(): !time.TimeError {
    let meeting = time.parseDateTime("2026-03-28T17:00:00Z")?
    for place in ["Europe/Madrid", "America/New_York", "Asia/Tokyo"] {
        let local = meeting.inZone(time.zone(place)?)
        console.println("{place}: {local.format("%a %H:%M %Z")}")
    }
    // A day later is the same hour on the wall, across a change of clocks.
    let madrid = meeting.inZone(time.zone("Europe/Madrid")?)
    console.println(madrid.addDays(1).format("%a %H:%M %Z"))
}
```

```text output
Europe/Madrid: Sat 18:00 CET
America/New_York: Sat 13:00 EDT
Asia/Tokyo: Sun 02:00 JST
Sun 18:00 CEST
```

Zones come from the system's tz database, and those it lacks from the
copy every program carries, so a zone is found on a machine with no zone
files too. `TONDO_ZONES=builtin` in the environment uses that copy always.

## What to reach for

| To | Use |
| --- | --- |
| Say how long | [`milliseconds`](#milliseconds), [`seconds`](#seconds), [`minutes`](#minutes), [`hours`](#hours), [`parseDuration`](#parseduration) |
| Measure how long something took | [`now`](#now), [`Instant.elapsed`](#instantelapsed), [`Instant.since`](#instantsince) |
| Wait | [`sleep`](#sleep) |
| Know how long the work has left | [`deadline`](#deadline), [`remaining`](#remaining) |
| Have the date and the time | [`utcNow`](#utcnow), [`localNow`](#localnow) |
| Read and write dates | [`parseDateTime`](#parsedatetime), [`DateTime.format`](#datetimeformat), [`parseZoned`](#parsezoned) |
| Add to a date | [`DateTime.addDays`](#datetimeadddays), [`DateTime.addMonths`](#datetimeaddmonths), [`DateTime.add`](#datetimeadd) |
| Show a moment somewhere | [`zone`](#zone), [`DateTime.inZone`](#datetimeinzone), [`localZone`](#localzone) |
| Keep a moment as a number | [`unixTime`](#unixtime), [`DateTime.sinceEpoch`](#datetimesinceepoch), [`utc`](#utc) |

A deadline for a block of code is `within`, in the language
([Contexts and deadlines](../language/14-contexts.md)), and a clock that a
test moves by hand is `testing.withVirtualTime`
([`std.testing`](testing.md)).

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.time` shows the same in a terminal.

- [Functions](#functions): [`nanoseconds`](#nanoseconds), [`microseconds`](#microseconds), [`milliseconds`](#milliseconds), [`seconds`](#seconds), [`minutes`](#minutes), [`hours`](#hours), [`now`](#now), [`sleep`](#sleep), [`utcNow`](#utcnow), [`unixTime`](#unixtime), [`utc`](#utc), [`deadline`](#deadline), [`remaining`](#remaining), [`parseDateTime`](#parsedatetime), [`parseDuration`](#parseduration), [`zone`](#zone), [`localZone`](#localzone), [`utcZone`](#utczone), [`fixedZone`](#fixedzone), [`localNow`](#localnow), [`parseZoned`](#parsezoned)
- [Duration](#duration): [`Duration.nanoseconds`](#durationnanoseconds), [`Duration.microseconds`](#durationmicroseconds), [`Duration.milliseconds`](#durationmilliseconds), [`Duration.seconds`](#durationseconds), [`Duration.minutes`](#durationminutes), [`Duration.hours`](#durationhours), [`Duration.multiply`](#durationmultiply), [`Duration.isZero`](#durationiszero), [`Duration.isNegative`](#durationisnegative), [`Duration.isLessThan`](#durationislessthan)
- [Instant](#instant): [`Instant.elapsed`](#instantelapsed), [`Instant.since`](#instantsince), [`Instant.add`](#instantadd)
- [DateTime](#datetime): [`DateTime.sinceEpoch`](#datetimesinceepoch), [`DateTime.inZone`](#datetimeinzone), [`DateTime.format`](#datetimeformat), [`DateTime.add`](#datetimeadd), [`DateTime.since`](#datetimesince), [`DateTime.addDays`](#datetimeadddays), [`DateTime.addMonths`](#datetimeaddmonths), [`DateTime.addYears`](#datetimeaddyears), [`DateTime.weekday`](#datetimeweekday), [`DateTime.dayOfYear`](#datetimedayofyear), [`DateTime.isoWeek`](#datetimeisoweek)
- [TimeError](#timeerror)
- [TimedOut](#timedout)
- [Zone](#zone-1): [`Zone.name`](#zonename), [`Zone.at`](#zoneat)
- [ZonedDateTime](#zoneddatetime): [`ZonedDateTime.utc`](#zoneddatetimeutc), [`ZonedDateTime.format`](#zoneddatetimeformat), [`ZonedDateTime.add`](#zoneddatetimeadd), [`ZonedDateTime.since`](#zoneddatetimesince), [`ZonedDateTime.addDays`](#zoneddatetimeadddays), [`ZonedDateTime.addMonths`](#zoneddatetimeaddmonths), [`ZonedDateTime.addYears`](#zoneddatetimeaddyears), [`ZonedDateTime.weekday`](#zoneddatetimeweekday), [`ZonedDateTime.dayOfYear`](#zoneddatetimedayofyear), [`ZonedDateTime.isoWeek`](#zoneddatetimeisoweek)
- [Weekday](#weekday)
- [IsoWeek](#isoweek)

### Functions

#### nanoseconds

```tondo
pub fn nanoseconds(count: Int): Duration
```

A duration of `count` nanoseconds.

#### microseconds

```tondo
pub fn microseconds(count: Int): Duration
```

A duration of `count` microseconds; one that does not fit panics.

#### milliseconds

```tondo
pub fn milliseconds(count: Int): Duration
```

A duration of `count` milliseconds; one that does not fit panics.

#### seconds

```tondo
pub fn seconds(count: Int): Duration
```

A duration of `count` seconds; one that does not fit panics.

#### minutes

```tondo
pub fn minutes(count: Int): Duration
```

A duration of `count` minutes; one that does not fit panics.

#### hours

```tondo
pub fn hours(count: Int): Duration
```

A duration of `count` hours; one that does not fit panics.

#### now

```tondo
pub fn now(): Instant
```

The current moment of the monotonic clock, for measuring durations.

#### sleep

```tondo
pub fn sleep(delay: Duration)
```

Stops the task, not the thread, at least `delay`; a `select` operation.

#### utcNow

```tondo
pub fn utcNow(): DateTime
```

The system clock, which may go back when someone sets it.

#### unixTime

```tondo
pub fn unixTime(): Duration
```

The time since 1970-01-01T00:00:00Z.

#### utc

```tondo
pub fn utc(sinceEpoch: Duration): DateTime
```

A time since the epoch as a date.

#### deadline

```tondo
pub fn deadline(): Instant?
```

When the deadline of the running task passes, the nearest of the
`within`s that hold it, or `none` without one.

#### remaining

```tondo
pub fn remaining(): Duration?
```

What is left until that deadline, zero once it passed.

#### parseDateTime

```tondo
pub fn parseDateTime(
    text: String,
    pattern: String? = none,
    locale: String = "en",
): DateTime ! TimeError
```

ISO 8601 in RFC 3339's form, `2026-10-22`, `2026-10-22T06:38:28Z` or
`2026-10-22 08:38:28.5+02:00`, or the form of a `strftime` pattern, as
a date and time in UTC; without an offset, the text is in UTC.

```tondo
import std.console
import std.time

let start = time.parseDateTime("2026-11-02T08:30:00Z")?
let end = start.add(time.hours(36))
console.println("{end} {end.format("%A %d %B, %H:%M")}")
```

#### parseDuration

```tondo
pub fn parseDuration(text: String): Duration ! TimeError
```

A duration as Go writes it: `1h30m`, `1.5s`, `-250ms`.

#### zone

```tondo
pub fn zone(name: String): Zone ! TimeError
```

The zone of the tz database `name` names: `Europe/Madrid`, `UTC`.

#### localZone

```tondo
pub fn localZone(): Zone
```

The zone `TZ` names, or `/etc/localtime` is, or `UTC`.

#### utcZone

```tondo
pub fn utcZone(): Zone
```

UTC, which has no offset.

#### fixedZone

```tondo
pub fn fixedZone(offset: Duration): Zone
```

A zone that always has `offset`, named by it: `+05:30`.

#### localNow

```tondo
pub fn localNow(): ZonedDateTime
```

The date and time as the local zone's clocks show them now.

#### parseZoned

```tondo
pub fn parseZoned(
    text: String,
    zone: Zone,
    pattern: String? = none,
    locale: String = "en",
): ZonedDateTime ! TimeError
```

What `parseDateTime` reads, kept in the zone it names: a text without an
offset is the time on `zone`'s clocks; one with an offset keeps it, in
`zone` if it had that offset then, or else in `fixedZone` of it.

### Duration

```tondo
pub type Duration
```

A signed interval in nanoseconds; it displays as Go writes it: `1h2m3.5s`.

```tondo
import std.console
import std.time

let timeout = time.seconds(90)
console.println("{timeout} = {timeout.minutes()} min {timeout.seconds() % 60} s, twice {timeout.multiply(2)}")
```

Implements `Additive`, `Ordered`, `reflect.Reflect`.

#### Duration.nanoseconds

```tondo
pub fn Duration.nanoseconds(self): Int
```

Readings divide and truncate toward zero.

#### Duration.microseconds

```tondo
pub fn Duration.microseconds(self): Int
```

The whole microseconds, truncated toward zero.

#### Duration.milliseconds

```tondo
pub fn Duration.milliseconds(self): Int
```

The whole milliseconds, truncated toward zero.

#### Duration.seconds

```tondo
pub fn Duration.seconds(self): Int
```

The whole seconds, truncated toward zero: `-1` for -1.5 s.

#### Duration.minutes

```tondo
pub fn Duration.minutes(self): Int
```

The whole minutes, truncated toward zero.

#### Duration.hours

```tondo
pub fn Duration.hours(self): Int
```

The whole hours, truncated toward zero.

#### Duration.multiply

```tondo
pub fn Duration.multiply(self, factor: Int): Duration
```

The duration `factor` times; one that does not fit panics.

#### Duration.isZero

```tondo
pub fn Duration.isZero(self): Bool
```

Whether the duration is zero.

#### Duration.isNegative

```tondo
pub fn Duration.isNegative(self): Bool
```

Whether the duration is below zero.

#### Duration.isLessThan

```tondo
pub fn Duration.isLessThan(self, other: Duration): Bool
```

Whether this duration is shorter than `other`.

### Instant

```tondo
pub type Instant
```

A moment of the monotonic clock, which never goes back.

```tondo
import std.console
import std.time

let started = time.now()
time.sleep(time.milliseconds(5))
let later = time.now()
console.println("{later.since(started)} {started.elapsed()}")
```

Implements `Ordered`.

#### Instant.elapsed

```tondo
pub fn Instant.elapsed(self): Duration
```

`now().since(self)`.

#### Instant.since

```tondo
pub fn Instant.since(self, earlier: Instant): Duration
```

The time from `earlier` to this moment.

#### Instant.add

```tondo
pub fn Instant.add(self, delta: Duration): Instant
```

The moment `delta` after this one.

### DateTime

```tondo
pub type DateTime = {
    year: Int
    month: Int
    day: Int
    hour: Int
    minute: Int
    second: Int
    nanosecond: Int
}
```

A date and time in UTC; it displays in RFC 3339: `2026-10-22T06:38:28Z`.

```tondo
import std.console
import std.time

let now = time.utcNow()
let due = now.addDays(30)
console.println("{due.format("%Y-%m-%d")} is a {due.weekday()}, day {due.dayOfYear()}")
```

Implements `Ordered`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### DateTime.sinceEpoch

```tondo
pub fn DateTime.sinceEpoch(self): Duration
```

The inverse of `utc`.

#### DateTime.inZone

```tondo
pub fn DateTime.inZone(self, place: Zone): ZonedDateTime
```

The date and time the clocks of `place` show at this moment, with their
offset then.

#### DateTime.format

```tondo
pub fn DateTime.format(self, pattern: String, locale: String = "en"): String
```

As a pattern of `strftime` writes it, in UTC: `%Y-%m-%d %H:%M`, with
the names of months and days of `locale`.

#### DateTime.add

```tondo
pub fn DateTime.add(self, delta: Duration): DateTime
```

The moment `delta` later, or earlier when it is negative.

#### DateTime.since

```tondo
pub fn DateTime.since(self, earlier: DateTime): Duration
```

How long after `earlier` it is, negative when it is before.

#### DateTime.addDays

```tondo
pub fn DateTime.addDays(self, days: Int): DateTime
```

The same time `days` days later.

#### DateTime.addMonths

```tondo
pub fn DateTime.addMonths(self, months: Int): DateTime
```

The same day and time `months` months later, or on the last day of that
month when it is shorter: January 31 and a month is February 28, or 29.

#### DateTime.addYears

```tondo
pub fn DateTime.addYears(self, years: Int): DateTime
```

As `addMonths` of twelve times as many.

#### DateTime.weekday

```tondo
pub fn DateTime.weekday(self): Weekday
```

The day of the week.

#### DateTime.dayOfYear

```tondo
pub fn DateTime.dayOfYear(self): Int
```

The day of the year: from 1, January 1, to 366.

#### DateTime.isoWeek

```tondo
pub fn DateTime.isoWeek(self): IsoWeek
```

The week of ISO 8601, and the year it is of.

### TimeError

```tondo
pub enum TimeError {
    InvalidDateTime
    InvalidDuration
    UnknownZone
}
```

Why a date, a duration or a zone could not be made: a date that does
not exist, as February 30, or a text that is not one; a text that is not
a duration; a name the tz database does not have.

```tondo
import std.console
import std.time

match time.zone("Mars/Olympus") {
    ok(zone) => console.println(zone.name())
    err(time.TimeError.UnknownZone) => console.println("no such zone")
    err(error) => console.println("{error}")
}
```

### TimedOut

```tondo
pub type TimedOut = {
    limit: Duration
}
```

The error of a `within` whose deadline passed first (11.17): the limit
it had.

```tondo
import std.console
import std.time

match within time.milliseconds(50) { time.sleep(time.seconds(1)) } {
    ok(_) => console.println("in time")
    err(late) => console.println("gave up after {late.limit}")
}
```

Implements `Display`.

### Zone

```tondo
pub type Zone = {
    // private fields
}
```

A time zone: the offsets from UTC its clocks have had through the
years. Equal and a key by its name, which `Display` writes.

```tondo
import std.console
import std.time

let madrid = time.zone("Europe/Madrid")?
let meeting = madrid.at(2026, 11, 2, hour: 9)?
console.println("{meeting} is {meeting.utc()} in UTC")
```

Implements `Display`, `reflect.Reflect`.

#### Zone.name

```tondo
pub fn Zone.name(self): String
```

Its name, as `time.zone` takes it.

#### Zone.at

```tondo
pub fn Zone.at(
    self,
    year: Int,
    month: Int,
    day: Int,
    hour: Int = 0,
    minute: Int = 0,
    second: Int = 0,
    nanosecond: Int = 0,
): ZonedDateTime ! TimeError
```

The moment the zone's clocks show this date and time. One they skip
moves on by what they skip; one they show twice is the first.

### ZonedDateTime

```tondo
pub type ZonedDateTime = {
    year: Int
    month: Int
    day: Int
    hour: Int
    minute: Int
    second: Int
    nanosecond: Int
    offset: Duration
    zone: Zone
}
```

A date and time as the clocks of a zone show it, with their offset from
UTC then.

```tondo
import std.console
import std.time

let here = time.localNow()
let tomorrow = here.addDays(1)
console.println("{tomorrow.format("%A %H:%M")} week {tomorrow.isoWeek().week}")
```

Implements `Ordered`, `Display`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### ZonedDateTime.utc

```tondo
pub fn ZonedDateTime.utc(self): DateTime
```

The moment, in UTC: the date and time less the offset.

#### ZonedDateTime.format

```tondo
pub fn ZonedDateTime.format(self, pattern: String, locale: String = "en"): String
```

As a pattern of `strftime` writes it, with the names of `locale`:
`%d/%m/%Y %H:%M %Z`.

#### ZonedDateTime.add

```tondo
pub fn ZonedDateTime.add(self, delta: Duration): ZonedDateTime
```

The moment `delta` later, in the same zone, whose clocks may then show
another offset.

#### ZonedDateTime.since

```tondo
pub fn ZonedDateTime.since(self, earlier: ZonedDateTime): Duration
```

How long after `earlier` it is, negative when it is before.

#### ZonedDateTime.addDays

```tondo
pub fn ZonedDateTime.addDays(self, days: Int): ZonedDateTime
```

The same time of the zone's clocks `days` days later, as `Zone.at` takes
it when the clocks change that day: a day may be 23 or 25 hours long.

#### ZonedDateTime.addMonths

```tondo
pub fn ZonedDateTime.addMonths(self, months: Int): ZonedDateTime
```

The same day and time of the zone's clocks `months` months later, or on
the last day of that month when it is shorter.

#### ZonedDateTime.addYears

```tondo
pub fn ZonedDateTime.addYears(self, years: Int): ZonedDateTime
```

As `addMonths` of twelve times as many.

#### ZonedDateTime.weekday

```tondo
pub fn ZonedDateTime.weekday(self): Weekday
```

The day of the week its clocks show.

#### ZonedDateTime.dayOfYear

```tondo
pub fn ZonedDateTime.dayOfYear(self): Int
```

The day of the year its clocks show: from 1, January 1, to 366.

#### ZonedDateTime.isoWeek

```tondo
pub fn ZonedDateTime.isoWeek(self): IsoWeek
```

The week of ISO 8601 of the day its clocks show.

### Weekday

```tondo
pub enum Weekday {
    Monday
    Tuesday
    Wednesday
    Thursday
    Friday
    Saturday
    Sunday
}
```

A day of the week, Monday first, as ISO 8601 counts them.

```tondo
import std.console
import std.time

let today = time.utcNow().weekday()
let weekend = today == time.Weekday.Saturday or today == time.Weekday.Sunday
console.println("{today}: weekend {weekend}")
```

Implements `Display`, `reflect.Reflect`.

### IsoWeek

```tondo
pub type IsoWeek = {
    year: Int
    week: Int
}
```

A week of ISO 8601: its number, from 1 to 53, and the year it is of,
which for the days near January 1 may be the year before theirs or the
one after. `Display` writes it as ISO 8601 does: `2026-W44`.

```tondo
import std.console
import std.time

let week = time.localNow().isoWeek()
console.println("{week.year}-W{week.week:02}")
```

Implements `Display`, `reflect.Reflect`.
