//! Contract-aware oracles shared by owner fuzzing and fixed regressions.

use tondo_stdlib::messagepack::{self, MessagePackErrorKind};
use tondo_stdlib::serialization::{
    self, Deserializer, Event, EventDeserializer, EventSerializer, Limits, SerializationError,
    Serializer,
};
use tondo_stdlib::{CodecError, json, protobuf};

/// Exercise arbitrary JSON and a bounded independently encoded value. The
/// arbitrary route preserves Tondo's exact-number and duplicate-key policies;
/// the generated route has a common, unambiguous serde_json domain.
pub fn json(input: &[u8]) {
    let parsed = json::parse(input);
    assert_eq!(
        json::validate(input),
        parsed.as_ref().map(|_| ()).map_err(Clone::clone)
    );
    match parsed {
        Ok(value) => {
            let encoded = json::encode(&value).expect("bounded parsed JSON encodes");
            assert_eq!(json::parse(&encoded).unwrap(), value);
            json::validate(&encoded).expect("encoded JSON validates");
        }
        Err(error) => assert!(error.location.offset <= input.len()),
    }
    let mut scalar = [0; 8];
    let len = input.len().min(scalar.len());
    scalar[..len].copy_from_slice(&input[..len]);
    let reference = serde_json::json!({
        "number": u64::from_le_bytes(scalar),
        "text": String::from_utf8_lossy(&input[..input.len().min(4096)]),
        "values": [true, false, null],
    });
    let bytes = serde_json::to_vec(&reference).unwrap();
    let value = json::parse(&bytes).expect("generated reference document parses");
    let encoded = json::encode(&value).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
        reference
    );
}

/// Decode a varint with a u128 arithmetic oracle, then check raw-field
/// partitioning and an independently constructed scalar/bytes message.
pub fn protobuf(input: &[u8]) {
    let mut offset = 0;
    let decoded = protobuf::decode_varint(input, &mut offset);
    let terminal = input.iter().take(10).position(|byte| byte & 0x80 == 0);
    let expected = if let Some(index) = terminal {
        let value: u128 = input[..=index]
            .iter()
            .enumerate()
            .map(|(index, byte)| u128::from(byte & 0x7f) * 128_u128.pow(index as u32))
            .sum();
        u64::try_from(value).map_err(|_| CodecError::VarintOverflow)
    } else if input.len() < 10 {
        Err(CodecError::UnexpectedEof)
    } else {
        Err(CodecError::VarintOverflow)
    };
    assert_eq!(decoded, expected);
    assert_eq!(
        offset,
        terminal.map_or(input.len().min(10), |index| index + 1)
    );
    if let Ok(fields) = protobuf::decode_fields(input) {
        let mut rebuilt = Vec::new();
        for field in fields {
            assert!((1..=536_870_911).contains(&field.number));
            assert!(matches!(field.wire_type, 0 | 1 | 2 | 3 | 5));
            assert!(!field.raw.is_empty());
            rebuilt.extend_from_slice(field.raw);
        }
        assert_eq!(rebuilt, input);
    }

    let payload = &input[..input.len().min(127)];
    let scalar = input.first().copied().unwrap_or_default();
    // Wire keys and the one-byte length are constructed independently of the
    // production encoder. Both canonical varint widths of this scalar occur.
    let varint = if scalar < 128 {
        vec![scalar]
    } else {
        vec![scalar | 0x80, 1]
    };
    let mut message = vec![8];
    message.extend_from_slice(&varint);
    message.extend_from_slice(&[18, payload.len() as u8]);
    message.extend_from_slice(payload);
    let fields = protobuf::decode_fields(&message).expect("generated wire fields decode");
    assert_eq!(fields.len(), 2);
    assert_eq!(
        (fields[0].number, fields[0].wire_type, fields[0].payload),
        (1, 0, varint.as_slice())
    );
    assert_eq!(
        (fields[1].number, fields[1].wire_type, fields[1].payload),
        (2, 2, payload)
    );
    let mut encoded = Vec::new();
    protobuf::encode_key(1, 0, &mut encoded).unwrap();
    protobuf::encode_varint(u64::from(scalar), &mut encoded);
    protobuf::encode_key(2, 2, &mut encoded).unwrap();
    protobuf::encode_varint(payload.len() as u64, &mut encoded);
    encoded.extend_from_slice(payload);
    assert_eq!(encoded, message);
}

pub fn messagepack(input: &[u8]) {
    let parsed = messagepack::parse(input, Default::default());
    assert_eq!(
        messagepack::validate(input, Default::default()).is_ok(),
        parsed.is_ok()
    );
    if let Ok(value) = parsed {
        // Ordinary dynamic maps preserve duplicate keys. Deterministic encoding
        // has a stricter domain and must reject canonical key collisions.
        let ordinary = messagepack::encode_value(&value, Default::default())
            .expect("a parsed bounded value supports ordinary encoding");
        messagepack::validate(&ordinary, Default::default()).expect("ordinary output validates");
        match messagepack::encode_deterministic(&value) {
            Ok(encoded) => {
                let reparsed = messagepack::parse(&encoded, Default::default())
                    .expect("deterministic output parses");
                assert_eq!(
                    messagepack::encode_deterministic(&reparsed).unwrap(),
                    encoded
                );
            }
            Err(error) => assert_eq!(error.kind, MessagePackErrorKind::DeterministicKeyCollision),
        }
    }
}

pub fn serialization(input: &[u8]) {
    let limits = Limits {
        max_depth: 8,
        max_events: 32,
        max_bytes: 1024,
        max_container_items: 16,
    };
    let text = String::from_utf8_lossy(input).into_owned();
    let expected = Event::String(text.clone());
    let mut serializer = EventSerializer::new(limits);
    serializer
        .write_event(expected.clone())
        .expect("one event fits the event budget");
    let result = serializer.finish();
    if text.len() > limits.max_bytes {
        assert_eq!(result, Err(SerializationError::LimitExceeded));
    } else {
        let events = result.expect("bounded scalar is balanced");
        assert_eq!(events.as_slice(), std::slice::from_ref(&expected));
        let mut deserializer = EventDeserializer::new(&events, limits).expect("bounded events");
        assert_eq!(deserializer.next_event().unwrap(), Some(expected));
        deserializer.finish().expect("all events consumed");
    }
    let bytes = &input[..input.len().min(1024)];
    let encoded = serialization::base64_encode(bytes);
    assert_eq!(serialization::base64_decode(&encoded).unwrap(), bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_owner_compares_values_errors_and_independent_unicode_documents() {
        for input in [
            b"".as_slice(),
            b"null",
            br#"{"a":1,"a":2}"#,
            b"[1e300,-0,1.2300]",
            b"\xff",
            "Tondo 🦀".as_bytes(),
        ] {
            json(input);
        }
        for value in 0..=255 {
            json(&[value]);
        }
    }

    #[test]
    fn protobuf_owner_checks_varint_boundaries_and_wire_payloads() {
        for input in [
            b"".as_slice(),
            &[0x80],
            &[0xff; 10],
            &[0x80; 10],
            &[8, 150, 1, 18, 2, b'o', b'k'],
        ] {
            protobuf(input);
        }
        for value in 0..=255 {
            protobuf(&[value]);
        }
        let mut largest = vec![0xff; 9];
        largest.push(1);
        protobuf(&largest);
    }

    #[test]
    fn protobuf_owner_rejects_field_numbers_above_the_wire_limit() {
        let invalid = [0x80, 0x80, 0x80, 0x80, 0x10, 0];
        assert_eq!(
            protobuf::decode_fields(&invalid),
            Err(CodecError::InvalidWireType)
        );
        protobuf(&invalid);
        let nested = [0x0b, 0x80, 0x80, 0x80, 0x80, 0x10, 0, 0x0c];
        assert_eq!(
            protobuf::decode_fields(&nested),
            Err(CodecError::InvalidWireType)
        );
        protobuf(&nested);
        let mismatched_end = [0x0b, 0x84, 0x80, 0x80, 0x80, 0x10];
        assert_eq!(
            protobuf::decode_fields(&mismatched_end),
            Err(CodecError::InvalidWireType)
        );
        protobuf(&mismatched_end);
        let largest_valid = [0xf8, 0xff, 0xff, 0xff, 0x0f, 0];
        assert_eq!(
            protobuf::decode_fields(&largest_valid).unwrap()[0].number,
            536_870_911
        );
        protobuf(&largest_valid);
    }

    #[test]
    fn messagepack_owner_accepts_preserved_duplicate_maps_but_not_canonical_collisions() {
        // Minimized nightly crash payload, excluding the owner selector 0xa6.
        let duplicate = [0x83, 0xfe, 0xd1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let value = messagepack::parse(&duplicate, Default::default()).unwrap();
        assert_eq!(
            messagepack::encode_deterministic(&value).unwrap_err().kind,
            MessagePackErrorKind::DeterministicKeyCollision
        );
        messagepack(&duplicate);
        for input in [&[0x81, 1, 2][..], &[0x92, 1, 2], &[0xc1], &[0x81]] {
            messagepack(input);
        }
        let unique = messagepack::parse(&[0x81, 1, 2], Default::default()).unwrap();
        assert_eq!(
            messagepack::encode_deterministic(&unique).unwrap(),
            [0x81, 1, 2]
        );
    }

    #[test]
    fn serialization_owner_checks_post_utf8_byte_budget_and_rejection() {
        for len in [0, 1, 1023, 1024, 1025, 65535] {
            serialization(&vec![b'a'; len]);
        }
        // Replacement characters expand invalid input from 342 to 1026 bytes.
        serialization(&vec![0xff; 341]);
        serialization(&vec![0xff; 342]);
        serialization("Tondo 🦀".as_bytes());
    }
}
