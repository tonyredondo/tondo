use std::io::{self, Read};

use tondo_reliability::toml_model::{
    MAX_REFERENCE_NODES, MAX_TOML_FUZZ_INPUT_BYTES, MAX_TOML_FUZZ_STEPS, ReferenceErrorKind,
    ReferenceValue, render_canonical, run_toml_fuzz_case, value_from_seed,
};
use tondo_stdlib::serialization::{Decoder, Event, Toml as TomlCodec};
use tondo_stdlib::toml::{
    self, TomlErrorKind, TomlEvent, TomlLimits, TomlMember, TomlOptions, TomlValue,
};

fn to_toml(value: &ReferenceValue) -> TomlValue {
    match value {
        ReferenceValue::Bool(value) => TomlValue::Bool(*value),
        ReferenceValue::Int(value) => TomlValue::Int(*value),
        ReferenceValue::UInt(value) => TomlValue::UInt(*value),
        ReferenceValue::Float(value) => TomlValue::Float(*value),
        ReferenceValue::Text(value) => TomlValue::Text(value.clone()),
        ReferenceValue::Array(values) => TomlValue::Array(values.iter().map(to_toml).collect()),
        ReferenceValue::Table(members) => TomlValue::Table(
            members
                .iter()
                .map(|(key, value)| TomlMember {
                    key: key.clone(),
                    value: to_toml(value),
                })
                .collect(),
        ),
    }
}

#[test]
fn bounded_reference_renderer_matches_hosted_canonical_bytes() {
    for seed in 0..4096_u64 {
        let input = seed.to_le_bytes();
        let reference = value_from_seed(&input);
        let expected = render_canonical(&reference).unwrap();
        let production = to_toml(&reference);
        let actual = toml::encode_canonical(&production, TomlLimits::default()).unwrap();
        assert_eq!(actual, expected, "canonical TOML diverged for seed {seed}");
        let parsed = toml::parse(&actual, TomlOptions::default()).unwrap();
        assert_eq!(
            toml::encode_canonical(&parsed, TomlLimits::default()).unwrap(),
            actual
        );
    }
}

#[test]
fn reference_model_replay_is_deterministic_bounded_and_independent() {
    let display = tondo_reliability::toml_model::ReferenceError {
        kind: ReferenceErrorKind::InvalidKey,
        offset: 3,
    };
    assert_eq!(display.to_string(), "InvalidKey at byte 3");
    assert_eq!(
        render_canonical(&ReferenceValue::Int(1)).unwrap_err().kind,
        ReferenceErrorKind::InvalidKey
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Float(f64::INFINITY))
            .unwrap_err()
            .kind,
        ReferenceErrorKind::InvalidKey
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Table(vec![
            ("same".into(), ReferenceValue::Int(1)),
            ("same".into(), ReferenceValue::Int(2)),
        ]))
        .unwrap_err()
        .kind,
        ReferenceErrorKind::DuplicateKey
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Table(vec![(
            "bad key".into(),
            ReferenceValue::Int(1),
        )]))
        .unwrap_err()
        .kind,
        ReferenceErrorKind::InvalidKey
    );
    let oversized = ReferenceValue::Array(vec![ReferenceValue::Bool(true); MAX_REFERENCE_NODES]);
    assert_eq!(
        render_canonical(&ReferenceValue::Table(vec![("x".into(), oversized)]))
            .unwrap_err()
            .kind,
        ReferenceErrorKind::Limit
    );

    for seed in 0..4096_u64 {
        let input = seed.to_le_bytes();
        let first = run_toml_fuzz_case(&input).unwrap();
        let second = run_toml_fuzz_case(&input).unwrap();
        assert_eq!(first, second, "TOML replay diverged for seed {seed}");
        assert!(first.steps <= MAX_TOML_FUZZ_STEPS);
        assert_eq!(first.valid_cases, first.invalid_cases);
        assert!(first.max_nodes <= MAX_REFERENCE_NODES);
    }
    for input in [
        Vec::new(),
        b"toml\0tables\xff".to_vec(),
        (0..=255).collect::<Vec<_>>(),
    ] {
        let first = run_toml_fuzz_case(&input).unwrap();
        let second = run_toml_fuzz_case(&input).unwrap();
        assert_eq!(first, second);
        assert!(first.steps <= MAX_TOML_FUZZ_STEPS);
        assert!(input.len() <= MAX_TOML_FUZZ_INPUT_BYTES || first.steps == MAX_TOML_FUZZ_STEPS);
    }
    let oversized_input = vec![0_u8; MAX_TOML_FUZZ_INPUT_BYTES + 1];
    assert_eq!(
        run_toml_fuzz_case(&oversized_input).unwrap().steps,
        MAX_TOML_FUZZ_STEPS
    );
}

#[test]
fn invalid_corpus_preserves_error_kinds_paths_and_spans() {
    for (input, expected) in [
        (b"\xff".as_slice(), TomlErrorKind::InvalidUtf8),
        (b"a = 01\n".as_slice(), TomlErrorKind::InvalidNumber),
        (
            b"a = 18446744073709551616\n".as_slice(),
            TomlErrorKind::IntegerOutOfRange,
        ),
        (
            b"a = 2024-02-30\n".as_slice(),
            TomlErrorKind::InvalidDateTime,
        ),
        (
            b"a = 12:00:00.1234567890\n".as_slice(),
            TomlErrorKind::DateTimePrecision,
        ),
        (br#"a = "\q""#.as_slice(), TomlErrorKind::InvalidEscape),
        (b"a = [1\n".as_slice(), TomlErrorKind::InvalidArray),
        (b"a = { x = 1\n".as_slice(), TomlErrorKind::InvalidArray),
        (
            b"[a]\nx = 1\n[a]\n".as_slice(),
            TomlErrorKind::DuplicateTable,
        ),
        (
            b"inline = { value = 1 }\ninline.value = 2\n".as_slice(),
            TomlErrorKind::InlineTableExtension,
        ),
    ] {
        let error = toml::parse(input, TomlOptions::default()).unwrap_err();
        assert_eq!(error.kind, expected, "input: {:?}", input);
        assert!(error.span.end_offset >= error.span.start_offset);
    }
    let duplicate = toml::parse(b"first = 1\nfirst = 2\n", TomlOptions::default()).unwrap_err();
    assert_eq!(duplicate.path.len(), 1);
    assert_eq!(duplicate.span.start_line, 2);
    assert_eq!(duplicate.span.start_column, 1);
    assert!(duplicate.span.end_offset > duplicate.span.start_offset);
}

struct OneByteReader {
    input: Vec<u8>,
    offset: usize,
}

impl Read for OneByteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.offset == self.input.len() {
            return Ok(0);
        }
        output[0] = self.input[self.offset];
        self.offset += 1;
        Ok(1)
    }
}

fn collect_events(reader: &mut toml::TomlReader) -> Vec<TomlEvent> {
    let mut events = Vec::new();
    while let Some(event) = reader.next().unwrap() {
        events.push(event);
    }
    events
}

#[test]
fn one_byte_reader_and_common_decoder_preserve_boundaries() {
    let input = b"items = [1, true]\n[meta]\nname = \"x\"\n";
    let options = TomlOptions::default();
    let mut from_bytes = toml::TomlReader::from_bytes(input, options).unwrap();
    let expected = collect_events(&mut from_bytes);
    let mut from_reader = toml::TomlReader::from_reader(
        OneByteReader {
            input: input.to_vec(),
            offset: 0,
        },
        options,
    )
    .unwrap();
    assert_eq!(collect_events(&mut from_reader), expected);
    assert_eq!(from_reader.next().unwrap_err().kind, TomlErrorKind::Closed);

    let mut decoder = toml::TomlReader::from_bytes(b"items = [1, true]\n", options).unwrap();
    assert_eq!(
        <toml::TomlReader as Decoder<TomlCodec, toml::TomlError>>::peek_event(&mut decoder)
            .unwrap(),
        Some(Event::StartMap(Some(1)))
    );
    assert_eq!(
        <toml::TomlReader as Decoder<TomlCodec, toml::TomlError>>::next(&mut decoder).unwrap(),
        Some(Event::StartMap(Some(1)))
    );
    assert_eq!(
        <toml::TomlReader as Decoder<TomlCodec, toml::TomlError>>::next(&mut decoder).unwrap(),
        Some(Event::MapKey)
    );
    assert_eq!(
        <toml::TomlReader as Decoder<TomlCodec, toml::TomlError>>::next(&mut decoder).unwrap(),
        Some(Event::String("items".into()))
    );
    assert_eq!(
        <toml::TomlReader as Decoder<TomlCodec, toml::TomlError>>::next(&mut decoder).unwrap(),
        Some(Event::StartArray(Some(2)))
    );
}

#[test]
fn stream_limits_and_terminal_states_remain_explicit() {
    let input = b"a = 1\n";
    assert_eq!(
        toml::TomlReader::from_chunks(
            [input.as_slice()],
            TomlOptions::create(TomlLimits {
                max_input_bytes: 2,
                ..TomlLimits::default()
            })
        )
        .unwrap_err()
        .kind,
        TomlErrorKind::ResourceLimit
    );
    assert_eq!(
        toml::TomlReader::from_reader(
            io::Cursor::new(input),
            TomlOptions::create(TomlLimits {
                max_input_bytes: 2,
                ..TomlLimits::default()
            })
        )
        .unwrap_err()
        .kind,
        TomlErrorKind::ResourceLimit
    );

    let mut reader = toml::TomlReader::from_bytes(input, TomlOptions::default()).unwrap();
    while reader.next().unwrap().is_some() {}
    assert_eq!(reader.finish().unwrap_err().kind, TomlErrorKind::Closed);
    let mut writer = toml::TomlWriter::to_writer(TomlOptions::default()).unwrap();
    writer.write(TomlEvent::StreamStart).unwrap();
    assert_eq!(
        writer.finish().unwrap_err().kind,
        TomlErrorKind::UnexpectedToken
    );
    assert_eq!(
        writer.write(TomlEvent::StreamEnd).unwrap_err().kind,
        TomlErrorKind::UnexpectedToken
    );
}
