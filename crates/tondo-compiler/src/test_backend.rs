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
    execute_compiled_with_environment(
        program,
        entry,
        limits,
        participation,
        diagnostics,
        BTreeMap::new(),
    )
}

/// Execute with the worker's explicitly materialized environment. No ambient
/// environment or process arguments are consulted by this hosted route.
pub fn execute_compiled_with_environment(
    program: &tondo_vm::bytecode::BytecodeProgram,
    entry: tondo_vm::bytecode::BytecodeFunctionId,
    limits: tondo_vm::runtime::VmLimits,
    participation: TestParticipation,
    diagnostics: Option<tondo_vm::runtime::DiagnosticConfig>,
    environment: BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<tondo_vm::runtime::VmExecution, tondo_vm::runtime::VmError> {
    let mut host = crate::process_host::BootstrapHost::with_test_environment(
        environment,
        limits.max_heap_bytes,
    );
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
    pub error_type: Option<String>,
    pub error_span: Option<tondo_vm::bytecode::BytecodeSpan>,
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
    temporary_root: Option<std::path::PathBuf>,
    phases: Option<Arc<dyn TestPhaseObserver>>,
    selected: Option<std::collections::BTreeSet<String>>,
    limits: EnvelopeLimits,
    expected: BTreeMap<String, BTreeMap<String, String>>,
    update_snapshots: bool,
    executions: Mutex<Vec<TestNodeExecution>>,
}

impl TestParticipation {
    /// Supply the coordinator-owned root through explicit worker transport.
    /// The coordinator retains ownership and verifies cleanup after reaping.
    pub fn with_temporary_root(mut self, root: std::path::PathBuf) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("install temporary root before sharing a participation")
            .temporary_root = Some(root);
        self
    }

    pub(crate) fn temporary_root(&self) -> Option<&std::path::Path> {
        self.inner.temporary_root.as_deref()
    }

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
                temporary_root: None,
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
        error: Option<(String, tondo_vm::bytecode::BytecodeSpan)>,
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
                error_type: error.as_ref().map(|(name, _)| name.clone()),
                error_span: error.map(|(_, span)| span),
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
    lower_participation_mapped(sources, file, cst, source_class, package_name, selectors)
        .map(|lowered| lowered.bytes)
}

/// Copies user fragments verbatim and records their original byte ranges.
/// Synthetic runner scaffolding is never presented as a user source location.
pub(crate) struct LoweredParticipation {
    pub bytes: Vec<u8>,
    pub testing_calls: Vec<crate::source::TextRange>,
    copies: Vec<(crate::source::TextRange, crate::source::TextRange)>,
    anchors: Vec<SourceAnchor>,
    active_anchor: Option<usize>,
}

struct SourceAnchor {
    start: usize,
    end: usize,
    original: crate::source::TextRange,
    parent: Option<usize>,
}

impl LoweredParticipation {
    fn runner_call(&mut self, name: &[u8]) -> Result<(), TestBackendError> {
        self.bytes.extend_from_slice(b"__tondoTesting.");
        let start = u32::try_from(self.bytes.len()).map_err(|_| {
            TestBackendError::InvalidBody("generated source exceeds the byte range limit".into())
        })?;
        self.bytes.extend_from_slice(name);
        let end = u32::try_from(self.bytes.len()).map_err(|_| {
            TestBackendError::InvalidBody("generated source exceeds the byte range limit".into())
        })?;
        self.testing_calls
            .push(crate::source::TextRange::new(start, end)?);
        self.bytes.push(b'(');
        Ok(())
    }

    fn begin_anchor(&mut self, original: crate::source::TextRange) -> usize {
        let index = self.anchors.len();
        self.anchors.push(SourceAnchor {
            start: self.bytes.len(),
            end: self.bytes.len(),
            original,
            parent: self.active_anchor,
        });
        self.active_anchor = Some(index);
        index
    }

    fn end_anchor(&mut self, index: usize) {
        self.anchors[index].end = self.bytes.len();
        self.active_anchor = self.anchors[index].parent;
    }

    fn copy(
        &mut self,
        source: &[u8],
        range: crate::source::TextRange,
    ) -> Result<(), TestBackendError> {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(slice(source, range)?);
        let start = u32::try_from(start).map_err(|_| {
            TestBackendError::InvalidBody("generated source exceeds the byte range limit".into())
        })?;
        let end = u32::try_from(self.bytes.len()).map_err(|_| {
            TestBackendError::InvalidBody("generated source exceeds the byte range limit".into())
        })?;
        self.copies
            .push((crate::source::TextRange::new(start, end)?, range));
        Ok(())
    }

    pub(crate) fn original_span(
        &self,
        mut span: tondo_vm::bytecode::BytecodeSpan,
        generated: &[u8],
    ) -> Option<tondo_vm::bytecode::BytecodeSpan> {
        // CST statement spans include trailing trivia. A generated separator
        // can follow the copied body's final newline, so remove whitespace
        // before deciding which original fragment owns the terminal.
        generated.get(span.start as usize..span.end as usize)?;
        while span.start < span.end && generated[span.start as usize].is_ascii_whitespace() {
            span.start += 1;
        }
        while span.end > span.start && generated[span.end as usize - 1].is_ascii_whitespace() {
            span.end -= 1;
        }
        if let Some(index) = self
            .copies
            .partition_point(|(range, _)| range.start() <= span.start)
            .checked_sub(1)
        {
            let (generated, original) = self.copies[index];
            if span.end <= generated.end() {
                span.start = original.start() + (span.start - generated.start());
                span.end = original.start() + (span.end - generated.start());
                return Some(span);
            }
        }
        let mut candidate = self
            .anchors
            .partition_point(|anchor| anchor.start <= span.start as usize)
            .checked_sub(1);
        while let Some(index) = candidate {
            let anchor = &self.anchors[index];
            if span.end as usize <= anchor.end {
                span.start = anchor.original.start();
                span.end = anchor.original.end();
                return Some(span);
            }
            candidate = anchor.parent;
        }
        None
    }

    pub(crate) fn copied_range(
        &self,
        range: crate::source::TextRange,
    ) -> Option<crate::source::TextRange> {
        let index = self
            .copies
            .partition_point(|(copy, _)| copy.start() <= range.start())
            .checked_sub(1)?;
        let (generated, original) = self.copies[index];
        (range.end() <= generated.end()).then(|| {
            crate::source::TextRange::new(
                original.start() + (range.start() - generated.start()),
                original.start() + (range.end() - generated.start()),
            )
            .expect("a copied range preserves ordered endpoints")
        })
    }
}

pub(crate) fn lower_participation_mapped<'a>(
    sources: &SourceDatabase,
    file: FileId,
    cst: &Cst,
    source_class: TestSourceClass,
    package_name: &str,
    selectors: impl IntoIterator<Item = &'a str>,
) -> Result<LoweredParticipation, TestBackendError> {
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
    let mut output = LoweredParticipation {
        bytes: Vec::with_capacity(source.bytes().len() + 128),
        testing_calls: Vec::new(),
        copies: Vec::new(),
        anchors: Vec::new(),
        active_anchor: None,
    };
    let module_anchor = output.begin_anchor(crate::source::TextRange::new(0, source.length())?);
    output
        .bytes
        .extend_from_slice(b"import std.testing as __tondoTesting\n");
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
                output.copy(source.bytes(), node.range())?;
            }
            _ => output.copy(source.bytes(), node.range())?,
        }
        output.bytes.extend_from_slice(b"\n");
    }
    if saw_main {
        return Err(TestBackendError::ProductionMain);
    }
    output.bytes.extend_from_slice(b"fn __tondoTestEntry() {\n");
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
    output.bytes.extend_from_slice(b"\n}\n");
    output.end_anchor(module_anchor);
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn emit_participation<'a>(
    output: &mut LoweredParticipation,
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
                let anchor = output.begin_anchor(test.syntax().range());
                output.runner_call(b"__runLeaf")?;
                append_string_literal(&mut output.bytes, &id);
                output.bytes.extend_from_slice(b", () {\n");
                output.copy(source, block_contents_range(body.syntax())?)?;
                output.bytes.extend_from_slice(b"\n})\n");
                output.end_anchor(anchor);
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
                    let anchor = output.begin_anchor(suite.syntax().range());
                    output.runner_call(b"__runSuite")?;
                    append_string_literal(&mut output.bytes, prefix.trim_end_matches("::"));
                    output.bytes.extend_from_slice(b", () {\n");
                    for statement in body.setup() {
                        output.copy(source, statement.syntax().range())?;
                        output.bytes.extend_from_slice(b"\n");
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
                    output.runner_call(b"__beginSuiteCleanup")?;
                    output.bytes.extend_from_slice(b")\n})\n");
                    output.end_anchor(anchor);
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
                    body: slice(source, block_contents_range(body.syntax())?)?.to_vec(),
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

fn block_contents_range(
    body: crate::syntax::SyntaxNodeRef<'_>,
) -> Result<crate::source::TextRange, TestBackendError> {
    let open = body
        .child_tokens()
        .find(|token| token.kind() == crate::syntax::TokenKind::LBrace)
        .ok_or_else(|| TestBackendError::InvalidBody("test body has no opening brace".into()))?;
    let close = body
        .child_tokens()
        .find(|token| token.kind() == crate::syntax::TokenKind::RBrace)
        .ok_or_else(|| TestBackendError::InvalidBody("test body has no closing brace".into()))?;
    Ok(crate::source::TextRange::new(
        open.range().end(),
        close.range().start(),
    )?)
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
    fn participation_maps_terminals_across_generated_whitespace() {
        let original = b"test direct {\n    fail Fault.Broken\n}\n";
        let (sources, file, tree, class) = parsed(original);
        let entries = discover(&sources, file, tree.cst(), class, "main").unwrap();
        let lowered = lower_participation_mapped(
            &sources,
            file,
            tree.cst(),
            class,
            "main",
            [entries[0].id()],
        )
        .unwrap();
        let (_, _, generated, _) = parsed(&lowered.bytes);
        let terminal = generated
            .cst()
            .nodes()
            .iter()
            .find(|node| node.kind() == SyntaxKind::FailStmt)
            .unwrap()
            .range();
        let span = tondo_vm::bytecode::BytecodeSpan {
            file: file.index(),
            start: terminal.start(),
            end: terminal.end(),
        };
        let mapped = lowered.original_span(span, &lowered.bytes).unwrap();
        assert_eq!(
            std::str::from_utf8(&original[mapped.start as usize..mapped.end as usize])
                .unwrap()
                .trim(),
            "fail Fault.Broken",
            "generated {:?}: {:?}",
            span,
            std::str::from_utf8(&lowered.bytes[span.start as usize..span.end as usize])
        );
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
