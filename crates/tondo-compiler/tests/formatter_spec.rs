use std::sync::Arc;

use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::syntax::{
    FormatError, LexMode, ParseLimits, ParseMode, format_parsed, lex, parse,
};

fn format_once(source: &[u8], mode: ParseMode) -> Vec<u8> {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("format:test").unwrap(),
            ModulePath::new("format").unwrap(),
            LogicalPath::new("format.to").unwrap(),
            Arc::<[u8]>::from(source),
        ))
        .unwrap();
    let lex_mode = if mode == ParseMode::Script {
        LexMode::Script
    } else if mode == ParseMode::Module {
        LexMode::Module
    } else {
        LexMode::Fragment
    };
    let lexed = lex(&sources, file, lex_mode).unwrap();
    let parsed = parse(&sources, file, lexed, mode, ParseLimits::default()).unwrap();
    assert!(
        parsed.diagnostics().is_empty(),
        "{:#?}",
        parsed.diagnostics()
    );
    format_parsed(&sources, file, &parsed).unwrap().into_bytes()
}

#[test]
fn normative_minimum_corpus_matches_byte_for_byte_and_is_idempotent() {
    let cases: &[(&[u8], &[u8], ParseMode)] = &[
        (
            b"fn add( a:Int,b:Int):Int {a+b}\n",
            b"fn add(a: Int, b: Int): Int {\n    a + b\n}\n",
            ParseMode::Module,
        ),
        (
            b"let values=[\n1,\n2\n]\n",
            b"let values = [1, 2]\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"import zeta\nimport alpha as a\nfn main(){}\n",
            b"import alpha as a\nimport zeta\n\nfn main() {}\n",
            ParseMode::Module,
        ),
        (
            b"let values=[1, // first\n2]\n",
            b"let values = [\n    1,  // first\n    2,\n]\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"type Loader=fn():Result[Option[Int],IoError]\n",
            b"type Loader = fn(): Int? ! IoError\n",
            ParseMode::Module,
        ),
        (
            b"fn make():impl Iterator[Int]+Discard{build()}\n",
            b"fn make(): impl Iterator[Int] + Discard {\n    build()\n}\n",
            ParseMode::Module,
        ),
        (
            b"let inverse=- -value\nlet nested=value? ?\n",
            b"let inverse = -(-value)\nlet nested = (value?)?\n",
            ParseMode::SyntaxSequence,
        ),
    ];

    for (input, expected, mode) in cases {
        let formatted = format_once(input, *mode);
        assert_eq!(
            formatted,
            *expected,
            "input: {}",
            String::from_utf8_lossy(input)
        );
        assert_eq!(format_once(&formatted, *mode), formatted);
    }
}

#[test]
fn list_layout_flattens_at_column_100_and_breaks_at_101() {
    for (payload, should_break) in [(82, false), (83, false), (84, true)] {
        let source = format!("let values = [\"{}\"]\n", "x".repeat(payload));
        let formatted =
            String::from_utf8(format_once(source.as_bytes(), ParseMode::SyntaxSequence)).unwrap();
        assert_eq!(formatted.contains("[\n"), should_break, "payload {payload}");
        assert_eq!(
            format_once(formatted.as_bytes(), ParseMode::SyntaxSequence),
            formatted.as_bytes()
        );
    }
}

#[test]
fn records_use_commas_only_in_flat_layout_and_blocks_preserve_one_blank_line() {
    let input = b"type Empty={}
type User={id:Int,name:String}
fn main(){let point=Point{x:1,y:2}


let user=User{
id:1,
name:\"Ada\"
}
}
";
    let expected = b"type Empty = {
}

type User = {
    id: Int
    name: String
}

fn main() {
    let point = Point { x: 1, y: 2 }

    let user = User { id: 1, name: \"Ada\" }
}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);

    let long = format!(
        "let value = User {{ first: \"{}\", second: 2 }}\n",
        "x".repeat(80)
    );
    let formatted =
        String::from_utf8(format_once(long.as_bytes(), ParseMode::SyntaxSequence)).unwrap();
    assert!(formatted.contains("User {\n"));
    assert!(formatted.contains("\n    second: 2\n}"));
    assert!(!formatted.contains("second: 2,"));
}

#[test]
fn comments_follow_their_units_and_import_groups_keep_section_boundaries() {
    let input = b"// copyright

import zeta // zeta note
/// alpha docs
import alpha

import omega
// platform imports

import beta
fn main(){}
";
    let expected = b"// copyright

/// alpha docs
import alpha
import zeta  // zeta note

import omega
// platform imports

import beta

fn main() {}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}

#[test]
fn doc_comments_remove_intervening_blanks_and_inline_comments_are_normalized() {
    let input = b"/// docs


fn main(){let value=1// note
let next=/* kept */2
}
";
    let expected = b"/// docs
fn main() {
    let value = 1  // note
    let next = /* kept */ 2
}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}

#[test]
fn script_shebang_crlf_and_identifier_nfc_are_canonicalized_without_touching_literals() {
    let input = "#!/usr/bin/env tondo\r\nlet cafe\u{301}=\"cafe\u{301}\"\r\n";
    let expected = "#!/usr/bin/env tondo\n\nlet caf\u{e9} = \"cafe\u{301}\"\n";

    let formatted = format_once(input.as_bytes(), ParseMode::Script);
    assert_eq!(formatted, expected.as_bytes());
    assert_eq!(format_once(&formatted, ParseMode::Script), formatted);

    assert_eq!(
        format_once(b"#!/usr/bin/env tondo", ParseMode::Script),
        b"#!/usr/bin/env tondo\n"
    );
}

#[test]
fn compact_type_normalization_inserts_required_parentheses() {
    let input = b"alias Nested = Option[Option[Int]]
alias OptionalUnion = Option[A | B]
alias FallibleUnit = Result[Unit, IoError]
alias CompactUnit = Unit ! IoError
alias FallibleUnions = Result[A | B, E | F]
alias BorrowedFunction = fn(ref A | B): Unit
alias VisibleUnion = User ! IoError | DecodeError
alias NestedResult = Result[Result[A, E1], E2]
alias OptionalResult = Option[Result[A, E]]
alias Qualified = pkg.Option[Int]
";
    let expected = b"alias Nested = (Int?)?

alias OptionalUnion = (A | B)?

alias FallibleUnit = !IoError

alias CompactUnit = !IoError

alias FallibleUnions = (A | B) ! (E | F)

alias BorrowedFunction = fn(ref (A | B))

alias VisibleUnion = (User ! IoError) | DecodeError

alias NestedResult = (A ! E1) ! E2

alias OptionalResult = (A ! E)?

alias Qualified = pkg.Option[Int]
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}

#[test]
fn long_operator_and_postfix_chains_break_only_at_normative_points() {
    let first = format!("A{}", "a".repeat(36));
    let second = format!("B{}", "b".repeat(36));
    let third = format!("C{}", "c".repeat(36));
    let source = format!(
        "alias Combined = {first} | {second} | {third}\n\
         fn make(): impl {first} + {second} + {third} {{}}\n"
    );
    let expected = format!(
        "alias Combined = {first} |\n    {second} |\n    {third}\n\n\
         fn make(): impl {first} +\n    {second} +\n    {third} {{}}\n"
    );
    let formatted = format_once(source.as_bytes(), ParseMode::Module);
    assert_eq!(formatted, expected.as_bytes());
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);

    let source = format!("let value = {first} + {second} + {third}\n");
    let expected = format!("let value = {first} +\n    {second} +\n    {third}\n");
    let formatted = format_once(source.as_bytes(), ParseMode::SyntaxSequence);
    assert_eq!(formatted, expected.as_bytes());

    let source = format!(
        "let value = {first}.first().second().third().fourth().fifth().sixth().seventh()\n"
    );
    let formatted =
        String::from_utf8(format_once(source.as_bytes(), ParseMode::SyntaxSequence)).unwrap();
    assert!(
        formatted.starts_with(&format!("let value = {first}\n    .first()")),
        "{formatted}"
    );
    assert_eq!(
        format_once(formatted.as_bytes(), ParseMode::SyntaxSequence),
        formatted.as_bytes()
    );
}

#[test]
fn every_shared_list_family_obeys_the_99_100_101_column_boundary() {
    let cases = [
        (
            "array",
            ParseMode::SyntaxSequence,
            "let value = [\"",
            "\"]",
            0,
        ),
        (
            "map",
            ParseMode::SyntaxSequence,
            "let value = [\"key\": \"",
            "\"]",
            0,
        ),
        (
            "set",
            ParseMode::SyntaxSequence,
            "let value = Set[\"",
            "\"]",
            0,
        ),
        (
            "tuple expression",
            ParseMode::SyntaxSequence,
            "let value = (\"",
            "\", 0)",
            0,
        ),
        (
            "call arguments",
            ParseMode::SyntaxSequence,
            "let value = call(\"",
            "\", 0)",
            0,
        ),
        (
            "parameters",
            ParseMode::SyntaxSequence,
            "fn work(value: Type",
            ")",
            0,
        ),
        (
            "generic arguments",
            ParseMode::Module,
            "alias Value = Box[Type",
            "]",
            0,
        ),
        (
            "generic parameters",
            ParseMode::SyntaxSequence,
            "fn work[Type",
            "]()",
            2,
        ),
        (
            "closure parameters",
            ParseMode::SyntaxSequence,
            "let callback = (value: Type",
            ") {}",
            3,
        ),
        (
            "function type parameters",
            ParseMode::Module,
            "alias Callback = fn(Type",
            ")",
            0,
        ),
        (
            "tuple type",
            ParseMode::Module,
            "alias Pair = (Type",
            ", Int)",
            0,
        ),
        (
            "array pattern",
            ParseMode::SyntaxSequence,
            "let [value",
            "] = values",
            9,
        ),
    ];

    for (name, mode, prefix, suffix, after_close) in cases {
        for target in [99, 100, 101] {
            let payload =
                "x".repeat(target + after_close - prefix.chars().count() - suffix.chars().count());
            let flat = format!("{prefix}{payload}{suffix}");
            assert_eq!(
                flat.chars().count() - after_close,
                target,
                "{name} at {target}"
            );
            let source = format!("{flat}\n");
            let formatted = String::from_utf8(format_once(source.as_bytes(), mode)).unwrap();
            if target <= 100 {
                assert_eq!(formatted, source, "{name} at {target}");
            } else {
                assert!(formatted.contains('\n'), "{name} at {target}");
                assert!(
                    formatted.lines().any(|line| line.trim_end().ends_with(',')),
                    "{name} at {target}: {formatted}"
                );
            }
            assert_eq!(
                format_once(formatted.as_bytes(), mode),
                formatted.as_bytes(),
                "{name} at {target}"
            );
        }
    }
}

#[test]
fn every_permitted_empty_delimited_form_is_compact() {
    let cases: &[(&[u8], &[u8], ParseMode)] = &[
        (
            b"let value=[]\n",
            b"let value = []\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"let value=[:]\n",
            b"let value = [:]\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"let value=Set[]\n",
            b"let value = Set[]\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"let value=()\n",
            b"let value = ()\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"let value=call()\n",
            b"let value = call()\n",
            ParseMode::SyntaxSequence,
        ),
        (
            b"let value=values[:]\n",
            b"let value = values[:]\n",
            ParseMode::SyntaxSequence,
        ),
        (b"fn empty()\n", b"fn empty()\n", ParseMode::SyntaxSequence),
        (
            b"let callback=(){}\n",
            b"let callback = () {}\n",
            ParseMode::SyntaxSequence,
        ),
        (b"fn empty(){}\n", b"fn empty() {}\n", ParseMode::Module),
        (b"trait Empty{}\n", b"trait Empty {\n}\n", ParseMode::Module),
    ];

    for (input, expected, mode) in cases {
        let formatted = format_once(input, *mode);
        assert_eq!(formatted, *expected, "{}", String::from_utf8_lossy(input));
        assert_eq!(format_once(&formatted, *mode), formatted);
    }
}

#[test]
fn list_comments_keep_punctuation_before_trailing_text_and_preserve_sections() {
    let input = b"let values=[
// first
1,
2 /* second */
]
let grouped=[
1,
// section

2
]
";
    let expected = b"let values = [
    // first
    1,
    2, /* second */
]
let grouped = [
    1,
    // section

    2,
]
";

    let formatted = format_once(input, ParseMode::SyntaxSequence);
    assert_eq!(formatted, expected);
    assert_eq!(
        format_once(&formatted, ParseMode::SyntaxSequence),
        formatted
    );
}

#[test]
fn multiline_records_drop_separator_commas_without_dropping_comments() {
    let input = b"type User={
id:Int, // identifier
name:String,
// profile

email:String
}
fn main(){
let user=User{id:1, /* stable */ name:\"Ada\"}
}
";
    let expected = b"type User = {
    id: Int  // identifier
    name: String
    // profile

    email: String
}

fn main() {
    let user = User {
        id: 1 /* stable */
        name: \"Ada\"
    }
}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}

#[test]
fn multiline_record_nested_in_a_broken_list_remains_parseable() {
    let input = b"fn main(){
let values=[Point{first:\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\",second:2}]
}
";
    let expected = b"fn main() {
    let values = [
        Point {
            first: \"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"
            second: 2
        },
    ]
}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}

#[test]
fn recoverable_invalid_syntax_is_rejected_without_fabricating_formatted_output() {
    let source: &[u8] = b"+ + malformed\nfn good() {}\n";
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("format:invalid").unwrap(),
            ModulePath::new("format").unwrap(),
            LogicalPath::new("invalid.to").unwrap(),
            Arc::<[u8]>::from(source),
        ))
        .unwrap();
    let lexed = lex(&sources, file, LexMode::Module).unwrap();
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .unwrap();

    assert!(!parsed.diagnostics().is_empty());
    assert_eq!(parsed.cst().reconstruct(source), source);
    assert!(matches!(
        format_parsed(&sources, file, &parsed),
        Err(FormatError::InvalidSyntax)
    ));
}

#[test]
fn comments_prevent_lossy_type_shorthand_and_unit_outcome_elision() {
    let input = b"alias Maybe = Option[Int /* keep long spelling */]
fn explicit(): Unit /* keep outcome */
";
    let expected = b"alias Maybe = Option[Int /* keep long spelling */]

fn explicit(): Unit /* keep outcome */
";

    let formatted = format_once(input, ParseMode::SyntaxSequence);
    assert_eq!(formatted, expected);
    assert_eq!(
        format_once(&formatted, ParseMode::SyntaxSequence),
        formatted
    );
}

#[test]
fn multiline_literals_are_indivisible_and_only_their_physical_newlines_change() {
    let input = b"let text=\"\"\"\r\n    first\r\n      second\r\n    \"\"\"\r\n";
    let expected = b"let text = \"\"\"\n    first\n      second\n    \"\"\"\n";

    let formatted = format_once(input, ParseMode::SyntaxSequence);
    assert_eq!(formatted, expected);
    assert_eq!(
        format_once(&formatted, ParseMode::SyntaxSequence),
        formatted
    );
}

#[test]
fn control_flow_braces_and_match_arms_have_one_canonical_shape() {
    let input = b"fn choose(value:Int):Int{
if value>0 {value}else if value<0 {-value}else{0}
}
fn classify(value:Int):String{
match value{
0=>\"zero\"
_=>\"other\"
}
}
";
    let expected = b"fn choose(value: Int): Int {
    if value > 0 {
        value
    } else if value < 0 {
        -value
    } else {
        0
    }
}

fn classify(value: Int): String {
    match value {
        0 => \"zero\"
        _ => \"other\"
    }
}
";

    let formatted = format_once(input, ParseMode::Module);
    assert_eq!(formatted, expected);
    assert_eq!(format_once(&formatted, ParseMode::Module), formatted);
}
