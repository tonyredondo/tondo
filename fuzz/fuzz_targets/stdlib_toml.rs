#![no_main]

// stdlib_toml is the bounded reference-model/hosted replay target.

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;
use tondo_reliability::toml_model::{
    MAX_REFERENCE_NODES, MAX_TOML_FUZZ_INPUT_BYTES, MAX_TOML_FUZZ_STEPS, ReferenceValue,
    TomlFuzzSummary, render_canonical, run_toml_fuzz_case, value_from_seed,
};
use tondo_stdlib::toml::{self, TomlLimits, TomlMember, TomlOptions, TomlValue};

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

fn observe(input: &[u8]) -> TomlFuzzSummary {
    let run = || {
        let summary = run_toml_fuzz_case(input)
            .unwrap_or_else(|error| panic!("std.toml model invariant failed: {error}"));
        let bounded = &input[..input.len().min(MAX_TOML_FUZZ_INPUT_BYTES)];
        let reference = value_from_seed(bounded);
        let expected = render_canonical(&reference)
            .unwrap_or_else(|error| panic!("std.toml reference render failed: {error}"));
        let actual = toml::encode_canonical(&to_toml(&reference), TomlLimits::default())
            .unwrap_or_else(|error| panic!("std.toml production render failed: {error}"));
        assert_eq!(actual, expected, "std.toml model/production rendering diverged");
        let parsed =
            toml::parse(&actual, TomlOptions::default()).expect("canonical TOML must parse");
        let reparsed = toml::encode_canonical(&parsed, TomlLimits::default())
            .expect("parsed canonical TOML must re-encode");
        assert_eq!(reparsed, actual, "std.toml canonical replay diverged");
        toml::validate(&actual, TomlOptions::default()).expect("canonical TOML must validate");
        summary
    };
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.toml model or production comparison panicked"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "std.toml replay diverged");
    assert!(
        first.steps <= MAX_TOML_FUZZ_STEPS,
        "std.toml replay exceeded step bound"
    );
    assert!(first.max_nodes <= MAX_REFERENCE_NODES);
    assert_eq!(first.valid_cases, first.invalid_cases);
});
