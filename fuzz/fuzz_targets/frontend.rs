#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::syntax::{
    LexLimits, LexMode, ParseLimits, ParseMode, format_parsed, lex_with_limits, parse,
};

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(64 * 1024)];
    for (index, (lex_mode, parse_mode)) in [
        (LexMode::Module, ParseMode::Module),
        (LexMode::Script, ParseMode::Script),
        (LexMode::Fragment, ParseMode::Fragment),
    ]
    .into_iter()
    .enumerate()
    {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new(format!("fuzz:frontend:{index}")).unwrap(),
                ModulePath::new("fuzz").unwrap(),
                LogicalPath::new(format!("fuzz/frontend-{index}.to")).unwrap(),
                Arc::<[u8]>::from(input),
            ))
            .unwrap();
        let Ok(lexed) = lex_with_limits(
            &sources,
            file,
            lex_mode,
            LexLimits {
                max_tokens: 16_384,
                max_diagnostics: 512,
                max_nesting_depth: 128,
            },
        ) else {
            continue;
        };
        let Ok(parsed) = parse(
            &sources,
            file,
            lexed,
            parse_mode,
            ParseLimits {
                max_nodes: 32_768,
                max_nesting_depth: 128,
                max_diagnostics: 512,
            },
        ) else {
            continue;
        };
        assert!(
            parsed
                .cst()
                .has_exact_physical_partition(input.len() as u32)
        );
        assert_eq!(parsed.cst().reconstruct(input), input);
        if parsed.diagnostics().is_empty() {
            let formatted = format_parsed(&sources, file, &parsed).unwrap().into_bytes();
            let mut second_sources = SourceDatabase::new();
            let second_file = second_sources
                .add(SourceInput::virtual_file(
                    SourceId::new(format!("fuzz:frontend:formatted:{index}")).unwrap(),
                    ModulePath::new("fuzz").unwrap(),
                    LogicalPath::new(format!("fuzz/formatted-{index}.to")).unwrap(),
                    Arc::<[u8]>::from(formatted.clone()),
                ))
                .unwrap();
            let lexed =
                lex_with_limits(&second_sources, second_file, lex_mode, LexLimits::default())
                    .unwrap();
            let reparsed = parse(
                &second_sources,
                second_file,
                lexed,
                parse_mode,
                ParseLimits::default(),
            )
            .unwrap();
            assert!(reparsed.diagnostics().is_empty());
            assert_eq!(
                format_parsed(&second_sources, second_file, &reparsed)
                    .unwrap()
                    .into_bytes(),
                formatted
            );
        }
    }
});
