use std::collections::BTreeMap;
use std::io::{self, Read, Write};

use tondo_reliability::cbor_model::{
    self as model, MAX_CBOR_FUZZ_INPUT_BYTES, MAX_CBOR_FUZZ_STEPS, MAX_REFERENCE_DEPTH,
    MAX_REFERENCE_NODES, MAX_REFERENCE_SCALAR_BYTES, ReferenceErrorKind, ReferenceValue,
    half_value, parse_reference, render_deterministic, render_ordinary, run_cbor_fuzz_case,
    value_from_seed,
};
use tondo_stdlib::cbor::{
    self, CborDecodeOptions, CborDuplicatePolicy, CborEncodeOptions, CborEntry, CborErrorKind,
    CborEvent, CborFloat16, CborIndefinitePolicy, CborLimits, CborNonMinimalPolicy, CborPath,
    CborReader, CborTag, CborUnknownTagPolicy, CborValue, CborWriter,
};

fn to_kernel(value: &ReferenceValue) -> CborValue {
    match value {
        ReferenceValue::Null => CborValue::Null,
        ReferenceValue::Undefined => CborValue::Undefined,
        ReferenceValue::Bool(v) => CborValue::Bool(*v),
        ReferenceValue::Simple(v) => CborValue::Simple(*v),
        ReferenceValue::UInt(v) => CborValue::UInt(*v),
        ReferenceValue::Negative(v) => CborValue::Negative(*v),
        ReferenceValue::Float16(bits) => CborValue::Float16(CborFloat16 { bits: *bits }),
        ReferenceValue::Float32(bits) => CborValue::Float32(*bits),
        ReferenceValue::Float64(bits) => CborValue::Float64(*bits),
        ReferenceValue::Bytes(v) => CborValue::Bytes(v.clone()),
        ReferenceValue::Text(v) => CborValue::Text(v.clone()),
        ReferenceValue::Array(items) => CborValue::Array(items.iter().map(to_kernel).collect()),
        ReferenceValue::Map(pairs) => CborValue::Map(
            pairs
                .iter()
                .map(|(key, value)| CborEntry {
                    key: to_kernel(key),
                    value: to_kernel(value),
                })
                .collect(),
        ),
        ReferenceValue::Tag(number, value) => CborValue::Tag(CborTag {
            number: *number,
            value: Box::new(to_kernel(value)),
        }),
    }
}

fn hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/cbor-wire.json")).unwrap()
}

fn assert_kernel(value: &ReferenceValue) {
    let expected = render_ordinary(value).unwrap();
    let kernel = to_kernel(value);
    assert_eq!(
        cbor::parse(&expected, CborDecodeOptions::default()).unwrap(),
        kernel
    );
    assert_eq!(
        cbor::encode(&kernel, CborEncodeOptions::default()).unwrap(),
        expected
    );
    let deterministic = render_deterministic(value).unwrap();
    assert_eq!(
        cbor::encode_deterministic(&kernel, CborLimits::default()).unwrap(),
        deterministic
    );
    let parsed = parse_reference(&deterministic).unwrap();
    assert_eq!(render_deterministic(&parsed).unwrap(), deterministic);
}

#[test]
fn persistent_wire_corpus_matches_independent_model_and_all_value_routes() {
    let corpus = corpus();
    for fixture in corpus["valid"].as_array().unwrap() {
        let id = fixture["id"].as_str().unwrap();
        let wire = hex(fixture["wire"].as_str().unwrap());
        let ordinary = hex(fixture["ordinary"].as_str().unwrap());
        let reference = parse_reference(&wire).unwrap_or_else(|e| panic!("{id}: {e}"));
        let kernel = to_kernel(&reference);
        assert_eq!(render_ordinary(&reference).unwrap(), ordinary, "{id}");
        assert_eq!(
            cbor::parse(&wire, CborDecodeOptions::default()).unwrap(),
            kernel,
            "{id}"
        );
        assert_eq!(
            cbor::encode(&kernel, CborEncodeOptions::default()).unwrap(),
            ordinary,
            "{id}"
        );
        cbor::validate(&wire, CborDecodeOptions::default()).unwrap();
        let raw = cbor::raw(&wire, CborDecodeOptions::default()).unwrap();
        assert_eq!(raw.as_bytes(), wire, "{id}");
        let view = cbor::parse_view(&wire, CborDecodeOptions::default()).unwrap();
        assert_eq!(
            view.as_bytes().as_ptr(),
            wire.as_ptr(),
            "view must borrow: {id}"
        );
        assert_eq!(view.own().unwrap(), kernel, "{id}");
        if let Some(expected) = fixture["deterministic"].as_str() {
            let expected = hex(expected);
            assert_eq!(render_deterministic(&reference).unwrap(), expected, "{id}");
            assert_eq!(
                cbor::encode_deterministic(&kernel, CborLimits::default()).unwrap(),
                expected,
                "{id}"
            );
        } else {
            assert_eq!(fixture["deterministic_error"], "KeyCollision");
            assert_eq!(
                render_deterministic(&reference).unwrap_err().kind,
                ReferenceErrorKind::KeyCollision
            );
            assert_eq!(
                cbor::encode_deterministic(&kernel, CborLimits::default())
                    .unwrap_err()
                    .kind,
                CborErrorKind::DeterministicKeyCollision,
                "{id}"
            );
        }
        let mut reader = CborReader::from_bytes(&wire, CborDecodeOptions::default()).unwrap();
        let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
        while let Some(event) = reader.next().unwrap() {
            writer.write(event).unwrap();
        }
        let streamed = writer.finish().unwrap();
        assert_eq!(
            cbor::parse(&streamed, CborDecodeOptions::default()).unwrap(),
            kernel,
            "events: {id}"
        );
        reader.finish().unwrap();
        assert_eq!(reader.next().unwrap_err().kind, CborErrorKind::Closed);
    }
}

#[test]
fn invalid_wire_corpus_is_rejected_before_any_value_or_event_is_published() {
    for fixture in corpus()["invalid"].as_array().unwrap() {
        let id = fixture["id"].as_str().unwrap();
        let wire = hex(fixture["wire"].as_str().unwrap());
        assert!(parse_reference(&wire).is_err(), "reference accepted {id}");
        let error = cbor::parse(&wire, CborDecodeOptions::default()).unwrap_err();
        assert_eq!(
            format!("{:?}", error.kind),
            fixture["error"].as_str().unwrap(),
            "{id}"
        );
        assert!(
            error.start_offset <= error.end_offset && error.end_offset <= wire.len(),
            "span: {id}"
        );
        assert!(
            cbor::validate(&wire, CborDecodeOptions::default()).is_err(),
            "{id}"
        );
        assert!(
            cbor::raw(&wire, CborDecodeOptions::default()).is_err(),
            "{id}"
        );
        assert!(
            cbor::parse_view(&wire, CborDecodeOptions::default()).is_err(),
            "{id}"
        );
        assert!(
            CborReader::from_bytes(&wire, CborDecodeOptions::default()).is_err(),
            "{id}"
        );
    }
    let wire = hex("81a16178c061ff");
    let error = cbor::parse(&wire, CborDecodeOptions::default()).unwrap_err();
    assert_eq!(error.kind, CborErrorKind::InvalidUtf8);
    assert_eq!((error.start_offset, error.end_offset), (5, 7));
    assert_eq!(
        error.path,
        [
            CborPath::ArrayIndex(0),
            CborPath::MapEntry(0),
            CborPath::MapValue,
            CborPath::Tag
        ]
    );
    assert_eq!(error.to_string(), "CBOR InvalidUtf8 at bytes 5..7");
}

#[test]
fn deterministic_seed_campaign_compares_both_encoders_and_bounded_replay() {
    for seed in 0..4096_u64 {
        let input = seed.to_le_bytes();
        assert_kernel(&value_from_seed(&input));
        let first = run_cbor_fuzz_case(&input).unwrap();
        assert_eq!(first, run_cbor_fuzz_case(&input).unwrap(), "seed {seed}");
        assert_eq!(first.valid_cases, first.invalid_cases);
        assert!(first.steps <= MAX_CBOR_FUZZ_STEPS);
    }
    for input in [
        Vec::new(),
        (0..=255).collect(),
        vec![0xff; MAX_CBOR_FUZZ_INPUT_BYTES + 1],
    ] {
        let first = run_cbor_fuzz_case(&input).unwrap();
        assert_eq!(first, run_cbor_fuzz_case(&input).unwrap());
        assert!(first.steps <= MAX_CBOR_FUZZ_STEPS);
    }
}

#[test]
fn every_binary16_pattern_and_wider_float_boundary_matches_arithmetic_oracle() {
    for bits in 0..=u16::MAX {
        let reference = ReferenceValue::Float16(bits);
        assert_kernel(&reference);
        let numeric = half_value(bits);
        // The independent arithmetic interpretation checks narrowing from
        // wider wire representations, including signed zero and all NaNs.
        for widened in [
            ReferenceValue::Float32((numeric as f32).to_bits()),
            ReferenceValue::Float64(numeric.to_bits()),
        ] {
            assert_kernel(&widened);
            assert_eq!(
                render_deterministic(&widened).unwrap(),
                render_deterministic(&reference).unwrap()
            );
        }
    }
    for bits in [
        1,
        0x0010_0000_0000_0000,
        0x3e60_0000_0000_0000,
        0x3e60_0000_0000_0001,
        0x3ff0_0000_0000_0001,
        0x40ef_ffff_ffff_ffff,
        0x7fef_ffff_ffff_ffff,
        0xffef_ffff_ffff_ffff,
        0xfff0_0000_0000_0001,
    ] {
        assert_kernel(&ReferenceValue::Float64(bits));
    }
}

#[test]
fn reference_grammar_and_renderer_enforce_their_independent_bounds() {
    assert_eq!(
        model::ReferenceError {
            kind: ReferenceErrorKind::Limit,
            offset: 3
        }
        .to_string(),
        "Limit at byte 3"
    );
    for simple in 20..32 {
        assert_eq!(
            render_ordinary(&ReferenceValue::Simple(simple))
                .unwrap_err()
                .kind,
            ReferenceErrorKind::InvalidSimple
        );
    }
    for value in [
        ReferenceValue::Bytes(vec![0; MAX_REFERENCE_SCALAR_BYTES + 1]),
        ReferenceValue::Text("x".repeat(MAX_REFERENCE_SCALAR_BYTES + 1)),
        ReferenceValue::Array(vec![ReferenceValue::Null; MAX_REFERENCE_NODES]),
        ReferenceValue::Array(vec![ReferenceValue::Bytes(vec![0; 96]); 43]),
    ] {
        assert_eq!(
            render_ordinary(&value).unwrap_err().kind,
            ReferenceErrorKind::Limit
        );
    }
    let mut nested = ReferenceValue::Null;
    for _ in 0..MAX_REFERENCE_DEPTH {
        nested = ReferenceValue::Tag(0, Box::new(nested));
    }
    assert_eq!(
        render_deterministic(&nested).unwrap_err().kind,
        ReferenceErrorKind::Limit
    );
    for wire in [
        vec![0; MAX_CBOR_FUZZ_INPUT_BYTES + 1],
        hex("5897"),
        hex("9880"),
        [vec![0xc0; MAX_REFERENCE_DEPTH], vec![0xf6]].concat(),
        [vec![0x5f], vec![0x40; MAX_REFERENCE_NODES + 1], vec![0xff]].concat(),
        [vec![0x5f, 0x58, 0x60], vec![0; 96], vec![0x41, 0, 0xff]].concat(),
    ] {
        assert_eq!(
            parse_reference(&wire).unwrap_err().kind,
            ReferenceErrorKind::Limit
        );
    }
    assert_eq!(
        render_ordinary(&ReferenceValue::Bytes(vec![0; 96]))
            .unwrap()
            .len(),
        98
    );
    assert_eq!(
        parse_reference(&[0xf8, 0]).unwrap_err().kind,
        ReferenceErrorKind::InvalidSimple
    );
    assert_eq!(
        parse_reference(&[0x61, 0xff]).unwrap_err().kind,
        ReferenceErrorKind::InvalidUtf8
    );
    assert_eq!(
        parse_reference(&[0x1c]).unwrap_err().kind,
        ReferenceErrorKind::Malformed
    );
    assert_eq!(
        parse_reference(&[0x18]).unwrap_err().kind,
        ReferenceErrorKind::Truncated
    );
    assert_eq!(
        parse_reference(&[0, 0]).unwrap_err().kind,
        ReferenceErrorKind::TrailingData
    );
}

#[test]
fn policies_preserve_pair_position_and_distinguish_wire_and_typed_duplicates() {
    let wire = hex("a3010203040105");
    for (policy, expected) in [
        (CborDuplicatePolicy::First, vec![(1, 2), (3, 4)]),
        (CborDuplicatePolicy::Last, vec![(1, 5), (3, 4)]),
        (CborDuplicatePolicy::Preserve, vec![(1, 2), (3, 4), (1, 5)]),
    ] {
        let options = CborDecodeOptions {
            dynamic_map_duplicates: policy,
            ..CborDecodeOptions::default()
        };
        let expected = CborValue::Map(
            expected
                .into_iter()
                .map(|(key, value)| CborEntry {
                    key: CborValue::UInt(key),
                    value: CborValue::UInt(value),
                })
                .collect(),
        );
        assert_eq!(cbor::parse(&wire, options).unwrap(), expected);
        assert_eq!(cbor::raw(&wire, options).unwrap().as_bytes(), wire);
    }
    let reject = CborDecodeOptions {
        dynamic_map_duplicates: CborDuplicatePolicy::Reject,
        ..CborDecodeOptions::default()
    };
    assert_eq!(
        cbor::parse(&wire, reject).unwrap_err().kind,
        CborErrorKind::DuplicateKey
    );
    assert_eq!(
        cbor::decode_typed::<BTreeMap<u64, u64>>(&wire, CborDecodeOptions::default())
            .unwrap_err()
            .kind,
        CborErrorKind::DuplicateKey
    );
    for (policy, value) in [
        (CborDuplicatePolicy::First, 2),
        (CborDuplicatePolicy::Last, 5),
    ] {
        let options = CborDecodeOptions {
            typed_map_duplicates: policy,
            ..CborDecodeOptions::default()
        };
        assert_eq!(
            cbor::decode_typed::<BTreeMap<u64, u64>>(&wire, options).unwrap(),
            BTreeMap::from([(1, value), (3, 4)])
        );
    }
    for wire in [hex("1801"), hex("fa3fc00000"), hex("fb3ff8000000000000")] {
        let options = CborDecodeOptions {
            non_minimal: CborNonMinimalPolicy::Reject,
            ..CborDecodeOptions::default()
        };
        assert_eq!(
            cbor::parse(&wire, options).unwrap_err().kind,
            CborErrorKind::NonMinimalEncoding
        );
        assert_eq!(
            CborReader::from_bytes(&wire, options).unwrap_err().kind,
            CborErrorKind::NonMinimalEncoding
        );
    }
    let options = CborDecodeOptions {
        indefinite: CborIndefinitePolicy::Reject,
        ..CborDecodeOptions::default()
    };
    for wire in [hex("5fff"), hex("7fff"), hex("9fff"), hex("bfff")] {
        assert_eq!(
            cbor::parse(&wire, options).unwrap_err().kind,
            CborErrorKind::IndefiniteNotAllowed
        );
    }
    let options = CborDecodeOptions {
        unknown_tags: CborUnknownTagPolicy::Reject,
        ..CborDecodeOptions::default()
    };
    assert_eq!(
        cbor::parse(&hex("d903e7f6"), options).unwrap_err().kind,
        CborErrorKind::UnknownTag
    );
    let values = vec![0_u64, 24, u64::MAX];
    let encoded = cbor::encode_typed(&values, CborEncodeOptions::default()).unwrap();
    assert_eq!(
        cbor::decode_typed::<Vec<u64>>(&encoded, CborDecodeOptions::default()).unwrap(),
        values
    );
}

#[test]
fn every_limit_rejects_before_publication_and_valid_boundary_is_accepted() {
    let default = CborLimits::default();
    let cases = [
        (
            CborLimits {
                max_document_bytes: 1,
                ..default
            },
            hex("1818"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_depth: 1,
                ..default
            },
            hex("818100"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_array_items: 1,
                ..default
            },
            hex("820000"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_map_pairs: 1,
                ..default
            },
            hex("a200000100"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_string_bytes: 1,
                ..default
            },
            hex("62c3bc"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_byte_string_bytes: 1,
                ..default
            },
            hex("420000"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_chunks: 1,
                ..default
            },
            hex("5f4040ff"),
            CborErrorKind::TooManyChunks,
        ),
        (
            CborLimits {
                max_tags: 1,
                ..default
            },
            hex("c0c0f6"),
            CborErrorKind::TooManyTags,
        ),
        (
            CborLimits {
                max_simple_values: 1,
                ..default
            },
            hex("82e0e1"),
            CborErrorKind::LimitExceeded,
        ),
        (
            CborLimits {
                max_events: 1,
                ..default
            },
            hex("8100"),
            CborErrorKind::LimitExceeded,
        ),
    ];
    for (limits, wire, kind) in cases {
        let options = CborDecodeOptions {
            limits,
            ..CborDecodeOptions::default()
        };
        assert_eq!(
            cbor::parse(&wire, options).unwrap_err().kind,
            kind,
            "{limits:?}"
        );
        assert!(cbor::raw(&wire, options).is_err());
        assert!(cbor::parse_view(&wire, options).is_err());
        assert!(CborReader::from_bytes(&wire, options).is_err());
    }
    let limits = CborLimits {
        max_output_bytes: 1,
        ..default
    };
    assert_eq!(
        cbor::encode(
            &CborValue::UInt(24),
            CborEncodeOptions {
                limits,
                deterministic: false
            }
        )
        .unwrap_err()
        .kind,
        CborErrorKind::LimitExceeded
    );
    assert_eq!(
        cbor::encode_deterministic(&CborValue::UInt(24), limits)
            .unwrap_err()
            .kind,
        CborErrorKind::LimitExceeded
    );
    let limits = CborLimits {
        max_depth: 0,
        ..default
    };
    assert_eq!(
        CborLimits::create(limits).unwrap_err().kind,
        CborErrorKind::LimitExceeded
    );
    let limits = CborLimits {
        max_document_bytes: 1,
        max_array_items: 1,
        max_map_pairs: 1,
        max_string_bytes: 1,
        max_byte_string_bytes: 1,
        max_tags: 1,
        max_chunks: 1,
        max_simple_values: 1,
        max_events: 1,
        max_output_bytes: 1,
        ..default
    };
    let options = CborDecodeOptions {
        limits,
        ..CborDecodeOptions::default()
    };
    assert_eq!(cbor::parse(&[0], options).unwrap(), CborValue::UInt(0));
    assert_eq!(
        cbor::encode(
            &CborValue::UInt(0),
            CborEncodeOptions {
                limits,
                deterministic: false
            }
        )
        .unwrap(),
        [0]
    );
}

struct Fragmented<'a> {
    bytes: &'a [u8],
    chunk: usize,
}
impl Read for Fragmented<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.len().min(output.len()).min(self.chunk);
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[test]
fn fragmented_reader_and_writer_have_explicit_terminal_and_io_states() {
    let wire = hex("9f5f42010240ff7f6062c3bc60ffbf01f5ffff");
    let mut direct = CborReader::from_bytes(&wire, CborDecodeOptions::default()).unwrap();
    let mut expected = Vec::new();
    while let Some(event) = direct.next().unwrap() {
        expected.push(event);
    }
    assert_eq!(direct.next().unwrap_err().kind, CborErrorKind::Closed);
    for chunk in 1..=wire.len() {
        let mut reader = CborReader::from_reader(
            Fragmented {
                bytes: &wire,
                chunk,
            },
            CborDecodeOptions::default(),
        )
        .unwrap();
        let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
        for expected in &expected {
            let event = reader.next().unwrap().unwrap();
            assert_eq!(&event, expected);
            assert_eq!(reader.own(&event).unwrap(), event);
            writer.write(event).unwrap();
        }
        assert_eq!(reader.next().unwrap(), None);
        reader.finish().unwrap();
        assert_eq!(reader.finish().unwrap_err().kind, CborErrorKind::Closed);
        assert_eq!(
            reader.own(&CborEvent::Null).unwrap_err().kind,
            CborErrorKind::Closed
        );
        assert_eq!(writer.finish().unwrap(), wire);
        assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);
    }
    let mut early = CborReader::from_bytes(&wire, CborDecodeOptions::default()).unwrap();
    assert_eq!(
        early.finish().unwrap_err().kind,
        CborErrorKind::TrailingData
    );
    assert_eq!(early.next().unwrap_err().kind, CborErrorKind::Closed);
    struct FailingIo;
    impl Read for FailingIo {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture read failure"))
        }
    }
    impl Write for FailingIo {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture write failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        CborReader::from_reader(FailingIo, CborDecodeOptions::default())
            .unwrap_err()
            .kind,
        CborErrorKind::IoError
    );
    let mut failing = CborWriter::to_writer(FailingIo, CborEncodeOptions::default()).unwrap();
    for event in [
        CborEvent::StreamStart,
        CborEvent::Null,
        CborEvent::StreamEnd,
    ] {
        failing.write(event).unwrap();
    }
    assert_eq!(failing.finish().err().unwrap().kind, CborErrorKind::IoError);
    assert_eq!(
        failing.write(CborEvent::Null).unwrap_err().kind,
        CborErrorKind::Closed
    );
    let mut untouched = Vec::new();
    let mut invalid = CborWriter::to_writer(&mut untouched, CborEncodeOptions::default()).unwrap();
    for event in [
        CborEvent::StreamStart,
        CborEvent::StartArray(Some(2)),
        CborEvent::Null,
    ] {
        invalid.write(event).unwrap();
    }
    assert_eq!(
        invalid.write(CborEvent::EndArray).unwrap_err().kind,
        CborErrorKind::InvalidLength
    );
    assert_eq!(invalid.finish().unwrap_err().kind, CborErrorKind::Closed);
    drop(invalid);
    assert!(
        untouched.is_empty(),
        "invalid sequence must not reach the sink"
    );
}

#[test]
fn reserved_two_byte_simple_encodings_are_rejected_in_every_route() {
    // RFC 8949 section 3.3: f8 followed by any value below 32 is malformed,
    // including the unassigned simple values that have a one-byte encoding.
    for simple in 0..32 {
        let input = [0xf8, simple];
        for policy in [CborNonMinimalPolicy::Accept, CborNonMinimalPolicy::Reject] {
            let options = CborDecodeOptions {
                non_minimal: policy,
                ..CborDecodeOptions::default()
            };
            assert_eq!(
                cbor::parse(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue,
                "two-byte simple {simple}, policy {policy:?}"
            );
            assert_eq!(
                CborReader::from_bytes(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue
            );
            assert_eq!(
                cbor::validate(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue
            );
            assert_eq!(
                cbor::parse_view(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue
            );
            assert_eq!(
                cbor::raw(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue
            );
            assert_eq!(
                cbor::decode_typed::<u64>(&input, options).unwrap_err().kind,
                CborErrorKind::InvalidSimpleValue
            );
        }
    }
}
