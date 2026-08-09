//! Interoperability proof against independently maintained codec engines.
//!
//! The owner kernels are intentionally not used as their own oracle here:
//! JSON is checked with `serde_json`, MessagePack with `rmpv`, and Protobuf
//! with `prost`.  The tests exercise both directions, arbitrary fragmentation,
//! malformed/truncated input, and finite-resource limits.

use std::io::Cursor;

use prost::Message;
use rmpv::Value as RmpValue;
use tondo_stdlib::{json, messagepack, protobuf};

#[test]
fn json_matches_serde_in_both_directions_and_across_fragments() {
    let external: serde_json::Value = serde_json::json!({
        "name": "Tondo",
        "items": [1, true, null, "héllo"],
        "nested": {"answer": 42, "enabled": false}
    });
    let external_bytes = serde_json::to_vec(&external).expect("serde encodes fixture");
    let parsed = json::parse(&external_bytes).expect("Tondo accepts serde JSON");
    let reencoded = json::encode(&parsed).expect("Tondo encodes parsed JSON");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reencoded).unwrap(),
        external
    );

    let tondo_value = json::JsonValue::Object(vec![
        json::JsonMember {
            key: "name".into(),
            value: json::JsonValue::String("Tondo".into()),
        },
        json::JsonMember {
            key: "items".into(),
            value: json::JsonValue::Array(vec![
                json::JsonValue::Number(json::JsonNumber::parse("1").unwrap()),
                json::JsonValue::Bool(true),
                json::JsonValue::Null,
                json::JsonValue::String("héllo".into()),
            ]),
        },
    ]);
    let tondo_bytes = json::encode(&tondo_value).expect("Tondo encodes fixture");
    let external_decoded =
        serde_json::from_slice::<serde_json::Value>(&tondo_bytes).expect("serde parses Tondo JSON");
    assert_eq!(external_decoded["name"], "Tondo");
    assert_eq!(external_decoded["items"][0], 1);

    let mut reader = json::JsonReader::from_chunks(external_bytes.chunks(1), Default::default())
        .expect("fragmented reader");
    reader
        .finish()
        .expect("one-byte fragmentation is equivalent");

    assert!(matches!(
        json::parse(br#"{"duplicate":1,"duplicate":2}"#),
        Err(error) if error.kind == json::JsonErrorKind::DuplicateKey
    ));
    assert!(json::parse(br#"{"name":"#).is_err());
    assert!(json::parse(br#"{"name":1} trailing"#).is_err());

    let limits = json::JsonLimits {
        max_document_bytes: external_bytes.len() - 1,
        ..Default::default()
    };
    let options = json::JsonDecodeOptions {
        limits,
        ..Default::default()
    };
    assert!(matches!(
        json::parse_with_options(&external_bytes, options),
        Err(error) if error.kind == json::JsonErrorKind::LimitExceeded
    ));
}

fn rmpv_to_tondo(value: &RmpValue) -> messagepack::MessagePackValue {
    use messagepack::{MessagePackEntry, MessagePackExt, MessagePackValue as TondoValue};

    match value {
        RmpValue::Nil => TondoValue::Nil,
        RmpValue::Boolean(value) => TondoValue::Bool(*value),
        RmpValue::Integer(value) => {
            if let Some(value) = value.as_i64().filter(|value| *value < 0) {
                TondoValue::Int(value)
            } else {
                TondoValue::UInt(value.as_u64().expect("positive MessagePack integer"))
            }
        }
        RmpValue::F32(value) => TondoValue::Float32(value.to_bits()),
        RmpValue::F64(value) => TondoValue::Float64(value.to_bits()),
        RmpValue::String(value) => {
            TondoValue::String(value.as_str().expect("fixture uses valid UTF-8").to_owned())
        }
        RmpValue::Binary(value) => TondoValue::Binary(value.clone()),
        RmpValue::Array(values) => TondoValue::Array(values.iter().map(rmpv_to_tondo).collect()),
        RmpValue::Map(entries) => TondoValue::Map(
            entries
                .iter()
                .map(|(key, value)| MessagePackEntry {
                    key: rmpv_to_tondo(key),
                    value: rmpv_to_tondo(value),
                })
                .collect(),
        ),
        RmpValue::Ext(type_code, payload) => TondoValue::Ext(MessagePackExt {
            type_code: *type_code,
            payload: payload.clone(),
        }),
    }
}

#[test]
fn messagepack_matches_rmpv_in_both_directions_and_preserves_extensions() {
    let external = RmpValue::Map(vec![
        (RmpValue::from("name"), RmpValue::from("Tondo")),
        (RmpValue::from("negative"), RmpValue::from(-42i64)),
        (RmpValue::from("binary"), RmpValue::Binary(vec![0, 1, 255])),
        (RmpValue::from("ext"), RmpValue::Ext(7, vec![9, 8, 7])),
        (
            RmpValue::from("floats"),
            RmpValue::Array(vec![RmpValue::F32(-0.0), RmpValue::F64(1.25)]),
        ),
    ]);
    let mut external_bytes = Vec::new();
    rmpv::encode::write_value(&mut external_bytes, &external).expect("rmpv encodes fixture");
    let parsed = messagepack::decode(&external_bytes).expect("Tondo accepts rmpv MessagePack");
    assert_eq!(parsed, rmpv_to_tondo(&external));

    let tondo_value = messagepack::MessagePackValue::Map(vec![
        messagepack::MessagePackEntry {
            key: messagepack::MessagePackValue::String("name".into()),
            value: messagepack::MessagePackValue::String("Tondo".into()),
        },
        messagepack::MessagePackEntry {
            key: messagepack::MessagePackValue::Int(-42),
            value: messagepack::MessagePackValue::Binary(vec![0, 1, 255]),
        },
        messagepack::MessagePackEntry {
            key: messagepack::MessagePackValue::Ext(messagepack::MessagePackExt {
                type_code: 7,
                payload: vec![9, 8, 7],
            }),
            value: messagepack::MessagePackValue::Float64(1.25f64.to_bits()),
        },
    ]);
    let tondo_bytes = messagepack::encode_value(
        &tondo_value,
        messagepack::MessagePackEncodeOptions::default(),
    )
    .expect("Tondo encodes fixture");
    let mut external_reader = Cursor::new(&tondo_bytes);
    let external_roundtrip =
        rmpv::decode::read_value(&mut external_reader).expect("rmpv parses Tondo MessagePack");
    assert_eq!(external_roundtrip, rmpv_to_rmpv(&tondo_value));
    assert_eq!(external_reader.position() as usize, tondo_bytes.len());

    let mut reader =
        messagepack::MessagePackReader::from_chunks(external_bytes.chunks(1), Default::default())
            .expect("fragmented reader");
    reader
        .finish()
        .expect("one-byte fragmentation is equivalent");

    for end in 0..external_bytes.len() {
        assert!(messagepack::validate(&external_bytes[..end], Default::default()).is_err());
    }
    let limits = messagepack::MessagePackLimits {
        max_document_bytes: external_bytes.len() - 1,
        ..Default::default()
    };
    let options = messagepack::MessagePackDecodeOptions {
        limits,
        ..Default::default()
    };
    assert!(matches!(
        messagepack::validate(&external_bytes, options),
        Err(error) if error.kind == messagepack::MessagePackErrorKind::LimitExceeded
    ));
}

fn rmpv_to_rmpv(value: &messagepack::MessagePackValue) -> RmpValue {
    use messagepack::MessagePackValue as TondoValue;

    match value {
        TondoValue::Nil => RmpValue::Nil,
        TondoValue::Bool(value) => RmpValue::Boolean(*value),
        TondoValue::Int(value) => RmpValue::from(*value),
        TondoValue::UInt(value) => RmpValue::from(*value),
        TondoValue::Float32(bits) => RmpValue::F32(f32::from_bits(*bits)),
        TondoValue::Float64(bits) => RmpValue::F64(f64::from_bits(*bits)),
        TondoValue::String(value) => RmpValue::from(value.as_str()),
        TondoValue::Binary(value) => RmpValue::Binary(value.clone()),
        TondoValue::Array(values) => RmpValue::Array(values.iter().map(rmpv_to_rmpv).collect()),
        TondoValue::Map(entries) => RmpValue::Map(
            entries
                .iter()
                .map(|entry| (rmpv_to_rmpv(&entry.key), rmpv_to_rmpv(&entry.value)))
                .collect(),
        ),
        TondoValue::Ext(value) => RmpValue::Ext(value.type_code, value.payload.clone()),
    }
}

#[derive(Clone, PartialEq, Message)]
struct ExternalMessage {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(bytes, tag = "3")]
    payload: Vec<u8>,
    #[prost(uint32, repeated, packed = "true", tag = "4")]
    values: Vec<u32>,
}

fn packed_varints(values: &[u64]) -> Vec<u8> {
    let mut output = Vec::new();
    for mut value in values.iter().copied() {
        while value >= 0x80 {
            output.push((value as u8) | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }
    output
}

#[test]
fn protobuf_matches_prost_and_preserves_unknown_wire_records() {
    let external = ExternalMessage {
        id: 150,
        name: "Tondo".into(),
        payload: vec![0, 1, 255],
        values: vec![1, 300, 42],
    };
    let external_bytes = external.encode_to_vec();
    let decoded_fields = protobuf::decode_message(&external_bytes, Default::default())
        .expect("Tondo accepts prost Protobuf");
    assert_eq!(decoded_fields[0].number, 1);
    assert_eq!(decoded_fields[1].number, 2);
    assert_eq!(decoded_fields[2].number, 3);
    assert_eq!(decoded_fields[3].number, 4);
    assert!(matches!(
        decoded_fields[3].value,
        protobuf::ProtoValue::Bytes(_)
    ));

    let tondo_fields = vec![
        protobuf::ProtoField {
            number: 1,
            value: protobuf::ProtoValue::Varint(150),
        },
        protobuf::ProtoField {
            number: 2,
            value: protobuf::ProtoValue::Bytes(b"Tondo".to_vec()),
        },
        protobuf::ProtoField {
            number: 3,
            value: protobuf::ProtoValue::Bytes(vec![0, 1, 255]),
        },
        protobuf::ProtoField {
            number: 4,
            value: protobuf::ProtoValue::Bytes(packed_varints(&[1, 300, 42])),
        },
    ];
    let tondo_bytes = protobuf::encode_message(
        &tondo_fields,
        protobuf::ProtoEncodeOptions {
            deterministic: true,
            ..Default::default()
        },
    )
    .expect("Tondo encodes schema wire fixture");
    let external_roundtrip =
        ExternalMessage::decode(tondo_bytes.as_slice()).expect("prost parses Tondo Protobuf");
    assert_eq!(external_roundtrip, external);

    let mut with_unknown = external_bytes.clone();
    protobuf::encode_key(99, 0, &mut with_unknown).expect("unknown field key");
    protobuf::encode_varint(7, &mut with_unknown);
    let wire_fields = protobuf::decode_fields(&with_unknown).expect("wire fields parse");
    let unknown = wire_fields.last().expect("unknown field present");
    assert_eq!(unknown.number, 99);
    assert_eq!(unknown.wire_type, 0);
    assert_eq!(unknown.raw, &[0x98, 0x06, 0x07]);
    let prost_unknown = ExternalMessage::decode(with_unknown.as_slice())
        .expect("prost preserves and skips unknown fields");
    assert_eq!(prost_unknown, external);

    let mut reader =
        protobuf::ProtoReader::<()>::from_chunks(external_bytes.chunks(1), Default::default())
            .expect("fragmented reader");
    reader
        .finish()
        .expect("one-byte fragmentation is equivalent");
    assert!(protobuf::validate::<()>(&external_bytes[..1], Default::default()).is_err());
    assert!(
        protobuf::validate::<()>(
            &external_bytes[..external_bytes.len() - 1],
            Default::default()
        )
        .is_err()
    );
    assert!(protobuf::validate::<()>(&[0x0a, 0x05, b'T'], Default::default()).is_err());
    let limits = protobuf::ProtoLimits {
        max_message_bytes: external_bytes.len() - 1,
        ..Default::default()
    };
    let options = protobuf::ProtoDecodeOptions {
        limits,
        ..Default::default()
    };
    assert!(matches!(
        protobuf::validate::<()>(&external_bytes, options),
        Err(error) if error.kind == protobuf::ProtoErrorKind::LimitExceeded
    ));
}
