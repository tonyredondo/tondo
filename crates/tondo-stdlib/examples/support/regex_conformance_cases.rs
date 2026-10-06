//! Shared bounded kernel assertions for the private VM and native Rust process.
//! Neither route registers production std.regex calls or a native regex ABI.

use tondo_stdlib::json::{self, JsonValue};
use tondo_stdlib::regex::{
    Regex, RegexError, RegexErrorKind as Kind, RegexLimits, RegexMatch, RegexOptions,
    RegexPhase as Phase, RegexSpan, UNICODE_VERSION,
};

pub const CASES: [&str; 7] = [
    "retained-vectors",
    "capture-priorities",
    "unicode-options",
    "lazy-lifecycle",
    "replacement",
    "limits-errors",
    "route-boundary",
];
pub const CORPUS: &[u8] =
    include_bytes!("../../../tondo-reliability/tests/fixtures/regex-cases.json");

fn field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    let JsonValue::Object(items) = value else {
        panic!("fixture object")
    };
    items
        .iter()
        .find(|item| item.key == key)
        .map(|item| &item.value)
}
fn text<'a>(value: &'a JsonValue, key: &str) -> &'a str {
    let Some(JsonValue::String(value)) = field(value, key) else {
        panic!("fixture string")
    };
    value
}
fn array(value: &JsonValue) -> &[JsonValue] {
    let JsonValue::Array(items) = value else {
        panic!("fixture array")
    };
    items
}
fn number(value: &JsonValue) -> usize {
    let JsonValue::Number(value) = value else {
        panic!("fixture integer")
    };
    value.to_uint().unwrap().try_into().unwrap()
}
fn span(start: usize, end: usize) -> RegexSpan {
    RegexSpan { start, end }
}
fn fixture_span(value: &JsonValue) -> RegexSpan {
    let items = array(value);
    assert_eq!(items.len(), 2);
    span(number(&items[0]), number(&items[1]))
}
fn captures(found: &RegexMatch, count: usize) -> Vec<Option<RegexSpan>> {
    (0..=count).map(|index| found.capture(index)).collect()
}
pub fn options(bits: usize) -> RegexOptions {
    RegexOptions {
        case_insensitive: bits & 1 != 0,
        multi_line: bits & 2 != 0,
        dot_matches_newline: bits & 4 != 0,
        crlf: bits & 8 != 0,
        ungreedy: bits & 16 != 0,
    }
}
fn compile(pattern: &str) -> Regex {
    Regex::compile(pattern, RegexOptions::default(), RegexLimits::default()).unwrap()
}
fn limited(kind: Kind, phase: Phase, offset: usize, limit: &'static str) -> RegexError {
    RegexError {
        kind,
        phase,
        offset,
        span: Some(span(offset, offset)),
        limit: Some(limit),
    }
}

fn retained_vectors() -> String {
    let corpus = json::parse(CORPUS).unwrap();
    let valid = array(field(&corpus, "valid").unwrap());
    let invalid = array(field(&corpus, "invalid").unwrap());
    for fixture in valid {
        let pattern = text(fixture, "pattern");
        let input = text(fixture, "input");
        let regex = Regex::compile(
            pattern,
            options(number(field(fixture, "options").unwrap())),
            Default::default(),
        )
        .unwrap();
        let expected = match field(fixture, "first").unwrap() {
            JsonValue::Null => None,
            value => Some(
                array(value)
                    .iter()
                    .map(|item| match item {
                        JsonValue::Null => None,
                        item => Some(fixture_span(item)),
                    })
                    .collect::<Vec<_>>(),
            ),
        };
        let found = regex.find(input).unwrap();
        assert_eq!(
            found
                .as_ref()
                .map(|item| captures(item, regex.capture_count())),
            expected,
            "{pattern}"
        );
        assert_eq!(regex.is_match(input).unwrap(), found.is_some());
        assert_eq!(
            field(fixture, "full"),
            Some(&JsonValue::Bool(regex.is_full_match(input).unwrap()))
        );
        if let Some(expected) = field(fixture, "all") {
            let mut cursor = regex.find_all(input).unwrap();
            let actual = cursor
                .by_ref()
                .map(|item| item.unwrap().span)
                .collect::<Vec<_>>();
            assert_eq!(
                actual,
                array(expected).iter().map(fixture_span).collect::<Vec<_>>()
            );
            assert!(cursor.next().is_none() && cursor.next().is_none());
        }
        if let Some(replacement) = field(fixture, "replacement") {
            let template = text(replacement, "template");
            assert_eq!(
                regex.replace(input, template).unwrap(),
                text(replacement, "first")
            );
            assert_eq!(
                regex.replace_all(input, template).unwrap(),
                text(replacement, "all")
            );
        }
    }
    for fixture in invalid {
        let pattern = text(fixture, "pattern");
        let first = Regex::compile(pattern, Default::default(), Default::default()).unwrap_err();
        let second = Regex::compile(pattern, Default::default(), Default::default()).unwrap_err();
        assert_eq!(first, second);
        assert_eq!(format!("{:?}", first.kind), text(fixture, "error"));
        assert_eq!(first.phase, Phase::Compile);
        let span = first.span.unwrap();
        assert_eq!(first.offset, span.start);
        assert!(span.start <= span.end && span.end <= pattern.len());
        assert!(pattern.is_char_boundary(span.start) && pattern.is_char_boundary(span.end));
    }
    format!(
        "retained-vectors:{}:{}:full-captures-replacements",
        valid.len(),
        invalid.len()
    )
}

fn capture_priorities() -> String {
    assert_eq!(
        compile("a|ab").find("ab").unwrap().unwrap().span,
        span(0, 1)
    );
    assert!(compile("a|ab").is_full_match("ab").unwrap());
    assert_eq!(
        compile("a*?a*").find("aaa").unwrap().unwrap().span,
        span(0, 3)
    );
    let regex = std::sync::Arc::new(compile("(?<empty>a*?)(?<rest>a*)(b)?"));
    let found = regex.find("aaa").unwrap().unwrap();
    assert_eq!(found.capture_name("empty"), Some(span(0, 0)));
    assert_eq!(found.capture_name("rest"), Some(span(0, 3)));
    assert_eq!(found.capture(3), None);
    assert_eq!(found.capture_name("absent"), None);
    assert_eq!(
        compile("(a*)*").find("aaa").unwrap().unwrap().capture(1),
        Some(span(3, 3))
    );
    let clone = regex.clone();
    assert_eq!(regex.fingerprint(), clone.fingerprint());
    assert_eq!(regex.find("aaa"), clone.find("aaa"));
    "capture-priorities:local-order:empty-absent-repeat:clone".into()
}

fn unicode_options() -> String {
    assert_eq!(UNICODE_VERSION, "16.0.0");
    assert!(
        compile(r"\p{scx=Tulu_Tigalari}")
            .is_full_match("\u{11380}")
            .unwrap()
    );
    assert!(!compile("é").is_match("e\u{301}").unwrap());
    assert!(
        Regex::compile("k", options(1), Default::default())
            .unwrap()
            .is_full_match("K")
            .unwrap()
    );
    assert!(
        !Regex::compile("ß", options(1), Default::default())
            .unwrap()
            .is_full_match("ss")
            .unwrap()
    );
    assert!(
        Regex::compile("^a$", options(2), Default::default())
            .unwrap()
            .is_match("x\na\n")
            .unwrap()
    );
    assert!(
        Regex::compile(".", options(4), Default::default())
            .unwrap()
            .is_full_match("\n")
            .unwrap()
    );
    assert!(
        Regex::compile("^a$", options(2 | 8), Default::default())
            .unwrap()
            .is_match("a\r\n")
            .unwrap()
    );
    assert_eq!(
        Regex::compile("a+", options(16), Default::default())
            .unwrap()
            .find("aaa")
            .unwrap()
            .unwrap()
            .span,
        span(0, 1)
    );
    "unicode-options:16-0-0:simple-fold:no-normalization:five-flags".into()
}

fn lazy_lifecycle() -> String {
    let empty = compile("");
    let mut cursor = empty.find_all("é🙂").unwrap();
    assert_eq!(
        cursor
            .by_ref()
            .map(|item| item.unwrap().span)
            .collect::<Vec<_>>(),
        [span(0, 0), span(2, 2), span(6, 6)]
    );
    assert!(cursor.next().is_none() && cursor.next().is_none());
    let regex = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_matches: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let first = {
        let mut cursor = regex.find_all("aa").unwrap();
        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first.span, span(0, 1));
        assert_eq!(
            cursor.next().unwrap().unwrap_err(),
            limited(Kind::MatchLimitExceeded, Phase::Match, 1, "max_matches")
        );
        assert!(cursor.next().is_none() && cursor.next().is_none());
        first
    };
    assert_eq!(first.span.slice("aa").unwrap(), "a");
    assert!(regex.is_match("a").unwrap());
    "lazy-lifecycle:utf8-eof-once:valid-prefix:error-once:fused".into()
}

fn replacement() -> String {
    let regex = compile("(?<letter>[ab])(c)?");
    assert_eq!(
        regex.replace("ab", "${letter}-$0-$1-$2-$$").unwrap(),
        "a-a-a--$b"
    );
    assert_eq!(regex.replace_all("ab", "[$1]").unwrap(), "[a][b]");
    assert_eq!(compile("").replace_all("é🙂", "_").unwrap(), "_é_🙂_");
    let invalid = regex.replace_all("zz", "$x").unwrap_err();
    assert_eq!(invalid.kind, Kind::InvalidReplacement);
    assert_eq!(invalid.phase, Phase::Replace);
    let regex = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_output_bytes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        regex.replace_all("aaaa", "x").unwrap_err(),
        limited(
            Kind::OutputLimitExceeded,
            Phase::Replace,
            2,
            "max_output_bytes"
        )
    );
    assert_eq!(regex.replace_all("a", "x").unwrap(), "x");
    "replacement:named-numbered-dollar:utf8-empty:atomic-output".into()
}

fn limits_errors() -> String {
    let limits = RegexLimits {
        max_pattern_bytes: 1,
        ..Default::default()
    };
    assert_eq!(
        Regex::compile("é", Default::default(), limits).unwrap_err(),
        limited(
            Kind::PatternLimitExceeded,
            Phase::Compile,
            0,
            "max_pattern_bytes"
        )
    );
    let limits = RegexLimits {
        max_program_states: 2,
        ..Default::default()
    };
    assert_eq!(
        Regex::compile("abcd", Default::default(), limits).unwrap_err(),
        limited(
            Kind::ProgramLimitExceeded,
            Phase::Compile,
            0,
            "max_program_states"
        )
    );
    let regex = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_input_bytes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        regex.find("éx").unwrap_err(),
        limited(Kind::InputLimitExceeded, Phase::Match, 0, "max_input_bytes")
    );
    let regex = Regex::compile(
        "a",
        Default::default(),
        RegexLimits {
            max_steps: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        regex.find("a").unwrap_err(),
        limited(Kind::StepLimitExceeded, Phase::Match, 0, "max_steps")
    );
    let error = Regex::compile("é[a-b-c]", Default::default(), Default::default()).unwrap_err();
    assert_eq!(
        error,
        RegexError {
            kind: Kind::InvalidClass,
            phase: Phase::Compile,
            offset: 6,
            span: Some(span(6, 7)),
            limit: None
        }
    );
    assert_eq!(
        span(1, 2).slice("é").unwrap_err().kind,
        Kind::InvalidBoundary
    );
    "limits-errors:pattern-program-input-steps:exact-byte-spans".into()
}

pub fn run_case(id: &str) -> Option<String> {
    Some(match id {
        "retained-vectors" => retained_vectors(),
        "capture-priorities" => capture_priorities(),
        "unicode-options" => unicode_options(),
        "lazy-lifecycle" => lazy_lifecycle(),
        "replacement" => replacement(),
        "limits-errors" => limits_errors(),
        "route-boundary" => {
            "route-boundary:scalar:public-api-not-implemented:native-abi-aot-not-claimed".into()
        }
        _ => return None,
    })
}
