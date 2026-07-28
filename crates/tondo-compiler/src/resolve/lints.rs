use std::collections::{BTreeMap, BTreeSet};

use unicode_security::skeleton;

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::Namespace;
use crate::source::{SourceDatabase, Span};

use super::{
    LocalKind, MemberKind, ResolveError, ResolvedEntity, ResolvedName, ResolvedProgram, SymbolKind,
};

/// Emits the closed `core` warning profile after successful semantic checking.
///
/// Resolution has already normalized every identifier to NFC. Confusable
/// skeletons therefore use exactly the Unicode 16.0.0 UTS #39 tables pinned by
/// the compiler dependency.
pub fn lint_core(
    sources: &SourceDatabase,
    program: &ResolvedProgram,
    max_diagnostics: usize,
) -> Result<Vec<Diagnostic>, ResolveError> {
    debug_assert_eq!(unicode_security::UNICODE_VERSION, (16, 0, 0));
    let mut diagnostics = Vec::new();
    lint_unused_imports(sources, program, max_diagnostics, &mut diagnostics)?;
    lint_unused_locals(program, max_diagnostics, &mut diagnostics)?;
    lint_names(program, max_diagnostics, &mut diagnostics)?;
    lint_confusables(program, max_diagnostics, &mut diagnostics)?;
    Ok(diagnostics)
}

fn lint_unused_imports(
    sources: &SourceDatabase,
    program: &ResolvedProgram,
    max: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ResolveError> {
    let mut used = BTreeSet::new();
    for reference in program.references() {
        let ResolvedEntity::Module(module) = reference.entity() else {
            continue;
        };
        let file = sources.get(reference.file())?;
        let Ok(text) = file.text() else {
            continue;
        };
        let range = reference.range();
        let spelling = &text[range.start() as usize..range.end() as usize];
        used.insert((reference.file(), module.clone(), spelling.to_owned()));
    }
    for (file, resolution) in &program.files {
        for import in resolution.imports().values() {
            if !used.contains(&(
                *file,
                import.module().clone(),
                import.alias().as_str().to_owned(),
            )) {
                push(
                    diagnostics,
                    max,
                    Diagnostic::new(
                        Severity::Warning,
                        DiagnosticCode::new("W1001")?,
                        format!("import `{}` is never used", import.alias()),
                        PrimaryLocation::Source(import.span()),
                    )?,
                    import.span(),
                )?;
            }
        }
    }
    Ok(())
}

fn lint_unused_locals(
    program: &ResolvedProgram,
    max: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ResolveError> {
    let used = program
        .references()
        .filter_map(|reference| match reference.entity() {
            ResolvedEntity::Name(ResolvedName::Local(local)) => Some(*local),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for local in program.locals().filter(|local| !used.contains(&local.id())) {
        let (code, noun) = match local.kind() {
            LocalKind::Parameter | LocalKind::ClosureParameter => ("W1003", "parameter"),
            LocalKind::Binding | LocalKind::Pattern | LocalKind::ForPattern => ("W1002", "binding"),
            LocalKind::GenericParameter => continue,
        };
        push(
            diagnostics,
            max,
            Diagnostic::new(
                Severity::Warning,
                DiagnosticCode::new(code)?,
                format!("{noun} `{}` is never used", local.name()),
                PrimaryLocation::Source(local.span()),
            )?,
            local.span(),
        )?;
    }
    Ok(())
}

fn lint_names(
    program: &ResolvedProgram,
    max: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ResolveError> {
    for symbol in program.symbols().filter(|symbol| !symbol.is_synthetic()) {
        let convention = match symbol.kind() {
            SymbolKind::Function => NameConvention::Camel,
            SymbolKind::Constant
            | SymbolKind::Type
            | SymbolKind::Alias
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::NewtypeConstructor => NameConvention::Pascal,
        };
        lint_name(
            symbol.name().as_str(),
            symbol.span(),
            convention,
            max,
            diagnostics,
        )?;
    }
    for member in program.members().filter(|member| !member.is_synthetic()) {
        let convention = if member.kind() == MemberKind::EnumVariant {
            NameConvention::Pascal
        } else {
            NameConvention::Camel
        };
        lint_name(
            member.name().as_str(),
            member.span(),
            convention,
            max,
            diagnostics,
        )?;
    }
    for local in program.locals() {
        let convention = if local.kind() == LocalKind::GenericParameter {
            NameConvention::Pascal
        } else {
            NameConvention::Camel
        };
        lint_name(
            local.name().as_str(),
            local.span(),
            convention,
            max,
            diagnostics,
        )?;
    }
    for resolution in program.files.values() {
        for import in resolution.imports().values() {
            lint_name(
                import.alias().as_str(),
                import.span(),
                NameConvention::Camel,
                max,
                diagnostics,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum NameConvention {
    Camel,
    Pascal,
}

fn lint_name(
    name: &str,
    span: Span,
    convention: NameConvention,
    max: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ResolveError> {
    let valid = match convention {
        NameConvention::Camel => is_camel_case(name),
        NameConvention::Pascal => is_pascal_case(name),
    };
    if valid {
        return Ok(());
    }
    let expected = match convention {
        NameConvention::Camel => "camelCase",
        NameConvention::Pascal => "PascalCase",
    };
    push(
        diagnostics,
        max,
        Diagnostic::new(
            Severity::Warning,
            DiagnosticCode::new("W1004")?,
            format!("`{name}` does not follow {expected} naming"),
            PrimaryLocation::Source(span),
        )?,
        span,
    )
}

fn is_camel_case(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_lowercase) && has_canonical_word_shape(name)
}

fn is_pascal_case(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase) && has_canonical_word_shape(name)
}

fn has_canonical_word_shape(name: &str) -> bool {
    if name.contains('_') {
        return false;
    }
    let mut previous_upper = false;
    for character in name.chars() {
        let upper = character.is_uppercase();
        if upper && previous_upper {
            return false;
        }
        previous_upper = upper;
    }
    true
}

#[derive(Clone)]
struct ConfusableCandidate {
    group: String,
    name: String,
    span: Span,
}

fn lint_confusables(
    program: &ResolvedProgram,
    max: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ResolveError> {
    let mut candidates = Vec::new();
    for symbol in program.symbols().filter(|symbol| !symbol.is_synthetic()) {
        candidates.push(ConfusableCandidate {
            group: format!(
                "symbol:{}:{}",
                symbol.identity().module(),
                symbol.identity().namespace()
            ),
            name: symbol.name().as_str().to_owned(),
            span: symbol.span(),
        });
    }
    for member in program.members().filter(|member| !member.is_synthetic()) {
        candidates.push(ConfusableCandidate {
            group: format!("member:{:?}", member.owner()),
            name: member.name().as_str().to_owned(),
            span: member.span(),
        });
    }
    for local in program.locals() {
        let namespace = if local.kind() == LocalKind::GenericParameter {
            Namespace::Type
        } else {
            Namespace::Value
        };
        candidates.push(ConfusableCandidate {
            group: format!(
                "local:{}:{}:{}",
                local.span().file().index(),
                local.scope(),
                namespace
            ),
            name: local.name().as_str().to_owned(),
            span: local.span(),
        });
    }
    for (file, resolution) in &program.files {
        for import in resolution.imports().values() {
            candidates.push(ConfusableCandidate {
                group: format!("import:{}", file.index()),
                name: import.alias().as_str().to_owned(),
                span: import.span(),
            });
        }
    }
    candidates.sort_by_key(|candidate| {
        (
            candidate.span.file(),
            candidate.span.range().start(),
            candidate.span.range().end(),
            candidate.group.clone(),
            candidate.name.clone(),
        )
    });

    let mut first: BTreeMap<(String, String), (String, Span)> = BTreeMap::new();
    for candidate in candidates {
        let key = (
            candidate.group,
            skeleton(&candidate.name).collect::<String>(),
        );
        if let Some((previous_name, previous_span)) = first.get(&key) {
            if previous_name != &candidate.name {
                let warning = Diagnostic::new(
                    Severity::Warning,
                    DiagnosticCode::new("W1005")?,
                    format!(
                        "`{}` is visually confusable with `{previous_name}` in the same scope",
                        candidate.name
                    ),
                    PrimaryLocation::Source(candidate.span),
                )?
                .with_related(Related::new(
                    "confusable identifier declared here",
                    *previous_span,
                )?);
                push(diagnostics, max, warning, candidate.span)?;
            }
        } else {
            first.insert(key, (candidate.name, candidate.span));
        }
    }
    Ok(())
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    max: usize,
    diagnostic: Diagnostic,
    span: Span,
) -> Result<(), ResolveError> {
    if diagnostics.len() >= max {
        return Err(ResolveError::DiagnosticLimit {
            file: span.file(),
            offset: span.range().start(),
        });
    }
    diagnostics.push(diagnostic);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{has_canonical_word_shape, is_camel_case, is_pascal_case};

    #[test]
    fn canonical_names_treat_acronyms_as_words() {
        assert!(is_pascal_case("HttpClient"));
        assert!(is_pascal_case("JsonValue"));
        assert!(is_camel_case("userId"));
        assert!(!is_pascal_case("HTTPClient"));
        assert!(!is_pascal_case("JSONValue"));
        assert!(!is_camel_case("userID"));
        assert!(!has_canonical_word_shape("snake_case"));
    }
}
