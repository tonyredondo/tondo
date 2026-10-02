//! Executable usage of the bounded Rust CBOR kernel.
//! This does not execute a public Tondo std.cbor call or native AOT lowering.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use tondo_stdlib::cbor::{
    self, CborDecodeOptions, CborDuplicatePolicy, CborEncodeOptions, CborEntry, CborError,
    CborErrorKind, CborEvent, CborFloat16, CborLimits, CborPath, CborReader, CborTag,
    CborUnknownTagPolicy, CborValue, CborWriter,
};
use tondo_stdlib::serialization::Bytes;

fn materialized_and_typed() -> Result<(), CborError> {
    let options = CborDecodeOptions::default();
    let values = vec![1_u64, 24, u64::MAX];
    let encoded = cbor::encode_typed(&values, CborEncodeOptions::default())?;
    assert_eq!(cbor::decode_typed::<Vec<u64>>(&encoded, options)?, values);
    assert_eq!(
        cbor::parse(&encoded, options)?,
        CborValue::Array(values.iter().copied().map(CborValue::UInt).collect())
    );

    // Vec<u8> is an array of integers; Bytes selects the binary payload shape.
    assert_eq!(
        cbor::encode_typed(&vec![1_u8, 2], CborEncodeOptions::default())?,
        [0x82, 0x01, 0x02]
    );
    assert_eq!(
        cbor::encode_typed(&Bytes::from_slice(&[1, 2]), CborEncodeOptions::default())?,
        [0x42, 0x01, 0x02]
    );

    let duplicate = [0xa2, 0x01, 0x02, 0x01, 0x03];
    assert_eq!(
        cbor::decode_typed::<BTreeMap<u64, u64>>(&duplicate, options)
            .unwrap_err()
            .kind,
        CborErrorKind::DuplicateKey
    );
    for (policy, expected) in [
        (CborDuplicatePolicy::First, 2),
        (CborDuplicatePolicy::Last, 3),
    ] {
        let selected = CborDecodeOptions {
            typed_map_duplicates: policy,
            ..options
        };
        assert_eq!(
            cbor::decode_typed::<BTreeMap<u64, u64>>(&duplicate, selected)?,
            BTreeMap::from([(1, expected)])
        );
    }
    Ok(())
}

fn tags_and_determinism() -> Result<(), CborError> {
    let options = CborDecodeOptions::default();
    let tagged = [0xd9, 0xff, 0xff, 0xf7];
    assert_eq!(
        cbor::parse(&tagged, options)?,
        CborValue::Tag(CborTag {
            number: 65535,
            value: Box::new(CborValue::Undefined),
        })
    );
    let reject_tags = CborDecodeOptions {
        unknown_tags: CborUnknownTagPolicy::Reject,
        ..options
    };
    assert_eq!(
        cbor::parse(&tagged, reject_tags).unwrap_err().kind,
        CborErrorKind::UnknownTag
    );

    let value = CborValue::Map(vec![
        CborEntry {
            key: CborValue::Text("b".into()),
            value: CborValue::Float64((-0.0_f64).to_bits()),
        },
        CborEntry {
            key: CborValue::UInt(1),
            value: CborValue::Float32(1.5_f32.to_bits()),
        },
    ]);
    let deterministic = cbor::encode_deterministic(&value, CborLimits::default())?;
    assert_eq!(
        deterministic,
        [0xa2, 0x01, 0xf9, 0x3e, 0x00, 0x61, b'b', 0xf9, 0x80, 0x00]
    );
    assert_eq!(
        cbor::encode_deterministic(
            &cbor::parse(&deterministic, options)?,
            CborLimits::default()
        )?,
        deterministic
    );
    let nan = CborValue::Float16(CborFloat16 { bits: 0x7e01 });
    assert_eq!(
        cbor::encode(&nan, CborEncodeOptions::default())?,
        [0xf9, 0x7e, 0x01]
    );
    assert_eq!(
        cbor::encode_deterministic(&nan, CborLimits::default())?,
        [0xf9, 0x7e, 0x00]
    );

    let collision = CborValue::Map(vec![
        CborEntry {
            key: nan,
            value: CborValue::Null,
        },
        CborEntry {
            key: CborValue::Float64(f64::NAN.to_bits()),
            value: CborValue::Null,
        },
    ]);
    assert_eq!(
        cbor::encode_deterministic(&collision, CborLimits::default())
            .unwrap_err()
            .kind,
        CborErrorKind::DeterministicKeyCollision
    );
    Ok(())
}

fn raw_view_and_costs() -> Result<(), CborError> {
    let source = [0x9f, 0x18, 0x01, 0xf7, 0xff];
    let options = CborDecodeOptions::default();
    let view = cbor::parse_view(&source, options)?;
    assert_eq!(view.as_bytes(), source);
    assert_eq!(view.as_bytes().as_ptr(), source.as_ptr());
    assert_eq!(view.own()?, cbor::parse(&source, options)?);
    // Validation builds and drops a DOM. own() reparses into an owned value.
    let raw = cbor::raw(&source, options)?;
    assert_eq!(raw.as_bytes(), source);
    assert_eq!(
        cbor::encode(&view.own()?, CborEncodeOptions::default())?,
        [0x82, 0x01, 0xf7]
    );
    Ok(())
}

struct OneByte<'a>(&'a [u8]);

impl Read for OneByte<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() || buffer.is_empty() {
            return Ok(0);
        }
        buffer[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}

fn buffered_events_and_lifecycle() -> Result<(), CborError> {
    let source = [0x9f, 0x01, 0x7f, 0x61, b'h', 0x61, b'i', 0xff, 0xff];
    let mut reader = CborReader::from_reader(OneByte(&source), CborDecodeOptions::default())?;
    let mut events = Vec::new();
    while let Some(event) = reader.next()? {
        events.push(reader.own(&event)?);
    }
    assert_eq!(reader.next().unwrap_err().kind, CborErrorKind::Closed);
    reader.finish()?;
    assert_eq!(reader.finish().unwrap_err().kind, CborErrorKind::Closed);
    assert_eq!(
        events,
        [
            CborEvent::StreamStart,
            CborEvent::StartArray(None),
            CborEvent::UInt(1),
            CborEvent::StartText(None),
            CborEvent::TextChunk("h".into()),
            CborEvent::TextChunk("i".into()),
            CborEvent::EndText,
            CborEvent::EndArray,
            CborEvent::StreamEnd,
        ]
    );
    let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default())?;
    for event in events {
        writer.write(event)?;
    }
    assert_eq!(writer.finish()?, source);
    assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);

    let mut deterministic = CborWriter::to_writer(
        Vec::new(),
        CborEncodeOptions {
            deterministic: true,
            ..CborEncodeOptions::default()
        },
    )?;
    deterministic.write(CborEvent::StreamStart)?;
    assert_eq!(
        deterministic
            .write(CborEvent::StartArray(None))
            .unwrap_err()
            .kind,
        CborErrorKind::IndefiniteNotAllowed
    );
    assert_eq!(
        deterministic.finish().unwrap_err().kind,
        CborErrorKind::Closed
    );
    Ok(())
}

fn errors_and_limits() -> Result<(), CborError> {
    let options = CborDecodeOptions::default();
    let error = cbor::parse(&[0x81, 0xa1, 0x01, 0xc1, 0x62, 0xc3, 0x00], options).unwrap_err();
    assert_eq!(error.kind, CborErrorKind::InvalidUtf8);
    assert_eq!((error.start_offset, error.end_offset), (4, 7));
    assert_eq!(
        error.path,
        [
            CborPath::ArrayIndex(0),
            CborPath::MapEntry(0),
            CborPath::MapValue,
            CborPath::Tag
        ]
    );
    let limits = CborLimits::create(CborLimits {
        max_document_bytes: 1,
        ..CborLimits::default()
    })?;
    assert_eq!(
        cbor::parse(&[0x18, 0x18], CborDecodeOptions { limits, ..options })
            .unwrap_err()
            .kind,
        CborErrorKind::LimitExceeded
    );
    assert_eq!(
        CborLimits::create(CborLimits {
            max_depth: 0,
            ..CborLimits::default()
        })
        .unwrap_err()
        .kind,
        CborErrorKind::LimitExceeded
    );
    let mut sink = Vec::new();
    let mut writer = CborWriter::to_writer(
        &mut sink,
        CborEncodeOptions {
            limits: CborLimits {
                max_output_bytes: 1,
                ..CborLimits::default()
            },
            deterministic: false,
        },
    )?;
    writer.write(CborEvent::StreamStart)?;
    assert_eq!(
        writer.write(CborEvent::UInt(24)).unwrap_err().kind,
        CborErrorKind::LimitExceeded
    );
    assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);
    drop(writer);
    assert!(sink.is_empty());
    Ok(())
}

struct PrefixThenError<'a> {
    accepted: &'a mut Vec<u8>,
    wrote: bool,
}

impl Write for PrefixThenError<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.wrote {
            return Err(std::io::Error::other("deliberate failure after a prefix"));
        }
        self.accepted.push(bytes[0]);
        self.wrote = true;
        Ok(1)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn partial_io_is_not_transactional() -> Result<(), CborError> {
    let mut accepted = Vec::new();
    let mut writer = CborWriter::to_writer(
        PrefixThenError {
            accepted: &mut accepted,
            wrote: false,
        },
        CborEncodeOptions::default(),
    )?;
    writer.write(CborEvent::StreamStart)?;
    writer.write(CborEvent::UInt(24))?;
    writer.write(CborEvent::StreamEnd)?;
    assert_eq!(
        writer.finish().err().expect("sink fails after prefix").kind,
        CborErrorKind::IoError
    );
    assert_eq!(
        writer.finish().err().expect("writer is terminal").kind,
        CborErrorKind::Closed
    );
    drop(writer);
    assert_eq!(accepted, [0x18]);
    Ok(())
}

fn main() -> Result<(), CborError> {
    materialized_and_typed()?;
    tags_and_determinism()?;
    raw_view_and_costs()?;
    buffered_events_and_lifecycle()?;
    errors_and_limits()?;
    partial_io_is_not_transactional()?;
    println!("cbor-doc-ok");
    Ok(())
}
