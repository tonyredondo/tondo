//! The sentences of a bound that data passed (standard library 7.4.1): what
//! passed it, the bound in the unit a person reads, the field of `limits`
//! that sets it and the code that raises it, always in the same shape, so
//! that the second such error a program meets reads like the first.

/// `bytes` as a person reads it: in KiB, MiB or GiB when it is a whole
/// number of one, else in bytes.
pub(crate) fn size(bytes: u64) -> String {
    for (shift, unit) in [(30, "GiB"), (20, "MiB"), (10, "KiB")] {
        if bytes >= 1 << shift && bytes.trailing_zeros() >= shift {
            return format!("{} {unit}", bytes >> shift);
        }
    }
    if bytes == 1 { "1 byte".into() } else { format!("{bytes} bytes") }
}

/// The sentence of a size bound passed: `what` "is larger than" the bound,
/// or `is` as the caller words it ("is longer than" for a line), and then
/// the field of `module.ty` that sets it and the code that raises it, with
/// `besides`, another way that needs no limit, after it.
pub(crate) fn passed(what: &str, is: &str, limit: u64, module: &str, ty: &str, field: &str, besides: &str) -> String {
    let raise = format!("`limits: {module}.{ty}.Default with {{ {field}: … }}`");
    let mut sentence = shaped(what, is, &size(limit), ty, field, &raise);
    if !besides.is_empty() {
        sentence.push_str(", or ");
        sentence.push_str(besides);
    }
    sentence
}

/// The sentence of a bound on a count passed: `what` `is` `count` of
/// `unit`, as "the input has more than 1000000 matches".
pub(crate) fn counted(what: &str, is: &str, count: u64, unit: &str, module: &str, ty: &str, field: &str) -> String {
    let raise = format!("`limits: {module}.{ty}.Default with {{ {field}: … }}`");
    shaped(what, is, &format!("{count} {unit}"), ty, field, &raise)
}

/// The shape every one of them has.
fn shaped(what: &str, is: &str, amount: &str, ty: &str, field: &str, raise: &str) -> String {
    format!("{what} {is} {amount}, the most `{ty}.{field}` allows: raise it with {raise}")
}

/// The sentence of a nesting bound passed: `what` "nests deeper than" the
/// bound in levels, and, where the stack of a task bounds how far it may be
/// raised, its `ceiling`.
pub(crate) fn deeper(what: &str, limit: u64, module: &str, ty: &str, ceiling: Option<u64>) -> String {
    deeper_up_to(what, limit, module, ty, &ceiling.map(|c| c.to_string()).unwrap_or_default())
}

/// `deeper`, with how far it may be raised in words, as YAML's `1200, and
/// 255 in flow style`, or nothing.
pub(crate) fn deeper_up_to(what: &str, limit: u64, module: &str, ty: &str, up_to: &str) -> String {
    let field = "maxNestingDepth";
    let raise = format!("`limits: {module}.{ty}.Default with {{ {field}: … }}`");
    let levels = if limit == 1 { "1 level".to_string() } else { format!("{limit} levels") };
    let mut sentence = shaped(what, "nests deeper than", &levels, ty, field, &raise);
    if !up_to.is_empty() {
        sentence.push_str(&format!(", up to {up_to}"));
    }
    sentence
}

/// That a `maxNestingDepth` given is more than its `ceiling`, what a task's
/// stack holds for the format: refused where the limits are used, never cut
/// to the ceiling in silence.
pub(crate) fn above_ceiling(ty: &str, given: u64, ceiling: u64) -> String {
    format!("`{ty}.maxNestingDepth` {given} is more than a task's stack holds: {ceiling} at most")
}

/// `above_ceiling` where the ceiling is the parser's own bound, lower than
/// what a task's stack holds, as TOML's.
pub(crate) fn above_parser(ty: &str, given: u64, ceiling: u64) -> String {
    format!("`{ty}.maxNestingDepth` {given} is more than its parser reads: {ceiling} at most")
}

/// `io.IoError.TooLarge(limit)`, and fs's and net's, as it shows: the one
/// bound of those modules is a line's.
pub(crate) fn line(limit: u64) -> String {
    passed("a line", "is longer than", limit, "io", "IoLimits", "maxLineBytes", "")
}

/// `passed` for the library's code written in Tondo, as
/// `std.internal.text`'s `limitPassed`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tondo_rt_limit_passed(
    what: *const crate::Str,
    is: *const crate::Str,
    limit: i64,
    module: *const crate::Str,
    ty: *const crate::Str,
    field: *const crate::Str,
    besides: *const crate::Str,
    region: *mut crate::regions::Region,
) -> *mut crate::Str {
    let text = |s: &*const crate::Str| String::from_utf8_lossy(unsafe { crate::str_bytes(s) }).into_owned();
    let sentence = passed(
        &text(&what),
        &text(&is),
        limit.max(0) as u64,
        &text(&module),
        &text(&ty),
        &text(&field),
        &text(&besides),
    );
    crate::strings::str_in(region, sentence.as_bytes())
}

/// `counted` for the library's code written in Tondo, as
/// `std.internal.text`'s `limitCounted`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tondo_rt_limit_counted(
    what: *const crate::Str,
    is: *const crate::Str,
    count: i64,
    unit: *const crate::Str,
    module: *const crate::Str,
    ty: *const crate::Str,
    field: *const crate::Str,
    region: *mut crate::regions::Region,
) -> *mut crate::Str {
    let text = |s: &*const crate::Str| String::from_utf8_lossy(unsafe { crate::str_bytes(s) }).into_owned();
    let count = count.max(0) as u64;
    let sentence = counted(&text(&what), &text(&is), count, &text(&unit), &text(&module), &text(&ty), &text(&field));
    crate::strings::str_in(region, sentence.as_bytes())
}

/// `deeper` for the library's code written in Tondo, as
/// `std.internal.text`'s `limitDeeper`; a `ceiling` of 0 is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_limit_deeper(
    what: *const crate::Str,
    limit: i64,
    module: *const crate::Str,
    ty: *const crate::Str,
    ceiling: i64,
    region: *mut crate::regions::Region,
) -> *mut crate::Str {
    let text = |s: &*const crate::Str| String::from_utf8_lossy(unsafe { crate::str_bytes(s) }).into_owned();
    let ceiling = (ceiling > 0).then_some(ceiling as u64);
    let sentence = deeper(&text(&what), limit.max(0) as u64, &text(&module), &text(&ty), ceiling);
    crate::strings::str_in(region, sentence.as_bytes())
}

/// `above_ceiling` for the library's code written in Tondo, as
/// `std.internal.text`'s `limitCeiling`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_limit_ceiling(
    ty: *const crate::Str,
    given: i64,
    ceiling: i64,
    region: *mut crate::regions::Region,
) -> *mut crate::Str {
    let ty = String::from_utf8_lossy(unsafe { crate::str_bytes(&ty) }).into_owned();
    let sentence = above_ceiling(&ty, given.max(0) as u64, ceiling.max(0) as u64);
    crate::strings::str_in(region, sentence.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_line_too_large(b: *mut super::Builder, limit: i64) {
    unsafe { (*b).extend(line(limit.max(0) as u64).as_bytes()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_the_largest_whole_unit() {
        assert_eq!(size(1 << 20), "1 MiB");
        assert_eq!(size(128 << 20), "128 MiB");
        assert_eq!(size(2 << 30), "2 GiB");
        assert_eq!(size(64 << 10), "64 KiB");
        assert_eq!(size(1000), "1000 bytes");
        assert_eq!(size((1 << 20) + 1), "1048577 bytes");
        assert_eq!(size(1), "1 byte");
    }

    #[test]
    fn a_depth_says_its_bound_its_ceiling_and_how_to_raise_it() {
        assert_eq!(
            deeper("the document", 128, "json", "JsonLimits", Some(2300)),
            "the document nests deeper than 128 levels, the most `JsonLimits.maxNestingDepth` allows: raise it with `limits: json.JsonLimits.Default with { maxNestingDepth: … }`, up to 2300"
        );
        assert_eq!(
            above_ceiling("XmlLimits", 5000, 700),
            "`XmlLimits.maxNestingDepth` 5000 is more than a task's stack holds: 700 at most"
        );
    }

    #[test]
    fn a_line_says_its_bound_and_how_to_raise_it() {
        assert_eq!(
            line(1 << 20),
            "a line is longer than 1 MiB, the most `IoLimits.maxLineBytes` allows: raise it with `limits: io.IoLimits.Default with { maxLineBytes: … }`"
        );
    }
}
