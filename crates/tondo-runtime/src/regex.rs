//! The engine of `std.regex` (standard library 14.16): the `regex` crate,
//! which runs in time linear in its input and has no backreferences or
//! look-around. The dialect is checked first with `regex-syntax`, so a
//! pattern outside it is an error with its kind and offset, and embedded
//! flags but `i`, `m`, `s` and `U`, and `(?P<name>...)`, are refused as the
//! spec does.
//!
//! Operations never wait, so each leaves its error, if any, for the calling
//! thread to read right after it.

use super::regex_dialect::{Kind, check};
use super::{ArrayHeader, LIVE_OBJECTS, Str, str_new};
use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;

thread_local! {
    /// The last operation's error: its kind, where, and the bound of
    /// `RegexLimits` it passed, 0 for none.
    static ERROR: Cell<(i64, i64, i64)> = const { Cell::new((0, 0, 0)) };
}

fn fail(kind: Kind, offset: usize, limit: usize) {
    ERROR.with(|e| e.set((kind as i64, offset as i64, limit.min(i64::MAX as usize) as i64)));
}

fn clear() {
    ERROR.with(|e| e.set((0, 0, 0)));
}

/// The bound of `RegexLimits` the last operation's error passed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_regex_error_limit() -> i64 {
    ERROR.with(|e| e.get().2)
}

/// The kind of the last operation's error, `0` for none.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_regex_error_kind() -> i64 {
    ERROR.with(|e| e.get().0)
}

/// The byte offset of the last operation's error.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_regex_error_offset() -> i64 {
    ERROR.with(|e| e.get().1)
}

#[repr(C)]
pub struct RegexObject {
    rc: AtomicUsize,
    pattern: String,
    flags: i64,
    size: usize,
    depth: u32,
    regex: regex::Regex,
    /// The pattern anchored at both ends, for `isFullMatch`.
    full: OnceLock<Option<regex::Regex>>,
    max_input: usize,
    max_matches: usize,
    max_output: usize,
    max_replacement: usize,
}

/// The text of a string, which always holds valid UTF-8: checked where it
/// was made, not again at each match.
fn utf8(s: &*const Str) -> &str {
    unsafe { super::strings::str_text(s) }
}

fn builder(pattern: &str, flags: i64, size: usize, depth: u32) -> regex::RegexBuilder {
    let mut builder = regex::RegexBuilder::new(pattern);
    builder
        .size_limit(size)
        .nest_limit(depth)
        .case_insensitive(flags & 1 != 0)
        .multi_line(flags & 2 != 0)
        .dot_matches_new_line(flags & 4 != 0)
        .crlf(flags & 8 != 0)
        .swap_greed(flags & 16 != 0);
    builder
}

/// A pattern in the dialect, compiled within the limits; the kind and
/// offset of what stops it.
fn compile(
    pattern: &str,
    flags: i64,
    depth: u32,
    max_captures: i64,
    size: usize,
) -> Result<regex::Regex, (Kind, usize)> {
    check(pattern, flags & 1 != 0, depth, max_captures)?;
    // The pattern is valid, so only its size can stop it.
    builder(pattern, flags, size, depth).build().map_err(|_| (Kind::ProgramLimitExceeded, 0))
}

/// A pattern with the default options and limits, for the runtime's own
/// use; an error reads as `std.regex` shows it.
pub(crate) fn pattern(pattern: &str) -> Result<regex::Regex, String> {
    compile(pattern, 0, 256, 256, 16_000_000).map_err(|(kind, offset)| format!("regex.{kind:?} at byte {offset}"))
}

/// `Regex.escape(text)`: a pattern that matches the text as it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_escape(text: *const Str, region: *mut crate::regions::Region) -> *mut Str {
    crate::strings::str_in(region, regex::escape(utf8(&text)).as_bytes())
}

/// `Regex.compile`: the pattern with its options as bits (case-insensitive,
/// multi-line, dot matches newline, CRLF, ungreedy) and its limits; null
/// with an error when it cannot be.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tondo_rt_regex_compile(
    pattern: *const Str,
    flags: i64,
    max_pattern: i64,
    max_depth: i64,
    max_captures: i64,
    max_states: i64,
    max_input: i64,
    max_matches: i64,
    max_output: i64,
    max_replacement: i64,
    region: *mut super::regions::Region,
) -> *mut RegexObject {
    clear();
    let pattern = utf8(&pattern);
    if pattern.len() as i64 > max_pattern {
        fail(Kind::PatternLimitExceeded, max_pattern as usize, max_pattern as usize);
        return std::ptr::null_mut();
    }
    let depth = u32::try_from(max_depth).unwrap_or(u32::MAX);
    let size = usize::try_from(max_states).unwrap_or(usize::MAX).saturating_mul(16);
    let regex = match compile(pattern, flags, depth, max_captures, size) {
        Ok(regex) => regex,
        Err((kind, offset)) => {
            let limit = match kind {
                Kind::NestingLimitExceeded => max_depth,
                Kind::CaptureLimitExceeded => max_captures,
                Kind::ProgramLimitExceeded => max_states,
                _ => 0,
            };
            fail(kind, offset, limit.max(0) as usize);
            return std::ptr::null_mut();
        }
    };
    let limit = |n: i64| usize::try_from(n).unwrap_or(usize::MAX);
    let object = RegexObject {
        rc: AtomicUsize::new(1),
        pattern: pattern.to_string(),
        flags,
        size,
        depth,
        regex,
        full: OnceLock::new(),
        max_input: limit(max_input),
        max_matches: limit(max_matches),
        max_output: limit(max_output),
        max_replacement: limit(max_replacement),
    };
    unsafe { super::objects::make(region, object) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_release(object: *mut RegexObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        match super::objects::dropped(&(*object).rc) {
            Some(true) => {
                drop(Box::from_raw(object));
                LIVE_OBJECTS.gone();
            }
            Some(false) => std::ptr::drop_in_place(object),
            None => {}
        }
    }
}

fn object<'a>(object: *mut RegexObject) -> &'a RegexObject {
    unsafe { &*object }
}

/// The input, unless it is longer than the limit.
fn input<'a>(r: &RegexObject, input: &'a *const Str) -> Option<&'a str> {
    let input = utf8(input);
    if input.len() > r.max_input {
        fail(Kind::InputLimitExceeded, r.max_input, r.max_input);
        return None;
    }
    Some(input)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_pattern(r: *mut RegexObject, region: *mut crate::regions::Region) -> *mut Str {
    crate::strings::str_in(region, object(r).pattern.as_bytes())
}

/// The numbered captures, without the whole match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_count(r: *mut RegexObject) -> i64 {
    object(r).regex.captures_len() as i64 - 1
}

/// The name of a capture, or `""`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_name(
    r: *mut RegexObject,
    index: i64,
    region: *mut crate::regions::Region,
) -> *mut Str {
    let name = object(r).regex.capture_names().nth(index.max(0) as usize).flatten().unwrap_or("");
    crate::strings::str_in(region, name.as_bytes())
}

/// The index of a named capture, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_index(r: *mut RegexObject, name: *const Str) -> i64 {
    let name = utf8(&name);
    object(r).regex.capture_names().position(|n| n == Some(name)).map_or(-1, |i| i as i64)
}

/// `isMatch`, or `isFullMatch` when `full`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_is_match(r: *mut RegexObject, text: *const Str, full: u8) -> u8 {
    clear();
    let r = object(r);
    let Some(text) = input(r, &text) else { return 0 };
    if full == 0 {
        return r.regex.is_match(text) as u8;
    }
    let anchored = || format!(r"\A(?:{})\z", r.pattern);
    match r.full.get_or_init(|| builder(&anchored(), r.flags, r.size, r.depth.saturating_add(1)).build().ok()) {
        Some(full) => full.is_match(text) as u8,
        None => {
            fail(Kind::ProgramLimitExceeded, 0, r.size / 16);
            0
        }
    }
}

/// Whether the input is within the limit, as an error if it is not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_check_input(r: *mut RegexObject, text: *const Str) {
    clear();
    input(object(r), &text);
}

/// The next match after one that ended at `last` (`-1` before the first),
/// with `found` before it: the start and end of each capture, the whole
/// match first, `-1` for one that did not take part; empty when there is
/// none. An empty match where the last ended is passed over, as
/// `replaceAll` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_find(
    r: *mut RegexObject,
    text: *const Str,
    last: i64,
    found: i64,
    region: *mut crate::regions::Region,
) -> *mut ArrayHeader {
    clear();
    let r = object(r);
    let Some(text) = input(r, &text) else { return crate::arrays::int_array_in(region, &[]) };
    let mut at = last.max(0) as usize;
    let captures = loop {
        let Some(captures) = text.get(at..).and_then(|_| r.regex.captures_at(text, at)) else {
            return crate::arrays::int_array_in(region, &[]);
        };
        let whole = captures.get(0).expect("a match has its whole");
        if !whole.is_empty() || whole.end() as i64 != last {
            break captures;
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    };
    let start = captures.get(0).expect("a match has its whole").start();
    if found as usize >= r.max_matches {
        fail(Kind::MatchLimitExceeded, start, r.max_matches);
        return crate::arrays::int_array_in(region, &[]);
    }
    let spans: Vec<i64> =
        captures.iter().flat_map(|group| group.map_or([-1, -1], |m| [m.start() as i64, m.end() as i64])).collect();
    crate::arrays::int_array_in(region, &spans)
}

/// The text between two byte offsets, which must fall between scalars.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_slice(
    text: *const Str,
    start: i64,
    end: i64,
    region: *mut crate::regions::Region,
) -> *mut Str {
    clear();
    let text = utf8(&text);
    match text.get(start.max(0) as usize..end.max(0) as usize) {
        Some(slice) if start >= 0 && end >= start => crate::strings::str_in(region, slice.as_bytes()),
        _ => {
            fail(Kind::InvalidBoundary, start.max(0) as usize, 0);
            str_new(b"")
        }
    }
}

/// A part of a replacement: text, or a capture by index or name.
enum Part<'a> {
    Text(&'a str),
    Index(usize),
    Name(&'a str),
}

/// Reads a replacement: `$0`, `$N`, `${name}` and `$$` (15.5 of the
/// spec); any other `$` is an error at its offset.
fn template<'a>(r: &RegexObject, replacement: &'a str) -> Result<Vec<Part<'a>>, usize> {
    let mut parts = Vec::new();
    let mut rest = replacement;
    let mut offset = 0;
    while let Some(at) = rest.find('$') {
        parts.push(Part::Text(&rest[..at]));
        let after = &rest[at + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let used = if after.starts_with('$') {
            parts.push(Part::Text("$"));
            1
        } else if digits > 0 {
            let index: usize = after[..digits].parse().map_err(|_| offset + at)?;
            if index >= r.regex.captures_len() {
                return Err(offset + at);
            }
            parts.push(Part::Index(index));
            digits
        } else if let Some(body) = after.strip_prefix('{') {
            let close = body.find('}').ok_or(offset + at)?;
            let name = &body[..close];
            if !r.regex.capture_names().any(|n| n == Some(name)) {
                return Err(offset + at);
            }
            parts.push(Part::Name(name));
            close + 2
        } else {
            return Err(offset + at);
        };
        offset += at + 1 + used;
        rest = &after[used..];
    }
    parts.push(Part::Text(rest));
    Ok(parts)
}

/// `replace`, or `replaceAll` when `all`; `""` with an error when it fails,
/// and never a partial result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_replace(
    r: *mut RegexObject,
    text: *const Str,
    replacement: *const Str,
    all: u8,
    region: *mut crate::regions::Region,
) -> *mut Str {
    clear();
    let r = object(r);
    let Some(text) = input(r, &text) else { return str_new(b"") };
    let replacement = utf8(&replacement);
    if replacement.len() > r.max_replacement {
        fail(Kind::ReplacementLimitExceeded, r.max_replacement, r.max_replacement);
        return str_new(b"");
    }
    // A replacement without `$` is its text alone: its matches need no
    // groups, which cost an allocation each.
    let parts = match replacement.contains('$') {
        false => None,
        true => match template(r, replacement) {
            Ok(parts) => Some(parts),
            Err(offset) => {
                fail(Kind::InvalidReplacement, offset, 0);
                return str_new(b"");
            }
        },
    };
    let made = REPLACED.with_borrow_mut(|out| {
        out.clear();
        let made = replaced(r, text, replacement, parts.as_deref(), all != 0, out)
            .map(|()| crate::strings::str_in(region, out.as_bytes()));
        if out.capacity() > REPLACED_KEPT {
            *out = String::new();
        }
        made
    });
    made.unwrap_or_else(|(kind, offset)| {
        let limit = match kind {
            Kind::MatchLimitExceeded => r.max_matches,
            Kind::OutputLimitExceeded => r.max_output,
            _ => 0,
        };
        fail(kind, offset, limit);
        str_new(b"")
    })
}

thread_local! {
    /// The text a replacement writes before it is copied where it goes,
    /// one for each thread, as each replacement ends before the next: a
    /// replacement allocates only the text it gives.
    static REPLACED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// The most room `REPLACED` keeps once a replacement is done.
const REPLACED_KEPT: usize = 64 * 1024;

/// Writes `text` with its first match, or each, replaced into `out`: the
/// replacement's text alone without `parts`, or the parts with the groups
/// of each match; the limit passed and where, when one is.
fn replaced(
    r: &RegexObject,
    text: &str,
    replacement: &str,
    parts: Option<&[Part]>,
    all: bool,
    out: &mut String,
) -> Result<(), (Kind, usize)> {
    let mut last = 0;
    let mut write = |count: usize, start: usize, end: usize, out: &mut String, written: &dyn Fn(&mut String)| {
        if count >= r.max_matches {
            return Err((Kind::MatchLimitExceeded, start));
        }
        out.push_str(&text[last..start]);
        written(out);
        last = end;
        match out.len() > r.max_output {
            true => Err((Kind::OutputLimitExceeded, last)),
            false => Ok(()),
        }
    };
    match parts {
        None => {
            for (count, found) in r.regex.find_iter(text).enumerate() {
                write(count, found.start(), found.end(), out, &|out| out.push_str(replacement))?;
                if !all {
                    break;
                }
            }
        }
        Some(parts) => {
            for (count, captures) in r.regex.captures_iter(text).enumerate() {
                let whole = captures.get(0).expect("a match has its whole");
                let written = |out: &mut String| {
                    for part in parts {
                        match part {
                            Part::Text(t) => out.push_str(t),
                            Part::Index(i) => out.push_str(captures.get(*i).map_or("", |m| m.as_str())),
                            Part::Name(n) => out.push_str(captures.name(n).map_or("", |m| m.as_str())),
                        }
                    }
                };
                write(count, whole.start(), whole.end(), out, &written)?;
                if !all {
                    break;
                }
            }
        }
    }
    out.push_str(&text[last..]);
    match out.len() > r.max_output {
        true => Err((Kind::OutputLimitExceeded, text.len())),
        false => Ok(()),
    }
}

/// Whether all of `text` matches `pattern`, 1 or 0, or -1 when the pattern
/// does not compile: the rule `@validate.Matches` (standard library 14.35),
/// whose patterns the runtime compiles once and keeps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_whole_match(pattern: *const Str, text: *const Str) -> i64 {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static KEPT: OnceLock<Mutex<HashMap<String, Option<regex::Regex>>>> = OnceLock::new();
    let pattern = utf8(&pattern);
    let mut kept = KEPT.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= 1024 {
        kept.clear();
    }
    let compiled =
        kept.entry(pattern.to_string()).or_insert_with(|| regex::Regex::new(&format!("^(?:{pattern})$")).ok());
    match compiled {
        Some(regex) => regex.is_match(utf8(&text)) as i64,
        None => -1,
    }
}
