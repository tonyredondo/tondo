//! Deterministic, compile-time discovery of the static `suite`/`test` tree.
//!
//! The parser owns lossless syntax.  This module is the next, deliberately
//! small boundary: it turns already parsed test declarations plus the closed
//! source metadata into tooling descriptors.  It never executes setup, walks
//! control flow or consults the host.  All source ordering is derived from
//! stable package/source metadata rather than `FileId` or input insertion
//! order.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::diagnostics::{
    Diagnostic, DiagnosticCode, DiagnosticError, PrimaryLocation, Related, Severity,
};
use crate::package::{Name, PackageId};
use crate::source::{
    FileId, LogicalPath, ModulePath, SourceDatabase, SourceError, Span, TextRange,
};
use crate::syntax::Cst;
use crate::syntax::ast::{Declaration, SourceFile};
use crate::test_plan::TestSourceClass;

const E2001: &str = "E2001";
const E2002: &str = "E2002";
const E2004: &str = "E2004";
const W1004: &str = "W1004";

/// The two nodes that are visible to the test tooling namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TestNodeKind {
    Suite,
    Test,
}

impl TestNodeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Suite => "suite",
            Self::Test => "test",
        }
    }
}

impl fmt::Display for TestNodeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The ordered path of a node in the source tree.
///
/// The first segment identifies the canonical source order within a
/// `(PackageId, source class, module)` group.  Each following segment is the
/// zero-based ordinal among direct `suite`/`test` members.  This makes the
/// identity independent of `FileId`, while retaining declaration order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderedNodePath(Vec<u32>);

impl OrderedNodePath {
    pub fn segments(&self) -> &[u32] {
        &self.0
    }
}

impl fmt::Display for OrderedNodePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(".")?;
            }
            write!(formatter, "{segment}")?;
        }
        Ok(())
    }
}

/// Stable semantic identity of one static test node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestNodeIdentity {
    package: PackageId,
    source_class: TestSourceClass,
    module: ModulePath,
    node_path: OrderedNodePath,
    kind: TestNodeKind,
}

impl TestNodeIdentity {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn source_class(&self) -> TestSourceClass {
        self.source_class
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn node_path(&self) -> &OrderedNodePath {
        &self.node_path
    }

    pub fn kind(&self) -> TestNodeKind {
        self.kind
    }
}

/// Parsed source metadata consumed by [`build`].
///
/// The caller supplies the already closed package/source identity and a CST
/// that was parsed for `file`.  Keeping this as a borrowed view means tree
/// construction does not copy syntax or source bytes.
#[derive(Debug, Clone, Copy)]
pub struct TestSourceInput<'a> {
    package: &'a PackageId,
    package_name: &'a str,
    source_class: TestSourceClass,
    module: &'a ModulePath,
    logical_path: &'a LogicalPath,
    file: FileId,
    cst: &'a Cst,
}

impl<'a> TestSourceInput<'a> {
    pub fn new(
        package: &'a PackageId,
        package_name: &'a str,
        source_class: TestSourceClass,
        module: &'a ModulePath,
        logical_path: &'a LogicalPath,
        file: FileId,
        cst: &'a Cst,
    ) -> Self {
        Self {
            package,
            package_name,
            source_class,
            module,
            logical_path,
            file,
            cst,
        }
    }

    pub fn package(&self) -> &PackageId {
        self.package
    }

    pub fn package_name(&self) -> &str {
        self.package_name
    }

    pub fn source_class(&self) -> TestSourceClass {
        self.source_class
    }

    pub fn module(&self) -> &ModulePath {
        self.module
    }

    pub fn logical_path(&self) -> &LogicalPath {
        self.logical_path
    }

    pub fn file(&self) -> FileId {
        self.file
    }

    pub fn cst(&self) -> &Cst {
        self.cst
    }
}

/// One immutable descriptor in the static tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestNode {
    identity: TestNodeIdentity,
    visible_id: String,
    name: String,
    parent: Option<TestNodeIdentity>,
    span: Span,
    name_span: Span,
}

impl TestNode {
    pub fn identity(&self) -> &TestNodeIdentity {
        &self.identity
    }

    pub fn visible_id(&self) -> &str {
        &self.visible_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn parent(&self) -> Option<&TestNodeIdentity> {
        self.parent.as_ref()
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name_span(&self) -> Span {
        self.name_span
    }
}

/// Result of static tree construction.  Warnings are preserved in stable
/// order so the eventual compiler/reporting pipeline can forward them without
/// rediscovering the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticTestTree {
    nodes: Vec<TestNode>,
    diagnostics: Vec<Diagnostic>,
}

impl StaticTestTree {
    pub fn nodes(&self) -> &[TestNode] {
        &self.nodes
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Errors produced while building the static tree.
#[derive(Debug)]
pub enum TestTreeError {
    Source(SourceError),
    Diagnostic(DiagnosticError),
    InvalidInput(String),
    InvalidNodeName { span: Span, message: String },
    Diagnostics(Vec<Diagnostic>),
}

impl TestTreeError {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::Diagnostics(diagnostics) => diagnostics,
            Self::Source(_)
            | Self::Diagnostic(_)
            | Self::InvalidInput(_)
            | Self::InvalidNodeName { .. } => &[],
        }
    }
}

impl fmt::Display for TestTreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::InvalidNodeName { message, .. } => formatter.write_str(message),
            Self::Diagnostics(diagnostics) => {
                write!(
                    formatter,
                    "static test tree rejected with {} diagnostic(s)",
                    diagnostics.len()
                )
            }
        }
    }
}

impl Error for TestTreeError {}

impl From<SourceError> for TestTreeError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<DiagnosticError> for TestTreeError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceOrderKey {
    package: PackageId,
    source_class: TestSourceClass,
    module: ModulePath,
    logical_path: LogicalPath,
    source_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScopeKey {
    package: PackageId,
    source_class: TestSourceClass,
    module: ModulePath,
    parent_names: Vec<String>,
}

#[derive(Debug, Clone)]
struct PendingDiagnostic {
    key: DiagnosticOrderKey,
    severity: Severity,
    code: &'static str,
    message: String,
    primary: Span,
    related: Option<(String, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DiagnosticOrderKey {
    source_id: String,
    module: String,
    logical_path: String,
    start: u32,
    end: u32,
    code: &'static str,
    message: String,
}

#[derive(Debug, Clone)]
struct BuildContext<'a> {
    sources: &'a SourceDatabase,
    nodes: Vec<TestNode>,
    pending: Vec<PendingDiagnostic>,
    siblings: BTreeMap<ScopeKey, BTreeMap<String, Span>>,
}

/// Builds the static test tree from already parsed source files.
pub fn build<'a>(
    sources: &'a SourceDatabase,
    inputs: impl IntoIterator<Item = TestSourceInput<'a>>,
) -> Result<StaticTestTree, TestTreeError> {
    let mut inputs = inputs.into_iter().collect::<Vec<_>>();
    let mut seen_files = BTreeSet::new();
    let mut entries = Vec::with_capacity(inputs.len());
    for input in inputs.drain(..) {
        if input.package_name.is_empty() || input.package_name.contains("::") {
            return Err(TestTreeError::InvalidInput(format!(
                "invalid visible package name `{}`",
                input.package_name
            )));
        }
        if !seen_files.insert(input.file) {
            return Err(TestTreeError::InvalidInput(format!(
                "source file {} is registered more than once",
                input.file
            )));
        }
        let source = sources.get(input.file)?;
        if source.module() != input.module || source.path() != input.logical_path {
            return Err(TestTreeError::InvalidInput(format!(
                "test source metadata does not match source file {}",
                input.file
            )));
        }
        let key = SourceOrderKey {
            package: input.package.clone(),
            source_class: input.source_class,
            module: input.module.clone(),
            logical_path: input.logical_path.clone(),
            source_id: source.source_id().to_string(),
        };
        entries.push((key, input));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut context = BuildContext {
        sources,
        nodes: Vec::new(),
        pending: Vec::new(),
        siblings: BTreeMap::new(),
    };
    let mut source_ordinals = BTreeMap::<(PackageId, TestSourceClass, ModulePath), u32>::new();
    for (_, input) in &entries {
        let group = (
            input.package.clone(),
            input.source_class,
            input.module.clone(),
        );
        let source_index = source_ordinals.entry(group).or_insert(0);
        let root = SourceFile::root(input.cst)
            .ok_or_else(|| TestTreeError::InvalidInput("CST root is not a source file".into()))?;
        let mut member_index = 0_u32;
        for declaration in root.declarations() {
            let kind = match declaration {
                Declaration::Test(_) => TestNodeKind::Test,
                Declaration::Suite(_) => TestNodeKind::Suite,
                _ => continue,
            };
            let mut path = vec![*source_index, member_index];
            member_index = member_index.saturating_add(1);
            visit_declaration(&mut context, input, declaration, kind, &mut path, &[], None)?;
        }
        *source_index = source_index.saturating_add(1);
    }

    context
        .pending
        .sort_by(|left, right| left.key.cmp(&right.key));
    let diagnostics = context
        .pending
        .into_iter()
        .map(materialize_diagnostic)
        .collect::<Result<Vec<_>, _>>()?;
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == Severity::Error)
    {
        return Err(TestTreeError::Diagnostics(diagnostics));
    }
    Ok(StaticTestTree {
        nodes: context.nodes,
        diagnostics,
    })
}

fn visit_declaration<'a>(
    context: &mut BuildContext<'a>,
    input: &TestSourceInput<'a>,
    declaration: Declaration<'a>,
    kind: TestNodeKind,
    path: &mut Vec<u32>,
    parent_names: &[String],
    parent: Option<TestNodeIdentity>,
) -> Result<(), TestTreeError> {
    let (name_token, syntax, children) = match declaration {
        Declaration::Test(test) => {
            let name_token = test.name_token().ok_or_else(|| {
                TestTreeError::InvalidInput("test declaration has no name".into())
            })?;
            (name_token, test.syntax(), Vec::new())
        }
        Declaration::Suite(suite) => {
            let name_token = suite.name_token().ok_or_else(|| {
                TestTreeError::InvalidInput("suite declaration has no name".into())
            })?;
            let body = suite.body().ok_or_else(|| {
                TestTreeError::InvalidInput("suite declaration has no body".into())
            })?;
            (
                name_token,
                suite.syntax(),
                body.members().collect::<Vec<_>>(),
            )
        }
        _ => return Ok(()),
    };
    let name_span = context.sources.span(input.file, name_token.range())?;
    let name = node_name(context.sources, input.file, name_token.range(), name_span)?;
    let span = context.sources.span(input.file, syntax.range())?;
    let mut scope_path = parent_names.to_vec();
    let scope = ScopeKey {
        package: input.package.clone(),
        source_class: input.source_class,
        module: input.module.clone(),
        parent_names: scope_path.clone(),
    };
    if let Some(previous) = context
        .siblings
        .entry(scope)
        .or_default()
        .insert(name.clone(), name_span)
    {
        context.pending.push(PendingDiagnostic {
            key: diagnostic_key(context.sources, input.file, name_span.range(), E2002, &name),
            severity: Severity::Error,
            code: E2002,
            message: format!(
                "test node `{name}` duplicates a sibling; suites cannot be reopened or merged"
            ),
            primary: name_span,
            related: Some(("first sibling with this name".into(), previous)),
        });
    }
    if input.source_class == TestSourceClass::Production {
        context.pending.push(PendingDiagnostic {
            key: diagnostic_key(context.sources, input.file, span.range(), E2001, &name),
            severity: Severity::Error,
            code: E2001,
            message: "suite and test declarations are only allowed in test sources".into(),
            primary: span,
            related: None,
        });
    }
    if !is_camel_case(&name) {
        context.pending.push(PendingDiagnostic {
            key: diagnostic_key(context.sources, input.file, name_span.range(), W1004, &name),
            severity: Severity::Warning,
            code: W1004,
            message: format!("`{name}` does not follow camelCase naming"),
            primary: name_span,
            related: None,
        });
    }

    let identity = TestNodeIdentity {
        package: input.package.clone(),
        source_class: input.source_class,
        module: input.module.clone(),
        node_path: OrderedNodePath(path.clone()),
        kind,
    };
    let visible_id = visible_id(input, &scope_path, &name);
    context.nodes.push(TestNode {
        identity: identity.clone(),
        visible_id,
        name: name.clone(),
        parent,
        span,
        name_span,
    });

    if kind == TestNodeKind::Suite {
        if children.is_empty() {
            context.pending.push(PendingDiagnostic {
                key: diagnostic_key(context.sources, input.file, span.range(), E2004, &name),
                severity: Severity::Error,
                code: E2004,
                message: format!(
                    "suite `{name}` must contain at least one direct test or suite member"
                ),
                primary: span,
                related: None,
            });
        }
        scope_path.push(name);
        for (index, child) in children.into_iter().enumerate() {
            let child_kind = match child {
                Declaration::Test(_) => TestNodeKind::Test,
                Declaration::Suite(_) => TestNodeKind::Suite,
                _ => continue,
            };
            path.push(u32::try_from(index).expect("suite member count fits u32"));
            visit_declaration(
                context,
                input,
                child,
                child_kind,
                path,
                &scope_path,
                Some(identity.clone()),
            )?;
            path.pop();
        }
    }
    Ok(())
}

fn node_name(
    sources: &SourceDatabase,
    file: FileId,
    range: TextRange,
    span: Span,
) -> Result<String, TestTreeError> {
    let source = sources.get(file)?;
    let bytes = source
        .bytes()
        .get(range.start() as usize..range.end() as usize)
        .ok_or_else(|| {
            TestTreeError::InvalidInput(format!("node name range {range} is outside source"))
        })?;
    let name = std::str::from_utf8(bytes).map_err(|_| TestTreeError::InvalidNodeName {
        span,
        message: "test node name is not valid UTF-8".into(),
    })?;
    Name::new(name)
        .map(|name| name.to_string())
        .map_err(|error| TestTreeError::InvalidNodeName {
            span,
            message: format!("invalid test node name: {error}"),
        })
}

fn visible_id(input: &TestSourceInput<'_>, parent_names: &[String], name: &str) -> String {
    let source_kind = match input.source_class {
        TestSourceClass::UnitTest => "unit",
        TestSourceClass::IntegrationTest => "integration",
        TestSourceClass::Production => "production",
    };
    let module = match input.source_class {
        TestSourceClass::IntegrationTest => integration_module_path(input.logical_path()),
        TestSourceClass::Production | TestSourceClass::UnitTest => input.module.to_string(),
    };
    let mut id = format!("{}::{source_kind}::{module}", input.package_name);
    for parent in parent_names {
        id.push_str("::");
        id.push_str(parent);
    }
    id.push_str("::");
    id.push_str(name);
    id
}

fn integration_module_path(path: &LogicalPath) -> String {
    let mut path = path.as_str();
    if let Some(relative) = path.strip_prefix("tests/") {
        path = relative;
    }
    if let Some(without_extension) = path.strip_suffix(".to") {
        path = without_extension;
    }
    path.replace('/', ".")
}

fn diagnostic_key(
    sources: &SourceDatabase,
    file: FileId,
    range: TextRange,
    code: &'static str,
    message_key: &str,
) -> DiagnosticOrderKey {
    let source = sources
        .get(file)
        .expect("diagnostic source was validated before construction");
    DiagnosticOrderKey {
        source_id: source.source_id().to_string(),
        module: source.module().to_string(),
        logical_path: source.path().to_string(),
        start: range.start(),
        end: range.end(),
        code,
        message: message_key.to_owned(),
    }
}

fn materialize_diagnostic(pending: PendingDiagnostic) -> Result<Diagnostic, TestTreeError> {
    let mut diagnostic = Diagnostic::new(
        pending.severity,
        DiagnosticCode::new(pending.code)?,
        pending.message,
        PrimaryLocation::Source(pending.primary),
    )?;
    if let Some((message, span)) = pending.related {
        diagnostic = diagnostic.with_related(Related::new(message, span)?);
    }
    Ok(diagnostic)
}

fn is_camel_case(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_lowercase) && has_canonical_word_shape(name)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    fn package() -> PackageId {
        PackageId::new("workspace:app@1").unwrap()
    }

    fn add_source(
        sources: &mut SourceDatabase,
        source_id: &str,
        module: &str,
        path: &str,
        text: &str,
    ) -> (FileId, crate::syntax::Parsed) {
        let file = sources
            .add(SourceInput::new(
                SourceId::new(source_id).unwrap(),
                ModulePath::new(module).unwrap(),
                LogicalPath::new(path).unwrap(),
                crate::source::SourceOrigin::Virtual,
                Arc::<[u8]>::from(text.as_bytes()),
            ))
            .unwrap();
        let lexed = lex(sources, file, LexMode::Module).unwrap();
        let parsed = parse(
            sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            parsed.diagnostics().is_empty(),
            "{}",
            parsed.diagnostics().len()
        );
        (file, parsed)
    }

    fn input<'a>(
        package: &'a PackageId,
        module: &'a ModulePath,
        path: &'a LogicalPath,
        file: FileId,
        parsed: &'a crate::syntax::Parsed,
        class: TestSourceClass,
    ) -> TestSourceInput<'a> {
        TestSourceInput::new(package, "app", class, module, path, file, parsed.cst())
    }

    fn codes(error: &TestTreeError) -> Vec<&str> {
        error
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    #[test]
    fn builds_nested_nodes_with_identity_visible_ids_and_parents() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:test",
            "math",
            "src/math_test.to",
            "suite arithmetic {\n    test adds {}\n    suite negatives {\n        test handlesZero {}\n    }\n}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        let tree = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::UnitTest,
            )],
        )
        .unwrap();

        assert_eq!(tree.nodes().len(), 4);
        assert_eq!(tree.nodes()[0].visible_id(), "app::unit::math::arithmetic");
        assert_eq!(
            tree.nodes()[1].visible_id(),
            "app::unit::math::arithmetic::adds"
        );
        assert_eq!(
            tree.nodes()[2].visible_id(),
            "app::unit::math::arithmetic::negatives"
        );
        assert_eq!(
            tree.nodes()[3].visible_id(),
            "app::unit::math::arithmetic::negatives::handlesZero"
        );
        assert_eq!(tree.nodes()[1].parent(), Some(tree.nodes()[0].identity()));
        assert_eq!(
            tree.nodes()[3].identity().node_path().segments(),
            &[0, 0, 1, 0]
        );
        assert!(tree.diagnostics().is_empty());
    }

    #[test]
    fn integration_ids_use_relative_logical_paths() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "tests:smoke",
            "smoke",
            "tests/http/client.to",
            "test connects {}\n",
        );
        let package = package();
        let module = ModulePath::new("smoke").unwrap();
        let path = LogicalPath::new("tests/http/client.to").unwrap();
        let tree = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::IntegrationTest,
            )],
        )
        .unwrap();
        assert_eq!(
            tree.nodes()[0].visible_id(),
            "app::integration::http.client::connects"
        );
    }

    #[test]
    fn duplicate_siblings_across_files_are_e2002_and_cannot_reopen_suites() {
        let mut sources = SourceDatabase::new();
        let (first_file, first) = add_source(
            &mut sources,
            "src:a",
            "math",
            "src/a_test.to",
            "suite arithmetic {\n    test adds {}\n}\n",
        );
        let (second_file, second) = add_source(
            &mut sources,
            "src:b",
            "math",
            "src/b_test.to",
            "suite arithmetic {\n    test subtracts {}\n}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let first_path = LogicalPath::new("src/a_test.to").unwrap();
        let second_path = LogicalPath::new("src/b_test.to").unwrap();
        let error = build(
            &sources,
            [
                input(
                    &package,
                    &module,
                    &first_path,
                    first_file,
                    &first,
                    TestSourceClass::UnitTest,
                ),
                input(
                    &package,
                    &module,
                    &second_path,
                    second_file,
                    &second,
                    TestSourceClass::UnitTest,
                ),
            ],
        )
        .unwrap_err();
        assert_eq!(codes(&error), [E2002]);
        let duplicate = &error.diagnostics()[0];
        assert_eq!(
            duplicate.location(),
            &PrimaryLocation::Source(
                sources
                    .span(second_file, TextRange::new(6, 16).unwrap())
                    .unwrap()
            )
        );
    }

    #[test]
    fn empty_suites_are_e2004_even_when_setup_exists() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:empty",
            "math",
            "src/math_test.to",
            "suite arithmetic {\n    let value = 1\n}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        let error = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::UnitTest,
            )],
        )
        .unwrap_err();
        assert_eq!(codes(&error), [E2004]);
    }

    #[test]
    fn production_nodes_are_e2001_and_invalid_names_are_warnings() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:production",
            "math",
            "src/math.to",
            "test not_camel {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math.to").unwrap();
        let error = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::Production,
            )],
        )
        .unwrap_err();
        assert_eq!(codes(&error), [E2001, "W1004"]);
    }

    #[test]
    fn source_permutation_does_not_change_nodes_or_diagnostics() {
        let mut sources = SourceDatabase::new();
        let (first_file, first) = add_source(
            &mut sources,
            "src:a",
            "math",
            "src/a_test.to",
            "test firstTest {}\n",
        );
        let (second_file, second) = add_source(
            &mut sources,
            "src:b",
            "math",
            "src/b_test.to",
            "test secondTest {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let first_path = LogicalPath::new("src/a_test.to").unwrap();
        let second_path = LogicalPath::new("src/b_test.to").unwrap();
        let a = build(
            &sources,
            [
                input(
                    &package,
                    &module,
                    &first_path,
                    first_file,
                    &first,
                    TestSourceClass::UnitTest,
                ),
                input(
                    &package,
                    &module,
                    &second_path,
                    second_file,
                    &second,
                    TestSourceClass::UnitTest,
                ),
            ],
        )
        .unwrap();
        let b = build(
            &sources,
            [
                input(
                    &package,
                    &module,
                    &second_path,
                    second_file,
                    &second,
                    TestSourceClass::UnitTest,
                ),
                input(
                    &package,
                    &module,
                    &first_path,
                    first_file,
                    &first,
                    TestSourceClass::UnitTest,
                ),
            ],
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.nodes()[0].identity().node_path().segments(), &[0, 0]);
        assert_eq!(a.nodes()[1].identity().node_path().segments(), &[1, 0]);
    }

    #[test]
    fn discarded_names_are_rejected_without_creating_an_identity() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:discard",
            "math",
            "src/math_test.to",
            "test _ {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        let error = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::UnitTest,
            )],
        )
        .unwrap_err();
        assert!(matches!(error, TestTreeError::InvalidNodeName { .. }));
    }

    #[test]
    fn public_accessors_and_empty_tree_are_stable() {
        assert_eq!(TestNodeKind::Suite.as_str(), "suite");
        assert_eq!(TestNodeKind::Test.to_string(), "test");
        let path = OrderedNodePath(vec![2, 4, 1]);
        assert_eq!(path.segments(), &[2, 4, 1]);
        assert_eq!(path.to_string(), "2.4.1");
        assert_eq!(OrderedNodePath(Vec::new()).to_string(), "");

        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:accessors",
            "math",
            "src/math.to",
            "fn helper() {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let logical_path = LogicalPath::new("src/math.to").unwrap();
        let source_input = input(
            &package,
            &module,
            &logical_path,
            file,
            &parsed,
            TestSourceClass::UnitTest,
        );
        assert_eq!(source_input.package(), &package);
        assert_eq!(source_input.package_name(), "app");
        assert_eq!(source_input.source_class(), TestSourceClass::UnitTest);
        assert_eq!(source_input.module(), &module);
        assert_eq!(source_input.logical_path(), &logical_path);
        assert_eq!(source_input.file(), file);
        assert_eq!(
            source_input.cst().root_node().kind(),
            crate::syntax::SyntaxKind::Module
        );

        let empty = build(&sources, [source_input]).unwrap();
        assert!(empty.is_empty());
        assert!(empty.nodes().is_empty());
        assert!(empty.diagnostics().is_empty());
    }

    #[test]
    fn accessors_expose_identity_nodes_and_warning_only_results() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:warning",
            "math",
            "src/math_test.to",
            "test not_camel {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        let tree = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::UnitTest,
            )],
        )
        .unwrap();
        assert_eq!(tree.diagnostics().len(), 1);
        let node = &tree.nodes()[0];
        assert_eq!(node.name(), "not_camel");
        assert_eq!(node.identity().package(), &package);
        assert_eq!(node.identity().source_class(), TestSourceClass::UnitTest);
        assert_eq!(node.identity().module(), &module);
        assert_eq!(node.identity().kind(), TestNodeKind::Test);
        assert_eq!(node.identity().node_path().segments(), &[0, 0]);
        assert_eq!(node.parent(), None);
        assert_eq!(node.span().range().start(), 0);
        assert_eq!(node.name_span().range().start(), 5);
    }

    #[test]
    fn cross_kind_and_nested_duplicate_names_are_rejected() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:duplicate",
            "math",
            "src/math_test.to",
            "test same {}\n\nsuite same {\n    test child {}\n}\n\nsuite outer {\n    test child {}\n    test child {}\n}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        let error = build(
            &sources,
            [input(
                &package,
                &module,
                &path,
                file,
                &parsed,
                TestSourceClass::UnitTest,
            )],
        )
        .unwrap_err();
        assert_eq!(codes(&error), [E2002, E2002]);
        assert!(
            error
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.message().contains("reopened or merged"))
        );
    }

    #[test]
    fn invalid_metadata_is_rejected_before_parsing_nodes() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = add_source(
            &mut sources,
            "src:metadata",
            "math",
            "src/math_test.to",
            "test works {}\n",
        );
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();

        let empty_name = TestSourceInput::new(
            &package,
            "",
            TestSourceClass::UnitTest,
            &module,
            &path,
            file,
            parsed.cst(),
        );
        assert!(matches!(
            build(&sources, [empty_name]),
            Err(TestTreeError::InvalidInput(message)) if message.contains("visible package name")
        ));

        let bad_separator = TestSourceInput::new(
            &package,
            "app::other",
            TestSourceClass::UnitTest,
            &module,
            &path,
            file,
            parsed.cst(),
        );
        assert!(matches!(
            build(&sources, [bad_separator]),
            Err(TestTreeError::InvalidInput(message)) if message.contains("visible package name")
        ));

        let duplicate = input(
            &package,
            &module,
            &path,
            file,
            &parsed,
            TestSourceClass::UnitTest,
        );
        assert!(matches!(
            build(&sources, [duplicate, duplicate]),
            Err(TestTreeError::InvalidInput(message)) if message.contains("registered more than once")
        ));

        let other_module = ModulePath::new("other").unwrap();
        let mismatched_module = input(
            &package,
            &other_module,
            &path,
            file,
            &parsed,
            TestSourceClass::UnitTest,
        );
        assert!(matches!(
            build(&sources, [mismatched_module]),
            Err(TestTreeError::InvalidInput(message)) if message.contains("metadata does not match")
        ));

        let other_path = LogicalPath::new("src/other_test.to").unwrap();
        let mismatched_path = input(
            &package,
            &module,
            &other_path,
            file,
            &parsed,
            TestSourceClass::UnitTest,
        );
        assert!(matches!(
            build(&sources, [mismatched_path]),
            Err(TestTreeError::InvalidInput(message)) if message.contains("metadata does not match")
        ));
    }

    #[test]
    fn error_surfaces_keep_diagnostics_and_display_messages_typed() {
        let source_error = TestTreeError::from(SourceError::EmptySourceId);
        assert_eq!(source_error.to_string(), "source ID cannot be empty");
        assert!(source_error.diagnostics().is_empty());

        let diagnostic_error = TestTreeError::from(DiagnosticError::InvalidCode("bad".into()));
        assert_eq!(
            diagnostic_error.to_string(),
            "invalid diagnostic code `bad`"
        );
        assert!(diagnostic_error.diagnostics().is_empty());

        let invalid_input = TestTreeError::InvalidInput("bad input".into());
        assert_eq!(invalid_input.to_string(), "bad input");
        assert!(invalid_input.diagnostics().is_empty());

        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("src:errors").unwrap(),
                ModulePath::new("errors").unwrap(),
                LogicalPath::new("src/errors.to").unwrap(),
                Arc::<[u8]>::from(&b""[..]),
            ))
            .unwrap();
        let span = sources.span(file, TextRange::empty(0)).unwrap();
        let invalid_name = TestTreeError::InvalidNodeName {
            span,
            message: "bad name".into(),
        };
        assert_eq!(invalid_name.to_string(), "bad name");
        assert!(invalid_name.diagnostics().is_empty());

        let warning = Diagnostic::new(
            Severity::Warning,
            DiagnosticCode::new(W1004).unwrap(),
            "warning",
            PrimaryLocation::Source(span),
        )
        .unwrap();
        let errors = TestTreeError::Diagnostics(vec![warning]);
        assert_eq!(
            errors.to_string(),
            "static test tree rejected with 1 diagnostic(s)"
        );
        assert_eq!(errors.diagnostics().len(), 1);
    }
}
