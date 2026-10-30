//! The dialect of `std.regex` (standard library 14.16), which the runtime
//! checks before it compiles a pattern and the compiler checks in a
//! literal pattern of `Regex.compile`, with the same verdict.

use regex_syntax::ast::{self, Ast, ErrorKind as Syntax};

/// The kinds of `RegexErrorKind`, in its order from 1.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    InvalidSyntax = 1,
    UnexpectedEnd,
    InvalidEscape,
    InvalidUnicodeScalar,
    InvalidUnicodeProperty,
    InvalidClass,
    InvalidRange,
    InvalidQuantifier,
    InvalidCaptureName,
    DuplicateCaptureName,
    UnsupportedFeature,
    PatternLimitExceeded,
    ProgramLimitExceeded,
    InputLimitExceeded,
    MatchLimitExceeded,
    ReplacementLimitExceeded,
    InvalidReplacement,
    OutputLimitExceeded,
    InvalidBoundary,
}

/// Whether embedded flags are those the dialect has: `i`, `m`, `s` and `U`,
/// each of them set or cleared.
fn allowed(flags: &ast::Flags) -> bool {
    flags.items.iter().all(|item| {
        matches!(
            item.kind,
            ast::FlagsItemKind::Negation
                | ast::FlagsItemKind::Flag(
                    ast::Flag::CaseInsensitive
                        | ast::Flag::MultiLine
                        | ast::Flag::DotMatchesNewLine
                        | ast::Flag::SwapGreed
                )
        )
    })
}

/// Where a parsed pattern leaves the dialect, first in the pattern: an
/// embedded flag it does not have, the `(?P<name>...)` form, or a capture
/// past the limit.
fn outside(ast: &Ast, max_captures: i64) -> Option<(Kind, usize)> {
    let mut stack = vec![ast];
    while let Some(node) = stack.pop() {
        let offset = node.span().start.offset;
        match node {
            Ast::Flags(flags) if !allowed(&flags.flags) => return Some((Kind::UnsupportedFeature, offset)),
            Ast::Group(group) => {
                let index = match &group.kind {
                    ast::GroupKind::NonCapturing(flags) if !allowed(flags) => {
                        return Some((Kind::UnsupportedFeature, offset));
                    }
                    ast::GroupKind::CaptureName { starts_with_p: true, .. } => {
                        return Some((Kind::UnsupportedFeature, offset));
                    }
                    ast::GroupKind::CaptureName { name, .. } => name.index,
                    ast::GroupKind::CaptureIndex(index) => *index,
                    ast::GroupKind::NonCapturing(_) => 0,
                };
                if i64::from(index) > max_captures {
                    return Some((Kind::PatternLimitExceeded, offset));
                }
                stack.push(&group.ast);
            }
            Ast::Repetition(repetition) => stack.push(&repetition.ast),
            Ast::Alternation(alternation) => stack.extend(alternation.asts.iter().rev()),
            Ast::Concat(concat) => stack.extend(concat.asts.iter().rev()),
            _ => {}
        }
    }
    None
}

fn syntax_kind(kind: &Syntax) -> Kind {
    match kind {
        Syntax::UnsupportedBackreference | Syntax::UnsupportedLookAround => Kind::UnsupportedFeature,
        Syntax::FlagDanglingNegation
        | Syntax::FlagDuplicate { .. }
        | Syntax::FlagRepeatedNegation { .. }
        | Syntax::FlagUnexpectedEof
        | Syntax::FlagUnrecognized => Kind::UnsupportedFeature,
        Syntax::ClassEscapeInvalid | Syntax::ClassRangeLiteral | Syntax::ClassUnclosed => Kind::InvalidClass,
        Syntax::ClassRangeInvalid => Kind::InvalidRange,
        Syntax::DecimalEmpty
        | Syntax::DecimalInvalid
        | Syntax::RepetitionCountDecimalEmpty
        | Syntax::RepetitionCountInvalid
        | Syntax::RepetitionCountUnclosed
        | Syntax::RepetitionMissing => Kind::InvalidQuantifier,
        Syntax::EscapeHexEmpty | Syntax::EscapeHexInvalidDigit | Syntax::EscapeHexInvalid => Kind::InvalidUnicodeScalar,
        Syntax::EscapeUnexpectedEof => Kind::UnexpectedEnd,
        Syntax::EscapeUnrecognized | Syntax::SpecialWordBoundaryUnrecognized => Kind::InvalidEscape,
        Syntax::GroupNameDuplicate { .. } => Kind::DuplicateCaptureName,
        Syntax::GroupNameEmpty | Syntax::GroupNameInvalid | Syntax::GroupNameUnexpectedEof => Kind::InvalidCaptureName,
        Syntax::GroupUnclosed => Kind::UnexpectedEnd,
        Syntax::NestLimitExceeded(_) | Syntax::CaptureLimitExceeded => Kind::PatternLimitExceeded,
        Syntax::UnicodeClassInvalid => Kind::InvalidUnicodeProperty,
        _ => Kind::InvalidSyntax,
    }
}

/// Whether a pattern is in the dialect, with case-insensitive matching or
/// not, as deep as `depth` and with up to `max_captures` groups; else the
/// kind and byte offset of what is not.
pub(crate) fn check(pattern: &str, case_insensitive: bool, depth: u32, max_captures: i64) -> Result<(), (Kind, usize)> {
    let parsed = ast::parse::ParserBuilder::new()
        .nest_limit(depth)
        .build()
        .parse(pattern)
        .map_err(|error| (syntax_kind(error.kind()), error.span().start.offset))?;
    if let Some(error) = outside(&parsed, max_captures) {
        return Err(error);
    }
    let translated = regex_syntax::hir::translate::TranslatorBuilder::new()
        .case_insensitive(case_insensitive)
        .build()
        .translate(pattern, &parsed);
    if let Err(error) = translated {
        use regex_syntax::hir::ErrorKind as Hir;
        let kind = match error.kind() {
            Hir::UnicodePropertyNotFound | Hir::UnicodePropertyValueNotFound | Hir::UnicodePerlClassNotFound => {
                Kind::InvalidUnicodeProperty
            }
            _ => Kind::UnsupportedFeature,
        };
        return Err((kind, error.span().start.offset));
    }
    Ok(())
}
