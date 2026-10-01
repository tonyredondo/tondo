//! Independent, bounded CBOR wire oracle for the Rust kernel test lane.
//!
//! The recursive grammar is capped at eight levels and 128 value nodes. It
//! deliberately uses a different traversal from the production worklists and
//! imports no production codec. Float narrowing uses arithmetic over exact
//! powers of two rather than the kernel's bit-field conversion algorithm.

use std::fmt;

pub const MAX_CBOR_FUZZ_INPUT_BYTES: usize = 4096;
pub const MAX_CBOR_FUZZ_STEPS: usize = 512;
pub const MAX_REFERENCE_NODES: usize = 128;
pub const MAX_REFERENCE_DEPTH: usize = 8;
pub const MAX_REFERENCE_SCALAR_BYTES: usize = 96;

/// Float variants contain wire bits, including NaN payload and zero sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceValue {
    Null,
    Undefined,
    Bool(bool),
    Simple(u8),
    UInt(u64),
    Negative(u64),
    Float16(u16),
    Float32(u32),
    Float64(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Self>),
    Map(Vec<(Self, Self)>),
    Tag(u64, Box<Self>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
    Truncated,
    Malformed,
    InvalidUtf8,
    InvalidSimple,
    TrailingData,
    KeyCollision,
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

fn error(kind: ReferenceErrorKind, offset: usize) -> ReferenceError {
    ReferenceError { kind, offset }
}

#[derive(Default)]
struct Budget {
    nodes: usize,
}

impl Budget {
    fn enter(&mut self, depth: usize, offset: usize) -> Result<(), ReferenceError> {
        if depth >= MAX_REFERENCE_DEPTH || self.nodes == MAX_REFERENCE_NODES {
            return Err(error(ReferenceErrorKind::Limit, offset));
        }
        self.nodes += 1;
        Ok(())
    }
}

struct Grammar<'a> {
    input: &'a [u8],
    offset: usize,
    budget: Budget,
}

impl Grammar<'_> {
    fn byte(&mut self) -> Result<u8, ReferenceError> {
        let byte = self
            .input
            .get(self.offset)
            .copied()
            .ok_or_else(|| error(ReferenceErrorKind::Truncated, self.offset))?;
        self.offset += 1;
        Ok(byte)
    }

    fn unsigned(&mut self, bytes: usize) -> Result<u64, ReferenceError> {
        let mut result = 0;
        for _ in 0..bytes {
            result = result * 256 + u64::from(self.byte()?);
        }
        Ok(result)
    }

    fn argument(&mut self, additional: u8) -> Result<Option<u64>, ReferenceError> {
        match additional {
            0..=23 => Ok(Some(u64::from(additional))),
            24..=27 => self.unsigned(1 << (additional - 24)).map(Some),
            31 => Ok(None),
            _ => Err(error(ReferenceErrorKind::Malformed, self.offset - 1)),
        }
    }

    fn payload(&mut self, length: u64) -> Result<Vec<u8>, ReferenceError> {
        if length > MAX_REFERENCE_SCALAR_BYTES as u64 {
            return Err(error(ReferenceErrorKind::Limit, self.offset));
        }
        let end = self.offset + length as usize;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| error(ReferenceErrorKind::Truncated, self.offset))?;
        self.offset = end;
        Ok(bytes.to_vec())
    }

    fn string(&mut self, major: u8, length: Option<u64>) -> Result<Vec<u8>, ReferenceError> {
        if let Some(length) = length {
            let bytes = self.payload(length)?;
            if major == 3 && std::str::from_utf8(&bytes).is_err() {
                return Err(error(ReferenceErrorKind::InvalidUtf8, self.offset));
            }
            return Ok(bytes);
        }
        let mut bytes = Vec::new();
        let mut chunks = 0;
        loop {
            let head = self.byte()?;
            if head == 0xff {
                return Ok(bytes);
            }
            if head / 32 != major || head % 32 == 31 {
                return Err(error(ReferenceErrorKind::Malformed, self.offset - 1));
            }
            chunks += 1;
            if chunks > MAX_REFERENCE_NODES {
                return Err(error(ReferenceErrorKind::Limit, self.offset));
            }
            let length = self
                .argument(head % 32)?
                .ok_or_else(|| error(ReferenceErrorKind::Malformed, self.offset))?;
            let chunk = self.payload(length)?;
            if major == 3 && std::str::from_utf8(&chunk).is_err() {
                return Err(error(ReferenceErrorKind::InvalidUtf8, self.offset));
            }
            if bytes.len() + chunk.len() > MAX_REFERENCE_SCALAR_BYTES {
                return Err(error(ReferenceErrorKind::Limit, self.offset));
            }
            bytes.extend(chunk);
        }
    }

    fn item(&mut self, depth: usize) -> Result<ReferenceValue, ReferenceError> {
        self.budget.enter(depth, self.offset)?;
        let head = self.byte()?;
        let major = head / 32;
        let additional = head % 32;
        if major == 7 {
            return match additional {
                0..=19 => Ok(ReferenceValue::Simple(additional)),
                20 => Ok(ReferenceValue::Bool(false)),
                21 => Ok(ReferenceValue::Bool(true)),
                22 => Ok(ReferenceValue::Null),
                23 => Ok(ReferenceValue::Undefined),
                24 => {
                    let simple = self.byte()?;
                    if simple < 32 {
                        Err(error(ReferenceErrorKind::InvalidSimple, self.offset - 2))
                    } else {
                        Ok(ReferenceValue::Simple(simple))
                    }
                }
                25 => self.unsigned(2).map(|n| ReferenceValue::Float16(n as u16)),
                26 => self.unsigned(4).map(|n| ReferenceValue::Float32(n as u32)),
                27 => self.unsigned(8).map(ReferenceValue::Float64),
                _ => Err(error(ReferenceErrorKind::Malformed, self.offset - 1)),
            };
        }
        let argument = self.argument(additional)?;
        match major {
            0 | 1 | 6 => {
                let value = argument
                    .ok_or_else(|| error(ReferenceErrorKind::Malformed, self.offset - 1))?;
                match major {
                    0 => Ok(ReferenceValue::UInt(value)),
                    1 => Ok(ReferenceValue::Negative(value)),
                    _ => Ok(ReferenceValue::Tag(value, Box::new(self.item(depth + 1)?))),
                }
            }
            2 => self.string(major, argument).map(ReferenceValue::Bytes),
            3 => {
                let bytes = self.string(major, argument)?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| error(ReferenceErrorKind::InvalidUtf8, self.offset))?;
                Ok(ReferenceValue::Text(text))
            }
            4 | 5 => {
                let width = if major == 5 { 2 } else { 1 };
                if argument
                    .is_some_and(|n| n > ((MAX_REFERENCE_NODES - self.budget.nodes) / width) as u64)
                {
                    return Err(error(ReferenceErrorKind::Limit, self.offset));
                }
                let mut items = Vec::new();
                let mut pairs = Vec::new();
                let mut count = 0_u64;
                loop {
                    if argument == Some(count) {
                        break;
                    }
                    if argument.is_none() && self.input.get(self.offset) == Some(&0xff) {
                        self.offset += 1;
                        break;
                    }
                    let value = self.item(depth + 1)?;
                    if major == 4 {
                        items.push(value);
                    } else {
                        pairs.push((value, self.item(depth + 1)?));
                    }
                    count += 1;
                }
                if major == 4 {
                    Ok(ReferenceValue::Array(items))
                } else {
                    Ok(ReferenceValue::Map(pairs))
                }
            }
            _ => Err(error(ReferenceErrorKind::Malformed, self.offset)),
        }
    }
}

/// Decode one bounded data item, preserving pairs, tags and float wire bits.
pub fn parse_reference(input: &[u8]) -> Result<ReferenceValue, ReferenceError> {
    if input.len() > MAX_CBOR_FUZZ_INPUT_BYTES {
        return Err(error(ReferenceErrorKind::Limit, 0));
    }
    let mut grammar = Grammar {
        input,
        offset: 0,
        budget: Budget::default(),
    };
    let value = grammar.item(0)?;
    if grammar.offset != input.len() {
        return Err(error(ReferenceErrorKind::TrailingData, grammar.offset));
    }
    Ok(value)
}

fn initial(major: u8, argument: u64) -> Vec<u8> {
    if argument < 24 {
        return vec![major * 32 + argument as u8];
    }
    let size: usize = [1, 2, 4, 8]
        .into_iter()
        .find(|&size| size == 8 || argument < (1_u64 << (size * 8)))
        .unwrap();
    let mut bytes = vec![major * 32 + 24 + size.trailing_zeros() as u8];
    bytes.extend_from_slice(&argument.to_be_bytes()[8 - size..]);
    bytes
}

/// Interpret binary16 with exact arithmetic, independent of production code.
pub fn half_value(bits: u16) -> f64 {
    let exponent = (bits / 1024) % 32;
    let fraction = bits % 1024;
    let magnitude = match exponent {
        0 => f64::from(fraction) * 2_f64.powi(-24),
        31 if fraction == 0 => f64::INFINITY,
        31 => f64::NAN,
        _ => f64::from(1024 + fraction) * 2_f64.powi(i32::from(exponent) - 25),
    };
    if bits >= 0x8000 {
        -magnitude
    } else {
        magnitude
    }
}

fn exact_half(value: f64) -> Option<u16> {
    let sign = if value.is_sign_negative() { 0x8000 } else { 0 };
    let magnitude = value.abs();
    if magnitude.is_infinite() {
        return Some(sign | 0x7c00);
    }
    if magnitude == 0.0 {
        return Some(sign);
    }
    if magnitude < 2_f64.powi(-14) {
        let units = magnitude * 2_f64.powi(24);
        return (units.fract() == 0.0).then_some(sign | units as u16);
    }
    for exponent in -14..=15 {
        let units = magnitude * 2_f64.powi(10 - exponent);
        if (1024.0..2048.0).contains(&units) && units.fract() == 0.0 {
            return Some(sign | (((exponent + 15) as u16) * 1024) | (units as u16 - 1024));
        }
    }
    None
}

fn float_bytes(value: &ReferenceValue, deterministic: bool) -> Vec<u8> {
    let (head, bytes, numeric) = match value {
        ReferenceValue::Float16(bits) => (0xf9, bits.to_be_bytes().to_vec(), half_value(*bits)),
        ReferenceValue::Float32(bits) => (
            0xfa,
            bits.to_be_bytes().to_vec(),
            f64::from(f32::from_bits(*bits)),
        ),
        ReferenceValue::Float64(bits) => (0xfb, bits.to_be_bytes().to_vec(), f64::from_bits(*bits)),
        _ => unreachable!("only float variants enter float_bytes"),
    };
    if deterministic {
        if numeric.is_nan() {
            return vec![0xf9, 0x7e, 0x00];
        }
        if let Some(half) = exact_half(numeric) {
            let mut bytes = vec![0xf9];
            bytes.extend_from_slice(&half.to_be_bytes());
            return bytes;
        }
        let single = numeric as f32;
        if f64::from(single).to_bits() == numeric.to_bits() {
            let mut bytes = vec![0xfa];
            bytes.extend_from_slice(&single.to_bits().to_be_bytes());
            return bytes;
        }
    }
    let mut result = vec![head];
    result.extend(bytes);
    result
}

fn render_value(
    value: &ReferenceValue,
    deterministic: bool,
    depth: usize,
    budget: &mut Budget,
) -> Result<Vec<u8>, ReferenceError> {
    budget.enter(depth, 0)?;
    let out = match value {
        ReferenceValue::Null => vec![0xf6],
        ReferenceValue::Undefined => vec![0xf7],
        ReferenceValue::Bool(false) => vec![0xf4],
        ReferenceValue::Bool(true) => vec![0xf5],
        ReferenceValue::Simple(n) if *n < 20 => vec![0xe0 + n],
        ReferenceValue::Simple(n) if *n >= 32 => vec![0xf8, *n],
        ReferenceValue::Simple(_) => return Err(error(ReferenceErrorKind::InvalidSimple, 0)),
        ReferenceValue::UInt(n) => initial(0, *n),
        ReferenceValue::Negative(n) => initial(1, *n),
        ReferenceValue::Float16(_) | ReferenceValue::Float32(_) | ReferenceValue::Float64(_) => {
            float_bytes(value, deterministic)
        }
        ReferenceValue::Bytes(bytes) => {
            if bytes.len() > MAX_REFERENCE_SCALAR_BYTES {
                return Err(error(ReferenceErrorKind::Limit, 0));
            }
            let mut out = initial(2, bytes.len() as u64);
            out.extend(bytes);
            out
        }
        ReferenceValue::Text(text) => {
            if text.len() > MAX_REFERENCE_SCALAR_BYTES {
                return Err(error(ReferenceErrorKind::Limit, 0));
            }
            let mut out = initial(3, text.len() as u64);
            out.extend_from_slice(text.as_bytes());
            out
        }
        ReferenceValue::Array(items) => {
            let mut out = initial(4, items.len() as u64);
            for item in items {
                out.extend(render_value(item, deterministic, depth + 1, budget)?);
            }
            out
        }
        ReferenceValue::Map(entries) => {
            let mut pairs = Vec::new();
            for (key, value) in entries {
                pairs.push((
                    render_value(key, deterministic, depth + 1, budget)?,
                    render_value(value, deterministic, depth + 1, budget)?,
                ));
            }
            if deterministic {
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                if pairs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err(error(ReferenceErrorKind::KeyCollision, 0));
                }
            }
            let mut out = initial(5, pairs.len() as u64);
            for (key, value) in pairs {
                out.extend(key);
                out.extend(value);
            }
            out
        }
        ReferenceValue::Tag(number, value) => {
            let mut out = initial(6, *number);
            out.extend(render_value(value, deterministic, depth + 1, budget)?);
            out
        }
    };
    if out.len() > MAX_CBOR_FUZZ_INPUT_BYTES {
        return Err(error(ReferenceErrorKind::Limit, 0));
    }
    Ok(out)
}

/// Encode definite lengths and preserve float widths, map order and duplicates.
pub fn render_ordinary(value: &ReferenceValue) -> Result<Vec<u8>, ReferenceError> {
    render_value(value, false, 0, &mut Budget::default())
}

/// Encode core deterministic bytes; colliding encoded keys are rejected.
pub fn render_deterministic(value: &ReferenceValue) -> Result<Vec<u8>, ReferenceError> {
    render_value(value, true, 0, &mut Budget::default())
}

/// Generate representative bounded values from bytes without ambient state.
pub fn value_from_seed(seed: &[u8]) -> ReferenceValue {
    let seed = &seed[..seed.len().min(MAX_REFERENCE_SCALAR_BYTES)];
    let mut number = 0xcbf29ce484222325_u64;
    for byte in seed {
        number = (number ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    let selector = seed.first().copied().unwrap_or(0);
    let scalar = match selector % 13 {
        0 => ReferenceValue::Null,
        1 => ReferenceValue::Undefined,
        2 => ReferenceValue::Bool(selector & 1 != 0),
        3 => ReferenceValue::Simple(if selector < 20 {
            selector
        } else {
            selector.max(32)
        }),
        4 => ReferenceValue::UInt(number),
        5 => ReferenceValue::Negative(number),
        6 => ReferenceValue::Float16(number as u16),
        7 => ReferenceValue::Float32(number as u32),
        8 => ReferenceValue::Float64(number),
        9 => ReferenceValue::Bytes(seed.to_vec()),
        10 => ReferenceValue::Text(format!("CBOR-ü-水-𐅑-{number:016x}")),
        11 => ReferenceValue::Float64((f64::from(selector) / 2.0).to_bits()),
        _ => ReferenceValue::UInt(u64::from(selector)),
    };
    match seed.get(1).copied().unwrap_or(0) % 5 {
        0 => scalar,
        1 => ReferenceValue::Tag(number, Box::new(scalar)),
        2 => ReferenceValue::Array(vec![
            scalar,
            ReferenceValue::Bool(true),
            ReferenceValue::Array(vec![ReferenceValue::Undefined]),
        ]),
        3 => ReferenceValue::Map(vec![
            (ReferenceValue::Text("z".into()), scalar),
            (
                ReferenceValue::UInt(number),
                ReferenceValue::Tag(24, Box::new(ReferenceValue::Bytes(seed.to_vec()))),
            ),
        ]),
        _ => ReferenceValue::Map(vec![
            (ReferenceValue::Array(vec![ReferenceValue::UInt(1)]), scalar),
            (
                ReferenceValue::Bytes(vec![0, 255]),
                ReferenceValue::Map(vec![(
                    ReferenceValue::Text("nested".into()),
                    ReferenceValue::Negative(number),
                )]),
            ),
        ]),
    }
}

/// Select one seed window in a bounded deterministic replay.
pub fn seed_at_step(input: &[u8], step: usize) -> &[u8] {
    let bounded = &input[..input.len().min(MAX_CBOR_FUZZ_INPUT_BYTES)];
    if bounded.is_empty() {
        return bounded;
    }
    let start = step % bounded.len();
    &bounded[start..(start + MAX_REFERENCE_SCALAR_BYTES).min(bounded.len())]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborFuzzSummary {
    pub steps: usize,
    pub valid_cases: usize,
    pub invalid_cases: usize,
    pub bytes_checked: usize,
}

/// Replay bounded generated values, including rejection of trailing data.
pub fn run_cbor_fuzz_case(input: &[u8]) -> Result<CborFuzzSummary, String> {
    let steps = input.len().clamp(1, MAX_CBOR_FUZZ_STEPS);
    let mut summary = CborFuzzSummary {
        steps,
        valid_cases: 0,
        invalid_cases: 0,
        bytes_checked: 0,
    };
    for step in 0..steps {
        let value = value_from_seed(seed_at_step(input, step));
        let ordinary = render_ordinary(&value).map_err(|e| e.to_string())?;
        if parse_reference(&ordinary).map_err(|e| e.to_string())? != value {
            return Err(format!("ordinary reference replay diverged at step {step}"));
        }
        let deterministic = render_deterministic(&value).map_err(|e| e.to_string())?;
        let decoded = parse_reference(&deterministic).map_err(|e| e.to_string())?;
        if render_deterministic(&decoded).map_err(|e| e.to_string())? != deterministic {
            return Err(format!(
                "deterministic reference replay diverged at step {step}"
            ));
        }
        summary.bytes_checked += ordinary.len() + deterministic.len();
        summary.valid_cases += 1;
        let mut trailing = ordinary;
        trailing.push(0);
        if parse_reference(&trailing).map_err(|e| e.kind) != Err(ReferenceErrorKind::TrailingData) {
            return Err(format!(
                "reference trailing-data rejection diverged at step {step}"
            ));
        }
        summary.invalid_cases += 1;
    }
    Ok(summary)
}
