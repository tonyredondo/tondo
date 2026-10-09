//! Kernel comparisons shared by fixed regressions and bounded owner fuzzing.

use crate::log_model::{self, queue, values as reference};
use reference::{Fields, Format, Limits, Value};
use tondo_stdlib::log as kernel;

pub fn level(index: usize) -> kernel::LogLevel {
    [
        kernel::LogLevel::Trace,
        kernel::LogLevel::Debug,
        kernel::LogLevel::Info,
        kernel::LogLevel::Warn,
        kernel::LogLevel::Error,
    ][index]
}
pub fn fields(values: &Fields) -> kernel::Fields {
    let mut fields = kernel::Fields::empty();
    for (key, value) in values.entries() {
        fields.put(key.clone(), convert(value)).unwrap();
    }
    fields
}
pub fn convert(value: &Value) -> kernel::LogValue {
    match value {
        Value::Null => kernel::LogValue::Null,
        Value::Bool(value) => kernel::LogValue::Bool(*value),
        Value::Int(value) => kernel::LogValue::Int(*value),
        Value::UInt(value) => kernel::LogValue::UInt(*value),
        Value::Float(value) => kernel::LogValue::Float(*value),
        Value::Text(value) => kernel::LogValue::Text(value.as_str().into()),
        Value::Bytes(value) => kernel::LogValue::Bytes(value.as_slice().into()),
        Value::Array(values) => kernel::LogValue::Array(values.iter().map(convert).collect()),
        Value::Object(value) => kernel::LogValue::Object(fields(value)),
        Value::Redacted => kernel::LogValue::Redacted,
    }
}
pub fn observed<T, E: std::fmt::Debug>(value: Result<T, E>) -> Result<T, String> {
    value.map_err(|error| format!("{error:?}"))
}
use reference::case_from_seed as seed_event;

pub fn compare_seed(seed: u64) {
    let reference = seed_event(seed);
    let actual = kernel::LogEvent::create(
        level(reference.level),
        reference.target.clone(),
        reference.message.clone(),
        fields(&reference.fields),
        None,
    )
    .unwrap();
    for minimum in 0..5 {
        assert_eq!(
            kernel::LoggerOptions::create(level(minimum)).enabled(actual.level()),
            reference.enabled(minimum),
        );
    }
    for format in [Format::Text, Format::JsonLines] {
        let actual_format = if format == Format::Text {
            kernel::LogFormat::Text
        } else {
            kernel::LogFormat::JsonLines
        };
        for limits in [
            Limits::default(),
            Limits {
                event: 128,
                ..Limits::default()
            },
            Limits {
                fields: 1,
                ..Limits::default()
            },
            Limits {
                depth: 1,
                ..Limits::default()
            },
            Limits {
                string: 4,
                ..Limits::default()
            },
            Limits {
                key: 1,
                ..Limits::default()
            },
        ] {
            let actual_limits = to_limits(limits);
            assert_eq!(
                observed(kernel::format_record(&actual, actual_format, actual_limits)),
                observed(reference::format_record(&reference, format, limits))
            );
        }
    }
}

pub fn run_case(input: &[u8]) {
    compare_zero_limit_dimensions();
    let input = &input[..input.len().min(log_model::MAX_FUZZ_INPUT_BYTES)];
    let steps = input.len().div_ceil(8).clamp(1, log_model::MAX_FUZZ_STEPS);
    let seed = input.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let replay = queue::replay(seed, steps).unwrap();
    assert_eq!(replay, queue::replay(seed, steps).unwrap());
    assert_eq!(replay.terminal_owners, 0);
    assert_eq!(replay.terminal_bytes, 0);
    assert_eq!(replay.close_count, 1);
    for step in 0..steps {
        compare_seed(seed ^ (step as u64).wrapping_mul(0x9e3779b97f4a7c15));
    }
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut expected = Fields::default();
        let mut actual = kernel::Fields::empty();
        assert_eq!(
            observed(actual.put("number".into(), kernel::LogValue::Float(number))),
            observed(expected.put("number", Value::Float(number)))
        );
        assert_eq!(actual.count(), 0);
        assert!(expected.entries().is_empty());
    }
}

/// Each zero dimension must refuse construction in both independent and kernel routes.
pub fn compare_zero_limit_dimensions() {
    for dimension in 0..6 {
        let mut limits = Limits::default();
        match dimension {
            0 => limits.event = 0,
            1 => limits.fields = 0,
            2 => limits.depth = 0,
            3 => limits.key = 0,
            4 => limits.string = 0,
            _ => limits.queue = 0,
        }
        assert_eq!(
            observed(
                kernel::LogLimits::create(
                    limits.event,
                    limits.fields,
                    limits.depth,
                    limits.key,
                    limits.string,
                    limits.queue,
                )
                .map(|_| ())
            ),
            observed(limits.validate().map(|_| ())),
        );
    }
}

pub fn to_limits(value: Limits) -> kernel::LogLimits {
    kernel::LogLimits {
        max_event_bytes: value.event,
        max_fields: value.fields,
        max_depth: value.depth,
        max_field_key_bytes: value.key,
        max_string_bytes: value.string,
        max_queue_entries: value.queue,
    }
}
