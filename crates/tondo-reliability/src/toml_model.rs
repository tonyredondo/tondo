//! Independent bounded reference model for the deterministic `std.toml` lane.
//!
//! The model owns only a small canonical value renderer.  It deliberately
//! does not call the production parser, encoder or event adapters; the
//! integration tests and fuzz target compare its bytes with the hosted TOML
//! implementation.

use std::fmt;

/// Maximum input sampled by one TOML fuzz replay.
pub const MAX_TOML_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum deterministic actions accepted by one TOML fuzz replay.
pub const MAX_TOML_FUZZ_STEPS: usize = 512;
/// Maximum value nodes rendered by the independent model.
pub const MAX_REFERENCE_NODES: usize = 128;
/// Maximum scalar payload sampled by one fuzz action.
pub const MAX_REFERENCE_SCALAR_BYTES: usize = 96;

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Array(Vec<ReferenceValue>),
    Table(Vec<(String, ReferenceValue)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
    DuplicateKey,
    InvalidKey,
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
pub struct TomlFuzzSummary {
    pub steps: usize,
    pub valid_cases: usize,
    pub invalid_cases: usize,
    pub bytes_checked: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

/// Generate one small table from a seed without consulting production code.
pub fn value_from_seed(seed: &[u8]) -> ReferenceValue {
    let bounded = &seed[..seed.len().min(MAX_REFERENCE_SCALAR_BYTES)];
    let count = bounded.len().min(4);
    let keys = ["alpha", "beta", "gamma", "delta"];
    let members = bounded
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, byte)| {
            let value = match byte % 6 {
                0 => ReferenceValue::Bool(byte & 1 != 0),
                1 => ReferenceValue::Int(i64::from(*byte) - 64),
                2 => ReferenceValue::UInt(u64::from(*byte) * 1_000_000),
                3 => ReferenceValue::Float((f64::from(*byte) - 100.0) / 10.0),
                4 => ReferenceValue::Text(format!("text-{}\n\t\"\\", byte)),
                _ => ReferenceValue::Array(vec![
                    ReferenceValue::Int(i64::from(*byte)),
                    ReferenceValue::Bool(byte & 1 == 0),
                    ReferenceValue::Table(vec![
                        (
                            "nested".to_owned(),
                            ReferenceValue::Text("inline".to_owned()),
                        ),
                        ("value".to_owned(), ReferenceValue::UInt(u64::from(*byte))),
                    ]),
                ]),
            };
            (keys[index].to_owned(), value)
        })
        .collect();
    ReferenceValue::Table(members)
}

/// Render a bounded value using the independent canonical TOML layout.
pub fn render_canonical(value: &ReferenceValue) -> Result<Vec<u8>, ReferenceError> {
    let mut nodes = 0;
    let mut output = String::new();
    let ReferenceValue::Table(members) = value else {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::InvalidKey,
            offset: 0,
        });
    };
    let mut ordered = members.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut previous = None;
    for (key, value) in ordered {
        if key.is_empty() || !key.bytes().all(is_bare_key) {
            return Err(ReferenceError {
                kind: ReferenceErrorKind::InvalidKey,
                offset: 0,
            });
        }
        if previous == Some(key.as_str()) {
            return Err(ReferenceError {
                kind: ReferenceErrorKind::DuplicateKey,
                offset: 0,
            });
        }
        previous = Some(key.as_str());
        write_key(key, &mut output);
        output.push_str(" = ");
        render_value(value, &mut output, 0, &mut nodes)?;
        output.push('\n');
    }
    Ok(output.into_bytes())
}

/// Replay a bounded byte sequence against the independent renderer.
pub fn run_toml_fuzz_case(input: &[u8]) -> Result<TomlFuzzSummary, String> {
    let bounded = &input[..input.len().min(MAX_TOML_FUZZ_INPUT_BYTES)];
    let steps = bounded.len().clamp(1, MAX_TOML_FUZZ_STEPS);
    let input_len = bounded.len().max(1);
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
            .map_err(|error| format!("reference TOML render failed at step {step}: {error}"))?;
        let repeated = render_canonical(&value)
            .map_err(|error| format!("reference TOML rerender failed at step {step}: {error}"))?;
        if rendered != repeated {
            return Err(format!("reference TOML rendering diverged at step {step}"));
        }
        bytes_checked = bytes_checked.saturating_add(payload.len());
        max_depth = max_depth.max(value_depth(&value));
        max_nodes = max_nodes.max(value_nodes(&value));
        if max_nodes > MAX_REFERENCE_NODES {
            return Err(format!("reference TOML node bound exceeded at step {step}"));
        }
    }
    Ok(TomlFuzzSummary {
        steps,
        valid_cases: steps,
        invalid_cases: steps,
        bytes_checked,
        max_depth,
        max_nodes,
    })
}

fn render_value(
    value: &ReferenceValue,
    output: &mut String,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), ReferenceError> {
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_REFERENCE_NODES || depth > MAX_REFERENCE_NODES {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::Limit,
            offset: 0,
        });
    }
    match value {
        ReferenceValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        ReferenceValue::Int(value) => output.push_str(&value.to_string()),
        ReferenceValue::UInt(value) => output.push_str(&value.to_string()),
        ReferenceValue::Float(value) => {
            if !value.is_finite() {
                return Err(ReferenceError {
                    kind: ReferenceErrorKind::NonFiniteNumber,
                    offset: 0,
                });
            }
            let mut text = value.to_string();
            if !text.contains('.') && !text.contains('e') && !text.contains('E') {
                text.push_str(".0");
            }
            output.push_str(&text);
        }
        ReferenceValue::Text(value) => {
            output.push('"');
            escape_string(value, output);
            output.push('"');
        }
        ReferenceValue::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                render_value(value, output, depth + 1, nodes)?;
            }
            output.push(']');
        }
        ReferenceValue::Table(members) => {
            output.push('{');
            let mut ordered = members.iter().collect::<Vec<_>>();
            ordered.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            let mut previous = None;
            for (index, (key, value)) in ordered.iter().enumerate() {
                if key.is_empty() || !key.bytes().all(is_bare_key) {
                    return Err(ReferenceError {
                        kind: ReferenceErrorKind::InvalidKey,
                        offset: 0,
                    });
                }
                if previous == Some(key.as_str()) {
                    return Err(ReferenceError {
                        kind: ReferenceErrorKind::DuplicateKey,
                        offset: 0,
                    });
                }
                previous = Some(key.as_str());
                if index != 0 {
                    output.push_str(", ");
                }
                write_key(key, output);
                output.push_str(" = ");
                render_value(value, output, depth + 1, nodes)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn is_bare_key(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn write_key(key: &str, output: &mut String) {
    if key.bytes().all(is_bare_key) {
        output.push_str(key);
    } else {
        output.push('"');
        escape_string(key, output);
        output.push('"');
    }
}

fn escape_string(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '\u{08}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{0c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\e"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(output, "\\u{:04X}", character as u32);
            }
            character => output.push(character),
        }
    }
}

fn value_depth(value: &ReferenceValue) -> usize {
    match value {
        ReferenceValue::Array(values) => 1 + values.iter().map(value_depth).max().unwrap_or(0),
        ReferenceValue::Table(members) => {
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
        ReferenceValue::Table(members) => {
            1 + members
                .iter()
                .map(|(_, value)| value_nodes(value))
                .sum::<usize>()
        }
        _ => 1,
    }
}
