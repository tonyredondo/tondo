//! The text of the refactors of `tondo lsp` that touch more than a line:
//! finding the lists of a declaration and its calls. What they make is
//! checked by compiling it, so this only needs to read code that compiles.

/// A list in parentheses: where they are, and each item, without blanks.
#[derive(Debug, PartialEq)]
pub struct List {
    pub open: usize,
    pub close: usize,
    pub items: Vec<(usize, usize)>,
}

/// The list that starts at `at`, or after a `[...]` of type arguments there.
pub fn list_at(text: &str, at: usize) -> Option<List> {
    let bytes = text.as_bytes();
    let mut open = at;
    if bytes.get(open) == Some(&b'[') {
        open = closing(bytes, open)? + 1;
    }
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let close = closing(bytes, open)?;
    let mut items = Vec::new();
    for (start, end) in top_level(bytes, open + 1, close, b',') {
        let item = &text[start..end];
        let trimmed = item.trim();
        if !trimmed.is_empty() {
            let from = start + (item.len() - item.trim_start().len());
            items.push((from, from + trimmed.len()));
        }
    }
    Some(List { open, close, items })
}

/// Where the bracket at `open` closes.
fn closing(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => at = string_end(bytes, at)?,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The closing quote of the text that starts at `at`.
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut at = at + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 1,
            b'"' => return Some(at),
            _ => {}
        }
        at += 1;
    }
    None
}

/// The parts of `bytes[from..to]` between each `separator` outside brackets
/// and texts.
fn top_level(bytes: &[u8], from: usize, to: usize, separator: u8) -> Vec<(usize, usize)> {
    let (mut parts, mut start, mut depth, mut at) = (Vec::new(), from, 0, from);
    while at < to {
        match bytes[at] {
            b'"' => at = string_end(bytes, at).unwrap_or(to),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            byte if byte == separator && depth == 0 => {
                parts.push((start, at));
                start = at + 1;
            }
            _ => {}
        }
        at += 1;
    }
    parts.push((start, to));
    parts
}

/// A parameter as a list of them writes it: what it declares, its name,
/// and for a new one, the value its calls pass.
#[derive(Debug, PartialEq)]
pub struct Parameter {
    pub declared: String,
    pub name: String,
    pub value: Option<String>,
}

/// The name a parameter declares.
pub fn parameter_name(text: &str) -> &str {
    let text = text.trim();
    let text = text.strip_prefix("mut ").unwrap_or(text).trim_start();
    text.split([':', ' ']).next().unwrap_or(text)
}

/// A block's lines, one level further in or out than they are.
fn indented(block: &str, by: isize) -> String {
    let moved = block.trim_end().trim_start_matches('\n').lines().map(|line| match by {
        4 if !line.is_empty() => format!("    {line}\n"),
        -4 => format!("{}\n", line.strip_prefix("    ").unwrap_or(line)),
        _ => format!("{line}\n"),
    });
    moved.collect()
}

/// The `if condition { a } else { b }` that starts the line at `start` as a
/// `match` of its condition: where it ends, and the text.
pub fn if_as_match(text: &str, start: usize) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let line_end = text[start..].find('\n')? + start;
    let line = &text[start..line_end];
    let indent = &line[..line.len() - line.trim_start().len()];
    let condition = line.trim().strip_prefix("if ")?.strip_suffix('{')?.trim();
    let open = line_end - 1;
    let close = closing(bytes, open)?;
    let rest = text[close + 1..].strip_prefix(" else {")?;
    let second = close + 1 + " else {".len() - 1;
    let end = closing(bytes, second)?;
    if !rest.starts_with('\n') {
        return None;
    }
    let (first, other) = (indented(&text[open + 1..close], 4), indented(&text[second + 1..end], 4));
    let inner = format!("{indent}    ");
    let written = format!(
        "{indent}match {condition} {{\n{inner}true => {{\n{first}{inner}}}\n{inner}false => {{\n{other}{inner}}}\n{indent}}}"
    );
    Some((end + 1, written))
}

/// The `match` of `true` and `false` that starts the line at `start` as an
/// `if`: where it ends, and the text.
pub fn match_as_if(text: &str, start: usize) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let line_end = text[start..].find('\n')? + start;
    let line = &text[start..line_end];
    let indent = &line[..line.len() - line.trim_start().len()];
    let condition = line.trim().strip_prefix("match ")?.strip_suffix('{')?.trim();
    let open = line_end - 1;
    let end = closing(bytes, open)?;
    // Each arm: its value, and its block, or the rest of its line.
    let arm = |name: &str| -> Option<String> {
        let at = text[open..end].find(&format!("{name} =>"))? + open + name.len() + " =>".len();
        let body = text[at..end].trim_start();
        let from = end - body.len();
        match body.starts_with('{') {
            true => Some(indented(&text[from + 1..closing(bytes, from)?], -4)),
            false => {
                let value = body.lines().next()?.trim().trim_end_matches(',');
                Some(format!("{indent}    {value}\n"))
            }
        }
    };
    let (yes, no) = (arm("true")?, arm("false")?);
    Some((end + 1, format!("{indent}if {condition} {{\n{yes}{indent}}} else {{\n{no}{indent}}}")))
}

/// Where the value a `.` at `dot` is after starts: a name, what calls and
/// indexes follow it, and the names before it.
pub fn receiver_start(text: &str, dot: usize) -> usize {
    let bytes = text.as_bytes();
    let mut at = dot;
    while at > 0 {
        let byte = bytes[at - 1];
        if byte == b')' || byte == b']' {
            let open = if byte == b')' { b'(' } else { b'[' };
            let mut depth = 0;
            while at > 0 {
                at -= 1;
                if bytes[at] == byte {
                    depth += 1;
                } else if bytes[at] == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
        } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
            at -= 1;
        } else {
            break;
        }
    }
    at
}

/// A list of parameters as a person writes it: `a: Int, b: String = "x"`.
pub fn parameters(list: &str) -> Result<Vec<Parameter>, String> {
    let bytes = list.as_bytes();
    let mut found = Vec::new();
    for (start, end) in top_level(bytes, 0, list.len(), b',') {
        let item = list[start..end].trim();
        if item.is_empty() {
            continue;
        }
        let parts = top_level(item.as_bytes(), 0, item.len(), b'=');
        let (declared, value) = match parts[..] {
            [(a, b)] => (item[a..b].trim(), None),
            [(a, b), (c, d)] => (item[a..b].trim(), Some(item[c..d].trim().to_string())),
            _ => return Err(format!("`{item}` is not `name: Type`, or `name: Type = value`")),
        };
        if !declared.contains(':') {
            return Err(format!("`{item}` is not `name: Type`, or `name: Type = value`"));
        }
        found.push(Parameter { declared: declared.to_string(), name: parameter_name(declared).to_string(), value });
    }
    Ok(found)
}

/// The arguments of a call once the parameters `old` become `new`. A
/// parameter that goes takes its argument with it, one that stays moves
/// with it, and a new one passes its value.
pub fn arguments(old: &[String], new: &[Parameter], given: &[&str]) -> Result<Vec<String>, String> {
    let named =
        |argument: &str, name: &str| argument.strip_prefix(name).is_some_and(|rest| rest.trim_start().starts_with(':'));
    let mut written = Vec::new();
    for parameter in new {
        let at = old.iter().position(|name| *name == parameter.name);
        let by_name = given.iter().find(|argument| named(argument, &parameter.name));
        match (by_name, at.and_then(|at| given.get(at)), &parameter.value) {
            (Some(argument), ..) => written.push(argument.to_string()),
            (None, Some(argument), _) => written.push(argument.to_string()),
            (None, None, Some(value)) => written.push(value.clone()),
            (None, None, None) if at.is_some() => {}
            (None, None, None) => {
                let name = &parameter.name;
                return Err(format!("`{name}` is new, so its calls need a value: write `{name}: Type = value`"));
            }
        }
    }
    Ok(written)
}

/// The body of a function that is one expression on a line of its own or
/// after its signature: its span within `declaration`.
pub fn single_expression(declaration: &str, after_parameters: usize) -> Option<(usize, usize)> {
    let open = after_parameters + declaration[after_parameters..].find('{')?;
    let close = open + closing(declaration.as_bytes(), open)? - open;
    let body = &declaration[open + 1..close];
    let trimmed = body.trim();
    let statement = ["let ", "var ", "return", "fail ", "for ", "with ", "defer "];
    if trimmed.is_empty()
        || trimmed.contains('\n')
        || trimmed.contains('?')
        || statement.iter().any(|s| trimmed.starts_with(s))
    {
        return None;
    }
    let from = open + 1 + (body.len() - body.trim_start().len());
    Some((from, from + trimmed.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items<'a>(text: &'a str, list: &List) -> Vec<&'a str> {
        list.items.iter().map(|&(a, b)| &text[a..b]).collect()
    }

    #[test]
    fn a_list_goes_past_what_nests_in_it() {
        let text = r#"f(a, g(b, c), "x, (y", [1, 2], { d: 1, e: 2 }) + 1"#;
        let list = list_at(text, 1).unwrap();
        assert_eq!(items(text, &list), ["a", "g(b, c)", r#""x, (y""#, "[1, 2]", "{ d: 1, e: 2 }"]);
        assert_eq!(&text[list.close..list.close + 1], ")");
        assert_eq!(list_at("f()", 1).unwrap().items, []);
        assert_eq!(items("f[Int](1)", &list_at("f[Int](1)", 1).unwrap()), ["1"]);
        assert_eq!(list_at("f + 1", 1), None);
    }

    #[test]
    fn parameters_read_as_they_are_written() {
        let read = parameters(r#"a: Int, mut b: Map[String, Int], c: String = "x, y""#).unwrap();
        assert_eq!(read[1].name, "b");
        assert_eq!(read[1].declared, "mut b: Map[String, Int]");
        assert_eq!(read[2].value.as_deref(), Some(r#""x, y""#));
        assert!(parameters("nothing").is_err());
    }

    #[test]
    fn arguments_follow_their_parameters() {
        let old = ["a".to_string(), "b".to_string(), "c".to_string()];
        let new = parameters("c: Int, a: Int, d: Int = 0").unwrap();
        assert_eq!(arguments(&old, &new, &["1", "2", "3"]).unwrap(), ["3", "1", "0"]);
        // By name, wherever they are.
        assert_eq!(arguments(&old, &new, &["c: 3", "a: 1"]).unwrap(), ["c: 3", "a: 1", "0"]);
        let missing = parameters("d: Int").unwrap();
        assert!(arguments(&old, &missing, &["1", "2", "3"]).unwrap_err().contains("`d` is new"));
    }

    #[test]
    fn only_a_single_expression_inlines() {
        let one = "fn f(a: Int): Int {\n    a + 1\n}\n";
        let (from, to) = single_expression(one, 8).unwrap();
        assert_eq!(&one[from..to], "a + 1");
        assert_eq!(single_expression("fn f() {\n    let a = 1\n    a\n}", 6), None);
        assert_eq!(single_expression("fn f(): Int ! E {\n    g()?\n}", 6), None);
    }
}
