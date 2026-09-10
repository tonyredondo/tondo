//! Compare authorized derive headers and locate independently removable bounds.

use crate::meta_derive::DeriveExecutionError;
use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, TextRange};
use crate::syntax::{
    LexMode, ParseLimits, ParseMode, SyntaxKind, SyntaxNodeRef, TokenKind, lex, parse,
};

#[derive(Debug, Clone)]
pub(crate) struct IntroducedBound {
    pub identity: String,
    pub removal: TextRange,
}

struct Bound {
    name: String,
    removal: TextRange,
}

struct Binder {
    name: String,
    bounds: Vec<Bound>,
}

struct Header {
    base: String,
    binders: Vec<Binder>,
}

fn token_text(node: SyntaxNodeRef<'_>, bytes: &[u8]) -> Result<String, DeriveExecutionError> {
    let mut result = String::new();
    for token in node
        .descendant_tokens()
        .filter(|token| !token.kind().is_trivia() && token.kind() != TokenKind::Nl)
    {
        let range = token.range();
        let text = std::str::from_utf8(&bytes[range.start() as usize..range.end() as usize])
            .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
        result.push_str(token.token().normalized_identifier().unwrap_or(text));
    }
    Ok(result)
}

fn header(bytes: &[u8]) -> Result<Header, DeriveExecutionError> {
    let invalid = || DeriveExecutionError::InvalidProviderBody;
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("meta:derive-header").map_err(|_| invalid())?,
            ModulePath::new("generated").map_err(|_| invalid())?,
            LogicalPath::new("generated/derive.to").map_err(|_| invalid())?,
            bytes,
        ))
        .map_err(|_| invalid())?;
    let lexed = lex(&sources, file, LexMode::Module).map_err(|_| invalid())?;
    if !lexed.diagnostics().is_empty() {
        return Err(invalid());
    }
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .map_err(|_| invalid())?;
    if !parsed.diagnostics().is_empty() {
        return Err(invalid());
    }
    let mut implementations = parsed
        .cst()
        .root_node()
        .child_nodes()
        .filter(|node| node.kind() == SyntaxKind::ImplDecl);
    let implementation = implementations.next().ok_or_else(invalid)?;
    if implementations.next().is_some() {
        return Err(invalid());
    }
    let parameters = implementation
        .child_nodes()
        .find(|node| node.kind() == SyntaxKind::GenericParams);
    let mut binders = Vec::new();
    if let Some(parameters) = parameters {
        for parameter in parameters
            .child_nodes()
            .filter(|node| node.kind() == SyntaxKind::GenericParam)
        {
            let name = parameter
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
                .and_then(|token| token.token().normalized_identifier())
                .ok_or_else(invalid)?
                .to_owned();
            let mut bounds = Vec::new();
            if let Some(group) = parameter
                .child_nodes()
                .find(|node| node.kind() == SyntaxKind::GenericBound)
            {
                let paths = group
                    .child_nodes()
                    .filter(|node| node.kind() == SyntaxKind::TypePath)
                    .collect::<Vec<_>>();
                for (index, path) in paths.iter().enumerate() {
                    let removal = if paths.len() == 1 {
                        // Include the colon, which belongs to GenericParam.
                        let start = parameter
                            .descendant_tokens()
                            .find(|token| token.kind() == TokenKind::Colon)
                            .ok_or_else(invalid)?
                            .range()
                            .start();
                        TextRange::new(start, path.range().end())
                    } else if index == 0 {
                        TextRange::new(path.range().start(), paths[1].range().start())
                    } else {
                        TextRange::new(paths[index - 1].range().end(), path.range().end())
                    }
                    .map_err(|_| invalid())?;
                    bounds.push(Bound {
                        name: token_text(*path, bytes)?,
                        removal,
                    });
                }
            }
            binders.push(Binder { name, bounds });
        }
    }
    let mut base = String::new();
    for token in implementation
        .descendant_tokens()
        .take_while(|token| token.kind() != TokenKind::LBrace)
    {
        let range = token.range();
        if token.kind().is_trivia()
            || token.kind() == TokenKind::Nl
            || parameters.is_some_and(|group| {
                range.start() >= group.range().start() && range.end() <= group.range().end()
            })
        {
            continue;
        }
        let text = std::str::from_utf8(&bytes[range.start() as usize..range.end() as usize])
            .map_err(|_| invalid())?;
        base.push_str(token.token().normalized_identifier().unwrap_or(text));
    }
    Ok(Header { base, binders })
}

/// The target, trait and binder order remain exact. Positive bounds may extend
/// the required set; the driver proves each extension necessary by typechecking.
pub(crate) fn introduced(
    bytes: &[u8],
    expected: &str,
) -> Result<Vec<IntroducedBound>, DeriveExecutionError> {
    let actual = header(bytes)?;
    let expected = header(format!("{expected} {{}}\n").as_bytes())?;
    if actual.base != expected.base || actual.binders.len() != expected.binders.len() {
        return Err(DeriveExecutionError::InvalidProviderBody);
    }
    let mut introduced = Vec::new();
    for (actual, expected) in actual.binders.iter().zip(&expected.binders) {
        let names = actual
            .bounds
            .iter()
            .map(|bound| &bound.name)
            .collect::<std::collections::BTreeSet<_>>();
        if actual.name != expected.name
            || names.len() != actual.bounds.len()
            || expected
                .bounds
                .iter()
                .any(|bound| !names.contains(&bound.name))
        {
            return Err(DeriveExecutionError::InvalidProviderBody);
        }
        for bound in &actual.bounds {
            if !expected
                .bounds
                .iter()
                .any(|required| required.name == bound.name)
            {
                introduced.push(IntroducedBound {
                    identity: format!("{}: {}", actual.name, bound.name),
                    removal: bound.removal,
                });
            }
        }
    }
    Ok(introduced)
}
