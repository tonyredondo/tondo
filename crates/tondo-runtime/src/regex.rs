//! The engine of `std.regex` (standard library 14.16): the `regex` crate,
//! which runs in time linear in its input and has no backreferences or
//! look-around. The dialect is checked first with `regex-syntax`, so a
//! pattern outside it is an error with its kind and offset, and embedded
//! flags and `(?P<name>...)` are refused as the spec does.
//!
//! Operations never wait, so each leaves its error, if any, for the calling
//! thread to read right after it.

use super::{ArrayHeader, LIVE_OBJECTS, Str, str_bytes, str_new};
use regex_syntax::ast::{self, Ast, ErrorKind as Syntax};
use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The kinds of `RegexErrorKind`, in its order from 1.
#[derive(Clone, Copy)]
enum Kind {
    InvalidSyntax = 1,
    UnexpectedEnd,
    InvalidEscape,
    InvalidUnicodeScalar,
    InvalidUnicodeProperty,
    InvalidClass,
    InvalidRange,
    InvalidQuantifier,
    InvalidCaptureName,
    DuplicateCaptureName,
    UnsupportedFeature,
    PatternLimitExceeded,
    ProgramLimitExceeded,
    InputLimitExceeded,
    MatchLimitExceeded,
    ReplacementLimitExceeded,
    InvalidReplacement,
    OutputLimitExceeded,
    InvalidBoundary,
}

thread_local! {
    static ERROR: Cell<(i64, i64)> = const { Cell::new((0, 0)) };
}

fn fail(kind: Kind, offset: usize) {
    ERROR.with(|e| e.set((kind as i64, offset as i64)));
}

fn clear() {
    ERROR.with(|e| e.set((0, 0)));
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

fn utf8<'a>(s: *const Str) -> &'a str {
    std::str::from_utf8(unsafe { str_bytes(s) }).unwrap_or_default()
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

/// Where a parsed pattern leaves the dialect, first in the pattern: an
/// embedded flag, the `(?P<name>...)` form, or a capture past the limit.
fn outside(ast: &Ast, max_captures: i64) -> Option<(Kind, usize)> {
    let mut stack = vec![ast];
    while let Some(node) = stack.pop() {
        let offset = node.span().start.offset;
        match node {
            Ast::Flags(_) => return Some((Kind::UnsupportedFeature, offset)),
            Ast::Group(group) => {
                let index = match &group.kind {
                    ast::GroupKind::NonCapturing(flags) if !flags.items.is_empty() => {
                        return Some((Kind::UnsupportedFeature, offset));
                    }
                    ast::GroupKind::CaptureName { starts_with_p: true, .. } => {
                        return Some((Kind::UnsupportedFeature, offset));
                    }
                    ast::GroupKind::CaptureName { name, .. } => name.index,
                    ast::GroupKind::CaptureIndex(index) => *index,
                    ast::GroupKind::NonCapturing(_) => 0,
                };
                if i64::from(index) > max_captures {
                    return Some((Kind::PatternLimitExceeded, offset));
                }
                stack.push(&group.ast);
            }
            Ast::Repetition(repetition) => stack.push(&repetition.ast),
            Ast::Alternation(alternation) => stack.extend(alternation.asts.iter().rev()),
            Ast::Concat(concat) => stack.extend(concat.asts.iter().rev()),
            _ => {}
        }
    }
    None
}

fn syntax_kind(kind: &Syntax) -> Kind {
    match kind {
        Syntax::UnsupportedBackreference | Syntax::UnsupportedLookAround => Kind::UnsupportedFeature,
        Syntax::FlagDanglingNegation
        | Syntax::FlagDuplicate { .. }
        | Syntax::FlagRepeatedNegation { .. }
        | Syntax::FlagUnexpectedEof
        | Syntax::FlagUnrecognized => Kind::UnsupportedFeature,
        Syntax::ClassEscapeInvalid | Syntax::ClassRangeLiteral | Syntax::ClassUnclosed => Kind::InvalidClass,
        Syntax::ClassRangeInvalid => Kind::InvalidRange,
        Syntax::DecimalEmpty
        | Syntax::DecimalInvalid
        | Syntax::RepetitionCountDecimalEmpty
        | Syntax::RepetitionCountInvalid
        | Syntax::RepetitionCountUnclosed
        | Syntax::RepetitionMissing => Kind::InvalidQuantifier,
        Syntax::EscapeHexEmpty | Syntax::EscapeHexInvalidDigit | Syntax::EscapeHexInvalid => Kind::InvalidUnicodeScalar,
        Syntax::EscapeUnexpectedEof => Kind::UnexpectedEnd,
        Syntax::EscapeUnrecognized | Syntax::SpecialWordBoundaryUnrecognized => Kind::InvalidEscape,
        Syntax::GroupNameDuplicate { .. } => Kind::DuplicateCaptureName,
        Syntax::GroupNameEmpty | Syntax::GroupNameInvalid | Syntax::GroupNameUnexpectedEof => Kind::InvalidCaptureName,
        Syntax::GroupUnclosed => Kind::UnexpectedEnd,
        Syntax::NestLimitExceeded(_) | Syntax::CaptureLimitExceeded => Kind::PatternLimitExceeded,
        Syntax::UnicodeClassInvalid => Kind::InvalidUnicodeProperty,
        _ => Kind::InvalidSyntax,
    }
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
) -> *mut RegexObject {
    clear();
    let pattern = utf8(pattern);
    if pattern.len() as i64 > max_pattern {
        fail(Kind::PatternLimitExceeded, max_pattern as usize);
        return std::ptr::null_mut();
    }
    let depth = u32::try_from(max_depth).unwrap_or(u32::MAX);
    let parsed = match ast::parse::ParserBuilder::new().nest_limit(depth).build().parse(pattern) {
        Ok(parsed) => parsed,
        Err(error) => {
            fail(syntax_kind(error.kind()), error.span().start.offset);
            return std::ptr::null_mut();
        }
    };
    if let Some((kind, offset)) = outside(&parsed, max_captures) {
        fail(kind, offset);
        return std::ptr::null_mut();
    }
    let translated = regex_syntax::hir::translate::TranslatorBuilder::new()
        .case_insensitive(flags & 1 != 0)
        .build()
        .translate(pattern, &parsed);
    if let Err(error) = translated {
        use regex_syntax::hir::ErrorKind as Hir;
        let kind = match error.kind() {
            Hir::UnicodePropertyNotFound | Hir::UnicodePropertyValueNotFound | Hir::UnicodePerlClassNotFound => {
                Kind::InvalidUnicodeProperty
            }
            _ => Kind::UnsupportedFeature,
        };
        fail(kind, error.span().start.offset);
        return std::ptr::null_mut();
    }
    let size = usize::try_from(max_states).unwrap_or(usize::MAX).saturating_mul(16);
    let Ok(regex) = builder(pattern, flags, size, depth).build() else {
        // The pattern is valid, so only its size can stop it.
        fail(Kind::ProgramLimitExceeded, 0);
        return std::ptr::null_mut();
    };
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
    let limit = |n: i64| usize::try_from(n).unwrap_or(usize::MAX);
    Box::into_raw(Box::new(RegexObject {
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
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_release(object: *mut RegexObject) {
    if object.is_null() || unsafe { (*object).rc.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    std::sync::atomic::fence(Ordering::Acquire);
    drop(unsafe { Box::from_raw(object) });
    LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
}

fn object<'a>(object: *mut RegexObject) -> &'a RegexObject {
    unsafe { &*object }
}

/// The input, unless it is longer than the limit.
fn input<'a>(r: &RegexObject, input: *const Str) -> Option<&'a str> {
    let input = utf8(input);
    if input.len() > r.max_input {
        fail(Kind::InputLimitExceeded, r.max_input);
        return None;
    }
    Some(input)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_pattern(r: *mut RegexObject) -> *mut Str {
    str_new(object(r).pattern.as_bytes())
}

/// The numbered captures, without the whole match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_count(r: *mut RegexObject) -> i64 {
    object(r).regex.captures_len() as i64 - 1
}

/// The name of a capture, or `""`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_name(r: *mut RegexObject, index: i64) -> *mut Str {
    let name = object(r).regex.capture_names().nth(index.max(0) as usize).flatten().unwrap_or("");
    str_new(name.as_bytes())
}

/// The index of a named capture, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_capture_index(r: *mut RegexObject, name: *const Str) -> i64 {
    let name = utf8(name);
    object(r).regex.capture_names().position(|n| n == Some(name)).map_or(-1, |i| i as i64)
}

/// `isMatch`, or `isFullMatch` when `full`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_is_match(r: *mut RegexObject, text: *const Str, full: u8) -> u8 {
    clear();
    let r = object(r);
    let Some(text) = input(r, text) else { return 0 };
    if full == 0 {
        return r.regex.is_match(text) as u8;
    }
    let anchored = || format!(r"\A(?:{})\z", r.pattern);
    match r.full.get_or_init(|| builder(&anchored(), r.flags, r.size, r.depth.saturating_add(1)).build().ok()) {
        Some(full) => full.is_match(text) as u8,
        None => {
            fail(Kind::ProgramLimitExceeded, 0);
            0
        }
    }
}

/// Whether the input is within the limit, as an error if it is not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_check_input(r: *mut RegexObject, text: *const Str) {
    clear();
    input(object(r), text);
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
) -> *mut ArrayHeader {
    clear();
    let r = object(r);
    let Some(text) = input(r, text) else { return ints(&[]) };
    let mut at = last.max(0) as usize;
    let captures = loop {
        let Some(captures) = text.get(at..).and_then(|_| r.regex.captures_at(text, at)) else { return ints(&[]) };
        let whole = captures.get(0).expect("a match has its whole");
        if !whole.is_empty() || whole.end() as i64 != last {
            break captures;
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    };
    let start = captures.get(0).expect("a match has its whole").start();
    if found as usize >= r.max_matches {
        fail(Kind::MatchLimitExceeded, start);
        return ints(&[]);
    }
    let spans: Vec<i64> =
        captures.iter().flat_map(|group| group.map_or([-1, -1], |m| [m.start() as i64, m.end() as i64])).collect();
    ints(&spans)
}

/// The text between two byte offsets, which must fall between scalars.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_regex_slice(text: *const Str, start: i64, end: i64) -> *mut Str {
    clear();
    let text = utf8(text);
    match text.get(start.max(0) as usize..end.max(0) as usize) {
        Some(slice) if start >= 0 && end >= start => str_new(slice.as_bytes()),
        _ => {
            fail(Kind::InvalidBoundary, start.max(0) as usize);
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
) -> *mut Str {
    clear();
    let r = object(r);
    let Some(text) = input(r, text) else { return str_new(b"") };
    let replacement = utf8(replacement);
    if replacement.len() > r.max_replacement {
        fail(Kind::ReplacementLimitExceeded, r.max_replacement);
        return str_new(b"");
    }
    let parts = match template(r, replacement) {
        Ok(parts) => parts,
        Err(offset) => {
            fail(Kind::InvalidReplacement, offset);
            return str_new(b"");
        }
    };
    let mut out = String::new();
    let mut last = 0;
    for (count, captures) in r.regex.captures_iter(text).enumerate() {
        let whole = captures.get(0).expect("a match has its whole");
        if count >= r.max_matches {
            fail(Kind::MatchLimitExceeded, whole.start());
            return str_new(b"");
        }
        out.push_str(&text[last..whole.start()]);
        for part in &parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Index(i) => out.push_str(captures.get(*i).map_or("", |m| m.as_str())),
                Part::Name(n) => out.push_str(captures.name(n).map_or("", |m| m.as_str())),
            }
        }
        last = whole.end();
        if out.len() > r.max_output {
            fail(Kind::OutputLimitExceeded, last);
            return str_new(b"");
        }
        if all == 0 {
            break;
        }
    }
    out.push_str(&text[last..]);
    if out.len() > r.max_output {
        fail(Kind::OutputLimitExceeded, text.len());
        return str_new(b"");
    }
    str_new(out.as_bytes())
}

/// An `Array[Int]` of the values.
fn ints(values: &[i64]) -> *mut ArrayHeader {
    let array = super::tondo_rt_array_new(8, values.len());
    for (index, &value) in values.iter().enumerate() {
        unsafe { *(super::element(array, 8, index) as *mut i64) = value };
    }
    array
}
