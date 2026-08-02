//! The bytecode backend for one Tondo `test` entry.
//!
//! Test declarations are a source-level construct, not host-language
//! callbacks.  The backend therefore lowers the selected declaration to an
//! ordinary private `main` entry and sends that entry through the same
//! resolver, HIR, MIR, bytecode and VM pipeline used by `tondo run`.  This is
//! intentionally small: the test runner can add its envelope around this
//! entry without creating a second language implementation.

use std::error::Error;
use std::fmt;

use crate::package::PackageId;
use crate::source::{FileId, SourceDatabase, SourceError};
use crate::syntax::ast::{Declaration, FunctionDecl, SourceFile};
use crate::syntax::{Cst, SyntaxKind, TokenKind};

/// A discovered test declaration and its enclosing suite setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEntry {
    file: FileId,
    logical_path: String,
    id: String,
    name: String,
    body: Vec<u8>,
    setup: Vec<Vec<u8>>,
    requires_async: bool,
}

impl TestEntry {
    pub fn file(&self) -> FileId {
        self.file
    }

    /// Canonical source path used for ownership matching and report metadata.
    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }

    pub(crate) fn setup(&self) -> &[Vec<u8>] {
        &self.setup
    }
}

/// Errors raised while discovering or lowering a test entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestBackendError {
    Source(SourceError),
    NoTests,
    MissingEntry(String),
    AmbiguousEntry(String),
    ProductionMain,
    InvalidBody(String),
}

impl fmt::Display for TestBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::NoTests => formatter.write_str("the test target contains no test declarations"),
            Self::MissingEntry(entry) => write!(formatter, "test entry `{entry}` was not found"),
            Self::AmbiguousEntry(entry) => {
                write!(
                    formatter,
                    "test entry selector `{entry}` matches more than one test"
                )
            }
            Self::ProductionMain => {
                formatter.write_str("a test target cannot declare a `main` entry point")
            }
            Self::InvalidBody(message) => write!(formatter, "invalid test body: {message}"),
        }
    }
}

impl Error for TestBackendError {}

impl From<SourceError> for TestBackendError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

/// Discovers test declarations in a parsed root module.
pub fn discover(
    sources: &SourceDatabase,
    file: FileId,
    cst: &Cst,
    _package: &PackageId,
    package_name: &str,
) -> Result<Vec<TestEntry>, TestBackendError> {
    let source = sources.get(file)?;
    let root = SourceFile::root(cst)
        .ok_or_else(|| TestBackendError::InvalidBody("missing module root".into()))?;
    let mut entries = Vec::new();
    let mut parents = Vec::new();
    let mut setup = Vec::new();
    let mut setup_async = Vec::new();
    visit_declarations(
        file,
        source.bytes(),
        root.declarations(),
        package_name,
        source.path().as_str(),
        source.module().as_str(),
        &mut parents,
        &mut setup,
        &mut setup_async,
        &mut entries,
    )?;
    Ok(entries)
}

/// Produces a complete ordinary module whose `main` body is the selected
/// test.  All non-test declarations and imports remain available, while
/// suites contribute setup statements in declaration order.
pub fn lower_selected(
    sources: &SourceDatabase,
    file: FileId,
    cst: &Cst,
    package: &PackageId,
    package_name: &str,
    selector: Option<&str>,
) -> Result<Vec<u8>, TestBackendError> {
    let entries = discover(sources, file, cst, package, package_name)?;
    let selected = match selector {
        Some(selector) => {
            let matches = entries
                .iter()
                .filter(|entry| entry.id == selector || entry.name == selector)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => return Err(TestBackendError::MissingEntry(selector.into())),
                [entry] => (*entry).clone(),
                _ => return Err(TestBackendError::AmbiguousEntry(selector.into())),
            }
        }
        None => match entries.as_slice() {
            [] => return Err(TestBackendError::NoTests),
            [entry] => entry.clone(),
            _ => {
                return Err(TestBackendError::AmbiguousEntry("<all>".into()));
            }
        },
    };

    let source = sources.get(file)?;
    let root = cst.root_node();
    let mut output = Vec::with_capacity(source.bytes().len() + 128);
    let mut saw_main = false;
    for node in root.child_nodes() {
        match node.kind() {
            SyntaxKind::TestDecl | SyntaxKind::SuiteDecl => {}
            SyntaxKind::FunctionDecl => {
                if FunctionDecl::cast(node)
                    .and_then(|function| function.head())
                    .and_then(|head| head.name_token())
                    .and_then(|token| token.token().normalized_identifier())
                    == Some("main")
                {
                    saw_main = true;
                }
                append_node(&mut output, source.bytes(), node.range());
            }
            _ => append_node(&mut output, source.bytes(), node.range()),
        }
        output.extend_from_slice(b"\n");
    }
    if saw_main {
        return Err(TestBackendError::ProductionMain);
    }
    if selected.requires_async {
        output.extend_from_slice(b"async fn main() {\n");
    } else {
        output.extend_from_slice(b"fn main() {\n");
    }
    for statement in selected.setup() {
        output.extend_from_slice(statement);
        output.extend_from_slice(b"\n");
    }
    output.extend_from_slice(selected.body());
    output.extend_from_slice(b"\n}\n");
    Ok(output)
}

fn append_node(output: &mut Vec<u8>, source: &[u8], range: crate::source::TextRange) {
    let start = range.start() as usize;
    let end = range.end() as usize;
    if start < end && end <= source.len() {
        output.extend_from_slice(&source[start..end]);
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_declarations<'a>(
    file: FileId,
    source: &[u8],
    declarations: impl Iterator<Item = Declaration<'a>>,
    package_name: &str,
    logical_path: &str,
    module: &str,
    parents: &mut Vec<String>,
    setup: &mut Vec<Vec<u8>>,
    setup_async: &mut Vec<bool>,
    entries: &mut Vec<TestEntry>,
) -> Result<(), TestBackendError> {
    for declaration in declarations {
        match declaration {
            Declaration::Test(test) => {
                let name = test
                    .name_token()
                    .and_then(|token| token.token().normalized_identifier())
                    .ok_or_else(|| {
                        TestBackendError::InvalidBody("test declaration has no name".into())
                    })?;
                let body = test
                    .body()
                    .ok_or_else(|| TestBackendError::InvalidBody("test has no body".into()))?;
                let source_class = if logical_path == "tests" || logical_path.starts_with("tests/")
                {
                    "integration"
                } else {
                    "unit"
                };
                let id = test_id(package_name, source_class, module, parents, name);
                entries.push(TestEntry {
                    file,
                    logical_path: logical_path.to_owned(),
                    id,
                    name: name.to_owned(),
                    body: block_contents(source, body.syntax().range())?,
                    setup: setup.clone(),
                    requires_async: setup_async.iter().any(|requires| *requires)
                        || body
                            .syntax()
                            .descendant_tokens()
                            .any(|token| token.kind() == TokenKind::Await),
                });
            }
            Declaration::Suite(suite) => {
                let name = suite
                    .name_token()
                    .and_then(|token| token.token().normalized_identifier())
                    .ok_or_else(|| {
                        TestBackendError::InvalidBody("suite declaration has no name".into())
                    })?;
                let body = suite
                    .body()
                    .ok_or_else(|| TestBackendError::InvalidBody("suite has no body".into()))?;
                parents.push(name.to_owned());
                for statement in body.setup() {
                    let range = statement.syntax().range();
                    setup.push(slice(source, range)?.to_vec());
                    setup_async.push(
                        statement
                            .syntax()
                            .descendant_tokens()
                            .any(|token| token.kind() == TokenKind::Await),
                    );
                }
                visit_declarations(
                    file,
                    source,
                    body.members(),
                    package_name,
                    logical_path,
                    module,
                    parents,
                    setup,
                    setup_async,
                    entries,
                )?;
                for _ in body.setup() {
                    setup.pop();
                    setup_async.pop();
                }
                parents.pop();
            }
            _ => {}
        }
    }
    Ok(())
}

fn test_id(
    package_name: &str,
    source_class: &str,
    module: &str,
    parents: &[String],
    name: &str,
) -> String {
    let mut id = format!("{package_name}::{source_class}::{module}");
    for parent in parents {
        id.push_str("::");
        id.push_str(parent);
    }
    id.push_str("::");
    id.push_str(name);
    id
}

fn slice(source: &[u8], range: crate::source::TextRange) -> Result<&[u8], TestBackendError> {
    let start = range.start() as usize;
    let end = range.end() as usize;
    source
        .get(start..end)
        .ok_or_else(|| TestBackendError::InvalidBody(format!("range {range} is outside source")))
}

fn block_contents(
    source: &[u8],
    range: crate::source::TextRange,
) -> Result<Vec<u8>, TestBackendError> {
    let bytes = slice(source, range)?;
    let open = bytes
        .iter()
        .position(|byte| *byte == b'{')
        .ok_or_else(|| TestBackendError::InvalidBody("test body has no opening brace".into()))?;
    let close = bytes
        .iter()
        .rposition(|byte| *byte == b'}')
        .ok_or_else(|| TestBackendError::InvalidBody("test body has no closing brace".into()))?;
    if close <= open {
        return Err(TestBackendError::InvalidBody(
            "test body braces are inverted".into(),
        ));
    }
    Ok(bytes[open + 1..close].to_vec())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseMode, lex, parse};

    fn parsed(source: &[u8]) -> (SourceDatabase, FileId, crate::syntax::Parsed, PackageId) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test-backend").unwrap(),
                ModulePath::new("tests").unwrap(),
                LogicalPath::new("tests.to").unwrap(),
                Arc::<[u8]>::from(source),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        let parsed = parse(&sources, file, lexed, ParseMode::Module, Default::default()).unwrap();
        let package = PackageId::new("root:test-backend").unwrap();
        (sources, file, parsed, package)
    }

    #[test]
    fn lowers_top_level_test_to_real_main_body() {
        let (sources, file, parsed, package) =
            parsed(b"import std.console\n\ntest smoke { assert(true) }\n");
        let output = lower_selected(&sources, file, parsed.cst(), &package, "main", None).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("fn main()"));
        assert!(text.contains("assert(true)"));
        assert!(!text.contains("test smoke"));
    }

    #[test]
    fn suite_setup_is_inlined_before_the_child_body() {
        let source = b"suite arithmetic { let offset = 2\n test adds { assert(offset == 2) } }\n";
        let (sources, file, parsed, package) = parsed(source);
        let entries = discover(&sources, file, parsed.cst(), &package, "main").unwrap();
        assert_eq!(entries.len(), 1);
        let output = lower_selected(
            &sources,
            file,
            parsed.cst(),
            &package,
            "main",
            Some(entries[0].id()),
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.find("let offset").unwrap() < text.find("assert(offset").unwrap());
    }

    #[test]
    fn async_is_inferred_from_suite_setup_or_test_body() {
        for source in [
            b"async fn value(): Int { 1 }\nsuite service { let item = await value()\n test reads { assert(item == 1) } }\n".as_slice(),
            b"async fn value(): Int { 1 }\ntest reads { assert(await value() == 1) }\n".as_slice(),
        ] {
            let (sources, file, parsed, package) = parsed(source);
            let output =
                lower_selected(&sources, file, parsed.cst(), &package, "main", None).unwrap();
            assert!(String::from_utf8(output)
                .unwrap()
                .contains("async fn main()"));
        }
    }

    #[test]
    fn rejects_a_production_main_in_a_test_target() {
        let source = b"fn main() {}\ntest smoke { assert(true) }\n";
        let (sources, file, parsed, package) = parsed(source);
        let error = lower_selected(&sources, file, parsed.cst(), &package, "main", None)
            .expect_err("main must be rejected by the test backend");
        assert_eq!(error, TestBackendError::ProductionMain);
    }
}
