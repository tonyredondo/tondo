//! Sealed execution envelope for test-only operations.
//!
//! The envelope is a runtime-owned Rust object, never a Tondo value.  A
//! clone is the inheritance link used by helpers and structured tasks; no
//! node identity, sink or policy is exposed through that link.  All writes
//! are linearized under one mutex so limits, tag merges and evidence names
//! remain atomic even when tasks move between host threads.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::artifact::sha256;
use crate::test_limits::{BudgetKind, BudgetLedger, LimitError, LimitProfile};
use crate::test_output::CapturedOutput;
use crate::test_plan::TestSourceClass;
use crate::test_virtual_time::{AutoAdvance, VirtualDomain, VirtualTimeError, WaitKind};

pub const TEST_CONTROL_FORMAT: &str = "tondo-test-control-draft/1";

/// Per-attempt limits enforced by the envelope before publishing a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeLimits {
    profile: LimitProfile,
}

impl EnvelopeLimits {
    pub const fn new(output_bytes: u64, artifact_bytes: u64, snapshot_bytes: u64) -> Self {
        Self::from_profile(
            LimitProfile::DEFAULT
                .with_output(output_bytes)
                .with_artifact_bytes(artifact_bytes)
                .with_snapshot_bytes(snapshot_bytes),
        )
    }

    pub(crate) const fn from_profile(profile: LimitProfile) -> Self {
        Self { profile }
    }

    pub const fn profile(self) -> LimitProfile {
        self.profile
    }

    pub const fn with_execution_limits(mut self, work: u64, virtual_timers: u64) -> Self {
        self.profile = self
            .profile
            .with_work(work)
            .with_virtual_timers(virtual_timers);
        self
    }

    pub(crate) fn virtual_timer_limit(&self) -> u64 {
        self.profile.virtual_timers()
    }

    pub const fn output_bytes(self) -> u64 {
        self.profile.output()
    }

    pub const fn artifact_bytes(self) -> u64 {
        self.profile.artifact_bytes()
    }

    pub const fn snapshot_bytes(self) -> u64 {
        self.profile.snapshot_bytes()
    }
}

/// Lifecycle phase used for cleanup precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionPhase {
    Setup,
    Body,
    Cleanup,
    Closed,
}

impl ExecutionPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Setup => 0,
            Self::Body => 1,
            Self::Cleanup => 2,
            Self::Closed => 3,
        }
    }
}

/// Terminal state recorded by `failNow`, `skip`, or a cleanup failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    FailNow { code: &'static str, message: String },
    Skipped { reason: String },
    CleanupFailure { code: String, message: String },
    ResourceLimit { kind: &'static str },
}

/// Ordered log entry. The sequence is local to one envelope/attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    sequence: u64,
    message: String,
}

impl LogRecord {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Immutable artifact descriptor; bytes are kept in the envelope until the
/// coordinator copies them to its content-addressed store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEvidence {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
    sha256: String,
}

impl ArtifactEvidence {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Snapshot result recorded for one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotOutcome {
    Matched {
        expected_sha256: String,
        actual_sha256: String,
    },
    Missing {
        actual_sha256: String,
    },
    Mismatched {
        expected_sha256: String,
        actual_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEvidence {
    name: String,
    outcome: SnapshotOutcome,
}

impl SnapshotEvidence {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn outcome(&self) -> &SnapshotOutcome {
        &self.outcome
    }
}

/// Deterministic virtual-time observation for the current envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualTimeRecord {
    index: u32,
    elapsed_ns: i128,
    automatic_advances: u32,
    settles: u32,
    advances: u32,
}

impl VirtualTimeRecord {
    pub const fn index(&self) -> u32 {
        self.index
    }

    pub const fn elapsed_ns(&self) -> i128 {
        self.elapsed_ns
    }

    pub const fn automatic_advances(&self) -> u32 {
        self.automatic_advances
    }

    pub const fn settles(&self) -> u32 {
        self.settles
    }

    pub const fn advances(&self) -> u32 {
        self.advances
    }
}

/// A detached, report-ready copy of the envelope's private evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeReport {
    phase: ExecutionPhase,
    terminal: Option<Terminal>,
    logs: Vec<LogRecord>,
    tags: BTreeMap<String, String>,
    artifacts: Vec<ArtifactEvidence>,
    snapshots: Vec<SnapshotEvidence>,
    virtual_time: Vec<VirtualTimeRecord>,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

impl EnvelopeReport {
    pub const fn phase(&self) -> ExecutionPhase {
        self.phase
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }

    pub fn logs(&self) -> &[LogRecord] {
        &self.logs
    }

    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    pub fn artifacts(&self) -> &[ArtifactEvidence] {
        &self.artifacts
    }

    pub fn snapshots(&self) -> &[SnapshotEvidence] {
        &self.snapshots
    }

    pub fn virtual_time(&self) -> &[VirtualTimeRecord] {
        &self.virtual_time
    }

    pub fn stdout(&self) -> &CapturedOutput {
        &self.stdout
    }

    pub fn stderr(&self) -> &CapturedOutput {
        &self.stderr
    }

    /// Encode detached evidence for the CLI process-worker transport. This is
    /// deliberately a private wire format rather than the public test report
    /// schema: it carries artifact bytes needed by the parent coordinator.
    pub fn encode_process(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&ProcessReportWire::from_report(self)).map_err(|error| error.to_string())
    }

    /// Decode evidence emitted by a same-version hidden worker process.
    pub fn decode_process(bytes: &[u8]) -> Result<Self, String> {
        let wire: ProcessReportWire =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        wire.into_report()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessReportWire {
    phase: u8,
    terminal: Option<ProcessTerminalWire>,
    logs: Vec<ProcessLogWire>,
    tags: BTreeMap<String, String>,
    artifacts: Vec<ProcessArtifactWire>,
    snapshots: Vec<ProcessSnapshotWire>,
    virtual_time: Vec<ProcessVirtualTimeWire>,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum ProcessTerminalWire {
    FailNow { code: String, message: String },
    Skipped { reason: String },
    CleanupFailure { code: String, message: String },
    ResourceLimit { resource_kind: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessLogWire {
    sequence: u64,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessArtifactWire {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessSnapshotWire {
    name: String,
    outcome: SnapshotOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessVirtualTimeWire {
    index: u32,
    elapsed_ns: i128,
    automatic_advances: u32,
    settles: u32,
    advances: u32,
}

impl ProcessReportWire {
    fn from_report(report: &EnvelopeReport) -> Self {
        Self {
            phase: match report.phase {
                ExecutionPhase::Setup => 0,
                ExecutionPhase::Body => 1,
                ExecutionPhase::Cleanup => 2,
                ExecutionPhase::Closed => 3,
            },
            terminal: report
                .terminal
                .as_ref()
                .map(ProcessTerminalWire::from_terminal),
            logs: report
                .logs
                .iter()
                .map(|log| ProcessLogWire {
                    sequence: log.sequence,
                    message: log.message.clone(),
                })
                .collect(),
            tags: report.tags.clone(),
            artifacts: report
                .artifacts
                .iter()
                .map(|artifact| ProcessArtifactWire {
                    name: artifact.name.clone(),
                    media_type: artifact.media_type.clone(),
                    bytes: artifact.bytes.clone(),
                    sha256: artifact.sha256.clone(),
                })
                .collect(),
            snapshots: report
                .snapshots
                .iter()
                .map(|snapshot| ProcessSnapshotWire {
                    name: snapshot.name.clone(),
                    outcome: snapshot.outcome.clone(),
                })
                .collect(),
            virtual_time: report
                .virtual_time
                .iter()
                .map(|record| ProcessVirtualTimeWire {
                    index: record.index,
                    elapsed_ns: record.elapsed_ns,
                    automatic_advances: record.automatic_advances,
                    settles: record.settles,
                    advances: record.advances,
                })
                .collect(),
            stdout: report.stdout.clone(),
            stderr: report.stderr.clone(),
        }
    }

    fn into_report(self) -> Result<EnvelopeReport, String> {
        let phase = match self.phase {
            0 => ExecutionPhase::Setup,
            1 => ExecutionPhase::Body,
            2 => ExecutionPhase::Cleanup,
            3 => ExecutionPhase::Closed,
            other => return Err(format!("invalid process report phase {other}")),
        };
        Ok(EnvelopeReport {
            phase,
            terminal: self.terminal.map(ProcessTerminalWire::into_terminal),
            logs: self
                .logs
                .into_iter()
                .map(|log| LogRecord {
                    sequence: log.sequence,
                    message: log.message,
                })
                .collect(),
            tags: self.tags,
            artifacts: self
                .artifacts
                .into_iter()
                .map(|artifact| ArtifactEvidence {
                    name: artifact.name,
                    media_type: artifact.media_type,
                    bytes: artifact.bytes,
                    sha256: artifact.sha256,
                })
                .collect(),
            snapshots: self
                .snapshots
                .into_iter()
                .map(|snapshot| SnapshotEvidence {
                    name: snapshot.name,
                    outcome: snapshot.outcome,
                })
                .collect(),
            virtual_time: self
                .virtual_time
                .into_iter()
                .map(|record| VirtualTimeRecord {
                    index: record.index,
                    elapsed_ns: record.elapsed_ns,
                    automatic_advances: record.automatic_advances,
                    settles: record.settles,
                    advances: record.advances,
                })
                .collect(),
            stdout: self.stdout,
            stderr: self.stderr,
        })
    }
}

impl ProcessTerminalWire {
    fn from_terminal(terminal: &Terminal) -> Self {
        match terminal {
            Terminal::FailNow { code, message } => Self::FailNow {
                code: (*code).into(),
                message: message.clone(),
            },
            Terminal::Skipped { reason } => Self::Skipped {
                reason: reason.clone(),
            },
            Terminal::CleanupFailure { code, message } => Self::CleanupFailure {
                code: code.clone(),
                message: message.clone(),
            },
            Terminal::ResourceLimit { kind } => Self::ResourceLimit {
                resource_kind: (*kind).into(),
            },
        }
    }

    fn into_terminal(self) -> Terminal {
        match self {
            Self::FailNow { code, message } => Terminal::FailNow {
                code: process_code(&code),
                message,
            },
            Self::Skipped { reason } => Terminal::Skipped { reason },
            Self::CleanupFailure { code, message } => Terminal::CleanupFailure { code, message },
            Self::ResourceLimit { resource_kind } => Terminal::ResourceLimit {
                kind: process_kind(&resource_kind),
            },
        }
    }
}

fn process_code(code: &str) -> &'static str {
    match code {
        "E2003" => "E2003",
        "E2100" => "E2100",
        "E2200" => "E2200",
        "E2201" => "E2201",
        "E3000" => "E3000",
        "E3001" => "E3001",
        "E3002" => "E3002",
        "E3003" => "E3003",
        "P0007" => "P0007",
        "P0008" => "P0008",
        "P2001" => "P2001",
        "P2002" => "P2002",
        "P2003" => "P2003",
        "P2004" => "P2004",
        "P2005" => "P2005",
        "P2006" => "P2006",
        "P2007" => "P2007",
        "P2008" => "P2008",
        "R0001" => "R0001",
        "R0002" => "R0002",
        "R0003" => "R0003",
        _ => "E3003",
    }
}

fn process_kind(kind: &str) -> &'static str {
    match kind {
        "bytes" => "bytes",
        "artifacts" => "artifacts",
        "snapshots" => "snapshots",
        "output" => "output",
        "memory" => "memory",
        "instructions" => "instructions",
        "depth" => "depth",
        "work" => "work",
        "metadata" => "metadata",
        "artifact-count" => "artifact-count",
        "snapshot-count" => "snapshot-count",
        "virtual-timers" => "virtual-timers",
        "ready-queue" => "ready-queue",
        _ => "resource",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    Closed,
    PhaseRegression {
        current: ExecutionPhase,
        requested: ExecutionPhase,
    },
    VirtualTimeActive,
    VirtualTimeMissing,
    OutputLimit,
    ArtifactLimit,
    SnapshotLimit,
    ResourceLimit {
        kind: BudgetKind,
    },
    TagConflict {
        key: String,
    },
    FailNow {
        message: String,
    },
    Skip {
        reason: String,
    },
    SkipDuringCleanup,
    CleanupFailure {
        code: String,
        message: String,
    },
    ArtifactConflict {
        name: String,
    },
    SnapshotConflict {
        name: String,
    },
    SnapshotMismatch {
        name: String,
    },
    InvalidName {
        value: String,
    },
    InvalidMediaType {
        value: String,
    },
    InvalidDuration,
    VirtualDeadlock,
    VirtualExternalWait {
        task: String,
    },
    VirtualLivelock {
        limit: u32,
    },
    VirtualClockRegression,
    VirtualOverflow,
    VirtualTask {
        message: String,
    },
    ProductionOperation {
        operation: &'static str,
    },
    Poisoned,
}

impl ControlError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Closed => "E3000",
            Self::PhaseRegression { .. } => "E3001",
            Self::VirtualTimeActive => "P2004",
            Self::VirtualTimeMissing => "E3002",
            Self::OutputLimit => "R0001",
            Self::ArtifactLimit => "R0002",
            Self::SnapshotLimit => "R0003",
            Self::ResourceLimit { .. } => "R0001",
            Self::TagConflict { .. } => "P2002",
            Self::FailNow { .. } => "P0007",
            Self::Skip { .. } => "P0008",
            Self::SkipDuringCleanup => "P2001",
            Self::CleanupFailure { .. } => "P0007",
            Self::ArtifactConflict { .. } => "P2006",
            Self::SnapshotConflict { .. } => "P2008",
            Self::SnapshotMismatch { .. } => "P2007",
            Self::InvalidName { .. } | Self::InvalidMediaType { .. } => "P2006",
            Self::InvalidDuration => "P2005",
            Self::VirtualDeadlock
            | Self::VirtualExternalWait { .. }
            | Self::VirtualLivelock { .. } => "P2003",
            Self::VirtualClockRegression | Self::VirtualOverflow => "P2005",
            Self::VirtualTask { .. } => "E2100",
            Self::ProductionOperation { .. } => "E2003",
            Self::Poisoned => "E3003",
        }
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("test envelope is closed"),
            Self::PhaseRegression { current, requested } => {
                write!(
                    formatter,
                    "cannot move envelope from {current:?} to {requested:?}"
                )
            }
            Self::VirtualTimeActive => formatter.write_str("virtual time domain is already active"),
            Self::VirtualTimeMissing => formatter.write_str("virtual time domain is not active"),
            Self::OutputLimit => formatter.write_str("test output budget is exhausted"),
            Self::ArtifactLimit => formatter.write_str("test artifact budget is exhausted"),
            Self::SnapshotLimit => formatter.write_str("test snapshot budget is exhausted"),
            Self::ResourceLimit { kind } => {
                write!(formatter, "test {} budget is exhausted", kind.as_str())
            }
            Self::TagConflict { key } => {
                write!(formatter, "test tag `{key}` has conflicting values")
            }
            Self::FailNow { message } => write!(formatter, "test failed immediately: {message}"),
            Self::Skip { reason } => write!(formatter, "test skipped: {reason}"),
            Self::SkipDuringCleanup => formatter.write_str("skip is not allowed during cleanup"),
            Self::CleanupFailure { code, message } => {
                write!(formatter, "cleanup failed with {code}: {message}")
            }
            Self::ArtifactConflict { name } => write!(formatter, "artifact `{name}` is duplicated"),
            Self::SnapshotConflict { name } => {
                write!(formatter, "snapshot `{name}` is duplicated or invalid")
            }
            Self::SnapshotMismatch { name } => {
                write!(formatter, "snapshot `{name}` does not match")
            }
            Self::InvalidName { value } => write!(formatter, "evidence name `{value}` is invalid"),
            Self::InvalidMediaType { value } => {
                write!(formatter, "media type `{value}` is invalid")
            }
            Self::InvalidDuration => {
                formatter.write_str("virtual duration is negative or overflows")
            }
            Self::VirtualDeadlock => formatter.write_str("virtual domain is deadlocked"),
            Self::VirtualExternalWait { task } => {
                write!(formatter, "task `{task}` waits on an external operation")
            }
            Self::VirtualLivelock { limit } => {
                write!(
                    formatter,
                    "virtual domain exceeded auto-advance limit {limit}"
                )
            }
            Self::VirtualClockRegression => formatter.write_str("virtual clock regressed"),
            Self::VirtualOverflow => formatter.write_str("virtual clock overflowed"),
            Self::VirtualTask { message } => formatter.write_str(message),
            Self::ProductionOperation { operation } => {
                write!(
                    formatter,
                    "{operation} is test-only and cannot enter production"
                )
            }
            Self::Poisoned => formatter.write_str("test envelope lock is poisoned"),
        }
    }
}

impl Error for ControlError {}

#[derive(Debug, Clone)]
struct EnvelopeState {
    _node_id: String,
    limits: EnvelopeLimits,
    phase: ExecutionPhase,
    terminal: Option<Terminal>,
    sequence: u64,
    budget: BudgetLedger,
    logs: Vec<LogRecord>,
    tags: BTreeMap<String, String>,
    artifacts: BTreeMap<String, ArtifactEvidence>,
    snapshots: BTreeMap<String, SnapshotEvidence>,
    expected_snapshots: BTreeMap<String, String>,
    expected_installed: bool,
    snapshot_update: bool,
    snapshot_updates: BTreeMap<String, String>,
    virtual_time: Vec<VirtualTimeRecord>,
    virtual_time_active: bool,
    next_child: u64,
    child_skip: Option<(u64, String)>,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

/// Private runtime link inherited by helpers and structured tasks.
#[derive(Debug, Clone)]
pub struct EnvelopeHandle {
    state: Arc<Mutex<EnvelopeState>>,
}

impl EnvelopeHandle {
    pub fn new(node_id: impl Into<String>, limits: EnvelopeLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(EnvelopeState {
                _node_id: node_id.into(),
                limits,
                phase: ExecutionPhase::Setup,
                terminal: None,
                sequence: 0,
                budget: BudgetLedger::closed(limits.profile()),
                logs: Vec::new(),
                tags: BTreeMap::new(),
                artifacts: BTreeMap::new(),
                snapshots: BTreeMap::new(),
                expected_snapshots: BTreeMap::new(),
                expected_installed: false,
                snapshot_update: false,
                snapshot_updates: BTreeMap::new(),
                virtual_time: Vec::new(),
                virtual_time_active: false,
                next_child: 0,
                child_skip: None,
                stdout: CapturedOutput::default(),
                stderr: CapturedOutput::default(),
            })),
        }
    }

    /// Expected values are test-runner input and can only be installed before
    /// any snapshot check in this attempt.
    pub fn with_expected_snapshots(
        &self,
        expected: BTreeMap<String, String>,
    ) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.expected_installed || !state.snapshots.is_empty() {
            return Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into(),
            });
        }
        for name in expected.keys() {
            validate_evidence_name(name)
                .map_err(|_| ControlError::SnapshotConflict { name: name.clone() })?;
        }
        state.expected_snapshots = expected;
        state.expected_installed = true;
        Ok(())
    }

    /// Enable update mode for this attempt. Actual values are retained only
    /// as coordinator-owned update candidates and never enter the public
    /// report wire format.
    pub fn with_snapshot_update(&self, enabled: bool) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if !state.snapshots.is_empty() {
            return Err(ControlError::SnapshotConflict {
                name: "snapshot-update-already-configured".into(),
            });
        }
        state.snapshot_update = enabled;
        Ok(())
    }

    pub fn set_phase(&self, phase: ExecutionPhase) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if phase.rank() < state.phase.rank() {
            return Err(ControlError::PhaseRegression {
                current: state.phase,
                requested: phase,
            });
        }
        if state.phase != phase && phase != ExecutionPhase::Closed {
            state.budget = BudgetLedger::closed(state.limits.profile());
        }
        state.phase = phase;
        Ok(())
    }

    pub fn phase(&self) -> Result<ExecutionPhase, ControlError> {
        Ok(self.lock()?.phase)
    }

    pub(crate) fn limits(&self) -> Result<EnvelopeLimits, ControlError> {
        Ok(self.lock()?.limits)
    }

    pub(crate) fn reserve_runtime_timer(&self) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        reserve_evidence(
            &mut state.budget,
            [(BudgetKind::Work, 1), (BudgetKind::Metadata, 64)],
        )
    }

    pub(crate) fn record_runtime_limit(
        &self,
        id: &str,
        kind: &'static str,
    ) -> Result<bool, ControlError> {
        let mut state = self.lock()?;
        if state._node_id != id {
            return Ok(false);
        }
        ensure_open(&state)?;
        set_failure_terminal(&mut state, Terminal::ResourceLimit { kind });
        Ok(true)
    }

    /// Preserve a hosted operation's terminal before the VM unwinds its node.
    /// Failed preflights have already left the evidence buffers unchanged.
    pub(crate) fn record_host_error(&self, error: &ControlError) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        match error {
            ControlError::Closed
            | ControlError::PhaseRegression { .. }
            | ControlError::VirtualTimeMissing
            | ControlError::ProductionOperation { .. }
            | ControlError::Poisoned => return Err(error.clone()),
            ControlError::Skip { reason } if state.terminal.is_none() => {
                let terminal = Terminal::Skipped {
                    reason: reason.clone(),
                };
                reserve_terminal(&mut state, &terminal)?;
                state.terminal = Some(terminal);
            }
            ControlError::Skip { .. } => {}
            ControlError::OutputLimit
            | ControlError::ArtifactLimit
            | ControlError::SnapshotLimit
            | ControlError::ResourceLimit { .. } => {
                let kind = match error {
                    ControlError::OutputLimit => "output",
                    ControlError::ArtifactLimit => "artifacts",
                    ControlError::ResourceLimit { kind } => kind.as_str(),
                    _ => "snapshots",
                };
                set_failure_terminal(&mut state, Terminal::ResourceLimit { kind });
            }
            _ => {
                let message = match error {
                    ControlError::FailNow { message }
                    | ControlError::CleanupFailure { message, .. } => message.clone(),
                    _ => error.to_string(),
                };
                let terminal = if state.phase == ExecutionPhase::Cleanup
                    || matches!(error, ControlError::CleanupFailure { .. })
                {
                    Terminal::CleanupFailure {
                        code: match error {
                            ControlError::CleanupFailure { code, .. } => code.clone(),
                            _ => error.code().into(),
                        },
                        message,
                    }
                } else {
                    Terminal::FailNow {
                        code: error.code(),
                        message,
                    }
                };
                reserve_terminal(&mut state, &terminal)?;
                set_failure_terminal(&mut state, terminal);
            }
        }
        Ok(())
    }

    pub fn log(&self, message: impl Into<String>) -> Result<(), ControlError> {
        let message = message.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let delta = message.len() as u64;
        reserve_evidence(
            &mut state.budget,
            [
                (BudgetKind::Work, 1),
                (BudgetKind::Output, delta),
                (BudgetKind::Metadata, 32),
            ],
        )?;
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        state.logs.push(LogRecord { sequence, message });
        Ok(())
    }

    pub fn tags(&self, values: BTreeMap<String, String>) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let mut delta = 0_u64;
        for (key, value) in &values {
            if let Some(previous) = state.tags.get(key) {
                if previous != value {
                    return Err(ControlError::TagConflict { key: key.clone() });
                }
            } else {
                delta = delta.saturating_add((key.len() + value.len()) as u64);
            }
        }
        let entries = values
            .keys()
            .filter(|key| !state.tags.contains_key(*key))
            .count() as u64;
        reserve_evidence(
            &mut state.budget,
            [
                (BudgetKind::Work, 1),
                (BudgetKind::Output, delta),
                (BudgetKind::Metadata, entries.saturating_mul(48)),
            ],
        )?;
        state.tags.extend(values);
        Ok(())
    }

    pub fn stdout(&self, text: impl AsRef<[u8]>) -> Result<(), ControlError> {
        self.append_stream(text.as_ref(), true, false)
    }

    pub fn stderr(&self, text: impl AsRef<[u8]>) -> Result<(), ControlError> {
        self.append_stream(text.as_ref(), false, false)
    }

    /// Includes the optional line ending in the same atomic output preflight.
    pub(crate) fn print_stdout(&self, text: &str, newline: bool) -> Result<(), ControlError> {
        self.append_stream(text.as_bytes(), true, newline)
    }

    pub(crate) fn output_len(&self, stdout: bool) -> Result<usize, ControlError> {
        let state = self.lock()?;
        ensure_open(&state)?;
        Ok(if stdout {
            state.stdout.len()
        } else {
            state.stderr.len()
        })
    }

    fn append_stream(&self, text: &[u8], stdout: bool, newline: bool) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let delta = (text.len() as u64)
            .checked_add(u64::from(newline))
            .ok_or(ControlError::OutputLimit)?;
        reserve_evidence(
            &mut state.budget,
            [(BudgetKind::Work, 1), (BudgetKind::Output, delta)],
        )?;
        let stream = if stdout {
            &mut state.stdout
        } else {
            &mut state.stderr
        };
        stream.append(text);
        if newline {
            stream.append(b"\n");
        }
        Ok(())
    }

    pub fn fail_now(&self, message: impl Into<String>) -> Result<(), ControlError> {
        let error = ControlError::FailNow {
            message: message.into(),
        };
        self.record_host_error(&error)?;
        Err(error)
    }

    pub fn skip(&self, reason: impl Into<String>) -> Result<(), ControlError> {
        let reason = reason.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.phase == ExecutionPhase::Cleanup {
            return Err(ControlError::SkipDuringCleanup);
        }
        if state.terminal.is_none() {
            let terminal = Terminal::Skipped {
                reason: reason.clone(),
            };
            reserve_terminal(&mut state, &terminal)?;
            state.terminal = Some(terminal);
        }
        Err(ControlError::Skip { reason })
    }

    /// Record a failure discovered while cleanup is unwinding. Cleanup
    /// failures always supersede a previously recorded skip.
    pub fn cleanup_failure(
        &self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), ControlError> {
        let error = ControlError::CleanupFailure {
            code: code.into(),
            message: message.into(),
        };
        self.record_host_error(&error)?;
        Err(error)
    }

    pub fn attach(
        &self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), ControlError> {
        let name = name.into();
        let media_type = media_type.into();
        let bytes = bytes.into();
        validate_evidence_name(&name)?;
        validate_media_type(&media_type)?;
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.artifacts.contains_key(&name) {
            return Err(ControlError::ArtifactConflict { name });
        }
        reserve_artifact(&mut state.budget, &name, &media_type, bytes.len() as u64)?;
        state.artifacts.insert(
            name.clone(),
            ArtifactEvidence {
                name,
                media_type,
                sha256: sha256(&bytes),
                bytes,
            },
        );
        Ok(())
    }

    pub fn snapshot(
        &self,
        name: impl Into<String>,
        actual: impl AsRef<str>,
    ) -> Result<SnapshotOutcome, ControlError> {
        let name = name.into();
        validate_evidence_name(&name)
            .map_err(|_| ControlError::SnapshotConflict { name: name.clone() })?;
        let actual = actual.as_ref();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.snapshots.contains_key(&name) {
            return Err(ControlError::SnapshotConflict { name });
        }
        let has_expected = state.expected_snapshots.contains_key(&name);
        reserve_snapshot(&mut state.budget, &name, actual.len() as u64, has_expected)?;
        let actual_sha256 = sha256(actual.as_bytes());
        let outcome = match state.expected_snapshots.get(&name) {
            Some(expected) => {
                let expected_sha256 = sha256(expected.as_bytes());
                if expected == actual {
                    SnapshotOutcome::Matched {
                        expected_sha256,
                        actual_sha256,
                    }
                } else {
                    SnapshotOutcome::Mismatched {
                        expected_sha256,
                        actual_sha256,
                    }
                }
            }
            None => SnapshotOutcome::Missing { actual_sha256 },
        };
        state.snapshots.insert(
            name.clone(),
            SnapshotEvidence {
                name: name.clone(),
                outcome: outcome.clone(),
            },
        );
        match outcome {
            SnapshotOutcome::Matched { .. } => Ok(outcome),
            SnapshotOutcome::Missing { .. } | SnapshotOutcome::Mismatched { .. } => {
                if state.snapshot_update {
                    state.snapshot_updates.insert(name, actual.into());
                    Ok(outcome)
                } else {
                    Err(ControlError::SnapshotMismatch { name })
                }
            }
        }
    }

    /// Return update candidates accumulated by this attempt.
    pub fn snapshot_updates(&self) -> Result<Vec<(String, String)>, ControlError> {
        Ok(self
            .lock()?
            .snapshot_updates
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect())
    }

    /// Import evidence from a process-isolated child into the coordinator's
    /// envelope while preserving the coordinator's own limits and lifecycle.
    pub fn merge_worker_report(
        &self,
        report: &EnvelopeReport,
        updates: &[(String, String)],
    ) -> Result<(), ControlError> {
        let mut committed = self.lock()?;
        ensure_open(&committed)?;
        // Report import is one atomic operation. Its bounded transaction also
        // protects earlier evidence from a late duplicate or exhausted delta.
        let mut state = committed.clone();
        for log in &report.logs {
            reserve_evidence(
                &mut state.budget,
                [
                    (BudgetKind::Work, 1),
                    (BudgetKind::Output, log.message.len() as u64),
                    (BudgetKind::Metadata, 32),
                ],
            )?;
            state.sequence = state.sequence.saturating_add(1);
            let sequence = state.sequence;
            state.logs.push(LogRecord {
                sequence,
                message: log.message.clone(),
            });
        }
        for (key, value) in &report.tags {
            if let Some(previous) = state.tags.get(key) {
                if previous != value {
                    return Err(ControlError::TagConflict { key: key.clone() });
                }
            } else {
                reserve_evidence(
                    &mut state.budget,
                    [
                        (BudgetKind::Work, 1),
                        (BudgetKind::Output, (key.len() + value.len()) as u64),
                        (BudgetKind::Metadata, 48),
                    ],
                )?;
                state.tags.insert(key.clone(), value.clone());
            }
        }
        reserve_evidence(
            &mut state.budget,
            [
                (BudgetKind::Work, 1),
                (
                    BudgetKind::Output,
                    (report.stdout.len() + report.stderr.len()) as u64,
                ),
            ],
        )?;
        state.stdout.append(report.stdout.as_bytes());
        state.stderr.append(report.stderr.as_bytes());
        for artifact in &report.artifacts {
            if state.artifacts.contains_key(&artifact.name) {
                return Err(ControlError::ArtifactConflict {
                    name: artifact.name.clone(),
                });
            }
            reserve_artifact(
                &mut state.budget,
                &artifact.name,
                &artifact.media_type,
                artifact.bytes.len() as u64,
            )?;
            state
                .artifacts
                .insert(artifact.name.clone(), artifact.clone());
        }
        for snapshot in &report.snapshots {
            if state.snapshots.contains_key(&snapshot.name) {
                return Err(ControlError::SnapshotConflict {
                    name: snapshot.name.clone(),
                });
            }
            reserve_snapshot(
                &mut state.budget,
                &snapshot.name,
                0,
                matches!(
                    snapshot.outcome,
                    SnapshotOutcome::Matched { .. } | SnapshotOutcome::Mismatched { .. }
                ),
            )?;
            state
                .snapshots
                .insert(snapshot.name.clone(), snapshot.clone());
        }
        reserve_evidence(
            &mut state.budget,
            [
                (BudgetKind::Work, report.virtual_time.len() as u64),
                (
                    BudgetKind::Metadata,
                    (report.virtual_time.len() as u64).saturating_mul(40),
                ),
            ],
        )?;
        state
            .virtual_time
            .extend(report.virtual_time.iter().cloned());
        for (name, value) in updates {
            reserve_evidence(
                &mut state.budget,
                [(BudgetKind::SnapshotBytes, value.len() as u64)],
            )?;
            if state
                .snapshot_updates
                .insert(name.clone(), value.clone())
                .is_some()
            {
                return Err(ControlError::SnapshotConflict { name: name.clone() });
            }
        }
        if state.terminal.is_none() {
            if let Some(terminal) = &report.terminal {
                reserve_terminal(&mut state, terminal)?;
            }
            state.terminal = report.terminal.clone();
        }
        *committed = state;
        Ok(())
    }

    pub fn with_virtual_time<T>(
        &self,
        operation: impl FnOnce(&VirtualTime<'_>) -> Result<T, ControlError>,
    ) -> Result<T, ControlError> {
        let controller = self.enter_virtual_time()?;
        let result = operation(&controller);
        let close_result = controller.close();
        match (result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn enter_virtual_time(&self) -> Result<VirtualTime<'_>, ControlError> {
        let mut state = self.lock()?;
        begin_virtual_domain(&mut state)?;
        Ok(VirtualTime {
            envelope: self,
            domain: RefCell::new(VirtualDomain::new()),
            closed: false,
        })
    }

    pub(crate) fn begin_runtime_virtual_time(&self) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        begin_virtual_domain(&mut state)
    }

    pub(crate) fn record_runtime_virtual_settle(&self) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if !state.virtual_time_active {
            return Err(ControlError::VirtualTimeMissing);
        }
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.settles = record.settles.saturating_add(1);
        Ok(())
    }

    pub(crate) fn record_runtime_virtual_advance(
        &self,
        elapsed_ns: i128,
    ) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if !state.virtual_time_active {
            return Err(ControlError::VirtualTimeMissing);
        }
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.elapsed_ns = elapsed_ns;
        record.advances = record.advances.saturating_add(1);
        Ok(())
    }

    pub(crate) fn record_runtime_virtual_auto_advance(
        &self,
        elapsed_ns: i128,
    ) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if !state.virtual_time_active {
            return Err(ControlError::VirtualTimeMissing);
        }
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.elapsed_ns = elapsed_ns;
        record.automatic_advances = record.automatic_advances.saturating_add(1);
        Ok(())
    }

    pub(crate) fn finish_runtime_virtual_time(&self, elapsed_ns: i128) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if !state.virtual_time_active {
            return Err(ControlError::VirtualTimeMissing);
        }
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.elapsed_ns = elapsed_ns;
        state.virtual_time_active = false;
        Ok(())
    }

    pub fn child(&self) -> Result<StructuredTask, ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let ordinal = state.next_child;
        state.next_child = state.next_child.saturating_add(1);
        Ok(StructuredTask {
            envelope: self.clone(),
            ordinal,
        })
    }

    pub fn close(&self) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.virtual_time_active {
            return Err(ControlError::VirtualTimeActive);
        }
        state.phase = ExecutionPhase::Closed;
        Ok(())
    }

    pub fn report(&self) -> Result<EnvelopeReport, ControlError> {
        let state = self.lock()?;
        Ok(EnvelopeReport {
            phase: state.phase,
            terminal: state.terminal.clone(),
            logs: state.logs.clone(),
            tags: state.tags.clone(),
            artifacts: state.artifacts.values().cloned().collect(),
            snapshots: state.snapshots.values().cloned().collect(),
            virtual_time: state.virtual_time.clone(),
            stdout: state.stdout.clone(),
            stderr: state.stderr.clone(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, EnvelopeState>, ControlError> {
        self.state.lock().map_err(|_| ControlError::Poisoned)
    }

    fn record_child_skip(&self, ordinal: u64, reason: String) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let replaces_skip = matches!(state.terminal, Some(Terminal::Skipped { .. }))
            && state
                .child_skip
                .as_ref()
                .is_some_and(|(first, _)| ordinal < *first);
        if state.terminal.is_none() || replaces_skip {
            reserve_terminal(
                &mut state,
                &Terminal::Skipped {
                    reason: reason.clone(),
                },
            )?;
            state.child_skip = Some((ordinal, reason.clone()));
            state.terminal = Some(Terminal::Skipped { reason });
        }
        Err(ControlError::Skip {
            reason: "structured child requested skip".into(),
        })
    }
}

/// A virtual-time controller borrowed from one envelope. It exposes no node
/// identity or sink and is invalid after the closure returns.
pub struct VirtualTime<'a> {
    envelope: &'a EnvelopeHandle,
    domain: RefCell<VirtualDomain>,
    closed: bool,
}

impl<'a> VirtualTime<'a> {
    pub fn settle(&self) -> Result<(), ControlError> {
        let _ = self
            .domain
            .borrow_mut()
            .settle()
            .map_err(control_virtual_error)?;
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.settles = record.settles.saturating_add(1);
        Ok(())
    }

    pub fn advance(&self, duration_ns: i128) -> Result<(), ControlError> {
        let report = self
            .domain
            .borrow_mut()
            .advance(duration_ns)
            .map_err(control_virtual_error)?;
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.elapsed_ns = i128::from(report.now());
        record.advances = record.advances.saturating_add(1);
        Ok(())
    }

    pub fn now(&self) -> Result<u64, ControlError> {
        Ok(self.domain.borrow().now())
    }

    pub fn register_task(&self, id: impl Into<String>) -> Result<(), ControlError> {
        self.domain
            .borrow_mut()
            .register_task(id)
            .map_err(control_virtual_error)
    }

    pub fn block(&self, id: &str, wait: WaitKind) -> Result<(), ControlError> {
        self.domain
            .borrow_mut()
            .block(id, wait)
            .map_err(control_virtual_error)
    }

    pub fn complete(&self, id: &str) -> Result<(), ControlError> {
        self.domain
            .borrow_mut()
            .complete(id)
            .map_err(control_virtual_error)
    }

    pub fn schedule_timer(
        &self,
        id: impl Into<String>,
        task: &str,
        delay_ns: i128,
    ) -> Result<u64, ControlError> {
        Ok(self
            .domain
            .borrow_mut()
            .schedule_timer(id, task, delay_ns)
            .map_err(control_virtual_error)?
            .deadline())
    }

    pub fn cancel_timer(&self, id: &str) -> Result<(), ControlError> {
        self.domain
            .borrow_mut()
            .cancel_timer(id)
            .map_err(control_virtual_error)
    }

    pub fn reschedule_timer(&self, id: &str, delay_ns: i128) -> Result<u64, ControlError> {
        Ok(self
            .domain
            .borrow_mut()
            .reschedule_timer(id, delay_ns)
            .map_err(control_virtual_error)?
            .deadline())
    }

    pub fn auto_advance(&self) -> Result<AutoAdvance, ControlError> {
        let outcome = self
            .domain
            .borrow_mut()
            .auto_advance_once()
            .map_err(control_virtual_error)?;
        if let AutoAdvance::Advanced { to, .. } = &outcome {
            let mut state = self.envelope.lock()?;
            ensure_open(&state)?;
            let Some(record) = state.virtual_time.last_mut() else {
                return Err(ControlError::VirtualTimeMissing);
            };
            record.elapsed_ns = i128::from(*to);
            record.automatic_advances = record.automatic_advances.saturating_add(1);
        }
        Ok(outcome)
    }

    fn close(mut self) -> Result<(), ControlError> {
        if self.closed {
            return Ok(());
        }
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        state.virtual_time_active = false;
        self.closed = true;
        Ok(())
    }
}

fn control_virtual_error(error: VirtualTimeError) -> ControlError {
    match error {
        VirtualTimeError::InvalidDuration => ControlError::InvalidDuration,
        VirtualTimeError::Overflow | VirtualTimeError::SequenceOverflow => {
            ControlError::VirtualOverflow
        }
        VirtualTimeError::ClockRegression { .. } => ControlError::VirtualClockRegression,
        VirtualTimeError::Deadlock => ControlError::VirtualDeadlock,
        VirtualTimeError::ExternalWait(task) => ControlError::VirtualExternalWait { task },
        VirtualTimeError::Livelock { limit } => ControlError::VirtualLivelock { limit },
        other => ControlError::VirtualTask {
            message: other.to_string(),
        },
    }
}

impl Drop for VirtualTime<'_> {
    fn drop(&mut self) {
        if !self.closed {
            if let Ok(mut state) = self.envelope.state.lock() {
                state.virtual_time_active = false;
            }
            self.closed = true;
        }
    }
}

/// Inherited handle for one structured task. Its ordinal only chooses the
/// deterministic first child skip; it is never part of user-visible data.
#[derive(Debug, Clone)]
pub struct StructuredTask {
    envelope: EnvelopeHandle,
    ordinal: u64,
}

impl StructuredTask {
    pub fn log(&self, message: impl Into<String>) -> Result<(), ControlError> {
        self.envelope.log(message)
    }

    pub fn tags(&self, values: BTreeMap<String, String>) -> Result<(), ControlError> {
        self.envelope.tags(values)
    }

    pub fn attach(
        &self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), ControlError> {
        self.envelope.attach(name, media_type, bytes)
    }

    pub fn skip(&self, reason: impl Into<String>) -> Result<(), ControlError> {
        self.envelope.record_child_skip(self.ordinal, reason.into())
    }
}

fn ensure_open(state: &EnvelopeState) -> Result<(), ControlError> {
    if state.phase == ExecutionPhase::Closed {
        Err(ControlError::Closed)
    } else {
        Ok(())
    }
}

fn reserve_artifact(
    budget: &mut BudgetLedger,
    name: &str,
    media_type: &str,
    bytes: u64,
) -> Result<(), ControlError> {
    reserve_evidence(
        budget,
        [
            (BudgetKind::Work, 1),
            (
                BudgetKind::Output,
                (name.len() as u64)
                    .saturating_add(media_type.len() as u64)
                    .saturating_add(71),
            ),
            (BudgetKind::Metadata, 96),
            (BudgetKind::ArtifactBytes, bytes),
            (BudgetKind::ArtifactCount, 1),
        ],
    )
}

fn reserve_snapshot(
    budget: &mut BudgetLedger,
    name: &str,
    bytes: u64,
    has_expected: bool,
) -> Result<(), ControlError> {
    reserve_evidence(
        budget,
        [
            (BudgetKind::Work, 1),
            (
                BudgetKind::Output,
                (name.len() as u64).saturating_add(if has_expected { 142 } else { 71 }),
            ),
            (BudgetKind::Metadata, 80),
            (BudgetKind::SnapshotBytes, bytes),
            (BudgetKind::SnapshotCount, 1),
        ],
    )
}

fn begin_virtual_domain(state: &mut EnvelopeState) -> Result<(), ControlError> {
    ensure_open(state)?;
    if state.virtual_time_active {
        return Err(ControlError::VirtualTimeActive);
    }
    let index = u32::try_from(state.virtual_time.len())
        .ok()
        .and_then(|index| index.checked_add(1))
        .ok_or(ControlError::VirtualOverflow)?;
    reserve_evidence(
        &mut state.budget,
        [(BudgetKind::Work, 1), (BudgetKind::Metadata, 40)],
    )?;
    state.virtual_time_active = true;
    state.virtual_time.push(VirtualTimeRecord {
        index,
        elapsed_ns: 0,
        automatic_advances: 0,
        settles: 0,
        advances: 0,
    });
    Ok(())
}

fn reserve_evidence(
    budget: &mut BudgetLedger,
    deltas: impl IntoIterator<Item = (BudgetKind, u64)>,
) -> Result<(), ControlError> {
    budget
        .reserve(deltas.into_iter().filter(|(_, amount)| *amount != 0))
        .map(|_| ())
        .map_err(|error| match error {
            LimitError::Exhausted { kind, .. } | LimitError::Overflow(kind) => match kind {
                BudgetKind::Output => ControlError::OutputLimit,
                BudgetKind::ArtifactBytes => ControlError::ArtifactLimit,
                BudgetKind::SnapshotBytes => ControlError::SnapshotLimit,
                kind => ControlError::ResourceLimit { kind },
            },
            _ => ControlError::Poisoned,
        })
}

fn set_failure_terminal(state: &mut EnvelopeState, terminal: Terminal) {
    if matches!(terminal, Terminal::ResourceLimit { .. })
        || (!matches!(state.terminal, Some(Terminal::ResourceLimit { .. }))
            && (matches!(terminal, Terminal::CleanupFailure { .. })
                || !matches!(state.terminal, Some(Terminal::CleanupFailure { .. }))))
    {
        state.terminal = Some(terminal);
    }
}

fn reserve_terminal(state: &mut EnvelopeState, terminal: &Terminal) -> Result<(), ControlError> {
    if state.terminal.as_ref() == Some(terminal)
        || matches!(state.terminal, Some(Terminal::ResourceLimit { .. }))
    {
        return Ok(());
    }
    let bytes = match terminal {
        Terminal::FailNow { message, .. } | Terminal::CleanupFailure { message, .. } => {
            message.len()
        }
        Terminal::Skipped { reason } => reason.len(),
        // The one fixed runner terminal slot must remain usable after exhaustion.
        Terminal::ResourceLimit { .. } => return Ok(()),
    };
    reserve_evidence(
        &mut state.budget,
        [
            (BudgetKind::Work, 1),
            (BudgetKind::Output, bytes as u64),
            (BudgetKind::Metadata, 64),
        ],
    )
}

fn validate_evidence_name(value: &str) -> Result<(), ControlError> {
    if value.is_empty()
        || value.contains(['/', '\\', '\n', '\r'])
        || value.chars().any(|character| character.is_control())
    {
        return Err(ControlError::InvalidName {
            value: value.into(),
        });
    }
    Ok(())
}

fn validate_media_type(value: &str) -> Result<(), ControlError> {
    let mut parts = value.split('/');
    let Some(major) = parts.next() else {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    };
    let Some(minor) = parts.next() else {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    };
    if parts.next().is_some()
        || major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(is_media_token)
        || !minor.bytes().all(is_media_token)
    {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    }
    Ok(())
}

fn is_media_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Production code cannot admit an intrinsic test operation.
pub fn admit_operation(
    source_class: TestSourceClass,
    operation: &'static str,
) -> Result<(), ControlError> {
    if source_class == TestSourceClass::Production {
        Err(ControlError::ProductionOperation { operation })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> EnvelopeLimits {
        EnvelopeLimits::new(1_000, 1_000, 1_000)
    }

    #[test]
    fn logs_tags_streams_and_report_are_isolated_and_ordered() {
        let envelope = EnvelopeHandle::new("hidden-node", limits());
        envelope.log("one").unwrap();
        envelope.log("two").unwrap();
        envelope
            .tags(BTreeMap::from([("suite".into(), "unit".into())]))
            .unwrap();
        envelope.stdout("out").unwrap();
        envelope.stderr("err").unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(
            report
                .logs()
                .iter()
                .map(LogRecord::sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(report.tags().get("suite"), Some(&"unit".into()));
        assert_eq!(report.stdout().as_text(), Some("out"));
        assert_eq!(report.stderr().as_text(), Some("err"));
    }

    #[test]
    fn process_report_round_trip_preserves_evidence() {
        let envelope = EnvelopeHandle::new("worker", limits());
        envelope.log("message").unwrap();
        envelope
            .tags(BTreeMap::from([("suite".into(), "unit".into())]))
            .unwrap();
        envelope
            .attach("trace", "text/plain", b"bytes".to_vec())
            .unwrap();
        envelope.snapshot("golden", "value").unwrap_err();
        envelope.stdout("out").unwrap();
        let report = envelope.report().unwrap();
        let decoded = EnvelopeReport::decode_process(&report.encode_process().unwrap()).unwrap();
        assert_eq!(decoded.logs(), report.logs());
        assert_eq!(decoded.tags(), report.tags());
        assert_eq!(decoded.artifacts(), report.artifacts());
        assert_eq!(decoded.snapshots(), report.snapshots());
        assert_eq!(decoded.stdout().as_text(), Some("out"));
    }

    #[test]
    fn stream_bytes_merge_across_workers_and_reject_whole_over_budget_writes() {
        let parent = EnvelopeHandle::new("parent", EnvelopeLimits::new(4, 0, 0));
        parent.stdout([0xe2]).unwrap();
        let worker = EnvelopeHandle::new("worker", EnvelopeLimits::new(4, 0, 0));
        worker.stdout([0x82, 0xac]).unwrap();
        worker.stderr([0xff]).unwrap();
        let wire = worker.report().unwrap().encode_process().unwrap();
        let decoded = EnvelopeReport::decode_process(&wire).unwrap();
        assert_eq!(decoded.stdout().as_bytes(), [0x82, 0xac]);
        assert_eq!(decoded.stderr().as_bytes(), [0xff]);
        parent.merge_worker_report(&decoded, &[]).unwrap();
        let before = parent.report().unwrap();
        assert_eq!(before.stdout().as_text(), Some("€"));
        assert_eq!(before.stderr().as_bytes(), [0xff]);
        assert_eq!(parent.stdout([0, 1]), Err(ControlError::OutputLimit));
        assert_eq!(parent.report().unwrap(), before);
        assert_eq!(parent.output_len(true).unwrap(), 3);
        assert_eq!(parent.output_len(false).unwrap(), 1);
        parent.set_phase(ExecutionPhase::Cleanup).unwrap();
        parent.stderr([0, 1, 2, 3]).unwrap();
        parent.close().unwrap();
        let complete = parent.report().unwrap();
        assert_eq!(complete.stderr().as_bytes(), [0xff, 0, 1, 2, 3]);
        assert!(parent.stdout([1]).is_err());
        assert!(parent.output_len(true).is_err());
        assert_eq!(
            EnvelopeReport::decode_process(&complete.encode_process().unwrap()).unwrap(),
            complete
        );
    }

    #[test]
    fn process_report_round_trip_maps_every_terminal_wire_vocabulary() {
        let codes = [
            "E2003", "E2100", "E2200", "E2201", "E3000", "E3001", "E3002", "E3003", "P0007",
            "P0008", "P2001", "P2002", "P2003", "P2004", "P2005", "P2006", "P2007", "P2008",
            "R0001", "R0002", "R0003",
        ];
        for code in codes {
            let report = EnvelopeReport {
                phase: ExecutionPhase::Closed,
                terminal: Some(Terminal::FailNow {
                    code,
                    message: "failed".into(),
                }),
                logs: Vec::new(),
                tags: BTreeMap::new(),
                artifacts: Vec::new(),
                snapshots: Vec::new(),
                virtual_time: Vec::new(),
                stdout: CapturedOutput::default(),
                stderr: CapturedOutput::default(),
            };
            assert_eq!(
                EnvelopeReport::decode_process(&report.encode_process().unwrap())
                    .unwrap()
                    .terminal(),
                report.terminal()
            );
        }
        for kind in [
            "bytes",
            "artifacts",
            "snapshots",
            "output",
            "memory",
            "instructions",
        ] {
            let report = EnvelopeReport {
                phase: ExecutionPhase::Closed,
                terminal: Some(Terminal::ResourceLimit { kind }),
                logs: Vec::new(),
                tags: BTreeMap::new(),
                artifacts: Vec::new(),
                snapshots: Vec::new(),
                virtual_time: Vec::new(),
                stdout: CapturedOutput::default(),
                stderr: CapturedOutput::default(),
            };
            assert!(EnvelopeReport::decode_process(&report.encode_process().unwrap()).is_ok());
        }
    }

    #[test]
    fn snapshot_update_mode_accepts_drift_and_exposes_private_candidates() {
        let envelope = EnvelopeHandle::new("worker", limits());
        envelope
            .with_expected_snapshots(BTreeMap::from([("golden".into(), "old".into())]))
            .unwrap();
        envelope.with_snapshot_update(true).unwrap();
        assert!(matches!(
            envelope.snapshot("golden", "new").unwrap(),
            SnapshotOutcome::Mismatched { .. }
        ));
        assert_eq!(
            envelope.snapshot_updates().unwrap(),
            [("golden".into(), "new".into())]
        );
    }

    #[test]
    fn merged_worker_evidence_is_limited_and_resequenced() {
        let child = EnvelopeHandle::new("child", limits());
        child.log("child-log").unwrap();
        child.stdout("child-out").unwrap();
        let report = child.report().unwrap();
        let parent = EnvelopeHandle::new("parent", limits());
        parent.log("parent-log").unwrap();
        parent
            .merge_worker_report(&report, &[("golden".into(), "value".into())])
            .unwrap();
        let merged = parent.report().unwrap();
        assert_eq!(
            merged
                .logs()
                .iter()
                .map(LogRecord::sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(merged.stdout().as_text(), Some("child-out"));
        assert_eq!(
            parent.snapshot_updates().unwrap(),
            [("golden".into(), "value".into())]
        );
    }

    #[test]
    fn tags_merge_atomically_and_choose_the_lexicographically_first_conflict() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .tags(BTreeMap::from([
                ("b".into(), "old".into()),
                ("z".into(), "old".into()),
            ]))
            .unwrap();
        let error = envelope
            .tags(BTreeMap::from([
                ("a".into(), "new".into()),
                ("b".into(), "different".into()),
                ("z".into(), "different".into()),
            ]))
            .unwrap_err();
        assert_eq!(error, ControlError::TagConflict { key: "b".into() });
        let report = envelope.report().unwrap();
        assert!(!report.tags().contains_key("a"));
        assert_eq!(report.tags().get("b"), Some(&"old".into()));
    }

    #[test]
    fn duplicate_equal_tags_are_idempotent_and_budgeted_once() {
        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(2, 0, 0));
        envelope
            .tags(BTreeMap::from([("a".into(), "b".into())]))
            .unwrap();
        envelope
            .tags(BTreeMap::from([("a".into(), "b".into())]))
            .unwrap();
        assert_eq!(
            envelope.tags(BTreeMap::from([("c".into(), "d".into())])),
            Err(ControlError::OutputLimit)
        );
    }

    #[test]
    fn fail_now_and_cleanup_failure_have_deterministic_precedence() {
        let envelope = EnvelopeHandle::new("node", limits());
        assert_eq!(
            envelope.skip("not applicable"),
            Err(ControlError::Skip {
                reason: "not applicable".into()
            })
        );
        envelope.set_phase(ExecutionPhase::Cleanup).unwrap();
        assert_eq!(envelope.skip("late"), Err(ControlError::SkipDuringCleanup));
        assert_eq!(
            envelope.cleanup_failure("P0007", "cleanup panic"),
            Err(ControlError::CleanupFailure {
                code: "P0007".into(),
                message: "cleanup panic".into()
            })
        );
        assert!(matches!(
            envelope.report().unwrap().terminal(),
            Some(Terminal::CleanupFailure { .. })
        ));

        let envelope = EnvelopeHandle::new("node", limits());
        assert_eq!(
            envelope.fail_now("stop"),
            Err(ControlError::FailNow {
                message: "stop".into()
            })
        );
        assert_eq!(
            envelope.report().unwrap().terminal().unwrap(),
            &Terminal::FailNow {
                code: "P0007",
                message: "stop".into()
            }
        );
    }

    #[test]
    fn artifacts_validate_names_media_and_atomic_limits() {
        // The descriptor consumes output even when the payload has its own cap.
        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(200, 3, 0));
        envelope.attach("a", "text/plain", b"abc".to_vec()).unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.artifacts()[0].sha256(), sha256(b"abc"));
        assert_eq!(
            envelope.attach("a", "text/plain", b"abc".to_vec()),
            Err(ControlError::ArtifactConflict { name: "a".into() })
        );
        assert_eq!(
            envelope.attach("b", "text/plain", b"x".to_vec()),
            Err(ControlError::ArtifactLimit)
        );
        assert_eq!(
            envelope.attach("bad/name", "text/plain", Vec::new()),
            Err(ControlError::InvalidName {
                value: "bad/name".into()
            })
        );
        assert_eq!(
            envelope.attach("bad", "plain", Vec::new()),
            Err(ControlError::InvalidMediaType {
                value: "plain".into()
            })
        );
    }

    #[test]
    fn descriptor_and_count_limits_reject_without_publishing_or_charging_partial_evidence() {
        // "a" + "text/plain" + "sha256:" and its 64 hex digits = 82 bytes.
        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(81, 3, 3));
        assert_eq!(
            envelope.attach("a", "text/plain", b"abc".to_vec()),
            Err(ControlError::OutputLimit)
        );
        assert!(envelope.report().unwrap().artifacts().is_empty());
        envelope.stdout("x".repeat(81)).unwrap();
        let profile = LimitProfile::default()
            .with_artifact_count(1)
            .with_snapshot_count(1);
        let envelope = EnvelopeHandle::new("node", profile.envelope_limits().unwrap());
        envelope.with_snapshot_update(true).unwrap();
        envelope.attach("a", "text/plain", Vec::new()).unwrap();
        assert_eq!(
            envelope.attach("b", "text/plain", Vec::new()),
            Err(ControlError::ResourceLimit {
                kind: BudgetKind::ArtifactCount
            })
        );
        envelope.snapshot("a", "").unwrap();
        assert_eq!(
            envelope.snapshot("b", ""),
            Err(ControlError::ResourceLimit {
                kind: BudgetKind::SnapshotCount
            })
        );
        let report = envelope.report().unwrap();
        assert_eq!(report.artifacts().len(), 1);
        assert_eq!(report.snapshots().len(), 1);
        assert_eq!(
            envelope.snapshot_updates().unwrap(),
            [("a".into(), "".into())]
        );
    }

    #[test]
    fn empty_logs_and_sequential_domains_exhaust_finite_metadata() {
        let profile = LimitProfile::default().with_metadata(80);
        let logs = EnvelopeHandle::new("logs", profile.envelope_limits().unwrap());
        logs.log("").unwrap();
        logs.log("").unwrap();
        assert_eq!(
            logs.log(""),
            Err(ControlError::ResourceLimit {
                kind: BudgetKind::Metadata
            })
        );
        assert_eq!(logs.report().unwrap().logs().len(), 2);
        let clocks = EnvelopeHandle::new("clocks", profile.envelope_limits().unwrap());
        clocks.with_virtual_time(|_| Ok(())).unwrap();
        clocks.with_virtual_time(|_| Ok(())).unwrap();
        assert_eq!(
            clocks.with_virtual_time(|_| Ok(())),
            Err(ControlError::ResourceLimit {
                kind: BudgetKind::Metadata
            })
        );
        assert_eq!(clocks.report().unwrap().virtual_time().len(), 2);
        clocks.close().unwrap();
    }

    #[test]
    fn suite_evidence_resets_allowances_once_for_teardown_and_keeps_setup_records() {
        let envelope = EnvelopeHandle::new("suite", EnvelopeLimits::new(3, 0, 0));
        envelope.log("abc").unwrap();
        envelope.set_phase(ExecutionPhase::Cleanup).unwrap();
        envelope.log("def").unwrap();
        envelope.set_phase(ExecutionPhase::Cleanup).unwrap();
        assert_eq!(envelope.log("x"), Err(ControlError::OutputLimit));
        let report = envelope.report().unwrap();
        assert_eq!(
            report
                .logs()
                .iter()
                .map(|log| (log.sequence(), log.message()))
                .collect::<Vec<_>>(),
            [(1, "abc"), (2, "def")]
        );
    }

    #[test]
    fn worker_report_merge_is_atomic_on_late_conflicts() {
        let parent = EnvelopeHandle::new("parent", limits());
        parent
            .tags(BTreeMap::from([("key".into(), "before".into())]))
            .unwrap();
        parent.log("before").unwrap();
        let before = parent.report().unwrap();
        let child = EnvelopeHandle::new("child", limits());
        child.log("must-not-be-imported").unwrap();
        child
            .tags(BTreeMap::from([("key".into(), "conflict".into())]))
            .unwrap();
        assert!(matches!(
            parent.merge_worker_report(&child.report().unwrap(), &[]),
            Err(ControlError::TagConflict { .. })
        ));
        assert_eq!(parent.report().unwrap(), before);
    }

    #[test]
    fn snapshots_record_match_missing_mismatch_and_duplicate() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .with_expected_snapshots(BTreeMap::from([
                ("ok".into(), "value".into()),
                ("bad".into(), "expected".into()),
            ]))
            .unwrap();
        assert!(matches!(
            envelope.snapshot("ok", "value"),
            Ok(SnapshotOutcome::Matched { .. })
        ));
        assert_eq!(
            envelope.snapshot("bad", "actual"),
            Err(ControlError::SnapshotMismatch { name: "bad".into() })
        );
        assert_eq!(
            envelope.snapshot("missing", "new"),
            Err(ControlError::SnapshotMismatch {
                name: "missing".into()
            })
        );
        assert_eq!(
            envelope.snapshot("ok", "value"),
            Err(ControlError::SnapshotConflict { name: "ok".into() })
        );
        assert_eq!(envelope.report().unwrap().snapshots().len(), 3);
    }

    #[test]
    fn virtual_time_is_single_domain_and_revoked_after_closure() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .with_virtual_time(|time| {
                time.advance(10)?;
                time.settle()?;
                Ok(())
            })
            .unwrap();
        envelope
            .with_virtual_time(|time| {
                time.settle()?;
                Ok(())
            })
            .unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time().len(), 2);
        assert_eq!(report.virtual_time()[0].index(), 1);
        assert_eq!(report.virtual_time()[1].index(), 2);
        assert_eq!(
            envelope.with_virtual_time(|_| envelope.with_virtual_time(|_| Ok(()))),
            Err(ControlError::VirtualTimeActive)
        );
        assert_eq!(
            envelope.with_virtual_time(|time| time.advance(-1)),
            Err(ControlError::InvalidDuration)
        );
    }

    #[test]
    fn virtual_time_exposes_local_timer_queue_and_maps_safety_errors() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .with_virtual_time(|time| {
                time.register_task("task")?;
                assert_eq!(time.schedule_timer("wake", "task", 5)?, 5);
                assert_eq!(
                    time.auto_advance()?,
                    AutoAdvance::Advanced {
                        from: 0,
                        to: 5,
                        timer: "wake".into()
                    }
                );
                time.complete("task")?;
                assert_eq!(time.auto_advance()?, AutoAdvance::Quiescent);
                Ok(())
            })
            .unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time()[0].elapsed_ns(), 5);
        assert_eq!(report.virtual_time()[0].automatic_advances(), 1);

        let deadlock = EnvelopeHandle::new("deadlock", limits());
        assert_eq!(
            deadlock.with_virtual_time(|time| {
                time.register_task("joiner")?;
                time.block("joiner", WaitKind::Join)?;
                time.auto_advance().map(|_| ())
            }),
            Err(ControlError::VirtualDeadlock)
        );

        let external = EnvelopeHandle::new("external", limits());
        assert_eq!(
            external.with_virtual_time(|time| {
                time.register_task("io")?;
                time.block("io", WaitKind::External)?;
                time.auto_advance().map(|_| ())
            }),
            Err(ControlError::VirtualExternalWait { task: "io".into() })
        );
        assert_eq!(ControlError::VirtualDeadlock.code(), "P2003");
        assert_eq!(ControlError::VirtualOverflow.code(), "P2005");
    }

    #[test]
    fn structured_child_shares_sinks_but_not_a_visible_context() {
        let envelope = EnvelopeHandle::new("node", limits());
        let child = envelope.child().unwrap();
        child.log("child").unwrap();
        child
            .tags(BTreeMap::from([("k".into(), "v".into())]))
            .unwrap();
        assert_eq!(envelope.report().unwrap().logs()[0].message(), "child");
        assert_eq!(
            envelope.report().unwrap().tags().get("k"),
            Some(&"v".into())
        );
        assert_eq!(
            child.skip("cancel"),
            Err(ControlError::Skip {
                reason: "structured child requested skip".into()
            })
        );
    }

    #[test]
    fn first_child_skip_wins_independent_of_completion_order() {
        let envelope = EnvelopeHandle::new("node", limits());
        let first = envelope.child().unwrap();
        let second = envelope.child().unwrap();
        second.skip("second").unwrap_err();
        first.skip("first").unwrap_err();
        assert_eq!(
            envelope.report().unwrap().terminal(),
            Some(&Terminal::Skipped {
                reason: "first".into()
            })
        );
        for cleanup in [false, true] {
            let envelope = EnvelopeHandle::new("node", limits());
            let first = envelope.child().unwrap();
            let second = envelope.child().unwrap();
            second.skip("second").unwrap_err();
            if cleanup {
                envelope
                    .cleanup_failure("P0008", "cleanup failed")
                    .unwrap_err();
            } else {
                assert!(envelope.record_runtime_limit("node", "memory").unwrap());
            }
            let terminal = envelope.report().unwrap().terminal().cloned();
            first.skip("first").unwrap_err();
            assert_eq!(envelope.report().unwrap().terminal(), terminal.as_ref());
        }
    }

    #[test]
    fn phase_and_close_boundaries_are_strict() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.set_phase(ExecutionPhase::Setup).unwrap();
        assert_eq!(envelope.set_phase(ExecutionPhase::Body), Ok(()));
        assert_eq!(
            envelope.set_phase(ExecutionPhase::Setup),
            Err(ControlError::PhaseRegression {
                current: ExecutionPhase::Body,
                requested: ExecutionPhase::Setup
            })
        );
        envelope.close().unwrap();
        assert_eq!(envelope.log("late"), Err(ControlError::Closed));
        assert_eq!(envelope.close(), Err(ControlError::Closed));
    }

    #[test]
    fn production_intrinsics_are_rejected_at_the_admission_boundary() {
        assert_eq!(admit_operation(TestSourceClass::UnitTest, "log"), Ok(()));
        assert_eq!(
            admit_operation(TestSourceClass::Production, "log"),
            Err(ControlError::ProductionOperation { operation: "log" })
        );
    }

    #[test]
    fn evidence_name_and_media_type_grammar_is_closed() {
        assert!(validate_evidence_name("résumé").is_ok());
        assert!(validate_evidence_name("line\n").is_err());
        assert!(validate_media_type("application/json").is_ok());
        assert!(validate_media_type("text/plain; charset=utf-8").is_err());
        assert!(validate_media_type("/plain").is_err());
        assert!(validate_media_type("text/plain/extra").is_err());
    }

    #[test]
    fn report_orders_maps_without_reordering_observed_logs() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.attach("z", "text/plain", b"z".to_vec()).unwrap();
        envelope.attach("a", "text/plain", b"a".to_vec()).unwrap();
        envelope.log("first").unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(
            report
                .artifacts()
                .iter()
                .map(|item| item.name())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(report.logs()[0].message(), "first");
    }

    #[test]
    fn expected_snapshots_install_only_before_first_check() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.with_expected_snapshots(BTreeMap::new()).unwrap();
        assert_eq!(
            envelope.with_expected_snapshots(BTreeMap::new()),
            Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into()
            })
        );
    }

    #[test]
    fn expected_snapshot_installation_closes_after_a_snapshot_check() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.snapshot("first", "actual").unwrap_err();
        assert_eq!(
            envelope.with_expected_snapshots(BTreeMap::new()),
            Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into()
            })
        );
    }

    #[test]
    fn hash_validation_helper_is_used_for_artifact_identity() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .attach("x", "application/octet-stream", b"data".to_vec())
            .unwrap();
        crate::artifact::validate_sha256(envelope.report().unwrap().artifacts()[0].sha256())
            .unwrap();
    }

    #[test]
    fn public_control_views_preserve_limits_evidence_and_error_codes() {
        let budget = EnvelopeLimits::new(7, 8, 9);
        assert_eq!(std::hint::black_box(budget.output_bytes()), 7);
        assert_eq!(std::hint::black_box(budget.artifact_bytes()), 8);
        assert_eq!(std::hint::black_box(budget.snapshot_bytes()), 9);
        assert_eq!(ExecutionPhase::Setup.rank(), 0);
        assert_eq!(ExecutionPhase::Closed.rank(), 3);

        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(1_000, 1_000, 1_000));
        envelope.log("hello").unwrap();
        envelope
            .tags(BTreeMap::from([("k".into(), "v".into())]))
            .unwrap();
        envelope.stdout("out").unwrap();
        envelope.stderr("err").unwrap();
        envelope
            .attach("artifact", "text/plain", b"bytes".to_vec())
            .unwrap();
        envelope.snapshot("snapshot", "value").unwrap_err();
        envelope
            .with_virtual_time(|time| {
                time.advance(2)?;
                time.settle()
            })
            .unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(std::hint::black_box(report.phase()), ExecutionPhase::Setup);
        assert!(std::hint::black_box(report.terminal()).is_none());
        assert_eq!(std::hint::black_box(report.logs())[0].sequence(), 1);
        assert_eq!(std::hint::black_box(report.logs())[0].message(), "hello");
        assert_eq!(
            std::hint::black_box(report.tags()).get("k"),
            Some(&"v".into())
        );
        assert_eq!(
            std::hint::black_box(report.artifacts())[0].name(),
            "artifact"
        );
        assert_eq!(
            std::hint::black_box(report.artifacts())[0].media_type(),
            "text/plain"
        );
        assert_eq!(
            std::hint::black_box(report.artifacts())[0].bytes(),
            b"bytes"
        );
        assert!(
            !std::hint::black_box(report.artifacts())[0]
                .sha256()
                .is_empty()
        );
        assert_eq!(
            std::hint::black_box(report.snapshots())[0].name(),
            "snapshot"
        );
        assert!(matches!(
            std::hint::black_box(report.snapshots())[0].outcome(),
            SnapshotOutcome::Missing { .. }
        ));
        assert_eq!(std::hint::black_box(report.virtual_time())[0].index(), 1);
        assert_eq!(
            std::hint::black_box(report.virtual_time())[0].elapsed_ns(),
            2
        );
        assert_eq!(
            std::hint::black_box(report.virtual_time())[0].automatic_advances(),
            0
        );
        assert_eq!(std::hint::black_box(report.virtual_time())[0].settles(), 1);
        assert_eq!(std::hint::black_box(report.virtual_time())[0].advances(), 1);
        assert_eq!(std::hint::black_box(report.stdout()).as_text(), Some("out"));
        assert_eq!(std::hint::black_box(report.stderr()).as_text(), Some("err"));

        for error in [
            ControlError::Closed,
            ControlError::OutputLimit,
            ControlError::SnapshotMismatch { name: "x".into() },
            ControlError::ProductionOperation { operation: "log" },
        ] {
            assert!(!error.to_string().is_empty());
            assert!(!error.code().is_empty());
        }
    }
}
