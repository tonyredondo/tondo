#![allow(dead_code)]

use std::collections::BTreeSet;

use serde_json::{Map, Value as KernelValue};

use crate::CodecError;

#[path = "json_api.rs"]
mod json_api;
pub use json_api::*;

/// Parse one strict JSON document and reject trailing bytes and duplicates.
pub(crate) fn kernel_parse(input: &[u8]) -> Result<KernelValue, CodecError> {
    let text = std::str::from_utf8(input).map_err(|_| CodecError::InvalidUtf8)?;
    scan_duplicates(text.as_bytes())?;
    let value: KernelValue = serde_json::from_str(text).map_err(|_| CodecError::InvalidSyntax)?;
    Ok(value)
}

/// Encode JSON with compact separators, retaining insertion order supplied by
/// the caller's `Value` map. `serde_json::Value` is only the dynamic API; typed
/// callers in the compiler write directly to this kernel.
pub(crate) fn kernel_encode(value: &KernelValue) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(value).map_err(|_| CodecError::InvalidSyntax)
}

/// Encode the RFC 8785-shaped deterministic subset used by Tondo. Objects are
/// sorted by their UTF-8 property names and non-finite numbers are rejected by
/// `serde_json` before this function is called.
pub(crate) fn kernel_encode_canonical(value: &KernelValue) -> Result<Vec<u8>, CodecError> {
    let canonical = canonical_value(value)?;
    kernel_encode(&canonical)
}

pub(crate) fn kernel_validate(input: &[u8]) -> Result<(), CodecError> {
    kernel_parse(input).map(|_| ())
}

fn canonical_value(value: &KernelValue) -> Result<KernelValue, CodecError> {
    Ok(match value {
        KernelValue::Null
        | KernelValue::Bool(_)
        | KernelValue::Number(_)
        | KernelValue::String(_) => value.clone(),
        KernelValue::Array(values) => KernelValue::Array(
            values
                .iter()
                .map(canonical_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        KernelValue::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_value(&object[key])?);
            }
            KernelValue::Object(sorted)
        }
    })
}

const MAX_DEPTH: usize = 256;

/// Scan JSON structure before deserializing so duplicate object names cannot
/// be lost by serde_json's map representation. The second pass remains the
/// syntax/number oracle and therefore keeps the implementation compact.
fn scan_duplicates(input: &[u8]) -> Result<(), CodecError> {
    let mut cursor = 0;
    scan_value(input, &mut cursor, 0)?;
    skip_whitespace(input, &mut cursor);
    (cursor == input.len())
        .then_some(())
        .ok_or(CodecError::InvalidSyntax)
}

fn scan_value(input: &[u8], cursor: &mut usize, depth: usize) -> Result<(), CodecError> {
    if depth > MAX_DEPTH {
        return Err(CodecError::LimitExceeded);
    }
    skip_whitespace(input, cursor);
    match input.get(*cursor).copied() {
        Some(b'"') => {
            skip_string(input, cursor)?;
            Ok(())
        }
        Some(b'{') => scan_object(input, cursor, depth),
        Some(b'[') => scan_array(input, cursor, depth),
        Some(b't') => scan_literal(input, cursor, b"true"),
        Some(b'f') => scan_literal(input, cursor, b"false"),
        Some(b'n') => scan_literal(input, cursor, b"null"),
        Some(b'-' | b'0'..=b'9') => scan_number(input, cursor),
        _ => Err(CodecError::InvalidSyntax),
    }
}

fn scan_object(input: &[u8], cursor: &mut usize, depth: usize) -> Result<(), CodecError> {
    *cursor += 1;
    skip_whitespace(input, cursor);
    let mut names = BTreeSet::new();
    if input.get(*cursor) == Some(&b'}') {
        *cursor += 1;
        return Ok(());
    }
    loop {
        skip_whitespace(input, cursor);
        let start = *cursor;
        skip_string(input, cursor)?;
        let end = *cursor;
        let name: String =
            serde_json::from_slice(&input[start..end]).map_err(|_| CodecError::InvalidSyntax)?;
        if !names.insert(name) {
            return Err(CodecError::DuplicateKey);
        }
        skip_whitespace(input, cursor);
        if input.get(*cursor) != Some(&b':') {
            return Err(CodecError::InvalidSyntax);
        }
        *cursor += 1;
        scan_value(input, cursor, depth + 1)?;
        skip_whitespace(input, cursor);
        match input.get(*cursor).copied() {
            Some(b',') => *cursor += 1,
            Some(b'}') => {
                *cursor += 1;
                return Ok(());
            }
            _ => return Err(CodecError::InvalidSyntax),
        }
    }
}

fn scan_array(input: &[u8], cursor: &mut usize, depth: usize) -> Result<(), CodecError> {
    *cursor += 1;
    skip_whitespace(input, cursor);
    if input.get(*cursor) == Some(&b']') {
        *cursor += 1;
        return Ok(());
    }
    loop {
        scan_value(input, cursor, depth + 1)?;
        skip_whitespace(input, cursor);
        match input.get(*cursor).copied() {
            Some(b',') => *cursor += 1,
            Some(b']') => {
                *cursor += 1;
                return Ok(());
            }
            _ => return Err(CodecError::InvalidSyntax),
        }
    }
}

fn skip_string(input: &[u8], cursor: &mut usize) -> Result<(), CodecError> {
    if input.get(*cursor) != Some(&b'"') {
        return Err(CodecError::InvalidSyntax);
    }
    *cursor += 1;
    let mut escaped = false;
    while let Some(byte) = input.get(*cursor).copied() {
        *cursor += 1;
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => return Ok(()),
            0..=0x1f => return Err(CodecError::InvalidSyntax),
            _ => {}
        }
    }
    Err(CodecError::UnexpectedEof)
}

fn scan_literal(input: &[u8], cursor: &mut usize, literal: &[u8]) -> Result<(), CodecError> {
    let end = cursor
        .checked_add(literal.len())
        .ok_or(CodecError::InvalidLength)?;
    if input.get(*cursor..end) == Some(literal) {
        *cursor = end;
        Ok(())
    } else {
        Err(CodecError::InvalidSyntax)
    }
}

fn scan_number(input: &[u8], cursor: &mut usize) -> Result<(), CodecError> {
    let start = *cursor;
    while let Some(byte) = input.get(*cursor).copied() {
        if matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | b',' | b']' | b'}') {
            break;
        }
        *cursor += 1;
    }
    (*cursor > start)
        .then_some(())
        .ok_or(CodecError::InvalidSyntax)
}

fn skip_whitespace(input: &[u8], cursor: &mut usize) {
    while input
        .get(*cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        *cursor += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_round_trip_and_trailing_data() {
        let input = br#"{"name":"tondo","items":[1,true,null]}"#;
        let value = kernel_parse(input).unwrap();
        assert_eq!(kernel_encode(&value).unwrap(), input);
        assert_eq!(kernel_parse(br#"{} {}"#), Err(CodecError::InvalidSyntax));
    }

    #[test]
    fn invalid_utf8_and_duplicate_keys_are_rejected() {
        assert_eq!(kernel_parse(&[0xff]), Err(CodecError::InvalidUtf8));
        assert_eq!(
            kernel_parse(br#"{"a":1,"a":2}"#),
            Err(CodecError::DuplicateKey)
        );
    }

    #[test]
    fn canonical_objects_sort_utf8_property_bytes() {
        let value = json!({"z": 1, "a": {"b": 2, "a": 3}});
        assert_eq!(
            kernel_encode_canonical(&value).unwrap(),
            br#"{"a":{"a":3,"b":2},"z":1}"#
        );
    }

    #[test]
    fn arrays_and_scalars_are_preserved() {
        let value = json!([null, false, "é", -0.0]);
        assert_eq!(
            kernel_parse(&kernel_encode(&value).unwrap()).unwrap(),
            value
        );
        kernel_validate(b"true").unwrap();
    }

    #[test]
    fn rfc_corpus_covers_whitespace_escapes_numbers_and_limits() {
        for input in [
            br#"null"#.as_slice(),
            br#" true "#.as_slice(),
            br#"[-1,0,1,1.25,1e3]"#.as_slice(),
            br#"{"emoji":"\uD83D\uDE80","slash":"\\"}"#.as_slice(),
            br#"[{"a":[]},{"b":{}}]"#.as_slice(),
        ] {
            kernel_validate(input).unwrap();
        }
        for input in [
            br#"+1"#.as_slice(),
            br#"01"#.as_slice(),
            br#"{"a":1,}"#.as_slice(),
            br#"[true false]"#.as_slice(),
            br#""#.as_slice(),
            br#"{"a":1} trailing"#.as_slice(),
        ] {
            assert!(
                kernel_parse(input).is_err(),
                "accepted invalid JSON: {input:?}"
            );
        }
        let mut deeply_nested = vec![b'['; MAX_DEPTH + 1];
        deeply_nested.extend(std::iter::repeat_n(b']', MAX_DEPTH + 1));
        assert!(matches!(
            kernel_parse(&deeply_nested),
            Err(CodecError::LimitExceeded | CodecError::InvalidSyntax)
        ));
    }

    #[test]
    fn canonical_order_is_utf8_byte_order_and_is_idempotent() {
        let value = json!({"é": 1, "a": {"z": 2, "b": 3}});
        let first = kernel_encode_canonical(&value).unwrap();
        assert_eq!(first, r#"{"a":{"b":3,"z":2},"é":1}"#.as_bytes());
        assert_eq!(
            kernel_encode_canonical(&kernel_parse(&first).unwrap()).unwrap(),
            first
        );
    }
}
