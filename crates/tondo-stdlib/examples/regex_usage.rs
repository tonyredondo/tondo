//! Executable usage of the bounded Rust regex kernel.
//! Public Tondo compiler/VM registration and native regex ABI/AOT are absent.

use tondo_stdlib::regex::{
    Regex, RegexError, RegexErrorKind, RegexLimits, RegexOptions, RegexPhase, RegexSpan,
    UNICODE_VERSION,
};

fn compile(pattern: &str) -> Result<Regex, RegexError> {
    Regex::compile(pattern, RegexOptions::default(), RegexLimits::default())
}

fn patterns_and_reuse() -> Result<(), RegexError> {
    let regex = compile(r"[A-Z][0-9]{2}")?;
    for value in ["A12", "B34"] {
        assert!(regex.is_full_match(value)?);
    }
    assert!(regex.is_match("prefix A12 suffix")?);
    assert!(!regex.is_full_match("prefix A12 suffix")?);
    assert_eq!(
        regex.fingerprint(),
        compile(r"[A-Z][0-9]{2}")?.fingerprint()
    );
    assert_eq!(
        compile("a|ab")?.find("ab")?.unwrap().span,
        RegexSpan { start: 0, end: 1 }
    );
    assert!(compile("a|ab")?.is_full_match("ab")?);
    assert_eq!(
        compile("a*?a*")?.find("aaa")?.unwrap().span,
        RegexSpan { start: 0, end: 3 }
    );
    assert_eq!(
        compile(r"(?=a)").unwrap_err().kind,
        RegexErrorKind::UnsupportedFeature
    );
    Ok(())
}

fn unicode_and_options() -> Result<(), RegexError> {
    assert_eq!(UNICODE_VERSION, "16.0.0");
    assert!(compile(r"\p{scx=Tulu_Tigalari}")?.is_full_match("\u{11380}")?);
    assert!(!compile("é")?.is_match("e\u{301}")?);
    let options = RegexOptions {
        case_insensitive: true,
        ..Default::default()
    };
    assert!(Regex::compile("k", options, Default::default())?.is_full_match("K")?);
    assert!(!Regex::compile("ß", options, Default::default())?.is_full_match("ss")?);
    let options = RegexOptions {
        multi_line: true,
        crlf: true,
        ..Default::default()
    };
    assert!(Regex::compile("^a$", options, Default::default())?.is_match("x\r\na\r\n")?);
    let options = RegexOptions {
        dot_matches_newline: true,
        ..Default::default()
    };
    assert!(Regex::compile(".", options, Default::default())?.is_full_match("\n")?);
    let options = RegexOptions {
        ungreedy: true,
        ..Default::default()
    };
    assert_eq!(
        Regex::compile("a+", options, Default::default())?
            .find("aaa")?
            .unwrap()
            .span,
        RegexSpan { start: 0, end: 1 }
    );
    Ok(())
}

fn captures_and_spans() -> Result<(), RegexError> {
    let regex = compile("(?<symbol>é)(?<empty>a*?)(b)?")?;
    let input = "é";
    let found = regex.find(input)?.unwrap();
    assert_eq!(found.capture(0), Some(RegexSpan { start: 0, end: 2 }));
    let symbol = found.capture_name("symbol").unwrap();
    assert_eq!(symbol.slice(input)?, "é");
    let empty = found.capture_name("empty").unwrap();
    assert_eq!(empty, RegexSpan { start: 2, end: 2 });
    assert_eq!(empty.slice(input)?, "");
    assert_eq!(found.capture(3), None);
    assert_eq!(found.capture_name("unknown"), None);
    assert_eq!(
        RegexSpan { start: 1, end: 2 }
            .slice(input)
            .unwrap_err()
            .kind,
        RegexErrorKind::InvalidBoundary
    );
    assert_eq!(
        compile("(a*)*")?.find("aaa")?.unwrap().capture(1),
        Some(RegexSpan { start: 3, end: 3 })
    );
    Ok(())
}

fn lazy_iteration_and_ownership() -> Result<(), RegexError> {
    let regex = compile("")?;
    let input = String::from("é🙂");
    let mut cursor = regex.find_all(&input)?;
    let positions = cursor
        .by_ref()
        .map(|item| item.map(|found| found.span.start))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(positions, [0, 2, 6]);
    assert!(cursor.next().is_none() && cursor.next().is_none());

    let limits = RegexLimits {
        max_matches: 1,
        ..Default::default()
    };
    let regex = Regex::compile("a", Default::default(), limits)?;
    let input = String::from("aa");
    let first = {
        let mut cursor = regex.find_all(&input)?;
        let first = cursor.next().unwrap()?;
        let error = cursor.next().unwrap().unwrap_err();
        assert_eq!(error.kind, RegexErrorKind::MatchLimitExceeded);
        assert_eq!(error.phase, RegexPhase::Match);
        assert_eq!(error.offset, 1);
        assert_eq!(error.limit, Some("max_matches"));
        assert!(cursor.next().is_none() && cursor.next().is_none());
        first
    };
    // The owned capture spans remain usable while the original input exists.
    assert_eq!(first.span.slice(&input)?, "a");
    assert!(regex.is_match("a")?);
    Ok(())
}

fn replacement_and_errors() -> Result<(), RegexError> {
    let regex = compile("(?<letter>[ab])(c)?")?;
    assert_eq!(regex.replace("ab", "${letter}-$0-$1-$2-$$")?, "a-a-a--$b");
    assert_eq!(regex.replace_all("ab", "[$1]")?, "[a][b]");
    assert_eq!(compile("")?.replace_all("é🙂", "_")?, "_é_🙂_");
    let invalid = regex.replace_all("zz", "$unknown").unwrap_err();
    assert_eq!(invalid.kind, RegexErrorKind::InvalidReplacement);
    assert_eq!(invalid.phase, RegexPhase::Replace);

    let limits = RegexLimits {
        max_output_bytes: 2,
        ..Default::default()
    };
    let regex = Regex::compile("a", Default::default(), limits)?;
    let error = regex.replace_all("aaaa", "x").unwrap_err();
    assert_eq!(error.kind, RegexErrorKind::OutputLimitExceeded);
    assert_eq!(error.phase, RegexPhase::Replace);
    assert_eq!(error.offset, 2);
    assert_eq!(error.limit, Some("max_output_bytes"));
    assert_eq!(regex.replace_all("a", "x")?, "x");
    Ok(())
}

fn limits_and_costs() -> Result<(), RegexError> {
    let limits = RegexLimits {
        max_pattern_bytes: 1,
        ..Default::default()
    };
    let error = Regex::compile("é", Default::default(), limits).unwrap_err();
    assert_eq!(error.kind, RegexErrorKind::PatternLimitExceeded);
    assert_eq!(error.phase, RegexPhase::Compile);
    assert_eq!(error.limit, Some("max_pattern_bytes"));
    let limits = RegexLimits {
        max_input_bytes: 2,
        ..Default::default()
    };
    let regex = Regex::compile("a", Default::default(), limits)?;
    assert_eq!(
        regex.find("éx").unwrap_err().kind,
        RegexErrorKind::InputLimitExceeded
    );
    let limits = RegexLimits {
        max_steps: 1,
        ..Default::default()
    };
    let regex = Regex::compile("a", Default::default(), limits)?;
    assert_eq!(
        regex.find("a").unwrap_err().kind,
        RegexErrorKind::StepLimitExceeded
    );
    let limits = RegexLimits {
        vm_heap: 1,
        ..Default::default()
    };
    assert_eq!(
        Regex::compile("abcd", Default::default(), limits)
            .unwrap_err()
            .kind,
        RegexErrorKind::OutOfMemory
    );
    let limits = RegexLimits {
        max_steps: 0,
        ..Default::default()
    };
    let error = Regex::compile("a", Default::default(), limits).unwrap_err();
    assert_eq!(error.kind, RegexErrorKind::PatternLimitExceeded);
    assert_eq!(error.phase, RegexPhase::Compile);
    assert_eq!(error.limit, Some("max_steps"));
    let error = compile("é[a-b-c]").unwrap_err();
    assert_eq!(error.kind, RegexErrorKind::InvalidClass);
    assert_eq!(error.phase, RegexPhase::Compile);
    assert_eq!(error.span, Some(RegexSpan { start: 6, end: 7 }));
    Ok(())
}

fn main() -> Result<(), RegexError> {
    patterns_and_reuse()?;
    unicode_and_options()?;
    captures_and_spans()?;
    lazy_iteration_and_ownership()?;
    replacement_and_errors()?;
    limits_and_costs()?;
    println!("regex-doc-ok");
    Ok(())
}
