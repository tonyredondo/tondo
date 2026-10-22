//! The part of TOML that `tondo.toml` and `tondo.lock` use: tables, arrays
//! of tables, and keys with strings, integers, inline tables of strings or
//! arrays on one line.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Table(Vec<(String, Value)>),
    Array(Vec<Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// A `[name]` or `[[name]]` table and its keys, with the line of each key.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub array: bool,
    pub line: usize,
    pub entries: Vec<(String, Value, usize)>,
}

/// Parses a document; the keys before any header form a table named `""`.
pub fn parse(text: &str) -> Result<Vec<Table>, String> {
    let mut tables = vec![Table { name: String::new(), array: false, line: 0, entries: Vec::new() }];
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let at = |message: String| format!("line {number}: {message}");
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            tables.push(Table { name: header.trim().to_string(), array: true, line: number, entries: Vec::new() });
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = header.trim().to_string();
            if tables.iter().any(|t| !t.array && t.name == name) {
                return Err(at(format!("`[{name}]` appears twice")));
            }
            tables.push(Table { name, array: false, line: number, entries: Vec::new() });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(at("expected `key = value`".into()));
        };
        let key = bare_key(key.trim()).ok_or_else(|| at(format!("`{}` is not a valid key", key.trim())))?;
        let (value, rest) = parse_value(value.trim()).map_err(at)?;
        if !rest.trim().is_empty() {
            return Err(at(format!("unexpected `{}` after the value", rest.trim())));
        }
        let table = tables.last_mut().unwrap();
        if table.entries.iter().any(|(k, _, _)| *k == key) {
            return Err(at(format!("`{key}` appears twice")));
        }
        table.entries.push((key, value, number));
    }
    Ok(tables)
}

/// Removes a `#` comment that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let (mut quoted, mut escaped) = (false, false);
    for (index, c) in line.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        } else if c == '#' {
            return &line[..index];
        }
    }
    line
}

fn bare_key(key: &str) -> Option<String> {
    let valid = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| key.to_string())
}

/// A value at the start of `text`, and what follows it.
fn parse_value(text: &str) -> Result<(Value, &str), String> {
    if let Some(rest) = text.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = rest.char_indices();
        while let Some((index, c)) = chars.next() {
            match c {
                '"' => return Ok((Value::String(value), &rest[index + 1..])),
                '\\' => match chars.next() {
                    Some((_, '"')) => value.push('"'),
                    Some((_, '\\')) => value.push('\\'),
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, 't')) => value.push('\t'),
                    _ => return Err("unknown escape in a string".into()),
                },
                c => value.push(c),
            }
        }
        return Err("unterminated string".into());
    }
    if let Some(mut rest) = text.strip_prefix('[') {
        let mut items = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                return Ok((Value::Array(items), after));
            }
            let (value, after) = parse_value(rest)?;
            items.push(value);
            rest = after.trim_start();
            if let Some(after) = rest.strip_prefix(',') {
                rest = after;
            } else if !rest.starts_with(']') {
                return Err("expected `,` or `]` in an array".into());
            }
        }
    }
    if let Some(mut rest) = text.strip_prefix('{') {
        let mut entries = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix('}') {
                return Ok((Value::Table(entries), after));
            }
            let (key, after) = rest.split_once('=').ok_or("expected `key = value` in an inline table")?;
            let key = bare_key(key.trim()).ok_or_else(|| format!("`{}` is not a valid key", key.trim()))?;
            let (value, after) = parse_value(after.trim_start())?;
            if entries.iter().any(|(k, _)| *k == key) {
                return Err(format!("`{key}` appears twice"));
            }
            entries.push((key, value));
            rest = after.trim_start();
            if let Some(after) = rest.strip_prefix(',') {
                rest = after;
            } else if !rest.starts_with('}') {
                return Err("expected `,` or `}` in an inline table".into());
            }
        }
    }
    let end = text.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '_')).unwrap_or(text.len());
    match text[..end].replace('_', "").parse() {
        Ok(number) if end > 0 => Ok((Value::Integer(number), &text[end..])),
        _ => Err(format!("unsupported value `{text}`: use a string, an integer, an inline table or an array")),
    }
}

/// Writes a string as a TOML basic string.
pub fn quote(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tables_and_inline_tables() {
        let text = "# comment\n[package]\nname = \"app\" # trailing\n\n[dependencies]\nutils = { path = \"../utils\" }\nhttp = { git = \"https://x/y#z\", tag = \"v1\" }\n\n[[package]]\nversion = 1\n";
        let tables = parse(text).unwrap();
        assert_eq!(tables.len(), 4);
        assert_eq!(tables[1].entries[0].1, Value::String("app".into()));
        let Value::Table(http) = &tables[2].entries[1].1 else { panic!() };
        assert_eq!(http[0], ("git".into(), Value::String("https://x/y#z".into())));
        assert!(tables[3].array);
        assert_eq!(tables[3].entries[0].1, Value::Integer(1));
    }

    #[test]
    fn reports_errors_with_lines() {
        assert!(parse("[a]\nkey\n").unwrap_err().starts_with("line 2:"));
        assert!(parse("key = true\n").is_err());
        assert!(parse("key = [\"a\"\n").is_err());
        let tables = parse("[native]\nlibraries = [\"m\", \"z\",]\n").unwrap();
        assert_eq!(tables[1].entries[0].1, Value::Array(vec![Value::String("m".into()), Value::String("z".into())]));
    }
}
