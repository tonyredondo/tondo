//! The JSON of fuzz inputs, as `json.encode` writes it and the mutator
//! changes it: read into a tree, and written back for `json.decode`.

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Json>),
    Map(Vec<(String, Json)>),
}

impl Json {
    /// The value as JSON text.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Int(value) => out.push_str(&value.to_string()),
            Json::Float(value) if !value.is_finite() => out.push('0'),
            Json::Float(value) if *value != 0.0 && !(1e-5..1e16).contains(&value.abs()) => {
                out.push_str(&format!("{value:e}"))
            }
            Json::Float(value) => out.push_str(&format!("{value:?}")),
            Json::Text(text) => quote(text, out),
            Json::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Map(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    quote(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
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

/// Reads a JSON text; none when it is not one.
pub(crate) fn parse(text: &str) -> Option<Json> {
    let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
    let value = reader.value(0)?;
    reader.space();
    (reader.at == reader.bytes.len()).then_some(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        let found = self.bytes.get(self.at) == Some(&byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > 256 {
            return None;
        }
        self.space();
        match *self.bytes.get(self.at)? {
            b'n' => self.word("null", Json::Null),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'"' => self.text().map(Json::Text),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Some(Json::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Json::List(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'{' => {
                self.at += 1;
                let mut members = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Map(members));
                }
                loop {
                    self.space();
                    let key = self.text()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    members.push((key, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Json::Map(members));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            _ => self.number(),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Option<Json> {
        let found = self.bytes[self.at..].starts_with(word.as_bytes());
        found.then(|| {
            self.at += word.len();
            value
        })
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(|b| b"+-.eE0123456789".contains(b)) {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
        if text.bytes().all(|b| b == b'-' || b.is_ascii_digit())
            && let Ok(value) = text.parse()
        {
            return Some(Json::Int(value));
        }
        text.parse().ok().filter(|v: &f64| v.is_finite()).map(Json::Float)
    }

    fn text(&mut self) -> Option<String> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while self.bytes.get(self.at).is_some_and(|&b| b != b'"' && b != b'\\') {
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(&self.bytes[start..self.at]).ok()?);
            match *self.bytes.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(out);
                }
                _ => {
                    self.at += 1;
                    let escaped = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match escaped {
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let unit = self.hex()?;
                            let code = if (0xd800..0xdc00).contains(&unit) && self.bytes[self.at..].starts_with(b"\\u")
                            {
                                self.at += 2;
                                let low = self.hex()?;
                                0x10000 + ((unit - 0xd800) << 10) + (low.wrapping_sub(0xdc00) & 0x3ff)
                            } else {
                                unit
                            };
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        other => out.push(other as char),
                    }
                }
            }
        }
    }

    fn hex(&mut self) -> Option<u32> {
        let digits = std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
        self.at += 4;
        u32::from_str_radix(digits, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_what_json_encode_writes() {
        for text in [
            r#"{"name":"a\"b\\c\n","count":3,"ratio":2.5,"tags":["x",null,true],"big":1e300,"empty":{}}"#,
            r#"[[],[1,-2],"\u0001é😀"]"#,
        ] {
            let value = parse(text).unwrap();
            assert_eq!(parse(&value.text()).unwrap(), value, "{text}");
        }
        assert_eq!(parse("[1,"), None);
        assert_eq!(parse(r#""😀""#), Some(Json::Text("😀".into())));
    }
}
