use tondo_reliability::log_fuzz::{self, convert, fields, observed};
use tondo_reliability::log_model::values::{Fields, Value};
use tondo_stdlib::log as kernel;

#[test]
fn generated_formats_and_tight_limits_match_independent_reference() {
    for seed in 0..4096 {
        log_fuzz::compare_seed(seed ^ 0x9e3779b97f4a7c15);
    }
}

#[test]
fn construction_errors_and_atomic_fields_match_reference() {
    let mut expected = Fields::default();
    let mut actual = kernel::Fields::empty();
    for (key, value) in [
        ("x", Value::Int(1)),
        ("x", Value::Float(f64::NAN)),
        ("", Value::Null),
        ("\0", Value::Text("value".into())),
        ("nan", Value::Float(f64::NAN)),
        ("inf", Value::Float(f64::INFINITY)),
        ("nested-nan", Value::Array(vec![Value::Float(f64::NAN)])),
        ("valid", Value::Redacted),
    ] {
        let before = actual.clone();
        let want = expected.put(key, value.clone());
        let got = actual.put(key.to_owned(), convert(&value));
        assert_eq!(observed(got), observed(want), "{key:?}");
        if got.is_err() {
            assert_eq!(actual, before);
        }
        assert_eq!(actual, fields(&expected));
    }
    for depth in [15, 16, 17] {
        let candidate = (0..depth).fold(Value::Null, |inner, _| Value::Array(vec![inner]));
        let mut expected = Fields::default();
        let mut actual = kernel::Fields::empty();
        assert_eq!(
            observed(actual.put("depth".into(), convert(&candidate))),
            observed(expected.put("depth", candidate)),
            "default depth boundary {depth}",
        );
        assert_eq!(actual, fields(&expected));
    }
    log_fuzz::compare_zero_limit_dimensions();
}
