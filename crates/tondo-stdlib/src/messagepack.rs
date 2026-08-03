use crate::CodecError;

const MAX_DEPTH: usize = 256;
const MAX_ELEMENTS: usize = 1_048_576;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float32(u32),
    Float64(u64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Ext(i8, Vec<u8>),
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut output = Vec::new();
    write_value(value, &mut output);
    output
}

/// Encode using Tondo's deterministic map ordering. The ordering key is the
/// deterministic wire encoding of each key, which also works for arbitrary
/// MessagePack values instead of assuming string keys.
pub fn encode_deterministic(value: &Value) -> Result<Vec<u8>, CodecError> {
    let canonical = deterministic_value(value)?;
    Ok(encode(&canonical))
}

fn deterministic_value(value: &Value) -> Result<Value, CodecError> {
    Ok(match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(deterministic_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Map(entries) => {
            let mut ordered = entries
                .iter()
                .map(|(key, value)| {
                    let key = deterministic_value(key)?;
                    let value = deterministic_value(value)?;
                    let encoded_key = encode(&key);
                    Ok((encoded_key, key, value))
                })
                .collect::<Result<Vec<_>, CodecError>>()?;
            ordered.sort_by(|left, right| left.0.cmp(&right.0));
            if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(CodecError::DuplicateKey);
            }
            Value::Map(
                ordered
                    .into_iter()
                    .map(|(_, key, value)| (key, value))
                    .collect(),
            )
        }
        _ => value.clone(),
    })
}

pub fn decode(input: &[u8]) -> Result<Value, CodecError> {
    let (value, offset) = read_value(input, 0, 0)?;
    if offset != input.len() {
        return Err(CodecError::TrailingData);
    }
    Ok(value)
}

fn write_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Nil => output.push(0xc0),
        Value::Bool(false) => output.push(0xc2),
        Value::Bool(true) => output.push(0xc3),
        Value::Int(value) if (0..=127).contains(value) => output.push(*value as u8),
        Value::Int(value) if (-32..=-1).contains(value) => output.push(*value as i8 as u8),
        Value::Int(value) if i8::try_from(*value).is_ok() => {
            output.extend([0xd0, *value as i8 as u8])
        }
        Value::Int(value) if i16::try_from(*value).is_ok() => {
            output.push(0xd1);
            output.extend_from_slice(&(*value as i16).to_be_bytes());
        }
        Value::Int(value) if i32::try_from(*value).is_ok() => {
            output.push(0xd2);
            output.extend_from_slice(&(*value as i32).to_be_bytes());
        }
        Value::Int(value) => {
            output.push(0xd3);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Value::UInt(value) if *value <= 127 => output.push(*value as u8),
        Value::UInt(value) if u8::try_from(*value).is_ok() => output.extend([0xcc, *value as u8]),
        Value::UInt(value) if u16::try_from(*value).is_ok() => {
            output.push(0xcd);
            output.extend_from_slice(&(*value as u16).to_be_bytes());
        }
        Value::UInt(value) if u32::try_from(*value).is_ok() => {
            output.push(0xce);
            output.extend_from_slice(&(*value as u32).to_be_bytes());
        }
        Value::UInt(value) => {
            output.push(0xcf);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Value::Float32(bits) => {
            output.push(0xca);
            output.extend_from_slice(&bits.to_be_bytes());
        }
        Value::Float64(bits) => {
            output.push(0xcb);
            output.extend_from_slice(&bits.to_be_bytes());
        }
        Value::String(value) => {
            let bytes = value.as_bytes();
            write_len(0xa0, 0xd9, 0xda, 0xdb, bytes.len(), output);
            output.extend_from_slice(bytes);
        }
        Value::Binary(value) => {
            write_len(0, 0xc4, 0xc5, 0xc6, value.len(), output);
            output.extend_from_slice(value);
        }
        Value::Array(values) => {
            write_collection_len(0x90, 0xdc, 0xdd, values.len(), output);
            values.iter().for_each(|value| write_value(value, output));
        }
        Value::Map(entries) => {
            write_collection_len(0x80, 0xde, 0xdf, entries.len(), output);
            for (key, value) in entries {
                write_value(key, output);
                write_value(value, output);
            }
        }
        Value::Ext(kind, payload) => {
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

fn read_value(input: &[u8], offset: usize, depth: usize) -> Result<(Value, usize), CodecError> {
    if depth > MAX_DEPTH {
        return Err(CodecError::LimitExceeded);
    }
    let tag = *input.get(offset).ok_or(CodecError::UnexpectedEof)?;
    let mut cursor = offset + 1;
    match tag {
        0xc0 => Ok((Value::Nil, cursor)),
        0xc2 => Ok((Value::Bool(false), cursor)),
        0xc3 => Ok((Value::Bool(true), cursor)),
        0x00..=0x7f => Ok((Value::UInt(tag as u64), cursor)),
        0xe0..=0xff => Ok((Value::Int((tag as i8) as i64), cursor)),
        0xcc => Ok((Value::UInt(read_u8(input, &mut cursor)? as u64), cursor)),
        0xcd => Ok((Value::UInt(read_u16(input, &mut cursor)? as u64), cursor)),
        0xce => Ok((Value::UInt(read_u32(input, &mut cursor)? as u64), cursor)),
        0xcf => Ok((Value::UInt(read_u64(input, &mut cursor)?), cursor)),
        0xd0 => Ok((
            Value::Int(read_u8(input, &mut cursor)? as i8 as i64),
            cursor,
        )),
        0xd1 => Ok((
            Value::Int(read_u16(input, &mut cursor)? as i16 as i64),
            cursor,
        )),
        0xd2 => Ok((
            Value::Int(read_u32(input, &mut cursor)? as i32 as i64),
            cursor,
        )),
        0xd3 => Ok((Value::Int(read_u64(input, &mut cursor)? as i64), cursor)),
        0xca => Ok((Value::Float32(read_u32(input, &mut cursor)?), cursor)),
        0xcb => Ok((Value::Float64(read_u64(input, &mut cursor)?), cursor)),
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
            Ok((Value::Ext(kind, payload), cursor))
        }
        0xc7 => {
            let len = read_u8(input, &mut cursor)? as usize;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((Value::Ext(kind, payload), cursor))
        }
        0xc8 => {
            let len = read_u16(input, &mut cursor)? as usize;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((Value::Ext(kind, payload), cursor))
        }
        0xc9 => {
            let len = usize::try_from(read_u32(input, &mut cursor)?)
                .map_err(|_| CodecError::InvalidLength)?;
            let kind = read_u8(input, &mut cursor)? as i8;
            let payload = read_bytes(input, &mut cursor, len)?.to_vec();
            Ok((Value::Ext(kind, payload), cursor))
        }
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_string(input: &[u8], cursor: &mut usize, len: usize) -> Result<(Value, usize), CodecError> {
    let bytes = read_bytes(input, cursor, len)?;
    let value = String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::InvalidUtf8)?;
    Ok((Value::String(value), *cursor))
}

fn read_binary(input: &[u8], cursor: &mut usize, len: usize) -> Result<(Value, usize), CodecError> {
    Ok((
        Value::Binary(read_bytes(input, cursor, len)?.to_vec()),
        *cursor,
    ))
}

fn read_array(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
    depth: usize,
) -> Result<(Value, usize), CodecError> {
    if len > MAX_ELEMENTS {
        return Err(CodecError::LimitExceeded);
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        let (value, next) = read_value(input, *cursor, depth + 1)?;
        values.push(value);
        *cursor = next;
    }
    Ok((Value::Array(values), *cursor))
}

fn read_map(
    input: &[u8],
    cursor: &mut usize,
    len: usize,
    depth: usize,
) -> Result<(Value, usize), CodecError> {
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
    Ok((Value::Map(entries), *cursor))
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
            Value::Nil,
            Value::Bool(true),
            Value::Int(-32),
            Value::Int(128),
            Value::UInt(255),
            Value::Float32(1.0f32.to_bits()),
            Value::Float64((-0.0f64).to_bits()),
            Value::String("hello".into()),
            Value::Binary(vec![0, 255]),
            Value::Ext(3, vec![1, 2]),
        ];
        for value in values {
            assert_eq!(decode(&encode(&value)).unwrap(), value);
        }
    }

    #[test]
    fn arrays_maps_and_truncation_are_checked() {
        let value = Value::Map(vec![
            (
                Value::String("items".into()),
                Value::Array(vec![Value::UInt(1)]),
            ),
            (Value::Int(-1), Value::Nil),
        ]);
        let encoded = encode(&value);
        assert_eq!(decode(&encoded).unwrap(), value);
        assert_eq!(
            decode(&encoded[..encoded.len() - 1]),
            Err(CodecError::UnexpectedEof)
        );
        assert_eq!(decode(&[0xc1]), Err(CodecError::InvalidTag));
    }

    #[test]
    fn deterministic_maps_sort_arbitrary_keys_and_reject_collisions() {
        let value = Value::Map(vec![
            (Value::String("z".into()), Value::UInt(1)),
            (Value::String("a".into()), Value::UInt(2)),
        ]);
        let encoded = encode_deterministic(&value).unwrap();
        let expected = Value::Map(vec![
            (Value::String("a".into()), Value::UInt(2)),
            (Value::String("z".into()), Value::UInt(1)),
        ]);
        assert_eq!(decode(&encoded).unwrap(), expected);
        let duplicate = Value::Map(vec![
            (Value::UInt(1), Value::Nil),
            (Value::UInt(1), Value::Nil),
        ]);
        assert_eq!(
            encode_deterministic(&duplicate),
            Err(CodecError::DuplicateKey)
        );
    }

    #[test]
    fn depth_and_collection_limits_fail_before_allocation() {
        let mut nested = vec![0x91; 258];
        nested.extend(std::iter::repeat_n(0xc0, 258));
        assert_eq!(decode(&nested), Err(CodecError::LimitExceeded));
        assert_eq!(
            decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]),
            Err(CodecError::LimitExceeded)
        );
    }

    #[test]
    fn specification_vectors_cover_unsigned_signed_string_binary_and_ext() {
        let vectors = [
            (vec![0xc0], Value::Nil),
            (vec![0xc3], Value::Bool(true)),
            (vec![0x2a], Value::UInt(42)),
            (vec![0xd0, 0xd6], Value::Int(-42)),
            (vec![0xa3, b'f', b'o', b'o'], Value::String("foo".into())),
            (vec![0xc4, 0x02, 0x00, 0xff], Value::Binary(vec![0, 255])),
            (vec![0xd4, 0x01, 0x7f], Value::Ext(1, vec![0x7f])),
        ];
        for (wire, expected) in vectors {
            assert_eq!(decode(&wire).unwrap(), expected);
            assert_eq!(encode(&expected), wire);
        }
    }

    #[test]
    fn malformed_corpus_never_publishes_a_partial_value() {
        let valid = encode(&Value::Array(vec![
            Value::Map(vec![(Value::String("x".into()), Value::UInt(1))]),
            Value::Binary(vec![1, 2, 3]),
        ]));
        for cut in 0..valid.len() {
            assert!(decode(&valid[..cut]).is_err(), "truncated input at {cut}");
        }
        for tag in [0xc1, 0xc7, 0xde, 0xdf] {
            assert!(decode(&[tag]).is_err(), "accepted malformed tag {tag:#x}");
        }
    }
}
