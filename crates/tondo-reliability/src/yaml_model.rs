//! Independent bounded reference model for `std.yaml`.
//!
//! The model intentionally implements only scalar Core resolution and the
//! canonical value renderer.  It does not call the production YAML parser or
//! serializer.  The model is small enough to use as a deterministic fuzz
//! oracle while the production tests exercise the complete hosted surface.

use std::fmt;

/// Maximum input consumed by one YAML fuzz replay.
pub const MAX_YAML_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum deterministic actions accepted by one YAML fuzz replay.
pub const MAX_YAML_FUZZ_STEPS: usize = 512;
/// Maximum value nodes rendered by the independent model.
pub const MAX_REFERENCE_NODES: usize = 128;
/// Maximum scalar payload sampled by one fuzz action.
pub const MAX_REFERENCE_SCALAR_BYTES: usize = 96;

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<ReferenceValue>),
    Object(Vec<(String, ReferenceValue)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
    InvalidUtf8,
    InvalidScalar,
    InvalidEscape,
    InvalidTag,
    InvalidAnchor,
    UndefinedAlias,
    AliasCycle,
    DuplicateKey,
    NonStringKey,
    MergeKeyForbidden,
    NumberOutOfRange,
    NonFiniteNumber,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceError {
    pub kind: ReferenceErrorKind,
    pub offset: usize,
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} at byte {}", self.kind, self.offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlFuzzSummary {
    pub steps: usize,
    pub valid_cases: usize,
    pub invalid_cases: usize,
    pub bytes_checked: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

/// Resolve one plain scalar using the YAML 1.2 Core rules independently.
pub fn parse_core_scalar(input: &[u8]) -> Result<ReferenceValue, ReferenceError> {
    let text = std::str::from_utf8(input).map_err(|_| ReferenceError {
        kind: ReferenceErrorKind::InvalidUtf8,
        offset: 0,
    })?;
    let text = text.trim();
    if text.is_empty() || text == "~" || text.eq_ignore_ascii_case("null") {
        return Ok(ReferenceValue::Null);
    }
    if text.eq_ignore_ascii_case("true") {
        return Ok(ReferenceValue::Bool(true));
    }
    if text.eq_ignore_ascii_case("false") {
        return Ok(ReferenceValue::Bool(false));
    }
    if matches!(
        text.to_ascii_lowercase().as_str(),
        ".nan" | ".inf" | "+.inf" | "-.inf"
    ) {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::NonFiniteNumber,
            offset: 0,
        });
    }
    if text == "<<:" || text.starts_with("<<:") {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::MergeKeyForbidden,
            offset: 0,
        });
    }
    if text.starts_with('!') {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::InvalidTag,
            offset: 0,
        });
    }
    if text.starts_with('*') {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::UndefinedAlias,
            offset: 0,
        });
    }
    if text.starts_with('&') {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::InvalidAnchor,
            offset: 0,
        });
    }
    if let Some(integer) = parse_integer(text) {
        return integer;
    }
    if looks_like_float(text) {
        let value = text.parse::<f64>().map_err(|_| ReferenceError {
            kind: ReferenceErrorKind::InvalidScalar,
            offset: 0,
        })?;
        if !value.is_finite() {
            return Err(ReferenceError {
                kind: ReferenceErrorKind::NonFiniteNumber,
                offset: 0,
            });
        }
        return Ok(ReferenceValue::Float(value));
    }
    Ok(ReferenceValue::Text(text.to_owned()))
}

/// Render a bounded value using the independent canonical YAML layout.
pub fn render_canonical(value: &ReferenceValue) -> Result<Vec<u8>, ReferenceError> {
    let mut nodes = 0;
    let mut output = render_value(value, 0, true, &mut nodes)?;
    output.push('\n');
    Ok(output.into_bytes())
}

/// Generate one small value from a seed without consulting production code.
pub fn value_from_seed(seed: &[u8]) -> ReferenceValue {
    value_from_seed_at(seed, 0)
}

/// Replay a bounded byte sequence against the reference model.
pub fn run_yaml_fuzz_case(input: &[u8]) -> Result<YamlFuzzSummary, String> {
    let bounded = &input[..input.len().min(MAX_YAML_FUZZ_INPUT_BYTES)];
    let steps = bounded.len().clamp(1, MAX_YAML_FUZZ_STEPS);
    let input_len = bounded.len().max(1);
    let mut valid_cases = 0;
    let mut invalid_cases = 0;
    let mut bytes_checked = 0usize;
    let mut max_depth = 0usize;
    let mut max_nodes = 0usize;

    for step in 0..steps {
        let selector = bounded.get(step % input_len).copied().unwrap_or_default();
        let available = bounded.len().saturating_sub(1);
        let payload_len = available.min(MAX_REFERENCE_SCALAR_BYTES);
        let start = if available == 0 {
            0
        } else {
            usize::from(selector) % (available + 1)
        };
        let end = (start + payload_len).min(bounded.len());
        let payload = if start < end {
            &bounded[start..end]
        } else {
            &[]
        };
        let value = value_from_seed(payload);
        let rendered = render_canonical(&value)
            .map_err(|error| format!("reference render failed at step {step}: {error}"))?;
        let repeated = render_canonical(&value)
            .map_err(|error| format!("reference rerender failed at step {step}: {error}"))?;
        if rendered != repeated {
            return Err(format!("reference YAML rendering diverged at step {step}"));
        }
        if is_scalar(&value) && !matches!(value, ReferenceValue::Bytes(_)) {
            let reparsed = parse_core_scalar(&rendered).map_err(|error| {
                format!("reference scalar parse failed at step {step}: {error}")
            })?;
            if reparsed != value {
                return Err(format!(
                    "reference scalar roundtrip diverged at step {step}"
                ));
            }
        }
        valid_cases += 1;
        bytes_checked = bytes_checked.saturating_add(payload.len());
        max_depth = max_depth.max(value_depth(&value));
        max_nodes = max_nodes.max(value_nodes(&value));

        let malformed = malformed_case(selector);
        if parse_core_scalar(malformed).is_ok() {
            return Err(format!("reference accepted malformed input at step {step}"));
        }
        invalid_cases += 1;
    }

    Ok(YamlFuzzSummary {
        steps,
        valid_cases,
        invalid_cases,
        bytes_checked,
        max_depth,
        max_nodes,
    })
}

fn parse_integer(text: &str) -> Option<Result<ReferenceValue, ReferenceError>> {
    let negative = text.starts_with('-');
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (radix, body) = if let Some(value) = digits.strip_prefix("0b") {
        (2, value)
    } else if let Some(value) = digits.strip_prefix("0o") {
        (8, value)
    } else if let Some(value) = digits.strip_prefix("0x") {
        (16, value)
    } else if digits.bytes().all(|byte| byte.is_ascii_digit()) {
        (10, digits)
    } else {
        return None;
    };
    if body.is_empty() || body.contains('_') {
        return Some(Ok(ReferenceValue::Text(text.to_owned())));
    }
    let magnitude = match u128::from_str_radix(body, radix) {
        Ok(value) => value,
        Err(_) => {
            return Some(Err(ReferenceError {
                kind: ReferenceErrorKind::NumberOutOfRange,
                offset: 0,
            }));
        }
    };
    if negative {
        if magnitude > (i64::MAX as u128) + 1 {
            return Some(Err(ReferenceError {
                kind: ReferenceErrorKind::NumberOutOfRange,
                offset: 0,
            }));
        }
        let value = if magnitude == (i64::MAX as u128) + 1 {
            i64::MIN
        } else {
            -(magnitude as i64)
        };
        Some(Ok(ReferenceValue::Int(value)))
    } else if magnitude <= i64::MAX as u128 {
        Some(Ok(ReferenceValue::Int(magnitude as i64)))
    } else if magnitude <= u64::MAX as u128 {
        Some(Ok(ReferenceValue::UInt(magnitude as u64)))
    } else {
        Some(Err(ReferenceError {
            kind: ReferenceErrorKind::NumberOutOfRange,
            offset: 0,
        }))
    }
}

fn looks_like_float(text: &str) -> bool {
    (text.contains('.') || text.contains('e') || text.contains('E')) && text.parse::<f64>().is_ok()
}

fn render_value(
    value: &ReferenceValue,
    indent: usize,
    canonical: bool,
    nodes: &mut usize,
) -> Result<String, ReferenceError> {
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_REFERENCE_NODES {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::Limit,
            offset: 0,
        });
    }
    match value {
        ReferenceValue::Null
        | ReferenceValue::Bool(_)
        | ReferenceValue::Int(_)
        | ReferenceValue::UInt(_)
        | ReferenceValue::Float(_)
        | ReferenceValue::Text(_)
        | ReferenceValue::Bytes(_) => render_scalar(value),
        ReferenceValue::Array(values) => {
            if values.is_empty() {
                return Ok("[]".into());
            }
            let mut output = String::new();
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                output.push_str(&" ".repeat(indent));
                output.push_str("- ");
                if is_inline(value) {
                    output.push_str(&render_value(value, indent + 2, canonical, nodes)?);
                } else {
                    output.push('\n');
                    output.push_str(&render_value(value, indent + 2, canonical, nodes)?);
                }
            }
            Ok(output)
        }
        ReferenceValue::Object(members) => {
            let mut seen = Vec::with_capacity(members.len());
            for (key, _) in members {
                if seen.iter().any(|candidate: &&String| *candidate == key) {
                    return Err(ReferenceError {
                        kind: ReferenceErrorKind::DuplicateKey,
                        offset: 0,
                    });
                }
                seen.push(key);
            }
            if members.is_empty() {
                return Ok("{}".into());
            }
            let mut order = (0..members.len()).collect::<Vec<_>>();
            if canonical {
                order.sort_by(|left, right| {
                    members[*left]
                        .0
                        .as_bytes()
                        .cmp(members[*right].0.as_bytes())
                });
            }
            let mut output = String::new();
            for (position, index) in order.into_iter().enumerate() {
                if position > 0 {
                    output.push('\n');
                }
                let (key, value) = &members[index];
                output.push_str(&" ".repeat(indent));
                output.push_str(&render_text(key));
                output.push(':');
                if is_inline(value) {
                    output.push(' ');
                    output.push_str(&render_value(value, indent + 2, canonical, nodes)?);
                } else {
                    output.push('\n');
                    output.push_str(&render_value(value, indent + 2, canonical, nodes)?);
                }
            }
            Ok(output)
        }
    }
}

fn render_scalar(value: &ReferenceValue) -> Result<String, ReferenceError> {
    match value {
        ReferenceValue::Null => Ok("null".into()),
        ReferenceValue::Bool(value) => Ok(value.to_string()),
        ReferenceValue::Int(value) => Ok(value.to_string()),
        ReferenceValue::UInt(value) => Ok(value.to_string()),
        ReferenceValue::Float(value) if value.is_finite() => Ok(value.to_string()),
        ReferenceValue::Float(_) => Err(ReferenceError {
            kind: ReferenceErrorKind::NonFiniteNumber,
            offset: 0,
        }),
        ReferenceValue::Text(value) => Ok(render_text(value)),
        ReferenceValue::Bytes(value) => Ok(format!("!!binary {}", base64_encode(value))),
        ReferenceValue::Array(_) | ReferenceValue::Object(_) => unreachable!("container scalar"),
    }
}

fn render_text(value: &str) -> String {
    if plain_scalar_safe(value) {
        return value.to_owned();
    }
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn plain_scalar_safe(value: &str) -> bool {
    if value.is_empty() || value.trim() != value || value.contains(['\n', '\r', '\t']) {
        return false;
    }
    if value.starts_with([
        '-', '?', ':', '!', '&', '*', '#', '{', '}', '[', ']', ',', '|', '>', '@', '`', '%',
    ]) {
        return false;
    }
    if value.contains(": ") || value.contains(" #") || matches!(value, "-" | "?" | ":") {
        return false;
    }
    matches!(
        parse_core_scalar(value.as_bytes()),
        Ok(ReferenceValue::Text(ref parsed)) if parsed == value
    )
}

fn is_inline(value: &ReferenceValue) -> bool {
    matches!(
        value,
        ReferenceValue::Null
            | ReferenceValue::Bool(_)
            | ReferenceValue::Int(_)
            | ReferenceValue::UInt(_)
            | ReferenceValue::Float(_)
            | ReferenceValue::Text(_)
            | ReferenceValue::Bytes(_)
    ) || matches!(value, ReferenceValue::Array(values) if values.is_empty())
        || matches!(value, ReferenceValue::Object(values) if values.is_empty())
}

fn value_from_seed_at(seed: &[u8], depth: usize) -> ReferenceValue {
    let selector = seed.first().copied().unwrap_or_default();
    match selector % 8 {
        0 => ReferenceValue::Null,
        1 => ReferenceValue::Bool(selector & 1 == 1),
        2 => ReferenceValue::Int(-((i64::from(selector) + 1) * 3)),
        3 => ReferenceValue::UInt(u64::MAX - u64::from(selector)),
        4 => ReferenceValue::Float((f64::from(selector) + 0.5) / 10.0),
        5 => ReferenceValue::Text(format!("item-{selector}")),
        6 => ReferenceValue::Bytes(seed[..seed.len().min(4)].to_vec()),
        _ if depth >= 2 => ReferenceValue::Text("nested".into()),
        _ if selector & 8 == 0 => ReferenceValue::Array(vec![
            ReferenceValue::Null,
            value_from_seed_at(seed.get(1..).unwrap_or_default(), depth + 1),
        ]),
        _ => ReferenceValue::Object(vec![
            (
                "value".into(),
                value_from_seed_at(seed.get(1..).unwrap_or_default(), depth + 1),
            ),
            ("kind".into(), ReferenceValue::Text("yaml".into())),
        ]),
    }
}

fn is_scalar(value: &ReferenceValue) -> bool {
    !matches!(value, ReferenceValue::Array(_) | ReferenceValue::Object(_))
}

fn value_depth(value: &ReferenceValue) -> usize {
    match value {
        ReferenceValue::Array(values) => 1 + values.iter().map(value_depth).max().unwrap_or(0),
        ReferenceValue::Object(members) => {
            1 + members
                .iter()
                .map(|(_, value)| value_depth(value))
                .max()
                .unwrap_or(0)
        }
        _ => 1,
    }
}

fn value_nodes(value: &ReferenceValue) -> usize {
    match value {
        ReferenceValue::Array(values) => 1 + values.iter().map(value_nodes).sum::<usize>(),
        ReferenceValue::Object(members) => {
            1 + members
                .iter()
                .map(|(_, value)| value_nodes(value))
                .sum::<usize>()
        }
        _ => 1,
    }
}

fn malformed_case(selector: u8) -> &'static [u8] {
    match selector % 8 {
        0 => b"!custom value",
        1 => b"*missing",
        2 => b"&1 value",
        3 => b".inf",
        4 => b"18446744073709551616",
        5 => b"<<: value",
        6 => b"\xff",
        _ => b"!!tag value",
    }
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}
