//! Transactional external interruption for the test coordinator.
//!
//! This module deliberately stops at the coordinator/worker boundary.  OS
//! signal registration belongs to the CLI; here an injected request exercises
//! the same deterministic transaction: stop dispatch, close output staging,
//! cancel workers, wait for cleanup and resource revocation, then either
//! restore isolation (exit 4) or record isolation loss (exit 3).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::test_limits::{InterruptController, InterruptState, LimitError, LimitProfile};

pub const TEST_INTERRUPT_FORMAT: &str = "tondo-test-interrupt-0.1/1";

/// Origin of an external cancellation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InterruptOrigin {
    User,
    Host,
    Supervisor,
}

impl InterruptOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Host => "host",
            Self::Supervisor => "supervisor",
        }
    }
}

/// Validated reason carried by an injected interrupt event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptRequest {
    origin: InterruptOrigin,
    reason: String,
}

impl InterruptRequest {
    pub fn new(origin: InterruptOrigin, reason: impl Into<String>) -> Result<Self, InterruptError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(InterruptError::EmptyReason);
        }
        if reason.contains(['\n', '\r']) {
            return Err(InterruptError::InvalidText("reason"));
        }
        Ok(Self { origin, reason })
    }

    pub const fn origin(&self) -> InterruptOrigin {
        self.origin
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// A machine-readable output that can be staged by a complete invocation.
/// Interrupting an invocation discards every staged value and never publishes
/// one as a partial report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputKind {
    Json,
    Junit,
    ArtifactManifest,
    SnapshotUpdate,
}

impl OutputKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Junit => "junit",
            Self::ArtifactManifest => "artifact-manifest",
            Self::SnapshotUpdate => "snapshot-update",
        }
    }
}

/// State visible for one final output path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputState {
    /// No bytes existed before this invocation and no staged bytes remain.
    Absent,
    /// Bytes are staged but have not crossed the atomic publication boundary.
    Staged,
    /// A complete output exists and is retained across an interruption.
    Published,
}

/// In-memory publication ledger used by the coordinator transaction.
///
/// Real filesystem stores perform the final atomic rename in their own
/// boundary.  This ledger makes the safety rule testable without touching the
/// filesystem: an interruption clears staged outputs, leaves previous outputs
/// intact, and allows only content-addressed object orphans to remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLedger {
    previous: BTreeSet<OutputKind>,
    staged: BTreeSet<OutputKind>,
    aborted: bool,
    orphan_blobs: BTreeSet<String>,
}

impl OutputLedger {
    pub fn new(previous: impl IntoIterator<Item = OutputKind>) -> Self {
        Self {
            previous: previous.into_iter().collect(),
            staged: BTreeSet::new(),
            aborted: false,
            orphan_blobs: BTreeSet::new(),
        }
    }

    pub fn stage(&mut self, kind: OutputKind) -> Result<(), InterruptError> {
        if self.aborted {
            return Err(InterruptError::OutputClosed(kind));
        }
        if !self.staged.insert(kind) {
            return Err(InterruptError::OutputAlreadyStaged(kind));
        }
        Ok(())
    }

    /// Atomically publish all staged outputs for a complete invocation.
    pub fn publish(&mut self) -> Result<(), InterruptError> {
        if self.aborted {
            return Err(InterruptError::OutputClosed(OutputKind::Json));
        }
        self.previous.extend(std::mem::take(&mut self.staged));
        Ok(())
    }

    /// Abort publication without deleting a previous complete output.
    pub fn abort_partial(&mut self) {
        self.staged.clear();
        self.aborted = true;
    }

    pub fn state(&self, kind: OutputKind) -> OutputState {
        if self.staged.contains(&kind) {
            OutputState::Staged
        } else if self.previous.contains(&kind) {
            OutputState::Published
        } else {
            OutputState::Absent
        }
    }

    pub const fn aborted(&self) -> bool {
        self.aborted
    }

    pub fn previous(&self) -> impl Iterator<Item = OutputKind> + '_ {
        self.previous.iter().copied()
    }

    pub fn staged(&self) -> impl Iterator<Item = OutputKind> + '_ {
        self.staged.iter().copied()
    }

    pub fn orphan_blobs(&self) -> impl Iterator<Item = &str> {
        self.orphan_blobs.iter().map(String::as_str)
    }

    /// Record a content-addressed object that may be collected after an
    /// interrupted invocation.  No arbitrary path or opaque blob is accepted.
    pub fn retain_orphan_blob(&mut self, digest: impl Into<String>) -> Result<(), InterruptError> {
        let digest = digest.into();
        let Some(hash) = digest.strip_prefix("sha256:") else {
            return Err(InterruptError::InvalidDigest(digest));
        };
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InterruptError::InvalidDigest(digest));
        }
        self.orphan_blobs.insert(digest);
        Ok(())
    }
}

/// Lifecycle state of one isolated worker during interruption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    Cancelling,
    Cancelled,
    Closed,
    Forced,
}

/// A worker may acknowledge cancellation only after all user cleanup and host
/// resource revocation have completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancellationAck {
    cleanup_complete: bool,
    resources_revoked: bool,
}

impl CancellationAck {
    pub const fn complete() -> Self {
        Self {
            cleanup_complete: true,
            resources_revoked: true,
        }
    }

    pub const fn incomplete() -> Self {
        Self {
            cleanup_complete: false,
            resources_revoked: false,
        }
    }

    pub const fn cleanup_complete(self) -> bool {
        self.cleanup_complete
    }

    pub const fn resources_revoked(self) -> bool {
        self.resources_revoked
    }
}

/// Coordinator transaction state.  `Interrupted` is the safe exit-4 state;
/// `LostIsolation` is the exit-3 state and is never converted into a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionPhase {
    Running,
    Cancelling { requested_at: u64 },
    Interrupted,
    LostIsolation,
}

/// Exit selected by the interruption transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptExit {
    Interrupted,
    IsolationLost,
}

impl InterruptExit {
    pub const fn code(self) -> u8 {
        match self {
            Self::Interrupted => 4,
            Self::IsolationLost => 3,
        }
    }
}

/// Effect of an injected request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptRequestResult {
    Started,
    Forced,
    Ignored,
}

/// Immutable evidence returned after the transaction reaches a terminal
/// phase.  `machine_output_published` is always false for an interruption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptionOutcome {
    exit: InterruptExit,
    request: InterruptRequest,
    outputs: OutputLedger,
    machine_output_published: bool,
}

impl InterruptionOutcome {
    pub const fn exit(&self) -> InterruptExit {
        self.exit
    }

    pub const fn exit_code(&self) -> u8 {
        self.exit.code()
    }

    pub const fn request(&self) -> &InterruptRequest {
        &self.request
    }

    pub const fn machine_output_published(&self) -> bool {
        self.machine_output_published
    }

    pub fn outputs(&self) -> &OutputLedger {
        &self.outputs
    }

    pub fn human_line(&self) -> String {
        format!(
            "interrupted: {} ({})",
            self.request.reason(),
            self.request.origin().as_str()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerSlot {
    state: WorkerState,
}

/// Coordinator-side interruption transaction with an injectable monotonic
/// clock and worker acknowledgements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptionCoordinator {
    controller: InterruptController,
    phase: InterruptionPhase,
    last_now: u64,
    request: Option<InterruptRequest>,
    workers: BTreeMap<String, WorkerSlot>,
    dispatch: BTreeSet<String>,
    outputs: OutputLedger,
    stores_closed: bool,
    reporters_closed: bool,
}

impl InterruptionCoordinator {
    pub fn new(
        profile: LimitProfile,
        workers: impl IntoIterator<Item = String>,
        outputs: OutputLedger,
        now: u64,
    ) -> Result<Self, InterruptError> {
        let mut worker_map = BTreeMap::new();
        for worker in workers {
            if worker.trim().is_empty() {
                return Err(InterruptError::EmptyWorker);
            }
            if worker.contains(['\n', '\r']) {
                return Err(InterruptError::InvalidText("worker"));
            }
            if worker_map
                .insert(
                    worker.clone(),
                    WorkerSlot {
                        state: WorkerState::Running,
                    },
                )
                .is_some()
            {
                return Err(InterruptError::DuplicateWorker(worker));
            }
        }
        Ok(Self {
            controller: InterruptController::new(profile).map_err(InterruptError::Clock)?,
            phase: InterruptionPhase::Running,
            last_now: now,
            request: None,
            workers: worker_map,
            dispatch: BTreeSet::new(),
            outputs,
            stores_closed: false,
            reporters_closed: false,
        })
    }

    pub const fn phase(&self) -> InterruptionPhase {
        self.phase
    }

    pub const fn dispatch_stopped(&self) -> bool {
        !matches!(self.phase, InterruptionPhase::Running)
    }

    pub const fn stores_closed(&self) -> bool {
        self.stores_closed
    }

    pub const fn reporters_closed(&self) -> bool {
        self.reporters_closed
    }

    pub fn worker_state(&self, worker: &str) -> Result<WorkerState, InterruptError> {
        self.workers
            .get(worker)
            .map(|slot| slot.state)
            .ok_or_else(|| InterruptError::UnknownWorker(worker.into()))
    }

    pub fn cancel_pending(&self) -> impl Iterator<Item = &str> {
        self.workers
            .iter()
            .filter(|(_, slot)| matches!(slot.state, WorkerState::Cancelling))
            .map(|(worker, _)| worker.as_str())
    }

    pub fn dispatch(&mut self, unit: impl Into<String>) -> Result<bool, InterruptError> {
        let unit = unit.into();
        if unit.trim().is_empty() {
            return Err(InterruptError::EmptyUnit);
        }
        if unit.contains(['\n', '\r']) {
            return Err(InterruptError::InvalidText("unit"));
        }
        if self.dispatch_stopped() {
            return Ok(false);
        }
        Ok(self.dispatch.insert(unit))
    }

    pub fn stage_output(&mut self, kind: OutputKind) -> Result<(), InterruptError> {
        if self.dispatch_stopped() {
            return Err(InterruptError::OutputClosed(kind));
        }
        self.outputs.stage(kind)
    }

    pub fn publish_complete(&mut self) -> Result<(), InterruptError> {
        if self.dispatch_stopped() {
            return Err(InterruptError::OutputClosed(OutputKind::Json));
        }
        self.outputs.publish()
    }

    pub fn retain_orphan_blob(&mut self, digest: impl Into<String>) -> Result<(), InterruptError> {
        self.outputs.retain_orphan_blob(digest)
    }

    pub fn outputs(&self) -> &OutputLedger {
        &self.outputs
    }

    /// Inject the first or subsequent external interruption event.
    pub fn request(
        &mut self,
        request: InterruptRequest,
        now: u64,
    ) -> Result<InterruptRequestResult, InterruptError> {
        self.observe(now)?;
        match self.phase {
            InterruptionPhase::Running => {
                if !self.controller.request(now) {
                    return Ok(InterruptRequestResult::Ignored);
                }
                self.request = Some(request);
                self.stores_closed = true;
                self.reporters_closed = true;
                self.outputs.abort_partial();
                self.dispatch.clear();
                for slot in self.workers.values_mut() {
                    if slot.state == WorkerState::Running {
                        slot.state = WorkerState::Cancelling;
                    }
                }
                self.phase = InterruptionPhase::Cancelling { requested_at: now };
                self.finish_if_isolated();
                Ok(InterruptRequestResult::Started)
            }
            InterruptionPhase::Cancelling { .. } => {
                self.force_isolation();
                Ok(InterruptRequestResult::Forced)
            }
            InterruptionPhase::Interrupted | InterruptionPhase::LostIsolation => {
                Ok(InterruptRequestResult::Ignored)
            }
        }
    }

    /// Record that a worker completed cleanup (including `defer await`) and
    /// revoked every host resource it owned.
    pub fn acknowledge_cancel(
        &mut self,
        worker: &str,
        ack: CancellationAck,
    ) -> Result<(), InterruptError> {
        if !matches!(self.phase, InterruptionPhase::Cancelling { .. }) {
            return Err(InterruptError::InvalidPhase {
                expected: "cancelling",
                actual: self.phase_name(),
            });
        }
        if !ack.cleanup_complete() || !ack.resources_revoked() {
            return Err(InterruptError::IncompleteCleanup(worker.into()));
        }
        let slot = self
            .workers
            .get_mut(worker)
            .ok_or_else(|| InterruptError::UnknownWorker(worker.into()))?;
        if slot.state != WorkerState::Cancelling {
            return Err(InterruptError::InvalidWorkerState(worker.into()));
        }
        slot.state = WorkerState::Cancelled;
        Ok(())
    }

    /// Close a worker session after its cancellation acknowledgement.  The
    /// coordinator reaches safe exit 4 only after every session is closed.
    pub fn close_worker(&mut self, worker: &str) -> Result<(), InterruptError> {
        if !matches!(self.phase, InterruptionPhase::Cancelling { .. }) {
            return Err(InterruptError::InvalidPhase {
                expected: "cancelling",
                actual: self.phase_name(),
            });
        }
        let slot = self
            .workers
            .get_mut(worker)
            .ok_or_else(|| InterruptError::UnknownWorker(worker.into()))?;
        if slot.state != WorkerState::Cancelled {
            return Err(InterruptError::InvalidWorkerState(worker.into()));
        }
        slot.state = WorkerState::Closed;
        self.finish_if_isolated();
        Ok(())
    }

    /// Advance the real monotonic grace clock.  Virtual time must not be used
    /// for this method; the caller supplies the host monotonic timestamp.
    pub fn poll(&mut self, now: u64) -> Result<InterruptionPhase, InterruptError> {
        self.observe(now)?;
        if matches!(self.phase, InterruptionPhase::Cancelling { .. })
            && self.controller.poll(now).map_err(InterruptError::Clock)? == InterruptState::Forced
        {
            self.force_isolation();
        }
        Ok(self.phase)
    }

    pub fn outcome(&self) -> Option<InterruptionOutcome> {
        let exit = match self.phase {
            InterruptionPhase::Interrupted => InterruptExit::Interrupted,
            InterruptionPhase::LostIsolation => InterruptExit::IsolationLost,
            InterruptionPhase::Running | InterruptionPhase::Cancelling { .. } => return None,
        };
        Some(InterruptionOutcome {
            exit,
            request: self
                .request
                .clone()
                .expect("terminal interruption has a request"),
            outputs: self.outputs.clone(),
            machine_output_published: false,
        })
    }

    fn observe(&mut self, now: u64) -> Result<(), InterruptError> {
        if now < self.last_now {
            return Err(InterruptError::Clock(LimitError::ClockRegression {
                previous: self.last_now,
                current: now,
            }));
        }
        self.last_now = now;
        Ok(())
    }

    fn phase_name(&self) -> &'static str {
        match self.phase {
            InterruptionPhase::Running => "running",
            InterruptionPhase::Cancelling { .. } => "cancelling",
            InterruptionPhase::Interrupted => "interrupted",
            InterruptionPhase::LostIsolation => "lost-isolation",
        }
    }

    fn finish_if_isolated(&mut self) {
        if matches!(self.phase, InterruptionPhase::Cancelling { .. })
            && self
                .workers
                .values()
                .all(|slot| slot.state == WorkerState::Closed)
        {
            self.phase = InterruptionPhase::Interrupted;
        }
    }

    fn force_isolation(&mut self) {
        for slot in self.workers.values_mut() {
            if slot.state != WorkerState::Closed {
                slot.state = WorkerState::Forced;
            }
        }
        self.phase = InterruptionPhase::LostIsolation;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptError {
    EmptyReason,
    EmptyWorker,
    EmptyUnit,
    InvalidText(&'static str),
    DuplicateWorker(String),
    UnknownWorker(String),
    InvalidWorkerState(String),
    IncompleteCleanup(String),
    InvalidDigest(String),
    InvalidPhase {
        expected: &'static str,
        actual: &'static str,
    },
    OutputClosed(OutputKind),
    OutputAlreadyStaged(OutputKind),
    Clock(LimitError),
}

impl fmt::Display for InterruptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyReason => formatter.write_str("interrupt reason cannot be empty"),
            Self::EmptyWorker => formatter.write_str("worker identity cannot be empty"),
            Self::EmptyUnit => formatter.write_str("dispatch unit cannot be empty"),
            Self::InvalidText(field) => write!(formatter, "{field} cannot contain line breaks"),
            Self::DuplicateWorker(worker) => write!(formatter, "worker `{worker}` is duplicated"),
            Self::UnknownWorker(worker) => write!(formatter, "worker `{worker}` is unknown"),
            Self::InvalidWorkerState(worker) => {
                write!(formatter, "worker `{worker}` is not in the expected state")
            }
            Self::IncompleteCleanup(worker) => {
                write!(
                    formatter,
                    "worker `{worker}` acknowledged before cleanup/revocation"
                )
            }
            Self::InvalidDigest(digest) => {
                write!(
                    formatter,
                    "orphan object digest `{digest}` is not sha256-addressed"
                )
            }
            Self::InvalidPhase { expected, actual } => {
                write!(
                    formatter,
                    "interrupt phase is {actual}, expected {expected}"
                )
            }
            Self::OutputClosed(kind) => {
                write!(formatter, "{} output staging is closed", kind.as_str())
            }
            Self::OutputAlreadyStaged(kind) => {
                write!(formatter, "{} output is already staged", kind.as_str())
            }
            Self::Clock(error) => error.fmt(formatter),
        }
    }
}

impl Error for InterruptError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> LimitProfile {
        LimitProfile::default().with_grace_ns(5)
    }

    fn coordinator(previous: impl IntoIterator<Item = OutputKind>) -> InterruptionCoordinator {
        InterruptionCoordinator::new(
            profile(),
            vec!["worker-a".into(), "worker-b".into()],
            OutputLedger::new(previous),
            100,
        )
        .unwrap()
    }

    #[test]
    fn first_request_stops_dispatch_closes_outputs_and_sends_cancel_to_all_workers() {
        let mut coordinator = coordinator([OutputKind::Json]);
        assert!(coordinator.dispatch("test::one").unwrap());
        coordinator
            .stage_output(OutputKind::Junit)
            .expect("staging is open before interruption");
        let request = InterruptRequest::new(InterruptOrigin::User, "ctrl-c").unwrap();
        assert_eq!(
            coordinator.request(request, 101).unwrap(),
            InterruptRequestResult::Started
        );
        assert!(coordinator.dispatch_stopped());
        assert!(!coordinator.dispatch("test::two").unwrap());
        assert!(coordinator.stores_closed());
        assert!(coordinator.reporters_closed());
        assert!(coordinator.outputs().aborted());
        assert_eq!(
            coordinator.outputs().state(OutputKind::Json),
            OutputState::Published
        );
        assert_eq!(
            coordinator.outputs().state(OutputKind::Junit),
            OutputState::Absent
        );
        assert_eq!(
            coordinator.cancel_pending().collect::<Vec<_>>(),
            ["worker-a", "worker-b"]
        );
    }

    #[test]
    fn complete_cleanup_and_close_restore_isolation_and_select_exit_four() {
        let mut coordinator = coordinator([]);
        coordinator
            .request(
                InterruptRequest::new(InterruptOrigin::Host, "shutdown").unwrap(),
                101,
            )
            .unwrap();
        for worker in ["worker-a", "worker-b"] {
            coordinator
                .acknowledge_cancel(worker, CancellationAck::complete())
                .unwrap();
            coordinator.close_worker(worker).unwrap();
        }
        assert_eq!(coordinator.phase(), InterruptionPhase::Interrupted);
        let outcome = coordinator.outcome().unwrap();
        assert_eq!(outcome.exit_code(), 4);
        assert_eq!(outcome.exit(), InterruptExit::Interrupted);
        assert!(!outcome.machine_output_published());
        assert!(outcome.human_line().contains("interrupted"));
    }

    #[test]
    fn interrupt_waits_for_defer_await_cleanup_before_safe_exit() {
        let mut coordinator = coordinator([]);
        coordinator
            .request(
                InterruptRequest::new(InterruptOrigin::User, "ctrl-c").unwrap(),
                101,
            )
            .unwrap();

        assert!(matches!(
            coordinator.acknowledge_cancel("worker-a", CancellationAck::incomplete()),
            Err(InterruptError::IncompleteCleanup(worker)) if worker == "worker-a"
        ));
        assert_eq!(
            coordinator.phase(),
            InterruptionPhase::Cancelling { requested_at: 101 }
        );

        coordinator
            .acknowledge_cancel("worker-a", CancellationAck::complete())
            .unwrap();
        coordinator.close_worker("worker-a").unwrap();
        coordinator
            .acknowledge_cancel("worker-b", CancellationAck::complete())
            .unwrap();
        coordinator.close_worker("worker-b").unwrap();

        assert_eq!(coordinator.outcome().unwrap().exit_code(), 4);
    }

    #[test]
    fn grace_expiry_or_second_request_forces_exit_three_and_blocks_late_acks() {
        let mut transaction = coordinator([]);
        transaction
            .request(
                InterruptRequest::new(InterruptOrigin::Supervisor, "cancel").unwrap(),
                101,
            )
            .unwrap();
        assert_eq!(
            transaction.poll(106).unwrap(),
            InterruptionPhase::LostIsolation
        );
        assert_eq!(transaction.outcome().unwrap().exit_code(), 3);
        assert_eq!(
            transaction.acknowledge_cancel("worker-a", CancellationAck::complete()),
            Err(InterruptError::InvalidPhase {
                expected: "cancelling",
                actual: "lost-isolation"
            })
        );

        let mut second = coordinator([]);
        second
            .request(
                InterruptRequest::new(InterruptOrigin::User, "first").unwrap(),
                101,
            )
            .unwrap();
        assert_eq!(
            second
                .request(
                    InterruptRequest::new(InterruptOrigin::User, "second").unwrap(),
                    102
                )
                .unwrap(),
            InterruptRequestResult::Forced
        );
        assert_eq!(second.phase(), InterruptionPhase::LostIsolation);
        assert_eq!(
            second.worker_state("worker-a").unwrap(),
            WorkerState::Forced
        );
    }

    #[test]
    fn incomplete_ack_is_rejected_without_marking_worker_cancelled() {
        let mut coordinator = coordinator([]);
        coordinator
            .request(
                InterruptRequest::new(InterruptOrigin::User, "interrupt").unwrap(),
                101,
            )
            .unwrap();
        assert!(matches!(
            coordinator.acknowledge_cancel("worker-a", CancellationAck::incomplete()),
            Err(InterruptError::IncompleteCleanup(worker)) if worker == "worker-a"
        ));
        assert_eq!(
            coordinator.worker_state("worker-a").unwrap(),
            WorkerState::Cancelling
        );
        assert!(matches!(
            coordinator.close_worker("worker-a"),
            Err(InterruptError::InvalidWorkerState(worker)) if worker == "worker-a"
        ));
    }

    #[test]
    fn output_ledger_preserves_previous_outputs_and_rejects_non_content_addressed_orphans() {
        let mut outputs = OutputLedger::new([OutputKind::Json]);
        outputs.stage(OutputKind::Junit).unwrap();
        outputs
            .retain_orphan_blob(format!("sha256:{}", "a".repeat(64)))
            .unwrap();
        outputs.abort_partial();
        assert_eq!(outputs.state(OutputKind::Json), OutputState::Published);
        assert_eq!(outputs.state(OutputKind::Junit), OutputState::Absent);
        assert_eq!(outputs.orphan_blobs().count(), 1);
        assert!(matches!(
            outputs.retain_orphan_blob("/tmp/object"),
            Err(InterruptError::InvalidDigest(_))
        ));
        assert!(matches!(
            outputs.stage(OutputKind::Json),
            Err(InterruptError::OutputClosed(OutputKind::Json))
        ));
    }

    #[test]
    fn validation_rejects_duplicates_bad_text_and_clock_regression() {
        assert!(matches!(
            InterruptRequest::new(InterruptOrigin::User, "\n"),
            Err(InterruptError::EmptyReason)
        ));
        assert!(matches!(
            InterruptionCoordinator::new(
                profile(),
                vec!["worker-a".into(), "worker-a".into()],
                OutputLedger::new([]),
                0,
            ),
            Err(InterruptError::DuplicateWorker(worker)) if worker == "worker-a"
        ));
        let mut coordinator = coordinator([]);
        assert!(matches!(
            coordinator.request(
                InterruptRequest::new(InterruptOrigin::User, "interrupt").unwrap(),
                99,
            ),
            Err(InterruptError::Clock(LimitError::ClockRegression { .. }))
        ));
    }
}
