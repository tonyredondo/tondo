//! The bytecode backend for one Tondo `test` entry.
//!
//! Test declarations are a source-level construct, not host-language
//! callbacks.  The backend therefore lowers the selected declaration to an
//! private `__tondoTestEntry` function and sends that entry through the same
//! resolver, HIR, MIR, bytecode and VM pipeline used by `tondo run`.  This is
//! intentionally small: the test runner can add its envelope around this
//! entry without creating a second language implementation.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tondo_vm::runtime::VmPanic;

use crate::source::{FileId, SourceDatabase, SourceError};
use crate::syntax::ast::{Declaration, FunctionDecl, SourceFile};
use crate::syntax::{Cst, SyntaxKind};
use crate::test_control::{EnvelopeHandle, EnvelopeLimits, EnvelopeReport, ExecutionPhase};
use crate::test_plan::TestSourceClass;

/// Executes an invocation-owned test artifact in a fresh hosted VM. Bytecode
/// verification and VM limits apply again at this process boundary; no source
/// parsing, name resolution, type checking or lowering occurs here.
pub fn execute_compiled(
    program: &tondo_vm::bytecode::BytecodeProgram,
    entry: tondo_vm::bytecode::BytecodeFunctionId,
    limits: tondo_vm::runtime::VmLimits,
    participation: TestParticipation,
    diagnostics: Option<tondo_vm::runtime::DiagnosticConfig>,
) -> Result<tondo_vm::runtime::VmExecution, tondo_vm::runtime::VmError> {
    let mut host =
        crate::process_host::BootstrapHost::with_max_bytes(Vec::new(), limits.max_heap_bytes);
    host.install_testing_participation(participation);
    tondo_vm::runtime::execute_with_limits_and_copy_strategy_and_diagnostics(
        program,
        entry,
        &mut host,
        limits,
        tondo_vm::runtime::ValueCopyStrategy::default(),
        diagnostics,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestExecutionKind {
    Leaf,
    Suite,
}

#[derive(Debug, Clone)]
pub struct TestNodeExecution {
    pub id: String,
    pub kind: TestExecutionKind,
    pub report: EnvelopeReport,
    pub phase: ExecutionPhase,
    pub panic: Option<VmPanic>,
    pub snapshot_updates: Vec<(String, String)>,
    pub timed_out: bool,
}

/// Runner-owned phase supervision, outside the Tondo evidence envelope.
/// Implementations must keep notifications bounded and preserve node nesting.
pub trait TestPhaseObserver: std::fmt::Debug + Send + Sync {
    fn enter(&self, id: &str, kind: TestExecutionKind) -> Result<(), String>;
    fn cleanup(&self) -> Result<(), String>;
    fn finish(&self, id: &str) -> Result<bool, String>;
    fn timeout_pending(&self) -> bool;
    fn take_timeout(&self) -> Option<String>;
}

#[derive(Debug, Clone)]
pub struct TestParticipation {
    inner: Arc<TestParticipationInner>,
}

#[derive(Debug)]
struct TestParticipationInner {
    interrupted: Arc<AtomicBool>,
    phases: Option<Arc<dyn TestPhaseObserver>>,
    selected: Option<std::collections::BTreeSet<String>>,
    limits: EnvelopeLimits,
    expected: BTreeMap<String, BTreeMap<String, String>>,
    update_snapshots: bool,
    executions: Mutex<Vec<TestNodeExecution>>,
}

impl TestParticipation {
    /// Restricts an immutable compiled participation to a retry unit. The
    /// entire target was checked before this runtime selection is installed.
    pub fn with_selection(mut self, selected: std::collections::BTreeSet<String>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("select before sharing a participation")
            .selected = Some(selected);
        self
    }

    pub(crate) fn selects(&self, id: &str) -> bool {
        self.inner.selected.as_ref().is_none_or(|selected| {
            selected.contains(id)
                || selected.iter().any(|leaf| {
                    leaf.strip_prefix(id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
                })
        })
    }

    pub fn with_phases(mut self, phases: Arc<dyn TestPhaseObserver>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("configure phases before sharing a participation")
            .phases = Some(phases);
        self
    }

    pub(crate) fn timeout_pending(&self) -> bool {
        self.inner
            .phases
            .as_ref()
            .is_some_and(|phases| phases.timeout_pending())
    }

    pub(crate) fn take_timeout(&self) -> Option<String> {
        self.inner
            .phases
            .as_ref()
            .and_then(|phases| phases.take_timeout())
    }

    pub(crate) fn begin_cleanup(&self) -> Result<(), String> {
        self.inner
            .phases
            .as_ref()
            .map_or(Ok(()), |phases| phases.cleanup())
    }

    /// Shares the worker's external cancellation request with the hosted VM.
    /// The request stops the participation; it is never a test assertion.
    pub fn with_interruption(mut self, interrupted: Arc<AtomicBool>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("configure interruption before sharing a participation")
            .interrupted = interrupted;
        self
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.inner.interrupted.load(Ordering::Acquire)
    }

    pub fn new(
        limits: EnvelopeLimits,
        expected: BTreeMap<String, BTreeMap<String, String>>,
        update_snapshots: bool,
    ) -> Self {
        Self {
            inner: Arc::new(TestParticipationInner {
                interrupted: Arc::new(AtomicBool::new(false)),
                phases: None,
                selected: None,
                limits,
                expected,
                update_snapshots,
                executions: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) fn envelope(
        &self,
        id: &str,
        kind: TestExecutionKind,
    ) -> Result<EnvelopeHandle, String> {
        let envelope = EnvelopeHandle::new(id, self.inner.limits);
        envelope
            .with_expected_snapshots(self.inner.expected.get(id).cloned().unwrap_or_default())
            .map_err(|error| error.to_string())?;
        envelope
            .with_snapshot_update(self.inner.update_snapshots)
            .map_err(|error| error.to_string())?;
        if kind == TestExecutionKind::Leaf {
            envelope
                .set_phase(ExecutionPhase::Body)
                .map_err(|error| error.to_string())?;
        }
        if let Some(phases) = &self.inner.phases {
            phases.enter(id, kind)?;
        }
        Ok(envelope)
    }

    /// Closes an unwound node without manufacturing an ordinary test result.
    /// Ancestor deadlines can unwind descendants before their own boundary.
    pub(crate) fn finish_interrupted(
        &self,
        id: &str,
        envelope: EnvelopeHandle,
    ) -> Result<(), String> {
        envelope.close().map_err(|error| error.to_string())?;
        if let Some(phases) = &self.inner.phases {
            phases.finish(id)?;
        }
        Ok(())
    }

    pub(crate) fn finish(
        &self,
        id: &str,
        kind: TestExecutionKind,
        envelope: EnvelopeHandle,
        panic: Option<VmPanic>,
    ) -> Result<(), String> {
        let phase = envelope.phase().map_err(|error| error.to_string())?;
        envelope.close().map_err(|error| error.to_string())?;
        let report = envelope.report().map_err(|error| error.to_string())?;
        let snapshot_updates = envelope
            .snapshot_updates()
            .map_err(|error| error.to_string())?;
        let timed_out = self
            .inner
            .phases
            .as_ref()
            .map_or(Ok(false), |phases| phases.finish(id))?;
        self.inner
            .executions
            .lock()
            .map_err(|_| "test participation record lock is poisoned".to_owned())?
            .push(TestNodeExecution {
                id: id.to_owned(),
                kind,
                report,
                phase,
                panic,
                snapshot_updates,
                timed_out,
            });
        Ok(())
    }

    pub fn executions(&self) -> Result<Vec<TestNodeExecution>, String> {
        self.inner
            .executions
            .lock()
            .map(|executions| executions.clone())
            .map_err(|_| "test participation record lock is poisoned".to_owned())
    }
}

/// A discovered test declaration and its enclosing suite setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEntry {
    file: FileId,
    logical_path: String,
    id: String,
    name: String,
    body: Vec<u8>,
    setup: Vec<Vec<u8>>,
    suites: Vec<String>,
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

    /// Canonical suite ancestors, ordered from the outermost to the innermost.
    pub fn suites(&self) -> &[String] {
        &self.suites
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
    source_class: TestSourceClass,
    package_name: &str,
) -> Result<Vec<TestEntry>, TestBackendError> {
    if cst.root_node().descendant_tokens().any(|token| {
        token.kind() == crate::syntax::TokenKind::Identifier
            && token
                .token()
                .normalized_identifier()
                .is_some_and(|name| name.starts_with("__tondo"))
    }) {
        return Err(TestBackendError::InvalidBody(
            "identifiers beginning with `__tondo` are reserved for the toolchain".into(),
        ));
    }
    let source = sources.get(file)?;
    let root = SourceFile::root(cst)
        .ok_or_else(|| TestBackendError::InvalidBody("missing module root".into()))?;
    let mut entries = Vec::new();
    let mut parents = Vec::new();
    let mut setup = Vec::new();
    visit_declarations(
        file,
        source.bytes(),
        root.declarations(),
        package_name,
        source.path().as_str(),
        source_class.test_id_segment(),
        source.module().as_str(),
        &mut parents,
        &mut setup,
        &mut entries,
    )?;
    Ok(entries)
}

/// Produces a complete ordinary module whose private entry body is the selected
/// test.  All non-test declarations and imports remain available, while
/// suites contribute setup statements in declaration order.
pub fn lower_selected(
    sources: &SourceDatabase,
    file: FileId,
    cst: &Cst,
    source_class: TestSourceClass,
    package_name: &str,
    selector: Option<&str>,
) -> Result<Vec<u8>, TestBackendError> {
    let entries = discover(sources, file, cst, source_class, package_name)?;
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
    output.extend_from_slice(b"fn __tondoTestEntry() {\n");
    for statement in selected.setup() {
        output.extend_from_slice(statement);
        output.extend_from_slice(b"\n");
    }
    output.extend_from_slice(selected.body());
    output.extend_from_slice(b"\n}\n");
    Ok(output)
}

/// Produces one ordinary module for a complete participation in a source
/// file. Suite scopes are preserved and each selected leaf is wrapped in a
/// compiler-owned VM boundary, allowing sibling leaves to continue after a
/// terminal without duplicating suite setup.
pub fn lower_participation<'a>(
    sources: &SourceDatabase,
    file: FileId,
    cst: &Cst,
    source_class: TestSourceClass,
    package_name: &str,
    selectors: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<u8>, TestBackendError> {
    let entries = discover(sources, file, cst, source_class, package_name)?;
    let selected = selectors
        .into_iter()
        .enumerate()
        .map(|(index, selector)| (selector.to_owned(), index))
        .collect::<BTreeMap<_, _>>();
    if selected.is_empty() {
        return Err(TestBackendError::NoTests);
    }
    for selector in selected.keys() {
        if !entries.iter().any(|entry| entry.id == *selector) {
            return Err(TestBackendError::MissingEntry(selector.clone()));
        }
    }

    let source = sources.get(file)?;
    let root = cst.root_node();
    let mut output = Vec::with_capacity(source.bytes().len() + 128);
    output.extend_from_slice(b"import std.testing as __tondoTesting\n");
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
    output.extend_from_slice(b"fn __tondoTestEntry() {\n");
    let root = SourceFile::root(cst)
        .ok_or_else(|| TestBackendError::InvalidBody("missing module root".into()))?;
    let mut parents = Vec::new();
    emit_participation(
        &mut output,
        source.bytes(),
        root.declarations(),
        package_name,
        source_class.test_id_segment(),
        source.module().as_str(),
        &mut parents,
        &selected,
    )?;
    output.extend_from_slice(b"\n}\n");
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn emit_participation<'a>(
    output: &mut Vec<u8>,
    source: &[u8],
    declarations: impl Iterator<Item = Declaration<'a>>,
    package_name: &str,
    source_class: &str,
    module: &str,
    parents: &mut Vec<String>,
    selected: &BTreeMap<String, usize>,
) -> Result<(), TestBackendError> {
    let mut declarations = declarations.collect::<Vec<_>>();
    declarations.sort_by_key(|declaration| {
        participation_rank(
            *declaration,
            package_name,
            source_class,
            module,
            parents,
            selected,
        )
    });
    for declaration in declarations {
        match declaration {
            Declaration::Test(test) => {
                let name = test
                    .name_token()
                    .and_then(|token| token.token().normalized_identifier())
                    .ok_or_else(|| {
                        TestBackendError::InvalidBody("test declaration has no name".into())
                    })?;
                let id = test_id(package_name, source_class, module, parents, name);
                if !selected.contains_key(&id) {
                    continue;
                }
                let body = test
                    .body()
                    .ok_or_else(|| TestBackendError::InvalidBody("test has no body".into()))?;
                output.extend_from_slice(b"__tondoTesting.__runLeaf(");
                append_string_literal(output, &id);
                output.extend_from_slice(b", () {\n");
                output.extend_from_slice(&block_contents(source, body.syntax().range())?);
                output.extend_from_slice(b"\n})\n");
            }
            Declaration::Suite(suite) => {
                let name = suite
                    .name_token()
                    .and_then(|token| token.token().normalized_identifier())
                    .ok_or_else(|| {
                        TestBackendError::InvalidBody("suite declaration has no name".into())
                    })?;
                parents.push(name.to_owned());
                let prefix = test_id_prefix(package_name, source_class, module, parents);
                let participates = selected.keys().any(|id| id.starts_with(&prefix));
                if participates {
                    let body = suite
                        .body()
                        .ok_or_else(|| TestBackendError::InvalidBody("suite has no body".into()))?;
                    output.extend_from_slice(b"__tondoTesting.__runSuite(");
                    append_string_literal(output, prefix.trim_end_matches("::"));
                    output.extend_from_slice(b", () {\n");
                    for statement in body.setup() {
                        output.extend_from_slice(slice(source, statement.syntax().range())?);
                        output.extend_from_slice(b"\n");
                    }
                    emit_participation(
                        output,
                        source,
                        body.members(),
                        package_name,
                        source_class,
                        module,
                        parents,
                        selected,
                    )?;
                    output.extend_from_slice(b"__tondoTesting.__beginSuiteCleanup()\n})\n");
                }
                parents.pop();
            }
            _ => {}
        }
    }
    Ok(())
}

fn participation_rank(
    declaration: Declaration<'_>,
    package_name: &str,
    source_class: &str,
    module: &str,
    parents: &[String],
    selected: &BTreeMap<String, usize>,
) -> usize {
    match declaration {
        Declaration::Test(test) => test
            .name_token()
            .and_then(|token| token.token().normalized_identifier())
            .and_then(|name| {
                selected
                    .get(&test_id(package_name, source_class, module, parents, name))
                    .copied()
            })
            .unwrap_or(usize::MAX),
        Declaration::Suite(suite) => suite
            .name_token()
            .and_then(|token| token.token().normalized_identifier())
            .and_then(|name| {
                let mut path = parents.to_vec();
                path.push(name.to_owned());
                let prefix = test_id_prefix(package_name, source_class, module, &path);
                selected
                    .iter()
                    .filter(|(id, _)| id.starts_with(&prefix))
                    .map(|(_, rank)| *rank)
                    .min()
            })
            .unwrap_or(usize::MAX),
        _ => usize::MAX,
    }
}

fn test_id_prefix(
    package_name: &str,
    source_class: &str,
    module: &str,
    parents: &[String],
) -> String {
    let mut id = format!("{package_name}::{source_class}::{module}");
    for parent in parents {
        id.push_str("::");
        id.push_str(parent);
    }
    id.push_str("::");
    id
}

fn append_string_literal(output: &mut Vec<u8>, value: &str) {
    output.push(b'"');
    for byte in value.bytes() {
        match byte {
            b'"' => output.extend_from_slice(b"\\\""),
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            b'\t' => output.extend_from_slice(b"\\t"),
            byte => output.push(byte),
        }
    }
    output.push(b'"');
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
    source_class: &str,
    module: &str,
    parents: &mut Vec<String>,
    setup: &mut Vec<Vec<u8>>,
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
                let id = test_id(package_name, source_class, module, parents, name);
                entries.push(TestEntry {
                    file,
                    logical_path: logical_path.to_owned(),
                    id,
                    name: name.to_owned(),
                    body: block_contents(source, body.syntax().range())?,
                    setup: setup.clone(),
                    suites: parents.clone(),
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
                }
                visit_declarations(
                    file,
                    source,
                    body.members(),
                    package_name,
                    logical_path,
                    source_class,
                    module,
                    parents,
                    setup,
                    entries,
                )?;
                for _ in body.setup() {
                    setup.pop();
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

    fn parsed(
        source: &[u8],
    ) -> (
        SourceDatabase,
        FileId,
        crate::syntax::Parsed,
        TestSourceClass,
    ) {
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
        let source_class = TestSourceClass::UnitTest;
        (sources, file, parsed, source_class)
    }

    #[test]
    fn explicit_source_class_controls_ids_independently_of_path() {
        let (sources, file, parsed, _) = parsed(b"test smoke { assert(true) }\n");
        for (class, segment) in [
            (TestSourceClass::UnitTest, "unit"),
            (TestSourceClass::IntegrationTest, "integration"),
        ] {
            let entries = discover(&sources, file, parsed.cst(), class, "main").unwrap();
            assert_eq!(entries[0].id(), format!("main::{segment}::tests::smoke"));
            let lowered = lower_participation(
                &sources,
                file,
                parsed.cst(),
                class,
                "main",
                [entries[0].id()],
            )
            .unwrap();
            assert!(
                String::from_utf8(lowered)
                    .unwrap()
                    .contains(entries[0].id())
            );
        }
    }

    #[test]
    fn lowers_top_level_test_to_real_main_body() {
        let (sources, file, parsed, source_class) =
            parsed(b"import std.console\n\ntest smoke { assert(true) }\n");
        let output =
            lower_selected(&sources, file, parsed.cst(), source_class, "main", None).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("fn __tondoTestEntry()"));
        assert!(text.contains("assert(true)"));
        assert!(!text.contains("test smoke"));
    }

    #[test]
    fn suite_setup_is_inlined_before_the_child_body() {
        let source = b"suite arithmetic { let offset = 2\n test adds { assert(offset == 2) } }\n";
        let (sources, file, parsed, source_class) = parsed(source);
        let entries = discover(&sources, file, parsed.cst(), source_class, "main").unwrap();
        assert_eq!(entries.len(), 1);
        let output = lower_selected(
            &sources,
            file,
            parsed.cst(),
            source_class,
            "main",
            Some(entries[0].id()),
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.find("let offset").unwrap() < text.find("assert(offset").unwrap());
    }

    #[test]
    fn participation_emits_selected_members_in_the_planned_tree_order() {
        let source = b"suite ordered {\n test first { assert(true) }\n suite nested { test middle { assert(true) } }\n test last { assert(true) }\n}\n";
        let (sources, file, parsed, source_class) = parsed(source);
        let entries = discover(&sources, file, parsed.cst(), source_class, "main").unwrap();
        let selectors = [entries[2].id(), entries[1].id(), entries[0].id()];
        let text = String::from_utf8(
            lower_participation(
                &sources,
                file,
                parsed.cst(),
                source_class,
                "main",
                selectors,
            )
            .unwrap(),
        )
        .unwrap();
        let last = text.find(entries[2].id()).unwrap();
        let middle = text.find(entries[1].id()).unwrap();
        let first = text.find(entries[0].id()).unwrap();
        assert!(last < middle && middle < first);
    }

    #[test]
    fn async_is_inferred_from_suite_setup_or_test_body() {
        for source in [
            b"fn value(): Int suspends { 1 }\nsuite service { let item = value()\n test reads { assert(item == 1) } }\n".as_slice(),
            b"fn value(): Int suspends { 1 }\ntest reads { assert(value() == 1) }\n".as_slice(),
        ] {
            let (sources, file, parsed, source_class) = parsed(source);
            let output =
                lower_selected(&sources, file, parsed.cst(), source_class, "main", None).unwrap();
            assert!(String::from_utf8(output)
                .unwrap()
                .contains("fn __tondoTestEntry()"));
        }
    }

    #[test]
    fn rejects_a_production_main_in_a_test_target() {
        let source = b"fn main() {}\ntest smoke { assert(true) }\n";
        let (sources, file, parsed, source_class) = parsed(source);
        let error = lower_selected(&sources, file, parsed.cst(), source_class, "main", None)
            .expect_err("main must be rejected by the test backend");
        assert_eq!(error, TestBackendError::ProductionMain);
    }

    #[test]
    fn toolchain_test_boundary_namespace_cannot_be_spelled_by_user_source() {
        let source = b"import std.testing as __tondoTesting\ntest smoke { assert(true) }\n";
        let (sources, file, parsed, source_class) = parsed(source);
        let error = discover(&sources, file, parsed.cst(), source_class, "main")
            .expect_err("toolchain namespace must stay sealed");
        assert!(error.to_string().contains("reserved for the toolchain"));
    }
}
