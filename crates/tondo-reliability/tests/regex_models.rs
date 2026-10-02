use tondo_reliability::regex_model::{
    self as model, ReferenceErrorKind, ReferenceMatch, ReferenceOptions, ReferenceRegex,
    ReferenceSpan,
};
use tondo_stdlib::regex::{Regex, RegexLimits, RegexMatch, RegexOptions};

fn kernel_options(options: ReferenceOptions) -> RegexOptions {
    RegexOptions {
        case_insensitive: options.case_insensitive,
        multi_line: options.multi_line,
        dot_matches_newline: options.dot_matches_newline,
        crlf: options.crlf,
        ungreedy: options.ungreedy,
    }
}

fn to_reference(found: &RegexMatch, count: usize) -> ReferenceMatch {
    let span = |span: tondo_stdlib::regex::RegexSpan| ReferenceSpan {
        start: span.start,
        end: span.end,
    };
    ReferenceMatch {
        span: span(found.span),
        captures: (0..=count)
            .map(|index| found.capture(index).map(span))
            .collect(),
    }
}

fn compare(case: &model::GeneratedCase) {
    let reference = ReferenceRegex::compile(&case.pattern, case.options)
        .unwrap_or_else(|error| panic!("reference compile: {case:?}: {error:?}"));
    let kernel = Regex::compile(
        &case.pattern,
        kernel_options(case.options),
        RegexLimits::default(),
    )
    .unwrap_or_else(|error| panic!("kernel compile: {case:?}: {error:?}"));
    let count = reference.capture_count();
    assert_eq!(kernel.capture_count(), count, "{case:?}");
    assert_eq!(
        kernel.capture_names().collect::<Vec<_>>(),
        reference
            .capture_names()
            .iter()
            .filter_map(|name| name.as_deref())
            .collect::<Vec<_>>(),
        "{case:?}",
    );
    let expected = reference
        .find(&case.input)
        .unwrap_or_else(|error| panic!("reference find: {case:?}: {error:?}"));
    assert_eq!(
        kernel
            .find(&case.input)
            .unwrap()
            .as_ref()
            .map(|found| to_reference(found, count)),
        expected,
        "find: {case:?}",
    );
    assert_eq!(
        kernel.is_match(&case.input).unwrap(),
        expected.is_some(),
        "boolean find: {case:?}",
    );
    assert_eq!(
        kernel.is_full_match(&case.input).unwrap(),
        reference.full_match(&case.input).unwrap().is_some(),
        "full match: {case:?}",
    );
    let expected = reference
        .find_all(&case.input)
        .unwrap_or_else(|error| panic!("reference all: {case:?}: {error:?}"));
    let actual = kernel
        .find_all(&case.input)
        .unwrap()
        .map(|found| to_reference(&found.unwrap(), count))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "all: {case:?}");
    for found in &expected {
        for span in found.captures.iter().flatten() {
            assert!(
                found.span.start <= span.start
                    && span.start <= span.end
                    && span.end <= found.span.end,
                "{case:?}"
            );
            assert!(
                case.input.is_char_boundary(span.start) && case.input.is_char_boundary(span.end)
            );
        }
    }
    assert_eq!(
        kernel.replace(&case.input, &case.replacement).unwrap(),
        reference
            .replace(&case.input, &case.replacement, false)
            .unwrap(),
        "replace: {case:?}",
    );
    assert_eq!(
        kernel.replace_all(&case.input, &case.replacement).unwrap(),
        reference
            .replace(&case.input, &case.replacement, true)
            .unwrap(),
        "replace all: {case:?}",
    );
}

#[test]
fn ordered_reference_has_independent_capture_and_progress_laws() {
    let options = ReferenceOptions::default();
    let first = ReferenceRegex::compile("a|ab", options).unwrap();
    assert_eq!(
        first.find("ab").unwrap().unwrap().span,
        ReferenceSpan { start: 0, end: 1 }
    );
    assert_eq!(
        first.full_match("ab").unwrap().unwrap().span,
        ReferenceSpan { start: 0, end: 2 }
    );
    let nested = ReferenceRegex::compile("((a?)*)", options).unwrap();
    let found = nested.find("aaa").unwrap().unwrap();
    assert_eq!(found.captures[2], Some(ReferenceSpan { start: 3, end: 3 }));
    let optional = ReferenceRegex::compile("(a)?()", options)
        .unwrap()
        .find("")
        .unwrap()
        .unwrap();
    assert_eq!(optional.captures[1], None);
    assert_eq!(
        optional.captures[2],
        Some(ReferenceSpan { start: 0, end: 0 })
    );
    let empty = ReferenceRegex::compile("", options).unwrap();
    assert_eq!(
        empty
            .find_all("é🙂")
            .unwrap()
            .iter()
            .map(|found| found.span.start)
            .collect::<Vec<_>>(),
        [0, 2, 6]
    );
    let named = ReferenceRegex::compile("(?<n>a)?", options).unwrap();
    assert_eq!(
        named.replace("b", &["$", "{n}"].concat(), false).unwrap(),
        "b"
    );
    assert_eq!(named.replace("b", "$01", false).unwrap(), "b");
    assert_eq!(
        named.replace("", "$2", true).unwrap_err().kind,
        ReferenceErrorKind::InvalidReplacement
    );
    for pattern in [
        r"\}",
        r"\]",
        r"[a\-c]",
        r"[\D]",
        r"[\S\w]",
        "(a|b){0,2}?",
        "(?:){2,}",
    ] {
        ReferenceRegex::compile(pattern, options).unwrap();
    }
    for pattern in [
        "a**",
        "(){0,0}",
        "[z-a]",
        "[a-b-c]",
        r"\q",
        r"\x{D800}",
        "(?<n>a)(?<n>b)",
        "]",
        "}",
    ] {
        assert_eq!(
            ReferenceRegex::compile(pattern, options).unwrap_err().kind,
            ReferenceErrorKind::Syntax,
            "{pattern}"
        );
    }
}

#[test]
fn generated_cases_compare_all_pure_operations_and_captures_for_4096_seeds() {
    for seed in 0..4096 {
        let first = model::case_from_seed(seed);
        assert_eq!(first, model::case_from_seed(seed), "seed {seed}");
        compare(&first);
    }
}

#[test]
fn reference_bounds_are_not_production_rejections() {
    let options = ReferenceOptions::default();
    for pattern in [
        "a".repeat(128),
        "()".repeat(9),
        "(?:".repeat(8) + "a" + &")".repeat(8),
        "a{9}".into(),
    ] {
        assert_eq!(
            ReferenceRegex::compile(&pattern, options).unwrap_err().kind,
            ReferenceErrorKind::Limit
        );
        Regex::compile(&pattern, RegexOptions::default(), RegexLimits::default()).unwrap();
    }
    assert_eq!(
        ReferenceRegex::compile(r"\p{L}", options).unwrap_err().kind,
        ReferenceErrorKind::OutsideDomain,
    );
    let word = ReferenceRegex::compile(r"\w", options).unwrap();
    assert_eq!(
        word.find("é").unwrap_err().kind,
        ReferenceErrorKind::OutsideDomain
    );
    assert!(
        Regex::compile(r"\w", RegexOptions::default(), RegexLimits::default())
            .unwrap()
            .is_match("é")
            .unwrap()
    );
    let ambiguous = ReferenceRegex::compile("(a|aa)*", options).unwrap();
    assert_eq!(
        ambiguous.find(&"a".repeat(32)).unwrap_err().kind,
        ReferenceErrorKind::Limit
    );
    assert!(
        Regex::compile("(a|aa)*", RegexOptions::default(), RegexLimits::default())
            .unwrap()
            .is_full_match(&"a".repeat(32))
            .unwrap()
    );
}

#[test]
fn closed_delimiters_and_middle_literal_hyphens_are_rejected() {
    use tondo_stdlib::regex::RegexErrorKind;
    let mut defects = Vec::new();
    for (pattern, expected) in [
        ("]", RegexErrorKind::InvalidSyntax),
        ("}", RegexErrorKind::InvalidSyntax),
        ("é]", RegexErrorKind::InvalidSyntax),
        ("[a-b-c]", RegexErrorKind::InvalidClass),
        ("[^a-b-c]", RegexErrorKind::InvalidClass),
        ("é[é-π-x]", RegexErrorKind::InvalidClass),
    ] {
        assert_eq!(
            ReferenceRegex::compile(pattern, ReferenceOptions::default())
                .unwrap_err()
                .kind,
            ReferenceErrorKind::Syntax,
        );
        match Regex::compile(pattern, RegexOptions::default(), RegexLimits::default()) {
            Ok(_) => defects.push(format!("{pattern:?}: accepted")),
            Err(error) if error.kind != expected => {
                defects.push(format!("{pattern:?}: {:?}", error.kind))
            }
            Err(error) => {
                if expected == RegexErrorKind::InvalidClass {
                    let start = pattern.rfind('-').unwrap();
                    assert_eq!(error.offset, start);
                    assert_eq!(
                        error.span.unwrap(),
                        tondo_stdlib::regex::RegexSpan {
                            start,
                            end: start + 1
                        }
                    );
                }
            }
        }
    }
    assert!(defects.is_empty(), "closed grammar violations: {defects:?}");
    for pattern in [
        "[-ab]",
        "[ab-]",
        "[^-ab]",
        r"[a-b\-c]",
        r"[a-b\x{2d}c]",
        r"[é-π\-x]",
    ] {
        compare(&model::GeneratedCase {
            pattern: pattern.into(),
            input: "-aπ".into(),
            replacement: "$0".into(),
            options: ReferenceOptions::default(),
        });
    }
}

fn options_from_bits(bits: u64) -> ReferenceOptions {
    ReferenceOptions {
        case_insensitive: bits & 1 != 0,
        multi_line: bits & 2 != 0,
        dot_matches_newline: bits & 4 != 0,
        crlf: bits & 8 != 0,
        ungreedy: bits & 16 != 0,
    }
}

fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/regex-cases.json")).unwrap()
}

#[test]
fn persistent_vectors_verify_model_domain_and_separate_unicode_expectations() {
    let corpus = corpus();
    for case in corpus["valid"].as_array().unwrap() {
        let pattern = case["pattern"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        let options = options_from_bits(case["options"].as_u64().unwrap());
        let kernel =
            Regex::compile(pattern, kernel_options(options), RegexLimits::default()).unwrap();
        let first = kernel.find(input).unwrap().map(|found| {
            (0..=kernel.capture_count())
                .map(|index| found.capture(index).map(|span| [span.start, span.end]))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            case["first"],
            "{}",
            case["id"]
        );
        assert_eq!(
            kernel.is_full_match(input).unwrap(),
            case["full"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
        if let Some(expected) = case.get("all") {
            let all = kernel
                .find_all(input)
                .unwrap()
                .map(|found| {
                    let span = found.unwrap().span;
                    [span.start, span.end]
                })
                .collect::<Vec<_>>();
            assert_eq!(
                serde_json::to_value(all).unwrap(),
                *expected,
                "{}",
                case["id"]
            );
        }
        if let Some(replacement) = case.get("replacement") {
            let template = replacement["template"].as_str().unwrap();
            assert_eq!(
                kernel.replace(input, template).unwrap(),
                replacement["first"].as_str().unwrap()
            );
            assert_eq!(
                kernel.replace_all(input, template).unwrap(),
                replacement["all"].as_str().unwrap()
            );
        }
        if case["model"].as_bool().unwrap() {
            compare(&model::GeneratedCase {
                pattern: pattern.into(),
                input: input.into(),
                replacement: "[$0]$$".into(),
                options,
            });
        }
    }
}

#[test]
fn persistent_rejections_have_repeatable_nominal_errors_and_scalar_spans() {
    use tondo_stdlib::regex::RegexPhase;
    let corpus = corpus();
    for case in corpus["invalid"].as_array().unwrap() {
        let pattern = case["pattern"].as_str().unwrap();
        let first = Regex::compile(pattern, Default::default(), Default::default()).unwrap_err();
        let second = Regex::compile(pattern, Default::default(), Default::default()).unwrap_err();
        assert_eq!(first, second, "{}", case["id"]);
        assert_eq!(
            format!("{:?}", first.kind),
            case["error"].as_str().unwrap(),
            "{}",
            case["id"]
        );
        assert_eq!(first.phase, RegexPhase::Compile);
        let span = first.span.unwrap();
        assert!(span.start <= span.end && span.end <= pattern.len());
        assert!(pattern.is_char_boundary(span.start) && pattern.is_char_boundary(span.end));
        assert_eq!(first.offset, span.start);
    }
}

#[test]
fn independent_replay_and_domain_limits_are_explicit() {
    for input in [
        Vec::new(),
        vec![0],
        vec![255; 512],
        vec![7; 4096],
        vec![7; 4097],
    ] {
        let first = model::run_regex_fuzz_case(&input).unwrap();
        assert_eq!(first, model::run_regex_fuzz_case(&input).unwrap());
        assert_eq!(
            first.steps,
            input.len().clamp(1, model::MAX_REGEX_FUZZ_STEPS)
        );
        assert!(first.output_bytes <= first.steps * model::MAX_REFERENCE_OUTPUT_BYTES);
    }
    let reference = ReferenceRegex::compile("a", ReferenceOptions::default()).unwrap();
    for input in ["a".repeat(33), "é".repeat(49)] {
        assert_eq!(
            reference.find(&input).unwrap_err().kind,
            ReferenceErrorKind::Limit
        );
    }
    let first = vec![7; model::MAX_REGEX_FUZZ_INPUT_BYTES];
    let mut longer = first.clone();
    longer.push(255);
    assert_eq!(
        model::seed_at_step(&first, 0),
        model::seed_at_step(&longer, 0)
    );
    assert_eq!(
        model::run_regex_fuzz_case(&first).unwrap(),
        model::run_regex_fuzz_case(&longer).unwrap()
    );
    let empty = ReferenceRegex::compile("", ReferenceOptions::default()).unwrap();
    assert_eq!(
        empty
            .replace(&"a".repeat(32), &"x".repeat(128), true)
            .unwrap_err()
            .kind,
        ReferenceErrorKind::Limit
    );
    assert_eq!(
        empty.replace("", &"x".repeat(129), true).unwrap_err().kind,
        ReferenceErrorKind::Limit
    );
}

#[test]
fn immutable_programs_have_independent_cursors_and_owned_capture_lifetimes() {
    use tondo_stdlib::regex::{RegexErrorKind, RegexPhase};
    let kernel = std::sync::Arc::new(
        Regex::compile(
            "(?<n>a)",
            Default::default(),
            RegexLimits {
                max_steps: 8,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let mut first = kernel.find_all("aaa").unwrap();
    let mut second = kernel.find_all("aaa").unwrap();
    let retained = first.next().unwrap().unwrap();
    assert_eq!(second.next().unwrap().unwrap(), retained);
    let error = loop {
        let item = first
            .next()
            .expect("bounded cursor must emit its limit error");
        assert_eq!(Some(item.clone()), second.next());
        if let Err(error) = item {
            break error;
        }
    };
    assert_eq!(error.kind, RegexErrorKind::StepLimitExceeded);
    assert_eq!(error.phase, RegexPhase::Match);
    assert_eq!(error.limit, Some("max_steps"));
    assert!(second.next().is_none() && second.next().is_none());
    assert!(first.next().is_none() && first.next().is_none());
    drop(kernel);
    assert_eq!(
        retained.capture_name("n").unwrap().slice("aaa").unwrap(),
        "a"
    );
    let kernel = std::sync::Arc::new(
        Regex::compile("(a|ab)+", Default::default(), Default::default()).unwrap(),
    );
    let threads = (0..4)
        .map(|_| {
            let kernel = kernel.clone();
            std::thread::spawn(move || {
                (
                    kernel.find("abab").unwrap(),
                    kernel.replace_all("abab", "$1").unwrap(),
                )
            })
        })
        .collect::<Vec<_>>();
    let expected = (
        kernel.find("abab").unwrap(),
        kernel.replace_all("abab", "$1").unwrap(),
    );
    for thread in threads {
        assert_eq!(thread.join().unwrap(), expected);
    }
}

#[test]
fn invalid_templates_are_checked_without_matches_and_output_errors_are_atomic() {
    use tondo_stdlib::regex::{RegexErrorKind, RegexPhase};
    let reference = ReferenceRegex::compile("(?<n>a)?", ReferenceOptions::default()).unwrap();
    let kernel = Regex::compile("(?<n>a)?", Default::default(), Default::default()).unwrap();
    for template in [
        "$",
        "$2",
        "${missing}",
        "${n",
        "$x",
        "$999999999999999999999999999",
    ] {
        assert_eq!(
            reference.replace("", template, true).unwrap_err().kind,
            ReferenceErrorKind::InvalidReplacement
        );
        let error = kernel.replace_all("", template).unwrap_err();
        assert_eq!(error.kind, RegexErrorKind::InvalidReplacement);
        assert_eq!(error.phase, RegexPhase::Replace);
        assert!(error.span.unwrap().end <= template.len());
    }
    let limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_output_bytes: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.replace_all("aa", "xx").unwrap_err().kind,
        RegexErrorKind::OutputLimitExceeded
    );
    assert_eq!(limited.replace_all("a", "xx").unwrap(), "xx");
    let replacement = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_replacement_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let error = replacement.replace("", "xx").unwrap_err();
    assert_eq!(error.kind, RegexErrorKind::ReplacementLimitExceeded);
    assert_eq!(error.limit, Some("max_replacement_bytes"));
    assert_eq!(replacement.replace("a", "x").unwrap(), "x");
}

#[test]
fn every_limit_and_option_is_bound_to_the_program_identity() {
    use tondo_stdlib::regex::RegexPhase;
    fn field(limits: &mut RegexLimits, index: usize) -> (&'static str, &mut usize) {
        match index {
            0 => ("max_pattern_bytes", &mut limits.max_pattern_bytes),
            1 => ("max_syntax_depth", &mut limits.max_syntax_depth),
            2 => ("max_capture_groups", &mut limits.max_capture_groups),
            3 => ("max_class_ranges", &mut limits.max_class_ranges),
            4 => ("max_repeat", &mut limits.max_repeat),
            5 => ("max_program_states", &mut limits.max_program_states),
            6 => ("max_input_bytes", &mut limits.max_input_bytes),
            7 => ("max_steps", &mut limits.max_steps),
            8 => ("max_matches", &mut limits.max_matches),
            9 => ("max_output_bytes", &mut limits.max_output_bytes),
            10 => ("max_replacement_bytes", &mut limits.max_replacement_bytes),
            11 => ("vm_heap", &mut limits.vm_heap),
            _ => unreachable!(),
        }
    }
    let base = Regex::compile("a", Default::default(), Default::default()).unwrap();
    let mut identities = std::collections::BTreeSet::from([base.fingerprint()]);
    for index in 0..12 {
        let mut limits = RegexLimits::default();
        *field(&mut limits, index).1 += 1;
        let changed = Regex::compile("a", Default::default(), limits).unwrap();
        assert!(identities.insert(changed.fingerprint()), "limit {index}");
        *field(&mut limits, index).1 = 0;
        let error = Regex::compile("a", Default::default(), limits).unwrap_err();
        assert_eq!(error.limit, Some(field(&mut limits, index).0));
        assert_eq!(error.phase, RegexPhase::Compile);
    }
    for index in 0..5 {
        let options = kernel_options(options_from_bits(1 << index));
        assert!(
            identities.insert(
                Regex::compile("a", options, Default::default())
                    .unwrap()
                    .fingerprint()
            )
        );
    }
    let exact = Regex::compile(
        "é",
        Default::default(),
        RegexLimits {
            max_input_bytes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(exact.is_full_match("é").unwrap());
    assert_eq!(exact.find("éa").unwrap_err().limit, Some("max_input_bytes"));
}
