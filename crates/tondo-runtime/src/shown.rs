//! The structure of a value as the debugger shows it (toolchain 7.6), so
//! that a tool can open it part by part and name a part by its path: a
//! record `Name { field: value }`, an array `[a, b]`, a map `["key": value]`,
//! a set `Set[a]`, a tuple `(a, b)`, a variant `Type.Variant(a)`, `some(a)`
//! or `ok(a)`, a member of a union `Int: 3`, texts and characters in quotes,
//! and anything else as it is.

/// A part of a value: its text, and its own parts, each by the name that
/// follows its parent's path: `.field`, `.0`, `[2]` or `["key"]`; and
/// whether those are by position, an array's or a set's.
pub(crate) struct Part<'a> {
    pub text: &'a str,
    pub parts: Vec<(String, Part<'a>)>,
    pub indexed: bool,
}

/// The parts of a value's text, one level down: theirs are not read, so
/// that a value of any size opens part by part. What does not read as a
/// value is one part without parts.
pub(crate) fn parse(text: &str) -> Part<'_> {
    let mut at = 0;
    match value(text, &mut at, true) {
        Some(part) if at == text.len() => part,
        _ => leaf(text),
    }
}

fn leaf(text: &str) -> Part<'_> {
    Part { text, parts: Vec::new(), indexed: false }
}

/// The text of the part a path names in a value's text: `.field`, `.0`,
/// `[2]` or `["key"]`, one after another.
pub(crate) fn find<'a>(mut text: &'a str, mut path: &str) -> Option<&'a str> {
    while !path.is_empty() {
        let length = match path.as_bytes()[0] {
            b'.' => path[1..].find(['.', '[']).map_or(path.len(), |i| i + 1),
            b'[' => closing(path)? + 1,
            _ => return None,
        };
        let (step, rest) = path.split_at(length);
        text = parse(text).parts.into_iter().find(|(name, _)| name == step)?.1.text;
        path = rest;
    }
    Some(text)
}

/// Where the `]` that closes the `[` a path starts with is, past quotes.
fn closing(path: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, c) in path.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ']' if !quoted => return Some(index),
            _ => {}
        }
    }
    None
}

/// A value from `at`, and, when `open`, its parts, but not theirs.
fn value<'a>(text: &'a str, at: &mut usize, open: bool) -> Option<Part<'a>> {
    let start = *at;
    let part = |at: usize, parts: Vec<(String, Part<'a>)>, indexed| Part { text: &text[start..at], parts, indexed };
    match text.as_bytes().get(start)? {
        b'"' | b'\'' => {
            *at = quote_end(text, start)?;
            Some(leaf(&text[start..*at]))
        }
        b'[' => {
            let (parts, keyed) = items(text, at, b']', true, open)?;
            Some(part(*at, parts, !keyed))
        }
        b'(' => {
            let (parts, _) = items(text, at, b')', false, open)?;
            Some(part(*at, numbered(parts), false))
        }
        _ => {
            let end =
                text[start..].find([' ', ',', '(', ')', '[', ']', '{', '}', ':']).map_or(text.len(), |i| start + i);
            if end == start {
                return None;
            }
            let word = &text[start..end];
            *at = end;
            let rest = &text[end..];
            if rest.starts_with('(') {
                // A variant and its payload.
                let (parts, _) = items(text, at, b')', false, open)?;
                return Some(part(*at, numbered(parts), false));
            }
            if rest.starts_with('[') && word == "Set" {
                let (parts, _) = items(text, at, b']', false, open)?;
                return Some(part(*at, parts, true));
            }
            if rest.starts_with(" { ") {
                // A record's fields, by name.
                *at += 3;
                let mut parts = Vec::new();
                loop {
                    let colon = text[*at..].find(": ")? + *at;
                    let name = &text[*at..colon];
                    *at = colon + 2;
                    let field = value(text, at, false)?;
                    if open {
                        parts.push((format!(".{name}"), field));
                    }
                    if text[*at..].starts_with(", ") {
                        *at += 2;
                    } else if text[*at..].starts_with(" }") {
                        *at += 2;
                        return Some(part(*at, parts, false));
                    } else {
                        return None;
                    }
                }
            }
            if rest.starts_with(": ") && word.chars().next().is_some_and(char::is_uppercase) {
                // A member of a union: its value's parts.
                *at += 2;
                let inner = value(text, at, open)?;
                return Some(part(*at, inner.parts, inner.indexed));
            }
            Some(leaf(word))
        }
    }
}

/// The items between brackets, `[`…`]` or `(`…`)`, when `open`: each by
/// its index, or by its key when they are a map's `key: value`, which
/// the second says.
fn items<'a>(
    text: &'a str,
    at: &mut usize,
    close: u8,
    keys: bool,
    open: bool,
) -> Option<(Vec<(String, Part<'a>)>, bool)> {
    *at += 1;
    let mut parts = Vec::new();
    // An empty map.
    if keys && text[*at..].starts_with(":]") {
        *at += 2;
        return Some((parts, true));
    }
    if text.as_bytes().get(*at) == Some(&close) {
        *at += 1;
        return Some((parts, false));
    }
    let mut keyed = false;
    loop {
        let item = value(text, at, false)?;
        let entry = match keys && text[*at..].starts_with(": ") {
            true => {
                *at += 2;
                keyed = true;
                Some(value(text, at, false)?)
            }
            false => None,
        };
        if open {
            parts.push(match entry {
                Some(entry) => (format!("[{}]", item.text), entry),
                None => (format!("[{}]", parts.len()), item),
            });
        }
        match text.as_bytes().get(*at)? {
            b',' if text[*at..].starts_with(", ") => *at += 2,
            &c if c == close => {
                *at += 1;
                return Some((parts, keyed));
            }
            _ => return None,
        }
    }
}

/// A tuple's or a payload's items, by their place: `.0`, `.1`.
fn numbered(parts: Vec<(String, Part<'_>)>) -> Vec<(String, Part<'_>)> {
    parts.into_iter().enumerate().map(|(index, (_, part))| (format!(".{index}"), part)).collect()
}

/// Where a text or character in quotes ends, past its escapes.
fn quote_end(text: &str, start: usize) -> Option<usize> {
    let quote = text.as_bytes()[start];
    let mut escaped = false;
    for (index, &byte) in text.as_bytes().iter().enumerate().skip(start + 1) {
        match byte {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            _ if byte == quote => return Some(index + 1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(part: &Part) -> Vec<(String, String)> {
        part.parts.iter().map(|(name, part)| (name.clone(), part.text.to_string())).collect()
    }

    #[test]
    fn reads_values_part_by_part() {
        let shown = r#"User { name: "Ada, \"the first\"", tags: ["a", "b"], scores: ["x": 1, "y": 2], at: (1, 2.5), role: Role.Member(3), extra: some(Set[1]) }"#;
        let user = parse(shown);
        assert_eq!(user.text, shown);
        let fields: Vec<String> = user.parts.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(fields, [".name", ".tags", ".scores", ".at", ".role", ".extra"]);
        // One level down: a part's own parts wait to be opened.
        assert!(user.parts.iter().all(|(_, part)| part.parts.is_empty()));
        assert_eq!(find(shown, ".name").unwrap(), r#""Ada, \"the first\"""#);
        let tags = parse(find(shown, ".tags").unwrap());
        assert_eq!(names(&tags), [("[0]".into(), "\"a\"".into()), ("[1]".into(), "\"b\"".into())]);
        assert!(tags.indexed && !parse(find(shown, ".scores").unwrap()).indexed);
        assert!(parse(find(shown, ".extra.0").unwrap()).indexed);
        assert_eq!(find(shown, r#".scores["y"]"#).unwrap(), "2");
        assert_eq!(find(shown, ".at.1").unwrap(), "2.5");
        assert_eq!(find(shown, ".role.0").unwrap(), "3");
        assert_eq!(find(shown, ".extra.0[0]").unwrap(), "1");
        assert!(find(shown, ".missing").is_none());
        // A union's member, an empty map, a map keyed by numbers, and what
        // is not a value.
        assert_eq!(names(&parse("Shape: Shape.Circle(1.5)")), [(".0".into(), "1.5".into())]);
        assert!(parse("[:]").parts.is_empty());
        assert!(!parse("[0: 1]").indexed);
        let odd = parse("fn(Int): Int");
        assert_eq!((odd.text, odd.parts.len()), ("fn(Int): Int", 0));
        assert_eq!(parse("[1, 2, 3...").parts.len(), 0);
    }

    #[test]
    fn finds_a_part_of_a_large_value() {
        let shown = format!("[{}]", (0..200_000).map(|i| format!("P {{ n: {i} }}")).collect::<Vec<_>>().join(", "));
        assert_eq!(parse(&shown).parts.len(), 200_000);
        assert_eq!(find(&shown, "[199999].n"), Some("199999"));
    }
}
