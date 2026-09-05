#![no_main]

// cargo-fuzz target: stdlib_encoding

use std::panic::{catch_unwind, AssertUnwindSafe};

use libfuzzer_sys::fuzz_target;
use tondo_reliability::encoding_model::{
    bounded_chunk_sizes, decode_reference, encode_reference, run_encoding_fuzz_case,
    EncodingFuzzSummary, ReferenceAlphabet, ReferenceCodec, ReferenceError, ReferenceErrorKind,
    ReferenceHexCase, ReferencePadding, MAX_ENCODING_FUZZ_INPUT_BYTES,
};
use tondo_stdlib::encoding::{
    Base64Alphabet, Base64Options, Base64Padding, EncodingError, EncodingErrorKind, EncodingLimits,
    HexCase, HexOptions,
};
use tondo_stdlib::serialization::Bytes;

const MAX_PAYLOAD_BYTES: usize = 96;

fn production_encode(codec: ReferenceCodec, input: &[u8]) -> Result<Vec<u8>, EncodingError> {
    let input = Bytes::from_slice(input);
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => Base64Options::create(
            production_alphabet(alphabet),
            production_padding(padding),
            EncodingLimits::default(),
        )
        .encode(&input)
        .map(Bytes::into_vec),
        ReferenceCodec::Hex { case } => {
            HexOptions::create(production_case(case), EncodingLimits::default())
                .encode(&input)
                .map(Bytes::into_vec)
        }
    }
}

fn production_decode(codec: ReferenceCodec, input: &[u8]) -> Result<Vec<u8>, EncodingError> {
    let input = Bytes::from_slice(input);
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => Base64Options::create(
            production_alphabet(alphabet),
            production_padding(padding),
            EncodingLimits::default(),
        )
        .decode(&input)
        .map(Bytes::into_vec),
        ReferenceCodec::Hex { case } => {
            HexOptions::create(production_case(case), EncodingLimits::default())
                .decode(&input)
                .map(Bytes::into_vec)
        }
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
                production_alphabet(alphabet),
                production_padding(padding),
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
            let options = HexOptions::create(production_case(case), EncodingLimits::default());
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
                production_alphabet(alphabet),
                production_padding(padding),
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
            let options = HexOptions::create(production_case(case), EncodingLimits::default());
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

fn production_alphabet(alphabet: ReferenceAlphabet) -> Base64Alphabet {
    match alphabet {
        ReferenceAlphabet::Standard => Base64Alphabet::Standard,
        ReferenceAlphabet::UrlSafe => Base64Alphabet::UrlSafe,
    }
}

fn production_padding(padding: ReferencePadding) -> Base64Padding {
    match padding {
        ReferencePadding::Required => Base64Padding::Required,
        ReferencePadding::Omitted => Base64Padding::Omitted,
    }
}

fn production_case(case: ReferenceHexCase) -> HexCase {
    match case {
        ReferenceHexCase::Lower => HexCase::Lower,
        ReferenceHexCase::Upper => HexCase::Upper,
        ReferenceHexCase::Any => HexCase::Any,
    }
}

fn reference_kind(kind: &EncodingErrorKind) -> ReferenceErrorKind {
    match kind {
        EncodingErrorKind::InvalidCharacter => ReferenceErrorKind::InvalidCharacter,
        EncodingErrorKind::InvalidLength => ReferenceErrorKind::InvalidLength,
        EncodingErrorKind::InvalidPadding => ReferenceErrorKind::InvalidPadding,
        EncodingErrorKind::NonCanonical => ReferenceErrorKind::NonCanonical,
        EncodingErrorKind::ResourceLimit => ReferenceErrorKind::ResourceLimit,
        other => panic!("stdlib.encoding fuzz returned unsupported error: {other:?}"),
    }
}

fn compare_result(
    expected: Result<Vec<u8>, ReferenceError>,
    actual: Result<Vec<u8>, EncodingError>,
) {
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => assert_eq!(actual, expected),
        (Err(expected), Err(actual)) => {
            assert_eq!(reference_kind(&actual.kind), expected.kind);
            assert_eq!(actual.offset, expected.offset);
        }
        (expected, actual) => {
            panic!("stdlib.encoding model/production mismatch: {expected:?} vs {actual:?}")
        }
    }
}

fn observe(input: &[u8]) -> EncodingFuzzSummary {
    let run = || {
        let summary = run_encoding_fuzz_case(input)
            .unwrap_or_else(|error| panic!("std.encoding model invariant failed: {error}"));
        let bounded = &input[..input.len().min(MAX_ENCODING_FUZZ_INPUT_BYTES)];
        let payload_len = bounded.len().min(MAX_PAYLOAD_BYTES);
        let payload = &bounded[..payload_len];
        let policies = [
            ReferenceCodec::Base64 {
                alphabet: ReferenceAlphabet::Standard,
                padding: ReferencePadding::Required,
            },
            ReferenceCodec::Base64 {
                alphabet: ReferenceAlphabet::UrlSafe,
                padding: ReferencePadding::Required,
            },
            ReferenceCodec::Base64 {
                alphabet: ReferenceAlphabet::UrlSafe,
                padding: ReferencePadding::Omitted,
            },
            ReferenceCodec::Hex {
                case: ReferenceHexCase::Lower,
            },
            ReferenceCodec::Hex {
                case: ReferenceHexCase::Upper,
            },
            ReferenceCodec::Hex {
                case: ReferenceHexCase::Any,
            },
        ];
        for codec in policies {
            let encoded = encode_reference(codec, payload)
                .unwrap_or_else(|error| panic!("reference encode failed: {error}"));
            compare_result(Ok(encoded.clone()), production_encode(codec, payload));
            compare_result(Ok(payload.to_vec()), production_decode(codec, &encoded));
            for split in bounded_chunk_sizes(encoded.len(), bounded) {
                assert_eq!(
                    production_stream_decode(codec, &encoded, split).unwrap(),
                    payload,
                    "decode chunk boundary diverged for {codec:?}"
                );
            }
            for split in bounded_chunk_sizes(payload.len(), bounded) {
                assert_eq!(
                    production_stream_encode(codec, payload, split).unwrap(),
                    encoded,
                    "encode chunk boundary diverged for {codec:?}"
                );
            }
            let mut malformed = encoded;
            if malformed.is_empty() {
                malformed.push(b'!');
            } else {
                malformed[0] = b'!';
            }
            compare_result(
                decode_reference(codec, &malformed),
                production_decode(codec, &malformed),
            );
        }
        summary
    };
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.encoding model or production comparison panicked"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "std.encoding replay diverged");
    assert!(
        first.steps <= 512,
        "std.encoding replay exceeded step bound"
    );
    assert_eq!(first.valid_cases, first.invalid_cases);
});
