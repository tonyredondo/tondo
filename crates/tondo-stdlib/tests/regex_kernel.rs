use tondo_stdlib::regex::{
    Regex, RegexErrorKind as Kind, RegexLimits, RegexOptions, RegexPhase, RegexSpan,
    UNICODE_VERSION,
};

fn regex(pattern: &str) -> Regex {
    Regex::compile(pattern, RegexOptions::default(), RegexLimits::default()).unwrap()
}
fn span(start: usize, end: usize) -> RegexSpan {
    RegexSpan { start, end }
}

#[test]
fn local_priorities_and_full_search_are_distinct() {
    for (pattern, input, expected) in [
        ("a|ab", "ab", Some(span(0, 1))),
        ("a*?a*", "aaa", Some(span(0, 3))),
        ("a*?", "aaa", Some(span(0, 0))),
        ("a*", "aaa", Some(span(0, 3))),
        ("ab|a", "ab", Some(span(0, 2))),
        ("a{2,4}?", "aaaa", Some(span(0, 2))),
        ("a{2,4}", "aaaa", Some(span(0, 4))),
        ("a*b|a", "aaaa", Some(span(0, 1))),
        ("a+", "xxaaa", Some(span(2, 5))),
        ("a+", "xxx", None),
    ] {
        assert_eq!(
            regex(pattern).find(input).unwrap().map(|found| found.span),
            expected,
            "{pattern}"
        );
    }
    assert!(regex("a|ab").is_full_match("ab").unwrap());
    assert!(!regex("a+").is_full_match("ba").unwrap());
}

#[test]
fn captures_preserve_optional_and_empty_repeated_groups() {
    let compiled = regex("(?<empty>a*?)(?<rest>a*)(b)?");
    assert_eq!(compiled.capture_count(), 3);
    assert_eq!(
        compiled.capture_names().collect::<Vec<_>>(),
        ["empty", "rest"]
    );
    let found = compiled.find("aaa").unwrap().unwrap();
    assert_eq!(found.capture(0), Some(span(0, 3)));
    assert_eq!(found.capture_name("empty"), Some(span(0, 0)));
    assert_eq!(found.capture_name("rest"), Some(span(0, 3)));
    assert_eq!(found.capture(3), None);
    assert_eq!(found.capture(99), None);
    assert_eq!(found.capture_name("absent"), None);
    let found = regex("(a*)*").find("aaa").unwrap().unwrap();
    assert_eq!(found.capture(1), Some(span(3, 3)));
    let found = regex("((a*?)*?)*").find("a").unwrap().unwrap();
    assert_eq!(found.span, span(0, 0));
}

#[test]
fn unicode_16_properties_simple_folding_and_exact_word_set() {
    assert_eq!(UNICODE_VERSION, "16.0.0");
    for pattern in [
        r"\p{Script=Tulu_Tigalari}",
        r"\p{scx=Tulu_Tigalari}",
        r"\p{L}",
        r"\p{Alphabetic}",
    ] {
        assert_eq!(
            regex(pattern).find("\u{11380}").unwrap().unwrap().span,
            span(0, 4)
        );
    }
    for pattern in [r"\w", r"[\w]", r"[^\W]"] {
        assert!(regex(pattern).is_match("\u{301}").unwrap(), "{pattern}");
        assert!(
            !regex(pattern).is_match("\u{200c}").unwrap(),
            "Join_Control excluded: {pattern}"
        );
    }
    assert!(regex(r"\bа\b").is_match("\u{200c}а\u{200d}").unwrap());
    assert!(regex(r"\P{Letter}").is_match("4").unwrap());
    let folded = Regex::compile(
        "k",
        RegexOptions {
            case_insensitive: true,
            ..Default::default()
        },
        RegexLimits::default(),
    )
    .unwrap();
    assert!(folded.is_match("K").unwrap());
    let sharp = Regex::compile(
        "ß",
        RegexOptions {
            case_insensitive: true,
            ..Default::default()
        },
        RegexLimits::default(),
    )
    .unwrap();
    assert!(!sharp.is_full_match("ss").unwrap());
    assert!(!regex("é").is_match("e\u{301}").unwrap());
}

#[test]
fn line_anchors_dot_and_options_are_explicit() {
    let compile =
        |pattern, options| Regex::compile(pattern, options, RegexLimits::default()).unwrap();
    assert!(!regex("^a$").is_match("x\na\n").unwrap());
    let lines = RegexOptions {
        multi_line: true,
        ..Default::default()
    };
    assert!(compile("^a$", lines).is_match("x\na\n").unwrap());
    assert!(!compile(r"\Aa\z", lines).is_match("x\na\n").unwrap());
    let crlf = RegexOptions {
        crlf: true,
        ..lines
    };
    assert!(compile("^a$", crlf).is_match("x\r\na\r\n").unwrap());
    assert!(!compile("^a$", crlf).is_match("x\ra\r").unwrap());
    assert!(!compile(".", crlf).is_match("\r").unwrap());
    assert!(
        compile(
            ".",
            RegexOptions {
                dot_matches_newline: true,
                ..crlf
            }
        )
        .is_match("\r\n")
        .unwrap()
    );
    assert_eq!(
        compile(
            "a+",
            RegexOptions {
                ungreedy: true,
                ..Default::default()
            }
        )
        .find("aaa")
        .unwrap()
        .unwrap()
        .span,
        span(0, 1)
    );
    assert_eq!(
        compile(
            "a+?",
            RegexOptions {
                ungreedy: true,
                ..Default::default()
            }
        )
        .find("aaa")
        .unwrap()
        .unwrap()
        .span,
        span(0, 3)
    );
}

#[test]
fn iterator_is_lazy_fallible_cumulative_and_fused() {
    let limited = Regex::compile(
        "a",
        RegexOptions::default(),
        RegexLimits {
            max_steps: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let mut iter = limited.find_all("aa").unwrap();
    let first = iter.next().unwrap().unwrap();
    assert_eq!(first.span, span(0, 1));
    assert_eq!(
        iter.next().unwrap().unwrap_err().kind,
        Kind::StepLimitExceeded
    );
    assert!(iter.next().is_none());
    assert!(iter.next().is_none());
    assert_eq!(first.span.slice("aa").unwrap(), "a");
    let limited = Regex::compile(
        "a",
        RegexOptions::default(),
        RegexLimits {
            max_matches: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut exact = limited.find_all("a").unwrap();
    assert!(exact.next().unwrap().is_ok());
    assert!(exact.next().is_none());
    let mut excess = limited.find_all("aa").unwrap();
    assert!(excess.next().unwrap().is_ok());
    assert_eq!(
        excess.next().unwrap().unwrap_err().kind,
        Kind::MatchLimitExceeded
    );
    assert!(excess.next().is_none());
    let empty = regex("");
    let found = empty
        .find_all("é🦀")
        .unwrap()
        .map(|item| item.unwrap().span)
        .collect::<Vec<_>>();
    assert_eq!(found, [span(0, 0), span(2, 2), span(6, 6)]);
    assert_eq!(empty.find_all("").unwrap().count(), 1);
    assert_eq!(regex("a").find_all("xx").unwrap().count(), 0);
}

#[test]
fn replacement_templates_and_zero_width_progress_are_atomic() {
    let compiled = regex("(?<letter>[ab])(c)?");
    assert_eq!(
        compiled.replace("ab", "${letter}-$0-$1-$2-$$").unwrap(),
        "a-a-a--$b"
    );
    assert_eq!(compiled.replace_all("ab", "[$1]").unwrap(), "[a][b]");
    assert_eq!(regex("").replace_all("é🦀", "_").unwrap(), "_é_🦀_");
    for template in [
        "$",
        "$99",
        "${unknown}",
        "${letter",
        "$x",
        "$999999999999999999999999999",
    ] {
        let error = compiled.replace_all("no match", template).unwrap_err();
        assert_eq!(error.kind, Kind::InvalidReplacement, "{template}");
        assert_eq!(error.phase, RegexPhase::Replace);
    }
    assert_eq!(compiled.replace("zz", "ok").unwrap(), "zz");
    let limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_output_bytes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.replace_all("aa", "xx").unwrap_err().kind,
        Kind::OutputLimitExceeded
    );
    let limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_matches: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.replace_all("aa", "x").unwrap_err().kind,
        Kind::MatchLimitExceeded
    );
}

#[test]
fn closed_dialect_rejects_extensions_and_reports_spans() {
    for pattern in [
        r"(a)\1",
        r"\k<a>",
        "(?=a)",
        "(?!a)",
        "(?<=a)",
        "(?<!a)",
        "(?>a)",
        "a*+",
        "(?i)a",
        "(?i:a)",
        "(?P<a>a)",
        "[[:alpha:]]",
        "[a&&b]",
        "[a--b]",
        "[a~~b]",
        "[a[b]]",
        r"\C",
        r"\B",
    ] {
        let error = Regex::compile(pattern, Default::default(), Default::default()).unwrap_err();
        assert_eq!(error.kind, Kind::UnsupportedFeature, "{pattern}");
        assert!(error.span.unwrap().end <= pattern.len());
    }
    for (pattern, kind) in [
        (r"\q", Kind::InvalidEscape),
        (r"\x41", Kind::InvalidEscape),
        (r"\x{d800}", Kind::InvalidUnicodeScalar),
        (r"\x{110000}", Kind::InvalidUnicodeScalar),
        (r"\p{NoSuchProperty}", Kind::InvalidUnicodeProperty),
        (r"\p{Age=16.0}", Kind::InvalidUnicodeProperty),
        ("[z-a]", Kind::InvalidRange),
        ("[", Kind::InvalidClass),
        ("a{3,2}", Kind::InvalidQuantifier),
        ("a**", Kind::InvalidQuantifier),
        ("(){0,0}", Kind::InvalidQuantifier),
        ("(?<1>a)", Kind::InvalidCaptureName),
        ("(?<n>a)(?<n>b)", Kind::DuplicateCaptureName),
    ] {
        assert_eq!(
            Regex::compile(pattern, Default::default(), Default::default())
                .unwrap_err()
                .kind,
            kind,
            "{pattern}"
        );
    }
    assert!(regex(r"\x{1f980}").is_match("🦀").unwrap());
    assert!(regex(r"[\-a-z]\n").is_match("-\n").unwrap());
}

#[test]
fn compile_and_match_limits_are_nominal_and_fingerprints_are_stable() {
    let heap_limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            vm_heap: 512,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(heap_limited.find("a").unwrap_err().kind, Kind::OutOfMemory);
    assert_eq!(
        heap_limited
            .replace_all("a", &"x".repeat(64))
            .unwrap_err()
            .kind,
        Kind::OutOfMemory
    );
    for property in ["Any", "any", "ASCII", "a_s_c_i_i", "Assigned", "assigned"] {
        assert_eq!(
            Regex::compile(
                &format!(r"\p{{{property}}}"),
                Default::default(),
                Default::default()
            )
            .unwrap_err()
            .kind,
            Kind::InvalidUnicodeProperty
        );
    }
    for (pattern, limits, id) in [
        (
            "aaa",
            RegexLimits {
                max_pattern_bytes: 2,
                ..Default::default()
            },
            "max_pattern_bytes",
        ),
        (
            "((a))",
            RegexLimits {
                max_syntax_depth: 1,
                ..Default::default()
            },
            "max_syntax_depth",
        ),
        (
            "(a)(b)",
            RegexLimits {
                max_capture_groups: 1,
                ..Default::default()
            },
            "max_capture_groups",
        ),
        (
            "a{3}",
            RegexLimits {
                max_repeat: 2,
                ..Default::default()
            },
            "max_repeat",
        ),
        (
            "ab",
            RegexLimits {
                max_program_states: 1,
                ..Default::default()
            },
            "max_program_states",
        ),
        (
            r"\p{L}",
            RegexLimits {
                max_class_ranges: 1,
                ..Default::default()
            },
            "max_class_ranges",
        ),
        (
            "a",
            RegexLimits {
                vm_heap: 1,
                ..Default::default()
            },
            "vm_heap",
        ),
        (
            "",
            RegexLimits {
                max_steps: 0,
                ..Default::default()
            },
            "max_steps",
        ),
    ] {
        let error = Regex::compile(pattern, Default::default(), limits).unwrap_err();
        assert_eq!(error.limit, Some(id), "{pattern}");
        assert_eq!(error.phase, RegexPhase::Compile);
    }
    let limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_input_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.find("é").unwrap_err().kind,
        Kind::InputLimitExceeded
    );
    let limited = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_replacement_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.replace("a", "xx").unwrap_err().kind,
        Kind::ReplacementLimitExceeded
    );
    assert_eq!(regex("é").fingerprint(), regex("é").fingerprint());
    assert_ne!(regex("a").fingerprint(), regex("b").fingerprint());
    assert_ne!(
        regex("a").fingerprint(),
        Regex::compile(
            "a",
            RegexOptions {
                ungreedy: true,
                ..Default::default()
            },
            Default::default()
        )
        .unwrap()
        .fingerprint()
    );
    assert_eq!(span(0, 2).slice("é").unwrap(), "é");
    for invalid in [span(1, 2), span(2, 1), span(0, 3)] {
        assert_eq!(invalid.slice("é").unwrap_err().kind, Kind::InvalidBoundary);
    }
}

#[test]
fn bounded_ambiguous_and_nested_patterns_terminate_without_recursion() {
    let shared = std::sync::Arc::new(regex("(?<value>é+)"));
    let workers = (0..3)
        .map(|_| {
            let compiled = shared.clone();
            std::thread::spawn(move || {
                for _ in 0..16 {
                    assert_eq!(
                        compiled.find("éé").unwrap().unwrap().capture_name("value"),
                        Some(span(0, 4))
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let pattern = "(a|aa)*b";
    let limited = Regex::compile(
        pattern,
        Default::default(),
        RegexLimits {
            max_steps: 1000,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited.find(&"a".repeat(2000)).unwrap_err().kind,
        Kind::StepLimitExceeded
    );
    let input = "a".repeat(64);
    assert!(!regex(pattern).is_match(&input).unwrap());
    let nested = format!("{}a{}", "(?:".repeat(128), ")".repeat(128));
    let thread = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            assert!(regex(&nested).is_match("a").unwrap());
            assert_eq!(regex("(?:)*").find("x").unwrap().unwrap().span, span(0, 0));
        })
        .unwrap();
    thread.join().unwrap();
}

#[test]
fn malformed_unicode_patterns_never_panic_or_split_error_boundaries() {
    let scalars = [
        'a', 'é', '🦀', '\\', '[', ']', '(', ')', '?', '*', '+', '{', '}', '|', '$', '^', '-', '1',
    ];
    let mut state = 4113u64;
    for seed in 0..4096 {
        let mut pattern = String::new();
        for _ in 0..seed % 32 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            pattern.push(scalars[(state >> 32) as usize % scalars.len()]);
        }
        let outcome = std::panic::catch_unwind(|| {
            Regex::compile(&pattern, Default::default(), Default::default())
        });
        match outcome.unwrap_or_else(|_| panic!("compile panicked for {pattern:?}")) {
            Ok(compiled) => {
                let _ = compiled.find("é🦀aaa").unwrap();
            }
            Err(error) => {
                let span = error.span.unwrap();
                assert!(
                    span.start <= span.end && span.end <= pattern.len(),
                    "{pattern:?}: {error:?}"
                );
                assert!(
                    pattern.is_char_boundary(span.start) && pattern.is_char_boundary(span.end),
                    "{pattern:?}: {error:?}"
                );
            }
        }
    }
}
