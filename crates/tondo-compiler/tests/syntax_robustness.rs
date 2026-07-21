use std::sync::Arc;

use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::syntax::{
    LexLimits, LexMode, ParseError, ParseLimits, ParseMode, ParseResource, format_parsed,
    lex_with_limits, parse,
};

fn exercise(bytes: &[u8], case: usize, mode: ParseMode) {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("robust:{case}:{mode:?}")).unwrap(),
            ModulePath::new("robust").unwrap(),
            LogicalPath::new(format!("robust/{case}.to")).unwrap(),
            Arc::<[u8]>::from(bytes),
        ))
        .unwrap();
    let lex_mode = match mode {
        ParseMode::Module => LexMode::Module,
        ParseMode::Script => LexMode::Script,
        ParseMode::Fragment | ParseMode::SyntaxSequence | ParseMode::StandaloneBlock => {
            LexMode::Fragment
        }
    };
    let lexed = match lex_with_limits(
        &sources,
        file,
        lex_mode,
        LexLimits {
            max_tokens: 4_096,
            max_diagnostics: 256,
            max_nesting_depth: 128,
        },
    ) {
        Ok(lexed) => lexed,
        Err(_) => return,
    };
    let parsed = match parse(
        &sources,
        file,
        lexed,
        mode,
        ParseLimits {
            max_nodes: 8_192,
            max_nesting_depth: 128,
            max_diagnostics: 256,
        },
    ) {
        Ok(parsed) => parsed,
        Err(_) => return,
    };
    assert!(
        parsed
            .cst()
            .has_exact_physical_partition(bytes.len() as u32)
    );
    assert_eq!(parsed.cst().reconstruct(bytes), bytes);
}

#[test]
fn every_single_byte_is_a_controlled_frontend_input() {
    for byte in 0_u8..=u8::MAX {
        for mode in [ParseMode::Module, ParseMode::Script, ParseMode::Fragment] {
            exercise(&[byte], byte as usize, mode);
        }
    }
}

#[test]
fn deterministic_arbitrary_byte_corpus_never_panics_or_loses_source() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for case in 0..2_048 {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        let length = (state as usize % 128) + 1;
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            state ^= state << 7;
            state ^= state >> 9;
            state ^= state << 8;
            bytes.push(state as u8);
        }
        exercise(&bytes, case + 256, ParseMode::Fragment);
    }
}

#[test]
fn deeply_nested_expression_reaches_a_typed_limit_before_the_process_stack() {
    let depth = 257;
    let mut source = String::with_capacity(depth * 2 + 7);
    source.extend(std::iter::repeat_n('(', depth));
    source.push_str("value");
    source.extend(std::iter::repeat_n(')', depth));
    source.push('\n');

    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("robust:deep-expression").unwrap(),
            ModulePath::new("robust").unwrap(),
            LogicalPath::new("robust/deep-expression.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    let lexed = lex_with_limits(
        &sources,
        file,
        LexMode::Fragment,
        LexLimits {
            max_nesting_depth: 512,
            ..LexLimits::default()
        },
    )
    .unwrap();
    let error = parse(
        &sources,
        file,
        lexed,
        ParseMode::Fragment,
        ParseLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ParseError::ResourceLimit {
            resource: ParseResource::NestingDepth,
            ..
        }
    ));
}

#[test]
fn deterministic_valid_program_corpus_formats_to_a_fixed_point() {
    let mut state = 0xd1b5_4a32_d192_ed03_u64;
    for case in 0..512 {
        let first = generated_expression(&mut state, 3);
        let second = generated_expression(&mut state, 3);
        let list = if case % 7 == 0 {
            format!("[{first}, // generated {case}\n{second}]")
        } else {
            format!("[{first},{second}]")
        };
        let source = format!(
            "fn generated{case}( input:Int ):Int{{\nlet values={list}\nif input>0{{\n{first}\n}}else{{\n{second}\n}}\n}}\n"
        );
        let formatted = format_valid_program(&source, case);
        let formatted_text = std::str::from_utf8(&formatted).unwrap();
        assert!(formatted_text.ends_with('\n'), "case {case}");
        assert!(!formatted_text.ends_with("\n\n"), "case {case}");
        assert!(
            formatted_text
                .split('\n')
                .all(|line| !line.ends_with(' ') && !line.ends_with('\t')),
            "case {case}: {formatted_text}"
        );
    }
}

fn format_valid_program(source: &str, case: usize) -> Vec<u8> {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("format-robust:{case}")).unwrap(),
            ModulePath::new("robust").unwrap(),
            LogicalPath::new(format!("robust/format-{case}.to")).unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    let lexed = lex_with_limits(&sources, file, LexMode::Module, LexLimits::default()).unwrap();
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .unwrap();
    assert!(
        parsed.diagnostics().is_empty(),
        "case {case}: {:#?}\n{source}",
        parsed.diagnostics()
    );
    let formatted = format_parsed(&sources, file, &parsed).unwrap().into_bytes();

    let mut formatted_sources = SourceDatabase::new();
    let formatted_file = formatted_sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("formatted-robust:{case}")).unwrap(),
            ModulePath::new("robust").unwrap(),
            LogicalPath::new(format!("robust/formatted-{case}.to")).unwrap(),
            Arc::<[u8]>::from(formatted.clone()),
        ))
        .unwrap();
    let lexed = lex_with_limits(
        &formatted_sources,
        formatted_file,
        LexMode::Module,
        LexLimits::default(),
    )
    .unwrap();
    let reparsed = parse(
        &formatted_sources,
        formatted_file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .unwrap_or_else(|error| {
        panic!(
            "case {case}: {error:?}\n{}",
            String::from_utf8_lossy(&formatted)
        )
    });
    assert!(
        reparsed.diagnostics().is_empty(),
        "case {case}: {:#?}\n{}",
        reparsed.diagnostics(),
        String::from_utf8_lossy(&formatted)
    );
    let second = format_parsed(&formatted_sources, formatted_file, &reparsed)
        .unwrap()
        .into_bytes();
    assert_eq!(second, formatted, "case {case}");
    formatted
}

fn generated_expression(state: &mut u64, depth: usize) -> String {
    let choice = next_random(state) % if depth == 0 { 3 } else { 11 };
    match choice {
        0 => (next_random(state) % 10_000).to_string(),
        1 => format!("value{}", next_random(state) % 97),
        2 => format!("\"text{}\"", next_random(state) % 97),
        3 => format!(
            "some({})",
            generated_expression(state, depth.saturating_sub(1))
        ),
        4 => format!(
            "[{},{}]",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        5 => format!(
            "({}, {})",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        6 => format!(
            "compute({},{})",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        7 => format!(
            "({}+{})",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        8 => format!(
            "Point{{x:{},y:{}}}",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        9 => format!(
            "Set[{},{}]",
            generated_expression(state, depth - 1),
            generated_expression(state, depth - 1)
        ),
        _ => format!("compute({}).field", generated_expression(state, depth - 1)),
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 7;
    *state ^= *state >> 9;
    *state ^= *state << 8;
    *state
}
