//! Shared, bounded kernel cases for the private VM adapter and native process.
//! Assertions use authored wire expectations; neither route is a public CBOR ABI.

use std::collections::BTreeMap;
use std::io::{self, Read};

use tondo_stdlib::cbor::{
    self, CborDecodeOptions, CborDuplicatePolicy, CborEncodeOptions, CborError, CborErrorKind,
    CborEvent, CborLimits, CborPath, CborReader, CborValue, CborWriter,
};
use tondo_stdlib::json::{self, JsonValue};

pub const CASES: [&str; 7] = [
    "typed-dynamic",
    "wire-model",
    "deterministic",
    "streaming",
    "errors-path",
    "limits-lifecycle",
    "route-boundary",
];
pub const CORPUS: &[u8] =
    include_bytes!("../../../tondo-reliability/tests/fixtures/cbor-wire.json");
pub const TYPED_WIRE: &[u8] = &[
    0x83, 0x01, 0x18, 0x18, 0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];
pub const PATH_WIRE: &[u8] = &[0x81, 0xa1, 0x61, b'x', 0xc0, 0x61, 0xff];

pub fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn field<'a>(object: &'a JsonValue, name: &str) -> Option<&'a JsonValue> {
    let JsonValue::Object(members) = object else {
        panic!("fixture object")
    };
    members
        .iter()
        .find(|member| member.key == name)
        .map(|member| &member.value)
}

fn text<'a>(object: &'a JsonValue, name: &str) -> &'a str {
    let Some(JsonValue::String(value)) = field(object, name) else {
        panic!("fixture string {name}")
    };
    value
}

fn array<'a>(object: &'a JsonValue, name: &str) -> &'a [JsonValue] {
    let Some(JsonValue::Array(values)) = field(object, name) else {
        panic!("fixture array {name}")
    };
    values
}

fn typed_dynamic() -> String {
    let values = vec![1u64, 24, u64::MAX];
    let encoded = cbor::encode_typed(&values, CborEncodeOptions::default()).unwrap();
    assert_eq!(encoded, TYPED_WIRE);
    let decoded: Vec<u64> = cbor::decode_typed(TYPED_WIRE, CborDecodeOptions::default()).unwrap();
    assert_eq!(decoded, values);
    let dynamic = cbor::parse(TYPED_WIRE, CborDecodeOptions::default()).unwrap();
    assert_eq!(
        dynamic,
        CborValue::Array(values.iter().copied().map(CborValue::UInt).collect())
    );
    let duplicate = unhex("a201020103");
    assert_eq!(
        cbor::decode_typed::<BTreeMap<u64, u64>>(&duplicate, CborDecodeOptions::default())
            .unwrap_err()
            .kind,
        CborErrorKind::DuplicateKey
    );
    let first: BTreeMap<u64, u64> = cbor::decode_typed(
        &duplicate,
        CborDecodeOptions {
            typed_map_duplicates: CborDuplicatePolicy::First,
            ..CborDecodeOptions::default()
        },
    )
    .unwrap();
    let last: BTreeMap<u64, u64> = cbor::decode_typed(
        &duplicate,
        CborDecodeOptions {
            typed_map_duplicates: CborDuplicatePolicy::Last,
            ..CborDecodeOptions::default()
        },
    )
    .unwrap();
    assert_eq!(first.get(&1), Some(&2));
    assert_eq!(last.get(&1), Some(&3));
    assert_eq!(
        cbor::decode_typed::<u64>(&[0x61, b'x'], CborDecodeOptions::default())
            .unwrap_err()
            .kind,
        CborErrorKind::TypeMismatch
    );
    format!(
        "typed-dynamic:{}:{}:{}:{}",
        decoded.len(),
        decoded[2],
        first[&1],
        last[&1]
    )
}

fn wire_model(corpus: &JsonValue) -> String {
    let valid = array(corpus, "valid");
    let invalid = array(corpus, "invalid");
    for fixture in valid {
        let wire = unhex(text(fixture, "wire"));
        let value = cbor::parse(&wire, CborDecodeOptions::default()).unwrap();
        assert_eq!(
            cbor::encode(&value, CborEncodeOptions::default()).unwrap(),
            unhex(text(fixture, "ordinary")),
            "{}",
            text(fixture, "id")
        );
        let view = cbor::parse_view(&wire, CborDecodeOptions::default()).unwrap();
        assert_eq!(view.as_bytes().as_ptr(), wire.as_ptr());
        assert_eq!(view.own().unwrap(), value);
        assert_eq!(
            cbor::raw(&wire, CborDecodeOptions::default())
                .unwrap()
                .as_bytes(),
            wire
        );
    }
    for fixture in invalid {
        let wire = unhex(text(fixture, "wire"));
        let error = cbor::parse(&wire, CborDecodeOptions::default()).unwrap_err();
        assert_eq!(format!("{:?}", error.kind), text(fixture, "error"));
        assert!(error.start_offset <= error.end_offset && error.end_offset <= wire.len());
        assert!(cbor::parse_view(&wire, CborDecodeOptions::default()).is_err());
        assert!(cbor::raw(&wire, CborDecodeOptions::default()).is_err());
        assert!(CborReader::from_bytes(&wire, CborDecodeOptions::default()).is_err());
    }
    format!(
        "wire-model:{}:{}:raw-view-widths-tags",
        valid.len(),
        invalid.len()
    )
}

fn deterministic(corpus: &JsonValue) -> String {
    let mut encoded_count = 0;
    let mut collisions = 0;
    for fixture in array(corpus, "valid") {
        let value =
            cbor::parse(&unhex(text(fixture, "wire")), CborDecodeOptions::default()).unwrap();
        let encoded = cbor::encode_deterministic(&value, CborLimits::default());
        if let Some(JsonValue::String(expected)) = field(fixture, "deterministic") {
            let encoded = encoded.unwrap();
            assert_eq!(encoded, unhex(expected));
            let reparsed = cbor::parse(&encoded, CborDecodeOptions::default()).unwrap();
            assert_eq!(
                cbor::encode_deterministic(&reparsed, CborLimits::default()).unwrap(),
                encoded
            );
            encoded_count += 1;
        } else {
            assert_eq!(text(fixture, "deterministic_error"), "KeyCollision");
            assert_eq!(
                encoded.unwrap_err().kind,
                CborErrorKind::DeterministicKeyCollision
            );
            collisions += 1;
        }
    }
    format!("deterministic:{encoded_count}:{collisions}:bytewise-shortest-nan-sign")
}

struct FragmentReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    chunk: usize,
}
impl Read for FragmentReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let size = output
            .len()
            .min(self.chunk)
            .min(self.bytes.len() - self.offset);
        output[..size].copy_from_slice(&self.bytes[self.offset..self.offset + size]);
        self.offset += size;
        Ok(size)
    }
}

fn events(mut reader: CborReader) -> Vec<CborEvent> {
    let mut output = Vec::new();
    while let Some(event) = reader.next().unwrap() {
        assert_eq!(reader.own(&event).unwrap(), event);
        output.push(event);
    }
    assert_eq!(reader.next().unwrap_err().kind, CborErrorKind::Closed);
    reader.finish().unwrap();
    assert_eq!(reader.next().unwrap_err().kind, CborErrorKind::Closed);
    assert_eq!(
        reader.own(&CborEvent::Null).unwrap_err().kind,
        CborErrorKind::Closed
    );
    assert_eq!(reader.finish().unwrap_err().kind, CborErrorKind::Closed);
    output
}

fn streaming(corpus: &JsonValue) -> String {
    let mut count = 0;
    for fixture in array(corpus, "valid") {
        let wire = unhex(text(fixture, "wire"));
        let expected = events(CborReader::from_bytes(&wire, CborDecodeOptions::default()).unwrap());
        for chunk in [1, 2, 7] {
            let fragmented = CborReader::from_reader(
                FragmentReader {
                    bytes: &wire,
                    offset: 0,
                    chunk,
                },
                CborDecodeOptions::default(),
            )
            .unwrap();
            assert_eq!(events(fragmented), expected);
        }
        let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
        for event in &expected {
            writer.write(event.clone()).unwrap();
        }
        let encoded = writer.finish().unwrap();
        assert_eq!(
            cbor::parse(&encoded, CborDecodeOptions::default()).unwrap(),
            cbor::parse(&wire, CborDecodeOptions::default()).unwrap()
        );
        assert_eq!(
            writer.write(CborEvent::Null).unwrap_err().kind,
            CborErrorKind::Closed
        );
        assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);
        count += expected.len();
    }
    format!(
        "streaming:{}:{count}:fragments-1-2-7:closed",
        array(corpus, "valid").len()
    )
}

fn errors_path() -> String {
    let expected = CborError {
        kind: CborErrorKind::InvalidUtf8,
        start_offset: 5,
        end_offset: 7,
        path: vec![
            CborPath::ArrayIndex(0),
            CborPath::MapEntry(0),
            CborPath::MapValue,
            CborPath::Tag,
        ],
    };
    assert_eq!(
        cbor::parse(PATH_WIRE, CborDecodeOptions::default()).unwrap_err(),
        expected
    );
    assert_eq!(
        cbor::validate(PATH_WIRE, CborDecodeOptions::default()).unwrap_err(),
        expected
    );
    assert_eq!(
        cbor::raw(PATH_WIRE, CborDecodeOptions::default()).unwrap_err(),
        expected
    );
    assert_eq!(
        cbor::parse_view(PATH_WIRE, CborDecodeOptions::default()).unwrap_err(),
        expected
    );
    assert_eq!(
        CborReader::from_bytes(PATH_WIRE, CborDecodeOptions::default()).unwrap_err(),
        expected
    );
    format!(
        "errors-path:{:?}:{}:{}:array-0-map-0-value-tag",
        expected.kind, expected.start_offset, expected.end_offset
    )
}

fn limits_lifecycle() -> String {
    let input_limits = CborLimits {
        max_document_bytes: 1,
        ..CborLimits::default()
    };
    let options = CborDecodeOptions {
        limits: input_limits,
        ..CborDecodeOptions::default()
    };
    assert_eq!(
        cbor::parse(TYPED_WIRE, options).unwrap_err().kind,
        CborErrorKind::LimitExceeded
    );
    assert!(
        CborReader::from_reader(
            FragmentReader {
                bytes: TYPED_WIRE,
                offset: 0,
                chunk: 1
            },
            options
        )
        .is_err()
    );
    let deep = unhex("818100");
    let depth = CborDecodeOptions {
        limits: CborLimits {
            max_depth: 1,
            ..CborLimits::default()
        },
        ..CborDecodeOptions::default()
    };
    assert_eq!(
        cbor::parse(&deep, depth).unwrap_err().kind,
        CborErrorKind::LimitExceeded
    );
    assert!(CborReader::from_bytes(&deep, depth).is_err());
    let value = cbor::parse(TYPED_WIRE, CborDecodeOptions::default()).unwrap();
    let encode = CborEncodeOptions {
        limits: CborLimits {
            max_output_bytes: 1,
            ..CborLimits::default()
        },
        ..CborEncodeOptions::default()
    };
    assert_eq!(
        cbor::encode(&value, encode).unwrap_err().kind,
        CborErrorKind::LimitExceeded
    );
    let mut reader = CborReader::from_bytes(&[0], CborDecodeOptions::default()).unwrap();
    assert_eq!(
        reader.finish().unwrap_err().kind,
        CborErrorKind::TrailingData
    );
    assert_eq!(reader.next().unwrap_err().kind, CborErrorKind::Closed);
    assert_eq!(
        reader.own(&CborEvent::Null).unwrap_err().kind,
        CborErrorKind::Closed
    );
    let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
    writer.write(CborEvent::StreamStart).unwrap();
    assert_eq!(
        writer.write(CborEvent::EndArray).unwrap_err().kind,
        CborErrorKind::TypeMismatch
    );
    assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);
    assert_eq!(
        writer.write(CborEvent::Null).unwrap_err().kind,
        CborErrorKind::Closed
    );
    "limits-lifecycle:input-depth-output:atomic:terminal".into()
}

pub fn run_case(case: &str) -> Option<String> {
    match case {
        "typed-dynamic" => Some(typed_dynamic()),
        "wire-model" | "deterministic" | "streaming" => {
            let corpus = json::parse(CORPUS).expect("retained corpus JSON");
            Some(match case {
                "wire-model" => wire_model(&corpus),
                "deterministic" => deterministic(&corpus),
                _ => streaming(&corpus),
            })
        }
        "errors-path" => Some(errors_path()),
        "limits-lifecycle" => Some(limits_lifecycle()),
        "route-boundary" => Some(
            "route-boundary:scalar:public-api-not-implemented:native-abi-aot-not-claimed".into(),
        ),
        _ => None,
    }
}
