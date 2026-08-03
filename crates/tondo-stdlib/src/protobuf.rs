use crate::CodecError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field<'a> {
    pub number: u32,
    pub wire_type: u8,
    pub payload: &'a [u8],
}

pub fn encode_key(number: u32, wire_type: u8, output: &mut Vec<u8>) -> Result<(), CodecError> {
    if number == 0
        || number > 536_870_911
        || !(wire_type == 0 || wire_type == 1 || wire_type == 2 || wire_type == 5)
    {
        return Err(CodecError::InvalidWireType);
    }
    encode_varint((u64::from(number) << 3) | u64::from(wire_type), output);
    Ok(())
}

pub fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

pub fn decode_varint(input: &[u8], offset: &mut usize) -> Result<u64, CodecError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *input.get(*offset).ok_or(CodecError::UnexpectedEof)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(CodecError::VarintOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(CodecError::VarintOverflow)
}

pub fn decode_fields(input: &[u8]) -> Result<Vec<Field<'_>>, CodecError> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < input.len() {
        let key = decode_varint(input, &mut offset)?;
        let number = u32::try_from(key >> 3).map_err(|_| CodecError::InvalidWireType)?;
        let wire_type = (key & 7) as u8;
        if number == 0 || wire_type == 3 || wire_type == 4 || wire_type > 5 {
            return Err(CodecError::InvalidWireType);
        }
        let payload = match wire_type {
            0 => {
                let start = offset;
                decode_varint(input, &mut offset)?;
                &input[start..offset]
            }
            1 => take(input, &mut offset, 8)?,
            2 => {
                let len = usize::try_from(decode_varint(input, &mut offset)?)
                    .map_err(|_| CodecError::InvalidLength)?;
                take(input, &mut offset, len)?
            }
            5 => take(input, &mut offset, 4)?,
            _ => return Err(CodecError::InvalidWireType),
        };
        fields.push(Field {
            number,
            wire_type,
            payload,
        });
    }
    Ok(fields)
}

fn take<'a>(input: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], CodecError> {
    let end = offset.checked_add(len).ok_or(CodecError::InvalidLength)?;
    let value = input.get(*offset..end).ok_or(CodecError::UnexpectedEof)?;
    *offset = end;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_all_scalar_wire_widths_round_trip() {
        let mut encoded = Vec::new();
        encode_key(1, 0, &mut encoded).unwrap();
        encode_varint(300, &mut encoded);
        encode_key(2, 1, &mut encoded).unwrap();
        encoded.extend_from_slice(&[0; 8]);
        encode_key(3, 2, &mut encoded).unwrap();
        encode_varint(2, &mut encoded);
        encoded.extend_from_slice(b"ok");
        encode_key(4, 5, &mut encoded).unwrap();
        encoded.extend_from_slice(&[0; 4]);
        let fields = decode_fields(&encoded).unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0].payload, &[0xac, 0x02]);
        assert_eq!(fields[2].payload, b"ok");
    }

    #[test]
    fn malformed_keys_and_payloads_fail_without_partial_success() {
        let mut output = Vec::new();
        assert_eq!(
            encode_key(0, 0, &mut output),
            Err(CodecError::InvalidWireType)
        );
        assert_eq!(decode_fields(&[0x80]), Err(CodecError::UnexpectedEof));
        assert_eq!(decode_fields(&[0x0b]), Err(CodecError::InvalidWireType));
        assert_eq!(
            decode_fields(&[0x0a, 0x05, 1]),
            Err(CodecError::UnexpectedEof)
        );
        let mut offset = 0;
        assert_eq!(
            decode_varint(&[0x80; 11], &mut offset),
            Err(CodecError::VarintOverflow)
        );
    }
}
