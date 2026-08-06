#![allow(dead_code)]

use crate::CodecError;

#[path = "messagepack_api.rs"]
mod messagepack_api;

pub use messagepack_api::*;

const MAX_DEPTH: usize = 256;
const MAX_ELEMENTS: usize = 1_048_576;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
enum KernelValue {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float32(u32),
    Float64(u64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<KernelValue>),
    Map(Vec<(KernelValue, KernelValue)>),
    Ext(i8, Vec<u8>),
}

fn kernel_encode(value: &KernelValue) -> Vec<u8> {
    let mut output = Vec::new();
    write_value(value, &mut output);
    output
}

/// Encode using Tondo's deterministic map ordering. The ordering key is the
/// deterministic wire encoding of each key, which also works for arbitrary
/// MessagePack values instead of assuming string keys.
fn kernel_encode_deterministic(value: &KernelValue) -> Result<Vec<u8>, CodecError> {
    let canonical = deterministic_value(value)?;
    Ok(kernel_encode(&canonical))
}

fn deterministic_value(value: &KernelValue) -> Result<KernelValue, CodecError> {
    Ok(match value {
        KernelValue::Array(values) => KernelValue::Array(
            values
                .iter()
                .map(deterministic_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        KernelValue::Map(entries) => {
            let mut ordered = entries
                .iter()
                .map(|(key, value)| {
                    let key = deterministic_value(key)?;
                    let value = deterministic_value(value)?;
                    let encoded_key = kernel_encode(&key);
                    Ok((encoded_key, key, value))
                })
                .collect::<Result<Vec<_>, CodecError>>()?;
            ordered.sort_by(|left, right| left.0.cmp(&right.0));
            if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(CodecError::DuplicateKey);
            }
            KernelValue::Map(
                ordered
                    .into_iter()
                    .map(|(_, key, value)| (key, value))
                    .collect(),
            )
        }
        _ => value.clone(),
    })
}

fn kernel_decode(input: &[u8]) -> Result<KernelValue, CodecError> {
    let (value, offset) = read_value(input, 0, 0)?;
    if offset != input.len() {
        return Err(CodecError::TrailingData);
    }
    Ok(value)
}

fn write_value(value: &KernelValue, output: &mut Vec<u8>) {
    match value {
        KernelValue::Nil => output.push(0xc0),
        KernelValue::Bool(false) => output.push(0xc2),
        KernelValue::Bool(true) => output.push(0xc3),
        KernelValue::Int(value) if (0..=127).contains(value) => output.push(*value as u8),
        KernelValue::Int(value) if (-32..=-1).contains(value) => output.push(*value as i8 as u8),
        KernelValue::Int(value) if i8::try_from(*value).is_ok() => {
            output.extend([0xd0, *value as i8 as u8])
        }
        KernelValue::Int(value) if i16::try_from(*value).is_ok() => {
            output.push(0xd1);
            output.extend_from_slice(&(*value as i16).to_be_bytes());
        }
        KernelValue::Int(value) if i32::try_from(*value).is_ok() => {
            output.push(0xd2);
            output.extend_from_slice(&(*value as i32).to_be_bytes());
        }
        KernelValue::Int(value) => {
            output.push(0xd3);
            output.extend_from_slice(&value.to_be_bytes());
        }
        KernelValue::UInt(value) if *value <= 127 => output.push(*value as u8),
        KernelValue::UInt(value) if u8::try_from(*value).is_ok() => {
            output.extend([0xcc, *value as u8])
        }
        KernelValue::UInt(value) if u16::try_from(*value).is_ok() => {
            output.push(0xcd);
            output.extend_from_slice(&(*value as u16).to_be_bytes());
        }
        KernelValue::UInt(value) if u32::try_from(*value).is_ok() => {
            output.push(0xce);
            output.extend_from_slice(&(*value as u32).to_be_bytes());
        }
        KernelValue::UInt(value) => {
            output.push(0xcf);
            output.extend_from_slice(&value.to_be_bytes());
        }
        KernelValue::Float32(bits) => {
            output.push(0xca);
            output.extend_from_slice(&bits.to_be_bytes());
        }
        KernelValue::Float64(bits) => {
            output.push(0xcb);
            output.extend_from_slice(&bits.to_be_bytes());
        }
        KernelValue::String(value) => {
            let bytes = value.as_bytes();
            write_len(0xa0, 0xd9, 0xda, 0xdb, bytes.len(), output);
            output.extend_from_slice(bytes);
        }
        KernelValue::Binary(value) => {
            write_len(0, 0xc4, 0xc5, 0xc6, value.len(), output);
            output.extend_from_slice(value);
        }
        KernelValue::Array(values) => {
            write_collection_len(0x90, 0xdc, 0xdd, values.len(), output);
            values.iter().for_each(|value| write_value(value, output));
        }
        KernelValue::Map(entries) => {
            write_collection_len(0x80, 0xde, 0xdf, entries.len(), output);
            for (key, value) in entries {
                write_value(key, output);
                write_value(value, output);
            }
        }
        KernelValue::Ext(kind, payload) => {
            let (tag, width) = match payload.len() {
                1 => (0xd4, 1),
                2 => (0xd5, 2),
                4 => (0xd6, 4),
                8 => (0xd7, 8),
                16 => (0xd8, 16),
                len if len <= u8::MAX as usize => (0xc7, len),
                len if len <= u16::MAX as usize => (0xc8, len),
                _ => (0xc9, payload.len()),
            };
            output.push(tag);
            match tag {
                0xc7 => output.push(width as u8),
                0xc8 => output.extend_from_slice(&(width as u16).to_be_bytes()),
                0xc9 => output.extend_from_slice(&(width as u32).to_be_bytes()),
                _ => {}
            }
            output.push(*kind as u8);
            output.extend_from_slice(payload);
        }
    }
}

fn write_len(fix: u8, short: u8, medium: u8, long: u8, len: usize, output: &mut Vec<u8>) {
    if fix != 0 && len < 32 {
        output.push(fix | len as u8);
    } else if len <= u8::MAX as usize {
        output.extend([short, len as u8]);
    } else if len <= u16::MAX as usize {
        output.push(medium);
        output.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        output.push(long);
        output.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

fn write_collection_len(fix: u8, short: u8, long: u8, len: usize, output: &mut Vec<u8>) {
    if len < 16 {
        output.push(fix | len as u8);
    } else if len <= u16::MAX as usize {
        output.push(short);
        output.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        output.push(long);
        output.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

fn read_value(
    input: &[u8],
    offset: usize,
    depth: usize,
) -> Result<(KernelValue, usize), CodecError> {
    if depth > MAX_DEPTH {
        return Err(CodecError::LimitExceeded);
    }
    let tag = *input.get(offset).ok_or(CodecError::UnexpectedEof)?;
    let mut cursor = offset + 1;
    match tag {
        0xc0 => Ok((KernelValue::Nil, cursor)),
        0xc2 => Ok((KernelValue::Bool(false), cursor)),
        0xc3 => Ok((KernelValue::Bool(true), cursor)),
        0x00..=0x7f => Ok((KernelValue::UInt(tag as u64), cursor)),
        0xe0..=0xff => Ok((KernelValue::Int((tag as i8) as i64), cursor)),
        0xcc => Ok((
            KernelValue::UInt(read_u8(input, &mut cursor)? as u64),
            cursor,
        )),
        0xcd => Ok((
            KernelValue::UInt(read_u16(input, &mut cursor)? as u64),
            cursor,
        )),
        0xce => Ok((
            KernelValue::UInt(read_u32(input, &mut cursor)? as u64),
            cursor,
        )),
        0xcf => Ok((KernelValue::UInt(read_u64(input, &mut cursor)?), cursor)),
        0xd0 => Ok((
            KernelValue::Int(read_u8(input, &mut cursor)? as i8 as i64),
            cursor,
        )),
        0xd1 => Ok((
            KernelValue::Int(read_u16(input, &mut cursor)? as i16 as i64),
            cursor,
        )),
        0xd2 => Ok((
            KernelValue::Int(read_u32(input, &mut cursor)? as i32 as i64),
            cursor,
        )),
        0xd3 => Ok((
            KernelValue::Int(read_u64(input, &mut cursor)? as i64),
            cursor,
        )),
        0xca => Ok((KernelValue::Float32(read_u32(input, &mut cursor)?), cursor)),
        0xcb => Ok((KernelValue::Float64(read_u64(input, &mut cursor)?), cursor)),
        0xa0..=0xbf => read_string(input, &mut cursor, (tag & 0x1f) as usize),
        0xd9 => {
            let len = read_u8(input, &mut cursor)? as usize;
            read_string(input, &mut cursor, len)
        }
        0xda => {
            let len = read_u16(input, &mut cursor)? as usize;
            read_string(input, &mut cursor, len)
        }
        0xdb => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            read_string(input, &mut cursor, len)
        }
        0xc4 => {
            let len = read_u8(input, &mut cursor)? as usize;
            read_binary(input, &mut cursor, len)
        }
        0xc5 => {
            let len = read_u16(input, &mut cursor)? as usize;
            read_binary(input, &mut cursor, len)
        }
        0xc6 => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            read_binary(input, &mut cursor, len)
        }
        0x90..=0x9f => read_array(input, &mut cursor, (tag & 0x0f) as usize, depth),
        0xdc => {
            let len = read_u16(input, &mut cursor)? as usize;
            read_array(input, &mut cursor, len, depth)
        }
        0xdd => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            read_array(input, &mut cursor, len, depth)
        }
        0x80..=0x8f => read_map(input, &mut cursor, (tag & 0x0f) as usize, depth),
        0xde => {
            let len = read_u16(input, &mut cursor)? as usize;
            read_map(input, &mut cursor, len, depth)
        }
        0xdf => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            read_map(input, &mut cursor, len, depth)
        }
        0xd4..=0xd8 => {
            let len = [1, 2, 4, 8, 16][(tag - 0xd4) as usize];
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((KernelValue::Ext(kind, payload), cursor))
        }
        0xc7 => {
            let len = read_u8(input, &mut cursor)? as usize;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((KernelValue::Ext(kind, payload), cursor))
        }
        0xc8 => {
            let len = read_u16(input, &mut cursor)? as usize;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((KernelValue::Ext(kind, payload), cursor))
        }
        0xc9 => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((KernelValue::Ext(kind, payload), cursor))
        }
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_string(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
) -> Result<(KernelValue, usize), CodecError> {
    let bytes = read_bytes(input, cursor, len)?;
    let value = String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::InvalidUtf8)?;
    Ok((KernelValue::String(value), *cursor))
}

fn read_binary(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
) -> Result<(KernelValue, usize), CodecError> {
    Ok((
        KernelValue::Binary(read_bytes(input, cursor, len)?.to_vec()),
        *cursor,
    ))
}

fn read_array(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
    depth: usize,
) -> Result<(KernelValue, usize), CodecError> {
    if len > MAX_ELEMENTS {
        return Err(CodecError::LimitExceeded);
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        let (value, next) = read_value(input, *cursor, depth + 1)?;
        values.push(value);
        *cursor = next;
    }
    Ok((KernelValue::Array(values), *cursor))
}

fn read_map(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
    depth: usize,
) -> Result<(KernelValue, usize), CodecError> {
    if len > MAX_ELEMENTS {
        return Err(CodecError::LimitExceeded);
    }
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        let (key, next) = read_value(input, *cursor, depth + 1)?;
        *cursor = next;
        let (value, next) = read_value(input, *cursor, depth + 1)?;
        *cursor = next;
        entries.push((key, value));
    }
    Ok((KernelValue::Map(entries), *cursor))
}

fn read_bytes<'a>(input: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8], CodecError> {
    if len > MAX_BYTES {
        return Err(CodecError::LimitExceeded);
    }
    let end = cursor.checked_add(len).ok_or(CodecError::InvalidLength)?;
    let bytes = input.get(*cursor..end).ok_or(CodecError::UnexpectedEof)?;
    *cursor = end;
    Ok(bytes)
}

fn read_u8(input: &[u8], cursor: &mut usize) -> Result<u8, CodecError> {
    Ok(read_bytes(input, cursor, 1)?[0])
}

fn read_u16(input: &[u8], cursor: &mut usize) -> Result<u16, CodecError> {
    Ok(u16::from_be_bytes(
        read_bytes(input, cursor, 2)?.try_into().unwrap(),
    ))
}

fn read_u32(input: &[u8], cursor: &mut usize) -> Result<u32, CodecError> {
    Ok(u32::from_be_bytes(
        read_bytes(input, cursor, 4)?.try_into().unwrap(),
    ))
}

fn read_u64(input: &[u8], cursor: &mut usize) -> Result<u64, CodecError> {
    Ok(u64::from_be_bytes(
        read_bytes(input, cursor, 8)?.try_into().unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_minimal_scalars_and_round_trips() {
        let values = [
            KernelValue::Nil,
            KernelValue::Bool(true),
            KernelValue::Int(-32),
            KernelValue::Int(128),
            KernelValue::UInt(255),
            KernelValue::Float32(1.0f32.to_bits()),
            KernelValue::Float64((-0.0f64).to_bits()),
            KernelValue::String("hello".into()),
            KernelValue::Binary(vec![0, 255]),
            KernelValue::Ext(3, vec![1, 2]),
        ];
        for value in values {
            assert_eq!(kernel_decode(&kernel_encode(&value)).unwrap(), value);
        }
    }

    #[test]
    fn arrays_maps_and_truncation_are_checked() {
        let value = KernelValue::Map(vec![
            (
                KernelValue::String("items".into()),
                KernelValue::Array(vec![KernelValue::UInt(1)]),
            ),
            (KernelValue::Int(-1), KernelValue::Nil),
        ]);
        let encoded = kernel_encode(&value);
        assert_eq!(kernel_decode(&encoded).unwrap(), value);
        assert_eq!(
            kernel_decode(&encoded[..encoded.len() - 1]),
            Err(CodecError::UnexpectedEof)
        );
        assert_eq!(kernel_decode(&[0xc1]), Err(CodecError::InvalidTag));
    }

    #[test]
    fn deterministic_maps_sort_arbitrary_keys_and_reject_collisions() {
        let value = KernelValue::Map(vec![
            (KernelValue::String("z".into()), KernelValue::UInt(1)),
            (KernelValue::String("a".into()), KernelValue::UInt(2)),
        ]);
        let encoded = kernel_encode_deterministic(&value).unwrap();
        let expected = KernelValue::Map(vec![
            (KernelValue::String("a".into()), KernelValue::UInt(2)),
            (KernelValue::String("z".into()), KernelValue::UInt(1)),
        ]);
        assert_eq!(kernel_decode(&encoded).unwrap(), expected);
        let duplicate = KernelValue::Map(vec![
            (KernelValue::UInt(1), KernelValue::Nil),
            (KernelValue::UInt(1), KernelValue::Nil),
        ]);
        assert_eq!(
            kernel_encode_deterministic(&duplicate),
            Err(CodecError::DuplicateKey)
        );
    }

    #[test]
    fn depth_and_collection_limits_fail_before_allocation() {
        let mut nested = vec![0x91; 258];
        nested.extend(std::iter::repeat_n(0xc0, 258));
        assert_eq!(kernel_decode(&nested), Err(CodecError::LimitExceeded));
        assert_eq!(
            kernel_decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]),
            Err(CodecError::LimitExceeded)
        );
    }

    #[test]
    fn specification_vectors_cover_unsigned_signed_string_binary_and_ext() {
        let vectors = [
            (vec![0xc0], KernelValue::Nil),
            (vec![0xc3], KernelValue::Bool(true)),
            (vec![0x2a], KernelValue::UInt(42)),
            (vec![0xd0, 0xd6], KernelValue::Int(-42)),
            (
                vec![0xa3, b'f', b'o', b'o'],
                KernelValue::String("foo".into()),
            ),
            (
                vec![0xc4, 0x02, 0x00, 0xff],
                KernelValue::Binary(vec![0, 255]),
            ),
            (vec![0xd4, 0x01, 0x7f], KernelValue::Ext(1, vec![0x7f])),
        ];
        for (wire, expected) in vectors {
            assert_eq!(kernel_decode(&wire).unwrap(), expected);
            assert_eq!(kernel_encode(&expected), wire);
        }
    }

    #[test]
    fn malformed_corpus_never_publishes_a_partial_value() {
        let valid = kernel_encode(&KernelValue::Array(vec![
            KernelValue::Map(vec![(
                KernelValue::String("x".into()),
                KernelValue::UInt(1),
            )]),
            KernelValue::Binary(vec![1, 2, 3]),
        ]));
        for cut in 0..valid.len() {
            assert!(
                kernel_decode(&valid[..cut]).is_err(),
                "truncated input at {cut}"
            );
        }
        for tag in [0xc1, 0xc7, 0xde, 0xdf] {
            assert!(
                kernel_decode(&[tag]).is_err(),
                "accepted malformed tag {tag:#x}"
            );
        }
    }
}
