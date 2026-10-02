//! Closed-dialect admission before the dependency's iterative AST/HIR parser.

use regex_syntax::{
    ast::{self, Ast, ClassSet, ClassSetItem},
    hir::{self, Hir, HirKind},
};

use super::{
    RegexError, RegexErrorKind as Kind, RegexLimits, RegexOptions, RegexPhase, copy_text, reserve,
};

const WORD: &str = r"[\p{Alphabetic}\p{M}\p{Nd}\p{Pc}]";

fn error(kind: Kind, start: usize, end: usize) -> RegexError {
    RegexError::at(kind, RegexPhase::Compile, start, end)
}
fn limit(kind: Kind, offset: usize, id: &'static str) -> RegexError {
    RegexError::limited(kind, RegexPhase::Compile, offset, id)
}

fn property_key_is(name: &str, expected: &[u8]) -> bool {
    name.bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .eq(expected.iter().copied())
}

/// Counts opening frames/captures and rejects extensions before AST allocation.
fn admit(pattern: &str, limits: RegexLimits) -> Result<(), RegexError> {
    if pattern.len() > limits.max_pattern_bytes {
        return Err(limit(Kind::PatternLimitExceeded, 0, "max_pattern_bytes"));
    }
    if pattern.len().saturating_mul(256) > limits.vm_heap {
        return Err(limit(Kind::OutOfMemory, 0, "vm_heap"));
    }
    let bytes = pattern.as_bytes();
    let mut offset = 0;
    let mut frames = 0usize;
    let mut captures = 0usize;
    let mut class = false;
    while offset < bytes.len() {
        let start = offset;
        let scalar = pattern[offset..]
            .chars()
            .next()
            .expect("valid nonempty UTF-8 suffix");
        offset += scalar.len_utf8();
        if scalar == '\\' {
            let Some(escaped_scalar) = pattern[offset..].chars().next() else {
                return Err(error(Kind::UnexpectedEnd, start, offset));
            };
            offset += escaped_scalar.len_utf8();
            if !escaped_scalar.is_ascii() {
                return Err(error(Kind::InvalidEscape, start, offset));
            }
            let escaped = escaped_scalar as u8;
            match escaped {
                b'x' => {
                    if bytes.get(offset) != Some(&b'{') {
                        return Err(error(Kind::InvalidEscape, start, offset));
                    }
                    let Some(relative) = pattern[offset + 1..].find('}') else {
                        return Err(error(Kind::UnexpectedEnd, start, bytes.len()));
                    };
                    let end = offset + 1 + relative;
                    let hex = &pattern[offset + 1..end];
                    if hex.is_empty()
                        || hex.len() > 6
                        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || u32::from_str_radix(hex, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .is_none()
                    {
                        return Err(error(Kind::InvalidUnicodeScalar, start, end + 1));
                    }
                    offset = end + 1;
                }
                b'p' | b'P' => {
                    if bytes.get(offset) != Some(&b'{') {
                        return Err(error(Kind::InvalidUnicodeProperty, start, offset));
                    }
                    let Some(relative) = pattern[offset + 1..].find('}') else {
                        return Err(error(Kind::UnexpectedEnd, start, bytes.len()));
                    };
                    let end = offset + 1 + relative;
                    let property = &pattern[offset + 1..end];
                    let allowed = if let Some((name, _)) = property.split_once('=') {
                        matches!(
                            name,
                            "General_Category"
                                | "gc"
                                | "Script"
                                | "sc"
                                | "Script_Extensions"
                                | "scx"
                        )
                    } else {
                        ![
                            b"any".as_slice(),
                            b"ascii".as_slice(),
                            b"assigned".as_slice(),
                        ]
                        .iter()
                        .any(|key| property_key_is(property, key))
                    };
                    if !allowed || property.contains(['!', ':']) {
                        return Err(error(Kind::InvalidUnicodeProperty, start, end + 1));
                    }
                    offset = end + 1;
                }
                b'b' | b'A' | b'z' if class => {
                    return Err(error(Kind::InvalidClass, start, offset));
                }
                b'b' if bytes.get(offset) == Some(&b'{') => {
                    return Err(error(Kind::UnsupportedFeature, start, offset + 1));
                }
                b'0'..=b'9' | b'k' | b'C' | b'B' => {
                    return Err(error(Kind::UnsupportedFeature, start, offset));
                }
                b'\\' | b'.' | b'^' | b'$' | b'|' | b'(' | b')' | b'[' | b']' | b'{' | b'}'
                | b'*' | b'+' | b'?' | b'-' | b'a' | b'f' | b'n' | b'r' | b't' | b'v' | b'd'
                | b'D' | b's' | b'S' | b'w' | b'W' | b'b' | b'A' | b'z' => {}
                _ => return Err(error(Kind::InvalidEscape, start, offset)),
            }
            continue;
        }
        if class {
            match scalar {
                '[' => return Err(error(Kind::UnsupportedFeature, start, offset)),
                ']' => {
                    class = false;
                    frames -= 1;
                }
                '&' | '~' | '-' if bytes.get(offset) == Some(&(scalar as u8)) => {
                    return Err(error(Kind::UnsupportedFeature, start, offset + 1));
                }
                _ => {}
            }
            continue;
        }
        match scalar {
            '(' | '[' => {
                if frames == limits.max_syntax_depth {
                    return Err(limit(Kind::PatternLimitExceeded, start, "max_syntax_depth"));
                }
                frames += 1;
                if scalar == '[' {
                    class = true;
                    continue;
                }
                if bytes.get(offset) == Some(&b'?') {
                    if bytes.get(offset + 1) == Some(&b':') {
                        offset += 2;
                    } else if bytes.get(offset + 1) == Some(&b'<')
                        && !matches!(bytes.get(offset + 2), Some(b'=' | b'!'))
                    {
                        captures += 1;
                        let name_start = offset + 2;
                        let Some(relative) = pattern[name_start..].find('>') else {
                            return Err(error(Kind::UnexpectedEnd, start, bytes.len()));
                        };
                        let end = name_start + relative;
                        let name = &pattern[name_start..end];
                        if name.is_empty()
                            || !name.bytes().enumerate().all(|(index, byte)| {
                                byte == b'_'
                                    || byte.is_ascii_alphabetic()
                                    || (index > 0 && byte.is_ascii_digit())
                            })
                        {
                            return Err(error(Kind::InvalidCaptureName, name_start, end));
                        }
                        offset = end + 1;
                    } else {
                        let end = offset
                            + pattern[offset..]
                                .chars()
                                .take(2)
                                .map(char::len_utf8)
                                .sum::<usize>();
                        return Err(error(Kind::UnsupportedFeature, start, end));
                    }
                } else {
                    captures += 1;
                }
                if captures > limits.max_capture_groups {
                    return Err(limit(
                        Kind::ProgramLimitExceeded,
                        start,
                        "max_capture_groups",
                    ));
                }
            }
            ')' => {
                frames = frames.saturating_sub(1);
            }
            ']' | '}' => return Err(error(Kind::InvalidSyntax, start, offset)),
            '{' => {
                let Some(relative) = pattern[offset..].find('}') else {
                    return Err(error(Kind::UnexpectedEnd, start, bytes.len()));
                };
                let end = offset + relative;
                for number in pattern[offset..end]
                    .split(',')
                    .filter(|part| !part.is_empty())
                {
                    if !number.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err(error(Kind::InvalidQuantifier, start, end + 1));
                    }
                    let value = number
                        .parse::<usize>()
                        .map_err(|_| error(Kind::InvalidQuantifier, start, end + 1))?;
                    if value > limits.max_repeat {
                        return Err(limit(Kind::ProgramLimitExceeded, start, "max_repeat"));
                    }
                }
                offset = end + 1;
                if bytes.get(offset) == Some(&b'+') {
                    return Err(error(Kind::UnsupportedFeature, start, offset + 1));
                }
            }
            '*' | '+' | '?' if bytes.get(offset) == Some(&b'+') => {
                return Err(error(Kind::UnsupportedFeature, start, offset + 1));
            }
            _ => {}
        }
    }
    Ok(())
}

fn ast_error(value: ast::Error) -> RegexError {
    use ast::ErrorKind::*;
    let kind = match value.kind() {
        ClassEscapeInvalid | ClassRangeLiteral | ClassUnclosed => Kind::InvalidClass,
        ClassRangeInvalid => Kind::InvalidRange,
        EscapeHexEmpty | EscapeHexInvalid | EscapeHexInvalidDigit => Kind::InvalidUnicodeScalar,
        EscapeUnexpectedEof | GroupUnclosed | GroupNameUnexpectedEof | RepetitionCountUnclosed => {
            Kind::UnexpectedEnd
        }
        EscapeUnrecognized => Kind::InvalidEscape,
        GroupNameEmpty | GroupNameInvalid => Kind::InvalidCaptureName,
        GroupNameDuplicate { .. } => Kind::DuplicateCaptureName,
        RepetitionCountInvalid
        | RepetitionCountDecimalEmpty
        | RepetitionMissing
        | DecimalInvalid => Kind::InvalidQuantifier,
        UnicodeClassInvalid => Kind::InvalidUnicodeProperty,
        UnsupportedBackreference | UnsupportedLookAround => Kind::UnsupportedFeature,
        NestLimitExceeded(_) | CaptureLimitExceeded => Kind::PatternLimitExceeded,
        _ => Kind::InvalidSyntax,
    };
    error(kind, value.span().start.offset, value.span().end.offset)
}

fn word_ast(span: ast::Span, negated: bool) -> Result<ast::ClassBracketed, RegexError> {
    let mut parsed = ast::parse::Parser::new().parse(WORD).map_err(ast_error)?;
    let Ast::ClassBracketed(class) = &mut parsed else {
        unreachable!("fixed word union is a bracketed class");
    };
    class.span = span;
    class.negated = negated;
    Ok((**class).clone())
}

fn class_items(class: &mut ast::ClassBracketed) -> Result<(), RegexError> {
    let ClassSet::Item(item) = &mut class.kind else {
        return Err(error(
            Kind::UnsupportedFeature,
            class.span.start.offset,
            class.span.end.offset,
        ));
    };
    let mut pending = vec![item];
    while let Some(item) = pending.pop() {
        match item {
            ClassSetItem::Perl(perl) if perl.kind == ast::ClassPerlKind::Word => {
                *item = ClassSetItem::Bracketed(Box::new(word_ast(perl.span, perl.negated)?));
            }
            ClassSetItem::Union(union) => pending.extend(union.items.iter_mut()),
            ClassSetItem::Ascii(value) => {
                return Err(error(
                    Kind::UnsupportedFeature,
                    value.span.start.offset,
                    value.span.end.offset,
                ));
            }
            ClassSetItem::Bracketed(value) => {
                return Err(error(
                    Kind::UnsupportedFeature,
                    value.span.start.offset,
                    value.span.end.offset,
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn parse(
    pattern: &str,
    options: RegexOptions,
    limits: RegexLimits,
) -> Result<(Hir, Vec<Option<String>>, hir::ClassUnicode), RegexError> {
    admit(pattern, limits)?;
    // Depth is admitted before parse; this also bounds the dependency's final AST check.
    let mut tree = ast::parse::ParserBuilder::new()
        .nest_limit(
            u32::try_from(limits.max_syntax_depth.saturating_mul(3).saturating_add(8))
                .unwrap_or(u32::MAX),
        )
        .build()
        .parse(pattern)
        .map_err(ast_error)?;
    let mut names = Vec::new();
    reserve(
        &mut names,
        limits
            .max_capture_groups
            .min(pattern.len())
            .saturating_add(1),
        RegexPhase::Compile,
        0,
    )?;
    names.push(None);
    let mut pending = vec![&mut tree];
    let mut range_count = 0usize;
    let mut needs_word_boundary = false;
    while let Some(node) = pending.pop() {
        match node {
            Ast::ClassPerl(_) | Ast::ClassUnicode(_) | Ast::ClassBracketed(_) => {
                if let Ast::ClassPerl(perl) = node
                    && perl.kind == ast::ClassPerlKind::Word
                {
                    *node = Ast::ClassBracketed(Box::new(word_ast(perl.span, perl.negated)?));
                } else if let Ast::ClassBracketed(class) = node {
                    class_items(class)?;
                }
                // Admit each materialized class before translating the whole pattern.
                // A single UCD lookup is table-bounded; cumulative class storage is not.
                let class = hir::translate::TranslatorBuilder::new()
                    .case_insensitive(options.case_insensitive)
                    .build()
                    .translate(pattern, node)
                    .map_err(|value| {
                        error(
                            Kind::InvalidUnicodeProperty,
                            value.span().start.offset,
                            value.span().end.offset,
                        )
                    })?;
                if let HirKind::Class(hir::Class::Unicode(class)) = class.kind() {
                    range_count = range_count.saturating_add(class.ranges().len());
                    if range_count > limits.max_class_ranges {
                        return Err(limit(
                            Kind::ProgramLimitExceeded,
                            node.span().start.offset,
                            "max_class_ranges",
                        ));
                    }
                    if range_count
                        .saturating_mul(std::mem::size_of::<hir::ClassUnicodeRange>())
                        .saturating_add(pattern.len().saturating_mul(256))
                        > limits.vm_heap
                    {
                        return Err(limit(
                            Kind::OutOfMemory,
                            node.span().start.offset,
                            "vm_heap",
                        ));
                    }
                }
            }
            Ast::Group(group) => {
                if let Some(index) = group.capture_index() {
                    names.resize_with(names.len().max(index as usize + 1), || None);
                    if let ast::GroupKind::CaptureName { name, .. } = &group.kind {
                        names[index as usize] = Some(copy_text(
                            &name.name,
                            RegexPhase::Compile,
                            name.span.start.offset,
                        )?);
                    }
                }
                pending.push(&mut group.ast);
            }
            Ast::Repetition(repetition) => {
                if matches!(*repetition.ast, Ast::Repetition(_)) {
                    return Err(error(
                        Kind::InvalidQuantifier,
                        repetition.span.start.offset,
                        repetition.span.end.offset,
                    ));
                }
                if matches!(
                    repetition.op.kind,
                    ast::RepetitionKind::Range(
                        ast::RepetitionRange::Exactly(0) | ast::RepetitionRange::Bounded(0, 0)
                    )
                ) {
                    let mut atom = &*repetition.ast;
                    while let Ast::Group(group) = atom {
                        atom = &group.ast;
                    }
                    if atom.is_empty() {
                        return Err(error(
                            Kind::InvalidQuantifier,
                            repetition.span.start.offset,
                            repetition.span.end.offset,
                        ));
                    }
                }
                pending.push(&mut repetition.ast);
            }
            Ast::Alternation(alternation) => pending.extend(alternation.asts.iter_mut()),
            Ast::Concat(concat) => pending.extend(concat.asts.iter_mut()),
            Ast::Assertion(assertion) if assertion.kind == ast::AssertionKind::WordBoundary => {
                needs_word_boundary = true;
            }
            _ => {}
        }
    }
    let parsed = hir::translate::TranslatorBuilder::new()
        .case_insensitive(options.case_insensitive)
        .multi_line(options.multi_line)
        .dot_matches_new_line(options.dot_matches_newline)
        .crlf(options.crlf)
        .swap_greed(options.ungreedy)
        .build()
        .translate(pattern, &tree)
        .map_err(|value| {
            error(
                Kind::InvalidUnicodeProperty,
                value.span().start.offset,
                value.span().end.offset,
            )
        })?;
    let word = if needs_word_boundary {
        let parsed_word = regex_syntax::Parser::new()
            .parse(WORD)
            .map_err(|_| error(Kind::InvalidUnicodeProperty, 0, 0))?;
        let HirKind::Class(hir::Class::Unicode(word)) = parsed_word.into_kind() else {
            unreachable!("fixed Unicode word class");
        };
        word
    } else {
        hir::ClassUnicode::new([])
    };
    Ok((parsed, names, word))
}
