use tondo_reliability::encoding_model::{
    MAX_ENCODING_FUZZ_INPUT_BYTES, MAX_ENCODING_FUZZ_STEPS, ReferenceAlphabet, ReferenceCodec,
    ReferenceErrorKind, ReferenceHexCase, ReferencePadding, bounded_chunk_sizes, decode_reference,
    decode_reference_with_limits, encode_reference, encode_reference_with_limits,
    run_encoding_fuzz_case,
};
use tondo_stdlib::encoding::{
    Base64Alphabet, Base64Options, Base64Padding, EncodingError, EncodingErrorKind, EncodingLimits,
    HexCase, HexOptions,
};
use tondo_stdlib::serialization::Bytes;

fn base64(alphabet: ReferenceAlphabet, padding: ReferencePadding) -> ReferenceCodec {
    ReferenceCodec::Base64 { alphabet, padding }
}

fn hex(case: ReferenceHexCase) -> ReferenceCodec {
    ReferenceCodec::Hex { case }
}

fn production_encode(
    codec: ReferenceCodec,
    input: &[u8],
    limits: EncodingLimits,
) -> Result<Vec<u8>, EncodingError> {
    let input = Bytes::from_slice(input);
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => Base64Options::create(
            match alphabet {
                ReferenceAlphabet::Standard => Base64Alphabet::Standard,
                ReferenceAlphabet::UrlSafe => Base64Alphabet::UrlSafe,
            },
            match padding {
                ReferencePadding::Required => Base64Padding::Required,
                ReferencePadding::Omitted => Base64Padding::Omitted,
            },
            limits,
        )
        .encode(&input)
        .map(Bytes::into_vec),
        ReferenceCodec::Hex { case } => HexOptions::create(
            match case {
                ReferenceHexCase::Lower => HexCase::Lower,
                ReferenceHexCase::Upper => HexCase::Upper,
                ReferenceHexCase::Any => HexCase::Any,
            },
            limits,
        )
        .encode(&input)
        .map(Bytes::into_vec),
    }
}

fn production_decode(
    codec: ReferenceCodec,
    input: &[u8],
    limits: EncodingLimits,
) -> Result<Vec<u8>, EncodingError> {
    let input = Bytes::from_slice(input);
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => Base64Options::create(
            match alphabet {
                ReferenceAlphabet::Standard => Base64Alphabet::Standard,
                ReferenceAlphabet::UrlSafe => Base64Alphabet::UrlSafe,
            },
            match padding {
                ReferencePadding::Required => Base64Padding::Required,
                ReferencePadding::Omitted => Base64Padding::Omitted,
            },
            limits,
        )
        .decode(&input)
        .map(Bytes::into_vec),
        ReferenceCodec::Hex { case } => HexOptions::create(
            match case {
                ReferenceHexCase::Lower => HexCase::Lower,
                ReferenceHexCase::Upper => HexCase::Upper,
                ReferenceHexCase::Any => HexCase::Any,
            },
            limits,
        )
        .decode(&input)
        .map(Bytes::into_vec),
    }
}

fn production_stream_encode(
    codec: ReferenceCodec,
    input: &[u8],
    chunk_size: usize,
) -> Result<Vec<u8>, EncodingError> {
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => {
            let options = Base64Options::create(
                match alphabet {
                    ReferenceAlphabet::Standard => Base64Alphabet::Standard,
                    ReferenceAlphabet::UrlSafe => Base64Alphabet::UrlSafe,
                },
                match padding {
                    ReferencePadding::Required => Base64Padding::Required,
                    ReferencePadding::Omitted => Base64Padding::Omitted,
                },
                EncodingLimits::default(),
            );
            let mut stream = options.encoder()?;
            let mut output = stream.push(&Bytes::default())?.into_vec();
            for chunk in input.chunks(chunk_size.max(1)) {
                output.extend_from_slice(stream.push(&Bytes::from_slice(chunk))?.as_slice());
            }
            output.extend_from_slice(stream.finish()?.as_slice());
            Ok(output)
        }
        ReferenceCodec::Hex { case } => {
            let options = HexOptions::create(
                match case {
                    ReferenceHexCase::Lower => HexCase::Lower,
                    ReferenceHexCase::Upper => HexCase::Upper,
                    ReferenceHexCase::Any => HexCase::Any,
                },
                EncodingLimits::default(),
            );
            let mut stream = options.encoder()?;
            let mut output = stream.push(&Bytes::default())?.into_vec();
            for chunk in input.chunks(chunk_size.max(1)) {
                output.extend_from_slice(stream.push(&Bytes::from_slice(chunk))?.as_slice());
            }
            output.extend_from_slice(stream.finish()?.as_slice());
            Ok(output)
        }
    }
}

fn production_stream_decode(
    codec: ReferenceCodec,
    input: &[u8],
    chunk_size: usize,
) -> Result<Vec<u8>, EncodingError> {
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => {
            let options = Base64Options::create(
                match alphabet {
                    ReferenceAlphabet::Standard => Base64Alphabet::Standard,
                    ReferenceAlphabet::UrlSafe => Base64Alphabet::UrlSafe,
                },
                match padding {
                    ReferencePadding::Required => Base64Padding::Required,
                    ReferencePadding::Omitted => Base64Padding::Omitted,
                },
                EncodingLimits::default(),
            );
            let mut stream = options.decoder()?;
            let mut output = stream.push(&Bytes::default())?.into_vec();
            for chunk in input.chunks(chunk_size.max(1)) {
                output.extend_from_slice(stream.push(&Bytes::from_slice(chunk))?.as_slice());
            }
            output.extend_from_slice(stream.finish()?.as_slice());
            Ok(output)
        }
        ReferenceCodec::Hex { case } => {
            let options = HexOptions::create(
                match case {
                    ReferenceHexCase::Lower => HexCase::Lower,
                    ReferenceHexCase::Upper => HexCase::Upper,
                    ReferenceHexCase::Any => HexCase::Any,
                },
                EncodingLimits::default(),
            );
            let mut stream = options.decoder()?;
            let mut output = stream.push(&Bytes::default())?.into_vec();
            for chunk in input.chunks(chunk_size.max(1)) {
                output.extend_from_slice(stream.push(&Bytes::from_slice(chunk))?.as_slice());
            }
            output.extend_from_slice(stream.finish()?.as_slice());
            Ok(output)
        }
    }
}

fn expected_error_kind(kind: &EncodingErrorKind) -> ReferenceErrorKind {
    match kind {
        EncodingErrorKind::InvalidCharacter => ReferenceErrorKind::InvalidCharacter,
        EncodingErrorKind::InvalidLength => ReferenceErrorKind::InvalidLength,
        EncodingErrorKind::InvalidPadding => ReferenceErrorKind::InvalidPadding,
        EncodingErrorKind::NonCanonical => ReferenceErrorKind::NonCanonical,
        EncodingErrorKind::ResourceLimit => ReferenceErrorKind::ResourceLimit,
        other => panic!("unexpected non-wire error in model comparison: {other:?}"),
    }
}

fn assert_same_result(
    expected: Result<Vec<u8>, tondo_reliability::encoding_model::ReferenceError>,
    actual: Result<Vec<u8>, EncodingError>,
) {
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => assert_eq!(actual, expected),
        (Err(expected), Err(actual)) => {
            assert_eq!(expected_error_kind(&actual.kind), expected.kind);
            assert_eq!(actual.offset, expected.offset);
        }
        (expected, actual) => {
            panic!("reference/production result diverged: {expected:?} vs {actual:?}")
        }
    }
}

#[test]
fn official_vectors_match_the_independent_model_at_every_chunk_boundary() {
    let policies = [
        base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
        base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Required),
        base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Omitted),
        hex(ReferenceHexCase::Lower),
        hex(ReferenceHexCase::Upper),
        hex(ReferenceHexCase::Any),
    ];
    let vectors: &[&[u8]] = &[
        b"",
        b"f",
        b"fo",
        b"foo",
        b"foob",
        b"fooba",
        b"foobar",
        b"\x00\x01\x02\xfb\xfe\xff",
    ];
    for codec in policies {
        for input in vectors {
            let expected = encode_reference(codec, input).unwrap();
            assert_same_result(
                Ok(expected.clone()),
                production_encode(codec, input, EncodingLimits::default()),
            );
            for split in 1..=input.len().max(1) {
                assert_eq!(
                    production_stream_encode(codec, input, split).unwrap(),
                    expected,
                    "encode split {split} for {codec:?}"
                );
            }
            for split in 1..=expected.len().max(1) {
                assert_eq!(
                    production_stream_decode(codec, &expected, split).unwrap(),
                    *input,
                    "decode split {split} for {codec:?}"
                );
            }
        }
    }
}

#[test]
fn invalid_padding_alphabet_case_and_length_errors_are_byte_exact() {
    let cases = [
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Zg".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Zg=".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Zh==".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Zg==x".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Zg==\n".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"-_8=".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Z===".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::Standard, ReferencePadding::Required),
            b"Z!".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Omitted),
            b"Zg==".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Omitted),
            b"Z".as_slice(),
        ),
        (
            base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Omitted),
            b"Zg=".as_slice(),
        ),
        (hex(ReferenceHexCase::Lower), b"00AB".as_slice()),
        (hex(ReferenceHexCase::Upper), b"00ab".as_slice()),
        (hex(ReferenceHexCase::Any), b"0x".as_slice()),
        (hex(ReferenceHexCase::Any), b"f".as_slice()),
        (hex(ReferenceHexCase::Lower), b"00-1".as_slice()),
    ];
    for (codec, input) in cases {
        let expected = decode_reference(codec, input);
        assert_same_result(
            expected,
            production_decode(codec, input, EncodingLimits::default()),
        );
    }
}

#[test]
fn limits_are_checked_before_publication_and_errors_close_the_handle() {
    let base64_codec = base64(ReferenceAlphabet::Standard, ReferencePadding::Required);
    let input_limit = EncodingLimits::create(2, 64).unwrap();
    assert_same_result(
        encode_reference_with_limits(base64_codec, b"abc", 2, 64),
        production_encode(base64_codec, b"abc", input_limit),
    );
    let output_limit = EncodingLimits::create(64, 3).unwrap();
    assert_same_result(
        encode_reference_with_limits(base64_codec, b"abc", 64, 3),
        production_encode(base64_codec, b"abc", output_limit),
    );
    let mut encoder = Base64Options::standard(input_limit).encoder().unwrap();
    assert_eq!(
        encoder.push(&Bytes::from_slice(b"abc")).unwrap_err(),
        EncodingError {
            kind: EncodingErrorKind::ResourceLimit,
            offset: 0,
        }
    );
    assert_eq!(
        encoder.push(&Bytes::default()).unwrap_err().kind,
        EncodingErrorKind::Closed
    );

    let decoder_limits = EncodingLimits::create(64, 0).unwrap();
    let decode_codec = base64_codec;
    assert_same_result(
        decode_reference_with_limits(decode_codec, b"Zg==", 64, 0),
        production_decode(decode_codec, b"Zg==", decoder_limits),
    );
    let mut decoder = Base64Options::standard(decoder_limits).decoder().unwrap();
    assert_eq!(
        decoder.push(&Bytes::from_slice(b"Zg==")).unwrap_err().kind,
        EncodingErrorKind::ResourceLimit
    );
    assert_eq!(
        decoder.push(&Bytes::default()).unwrap_err().kind,
        EncodingErrorKind::Closed
    );

    let hex_codec = hex(ReferenceHexCase::Lower);
    let hex_limits = EncodingLimits::create(64, 1).unwrap();
    assert_same_result(
        encode_reference_with_limits(hex_codec, b"x", 64, 1),
        production_encode(hex_codec, b"x", hex_limits),
    );
    let mut hex_encoder = HexOptions::lower(hex_limits).encoder().unwrap();
    assert_eq!(
        hex_encoder.push(&Bytes::from_slice(b"x")).unwrap_err().kind,
        EncodingErrorKind::ResourceLimit
    );
    assert_eq!(
        hex_encoder.finish().unwrap_err().kind,
        EncodingErrorKind::Closed
    );
}

#[test]
fn finish_and_empty_chunks_preserve_lifecycle_and_chunk_invariance() {
    let options = Base64Options::url_safe_unpadded(EncodingLimits::default());
    let mut decoder = options.decoder().unwrap();
    assert!(
        decoder
            .push(&Bytes::default())
            .unwrap()
            .as_slice()
            .is_empty()
    );
    decoder.push(&Bytes::from_slice(b"Zg")).unwrap();
    assert_eq!(decoder.finish().unwrap().as_slice(), b"f");
    assert_eq!(
        decoder.finish().unwrap_err().kind,
        EncodingErrorKind::Closed
    );

    let required = Base64Options::standard(EncodingLimits::default());
    let mut incomplete = required.decoder().unwrap();
    incomplete.push(&Bytes::from_slice(b"Zg")).unwrap();
    assert_eq!(
        incomplete.finish().unwrap_err(),
        EncodingError {
            kind: EncodingErrorKind::InvalidLength,
            offset: 0,
        }
    );
    assert_eq!(
        incomplete.push(&Bytes::default()).unwrap_err().kind,
        EncodingErrorKind::Closed
    );

    let payload = b"chunk boundaries are semantic no-ops";
    let codec = base64(ReferenceAlphabet::UrlSafe, ReferencePadding::Omitted);
    let expected = encode_reference(codec, payload).unwrap();
    for split in bounded_chunk_sizes(payload.len(), b"encoding") {
        assert_eq!(
            production_stream_encode(codec, payload, split).unwrap(),
            expected
        );
    }
}

#[test]
fn bounded_model_replay_is_deterministic_and_has_explicit_limits() {
    for seed in 0..4_096_u64 {
        let input = seed.to_le_bytes();
        let first = run_encoding_fuzz_case(&input).unwrap();
        let second = run_encoding_fuzz_case(&input).unwrap();
        assert_eq!(first, second, "encoding replay diverged for seed {seed}");
        assert!(first.steps <= MAX_ENCODING_FUZZ_STEPS);
        assert_eq!(first.valid_cases, first.invalid_cases);
        assert!(first.max_payload_bytes <= 96);
    }
    for input in [
        Vec::new(),
        b"rfc4648\0invalid\xff".to_vec(),
        (0..=255).collect::<Vec<_>>(),
    ] {
        let first = run_encoding_fuzz_case(&input).unwrap();
        let second = run_encoding_fuzz_case(&input).unwrap();
        assert_eq!(first, second);
        assert!(first.steps <= MAX_ENCODING_FUZZ_STEPS);
        assert!(
            input.len() <= MAX_ENCODING_FUZZ_INPUT_BYTES || first.steps == MAX_ENCODING_FUZZ_STEPS
        );
        assert_eq!(first.valid_cases, first.invalid_cases);
        assert!(first.max_payload_bytes <= 96);
    }
    let oversized = vec![0_u8; MAX_ENCODING_FUZZ_INPUT_BYTES + 1];
    let summary = run_encoding_fuzz_case(&oversized).unwrap();
    assert_eq!(summary.steps, MAX_ENCODING_FUZZ_STEPS);
}
