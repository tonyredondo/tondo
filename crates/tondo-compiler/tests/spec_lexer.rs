use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::syntax::{LexMode, ParseLimits, ParseMode, format_parsed, lex, parse};

#[derive(Debug)]
struct Fence {
    index: usize,
    line: usize,
    info: String,
    source: String,
}

#[test]
fn every_tondo_fence_in_the_pinned_spec_is_lexically_well_formed() {
    let path = specification_path();
    let markdown = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read pinned spec {}: {error}", path.display()));
    let fences = extract_tondo_fences(&markdown);
    assert!(
        fences.len() >= 250,
        "unexpectedly extracted only {} fences",
        fences.len()
    );

    let mut failures = Vec::new();
    for fence in fences {
        let mut sources = SourceDatabase::new();
        let logical_path = format!("spec/fence-{:04}.to", fence.index);
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new(format!("spec:0.1-draft.8:fence-{:04}", fence.index)).unwrap(),
                ModulePath::new("spec").unwrap(),
                LogicalPath::new(logical_path).unwrap(),
                Arc::<[u8]>::from(fence.source.as_bytes()),
            ))
            .unwrap();
        let mode = if fence
            .info
            .split_ascii_whitespace()
            .any(|item| item == "script")
        {
            LexMode::Script
        } else {
            LexMode::Fragment
        };
        let lexed = lex(&sources, file, mode).unwrap();
        let source = sources.get(file).unwrap();
        assert!(lexed.has_exact_physical_partition(source.length()));
        assert_eq!(lexed.reconstruct(source.bytes()), source.bytes());
        if !lexed.diagnostics().is_empty() {
            failures.push(format!(
                "fence {} at spec line {} ({}) -> {:?}",
                fence.index,
                fence.line,
                fence.info,
                lexed
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.code().as_str())
                    .collect::<Vec<_>>()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "lexical spec failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn executable_spec_fences_reach_their_expected_syntax_result() {
    let path = specification_path();
    let markdown = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read pinned spec {}: {error}", path.display()));
    let mut failures = Vec::new();

    for fence in extract_tondo_fences(&markdown) {
        let category = fence.info.split_ascii_whitespace().next();
        let mode = match category {
            Some("fragment" | "compile-fail") => ParseMode::Fragment,
            Some("script") => ParseMode::Script,
            _ => continue,
        };
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new(format!("spec:0.1-draft.8:fence-{:04}", fence.index)).unwrap(),
                ModulePath::new("spec").unwrap(),
                LogicalPath::new(format!("spec/fence-{:04}.to", fence.index)).unwrap(),
                Arc::<[u8]>::from(fence.source.as_bytes()),
            ))
            .unwrap();
        let lex_mode = if mode == ParseMode::Script {
            LexMode::Script
        } else {
            LexMode::Fragment
        };
        let lexed = lex(&sources, file, lex_mode).unwrap();
        let parsed = parse(&sources, file, lexed, mode, ParseLimits::default()).unwrap();
        let actual = parsed
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .filter(|code| code.starts_with("E000"))
            .collect::<Vec<_>>();
        let expects_syntax_error = fence
            .info
            .split_ascii_whitespace()
            .any(|item| item == "E0005");
        let matches = if expects_syntax_error {
            actual == ["E0005"]
        } else {
            actual.is_empty()
        };
        if !matches {
            failures.push(format!(
                "fence {} line {} ({}) -> {:?}",
                fence.index, fence.line, fence.info, actual
            ));
        }
        assert_eq!(
            parsed.cst().reconstruct(fence.source.as_bytes()),
            fence.source.as_bytes()
        );
    }

    assert!(
        failures.is_empty(),
        "spec parser failures ({}):\n{}",
        failures.len(),
        failures
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn syntax_spec_fences_match_one_of_the_normative_parser_surfaces() {
    let path = specification_path();
    let markdown = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read pinned spec {}: {error}", path.display()));
    let mut failures = Vec::new();

    for fence in extract_tondo_fences(&markdown)
        .into_iter()
        .filter(|fence| fence.info.is_empty())
    {
        let mut attempts = Vec::new();
        let mut accepted = false;
        for mode in [
            ParseMode::Module,
            ParseMode::SyntaxSequence,
            ParseMode::StandaloneBlock,
        ] {
            let mut sources = SourceDatabase::new();
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new(format!("spec:0.1-draft.8:fence-{:04}", fence.index)).unwrap(),
                    ModulePath::new("spec").unwrap(),
                    LogicalPath::new(format!("spec/fence-{:04}.to", fence.index)).unwrap(),
                    Arc::<[u8]>::from(fence.source.as_bytes()),
                ))
                .unwrap();
            let lexed = lex(&sources, file, LexMode::Fragment).unwrap();
            let parsed = match parse(&sources, file, lexed, mode, ParseLimits::default()) {
                Ok(parsed) => parsed,
                Err(error) => {
                    attempts.push(format!("{mode:?}=resource-or-internal:{error}"));
                    continue;
                }
            };
            let codes = parsed
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code().as_str())
                .filter(|code| code.starts_with("E000"))
                .collect::<Vec<_>>();
            if codes.is_empty() {
                assert_eq!(
                    parsed.cst().reconstruct(fence.source.as_bytes()),
                    fence.source.as_bytes()
                );
                accepted = true;
                break;
            }
            let details = parsed
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code().as_str().starts_with("E000"))
                .map(|diagnostic| {
                    format!(
                        "{}@{:?}[{:?}->{:?}]",
                        diagnostic.code(),
                        diagnostic.location(),
                        diagnostic.expected(),
                        diagnostic.actual()
                    )
                })
                .collect::<Vec<_>>();
            attempts.push(format!("{mode:?}={details:?}"));
        }
        if !accepted {
            failures.push(format!(
                "fence {} line {} -> {}",
                fence.index,
                fence.line,
                attempts.join(", ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "syntax fence failures ({}):\n{}",
        failures.len(),
        failures
            .iter()
            .take(100)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_syntactically_valid_spec_fence_formats_reparses_and_is_idempotent() {
    let path = specification_path();
    let markdown = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read pinned spec {}: {error}", path.display()));
    let mut failures = Vec::new();

    for fence in extract_tondo_fences(&markdown) {
        let category = fence.info.split_ascii_whitespace().next();
        if category == Some("pseudocode") {
            continue;
        }
        let modes: &[ParseMode] = match category {
            Some("fragment" | "compile-fail") => &[ParseMode::Fragment],
            Some("script") => &[ParseMode::Script],
            _ => &[
                ParseMode::Module,
                ParseMode::SyntaxSequence,
                ParseMode::StandaloneBlock,
            ],
        };
        let mut accepted = false;
        for &mode in modes {
            let mut sources = SourceDatabase::new();
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new(format!("format-spec:{:04}", fence.index)).unwrap(),
                    ModulePath::new("spec").unwrap(),
                    LogicalPath::new(format!("spec/fence-{:04}.to", fence.index)).unwrap(),
                    Arc::<[u8]>::from(fence.source.as_bytes()),
                ))
                .unwrap();
            let lex_mode = if mode == ParseMode::Script {
                LexMode::Script
            } else {
                LexMode::Fragment
            };
            let lexed = lex(&sources, file, lex_mode).unwrap();
            let parsed = parse(&sources, file, lexed, mode, ParseLimits::default()).unwrap();
            if parsed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code().as_str().starts_with("E000"))
            {
                continue;
            }
            accepted = true;
            let formatted = match format_parsed(&sources, file, &parsed) {
                Ok(formatted) => formatted.into_bytes(),
                Err(error) => {
                    failures.push(format!(
                        "fence {} line {} failed formatting: {error}",
                        fence.index, fence.line
                    ));
                    break;
                }
            };
            if !formatted.ends_with(b"\n")
                || formatted.ends_with(b"\n\n")
                || formatted.contains(&b'\r')
            {
                failures.push(format!(
                    "fence {} line {} has noncanonical line endings",
                    fence.index, fence.line
                ));
                break;
            }

            let mut formatted_sources = SourceDatabase::new();
            let formatted_file = formatted_sources
                .add(SourceInput::virtual_file(
                    SourceId::new(format!("formatted-spec:{:04}", fence.index)).unwrap(),
                    ModulePath::new("spec").unwrap(),
                    LogicalPath::new(format!("spec/formatted-{:04}.to", fence.index)).unwrap(),
                    Arc::<[u8]>::from(formatted.clone()),
                ))
                .unwrap();
            let lexed = lex(&formatted_sources, formatted_file, lex_mode).unwrap();
            let reparsed = parse(
                &formatted_sources,
                formatted_file,
                lexed,
                mode,
                ParseLimits::default(),
            )
            .unwrap();
            let syntax_codes = reparsed
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code().as_str())
                .filter(|code| code.starts_with("E000"))
                .collect::<Vec<_>>();
            if !syntax_codes.is_empty() {
                failures.push(format!(
                    "fence {} line {} no longer parses in {mode:?}: {syntax_codes:?}\n{}",
                    fence.index,
                    fence.line,
                    String::from_utf8_lossy(&formatted)
                ));
                break;
            }
            let second = format_parsed(&formatted_sources, formatted_file, &reparsed)
                .unwrap()
                .into_bytes();
            if second != formatted {
                failures.push(format!(
                    "fence {} line {} is not idempotent in {mode:?}\nfirst:\n{}\nsecond:\n{}",
                    fence.index,
                    fence.line,
                    String::from_utf8_lossy(&formatted),
                    String::from_utf8_lossy(&second)
                ));
            }
            break;
        }
        let expects_syntax_failure = fence
            .info
            .split_ascii_whitespace()
            .any(|item| item == "E0005");
        if !accepted && !expects_syntax_failure {
            failures.push(format!(
                "fence {} line {} had no valid formatter surface",
                fence.index, fence.line
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "spec formatter failures ({}):\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn specification_path() -> PathBuf {
    std::env::var_os("TONDO_LANGUAGE_SPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../TONDO_LANGUAGE_SPEC.md")
        })
}

fn extract_tondo_fences(markdown: &str) -> Vec<Fence> {
    let mut result = Vec::new();
    let mut lines = markdown.lines().enumerate();
    while let Some((line_index, line)) = lines.next() {
        let Some(info) = line.strip_prefix("~~~tondo") else {
            continue;
        };
        let mut source = String::new();
        let mut closed = false;
        for (_, content) in lines.by_ref() {
            if content == "~~~" {
                closed = true;
                break;
            }
            source.push_str(content);
            source.push('\n');
        }
        assert!(
            closed,
            "unclosed Tondo fence at spec line {}",
            line_index + 1
        );
        result.push(Fence {
            index: result.len() + 1,
            line: line_index + 1,
            info: info.trim().to_owned(),
            source,
        });
    }
    result
}

#[test]
fn fence_extraction_is_closed_and_stable() {
    let markdown = "before\n~~~tondo fragment demo\nlet value = 1\n~~~\nafter\n";
    let fences = extract_tondo_fences(markdown);
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0].line, 2);
    assert_eq!(fences[0].info, "fragment demo");
    assert_eq!(fences[0].source, "let value = 1\n");
}
