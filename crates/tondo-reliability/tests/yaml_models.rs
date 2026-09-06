use std::io::{self, Read};

use tondo_reliability::yaml_model::{
    MAX_YAML_FUZZ_INPUT_BYTES, MAX_YAML_FUZZ_STEPS, ReferenceErrorKind, ReferenceValue,
    parse_core_scalar, render_canonical, run_yaml_fuzz_case, value_from_seed,
};
use tondo_stdlib::serialization::{Decoder, Event, Yaml as YamlCodec};
use tondo_stdlib::yaml::{self, YamlErrorKind, YamlEvent, YamlLimits, YamlOptions, YamlValue};

fn to_yaml(value: &ReferenceValue) -> YamlValue {
    match value {
        ReferenceValue::Null => YamlValue::Null,
        ReferenceValue::Bool(value) => YamlValue::Bool(*value),
        ReferenceValue::Int(value) => YamlValue::Int(*value),
        ReferenceValue::UInt(value) => YamlValue::UInt(*value),
        ReferenceValue::Float(value) => YamlValue::Float(*value),
        ReferenceValue::Text(value) => YamlValue::Text(value.clone()),
        ReferenceValue::Bytes(value) => YamlValue::Bytes(value.clone()),
        ReferenceValue::Array(values) => YamlValue::Array(values.iter().map(to_yaml).collect()),
        ReferenceValue::Object(members) => YamlValue::Object(
            members
                .iter()
                .map(|(key, value)| yaml::YamlMember {
                    key: key.clone(),
                    value: to_yaml(value),
                })
                .collect(),
        ),
    }
}

fn assert_scalar(input: &[u8], expected: ReferenceValue) {
    assert_eq!(parse_core_scalar(input).unwrap(), expected);
    assert_eq!(yaml::parse(input).unwrap(), to_yaml(&expected));
}

#[test]
fn core_scalar_model_matches_production() {
    for (input, expected) in [
        (b"".as_slice(), ReferenceValue::Null),
        (b"~", ReferenceValue::Null),
        (b"TRUE", ReferenceValue::Bool(true)),
        (b"false", ReferenceValue::Bool(false)),
        (b"0b101", ReferenceValue::Int(5)),
        (b"0o17", ReferenceValue::Int(15)),
        (b"0x1f", ReferenceValue::Int(31)),
        (
            b"9223372036854775808",
            ReferenceValue::UInt(9_223_372_036_854_775_808),
        ),
        (b"-9223372036854775808", ReferenceValue::Int(i64::MIN)),
        (b"1.25", ReferenceValue::Float(1.25)),
        (b"1e2", ReferenceValue::Float(100.0)),
        (b"yes", ReferenceValue::Text("yes".into())),
        (b"2026-09-06", ReferenceValue::Text("2026-09-06".into())),
    ] {
        assert_scalar(input, expected);
    }
}

#[test]
fn canonical_model_matches_production_and_is_idempotent() {
    let values = [
        ReferenceValue::Null,
        ReferenceValue::Text("plain".into()),
        ReferenceValue::Text(String::from("line\n\tquote \" ") + "\\"),
        ReferenceValue::Bytes(vec![0, 1, 2, 255]),
        ReferenceValue::Array(vec![
            ReferenceValue::Int(-3),
            ReferenceValue::UInt(u64::MAX),
        ]),
        ReferenceValue::Object(vec![
            (
                "z".into(),
                ReferenceValue::Array(vec![ReferenceValue::Bool(true)]),
            ),
            ("a".into(), ReferenceValue::Text("safe".into())),
        ]),
    ];
    for value in values {
        let expected = render_canonical(&value).unwrap();
        let actual = yaml::encode_canonical(&to_yaml(&value), YamlLimits::default()).unwrap();
        assert_eq!(actual, expected);
        let parsed = yaml::parse(&actual).unwrap();
        assert_eq!(
            yaml::encode_canonical(&parsed, YamlLimits::default()).unwrap(),
            actual
        );
        assert_eq!(
            yaml::encode_canonical(&to_yaml(&value), YamlLimits::default()).unwrap(),
            actual
        );
    }
}

#[test]
fn invalid_scalar_model_and_security_boundaries_match_production() {
    for (input, kind) in [
        (b".inf".as_slice(), ReferenceErrorKind::NonFiniteNumber),
        (
            b"18446744073709551616",
            ReferenceErrorKind::NumberOutOfRange,
        ),
        (b"!custom value", ReferenceErrorKind::InvalidTag),
        (b"*missing", ReferenceErrorKind::UndefinedAlias),
        (b"&1 value", ReferenceErrorKind::InvalidAnchor),
        (b"<<: value", ReferenceErrorKind::MergeKeyForbidden),
        (b"\xff", ReferenceErrorKind::InvalidUtf8),
    ] {
        let model = parse_core_scalar(input).unwrap_err();
        assert_eq!(model.kind, kind);
        let actual = yaml::parse(input).unwrap_err().kind;
        let expected = match kind {
            ReferenceErrorKind::NonFiniteNumber => YamlErrorKind::NonFiniteNumber,
            ReferenceErrorKind::NumberOutOfRange => YamlErrorKind::NumberOutOfRange,
            ReferenceErrorKind::InvalidTag => YamlErrorKind::InvalidTag,
            ReferenceErrorKind::UndefinedAlias => YamlErrorKind::UndefinedAlias,
            ReferenceErrorKind::InvalidAnchor => YamlErrorKind::InvalidAnchor,
            ReferenceErrorKind::MergeKeyForbidden => YamlErrorKind::MergeKeyForbidden,
            ReferenceErrorKind::InvalidUtf8 => YamlErrorKind::InvalidUtf8,
            other => panic!("unexpected model error: {other:?}"),
        };
        assert_eq!(actual, expected);
    }
    assert_eq!(
        yaml::parse(b"a: 1\na: 2\n").unwrap_err().kind,
        YamlErrorKind::DuplicateKey
    );
    assert_eq!(
        yaml::parse(b"{1: value}\n").unwrap_err().kind,
        YamlErrorKind::NonStringKey
    );
}

#[test]
fn reference_model_edge_cases_cover_limits_and_canonical_safety() {
    let display = tondo_reliability::yaml_model::ReferenceError {
        kind: ReferenceErrorKind::InvalidScalar,
        offset: 3,
    };
    assert_eq!(display.to_string(), "InvalidScalar at byte 3");

    assert_eq!(
        parse_core_scalar(b"0x").unwrap(),
        ReferenceValue::Text("0x".into())
    );
    assert_eq!(
        parse_core_scalar(b"0xgg").unwrap_err().kind,
        ReferenceErrorKind::NumberOutOfRange
    );
    assert_eq!(
        parse_core_scalar(b"-18446744073709551616")
            .unwrap_err()
            .kind,
        ReferenceErrorKind::NumberOutOfRange
    );
    assert_eq!(
        parse_core_scalar(b"1e999").unwrap_err().kind,
        ReferenceErrorKind::NonFiniteNumber
    );

    assert_eq!(
        render_canonical(&ReferenceValue::Array(Vec::new())).unwrap(),
        b"[]\n"
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Object(Vec::new())).unwrap(),
        b"{}\n"
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Float(f64::INFINITY))
            .unwrap_err()
            .kind,
        ReferenceErrorKind::NonFiniteNumber
    );
    assert_eq!(
        render_canonical(&ReferenceValue::Object(vec![
            ("name".into(), ReferenceValue::Text("one".into())),
            ("name".into(), ReferenceValue::Text("two".into())),
        ]))
        .unwrap_err()
        .kind,
        ReferenceErrorKind::DuplicateKey
    );

    for text in ["", " leading", "-flag", "a: b", "a # b", "?"] {
        let encoded = render_canonical(&ReferenceValue::Text(text.into())).unwrap();
        assert!(encoded.ends_with(b"\n"));
    }
    let escaped = render_canonical(&ReferenceValue::Text(
        "quote \" slash \\ carriage\r tab\t control\u{0007}".into(),
    ))
    .unwrap();
    let escaped = String::from_utf8(escaped).unwrap();
    assert!(escaped.contains("\\r"));
    assert!(escaped.contains("\\t"));
    assert!(escaped.contains("\\u0007"));

    let nested = value_from_seed(&[7, 15, 0]);
    assert!(matches!(nested, ReferenceValue::Array(_)));
    let nested_rendered = render_canonical(&nested).unwrap();
    assert!(nested_rendered.windows(3).any(|window| window == b"- \n"));
    let object = value_from_seed(&[15, 7, 0]);
    assert!(matches!(object, ReferenceValue::Object(_)));
    assert!(run_yaml_fuzz_case(&[7, 15, 0]).unwrap().max_depth >= 2);

    let limited = ReferenceValue::Array(vec![ReferenceValue::Null; 129]);
    assert_eq!(
        render_canonical(&limited).unwrap_err().kind,
        ReferenceErrorKind::Limit
    );
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

fn collect_events(reader: &mut yaml::YamlReader) -> Vec<YamlEvent> {
    let mut events = Vec::new();
    while let Some(event) = reader.next().unwrap() {
        events.push(event);
    }
    events
}

#[test]
fn one_byte_reader_and_event_decoder_preserve_stream_boundaries() {
    let input = b"---\na: [1, true]\n...\n---\nnull\n";
    let options = YamlOptions::default();
    let mut from_bytes = yaml::YamlReader::from_bytes(input, options).unwrap();
    let expected = collect_events(&mut from_bytes);
    let mut from_reader = yaml::YamlReader::from_reader(
        OneByteReader {
            input: input.to_vec(),
            offset: 0,
        },
        options,
    )
    .unwrap();
    assert_eq!(collect_events(&mut from_reader), expected);
    assert_eq!(from_reader.next().unwrap_err().kind, YamlErrorKind::Closed);

    let mut decoder = yaml::YamlReader::from_bytes(b"[1, true]", options).unwrap();
    assert_eq!(
        <yaml::YamlReader as Decoder<YamlCodec, yaml::YamlError>>::peek_event(&mut decoder)
            .unwrap(),
        Some(Event::StartArray(None))
    );
    assert_eq!(
        <yaml::YamlReader as Decoder<YamlCodec, yaml::YamlError>>::next(&mut decoder).unwrap(),
        Some(Event::StartArray(None))
    );
    assert_eq!(
        <yaml::YamlReader as Decoder<YamlCodec, yaml::YamlError>>::next(&mut decoder).unwrap(),
        Some(Event::Int(1))
    );
}

#[test]
fn bounded_yaml_model_replay_is_deterministic_and_bounded() {
    for seed in 0..4_096_u64 {
        let input = seed.to_le_bytes();
        let first = run_yaml_fuzz_case(&input).unwrap();
        let second = run_yaml_fuzz_case(&input).unwrap();
        assert_eq!(first, second, "YAML replay diverged for seed {seed}");
        assert!(first.steps <= MAX_YAML_FUZZ_STEPS);
        assert_eq!(first.valid_cases, first.invalid_cases);
        assert!(first.max_nodes <= 128);
    }
    for input in [
        Vec::new(),
        b"yaml\0anchors\xff".to_vec(),
        (0..=255).collect::<Vec<_>>(),
    ] {
        let first = run_yaml_fuzz_case(&input).unwrap();
        let second = run_yaml_fuzz_case(&input).unwrap();
        assert_eq!(first, second);
        assert!(first.steps <= MAX_YAML_FUZZ_STEPS);
        assert!(input.len() <= MAX_YAML_FUZZ_INPUT_BYTES || first.steps == MAX_YAML_FUZZ_STEPS);
    }
    let oversized = vec![0_u8; MAX_YAML_FUZZ_INPUT_BYTES + 1];
    assert_eq!(
        run_yaml_fuzz_case(&oversized).unwrap().steps,
        MAX_YAML_FUZZ_STEPS
    );
}

#[test]
fn stream_limits_and_terminal_rejection_are_explicit() {
    let documents = yaml::parse_all(b"---\none\n---\ntwo\n").unwrap();
    assert_eq!(documents.len(), 2);
    let limited = YamlOptions::create(YamlLimits {
        max_documents: 1,
        ..YamlLimits::default()
    });
    assert_eq!(
        yaml::parse_all_with_options(b"---\none\n---\ntwo\n", limited)
            .unwrap_err()
            .kind,
        YamlErrorKind::DocumentLimit
    );
    let mut reader = yaml::YamlReader::from_bytes(b"one\n", YamlOptions::default()).unwrap();
    while reader.next().unwrap().is_some() {}
    assert_eq!(reader.finish().unwrap_err().kind, YamlErrorKind::Closed);
    let mut writer = yaml::YamlWriter::to_writer(YamlOptions::default()).unwrap();
    writer
        .write(YamlEvent::Scalar(yaml::YamlScalar::Null))
        .unwrap();
    assert_eq!(writer.finish().unwrap(), b"null\n");
    assert_eq!(writer.finish().unwrap_err().kind, YamlErrorKind::Closed);
}
