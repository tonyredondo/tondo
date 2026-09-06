#![no_main]

// stdlib_yaml is the bounded YAML model/production replay target.

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;
use tondo_reliability::yaml_model::{
    MAX_YAML_FUZZ_INPUT_BYTES, MAX_YAML_FUZZ_STEPS, ReferenceValue, YamlFuzzSummary,
    render_canonical, run_yaml_fuzz_case, value_from_seed,
};
use tondo_stdlib::yaml::{self, YamlLimits, YamlMember, YamlValue};

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
                .map(|(key, value)| YamlMember {
                    key: key.clone(),
                    value: to_yaml(value),
                })
                .collect(),
        ),
    }
}

fn observe(input: &[u8]) -> YamlFuzzSummary {
    let run = || {
        let summary = run_yaml_fuzz_case(input)
            .unwrap_or_else(|error| panic!("std.yaml model invariant failed: {error}"));
        let bounded = &input[..input.len().min(MAX_YAML_FUZZ_INPUT_BYTES)];
        let value = value_from_seed(bounded);
        let expected = render_canonical(&value)
            .unwrap_or_else(|error| panic!("std.yaml reference render failed: {error}"));
        let actual = yaml::encode_canonical(&to_yaml(&value), YamlLimits::default())
            .unwrap_or_else(|error| panic!("std.yaml production render failed: {error}"));
        assert_eq!(actual, expected, "std.yaml model/production rendering diverged");
        let parsed = yaml::parse(&actual).expect("canonical YAML must parse");
        let reparsed = yaml::encode_canonical(&parsed, YamlLimits::default())
            .expect("parsed canonical YAML must re-encode");
        assert_eq!(reparsed, actual, "std.yaml canonical replay diverged");
        summary
    };
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.yaml model or production comparison panicked"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "std.yaml replay diverged");
    assert!(first.steps <= MAX_YAML_FUZZ_STEPS, "std.yaml replay exceeded step bound");
    assert_eq!(first.valid_cases, first.invalid_cases);
});
