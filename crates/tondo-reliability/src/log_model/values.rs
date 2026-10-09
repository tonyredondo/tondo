//! Independent finite logging value, validation and record-format oracle.
use std::collections::BTreeMap;

pub const MAX_NODES: usize = 128;
pub const MAX_SCALAR: usize = 256;
pub const MAX_HEIGHT: usize = 20;
pub const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidTarget,
    InvalidFieldKey,
    DuplicateField,
    InvalidLimit,
    ResourceLimit,
    NonFiniteValue,
    OutsideDomain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    JsonLines,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Object(Fields),
    Redacted,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fields(Vec<(String, Value)>);
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub level: usize,
    pub target: String,
    pub message: String,
    pub fields: Fields,
    /// A caller-supplied known canonical UTC token. This model does not validate
    /// calendar arithmetic, parse timestamps or consult a provider.
    pub timestamp: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub event: usize,
    pub fields: usize,
    pub depth: usize,
    pub key: usize,
    pub string: usize,
    pub queue: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            event: 1 << 20,
            fields: 64,
            depth: 16,
            key: 128,
            string: 64 << 10,
            queue: 1024,
        }
    }
}
impl Limits {
    pub fn validate(self) -> Result<Self, Error> {
        if [
            self.event,
            self.fields,
            self.depth,
            self.key,
            self.string,
            self.queue,
        ]
        .contains(&0)
        {
            Err(Error::InvalidLimit)
        } else {
            Ok(self)
        }
    }
}
fn key_valid(key: &str, limits: Limits) -> Result<(), Error> {
    if key.is_empty() || key.chars().any(char::is_control) {
        return Err(Error::InvalidFieldKey);
    }
    if key.len() > limits.key {
        return Err(Error::ResourceLimit);
    }
    Ok(())
}
/// Reference-domain admission is separate from any Tondo semantic refusal.
fn domain(fields: &Fields) -> Result<(), Error> {
    fn walk(value: &Value, height: usize, nodes: &mut usize) -> Result<(), Error> {
        *nodes += 1;
        if *nodes > MAX_NODES || height > MAX_HEIGHT {
            return Err(Error::OutsideDomain);
        }
        match value {
            Value::Text(text) if text.len() > MAX_SCALAR => Err(Error::OutsideDomain),
            Value::Bytes(bytes) if bytes.len() > MAX_SCALAR => Err(Error::OutsideDomain),
            Value::Array(values) => {
                for value in values {
                    walk(value, height + 1, nodes)?;
                }
                Ok(())
            }
            Value::Object(fields) => {
                for (key, value) in &fields.0 {
                    if key.len() > MAX_SCALAR {
                        return Err(Error::OutsideDomain);
                    }
                    walk(value, height + 1, nodes)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    let mut nodes = 0;
    for (key, value) in &fields.0 {
        if key.len() > MAX_SCALAR {
            return Err(Error::OutsideDomain);
        }
        walk(value, 0, &mut nodes)?;
    }
    Ok(())
}
struct Validation {
    bytes: usize,
    fields: usize,
    limits: Limits,
}
impl Validation {
    fn bytes(&mut self, size: usize) -> Result<(), Error> {
        self.bytes = self.bytes.checked_add(size).ok_or(Error::ResourceLimit)?;
        if self.bytes > self.limits.event {
            Err(Error::ResourceLimit)
        } else {
            Ok(())
        }
    }
    fn entry(&mut self, key: &str, value: &Value, depth: usize) -> Result<(), Error> {
        self.fields += 1;
        key_valid(key, self.limits)?;
        self.bytes(key.len())?;
        self.value(value, depth)
    }
    fn value(&mut self, value: &Value, depth: usize) -> Result<(), Error> {
        self.bytes(8)?;
        match value {
            Value::Float(number) if !number.is_finite() => return Err(Error::NonFiniteValue),
            Value::Text(text) => {
                if text.len() > self.limits.string {
                    return Err(Error::ResourceLimit);
                }
                self.bytes(text.len())?;
            }
            Value::Bytes(bytes) => self.bytes(bytes.len())?,
            Value::Array(_) | Value::Object(_) if depth >= self.limits.depth => {
                return Err(Error::ResourceLimit);
            }
            _ => {}
        }
        if self.fields > self.limits.fields {
            return Err(Error::ResourceLimit);
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    self.value(value, depth + 1)?;
                }
            }
            Value::Object(fields) => {
                for (key, value) in &fields.0 {
                    self.entry(key, value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn validate(fields: &Fields, limits: Limits) -> Result<usize, Error> {
    domain(fields)?;
    let mut state = Validation {
        bytes: 0,
        fields: 0,
        limits,
    };
    for (key, value) in &fields.0 {
        state.entry(key, value, 0)?;
    }
    Ok(state.bytes)
}
impl Fields {
    pub fn entries(&self) -> &[(String, Value)] {
        &self.0
    }
    pub fn put(&mut self, key: &str, value: Value) -> Result<(), Error> {
        let limits = Limits::default();
        key_valid(key, limits)?;
        if self.0.iter().any(|(existing, _)| existing == key) {
            return Err(Error::DuplicateField);
        }
        if self.0.len() >= limits.fields {
            return Err(Error::ResourceLimit);
        }
        let mut candidate = self.clone();
        candidate.0.push((key.to_owned(), value));
        validate(&candidate, limits)?;
        *self = candidate;
        Ok(())
    }
}
impl Event {
    pub fn create(
        level: usize,
        target: &str,
        message: &str,
        fields: Fields,
        timestamp: Option<&str>,
    ) -> Result<Self, Error> {
        if target.is_empty() {
            return Err(Error::InvalidTarget);
        }
        if level > 4
            || target.len() > MAX_SCALAR
            || message.len() > MAX_SCALAR
            || timestamp.is_some_and(|text| text.len() > MAX_SCALAR)
        {
            return Err(Error::OutsideDomain);
        }
        let event = Self {
            level,
            target: target.to_owned(),
            message: message.to_owned(),
            fields,
            timestamp: timestamp.map(str::to_owned),
        };
        event.validate(Limits::default())?;
        Ok(event)
    }
    fn validate(&self, limits: Limits) -> Result<(), Error> {
        if self.target.len() > limits.string || self.message.len() > limits.string {
            return Err(Error::ResourceLimit);
        }
        let bytes = validate(&self.fields, limits)?;
        if bytes
            .checked_add(self.target.len())
            .and_then(|size| size.checked_add(self.message.len()))
            .is_none_or(|size| size > limits.event)
        {
            return Err(Error::ResourceLimit);
        }
        Ok(())
    }
    pub fn enabled(&self, minimum: usize) -> bool {
        self.level >= minimum
    }
}
struct Render {
    result: String,
    limit: usize,
}
impl Render {
    fn append(&mut self, text: &str) -> Result<(), Error> {
        let size = self
            .result
            .len()
            .checked_add(text.len())
            .ok_or(Error::ResourceLimit)?;
        if size > self.limit {
            return Err(Error::ResourceLimit);
        }
        if size > MAX_OUTPUT {
            return Err(Error::OutsideDomain);
        }
        self.result.push_str(text);
        Ok(())
    }
    fn quoted(&mut self, text: &str) -> Result<(), Error> {
        self.append("\"")?;
        for scalar in text.chars() {
            match scalar {
                '\\' => self.append("\\\\")?,
                '"' => self.append("\\\"")?,
                '\n' => self.append("\\n")?,
                '\r' => self.append("\\r")?,
                '\t' => self.append("\\t")?,
                '\u{8}' => self.append("\\b")?,
                '\u{c}' => self.append("\\f")?,
                scalar if scalar.is_control() => {
                    self.append(&format!("\\u{:04x}", scalar as u32))?
                }
                scalar => self.append(scalar.encode_utf8(&mut [0; 4]))?,
            }
        }
        self.append("\"")
    }
    fn object(&mut self, fields: &Fields, format: Format) -> Result<(), Error> {
        self.append("{")?;
        let entries: Vec<_> = if format == Format::JsonLines {
            fields
                .0
                .iter()
                .map(|(key, value)| (key.as_str(), value))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect()
        } else {
            fields
                .0
                .iter()
                .map(|(key, value)| (key.as_str(), value))
                .collect()
        };
        for (index, (key, value)) in entries.into_iter().enumerate() {
            if index > 0 {
                self.append(",")?;
            }
            self.quoted(key)?;
            self.append(":")?;
            self.value(value, format)?;
        }
        self.append("}")
    }
    fn value(&mut self, value: &Value, format: Format) -> Result<(), Error> {
        match value {
            Value::Null => self.append("null"),
            Value::Bool(value) => self.append(if *value { "true" } else { "false" }),
            Value::Int(value) => self.append(&value.to_string()),
            Value::UInt(value) => self.append(&value.to_string()),
            Value::Float(value) => self.append(&value.to_string()),
            Value::Text(text) => self.quoted(text),
            Value::Redacted => self.append(if format == Format::Text {
                "[REDACTED]"
            } else {
                "{\"$redacted\":true}"
            }),
            Value::Bytes(bytes) => {
                self.append(if format == Format::Text {
                    "b64:"
                } else {
                    "{\"$bytes\":\""
                })?;
                self.append(&base64(bytes))?;
                if format == Format::JsonLines {
                    self.append("\"}")?;
                }
                Ok(())
            }
            Value::Array(values) => {
                self.append("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.append(",")?;
                    }
                    self.value(value, format)?;
                }
                self.append("]")
            }
            Value::Object(fields) => self.object(fields, format),
        }
    }
}
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for shift in [18, 12, 6, 0] {
            let symbol = if (shift == 6 && chunk.len() == 1) || (shift == 0 && chunk.len() < 3) {
                b'='
            } else {
                ALPHABET[((word >> shift) & 63) as usize]
            };
            result.push(char::from(symbol));
        }
    }
    result
}
pub fn format_record(event: &Event, format: Format, limits: Limits) -> Result<Vec<u8>, Error> {
    event.validate(limits)?;
    let mut output = Render {
        result: String::new(),
        limit: limits.event,
    };
    let level = ["trace", "debug", "info", "warn", "error"]
        .get(event.level)
        .ok_or(Error::OutsideDomain)?;
    match format {
        Format::Text => {
            output.append(level)?;
            output.append(" ")?;
            output.quoted(&event.target)?;
            output.append(" ")?;
            output.quoted(&event.message)?;
            for (key, value) in event.fields.entries() {
                output.append(" ")?;
                output.quoted(key)?;
                output.append("=")?;
                output.value(value, format)?;
            }
            output.append("\n")?;
        }
        Format::JsonLines => {
            output.append("{\"schema\":\"tondo-log-event-0.1/1\",\"level\":")?;
            output.quoted(level)?;
            output.append(",\"target\":")?;
            output.quoted(&event.target)?;
            output.append(",\"message\":")?;
            output.quoted(&event.message)?;
            output.append(",\"time\":")?;
            match &event.timestamp {
                Some(time) => output.quoted(time)?,
                None => output.append("null")?,
            }
            output.append(",\"fields\":")?;
            output.object(&event.fields, format)?;
            output.append("}\n")?;
        }
    }
    Ok(output.result.into_bytes())
}

/// Generates a deterministic finite event for kernel comparisons and fuzz replay.
pub fn case_from_seed(mut seed: u64) -> Event {
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut nested = Fields::default();
    nested.put("z", Value::Bool(next() & 1 != 0)).unwrap();
    nested.put("a", Value::Int(next() as i64)).unwrap();
    let mut fields = Fields::default();
    fields
        .put(
            "z",
            Value::Array(vec![
                Value::Null,
                Value::Text("line\n世界\"\\\t\u{8}\u{c}\u{80}".into()),
                Value::Float((next() as i64 % 1_000_000) as f64 / 8.),
            ]),
        )
        .unwrap();
    fields
        .put(
            "bytes",
            Value::Bytes(next().to_le_bytes()[..(next() % 9) as usize].to_vec()),
        )
        .unwrap();
    fields.put("secret", Value::Redacted).unwrap();
    fields.put("n", Value::UInt(next())).unwrap();
    fields.put("nested", Value::Object(nested)).unwrap();
    let level = (next() % 5) as usize;
    Event::create(
        level,
        "model",
        if next() & 1 == 0 {
            "message\r\n"
        } else {
            "κ世界"
        },
        fields,
        None,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(fields: Fields) -> Event {
        Event::create(2, "test", "hello\n世界", fields, None).unwrap()
    }
    #[test]
    fn text_keeps_field_order_and_json_sorts_utf8_without_normalization() {
        let mut fields = Fields::default();
        fields.put("z", Value::Int(1)).unwrap();
        fields.put("a", Value::Bool(true)).unwrap();
        let event = sample(fields);
        assert_eq!(
            format_record(&event, Format::Text, Limits::default()).unwrap(),
            "info \"test\" \"hello\\n世界\" \"z\"=1 \"a\"=true\n".as_bytes()
        );
        assert_eq!(format_record(&event, Format::JsonLines, Limits::default()).unwrap(), "{\"schema\":\"tondo-log-event-0.1/1\",\"level\":\"info\",\"target\":\"test\",\"message\":\"hello\\n世界\",\"time\":null,\"fields\":{\"a\":true,\"z\":1}}\n".as_bytes());
    }
    #[test]
    fn duplicate_controls_and_nonfinite_values_never_mutate_fields() {
        let mut fields = Fields::default();
        fields.put("x", Value::Null).unwrap();
        for (key, value, error) in [
            ("x", Value::Int(2), Error::DuplicateField),
            ("", Value::Null, Error::InvalidFieldKey),
            ("\0", Value::Null, Error::InvalidFieldKey),
            ("n", Value::Float(f64::NAN), Error::NonFiniteValue),
        ] {
            let before = fields.clone();
            assert_eq!(fields.put(key, value), Err(error));
            assert_eq!(fields, before);
        }
    }
    #[test]
    fn byte_encoding_and_explicit_redaction_have_separate_tokens() {
        for (bytes, expected) in [
            (vec![], ""),
            (vec![255], "/w=="),
            (vec![255, 0], "/wA="),
            (vec![0, 1, 2], "AAEC"),
        ] {
            assert_eq!(base64(&bytes), expected);
            let mut fields = Fields::default();
            fields.put("bytes", Value::Bytes(bytes)).unwrap();
            fields.put("secret", Value::Redacted).unwrap();
            let record =
                format_record(&sample(fields), Format::JsonLines, Limits::default()).unwrap();
            let record = String::from_utf8(record).unwrap();
            assert!(record.contains(&format!("\"bytes\":{{\"$bytes\":\"{expected}\"}}")));
            assert!(record.contains("\"secret\":{\"$redacted\":true}"));
        }
    }
    #[test]
    fn encoded_output_limit_is_exact_and_refuses_without_a_partial_record() {
        let event = sample(Fields::default());
        for format in [Format::Text, Format::JsonLines] {
            let record = format_record(&event, format, Limits::default()).unwrap();
            let limits = Limits {
                event: record.len(),
                ..Limits::default()
            };
            assert_eq!(format_record(&event, format, limits).unwrap(), record);
            assert_eq!(
                format_record(
                    &event,
                    format,
                    Limits {
                        event: record.len() - 1,
                        ..limits
                    }
                ),
                Err(Error::ResourceLimit)
            );
        }
    }
    #[test]
    fn empty_nodes_charge_bytes_and_container_depth_is_explicit() {
        let mut fields = Fields::default();
        fields
            .put("a", Value::Array(vec![Value::Array(vec![])]))
            .unwrap();
        let event = sample(fields);
        assert_eq!(
            format_record(
                &event,
                Format::Text,
                Limits {
                    depth: 1,
                    ..Limits::default()
                }
            ),
            Err(Error::ResourceLimit)
        );
        assert!(
            format_record(
                &event,
                Format::Text,
                Limits {
                    depth: 2,
                    ..Limits::default()
                }
            )
            .is_ok()
        );
        assert_eq!(
            format_record(
                &event,
                Format::Text,
                Limits {
                    event: 16,
                    ..Limits::default()
                }
            ),
            Err(Error::ResourceLimit)
        );
    }
    #[test]
    fn all_levels_filter_and_all_zero_limit_dimensions_are_closed() {
        for level in 0..5 {
            for minimum in 0..5 {
                assert_eq!(
                    Event::create(level, "x", "", Fields::default(), None)
                        .unwrap()
                        .enabled(minimum),
                    level >= minimum
                );
            }
        }
        for index in 0..6 {
            let mut limits = Limits::default();
            match index {
                0 => limits.event = 0,
                1 => limits.fields = 0,
                2 => limits.depth = 0,
                3 => limits.key = 0,
                4 => limits.string = 0,
                _ => limits.queue = 0,
            }
            assert_eq!(limits.validate(), Err(Error::InvalidLimit));
        }
    }
    #[test]
    fn outside_reference_domain_does_not_claim_a_production_limit() {
        let mut fields = Fields::default();
        fields.put("kept", Value::Bool(true)).unwrap();
        let nested = (0..MAX_HEIGHT + 2).fold(Value::Null, |inner, _| Value::Array(vec![inner]));
        for value in [
            Value::Text("x".repeat(MAX_SCALAR + 1)),
            Value::Bytes(vec![0; MAX_SCALAR + 1]),
            Value::Array(vec![Value::Null; MAX_NODES]),
            nested,
        ] {
            let before = fields.clone();
            assert_eq!(fields.put("outside", value), Err(Error::OutsideDomain));
            assert_eq!(fields, before);
        }
        assert_eq!(
            Event::create(5, "test", "", Fields::default(), None),
            Err(Error::OutsideDomain)
        );
        assert_eq!(
            Event::create(
                2,
                "test",
                &"x".repeat(MAX_SCALAR + 1),
                Fields::default(),
                None
            ),
            Err(Error::OutsideDomain)
        );
        assert_eq!(
            Event::create(2, "", "", Fields::default(), None),
            Err(Error::InvalidTarget)
        );
    }
}
