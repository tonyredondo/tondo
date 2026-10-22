//! The text `testing.snapshot` compares (testing 9.7): a `String` as it
//! is, or the JSON of any other value, indented with its keys sorted, with
//! the values at `ignore` paths and the ones `matches` checks replaced by
//! markers; then the `replace` substitutions, in order.

/// Why a snapshot cannot be taken: a value that does not have the form
/// `matches` asks for (`P2007`), or a bad path or pattern (`P2008`).
pub(crate) enum Refused {
    Mismatch(String),
    Invalid(String),
}

enum Json {
    /// `null`, a Boolean or a number, as written.
    Raw(String),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// A step from a value to one inside it.
enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

/// A path of `ignore` or `matches`: one key, at any depth, or a dotted path
/// from the root whose segments are keys, indexes or `*`, any of them.
enum Path<'a> {
    Anywhere(&'a str),
    Rooted(Vec<&'a str>),
}

impl<'a> Path<'a> {
    fn new(text: &'a str) -> Result<Path<'a>, Refused> {
        let segments: Vec<&str> = text.split('.').collect();
        if segments.iter().any(|s| s.is_empty()) {
            return Err(Refused::Invalid(format!(
                "{text:?} is not a path: write a key such as \"id\", or a path such as \"items.*.id\""
            )));
        }
        Ok(match segments[..] {
            [key] if key != "*" => Path::Anywhere(key),
            _ => Path::Rooted(segments),
        })
    }

    fn reaches(&self, steps: &[Step]) -> bool {
        match self {
            Path::Anywhere(key) => matches!(steps.last(), Some(Step::Key(k)) if k == key),
            Path::Rooted(segments) => {
                segments.len() == steps.len()
                    && segments.iter().zip(steps).all(|(segment, step)| match step {
                        _ if *segment == "*" => true,
                        Step::Key(key) => key == segment,
                        Step::Index(index) => segment.parse() == Ok(*index),
                    })
            }
        }
    }
}

/// The text to compare for a value: `text` itself, or its JSON when `json`.
pub(crate) fn render(
    text: String,
    json: bool,
    ignore: &[String],
    matches: &[(String, String)],
    replace: &[(String, String)],
) -> Result<String, Refused> {
    let mut out = text;
    if json {
        let mut value = Parser { text: out.as_bytes(), at: 0 }.value();
        let ignore = ignore.iter().map(|p| Path::new(p)).collect::<Result<Vec<_>, _>>()?;
        let mut checks = Vec::new();
        for (path, pattern) in matches {
            // Checked on its own, so that an error's offset is in it.
            compile(pattern)?;
            checks.push((Path::new(path)?, pattern, compile(&format!(r"\A(?:{pattern})\z"))?));
        }
        mark(&mut value, &mut Vec::new(), &ignore, &checks)?;
        out = String::new();
        write(&value, 0, &mut out);
    }
    for (pattern, with) in replace {
        out = compile(pattern)?.replace_all(&out, regex::NoExpand(with)).into_owned();
    }
    Ok(out)
}

fn compile(pattern: &str) -> Result<regex::Regex, Refused> {
    super::regex::pattern(pattern)
        .map_err(|why| Refused::Invalid(format!("the pattern {pattern:?} is not valid: {why}")))
}

/// Replaces what the paths reach: an ignored value by `"[ignored]"`, and a
/// value its pattern matches whole by `"[matches <pattern>]"`.
fn mark<'a>(
    value: &'a mut Json,
    steps: &mut Vec<Step<'a>>,
    ignore: &[Path],
    checks: &[(Path, &String, regex::Regex)],
) -> Result<(), Refused> {
    if !steps.is_empty() {
        if ignore.iter().any(|p| p.reaches(steps)) {
            *value = Json::Str("[ignored]".into());
            return Ok(());
        }
        if let Some((_, pattern, regex)) = checks.iter().find(|(p, ..)| p.reaches(steps)) {
            let text = match &*value {
                Json::Str(s) => s.clone(),
                Json::Raw(r) => r.clone(),
                other => {
                    let mut out = String::new();
                    write(other, 0, &mut out);
                    out
                }
            };
            if !regex.is_match(&text) {
                let path: Vec<String> = steps
                    .iter()
                    .map(|s| match s {
                        Step::Key(k) => k.to_string(),
                        Step::Index(i) => i.to_string(),
                    })
                    .collect();
                return Err(Refused::Mismatch(format!(
                    "the value at {:?} is {text:?}, which does not match the pattern {pattern:?}",
                    path.join(".")
                )));
            }
            *value = Json::Str(format!("[matches {pattern}]"));
            return Ok(());
        }
    }
    match value {
        Json::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                steps.push(Step::Index(index));
                mark(item, steps, ignore, checks)?;
                steps.pop();
            }
        }
        Json::Object(members) => {
            for (key, member) in members.iter_mut() {
                steps.push(Step::Key(key));
                mark(member, steps, ignore, checks)?;
                steps.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

/// Indented by two spaces, with the keys of each object sorted.
fn write(value: &Json, indent: usize, out: &mut String) {
    let open = |out: &mut String, items: usize, bracket: char| {
        out.push(bracket);
        if items > 0 {
            out.push('\n');
        }
    };
    let close = |out: &mut String, items: usize, bracket: char| {
        if items > 0 {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
        }
        out.push(bracket);
    };
    match value {
        Json::Raw(text) => out.push_str(text),
        Json::Str(text) => quote(text, out),
        Json::Array(items) => {
            open(out, items.len(), '[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&" ".repeat(indent + 2));
                write(item, indent + 2, out);
            }
            close(out, items.len(), ']');
        }
        Json::Object(members) => {
            let mut sorted: Vec<&(String, Json)> = members.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            open(out, sorted.len(), '{');
            for (index, (key, member)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&" ".repeat(indent + 2));
                quote(key, out);
                out.push_str(": ");
                write(member, indent + 2, out);
            }
            close(out, members.len(), '}');
        }
    }
}

fn quote(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Reads the JSON `std.json` wrote, which is well formed.
struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.text.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Json {
        self.skip_space();
        match self.text.get(self.at) {
            Some(b'"') => Json::Str(self.string()),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                while !self.close(b']') {
                    items.push(self.value());
                }
                Json::Array(items)
            }
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                while !self.close(b'}') {
                    self.skip_space();
                    let key = self.string();
                    self.skip_space();
                    self.at += 1; // `:`
                    members.push((key, self.value()));
                }
                Json::Object(members)
            }
            _ => {
                let start = self.at;
                while self.text.get(self.at).is_some_and(|b| !b",]} \n\r\t".contains(b)) {
                    self.at += 1;
                }
                Json::Raw(String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
            }
        }
    }

    /// Passes a `,` and tells whether the bracket closes.
    fn close(&mut self, bracket: u8) -> bool {
        self.skip_space();
        match self.text.get(self.at) {
            Some(&b) if b == bracket => {
                self.at += 1;
                true
            }
            Some(b',') => {
                self.at += 1;
                false
            }
            Some(_) => false,
            None => true,
        }
    }

    fn string(&mut self) -> String {
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        while let Some(&b) = self.text.get(self.at) {
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let escape = self.text.get(self.at).copied().unwrap_or(b'\\');
                    self.at += 1;
                    match escape {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let mut code = self.hex();
                            if (0xD800..0xDC00).contains(&code) && self.text.get(self.at..self.at + 2) == Some(b"\\u") {
                                self.at += 2;
                                code = 0x10000 + ((code - 0xD800) << 10) + (self.hex().wrapping_sub(0xDC00) & 0x3FF);
                            }
                            let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn hex(&mut self) -> u32 {
        let digits = self.text.get(self.at..self.at + 4).unwrap_or_default();
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).unwrap_or("0"), 16).unwrap_or(0xFFFD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn renders_json_with_markers() {
        let json = r#"{"total":12,"id":"a1","items":[{"name":"book","id":"x"},{"name":"pen","id":"y"}],"user":{"id":7,"at":"2026-10-22"},"empty":[],"note":"line\n\"q\"é"}"#;
        let ignore = ["id".to_string()];
        let matches = pairs(&[("user.at", r"\d{4}-\d{2}-\d{2}")]);
        let Ok(text) = render(json.into(), true, &ignore, &matches, &[]) else { panic!() };
        assert_eq!(
            text,
            r#"{
  "empty": [],
  "id": "[ignored]",
  "items": [
    {
      "id": "[ignored]",
      "name": "book"
    },
    {
      "id": "[ignored]",
      "name": "pen"
    }
  ],
  "note": "line\n\"q\"é",
  "total": 12,
  "user": {
    "at": "[matches \\d{4}-\\d{2}-\\d{2}]",
    "id": "[ignored]"
  }
}"#
        );
        let rooted = ["items.*.id".to_string(), "items.0".to_string()];
        let Ok(text) = render(r#"{"id":1,"items":[{"id":2},{"id":3}]}"#.into(), true, &rooted, &[], &[]) else {
            panic!()
        };
        assert_eq!(
            text,
            "{\n  \"id\": 1,\n  \"items\": [\n    \"[ignored]\",\n    {\n      \"id\": \"[ignored]\"\n    }\n  ]\n}"
        );
    }

    #[test]
    fn refuses_values_and_paths() {
        let matches = pairs(&[("id", "[0-9]+")]);
        let Err(Refused::Mismatch(why)) = render(r#"{"id":"12a"}"#.into(), true, &[], &matches, &[]) else { panic!() };
        assert_eq!(why, r#"the value at "id" is "12a", which does not match the pattern "[0-9]+""#);
        assert!(matches!(render("{}".into(), true, &["a..b".into()], &[], &[]), Err(Refused::Invalid(_))));
        let bad = pairs(&[("id", "(")]);
        let Err(Refused::Invalid(why)) = render("{}".into(), true, &[], &bad, &[]) else { panic!() };
        assert_eq!(why, r#"the pattern "(" is not valid: regex.UnexpectedEnd at byte 0"#);
    }

    #[test]
    fn replaces_in_order() {
        let replace = pairs(&[(r"\d+ms", "[time]"), (r"\[time\]", "$0")]);
        let Ok(text) = render("took 12ms, then 3ms".into(), false, &[], &[], &replace) else { panic!() };
        assert_eq!(text, "took $0, then $0");
    }
}
