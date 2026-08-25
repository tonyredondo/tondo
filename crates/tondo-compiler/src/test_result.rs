//! Validated execution results and the coordinator/worker protocol for Tondo tests.
//!
//! This module is deliberately independent of test bodies, the host and the
//! VM.  It owns the one canonical result tree that JSON, human and JUnit
//! reporters will consume, plus the bounded wire protocol used to transport
//! attempts between a coordinator and an isolated worker.
//!
//! The wire model intentionally keeps complete attempt/error payloads inline:
//! these values are serialized as one canonical event and are not hot-path
//! allocations. The corresponding Clippy size lints are therefore suppressed
//! at this protocol boundary rather than changing the public representation.

#![allow(clippy::large_enum_variant, clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::validate_sha256;

pub const TEST_REPORT_FORMAT: &str = "tondo-test-report-0.1/7";
pub const TEST_WORKER_PROTOCOL_FORMAT: &str = "tondo-test-worker-0.1/1";
pub const DIAGNOSTIC_REPORT_FORMAT: &str = "tondo-diagnostic-report/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResultNodeKind {
    Suite,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptStatus {
    Passed,
    Skipped,
    FailedError,
    FailedPanic,
    ResourceLimit,
    Timeout,
    Infrastructure,
    BlockedSetup,
    BlockedSkip,
}

impl AttemptStatus {
    fn is_failure(self) -> bool {
        matches!(
            self,
            Self::FailedError
                | Self::FailedPanic
                | Self::ResourceLimit
                | Self::Timeout
                | Self::Infrastructure
        )
    }

    fn is_blocked(self) -> bool {
        matches!(self, Self::BlockedSetup | Self::BlockedSkip)
    }

    fn aggregate(self) -> AggregateStatus {
        match self {
            Self::Passed => AggregateStatus::Passed,
            Self::Skipped => AggregateStatus::Skipped,
            Self::FailedError => AggregateStatus::FailedError,
            Self::FailedPanic => AggregateStatus::FailedPanic,
            Self::ResourceLimit => AggregateStatus::ResourceLimit,
            Self::Timeout => AggregateStatus::Timeout,
            Self::Infrastructure => AggregateStatus::Infrastructure,
            Self::BlockedSetup => AggregateStatus::BlockedSetup,
            Self::BlockedSkip => AggregateStatus::BlockedSkip,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AggregateStatus {
    Passed,
    FlakyPass,
    Skipped,
    FailedError,
    FailedPanic,
    ResourceLimit,
    Timeout,
    Infrastructure,
    BlockedSetup,
    BlockedSkip,
}

impl AggregateStatus {
    fn is_failure(self) -> bool {
        matches!(
            self,
            Self::FailedError
                | Self::FailedPanic
                | Self::ResourceLimit
                | Self::Timeout
                | Self::Infrastructure
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptPhase {
    Setup,
    Teardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetryUnitKind {
    Test,
    Suite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub file: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecord {
    pub kind: String,
    pub code: Option<String>,
    pub message: String,
    pub source: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipRecord {
    pub reason: String,
    pub source: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    pub name: String,
    pub media_type: String,
    #[serde(
        serialize_with = "serialize_u64_string",
        deserialize_with = "deserialize_u64_string"
    )]
    pub size: u64,
    pub sha256: String,
    pub object: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticStatus {
    Clean,
    Finding,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPrivacy {
    pub payloads: String,
    pub secrets: String,
    pub paths: String,
    pub network_upload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRecord {
    pub format: String,
    pub run_id: String,
    pub attempt_id: String,
    pub shard: String,
    pub profile: String,
    pub status: DiagnosticStatus,
    pub target: String,
    pub backend: String,
    pub toolchain: String,
    pub source_revision: String,
    pub observations: u64,
    pub limitations: Vec<String>,
    pub artifacts: Vec<ArtifactRecord>,
    pub privacy: DiagnosticPrivacy,
    pub program_exit_status: Option<i32>,
    pub command_exit_status: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotStatus {
    Matched,
    Missing,
    Mismatched,
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecord {
    pub name: String,
    pub status: SnapshotStatus,
    pub expected_sha256: Option<String>,
    pub actual_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualTimeRecord {
    pub index: u32,
    pub elapsed_ns: String,
    pub automatic_advances: u32,
    pub explicit_advances: u32,
    pub settles: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockedBy {
    pub id: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestAttempt {
    pub index: u32,
    pub iteration: u32,
    pub round: u32,
    pub unit: Option<u32>,
    pub status: AttemptStatus,
    pub phase: Option<AttemptPhase>,
    pub blocked_by: Option<BlockedBy>,
    pub failure: Option<FailureRecord>,
    pub skip: Option<SkipRecord>,
    pub tags: BTreeMap<String, String>,
    pub artifacts: Vec<ArtifactRecord>,
    pub snapshots: Vec<SnapshotRecord>,
    pub virtual_time: Vec<VirtualTimeRecord>,
    pub logs: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticRecord>,
}

impl TestAttempt {
    pub fn new(
        index: u32,
        iteration: u32,
        round: u32,
        unit: Option<u32>,
        status: AttemptStatus,
    ) -> Self {
        Self {
            index,
            iteration,
            round,
            unit,
            status,
            phase: None,
            blocked_by: None,
            failure: None,
            skip: None,
            tags: BTreeMap::new(),
            artifacts: Vec::new(),
            snapshots: Vec::new(),
            virtual_time: Vec::new(),
            logs: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestNode {
    pub id: String,
    pub parent: Option<String>,
    pub package: String,
    pub kind: ResultNodeKind,
    pub module: String,
    pub path: Vec<String>,
    pub name: String,
    pub source: Option<SourceSpan>,
    pub owners: Vec<String>,
    pub status: AggregateStatus,
    pub decisive_attempt: u32,
    pub attempts: Vec<TestAttempt>,
}

impl TestNode {
    pub fn new(
        id: impl Into<String>,
        parent: Option<String>,
        package: impl Into<String>,
        kind: ResultNodeKind,
        module: impl Into<String>,
        name: impl Into<String>,
        attempts: Vec<TestAttempt>,
    ) -> Self {
        Self {
            id: id.into(),
            parent,
            package: package.into(),
            kind,
            module: module.into(),
            path: Vec::new(),
            name: name.into(),
            source: None,
            owners: Vec::new(),
            status: AggregateStatus::Infrastructure,
            decisive_attempt: 0,
            attempts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryUnit {
    pub kind: RetryUnitKind,
    pub id: String,
    pub execution_plan: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultPolicy {
    pub jobs: u32,
    pub deny_skips: bool,
    pub allow_flaky: bool,
    pub max_additional_rounds: u32,
    pub repeat_count: u32,
}

impl Default for ResultPolicy {
    fn default() -> Self {
        Self {
            jobs: 1,
            deny_skips: false,
            allow_flaky: false,
            max_additional_rounds: 0,
            repeat_count: 1,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSummary {
    pub selected: u32,
    pub executed: u32,
    pub passed: u32,
    pub flaky_passed: u32,
    pub skipped: u32,
    pub blocked_setup: u32,
    pub blocked_skip: u32,
    pub failed_error: u32,
    pub failed_panic: u32,
    pub resource_limit: u32,
    pub timeout: u32,
    pub infrastructure: u32,
    pub retried: u32,
    pub repeated: u32,
    pub test_attempts: u32,
    pub suite_selected: u32,
    pub suite_passed: u32,
    pub suite_flaky_passed: u32,
    pub suite_skipped: u32,
    pub suite_blocked_setup: u32,
    pub suite_blocked_skip: u32,
    pub suite_failed: u32,
    pub suite_retried: u32,
    pub suite_repeated: u32,
    pub suite_attempts: u32,
    pub artifacts: u32,
    #[serde(
        serialize_with = "serialize_u64_string",
        deserialize_with = "deserialize_u64_string"
    )]
    pub artifact_bytes: u64,
    pub snapshots: u32,
    pub snapshot_matched: u32,
    pub snapshot_missing: u32,
    pub snapshot_mismatched: u32,
    pub snapshot_created: u32,
    pub snapshot_updated: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestResultTree {
    pub format: String,
    pub edition: String,
    pub execution_plan: Vec<String>,
    pub policy: ResultPolicy,
    pub suites: Vec<TestNode>,
    pub tests: Vec<TestNode>,
    pub summary: ResultSummary,
}

impl TestResultTree {
    /// Build the canonical tree from node attempts, deriving aggregate status
    /// and summary exactly once for all reporters.
    pub fn assemble(
        execution_plan: Vec<String>,
        policy: ResultPolicy,
        mut suites: Vec<TestNode>,
        mut tests: Vec<TestNode>,
    ) -> Result<Self, ResultModelError> {
        for node in suites.iter_mut().chain(tests.iter_mut()) {
            let (status, decisive_attempt) = aggregate_attempts(&node.attempts)?;
            node.status = status;
            node.decisive_attempt = decisive_attempt;
        }
        let summary = summarize(&execution_plan, &suites, &tests)?;
        let tree = Self {
            format: TEST_REPORT_FORMAT.into(),
            edition: "0.1".into(),
            execution_plan,
            policy,
            suites,
            tests,
            summary,
        };
        tree.validate()?;
        Ok(tree.canonicalized())
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ResultModelError> {
        let tree: Self = serde_json::from_slice(bytes)
            .map_err(|error| ResultModelError::InvalidJson(error.to_string()))?;
        tree.validate()?;
        Ok(tree.canonicalized())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ResultModelError> {
        serde_json::to_vec(&self.canonicalized())
            .map_err(|error| ResultModelError::Serialization(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), ResultModelError> {
        if self.format != TEST_REPORT_FORMAT {
            return Err(ResultModelError::InvalidField {
                field: "format",
                message: format!("expected `{TEST_REPORT_FORMAT}`"),
            });
        }
        if self.edition != "0.1" {
            return Err(ResultModelError::InvalidField {
                field: "edition",
                message: "expected `0.1`".into(),
            });
        }
        if self.policy.jobs == 0 {
            return Err(ResultModelError::InvalidField {
                field: "policy.jobs",
                message: "must be positive".into(),
            });
        }
        if self.policy.repeat_count == 0 {
            return Err(ResultModelError::InvalidField {
                field: "policy.repeat_count",
                message: "must be positive".into(),
            });
        }
        if self.policy.repeat_count > 1 && self.policy.max_additional_rounds > 0 {
            return Err(ResultModelError::InvalidField {
                field: "policy",
                message: "retry and repeat cannot be active together".into(),
            });
        }

        let mut ids = BTreeSet::new();
        for node in self.suites.iter().chain(self.tests.iter()) {
            validate_node(node, &mut ids)?;
        }
        let suite_ids = self
            .suites
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        let test_ids = self
            .tests
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        for node in self.suites.iter().chain(self.tests.iter()) {
            if let Some(parent) = node.parent.as_deref() {
                if node.kind == ResultNodeKind::Suite {
                    if !suite_ids.contains(parent) {
                        return Err(ResultModelError::InvalidReference {
                            field: "parent",
                            value: parent.into(),
                        });
                    }
                } else if !suite_ids.contains(parent) {
                    return Err(ResultModelError::InvalidReference {
                        field: "parent",
                        value: parent.into(),
                    });
                }
            }
            for attempt in &node.attempts {
                if let Some(blocked_by) = &attempt.blocked_by {
                    if !suite_ids.contains(blocked_by.id.as_str()) {
                        return Err(ResultModelError::InvalidReference {
                            field: "blocked_by.id",
                            value: blocked_by.id.clone(),
                        });
                    }
                    let target = self
                        .suites
                        .iter()
                        .find(|suite| suite.id == blocked_by.id)
                        .expect("suite id set and suite list must agree");
                    if !target
                        .attempts
                        .iter()
                        .any(|candidate| candidate.index == blocked_by.attempt)
                    {
                        return Err(ResultModelError::InvalidReference {
                            field: "blocked_by.attempt",
                            value: format!("{}#{}", blocked_by.id, blocked_by.attempt),
                        });
                    }
                }
            }
        }
        if self.execution_plan.len() != self.tests.len() {
            return Err(ResultModelError::InvalidField {
                field: "execution_plan",
                message: "must contain exactly one entry per test".into(),
            });
        }
        let mut selected = BTreeSet::new();
        for id in &self.execution_plan {
            if !selected.insert(id.as_str()) || !test_ids.contains(id.as_str()) {
                return Err(ResultModelError::InvalidReference {
                    field: "execution_plan",
                    value: id.clone(),
                });
            }
        }
        let expected = summarize(&self.execution_plan, &self.suites, &self.tests)?;
        if expected != self.summary {
            return Err(ResultModelError::SummaryMismatch {
                expected,
                actual: self.summary.clone(),
            });
        }
        Ok(())
    }

    pub fn suites(&self) -> &[TestNode] {
        &self.suites
    }

    pub fn tests(&self) -> &[TestNode] {
        &self.tests
    }

    pub fn summary(&self) -> &ResultSummary {
        &self.summary
    }

    fn canonicalized(&self) -> Self {
        let mut value = self.clone();
        value.suites.sort_by(|left, right| left.id.cmp(&right.id));
        value.tests.sort_by(|left, right| left.id.cmp(&right.id));
        for node in value.suites.iter_mut().chain(value.tests.iter_mut()) {
            for attempt in &mut node.attempts {
                attempt
                    .artifacts
                    .sort_by(|left, right| left.name.cmp(&right.name));
                attempt
                    .snapshots
                    .sort_by(|left, right| left.name.cmp(&right.name));
                attempt.virtual_time.sort_by_key(|entry| entry.index);
            }
        }
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolLimits {
    pub timeout_ms: u64,
    pub setup_timeout_ms: u64,
    pub teardown_timeout_ms: u64,
    pub output_bytes: u64,
    pub artifact_bytes: u64,
    pub snapshot_bytes: u64,
    pub memory_bytes: u64,
    pub instructions: u64,
    pub virtual_timers: u64,
}

impl ProtocolLimits {
    fn validate(&self) -> Result<(), ProtocolError> {
        let fields = [
            ("timeout_ms", self.timeout_ms),
            ("setup_timeout_ms", self.setup_timeout_ms),
            ("teardown_timeout_ms", self.teardown_timeout_ms),
            ("output_bytes", self.output_bytes),
            ("artifact_bytes", self.artifact_bytes),
            ("snapshot_bytes", self.snapshot_bytes),
            ("memory_bytes", self.memory_bytes),
            ("instructions", self.instructions),
            ("virtual_timers", self.virtual_timers),
        ];
        if let Some((field, _)) = fields.into_iter().find(|(_, value)| *value == 0) {
            return Err(ProtocolError::InvalidField {
                field,
                message: "must be positive".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum CoordinatorCommand {
    Hello {
        worker_id: String,
        target: String,
        plan_sha256: String,
        limits: ProtocolLimits,
    },
    Run {
        unit: RetryUnit,
        iteration: u32,
        round: u32,
    },
    Cancel {
        reason: String,
        grace_ms: u64,
    },
    Shutdown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "event")]
pub enum WorkerEvent {
    Ready {
        worker_id: String,
    },
    Started {
        unit: RetryUnit,
        iteration: u32,
        round: u32,
    },
    Attempt {
        node_id: String,
        attempt: TestAttempt,
    },
    Finished {
        clean: bool,
    },
    Cancelled {
        complete: bool,
    },
    Closed {
        clean: bool,
    },
    Error {
        kind: String,
        message: String,
        fatal: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorFrame {
    pub format: String,
    pub run_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub command: CoordinatorCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFrame {
    pub format: String,
    pub run_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub event: WorkerEvent,
}

impl CoordinatorFrame {
    pub fn new(run_id: impl Into<String>, sequence: u64, command: CoordinatorCommand) -> Self {
        Self {
            format: TEST_WORKER_PROTOCOL_FORMAT.into(),
            run_id: run_id.into(),
            sequence,
            command,
        }
    }
}

impl WorkerFrame {
    pub fn new(run_id: impl Into<String>, sequence: u64, event: WorkerEvent) -> Self {
        Self {
            format: TEST_WORKER_PROTOCOL_FORMAT.into(),
            run_id: run_id.into(),
            sequence,
            event,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolState {
    New,
    AwaitReady,
    Ready,
    Running,
    Cancelling,
    ShuttingDown,
    Closed,
}

/// Stateful, direction-aware validation of one worker conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSession {
    run_id: String,
    worker_id: String,
    state: ProtocolState,
    next_coordinator_sequence: u64,
    next_worker_sequence: u64,
}

impl ProtocolSession {
    pub fn new(
        run_id: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Result<Self, ProtocolError> {
        let run_id = non_empty("run_id", run_id.into())?;
        let worker_id = non_empty("worker_id", worker_id.into())?;
        Ok(Self {
            run_id,
            worker_id,
            state: ProtocolState::New,
            next_coordinator_sequence: 1,
            next_worker_sequence: 1,
        })
    }

    pub fn accept_coordinator(&mut self, frame: &CoordinatorFrame) -> Result<(), ProtocolError> {
        self.check_frame(
            frame.format.as_str(),
            frame.run_id.as_str(),
            frame.sequence,
            true,
        )?;
        match (&self.state, &frame.command) {
            (
                ProtocolState::New,
                CoordinatorCommand::Hello {
                    worker_id,
                    target,
                    plan_sha256,
                    limits,
                },
            ) => {
                if worker_id != &self.worker_id {
                    return Err(ProtocolError::InvalidField {
                        field: "hello.worker_id",
                        message: "does not match the session worker".into(),
                    });
                }
                non_empty("hello.target", target.clone())?;
                validate_sha256(plan_sha256).map_err(|error| ProtocolError::InvalidField {
                    field: "hello.plan_sha256",
                    message: error.to_string(),
                })?;
                limits.validate()?;
                self.state = ProtocolState::AwaitReady;
            }
            (
                ProtocolState::Ready,
                CoordinatorCommand::Run {
                    unit,
                    iteration,
                    round,
                },
            ) => {
                validate_unit(unit)?;
                if *iteration == 0 {
                    return Err(ProtocolError::InvalidField {
                        field: "run.iteration",
                        message: "must be positive".into(),
                    });
                }
                if *round == 0 && unit.execution_plan.is_empty() {
                    return Err(ProtocolError::InvalidField {
                        field: "run.unit.execution_plan",
                        message: "must not be empty".into(),
                    });
                }
                self.state = ProtocolState::Running;
            }
            (
                ProtocolState::AwaitReady | ProtocolState::Ready | ProtocolState::Running,
                CoordinatorCommand::Cancel { reason, grace_ms },
            ) => {
                non_empty("cancel.reason", reason.clone())?;
                self.ensure_positive("cancel.grace_ms", *grace_ms)?;
                self.state = ProtocolState::Cancelling;
            }
            (
                ProtocolState::New
                | ProtocolState::AwaitReady
                | ProtocolState::Ready
                | ProtocolState::Running
                | ProtocolState::Cancelling,
                CoordinatorCommand::Shutdown { reason },
            ) => {
                non_empty("shutdown.reason", reason.clone())?;
                self.state = ProtocolState::ShuttingDown;
            }
            (state, command) => {
                return Err(ProtocolError::Unexpected {
                    state: format!("{state:?}"),
                    message: format!("coordinator command `{command:?}"),
                });
            }
        }
        self.next_coordinator_sequence += 1;
        Ok(())
    }

    pub fn accept_worker(&mut self, frame: &WorkerFrame) -> Result<(), ProtocolError> {
        self.check_frame(
            frame.format.as_str(),
            frame.run_id.as_str(),
            frame.sequence,
            false,
        )?;
        match (&self.state, &frame.event) {
            (ProtocolState::AwaitReady, WorkerEvent::Ready { worker_id }) => {
                if worker_id != &self.worker_id {
                    return Err(ProtocolError::InvalidField {
                        field: "ready.worker_id",
                        message: "does not match the session worker".into(),
                    });
                }
                self.state = ProtocolState::Ready;
            }
            (
                ProtocolState::Running,
                WorkerEvent::Started {
                    unit,
                    iteration,
                    round,
                },
            ) => {
                validate_unit(unit)?;
                if *iteration == 0 {
                    return Err(ProtocolError::InvalidField {
                        field: "started.iteration",
                        message: "must be positive".into(),
                    });
                }
                if *round == 0 && unit.execution_plan.is_empty() {
                    return Err(ProtocolError::InvalidField {
                        field: "started.unit.execution_plan",
                        message: "must not be empty".into(),
                    });
                }
            }
            (ProtocolState::Running, WorkerEvent::Attempt { node_id, attempt }) => {
                non_empty("attempt.node_id", node_id.clone())?;
                validate_attempt(attempt, ResultNodeKind::Test, None, 0).map_err(|error| {
                    ProtocolError::InvalidField {
                        field: "attempt",
                        message: error.to_string(),
                    }
                })?;
            }
            (ProtocolState::Running, WorkerEvent::Finished { .. }) => {
                self.state = ProtocolState::Ready;
            }
            (ProtocolState::Cancelling, WorkerEvent::Cancelled { complete }) => {
                if *complete {
                    self.state = ProtocolState::Ready;
                }
            }
            (ProtocolState::ShuttingDown, WorkerEvent::Closed { clean: _ }) => {
                self.state = ProtocolState::Closed;
            }
            (
                _,
                WorkerEvent::Error {
                    kind,
                    message,
                    fatal,
                },
            ) => {
                non_empty("error.kind", kind.clone())?;
                non_empty("error.message", message.clone())?;
                if *fatal {
                    self.state = ProtocolState::Closed;
                }
            }
            (state, event) => {
                return Err(ProtocolError::Unexpected {
                    state: format!("{state:?}"),
                    message: format!("worker event `{event:?}"),
                });
            }
        }
        self.next_worker_sequence += 1;
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.state == ProtocolState::Closed
    }

    fn check_frame(
        &self,
        format: &str,
        run_id: &str,
        sequence: u64,
        coordinator: bool,
    ) -> Result<(), ProtocolError> {
        if format != TEST_WORKER_PROTOCOL_FORMAT {
            return Err(ProtocolError::InvalidField {
                field: "format",
                message: format!("expected `{TEST_WORKER_PROTOCOL_FORMAT}`"),
            });
        }
        if run_id != self.run_id {
            return Err(ProtocolError::InvalidField {
                field: "run_id",
                message: "does not match the session".into(),
            });
        }
        let expected = if coordinator {
            self.next_coordinator_sequence
        } else {
            self.next_worker_sequence
        };
        if sequence != expected {
            return Err(ProtocolError::Sequence {
                expected,
                actual: sequence,
            });
        }
        if self.state == ProtocolState::Closed {
            return Err(ProtocolError::Unexpected {
                state: "Closed".into(),
                message: "no messages are accepted after closure".into(),
            });
        }
        Ok(())
    }

    fn ensure_positive(&self, field: &'static str, value: u64) -> Result<(), ProtocolError> {
        if value == 0 {
            return Err(ProtocolError::InvalidField {
                field,
                message: "must be positive".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultModelError {
    InvalidJson(String),
    InvalidField {
        field: &'static str,
        message: String,
    },
    InvalidReference {
        field: &'static str,
        value: String,
    },
    InvalidAttempt {
        node: String,
        message: String,
    },
    SummaryMismatch {
        expected: ResultSummary,
        actual: ResultSummary,
    },
    Serialization(String),
}

impl fmt::Display for ResultModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(formatter, "invalid test result JSON: {message}"),
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::InvalidReference { field, value } => {
                write!(formatter, "invalid {field} reference `{value}`")
            }
            Self::InvalidAttempt { node, message } => {
                write!(formatter, "invalid attempt for `{node}`: {message}")
            }
            Self::SummaryMismatch { .. } => {
                write!(formatter, "test result summary does not match its nodes")
            }
            Self::Serialization(message) => {
                write!(formatter, "cannot encode test result: {message}")
            }
        }
    }
}

impl Error for ResultModelError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidField {
        field: &'static str,
        message: String,
    },
    Sequence {
        expected: u64,
        actual: u64,
    },
    Unexpected {
        state: String,
        message: String,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Sequence { expected, actual } => {
                write!(formatter, "protocol sequence {actual}, expected {expected}")
            }
            Self::Unexpected { state, message } => write!(
                formatter,
                "unexpected protocol message in {state}: {message}"
            ),
        }
    }
}

impl Error for ProtocolError {}

fn aggregate_attempts(
    attempts: &[TestAttempt],
) -> Result<(AggregateStatus, u32), ResultModelError> {
    if attempts.is_empty() {
        return Err(ResultModelError::InvalidField {
            field: "attempts",
            message: "must not be empty".into(),
        });
    }
    let last = attempts.last().expect("non-empty attempts");
    if last.status == AttemptStatus::Passed {
        if attempts
            .iter()
            .all(|attempt| attempt.status == AttemptStatus::Passed)
        {
            Ok((AggregateStatus::Passed, last.index))
        } else {
            Ok((AggregateStatus::FlakyPass, last.index))
        }
    } else if let Some(attempt) = attempts
        .iter()
        .rev()
        .find(|attempt| attempt.status.is_failure())
    {
        Ok((attempt.status.aggregate(), attempt.index))
    } else {
        Ok((last.status.aggregate(), last.index))
    }
}

fn validate_node(node: &TestNode, ids: &mut BTreeSet<String>) -> Result<(), ResultModelError> {
    validate_text("id", &node.id)?;
    validate_text("package", &node.package)?;
    validate_text("module", &node.module)?;
    validate_text("name", &node.name)?;
    if !ids.insert(node.id.clone()) {
        return Err(ResultModelError::InvalidReference {
            field: "node.id",
            value: node.id.clone(),
        });
    }
    if node
        .path
        .iter()
        .any(|part| part.is_empty() || part.contains(['\n', '\r']))
    {
        return Err(ResultModelError::InvalidField {
            field: "path",
            message: "components must be non-empty and line-break free".into(),
        });
    }
    for owner in &node.owners {
        validate_text("owners", owner)?;
    }
    if let Some(source) = &node.source {
        validate_text("source.file", &source.file)?;
        if source.start > source.end {
            return Err(ResultModelError::InvalidField {
                field: "source",
                message: "start must not exceed end".into(),
            });
        }
    }
    for (position, attempt) in node.attempts.iter().enumerate() {
        validate_attempt(attempt, node.kind, Some(node.id.as_str()), position as u32)?;
        let expected_index = position as u32 + 1;
        if attempt.index != expected_index {
            return Err(ResultModelError::InvalidAttempt {
                node: node.id.clone(),
                message: format!(
                    "indices must be contiguous from 1 (found {})",
                    attempt.index
                ),
            });
        }
    }
    let (status, decisive) = aggregate_attempts(&node.attempts)?;
    if node.status != status || node.decisive_attempt != decisive {
        return Err(ResultModelError::InvalidAttempt {
            node: node.id.clone(),
            message: "aggregate status or decisive_attempt is not derived from attempts".into(),
        });
    }
    Ok(())
}

fn validate_attempt(
    attempt: &TestAttempt,
    kind: ResultNodeKind,
    node: Option<&str>,
    _position: u32,
) -> Result<(), ResultModelError> {
    if attempt.index == 0 || attempt.iteration == 0 {
        return Err(ResultModelError::InvalidAttempt {
            node: node.unwrap_or("<protocol>").into(),
            message: "index and iteration must be positive".into(),
        });
    }
    if attempt.round == 0 && attempt.unit.is_some() {
        return Err(ResultModelError::InvalidAttempt {
            node: node.unwrap_or("<protocol>").into(),
            message: "round zero cannot carry a retry unit".into(),
        });
    }
    if attempt.round > 0 && attempt.unit.is_none() {
        return Err(ResultModelError::InvalidAttempt {
            node: node.unwrap_or("<protocol>").into(),
            message: "retry rounds require a unit index".into(),
        });
    }
    if kind == ResultNodeKind::Test && attempt.phase.is_some() {
        return Err(ResultModelError::InvalidAttempt {
            node: node.unwrap_or("<protocol>").into(),
            message: "test attempts cannot carry a phase".into(),
        });
    }
    match attempt.status {
        AttemptStatus::Passed | AttemptStatus::BlockedSetup | AttemptStatus::BlockedSkip => {
            if attempt.phase.is_some() || attempt.failure.is_some() || attempt.skip.is_some() {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "passed/blocked attempts cannot carry phase, failure or skip".into(),
                });
            }
            if attempt.status.is_blocked() && attempt.blocked_by.is_none() {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "blocked attempts require blocked_by".into(),
                });
            }
            if !attempt.status.is_blocked() && attempt.blocked_by.is_some() {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "only blocked attempts may carry blocked_by".into(),
                });
            }
        }
        AttemptStatus::Skipped => {
            if attempt.failure.is_some() || attempt.blocked_by.is_some() || attempt.skip.is_none() {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "skipped attempts require only a skip payload".into(),
                });
            }
            if kind == ResultNodeKind::Suite && attempt.phase != Some(AttemptPhase::Setup) {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "suite skips occur during setup".into(),
                });
            }
        }
        status @ (AttemptStatus::FailedError
        | AttemptStatus::FailedPanic
        | AttemptStatus::ResourceLimit
        | AttemptStatus::Timeout
        | AttemptStatus::Infrastructure) => {
            if attempt.failure.is_none() || attempt.skip.is_some() || attempt.blocked_by.is_some() {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "failure attempts require failure and no skip/block payload".into(),
                });
            }
            if kind == ResultNodeKind::Suite
                && attempt.phase == Some(AttemptPhase::Teardown)
                && !matches!(
                    status,
                    AttemptStatus::FailedPanic
                        | AttemptStatus::ResourceLimit
                        | AttemptStatus::Timeout
                        | AttemptStatus::Infrastructure
                )
            {
                return Err(ResultModelError::InvalidAttempt {
                    node: node.unwrap_or("<protocol>").into(),
                    message: "teardown cannot report a recoverable error".into(),
                });
            }
        }
    }
    if let Some(skip) = &attempt.skip {
        validate_text("skip.reason", &skip.reason)?;
    }
    if let Some(failure) = &attempt.failure {
        validate_text("failure.kind", &failure.kind)?;
        validate_text("failure.message", &failure.message)?;
        if let Some(code) = &failure.code {
            validate_text("failure.code", code)?;
        }
    }
    for (name, value) in &attempt.tags {
        validate_text("tags.key", name)?;
        validate_text("tags.value", value)?;
    }
    let mut artifact_names = BTreeSet::new();
    for artifact in &attempt.artifacts {
        validate_text("artifact.name", &artifact.name)?;
        validate_text("artifact.media_type", &artifact.media_type)?;
        validate_sha256(&format!("sha256:{}", artifact.sha256)).map_err(|error| {
            ResultModelError::InvalidField {
                field: "artifact.sha256",
                message: error.to_string(),
            }
        })?;
        if !artifact_names.insert(&artifact.name) {
            return Err(ResultModelError::InvalidField {
                field: "artifacts",
                message: "names must be unique per attempt".into(),
            });
        }
    }
    let mut diagnostic_profiles = BTreeSet::new();
    for diagnostic in &attempt.diagnostics {
        if diagnostic.format != DIAGNOSTIC_REPORT_FORMAT {
            return Err(ResultModelError::InvalidField {
                field: "diagnostics.format",
                message: "unsupported diagnostic report format".into(),
            });
        }
        validate_text("diagnostics.run_id", &diagnostic.run_id)?;
        validate_text("diagnostics.attempt_id", &diagnostic.attempt_id)?;
        validate_text("diagnostics.shard", &diagnostic.shard)?;
        validate_text("diagnostics.profile", &diagnostic.profile)?;
        validate_text("diagnostics.target", &diagnostic.target)?;
        validate_text("diagnostics.backend", &diagnostic.backend)?;
        validate_text("diagnostics.toolchain", &diagnostic.toolchain)?;
        validate_text("diagnostics.source_revision", &diagnostic.source_revision)?;
        if !["race", "leaks", "crash"].contains(&diagnostic.profile.as_str()) {
            return Err(ResultModelError::InvalidField {
                field: "diagnostics.profile",
                message: "unknown profile".into(),
            });
        }
        if !diagnostic_profiles.insert(&diagnostic.profile) {
            return Err(ResultModelError::InvalidField {
                field: "diagnostics",
                message: "profiles must be unique per attempt".into(),
            });
        }
        if diagnostic.limitations.iter().any(|value| value.is_empty())
            || diagnostic
                .limitations
                .windows(2)
                .any(|window| window[0] >= window[1])
        {
            return Err(ResultModelError::InvalidField {
                field: "diagnostics.limitations",
                message: "limitations must be sorted, unique and non-empty".into(),
            });
        }
        if diagnostic.privacy.payloads != "omitted-by-default"
            || diagnostic.privacy.secrets != "never-emitted-by-default"
            || diagnostic.privacy.paths != "logical-only"
            || diagnostic.privacy.network_upload
        {
            return Err(ResultModelError::InvalidField {
                field: "diagnostics.privacy",
                message: "diagnostic privacy policy is fixed".into(),
            });
        }
        for artifact in &diagnostic.artifacts {
            if !artifact_names.contains(&artifact.name) {
                return Err(ResultModelError::InvalidField {
                    field: "diagnostics.artifacts",
                    message: "diagnostic artifacts must also be attempt artifacts".into(),
                });
            }
        }
    }
    let mut snapshot_names = BTreeSet::new();
    for snapshot in &attempt.snapshots {
        validate_text("snapshot.name", &snapshot.name)?;
        validate_hex("snapshot.actual_sha256", &snapshot.actual_sha256)?;
        if let Some(expected) = &snapshot.expected_sha256 {
            validate_hex("snapshot.expected_sha256", expected)?;
        }
        if !snapshot_names.insert(&snapshot.name) {
            return Err(ResultModelError::InvalidField {
                field: "snapshots",
                message: "names must be unique per attempt".into(),
            });
        }
    }
    for (position, virtual_time) in attempt.virtual_time.iter().enumerate() {
        if virtual_time.index != position as u32 + 1
            || virtual_time.elapsed_ns.parse::<u128>().is_err()
        {
            return Err(ResultModelError::InvalidField {
                field: "virtual_time",
                message: "indices and elapsed_ns must be canonical".into(),
            });
        }
    }
    Ok(())
}

fn summarize(
    execution_plan: &[String],
    suites: &[TestNode],
    tests: &[TestNode],
) -> Result<ResultSummary, ResultModelError> {
    let mut summary = ResultSummary {
        selected: tests.len() as u32,
        suite_selected: suites.len() as u32,
        ..ResultSummary::default()
    };
    for node in tests.iter().chain(suites.iter()) {
        let target = &mut summary;
        if node.kind == ResultNodeKind::Test {
            target.executed += u32::from(
                node.attempts
                    .iter()
                    .any(|attempt| !attempt.status.is_blocked()),
            );
            target.retried += u32::from(node.attempts.iter().any(|attempt| attempt.round > 0));
            target.repeated += u32::from(node.attempts.iter().any(|attempt| attempt.iteration > 1));
            target.test_attempts += node.attempts.len() as u32;
            match node.status {
                AggregateStatus::Passed => target.passed += 1,
                AggregateStatus::FlakyPass => target.flaky_passed += 1,
                AggregateStatus::Skipped => target.skipped += 1,
                AggregateStatus::BlockedSetup => target.blocked_setup += 1,
                AggregateStatus::BlockedSkip => target.blocked_skip += 1,
                AggregateStatus::FailedError => target.failed_error += 1,
                AggregateStatus::FailedPanic => target.failed_panic += 1,
                AggregateStatus::ResourceLimit => target.resource_limit += 1,
                AggregateStatus::Timeout => target.timeout += 1,
                AggregateStatus::Infrastructure => target.infrastructure += 1,
            }
        } else {
            target.suite_retried +=
                u32::from(node.attempts.iter().any(|attempt| attempt.round > 0));
            target.suite_repeated +=
                u32::from(node.attempts.iter().any(|attempt| attempt.iteration > 1));
            target.suite_attempts += node.attempts.len() as u32;
            match node.status {
                AggregateStatus::Passed => target.suite_passed += 1,
                AggregateStatus::FlakyPass => target.suite_flaky_passed += 1,
                AggregateStatus::Skipped => target.suite_skipped += 1,
                AggregateStatus::BlockedSetup => target.suite_blocked_setup += 1,
                AggregateStatus::BlockedSkip => target.suite_blocked_skip += 1,
                status if status.is_failure() => target.suite_failed += 1,
                _ => {}
            }
        }
        for attempt in &node.attempts {
            target.artifacts += attempt.artifacts.len() as u32;
            target.artifact_bytes += attempt
                .artifacts
                .iter()
                .map(|artifact| artifact.size)
                .sum::<u64>();
            target.snapshots += attempt.snapshots.len() as u32;
            for snapshot in &attempt.snapshots {
                match snapshot.status {
                    SnapshotStatus::Matched => target.snapshot_matched += 1,
                    SnapshotStatus::Missing => target.snapshot_missing += 1,
                    SnapshotStatus::Mismatched => target.snapshot_mismatched += 1,
                    SnapshotStatus::Created => target.snapshot_created += 1,
                    SnapshotStatus::Updated => target.snapshot_updated += 1,
                }
            }
        }
    }
    summary.failed = summary.failed_error
        + summary.failed_panic
        + summary.resource_limit
        + summary.timeout
        + summary.infrastructure
        + summary.suite_failed;
    if summary.selected != execution_plan.len() as u32 {
        return Err(ResultModelError::InvalidField {
            field: "execution_plan",
            message: "length must equal selected tests".into(),
        });
    }
    Ok(summary)
}

fn validate_unit(unit: &RetryUnit) -> Result<(), ProtocolError> {
    non_empty("unit.id", unit.id.clone())?;
    if unit.execution_plan.is_empty() {
        return Err(ProtocolError::InvalidField {
            field: "unit.execution_plan",
            message: "must not be empty".into(),
        });
    }
    let mut ids = BTreeSet::new();
    for id in &unit.execution_plan {
        let id = non_empty("unit.execution_plan", id.clone())?;
        if !ids.insert(id) {
            return Err(ProtocolError::InvalidField {
                field: "unit.execution_plan",
                message: "must not contain duplicates".into(),
            });
        }
    }
    Ok(())
}

fn non_empty(field: &'static str, value: String) -> Result<String, ProtocolError> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(ProtocolError::InvalidField {
            field,
            message: "must be non-empty and line-break free".into(),
        });
    }
    Ok(value)
}

fn validate_text(field: &'static str, value: &str) -> Result<(), ResultModelError> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(ResultModelError::InvalidField {
            field,
            message: "must be non-empty and line-break free".into(),
        });
    }
    Ok(())
}

fn validate_hex(field: &'static str, value: &str) -> Result<(), ResultModelError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ResultModelError::InvalidField {
            field,
            message: "must contain 64 lowercase hexadecimal digits".into(),
        });
    }
    Ok(())
}

fn serialize_u64_string<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn deserialize_u64_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value
        .parse::<u64>()
        .map_err(|_| serde::de::Error::custom("expected canonical unsigned decimal string"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn passing_test(id: &str) -> TestNode {
        TestNode::new(
            id,
            Some("suite::root".into()),
            "app",
            ResultNodeKind::Test,
            "main",
            id.rsplit("::").next().unwrap_or(id),
            vec![TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed)],
        )
    }

    fn passing_suite() -> TestNode {
        TestNode::new(
            "suite::root",
            None,
            "app",
            ResultNodeKind::Suite,
            "main",
            "root",
            vec![TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed)],
        )
    }

    #[test]
    fn assemble_derives_one_summary_and_flaky_status_for_reporters() {
        let mut flaky = passing_test("suite::root::flaky");
        let mut failed = TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedPanic);
        failed.failure = Some(FailureRecord {
            kind: "panic".into(),
            code: Some("P0007".into()),
            message: "boom".into(),
            source: None,
        });
        flaky.attempts = vec![
            failed,
            TestAttempt::new(2, 1, 1, Some(1), AttemptStatus::Passed),
        ];
        let report = TestResultTree::assemble(
            vec!["suite::root::flaky".into()],
            ResultPolicy {
                max_additional_rounds: 1,
                ..ResultPolicy::default()
            },
            vec![passing_suite()],
            vec![flaky],
        )
        .unwrap();
        assert_eq!(report.tests()[0].status, AggregateStatus::FlakyPass);
        assert_eq!(report.summary().flaky_passed, 1);
        assert_eq!(report.summary().failed, 0);
    }

    #[test]
    fn parse_rejects_summary_phase_and_unknown_schema_drift() {
        let report = TestResultTree::assemble(
            vec!["suite::root::ok".into()],
            ResultPolicy::default(),
            vec![passing_suite()],
            vec![passing_test("suite::root::ok")],
        )
        .unwrap();
        let mut value: Value = serde_json::from_slice(&report.canonical_bytes().unwrap()).unwrap();
        value["summary"]["passed"] = json!(0);
        assert!(matches!(
            TestResultTree::parse(&serde_json::to_vec(&value).unwrap()),
            Err(ResultModelError::SummaryMismatch { .. })
        ));
        let mut value: Value = serde_json::from_slice(&report.canonical_bytes().unwrap()).unwrap();
        value["unexpected"] = json!(true);
        assert!(matches!(
            TestResultTree::parse(&serde_json::to_vec(&value).unwrap()),
            Err(ResultModelError::InvalidJson(_))
        ));
    }

    #[test]
    fn blocked_attempt_requires_a_suite_cause_and_is_counted_once() {
        let mut blocked = passing_test("suite::root::blocked");
        blocked.attempts[0].status = AttemptStatus::BlockedSetup;
        blocked.attempts[0].blocked_by = Some(BlockedBy {
            id: "suite::root".into(),
            attempt: 1,
        });
        blocked.status = AggregateStatus::BlockedSetup;
        blocked.decisive_attempt = 1;
        let report = TestResultTree::assemble(
            vec!["suite::root::blocked".into()],
            ResultPolicy::default(),
            vec![passing_suite()],
            vec![blocked],
        )
        .unwrap();
        assert_eq!(report.summary().blocked_setup, 1);
        assert_eq!(report.summary().executed, 0);
    }

    #[test]
    fn canonical_bytes_sort_nodes_and_attempt_payloads_stably() {
        let mut suite = passing_suite();
        suite.attempts[0].artifacts.push(ArtifactRecord {
            name: "b".into(),
            media_type: "text/plain".into(),
            size: 1,
            sha256: "b".repeat(64),
            object: "objects/b".into(),
        });
        suite.attempts[0].artifacts.push(ArtifactRecord {
            name: "a".into(),
            media_type: "text/plain".into(),
            size: 1,
            sha256: "a".repeat(64),
            object: "objects/a".into(),
        });
        let report = TestResultTree::assemble(
            vec!["suite::root::z".into(), "suite::root::a".into()],
            ResultPolicy::default(),
            vec![suite],
            vec![
                passing_test("suite::root::z"),
                passing_test("suite::root::a"),
            ],
        )
        .unwrap();
        let first = report.canonical_bytes().unwrap();
        let second = TestResultTree::parse(&first)
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.find("\"name\":\"a\"").unwrap() < text.find("\"name\":\"b\"").unwrap());
    }

    fn limits() -> ProtocolLimits {
        ProtocolLimits {
            timeout_ms: 1,
            setup_timeout_ms: 1,
            teardown_timeout_ms: 1,
            output_bytes: 1,
            artifact_bytes: 1,
            snapshot_bytes: 1,
            memory_bytes: 1,
            instructions: 1,
            virtual_timers: 1,
        }
    }

    fn hello() -> CoordinatorFrame {
        CoordinatorFrame::new(
            "run-1",
            1,
            CoordinatorCommand::Hello {
                worker_id: "worker-1".into(),
                target: "tondo-vm-hosted".into(),
                plan_sha256: "sha256:".to_owned() + &"a".repeat(64),
                limits: limits(),
            },
        )
    }

    fn unit() -> RetryUnit {
        RetryUnit {
            kind: RetryUnitKind::Test,
            id: "suite::root::ok".into(),
            execution_plan: vec!["suite::root::ok".into()],
        }
    }

    #[test]
    fn protocol_requires_handshake_order_and_exact_directional_sequences() {
        let mut session = ProtocolSession::new("run-1", "worker-1").unwrap();
        session.accept_coordinator(&hello()).unwrap();
        session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                1,
                WorkerEvent::Ready {
                    worker_id: "worker-1".into(),
                },
            ))
            .unwrap();
        session
            .accept_coordinator(&CoordinatorFrame::new(
                "run-1",
                2,
                CoordinatorCommand::Run {
                    unit: unit(),
                    iteration: 1,
                    round: 0,
                },
            ))
            .unwrap();
        assert!(matches!(
            session.accept_worker(&WorkerFrame::new(
                "run-1",
                3,
                WorkerEvent::Started {
                    unit: unit(),
                    iteration: 1,
                    round: 0,
                },
            )),
            Err(ProtocolError::Sequence {
                expected: 2,
                actual: 3
            })
        ));
    }

    #[test]
    fn protocol_cancel_requires_ack_then_shutdown_closes_session() {
        let mut session = ProtocolSession::new("run-1", "worker-1").unwrap();
        session.accept_coordinator(&hello()).unwrap();
        session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                1,
                WorkerEvent::Ready {
                    worker_id: "worker-1".into(),
                },
            ))
            .unwrap();
        session
            .accept_coordinator(&CoordinatorFrame::new(
                "run-1",
                2,
                CoordinatorCommand::Cancel {
                    reason: "interrupt".into(),
                    grace_ms: 10,
                },
            ))
            .unwrap();
        session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                2,
                WorkerEvent::Cancelled { complete: true },
            ))
            .unwrap();
        session
            .accept_coordinator(&CoordinatorFrame::new(
                "run-1",
                3,
                CoordinatorCommand::Shutdown {
                    reason: "done".into(),
                },
            ))
            .unwrap();
        session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                3,
                WorkerEvent::Closed { clean: true },
            ))
            .unwrap();
        assert!(session.is_closed());
    }

    #[test]
    fn protocol_rejects_zero_limits_unknown_fields_and_invalid_run_unit() {
        let mut bad = limits();
        bad.instructions = 0;
        let frame = CoordinatorFrame::new(
            "run-1",
            1,
            CoordinatorCommand::Hello {
                worker_id: "worker-1".into(),
                target: "host".into(),
                plan_sha256: "sha256:".to_owned() + &"a".repeat(64),
                limits: bad,
            },
        );
        let mut session = ProtocolSession::new("run-1", "worker-1").unwrap();
        assert!(matches!(
            session.accept_coordinator(&frame),
            Err(ProtocolError::InvalidField {
                field: "instructions",
                ..
            })
        ));
        let mut value = serde_json::to_value(hello()).unwrap();
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CoordinatorFrame>(value).is_err());
        let mut session = ProtocolSession::new("run-1", "worker-1").unwrap();
        session.accept_coordinator(&hello()).unwrap();
        session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                1,
                WorkerEvent::Ready {
                    worker_id: "worker-1".into(),
                },
            ))
            .unwrap();
        let mut empty = unit();
        empty.execution_plan.clear();
        assert!(
            session
                .accept_coordinator(&CoordinatorFrame::new(
                    "run-1",
                    2,
                    CoordinatorCommand::Run {
                        unit: empty,
                        iteration: 1,
                        round: 0,
                    },
                ))
                .is_err()
        );
    }

    #[test]
    fn result_statuses_validation_and_summary_cover_all_closed_states() {
        for status in [
            AttemptStatus::Passed,
            AttemptStatus::Skipped,
            AttemptStatus::FailedError,
            AttemptStatus::FailedPanic,
            AttemptStatus::ResourceLimit,
            AttemptStatus::Timeout,
            AttemptStatus::Infrastructure,
            AttemptStatus::BlockedSetup,
            AttemptStatus::BlockedSkip,
        ] {
            let _ = status.aggregate();
        }
        assert!(AttemptStatus::FailedError.is_failure());
        assert!(AttemptStatus::BlockedSetup.is_blocked());
        assert!(!AggregateStatus::Passed.is_failure());
        assert!(AggregateStatus::Infrastructure.is_failure());
        assert!(aggregate_attempts(&[]).is_err());
        assert_eq!(
            aggregate_attempts(&[
                TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedError),
                TestAttempt::new(2, 1, 1, Some(1), AttemptStatus::Passed),
            ])
            .unwrap(),
            (AggregateStatus::FlakyPass, 2)
        );
        assert_eq!(
            aggregate_attempts(&[TestAttempt::new(1, 1, 0, None, AttemptStatus::BlockedSkip)])
                .unwrap(),
            (AggregateStatus::BlockedSkip, 1)
        );

        let mut suite = passing_suite();
        suite.status = AggregateStatus::FailedPanic;
        suite.attempts[0].status = AttemptStatus::FailedPanic;
        suite.attempts[0].failure = Some(FailureRecord {
            kind: "panic".into(),
            code: None,
            message: "boom".into(),
            source: Some(SourceSpan {
                file: "main.to".into(),
                start: 4,
                end: 2,
            }),
        });
        assert!(validate_node(&suite, &mut BTreeSet::new()).is_err());

        let mut valid = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        assert!(
            validate_attempt(
                &TestAttempt {
                    index: 0,
                    ..valid.clone()
                },
                ResultNodeKind::Test,
                Some("node"),
                0
            )
            .is_err()
        );
        assert!(
            validate_attempt(
                &TestAttempt {
                    iteration: 0,
                    ..valid.clone()
                },
                ResultNodeKind::Test,
                Some("node"),
                0
            )
            .is_err()
        );
        valid.unit = Some(1);
        assert!(validate_attempt(&valid, ResultNodeKind::Test, Some("node"), 0).is_err());
        valid = TestAttempt::new(1, 1, 1, None, AttemptStatus::Passed);
        assert!(validate_attempt(&valid, ResultNodeKind::Test, Some("node"), 0).is_err());
        valid = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        valid.phase = Some(AttemptPhase::Setup);
        assert!(validate_attempt(&valid, ResultNodeKind::Test, Some("node"), 0).is_err());

        let mut invalid = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        invalid.failure = Some(FailureRecord {
            kind: "x".into(),
            code: None,
            message: "x".into(),
            source: None,
        });
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid = TestAttempt::new(1, 1, 0, None, AttemptStatus::BlockedSetup);
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid.blocked_by = Some(BlockedBy {
            id: "suite".into(),
            attempt: 1,
        });
        invalid.status = AttemptStatus::Passed;
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid = TestAttempt::new(1, 1, 0, None, AttemptStatus::Skipped);
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid.skip = Some(SkipRecord {
            reason: "skip".into(),
            source: None,
        });
        invalid.failure = Some(FailureRecord {
            kind: "x".into(),
            code: None,
            message: "x".into(),
            source: None,
        });
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid.failure = None;
        invalid.phase = Some(AttemptPhase::Teardown);
        assert!(validate_attempt(&invalid, ResultNodeKind::Suite, Some("suite"), 0).is_err());
        invalid = TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedError);
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid.failure = Some(FailureRecord {
            kind: "x".into(),
            code: None,
            message: "x".into(),
            source: None,
        });
        invalid.skip = Some(SkipRecord {
            reason: "skip".into(),
            source: None,
        });
        assert!(validate_attempt(&invalid, ResultNodeKind::Test, Some("node"), 0).is_err());
        invalid.skip = None;
        invalid.phase = Some(AttemptPhase::Teardown);
        assert!(validate_attempt(&invalid, ResultNodeKind::Suite, Some("suite"), 0).is_err());
        invalid.status = AttemptStatus::FailedPanic;
        assert!(validate_attempt(&invalid, ResultNodeKind::Suite, Some("suite"), 0).is_ok());

        let mut payload = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        payload.tags.insert("".into(), "value".into());
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        payload.tags.clear();
        payload.artifacts.push(ArtifactRecord {
            name: "artifact".into(),
            media_type: "text/plain".into(),
            size: 1,
            sha256: "bad".into(),
            object: "object".into(),
        });
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        payload.artifacts[0].sha256 = "a".repeat(64);
        payload.artifacts.push(payload.artifacts[0].clone());
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        payload.artifacts.pop();
        payload.snapshots.push(SnapshotRecord {
            name: "snap".into(),
            status: SnapshotStatus::Matched,
            expected_sha256: Some("bad".into()),
            actual_sha256: "a".repeat(64),
        });
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        payload.snapshots[0].expected_sha256 = Some("b".repeat(64));
        payload.snapshots.push(payload.snapshots[0].clone());
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        payload.snapshots.pop();
        payload.virtual_time.push(VirtualTimeRecord {
            index: 2,
            elapsed_ns: "not-a-number".into(),
            automatic_advances: 0,
            explicit_advances: 0,
            settles: 0,
        });
        assert!(validate_attempt(&payload, ResultNodeKind::Test, Some("node"), 0).is_err());
        assert!(validate_text("text", "").is_err());
        assert!(validate_text("text", "bad\ntext").is_err());
        assert!(validate_hex("hex", "A".repeat(64).as_str()).is_err());
        assert!(validate_hex("hex", "bad").is_err());

        let mut test_nodes = Vec::new();
        for (index, status) in [
            AggregateStatus::Passed,
            AggregateStatus::FlakyPass,
            AggregateStatus::Skipped,
            AggregateStatus::BlockedSetup,
            AggregateStatus::BlockedSkip,
            AggregateStatus::FailedError,
            AggregateStatus::FailedPanic,
            AggregateStatus::ResourceLimit,
            AggregateStatus::Timeout,
            AggregateStatus::Infrastructure,
        ]
        .into_iter()
        .enumerate()
        {
            let mut node = passing_test(&format!("suite::root::status{index}"));
            node.status = status;
            node.attempts[0].status = if status == AggregateStatus::Passed {
                AttemptStatus::Passed
            } else {
                AttemptStatus::Infrastructure
            };
            test_nodes.push(node);
        }
        let mut suite_statuses = Vec::new();
        for (index, status) in [
            AggregateStatus::Passed,
            AggregateStatus::FlakyPass,
            AggregateStatus::Skipped,
            AggregateStatus::BlockedSetup,
            AggregateStatus::BlockedSkip,
            AggregateStatus::FailedError,
            AggregateStatus::Infrastructure,
        ]
        .into_iter()
        .enumerate()
        {
            let mut node = passing_suite();
            node.id = format!("suite::status{index}");
            node.status = status;
            suite_statuses.push(node);
        }
        test_nodes[0].attempts[0].artifacts.push(ArtifactRecord {
            name: "a".into(),
            media_type: "text/plain".into(),
            size: 2,
            sha256: "a".repeat(64),
            object: "a".into(),
        });
        for (index, status) in [
            SnapshotStatus::Matched,
            SnapshotStatus::Missing,
            SnapshotStatus::Mismatched,
            SnapshotStatus::Created,
            SnapshotStatus::Updated,
        ]
        .into_iter()
        .enumerate()
        {
            test_nodes[0].attempts[0].snapshots.push(SnapshotRecord {
                name: format!("s{index}"),
                status,
                expected_sha256: None,
                actual_sha256: "a".repeat(64),
            });
        }
        let execution = test_nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        let summary = summarize(&execution, &suite_statuses, &test_nodes).unwrap();
        assert_eq!(summary.selected, test_nodes.len() as u32);
        assert_eq!(summary.suite_selected, suite_statuses.len() as u32);
        assert_eq!(summary.snapshot_updated, 1);
        assert!(summarize(&[], &suite_statuses, &test_nodes).is_err());

        let mut duplicate = passing_test("suite::root::duplicate");
        duplicate.status = AggregateStatus::Passed;
        duplicate.decisive_attempt = 1;
        assert!(validate_node(&duplicate, &mut BTreeSet::new()).is_ok());
        assert!(validate_node(&duplicate, &mut BTreeSet::from([duplicate.id.clone()])).is_err());
        let mut bad_path = duplicate.clone();
        bad_path.path = vec!["".into()];
        assert!(validate_node(&bad_path, &mut BTreeSet::new()).is_err());
        let mut bad_source = duplicate.clone();
        bad_source.source = Some(SourceSpan {
            file: "x".into(),
            start: 2,
            end: 1,
        });
        assert!(validate_node(&bad_source, &mut BTreeSet::new()).is_err());
        let mut bad_index = duplicate.clone();
        bad_index.attempts[0].index = 2;
        assert!(validate_node(&bad_index, &mut BTreeSet::new()).is_err());
        let mut bad_aggregate = duplicate.clone();
        bad_aggregate.status = AggregateStatus::Infrastructure;
        assert!(validate_node(&bad_aggregate, &mut BTreeSet::new()).is_err());

        for error in [
            ResultModelError::InvalidJson("bad".into()),
            ResultModelError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            ResultModelError::InvalidReference {
                field: "x",
                value: "v".into(),
            },
            ResultModelError::InvalidAttempt {
                node: "n".into(),
                message: "bad".into(),
            },
            ResultModelError::SummaryMismatch {
                expected: ResultSummary::default(),
                actual: ResultSummary::default(),
            },
            ResultModelError::Serialization("bad".into()),
        ] {
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn protocol_validation_covers_directional_errors_and_terminal_events() {
        assert!(ProtocolSession::new("", "worker").is_err());
        assert!(ProtocolSession::new("run", "worker\n").is_err());
        assert!(
            validate_unit(&RetryUnit {
                kind: RetryUnitKind::Test,
                id: "".into(),
                execution_plan: vec!["x".into()]
            })
            .is_err()
        );
        assert!(
            validate_unit(&RetryUnit {
                kind: RetryUnitKind::Test,
                id: "x".into(),
                execution_plan: vec![]
            })
            .is_err()
        );
        assert!(
            validate_unit(&RetryUnit {
                kind: RetryUnitKind::Test,
                id: "x".into(),
                execution_plan: vec!["x".into(), "x".into()]
            })
            .is_err()
        );
        assert!(non_empty("field", "bad\nvalue".into()).is_err());
        assert!(
            ProtocolLimits {
                timeout_ms: 0,
                ..limits()
            }
            .validate()
            .is_err()
        );

        let mut session = ProtocolSession::new("run-1", "worker-1").unwrap();
        let mut wrong = hello();
        wrong.command = CoordinatorCommand::Hello {
            worker_id: "other".into(),
            target: "host".into(),
            plan_sha256: "sha256:".to_owned() + &"a".repeat(64),
            limits: limits(),
        };
        assert!(session.accept_coordinator(&wrong).is_err());
        let mut bad_format = hello();
        bad_format.format = "other".into();
        assert!(session.accept_coordinator(&bad_format).is_err());
        let mut bad_run = hello();
        bad_run.run_id = "other".into();
        assert!(session.accept_coordinator(&bad_run).is_err());
        session.accept_coordinator(&hello()).unwrap();
        let mut wrong_ready = WorkerFrame::new(
            "run-1",
            1,
            WorkerEvent::Ready {
                worker_id: "other".into(),
            },
        );
        assert!(session.accept_worker(&wrong_ready).is_err());
        wrong_ready.event = WorkerEvent::Ready {
            worker_id: "worker-1".into(),
        };
        session.accept_worker(&wrong_ready).unwrap();

        let mut run_session = ProtocolSession::new("run-1", "worker-1").unwrap();
        run_session.accept_coordinator(&hello()).unwrap();
        run_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                1,
                WorkerEvent::Ready {
                    worker_id: "worker-1".into(),
                },
            ))
            .unwrap();
        let mut zero_iteration = CoordinatorFrame::new(
            "run-1",
            2,
            CoordinatorCommand::Run {
                unit: unit(),
                iteration: 0,
                round: 0,
            },
        );
        assert!(run_session.accept_coordinator(&zero_iteration).is_err());
        zero_iteration.command = CoordinatorCommand::Run {
            unit: unit(),
            iteration: 1,
            round: 0,
        };
        run_session.accept_coordinator(&zero_iteration).unwrap();
        assert!(
            run_session
                .accept_worker(&WorkerFrame::new(
                    "run-1",
                    1,
                    WorkerEvent::Started {
                        unit: unit(),
                        iteration: 1,
                        round: 0
                    }
                ))
                .is_err()
        );
        run_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                2,
                WorkerEvent::Started {
                    unit: unit(),
                    iteration: 1,
                    round: 0,
                },
            ))
            .unwrap();
        let attempt = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        run_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                3,
                WorkerEvent::Attempt {
                    node_id: "node".into(),
                    attempt: attempt.clone(),
                },
            ))
            .unwrap();
        run_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                4,
                WorkerEvent::Finished { clean: true },
            ))
            .unwrap();
        assert!(
            run_session
                .accept_coordinator(&CoordinatorFrame::new(
                    "run-1",
                    3,
                    CoordinatorCommand::Cancel {
                        reason: "".into(),
                        grace_ms: 1
                    }
                ))
                .is_err()
        );

        let mut error_session = ProtocolSession::new("run-1", "worker-1").unwrap();
        error_session.accept_coordinator(&hello()).unwrap();
        error_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                1,
                WorkerEvent::Ready {
                    worker_id: "worker-1".into(),
                },
            ))
            .unwrap();
        error_session
            .accept_coordinator(&CoordinatorFrame::new(
                "run-1",
                2,
                CoordinatorCommand::Cancel {
                    reason: "stop".into(),
                    grace_ms: 1,
                },
            ))
            .unwrap();
        error_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                2,
                WorkerEvent::Cancelled { complete: false },
            ))
            .unwrap();
        error_session
            .accept_worker(&WorkerFrame::new(
                "run-1",
                3,
                WorkerEvent::Error {
                    kind: "x".into(),
                    message: "bad".into(),
                    fatal: true,
                },
            ))
            .unwrap();
        assert!(error_session.is_closed());

        for error in [
            ProtocolError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            ProtocolError::Sequence {
                expected: 1,
                actual: 2,
            },
            ProtocolError::Unexpected {
                state: "New".into(),
                message: "bad".into(),
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
