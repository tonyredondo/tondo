//! Common lowering for checked test entries.
//!
//! The test frontend stops at [`crate::test_check::TestBodyContract`].  This
//! module is the single, deterministic bridge from that contract to the
//! three compiler representations.  HIR, MIR and bytecode deliberately share
//! the same sealed operation vocabulary; the admission verifier proves that a
//! later pass did not silently create a second test-only execution path.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::artifact::sha256;
use crate::test_check::{
    ErrorMember, OperandShape, TestBodyContract, TestEntryKind, TestOperation,
    VirtualTimeClosureFacts,
};

pub const TEST_LOWER_FORMAT: &str = "tondo-test-lower-draft/1";

/// A source range retained by every lowered entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceSpan {
    file: String,
    start: u32,
    end: u32,
}

impl SourceSpan {
    pub fn new(file: impl Into<String>, start: u32, end: u32) -> Self {
        Self {
            file: file.into(),
            start,
            end,
        }
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn end(&self) -> u32 {
        self.end
    }
}

/// A closed environment slot captured by a suite child.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvironmentSnapshot {
    binding: String,
    type_name: String,
    digest: String,
}

impl EnvironmentSnapshot {
    pub fn new(
        binding: impl Into<String>,
        type_name: impl Into<String>,
        digest: impl Into<String>,
    ) -> Self {
        Self {
            binding: binding.into(),
            type_name: type_name.into(),
            digest: digest.into(),
        }
    }

    pub fn binding(&self) -> &str {
        &self.binding
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// The statically known input/output domain of an entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestDomain {
    input: String,
    output: String,
}

impl TestDomain {
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
        }
    }

    pub fn unit() -> Self {
        Self::new("Unit", "Unit ! E")
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn output(&self) -> &str {
        &self.output
    }
}

/// Explicit cleanup metadata retained by all three representations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CleanupPlan {
    deferred: bool,
    hooks: Vec<String>,
}

impl CleanupPlan {
    pub fn new(deferred: bool, hooks: impl IntoIterator<Item = String>) -> Self {
        let mut hooks = hooks.into_iter().collect::<Vec<_>>();
        hooks.sort();
        Self { deferred, hooks }
    }

    pub const fn deferred(&self) -> bool {
        self.deferred
    }

    pub fn hooks(&self) -> &[String] {
        &self.hooks
    }
}

/// Unchecked metadata for one already checked test or suite entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestLowerEntryInput {
    node: String,
    parent: Option<String>,
    span: SourceSpan,
    contract: TestBodyContract,
    environment: Vec<EnvironmentSnapshot>,
    domain: TestDomain,
    cleanup_hooks: Vec<String>,
}

impl TestLowerEntryInput {
    pub fn new(node: impl Into<String>, span: SourceSpan, contract: TestBodyContract) -> Self {
        Self {
            node: node.into(),
            parent: None,
            span,
            contract,
            environment: Vec::new(),
            domain: TestDomain::unit(),
            cleanup_hooks: Vec::new(),
        }
    }

    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    pub fn with_environment(
        mut self,
        environment: impl IntoIterator<Item = EnvironmentSnapshot>,
    ) -> Self {
        self.environment = environment.into_iter().collect();
        self
    }

    pub fn with_domain(mut self, domain: TestDomain) -> Self {
        self.domain = domain;
        self
    }

    pub fn with_cleanup_hooks(mut self, hooks: impl IntoIterator<Item = String>) -> Self {
        self.cleanup_hooks = hooks.into_iter().collect();
        self
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    pub fn contract(&self) -> &TestBodyContract {
        &self.contract
    }

    pub fn environment(&self) -> &[EnvironmentSnapshot] {
        &self.environment
    }

    pub fn domain(&self) -> &TestDomain {
        &self.domain
    }

    pub fn cleanup_hooks(&self) -> &[String] {
        &self.cleanup_hooks
    }
}

/// Unchecked input for one test target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestLowerInput {
    target: String,
    main_present: bool,
    entries: Vec<TestLowerEntryInput>,
}

impl TestLowerInput {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            main_present: false,
            entries: Vec::new(),
        }
    }

    pub fn with_main(mut self, present: bool) -> Self {
        self.main_present = present;
        self
    }

    pub fn with_entries(mut self, entries: impl IntoIterator<Item = TestLowerEntryInput>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub const fn main_present(&self) -> bool {
        self.main_present
    }

    pub fn entries(&self) -> &[TestLowerEntryInput] {
        &self.entries
    }
}

/// Stable identity shared by HIR, MIR and bytecode.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestIdentity {
    target: String,
    node: String,
    kind: TestEntryKind,
    digest: String,
}

impl TestIdentity {
    fn new(target: &str, node: &str, kind: TestEntryKind) -> Self {
        let digest = sha256(format!("{target}\0{node}\0{}", kind.as_str()).as_bytes());
        Self {
            target: target.into(),
            node: node.into(),
            kind,
            digest,
        }
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub const fn kind(&self) -> TestEntryKind {
        self.kind
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// The operation vocabulary copied into each common compiler representation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoweredOperation {
    TestLog {
        message: String,
    },
    TestTags {
        values: String,
    },
    TestFailNow {
        message: String,
    },
    TestSkip {
        reason: String,
    },
    TestAttach {
        name: String,
        media_type: String,
        bytes: String,
    },
    TestSnapshot {
        name: String,
        actual: String,
    },
    WithVirtualTime {
        is_async: bool,
        send: bool,
        call_once: bool,
        returns_unit: bool,
        accepts_ref: bool,
        escapes_controller: bool,
        shares_controller: bool,
    },
    VirtualTimeSettle,
    VirtualTimeAdvance {
        duration_ns: i128,
    },
}

impl LoweredOperation {
    fn from_test_operation(operation: &TestOperation) -> Self {
        match operation {
            TestOperation::Log { message } => Self::TestLog {
                message: operand_name(message),
            },
            TestOperation::Tags { values } => Self::TestTags {
                values: operand_name(values),
            },
            TestOperation::FailNow { message } => Self::TestFailNow {
                message: operand_name(message),
            },
            TestOperation::Skip { reason } => Self::TestSkip {
                reason: operand_name(reason),
            },
            TestOperation::Attach {
                name,
                media_type,
                bytes,
            } => Self::TestAttach {
                name: name.clone(),
                media_type: media_type.clone(),
                bytes: operand_name(bytes),
            },
            TestOperation::Snapshot { name, actual } => Self::TestSnapshot {
                name: name.clone(),
                actual: operand_name(actual),
            },
            TestOperation::WithVirtualTime(facts) => Self::from_virtual_time(*facts),
            TestOperation::Settle => Self::VirtualTimeSettle,
            TestOperation::Advance { duration_ns } => Self::VirtualTimeAdvance {
                duration_ns: *duration_ns,
            },
        }
    }

    fn from_virtual_time(facts: VirtualTimeClosureFacts) -> Self {
        Self::WithVirtualTime {
            is_async: facts.is_async(),
            send: facts.send(),
            call_once: facts.call_once(),
            returns_unit: facts.returns_unit(),
            accepts_ref: facts.accepts_ref(),
            escapes_controller: facts.escapes_controller(),
            shares_controller: facts.shares_controller(),
        }
    }
}

impl fmt::Display for LoweredOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TestLog { .. } => formatter.write_str("TestLog"),
            Self::TestTags { .. } => formatter.write_str("TestTags"),
            Self::TestFailNow { .. } => formatter.write_str("TestFailNow"),
            Self::TestSkip { .. } => formatter.write_str("TestSkip"),
            Self::TestAttach { .. } => formatter.write_str("TestAttach"),
            Self::TestSnapshot { .. } => formatter.write_str("TestSnapshot"),
            Self::WithVirtualTime { .. } => formatter.write_str("WithVirtualTime"),
            Self::VirtualTimeSettle => formatter.write_str("VirtualTimeSettle"),
            Self::VirtualTimeAdvance { .. } => formatter.write_str("VirtualTimeAdvance"),
        }
    }
}

fn operand_name(shape: &OperandShape) -> String {
    match shape {
        OperandShape::String => "String".into(),
        OperandShape::TagsMap => "Map[String, String]".into(),
        OperandShape::Bytes => "Bytes".into(),
        OperandShape::Duration => "Duration".into(),
        OperandShape::VirtualTimeRef => "ref VirtualTime".into(),
        OperandShape::Other(value) => format!("Other({value})"),
    }
}

/// A lowered error member retained in the entry's output domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoweredError {
    name: String,
    discard: bool,
}

impl LoweredError {
    fn from_member(member: &ErrorMember) -> Self {
        Self {
            name: member.name().into(),
            discard: member.discard(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn discard(&self) -> bool {
        self.discard
    }
}

/// One entry after common lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredTestEntry {
    identity: TestIdentity,
    parent: Option<TestIdentity>,
    span: SourceSpan,
    environment: Vec<EnvironmentSnapshot>,
    domain: TestDomain,
    errors: Vec<LoweredError>,
    is_async: bool,
    cleanup: CleanupPlan,
    hir: Vec<LoweredOperation>,
    mir: Vec<LoweredOperation>,
    bytecode: Vec<LoweredOperation>,
}

impl LoweredTestEntry {
    pub fn identity(&self) -> &TestIdentity {
        &self.identity
    }

    pub fn parent(&self) -> Option<&TestIdentity> {
        self.parent.as_ref()
    }

    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    pub fn environment(&self) -> &[EnvironmentSnapshot] {
        &self.environment
    }

    pub fn domain(&self) -> &TestDomain {
        &self.domain
    }

    pub fn errors(&self) -> &[LoweredError] {
        &self.errors
    }

    pub const fn is_async(&self) -> bool {
        self.is_async
    }

    pub fn cleanup(&self) -> &CleanupPlan {
        &self.cleanup
    }

    pub fn hir(&self) -> &[LoweredOperation] {
        &self.hir
    }

    pub fn mir(&self) -> &[LoweredOperation] {
        &self.mir
    }

    pub fn bytecode(&self) -> &[LoweredOperation] {
        &self.bytecode
    }
}

/// An immutable test-target artifact admitted by the compiler pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestArtifact {
    target: String,
    entries: Vec<LoweredTestEntry>,
    artifact_hash: String,
}

impl TestArtifact {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn entries(&self) -> &[LoweredTestEntry] {
        &self.entries
    }

    pub fn artifact_hash(&self) -> &str {
        &self.artifact_hash
    }

    /// The canonical bytes are suitable for a cache key and contain no host
    /// addresses, `FileId`s or insertion-order artifacts.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_artifact_bytes(&self.target, &self.entries)
    }

    pub fn verify(&self) -> Result<(), LowerError> {
        verify(self)
    }
}

/// Errors found while lowering or admitting a test artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    EmptyTarget,
    MainEntry,
    EmptyNode,
    ContractIdentity { input: String, contract: String },
    DuplicateNode(String),
    MissingParent { node: String, parent: String },
    ParentOrder { node: String, parent: String },
    InvalidSpan(String),
    InvalidEnvironment { node: String, binding: String },
    DuplicateCleanup { node: String, hook: String },
    InvalidCleanup { node: String },
    Admission { node: String, message: String },
}

impl LowerError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyTarget => "E2010",
            Self::MainEntry => "E2011",
            Self::EmptyNode => "E2012",
            Self::ContractIdentity { .. } => "E2013",
            Self::DuplicateNode(_) => "E2014",
            Self::MissingParent { .. } => "E2015",
            Self::ParentOrder { .. } => "E2016",
            Self::InvalidSpan(_) => "E2017",
            Self::InvalidEnvironment { .. } => "E2018",
            Self::DuplicateCleanup { .. } | Self::InvalidCleanup { .. } => "E2019",
            Self::Admission { .. } => "E2020",
        }
    }
}

impl fmt::Display for LowerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTarget => formatter.write_str("test target has an empty identity"),
            Self::MainEntry => {
                formatter.write_str("main is not lowered or executed in a test target")
            }
            Self::EmptyNode => formatter.write_str("test entry has an empty identity"),
            Self::ContractIdentity { input, contract } => {
                write!(
                    formatter,
                    "entry `{input}` does not match checked contract `{contract}`"
                )
            }
            Self::DuplicateNode(node) => write!(formatter, "test node `{node}` is duplicated"),
            Self::MissingParent { node, parent } => {
                write!(
                    formatter,
                    "test node `{node}` refers to missing parent `{parent}`"
                )
            }
            Self::ParentOrder { node, parent } => write!(
                formatter,
                "parent `{parent}` must be declared before child `{node}`"
            ),
            Self::InvalidSpan(file) => write!(formatter, "source span in `{file}` is invalid"),
            Self::InvalidEnvironment { node, binding } => write!(
                formatter,
                "environment snapshot `{binding}` in `{node}` is invalid or duplicated"
            ),
            Self::DuplicateCleanup { node, hook } => {
                write!(formatter, "cleanup hook `{hook}` in `{node}` is duplicated")
            }
            Self::InvalidCleanup { node } => write!(formatter, "cleanup in `{node}` is invalid"),
            Self::Admission { node, message } => {
                write!(formatter, "admission failed for `{node}`: {message}")
            }
        }
    }
}

impl Error for LowerError {}

/// Lower one target through the common HIR/MIR/bytecode boundary.
pub fn lower(input: TestLowerInput) -> Result<TestArtifact, LowerError> {
    if input.target.trim().is_empty() {
        return Err(LowerError::EmptyTarget);
    }
    if input.main_present {
        return Err(LowerError::MainEntry);
    }

    let mut entries = input.entries;
    entries.sort_by(|left, right| {
        (&left.span.file, left.span.start, left.span.end, &left.node).cmp(&(
            &right.span.file,
            right.span.start,
            right.span.end,
            &right.node,
        ))
    });

    let mut identity_by_node = BTreeMap::new();
    for entry in &entries {
        validate_entry_input(entry)?;
        if identity_by_node
            .insert(
                entry.node.clone(),
                TestIdentity::new(&input.target, &entry.node, entry.contract.kind()),
            )
            .is_some()
        {
            return Err(LowerError::DuplicateNode(entry.node.clone()));
        }
    }

    let mut index_by_node = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        index_by_node.insert(entry.node.clone(), index);
    }

    let mut lowered = Vec::with_capacity(entries.len());
    for entry in entries {
        let parent = match entry.parent.as_deref() {
            Some(parent) => {
                let parent_index =
                    *index_by_node
                        .get(parent)
                        .ok_or_else(|| LowerError::MissingParent {
                            node: entry.node.clone(),
                            parent: parent.into(),
                        })?;
                let child_index = index_by_node[&entry.node];
                if parent_index >= child_index {
                    return Err(LowerError::ParentOrder {
                        node: entry.node.clone(),
                        parent: parent.into(),
                    });
                }
                Some(identity_by_node[parent].clone())
            }
            None => None,
        };
        let operations = entry
            .contract
            .operations()
            .iter()
            .map(LoweredOperation::from_test_operation)
            .collect::<Vec<_>>();
        let errors = entry
            .contract
            .errors()
            .iter()
            .map(LoweredError::from_member)
            .collect::<Vec<_>>();
        let mut environment = entry.environment;
        environment.sort();
        let cleanup = CleanupPlan::new(entry.contract.contains_defer(), entry.cleanup_hooks);
        let identity = identity_by_node[entry.node.as_str()].clone();
        lowered.push(LoweredTestEntry {
            identity,
            parent,
            span: entry.span,
            environment,
            domain: entry.domain,
            errors,
            is_async: entry.contract.is_async(),
            cleanup,
            hir: operations.clone(),
            mir: operations.clone(),
            bytecode: operations,
        });
    }

    let artifact_hash = sha256(&canonical_artifact_bytes(&input.target, &lowered));
    let artifact = TestArtifact {
        target: input.target,
        entries: lowered,
        artifact_hash,
    };
    verify(&artifact)?;
    Ok(artifact)
}

fn validate_entry_input(entry: &TestLowerEntryInput) -> Result<(), LowerError> {
    if entry.node.trim().is_empty() {
        return Err(LowerError::EmptyNode);
    }
    if entry.contract.node() != entry.node {
        return Err(LowerError::ContractIdentity {
            input: entry.node.clone(),
            contract: entry.contract.node().into(),
        });
    }
    if entry.span.file.trim().is_empty()
        || entry.span.file.contains(['\n', '\r'])
        || entry.span.start > entry.span.end
    {
        return Err(LowerError::InvalidSpan(entry.span.file.clone()));
    }
    let mut bindings = BTreeSet::new();
    for snapshot in &entry.environment {
        if snapshot.binding.trim().is_empty()
            || snapshot.type_name.trim().is_empty()
            || snapshot.digest.trim().is_empty()
            || snapshot.binding.contains(['\n', '\r'])
            || !bindings.insert(snapshot.binding.clone())
        {
            return Err(LowerError::InvalidEnvironment {
                node: entry.node.clone(),
                binding: snapshot.binding.clone(),
            });
        }
    }
    let mut hooks = BTreeSet::new();
    for hook in &entry.cleanup_hooks {
        if hook.trim().is_empty() || hook.contains(['\n', '\r']) {
            return Err(LowerError::InvalidCleanup {
                node: entry.node.clone(),
            });
        }
        if !hooks.insert(hook.clone()) {
            return Err(LowerError::DuplicateCleanup {
                node: entry.node.clone(),
                hook: hook.clone(),
            });
        }
    }
    if entry.domain.input().trim().is_empty()
        || entry.domain.output().trim().is_empty()
        || entry.domain.input().contains(['\n', '\r'])
        || entry.domain.output().contains(['\n', '\r'])
    {
        return Err(LowerError::InvalidCleanup {
            node: entry.node.clone(),
        });
    }
    Ok(())
}

/// Re-check a lowered artifact at the admission boundary.
pub fn verify(artifact: &TestArtifact) -> Result<(), LowerError> {
    if artifact.target.trim().is_empty() {
        return Err(LowerError::EmptyTarget);
    }
    let mut seen = BTreeSet::new();
    let mut positions = BTreeMap::new();
    for (index, entry) in artifact.entries.iter().enumerate() {
        let node = entry.identity.node().to_owned();
        if !seen.insert(node.clone()) {
            return Err(LowerError::DuplicateNode(node));
        }
        positions.insert(node, index);
        if entry.identity.target() != artifact.target
            || entry.identity.digest()
                != TestIdentity::new(
                    artifact.target(),
                    entry.identity.node(),
                    entry.identity.kind(),
                )
                .digest()
        {
            return Err(LowerError::Admission {
                node: entry.identity.node().into(),
                message: "identity is not canonical".into(),
            });
        }
        if entry.hir != entry.mir || entry.mir != entry.bytecode {
            return Err(LowerError::Admission {
                node: entry.identity.node().into(),
                message: "HIR, MIR and bytecode operation streams diverge".into(),
            });
        }
        if entry
            .environment
            .windows(2)
            .any(|pair| pair[0].binding >= pair[1].binding)
            || entry
                .cleanup
                .hooks
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(LowerError::Admission {
                node: entry.identity.node().into(),
                message: "environment or cleanup metadata is not canonical".into(),
            });
        }
    }
    for (index, entry) in artifact.entries.iter().enumerate() {
        if let Some(parent) = entry.parent() {
            let Some(parent_index) = positions.get(parent.node()) else {
                return Err(LowerError::MissingParent {
                    node: entry.identity.node().into(),
                    parent: parent.node().into(),
                });
            };
            if *parent_index >= index {
                return Err(LowerError::ParentOrder {
                    node: entry.identity.node().into(),
                    parent: parent.node().into(),
                });
            }
        }
    }
    let expected_hash = sha256(&canonical_artifact_bytes(
        &artifact.target,
        &artifact.entries,
    ));
    if expected_hash != artifact.artifact_hash {
        return Err(LowerError::Admission {
            node: artifact.target.clone(),
            message: "artifact hash does not match canonical bytes".into(),
        });
    }
    Ok(())
}

fn canonical_artifact_bytes(target: &str, entries: &[LoweredTestEntry]) -> Vec<u8> {
    let mut output = String::new();
    push_field(&mut output, TEST_LOWER_FORMAT);
    push_field(&mut output, target);
    for entry in entries {
        push_field(&mut output, entry.identity.target());
        push_field(&mut output, entry.identity.node());
        push_field(&mut output, entry.identity.kind().as_str());
        push_field(&mut output, entry.identity.digest());
        push_field(
            &mut output,
            entry.parent.as_ref().map_or("", TestIdentity::node),
        );
        push_field(&mut output, entry.span.file());
        push_field(&mut output, &entry.span.start().to_string());
        push_field(&mut output, &entry.span.end().to_string());
        push_field(&mut output, if entry.is_async { "async" } else { "sync" });
        push_field(
            &mut output,
            if entry.cleanup.deferred {
                "deferred"
            } else {
                "plain"
            },
        );
        for snapshot in &entry.environment {
            push_field(&mut output, "env");
            push_field(&mut output, snapshot.binding());
            push_field(&mut output, snapshot.type_name());
            push_field(&mut output, snapshot.digest());
        }
        for hook in &entry.cleanup.hooks {
            push_field(&mut output, "cleanup");
            push_field(&mut output, hook);
        }
        push_field(&mut output, entry.domain.input());
        push_field(&mut output, entry.domain.output());
        for error in &entry.errors {
            push_field(&mut output, "error");
            push_field(&mut output, error.name());
            push_field(&mut output, if error.discard { "discard" } else { "keep" });
        }
        for operation in &entry.hir {
            push_operation(&mut output, operation);
        }
    }
    output.into_bytes()
}

fn push_field(output: &mut String, value: &str) {
    use std::fmt::Write as _;
    write!(output, "{}:", value.len()).expect("writing to String cannot fail");
    output.push_str(value);
    output.push('|');
}

fn push_operation(output: &mut String, operation: &LoweredOperation) {
    push_field(output, &operation.to_string());
    match operation {
        LoweredOperation::TestLog { message }
        | LoweredOperation::TestTags { values: message }
        | LoweredOperation::TestFailNow { message }
        | LoweredOperation::TestSkip { reason: message } => push_field(output, message),
        LoweredOperation::TestAttach {
            name,
            media_type,
            bytes,
        } => {
            push_field(output, name);
            push_field(output, media_type);
            push_field(output, bytes);
        }
        LoweredOperation::TestSnapshot { name, actual } => {
            push_field(output, name);
            push_field(output, actual);
        }
        LoweredOperation::WithVirtualTime {
            is_async,
            send,
            call_once,
            returns_unit,
            accepts_ref,
            escapes_controller,
            shares_controller,
        } => {
            for value in [
                *is_async,
                *send,
                *call_once,
                *returns_unit,
                *accepts_ref,
                *escapes_controller,
                *shares_controller,
            ] {
                push_field(output, if value { "1" } else { "0" });
            }
        }
        LoweredOperation::VirtualTimeSettle => {}
        LoweredOperation::VirtualTimeAdvance { duration_ns } => {
            push_field(output, &duration_ns.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_check::{
        OrdinaryCheckFacts, ReturnShape, TestBodyInput, TestOperation, VirtualTimeClosureFacts,
    };

    fn contract(
        node: &str,
        kind: TestEntryKind,
        operations: Vec<TestOperation>,
    ) -> TestBodyContract {
        crate::test_check::check(
            TestBodyInput::new(node, kind)
                .with_return(ReturnShape::None)
                .with_facts(OrdinaryCheckFacts::valid())
                .with_operations(operations),
        )
        .expect("test contract should be valid")
    }

    fn entry(node: &str, start: u32, kind: TestEntryKind) -> TestLowerEntryInput {
        TestLowerEntryInput::new(
            node,
            SourceSpan::new("tests/main.to", start, start + 2),
            contract(node, kind, vec![]),
        )
    }

    #[test]
    fn lowers_all_operations_into_identical_common_streams() {
        let operations = vec![
            TestOperation::log(),
            TestOperation::tags(),
            TestOperation::fail_now(),
            TestOperation::skip(),
            TestOperation::attach("trace", "text/plain"),
            TestOperation::snapshot("golden"),
            TestOperation::with_virtual_time(VirtualTimeClosureFacts::new(
                true, true, true, true, true, false, false,
            )),
            TestOperation::settle(),
            TestOperation::advance(12),
        ];
        let artifact = lower(
            TestLowerInput::new("unit").with_entries([TestLowerEntryInput::new(
                "root",
                SourceSpan::new("tests/main.to", 0, 20),
                contract("root", TestEntryKind::Test, operations),
            )]),
        )
        .expect("lowering should succeed");
        let lowered = &artifact.entries()[0];
        assert_eq!(lowered.hir(), lowered.mir());
        assert_eq!(lowered.mir(), lowered.bytecode());
        assert_eq!(lowered.hir().len(), 9);
        assert_eq!(
            lowered.hir()[8],
            LoweredOperation::VirtualTimeAdvance { duration_ns: 12 }
        );
        assert!(artifact.verify().is_ok());
    }

    #[test]
    fn sorts_by_source_span_and_resolves_parent_identity() {
        let child = entry("child", 10, TestEntryKind::Test).with_parent("suite");
        let suite = entry("suite", 0, TestEntryKind::SuiteSetup);
        let artifact = lower(TestLowerInput::new("unit").with_entries([child, suite])).unwrap();
        assert_eq!(artifact.entries()[0].identity().node(), "suite");
        assert_eq!(artifact.entries()[1].parent().unwrap().node(), "suite");
        assert_ne!(
            artifact.entries()[0].identity().digest(),
            artifact.entries()[1].identity().digest()
        );
    }

    #[test]
    fn canonical_bytes_and_hash_are_insertion_order_independent() {
        let first = entry("first", 0, TestEntryKind::Test);
        let second = entry("second", 4, TestEntryKind::Test);
        let left = lower(TestLowerInput::new("unit").with_entries([first.clone(), second.clone()]))
            .unwrap();
        let right = lower(TestLowerInput::new("unit").with_entries([second, first])).unwrap();
        assert_eq!(left.canonical_bytes(), right.canonical_bytes());
        assert_eq!(left.artifact_hash(), right.artifact_hash());
    }

    #[test]
    fn preserves_environment_domain_errors_suspension_and_cleanup() {
        let body = contract(
            "suite",
            TestEntryKind::SuiteSetup,
            vec![TestOperation::settle()],
        );
        let input = TestLowerEntryInput::new("suite", SourceSpan::new("suite.to", 2, 9), body)
            .with_environment([EnvironmentSnapshot::new(
                "config",
                "Config",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            )])
            .with_domain(TestDomain::new("SuiteInput", "Unit ! SetupError"))
            .with_cleanup_hooks(["close-service".into()]);
        let artifact = lower(TestLowerInput::new("unit").with_entries([input])).unwrap();
        let lowered = &artifact.entries()[0];
        assert!(lowered.is_async());
        assert_eq!(lowered.environment()[0].binding(), "config");
        assert_eq!(lowered.domain().output(), "Unit ! SetupError");
        assert!(
            lowered
                .cleanup()
                .hooks()
                .iter()
                .any(|hook| hook == "close-service")
        );
    }

    #[test]
    fn rejects_main_empty_target_identity_mismatch_and_duplicate_nodes() {
        assert_eq!(
            lower(TestLowerInput::new("unit").with_main(true)).unwrap_err(),
            LowerError::MainEntry
        );
        assert_eq!(
            lower(TestLowerInput::new(" ")).unwrap_err(),
            LowerError::EmptyTarget
        );
        let mismatched = TestLowerEntryInput::new(
            "input",
            SourceSpan::new("test.to", 0, 1),
            contract("contract", TestEntryKind::Test, vec![]),
        );
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([mismatched])),
            Err(LowerError::ContractIdentity { .. })
        ));
        let duplicate = entry("same", 0, TestEntryKind::Test);
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([duplicate.clone(), duplicate])),
            Err(LowerError::DuplicateNode(_))
        ));
    }

    #[test]
    fn rejects_parent_and_metadata_boundaries() {
        let missing = entry("child", 1, TestEntryKind::Test).with_parent("missing");
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([missing])),
            Err(LowerError::MissingParent { .. })
        ));
        let backwards = entry("child", 0, TestEntryKind::Test).with_parent("parent");
        let parent = entry("parent", 4, TestEntryKind::SuiteSetup);
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([backwards, parent])),
            Err(LowerError::ParentOrder { .. })
        ));
        let invalid_span = TestLowerEntryInput::new(
            "node",
            SourceSpan::new("test.to", 4, 1),
            contract("node", TestEntryKind::Test, vec![]),
        );
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([invalid_span])),
            Err(LowerError::InvalidSpan(_))
        ));
        let duplicate_env = entry("node", 0, TestEntryKind::Test).with_environment([
            EnvironmentSnapshot::new("x", "Int", "digest"),
            EnvironmentSnapshot::new("x", "Int", "digest"),
        ]);
        assert!(matches!(
            lower(TestLowerInput::new("unit").with_entries([duplicate_env])),
            Err(LowerError::InvalidEnvironment { .. })
        ));
    }

    #[test]
    fn verifier_rejects_divergent_streams_and_hash_tampering() {
        let mut artifact = lower(TestLowerInput::new("unit").with_entries([entry(
            "node",
            0,
            TestEntryKind::Test,
        )]))
        .unwrap();
        artifact.entries[0].mir.push(LoweredOperation::TestSkip {
            reason: "String".into(),
        });
        assert!(matches!(
            artifact.verify(),
            Err(LowerError::Admission { .. })
        ));
        let mut artifact = lower(TestLowerInput::new("unit").with_entries([entry(
            "node",
            0,
            TestEntryKind::Test,
        )]))
        .unwrap();
        artifact.artifact_hash = "sha256:bad".into();
        assert!(matches!(
            artifact.verify(),
            Err(LowerError::Admission { .. })
        ));
    }

    #[test]
    fn cleanup_and_environment_order_are_canonicalized() {
        let input = entry("node", 0, TestEntryKind::Test)
            .with_environment([
                EnvironmentSnapshot::new("z", "Int", "z"),
                EnvironmentSnapshot::new("a", "Int", "a"),
            ])
            .with_cleanup_hooks(["z-hook".into(), "a-hook".into()]);
        let artifact = lower(TestLowerInput::new("unit").with_entries([input])).unwrap();
        assert_eq!(artifact.entries()[0].environment()[0].binding(), "a");
        assert_eq!(artifact.entries()[0].cleanup().hooks()[0], "a-hook");
    }

    #[test]
    fn operation_display_and_unknown_operand_are_stable() {
        let operation = LoweredOperation::TestAttach {
            name: "x".into(),
            media_type: "text/plain".into(),
            bytes: "Bytes".into(),
        };
        assert_eq!(operation.to_string(), "TestAttach");
        assert_eq!(
            operand_name(&OperandShape::Other("Custom".into())),
            "Other(Custom)"
        );
    }
}
