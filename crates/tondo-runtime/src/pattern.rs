//! The patterns of `format` and of the parsing of `std.time` (standard
//! library 14.3.5), as `strftime` writes them: their pieces, which the
//! runtime follows and the compiler checks in a literal pattern. This file
//! is the compiler's too.

/// A piece of a pattern: a character as it is, or a directive and whether
/// `-` takes its padding away.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Piece {
    Text(char),
    Directive(Directive, bool),
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Directive {
    Year,
    ShortYear,
    Month,
    MonthName,
    MonthShort,
    MonthAlone,
    Day,
    DaySpaced,
    DayOfYear,
    Weekday,
    WeekdayShort,
    Hour,
    Hour12,
    Meridiem,
    Minute,
    Second,
    /// The fraction of the second in 3, 6 or 9 digits.
    Fraction(u8),
    Offset,
    OffsetColon,
    Zone,
    Seconds,
}

/// The directives there are, for messages.
pub const DIRECTIVES: &str = "%Y %y %m %B %b %OB %d %e %j %A %a %H %I %p %M %S %3f %6f %9f %z %:z %Z %s %F %T %%";

/// The pieces of a pattern, or what is wrong with it.
pub fn pieces(pattern: &str) -> Result<Vec<Piece>, String> {
    use Directive as D;
    let mut out = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(Piece::Text(c));
            continue;
        }
        let bare = chars.next_if_eq(&'-').is_none();
        let mut name = String::new();
        for _ in 0..2 {
            match chars.next() {
                Some(next) => name.push(next),
                None => break,
            }
            if !matches!(name.as_str(), "O" | ":" | "3" | "6" | "9") {
                break;
            }
        }
        let directive = match name.as_str() {
            "%" => {
                out.push(Piece::Text('%'));
                continue;
            }
            "F" | "T" => {
                let expanded = if name == "F" { "%Y-%m-%d" } else { "%H:%M:%S" };
                out.extend(pieces(expanded)?);
                continue;
            }
            "Y" => D::Year,
            "y" => D::ShortYear,
            "m" => D::Month,
            "B" => D::MonthName,
            "b" => D::MonthShort,
            "OB" => D::MonthAlone,
            "d" => D::Day,
            "e" => D::DaySpaced,
            "j" => D::DayOfYear,
            "A" => D::Weekday,
            "a" => D::WeekdayShort,
            "H" => D::Hour,
            "I" => D::Hour12,
            "p" => D::Meridiem,
            "M" => D::Minute,
            "S" => D::Second,
            "3f" => D::Fraction(3),
            "6f" => D::Fraction(6),
            "9f" => D::Fraction(9),
            "z" => D::Offset,
            ":z" => D::OffsetColon,
            "Z" => D::Zone,
            "s" => D::Seconds,
            "" => return Err("the pattern ends in `%`: write `%%` for a `%` itself".into()),
            "f" => return Err("`%f` is not a directive: write `%3f`, `%6f` or `%9f` for the fraction of the second in milliseconds, microseconds or nanoseconds".into()),
            other => return Err(format!("`%{other}` is not a directive; they are {DIRECTIVES}")),
        };
        out.push(Piece::Directive(directive, bare));
    }
    Ok(out)
}

/// Why a pattern cannot read a date: it reads no year, month or day.
pub fn parsing(pieces: &[Piece]) -> Result<(), String> {
    use Directive as D;
    let has = |wanted: &[Directive]| pieces.iter().any(|p| matches!(p, Piece::Directive(d, _) if wanted.contains(d)));
    if has(&[D::Seconds]) {
        return Ok(());
    }
    let missing = if !has(&[D::Year, D::ShortYear]) {
        "the year: add `%Y`"
    } else if has(&[D::DayOfYear]) {
        return Ok(());
    } else if !has(&[D::Month, D::MonthName, D::MonthShort, D::MonthAlone]) {
        "the month: add `%m` or `%B`"
    } else if !has(&[D::Day, D::DaySpaced]) {
        "the day: add `%d`"
    } else {
        return Ok(());
    };
    Err(format!("the pattern does not read {missing}, or `%s` for seconds since 1970"))
}
