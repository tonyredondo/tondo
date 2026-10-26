//! The structure of a value as the debugger shows it (toolchain 7.6), so
//! that a tool can open it part by part and name a part by its path: a
//! record `Name { field: value }`, an array `[a, b]`, a map `["key": value]`,
//! a set `Set[a]`, a tuple `(a, b)`, a variant `Type.Variant(a)`, `some(a)`
//! or `ok(a)`, a member of a union `Int: 3`, texts and characters in quotes,
//! and anything else as it is.

/// A part of a value: its text, and its own parts, each by the name that
/// follows its parent's path: `.field`, `.0`, `[2]` or `["key"]`.
pub(crate) struct Part<'a> {
    pub text: &'a str,
    pub parts: Vec<(String, Part<'a>)>,
}

/// The parts of a value's text; what does not read as a value is one part
/// without parts.
pub(crate) fn parse(text: &str) -> Part<'_> {
    let mut at = 0;
    match value(text, &mut at) {
        Some(part) if at == text.len() => part,
        _ => Part { text, parts: Vec::new() },
    }
}

/// The part a path names, from the parts of the value it starts at:
/// `.field`, `.0`, `[2]` or `["key"]`, one after another.
pub(crate) fn find<'p, 'a>(mut part: &'p Part<'a>, mut path: &str) -> Option<&'p Part<'a>> {
    while !path.is_empty() {
        let length = match path.as_bytes()[0] {
            b'.' => path[1..].find(['.', '[']).map_or(path.len(), |i| i + 1),
            b'[' => closing(path)? + 1,
            _ => return None,
        };
        let (step, rest) = path.split_at(length);
        part = &part.parts.iter().find(|(name, _)| name == step)?.1;
        path = rest;
    }
    Some(part)
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

fn value<'a>(text: &'a str, at: &mut usize) -> Option<Part<'a>> {
    let start = *at;
    let leaf = |end: usize| Part { text: &text[start..end], parts: Vec::new() };
    match text.as_bytes().get(start)? {
        b'"' | b'\'' => {
            *at = quote_end(text, start)?;
            Some(leaf(*at))
        }
        b'[' => {
            let parts = items(text, at, b']', true)?;
            Some(Part { text: &text[start..*at], parts })
        }
        b'(' => {
            let parts = items(text, at, b')', false)?;
            Some(Part { text: &text[start..*at], parts: numbered(parts) })
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
                let parts = items(text, at, b')', false)?;
                return Some(Part { text: &text[start..*at], parts: numbered(parts) });
            }
            if rest.starts_with('[') && word == "Set" {
                let parts = items(text, at, b']', true)?;
                return Some(Part { text: &text[start..*at], parts });
            }
            if rest.starts_with(" { ") {
                // A record's fields, by name.
                *at += 3;
                let mut parts = Vec::new();
                loop {
                    let colon = text[*at..].find(": ")? + *at;
                    let name = format!(".{}", &text[*at..colon]);
                    *at = colon + 2;
                    parts.push((name, value(text, at)?));
                    if text[*at..].starts_with(", ") {
                        *at += 2;
                    } else if text[*at..].starts_with(" }") {
                        *at += 2;
                        return Some(Part { text: &text[start..*at], parts });
                    } else {
                        return None;
                    }
                }
            }
            if rest.starts_with(": ") && word.chars().next().is_some_and(char::is_uppercase) {
                // A member of a union: its value's parts.
                *at += 2;
                let inner = value(text, at)?;
                return Some(Part { text: &text[start..*at], parts: inner.parts });
            }
            Some(leaf(end))
        }
    }
}

/// The items between brackets, `[`…`]` or `(`…`)`: each by its index, or
/// by its key when they are a map's `key: value`.
fn items<'a>(text: &'a str, at: &mut usize, close: u8, keyed: bool) -> Option<Vec<(String, Part<'a>)>> {
    *at += 1;
    let mut parts = Vec::new();
    // An empty map.
    if keyed && text[*at..].starts_with(":]") {
        *at += 2;
        return Some(parts);
    }
    if text.as_bytes().get(*at) == Some(&close) {
        *at += 1;
        return Some(parts);
    }
    loop {
        let item = value(text, at)?;
        let part = if keyed && text[*at..].starts_with(": ") {
            *at += 2;
            (format!("[{}]", item.text), value(text, at)?)
        } else {
            (format!("[{}]", parts.len()), item)
        };
        parts.push(part);
        match text.as_bytes().get(*at)? {
            b',' if text[*at..].starts_with(", ") => *at += 2,
            &c if c == close => {
                *at += 1;
                return Some(parts);
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
        assert_eq!(find(&user, ".name").unwrap().text, r#""Ada, \"the first\"""#);
        assert_eq!(
            names(find(&user, ".tags").unwrap()),
            [("[0]".into(), "\"a\"".into()), ("[1]".into(), "\"b\"".into())]
        );
        assert_eq!(find(&user, r#".scores["y"]"#).unwrap().text, "2");
        assert_eq!(find(&user, ".at.1").unwrap().text, "2.5");
        assert_eq!(find(&user, ".role.0").unwrap().text, "3");
        assert_eq!(find(&user, ".extra.0[0]").unwrap().text, "1");
        assert!(find(&user, ".missing").is_none());
        // A union's member, an empty map, and what is not a value.
        assert_eq!(names(&parse("Shape: Shape.Circle(1.5)")), [(".0".into(), "1.5".into())]);
        assert!(parse("[:]").parts.is_empty());
        let odd = parse("fn(Int): Int");
        assert_eq!((odd.text, odd.parts.len()), ("fn(Int): Int", 0));
        assert_eq!(parse("[1, 2, 3...").parts.len(), 0);
    }
}
