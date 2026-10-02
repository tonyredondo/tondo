#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_stdlib::regex::{Regex, RegexLimits, RegexMatch, RegexOptions};

// The independent oracle is included directly: the minimal target links no
// compiler, VM, conformance runner or reliability CLI.
#[path = "../../crates/tondo-reliability/src/regex_model.rs"]
mod regex_model;

use regex_model::{ReferenceMatch, ReferenceOptions, ReferenceRegex, ReferenceSpan};

fn options(value: ReferenceOptions) -> RegexOptions {
    RegexOptions {
        case_insensitive: value.case_insensitive,
        multi_line: value.multi_line,
        dot_matches_newline: value.dot_matches_newline,
        crlf: value.crlf,
        ungreedy: value.ungreedy,
    }
}

fn observed(found: &RegexMatch, count: usize) -> ReferenceMatch {
    let span = |value: tondo_stdlib::regex::RegexSpan| ReferenceSpan {
        start: value.start,
        end: value.end,
    };
    ReferenceMatch {
        span: span(found.span),
        captures: (0..=count)
            .map(|index| found.capture(index).map(span))
            .collect(),
    }
}

fn compare_generated(seed: u64) {
    let case = regex_model::case_from_seed(seed);
    let reference = ReferenceRegex::compile(&case.pattern, case.options).unwrap();
    let kernel =
        Regex::compile(&case.pattern, options(case.options), RegexLimits::default()).unwrap();
    let count = reference.capture_count();
    assert_eq!(kernel.capture_count(), count);
    assert_eq!(
        kernel.capture_names().collect::<Vec<_>>(),
        reference
            .capture_names()
            .iter()
            .filter_map(|name| name.as_deref())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        kernel
            .find(&case.input)
            .unwrap()
            .as_ref()
            .map(|found| observed(found, count)),
        reference.find(&case.input).unwrap()
    );
    assert_eq!(
        kernel.is_full_match(&case.input).unwrap(),
        reference.full_match(&case.input).unwrap().is_some()
    );
    let actual = kernel
        .find_all(&case.input)
        .unwrap()
        .map(|found| observed(&found.unwrap(), count))
        .collect::<Vec<_>>();
    assert_eq!(actual, reference.find_all(&case.input).unwrap());
    assert_eq!(
        kernel.replace(&case.input, &case.replacement).unwrap(),
        reference
            .replace(&case.input, &case.replacement, false)
            .unwrap()
    );
    assert_eq!(
        kernel.replace_all(&case.input, &case.replacement).unwrap(),
        reference
            .replace(&case.input, &case.replacement, true)
            .unwrap()
    );
}

fuzz_target!(|bytes: &[u8]| {
    let bytes = &bytes[..bytes.len().min(regex_model::MAX_REGEX_FUZZ_INPUT_BYTES)];
    let first = regex_model::run_regex_fuzz_case(bytes).unwrap();
    assert_eq!(first, regex_model::run_regex_fuzz_case(bytes).unwrap());
    assert!(first.steps <= regex_model::MAX_REGEX_FUZZ_STEPS);
    for step in 0..first.steps {
        compare_generated(regex_model::seed_at_step(bytes, step));
    }

    // Retained vectors use one option byte followed by NUL-separated UTF-8
    // pattern, input and replacement. Arbitrary mutations exercise the same
    // bounded entry points; reference Limit/OutsideDomain is never rejection.
    let bits = bytes.first().copied().unwrap_or(0);
    let mut fields = bytes
        .get(1..)
        .unwrap_or_default()
        .splitn(3, |byte| *byte == 0);
    let pattern = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let input = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let template = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let reference_options = ReferenceOptions {
        case_insensitive: bits & 1 != 0,
        multi_line: bits & 2 != 0,
        dot_matches_newline: bits & 4 != 0,
        crlf: bits & 8 != 0,
        ungreedy: bits & 16 != 0,
    };
    let limits = RegexLimits {
        max_pattern_bytes: 4096,
        max_syntax_depth: 32,
        max_capture_groups: 16,
        max_class_ranges: 16_384,
        max_repeat: 8,
        max_program_states: 4096,
        max_input_bytes: 4096,
        max_steps: 100_000,
        max_matches: 512,
        max_output_bytes: 8192,
        max_replacement_bytes: 4096,
        vm_heap: 16_777_216,
    };
    let compiled = Regex::compile(&pattern, options(reference_options), limits);
    let repeated = Regex::compile(&pattern, options(reference_options), limits);
    match (compiled, repeated) {
        (Err(first), Err(second)) => assert_eq!(first, second),
        (Ok(kernel), Ok(second)) => {
            assert_eq!(kernel.fingerprint(), second.fingerprint());
            assert_eq!(kernel.find(&input), second.find(&input));
            assert_eq!(kernel.is_full_match(&input), second.is_full_match(&input));
            assert_eq!(
                kernel.replace(&input, &template),
                second.replace(&input, &template)
            );
            assert_eq!(
                kernel.replace_all(&input, &template),
                second.replace_all(&input, &template)
            );
            if let Ok(mut cursor) = kernel.find_all(&input) {
                let mut previous_end = 0;
                while let Some(item) = cursor.next() {
                    match item {
                        Ok(found) => {
                            assert!(
                                previous_end <= found.span.start
                                    && found.span.start <= found.span.end
                            );
                            assert!(found.span.end <= input.len());
                            previous_end = found.span.end;
                            for index in 0..=kernel.capture_count() {
                                if let Some(span) = found.capture(index) {
                                    assert!(
                                        found.span.start <= span.start
                                            && span.start <= span.end
                                            && span.end <= found.span.end
                                    );
                                    assert!(
                                        input.is_char_boundary(span.start)
                                            && input.is_char_boundary(span.end)
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            assert!(cursor.next().is_none());
                            break;
                        }
                    }
                }
                assert!(cursor.next().is_none());
            }
            match ReferenceRegex::compile(&pattern, reference_options) {
                Err(error) if error.kind == regex_model::ReferenceErrorKind::Syntax => {
                    panic!(
                        "kernel accepted an invalid pattern in the reference domain: {pattern:?}"
                    );
                }
                Ok(reference) => {
                    if let Ok(expected) = reference.find(&input)
                        && let Ok(actual) = kernel.find(&input)
                    {
                        assert_eq!(
                            actual
                                .as_ref()
                                .map(|found| observed(found, reference.capture_count())),
                            expected
                        );
                    }
                }
                Err(_) => {}
            }
        }
        _ => panic!("regex compile replay diverged"),
    }
});
