#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_stdlib::cbor::{
    self, CborDecodeOptions, CborEncodeOptions, CborEntry, CborFloat16, CborLimits, CborReader,
    CborTag, CborValue,
};

// Include the independent oracle without linking the reliability CLI,
// compiler, conformance runner or VM into this kernel-only harness.
#[path = "../../crates/tondo-reliability/src/cbor_model.rs"]
mod cbor_model;

use cbor_model::{
    MAX_CBOR_FUZZ_INPUT_BYTES, MAX_CBOR_FUZZ_STEPS, ReferenceErrorKind, ReferenceValue,
    parse_reference, render_deterministic, render_ordinary, run_cbor_fuzz_case, seed_at_step,
    value_from_seed,
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

fuzz_target!(|input: &[u8]| {
    let first = run_cbor_fuzz_case(input).expect("std.cbor independent replay failed");
    assert_eq!(
        first,
        run_cbor_fuzz_case(input).unwrap(),
        "std.cbor replay diverged"
    );
    assert!(first.steps <= MAX_CBOR_FUZZ_STEPS);
    assert_eq!(first.valid_cases, first.invalid_cases);
    for step in 0..first.steps {
        let reference = value_from_seed(seed_at_step(input, step));
        let ordinary = render_ordinary(&reference).unwrap();
        let value = to_kernel(&reference);
        assert_eq!(
            cbor::encode(&value, CborEncodeOptions::default()).unwrap(),
            ordinary
        );
        assert_eq!(
            cbor::parse(&ordinary, CborDecodeOptions::default()).unwrap(),
            value
        );
        assert_eq!(
            cbor::encode_deterministic(&value, CborLimits::default()).unwrap(),
            render_deterministic(&reference).unwrap()
        );
        let mut trailing = ordinary;
        trailing.push(0);
        assert!(cbor::parse(&trailing, CborDecodeOptions::default()).is_err());
    }
    // Compare arbitrary wire input as well as generated valid values. A model
    // Limit means outside its deliberately smaller domain, not malformed CBOR.
    let bounded = &input[..input.len().min(MAX_CBOR_FUZZ_INPUT_BYTES)];
    match parse_reference(bounded) {
        Ok(reference) => {
            assert_eq!(
                cbor::parse(bounded, CborDecodeOptions::default()).unwrap(),
                to_kernel(&reference)
            );
            CborReader::from_bytes(bounded, CborDecodeOptions::default()).unwrap();
        }
        Err(error) if error.kind == ReferenceErrorKind::Limit => {}
        Err(_) => {
            assert!(cbor::parse(bounded, CborDecodeOptions::default()).is_err());
            assert!(CborReader::from_bytes(bounded, CborDecodeOptions::default()).is_err());
        }
    }
});
