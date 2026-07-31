//! Finite resource profiles, atomic budgets and deterministic phase deadlines.
//!
//! This module is deliberately host-independent. The runner supplies real
//! monotonic timestamps at its boundary; tests and virtual-time domains can
//! use the same integer nanosecond API without changing the limit semantics.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::artifact::sha256;
use crate::test_control::EnvelopeLimits;

pub const TEST_LIMITS_FORMAT: &str = "tondo-test-limits-draft/1";

/// Normative defaults for one test attempt. Every structural budget is finite;
/// only the wall-clock timeout may be disabled explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitProfile {
    work: u64,
    memory: u64,
    depth: u64,
    output: u64,
    artifact_bytes: u64,
    artifact_count: u64,
    snapshot_bytes: u64,
    snapshot_count: u64,
    metadata: u64,
    virtual_timers: u64,
    ready_queue: u64,
    instructions: u64,
    timeout_ns: Option<u64>,
    grace_ns: u64,
}

impl Default for LimitProfile {
    fn default() -> Self {
        Self {
            work: 10_000_000,
            memory: 64 * 1024 * 1024,
            depth: 256,
            output: 1024 * 1024,
            artifact_bytes: 16 * 1024 * 1024,
            artifact_count: 256,
            snapshot_bytes: 16 * 1024 * 1024,
            snapshot_count: 256,
            metadata: 1024 * 1024,
            virtual_timers: 1_024,
            ready_queue: 4_096,
            instructions: 10_000_000,
            timeout_ns: Some(30_000_000_000),
            grace_ns: 1_000_000_000,
        }
    }
}

impl LimitProfile {
    pub const fn work(self) -> u64 {
        self.work
    }
    pub const fn memory(self) -> u64 {
        self.memory
    }
    pub const fn depth(self) -> u64 {
        self.depth
    }
    pub const fn output(self) -> u64 {
        self.output
    }
    pub const fn artifact_bytes(self) -> u64 {
        self.artifact_bytes
    }
    pub const fn artifact_count(self) -> u64 {
        self.artifact_count
    }
    pub const fn snapshot_bytes(self) -> u64 {
        self.snapshot_bytes
    }
    pub const fn snapshot_count(self) -> u64 {
        self.snapshot_count
    }
    pub const fn metadata(self) -> u64 {
        self.metadata
    }
    pub const fn virtual_timers(self) -> u64 {
        self.virtual_timers
    }
    pub const fn ready_queue(self) -> u64 {
        self.ready_queue
    }
    pub const fn instructions(self) -> u64 {
        self.instructions
    }
    pub const fn timeout_ns(self) -> Option<u64> {
        self.timeout_ns
    }
    pub const fn grace_ns(self) -> u64 {
        self.grace_ns
    }

    pub const fn with_work(mut self, value: u64) -> Self {
        self.work = value;
        self
    }
    pub const fn with_memory(mut self, value: u64) -> Self {
        self.memory = value;
        self
    }
    pub const fn with_depth(mut self, value: u64) -> Self {
        self.depth = value;
        self
    }
    pub const fn with_output(mut self, value: u64) -> Self {
        self.output = value;
        self
    }
    pub const fn with_artifact_bytes(mut self, value: u64) -> Self {
        self.artifact_bytes = value;
        self
    }
    pub const fn with_artifact_count(mut self, value: u64) -> Self {
        self.artifact_count = value;
        self
    }
    pub const fn with_snapshot_bytes(mut self, value: u64) -> Self {
        self.snapshot_bytes = value;
        self
    }
    pub const fn with_snapshot_count(mut self, value: u64) -> Self {
        self.snapshot_count = value;
        self
    }
    pub const fn with_metadata(mut self, value: u64) -> Self {
        self.metadata = value;
        self
    }
    pub const fn with_virtual_timers(mut self, value: u64) -> Self {
        self.virtual_timers = value;
        self
    }
    pub const fn with_ready_queue(mut self, value: u64) -> Self {
        self.ready_queue = value;
        self
    }
    pub const fn with_instructions(mut self, value: u64) -> Self {
        self.instructions = value;
        self
    }
    pub const fn with_timeout_ns(mut self, value: Option<u64>) -> Self {
        self.timeout_ns = value;
        self
    }
    pub const fn with_grace_ns(mut self, value: u64) -> Self {
        self.grace_ns = value;
        self
    }

    pub fn validate(self) -> Result<(), LimitError> {
        let finite = [
            (BudgetKind::Work, self.work),
            (BudgetKind::Memory, self.memory),
            (BudgetKind::Depth, self.depth),
            (BudgetKind::Output, self.output),
            (BudgetKind::ArtifactBytes, self.artifact_bytes),
            (BudgetKind::ArtifactCount, self.artifact_count),
            (BudgetKind::SnapshotBytes, self.snapshot_bytes),
            (BudgetKind::SnapshotCount, self.snapshot_count),
            (BudgetKind::Metadata, self.metadata),
            (BudgetKind::VirtualTimers, self.virtual_timers),
            (BudgetKind::ReadyQueue, self.ready_queue),
            (BudgetKind::Instructions, self.instructions),
            (BudgetKind::Grace, self.grace_ns),
        ];
        if let Some((kind, _)) = finite.into_iter().find(|(_, value)| *value == 0) {
            return Err(LimitError::ZeroLimit(kind));
        }
        if self.timeout_ns == Some(0) {
            return Err(LimitError::ZeroTimeout);
        }
        Ok(())
    }

    /// Canonical bytes used by reports to identify the effective resource
    /// profile. The shape is independent of host paths or map iteration.
    pub fn canonical_bytes(self) -> Vec<u8> {
        format!(
            "work={};memory={};depth={};output={};artifact_bytes={};artifact_count={};snapshot_bytes={};snapshot_count={};metadata={};virtual_timers={};ready_queue={};instructions={};timeout_ns={};grace_ns={}",
            self.work,
            self.memory,
            self.depth,
            self.output,
            self.artifact_bytes,
            self.artifact_count,
            self.snapshot_bytes,
            self.snapshot_count,
            self.metadata,
            self.virtual_timers,
            self.ready_queue,
            self.instructions,
            self.timeout_ns.map_or_else(|| "none".into(), |value| value.to_string()),
            self.grace_ns,
        )
        .into_bytes()
    }

    pub fn sha256(self) -> String {
        sha256(&self.canonical_bytes())
            .strip_prefix("sha256:")
            .unwrap_or_default()
            .to_owned()
    }

    pub fn envelope_limits(self) -> Result<EnvelopeLimits, LimitError> {
        self.validate()?;
        Ok(EnvelopeLimits::new(
            self.output,
            self.artifact_bytes,
            self.snapshot_bytes,
        ))
    }
}

/// One finite dimension charged by a test operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetKind {
    Work,
    Memory,
    Depth,
    Output,
    ArtifactBytes,
    ArtifactCount,
    SnapshotBytes,
    SnapshotCount,
    Metadata,
    VirtualTimers,
    ReadyQueue,
    Instructions,
    Grace,
}

impl BudgetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Memory => "memory",
            Self::Depth => "depth",
            Self::Output => "output",
            Self::ArtifactBytes => "artifact-bytes",
            Self::ArtifactCount => "artifact-count",
            Self::SnapshotBytes => "snapshot-bytes",
            Self::SnapshotCount => "snapshot-count",
            Self::Metadata => "metadata",
            Self::VirtualTimers => "virtual-timers",
            Self::ReadyQueue => "ready-queue",
            Self::Instructions => "instructions",
            Self::Grace => "grace",
        }
    }

    fn limit(self, profile: LimitProfile) -> u64 {
        match self {
            Self::Work => profile.work,
            Self::Memory => profile.memory,
            Self::Depth => profile.depth,
            Self::Output => profile.output,
            Self::ArtifactBytes => profile.artifact_bytes,
            Self::ArtifactCount => profile.artifact_count,
            Self::SnapshotBytes => profile.snapshot_bytes,
            Self::SnapshotCount => profile.snapshot_count,
            Self::Metadata => profile.metadata,
            Self::VirtualTimers => profile.virtual_timers,
            Self::ReadyQueue => profile.ready_queue,
            Self::Instructions => profile.instructions,
            Self::Grace => profile.grace_ns,
        }
    }

    const fn all() -> [Self; 13] {
        [
            Self::Work,
            Self::Memory,
            Self::Depth,
            Self::Output,
            Self::ArtifactBytes,
            Self::ArtifactCount,
            Self::SnapshotBytes,
            Self::SnapshotCount,
            Self::Metadata,
            Self::VirtualTimers,
            Self::ReadyQueue,
            Self::Instructions,
            Self::Grace,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitError {
    ZeroLimit(BudgetKind),
    ZeroTimeout,
    InvalidDelta(BudgetKind),
    Overflow(BudgetKind),
    Exhausted {
        kind: BudgetKind,
        requested: u64,
        remaining: u64,
    },
    ClockRegression {
        previous: u64,
        current: u64,
    },
    DeadlineOverflow,
}

impl fmt::Display for LimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit(kind) => write!(formatter, "{} budget must be positive", kind.as_str()),
            Self::ZeroTimeout => formatter.write_str("timeout must be positive or disabled"),
            Self::InvalidDelta(kind) => {
                write!(formatter, "{} reservation must be positive", kind.as_str())
            }
            Self::Overflow(kind) => write!(formatter, "{} reservation overflows", kind.as_str()),
            Self::Exhausted {
                kind,
                requested,
                remaining,
            } => write!(
                formatter,
                "{} budget exhausted: requested {requested}, remaining {remaining}",
                kind.as_str()
            ),
            Self::ClockRegression { previous, current } => {
                write!(
                    formatter,
                    "monotonic clock regressed from {previous} to {current}"
                )
            }
            Self::DeadlineOverflow => formatter.write_str("deadline arithmetic overflowed"),
        }
    }
}

impl Error for LimitError {}

/// A preflighted set of charges. `reserve` commits all dimensions together;
/// there is no partially published operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    charges: Vec<(BudgetKind, u64)>,
}

impl Reservation {
    pub fn charges(&self) -> &[(BudgetKind, u64)] {
        &self.charges
    }
}

#[derive(Debug, Clone)]
pub struct BudgetLedger {
    profile: LimitProfile,
    used: BTreeMap<BudgetKind, u64>,
}

impl BudgetLedger {
    pub fn new(profile: LimitProfile) -> Result<Self, LimitError> {
        profile.validate()?;
        Ok(Self {
            profile,
            used: BTreeMap::new(),
        })
    }

    pub const fn profile(&self) -> LimitProfile {
        self.profile
    }

    pub fn used(&self, kind: BudgetKind) -> u64 {
        self.used.get(&kind).copied().unwrap_or(0)
    }

    pub fn remaining(&self, kind: BudgetKind) -> u64 {
        kind.limit(self.profile).saturating_sub(self.used(kind))
    }

    /// Preflight every dimension, then commit every charge. Duplicate
    /// dimensions are folded before validation, so the operation is atomic
    /// even when deltas come from multiple subsystems.
    pub fn reserve(
        &mut self,
        deltas: impl IntoIterator<Item = (BudgetKind, u64)>,
    ) -> Result<Reservation, LimitError> {
        let mut folded = BTreeMap::<BudgetKind, u64>::new();
        for (kind, amount) in deltas {
            if amount == 0 {
                return Err(LimitError::InvalidDelta(kind));
            }
            let entry = folded.entry(kind).or_default();
            *entry = entry
                .checked_add(amount)
                .ok_or(LimitError::Overflow(kind))?;
        }
        for (&kind, &amount) in &folded {
            let remaining = self.remaining(kind);
            if amount > remaining {
                return Err(LimitError::Exhausted {
                    kind,
                    requested: amount,
                    remaining,
                });
            }
        }
        for (&kind, &amount) in &folded {
            *self.used.entry(kind).or_default() += amount;
        }
        Ok(Reservation {
            charges: folded.into_iter().collect(),
        })
    }

    pub fn effective_limits(&self) -> BTreeMap<&'static str, u64> {
        BudgetKind::all()
            .into_iter()
            .map(|kind| (kind.as_str(), kind.limit(self.profile)))
            .collect()
    }
}

/// A phase deadline that can pause while a suite waits for selected children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseDeadline {
    started_at: u64,
    timeout_ns: Option<u64>,
    paused_at: Option<u64>,
    paused_ns: u64,
    last_now: u64,
}

impl PhaseDeadline {
    pub fn new(profile: LimitProfile, now: u64) -> Result<Self, LimitError> {
        profile.validate()?;
        Ok(Self {
            started_at: now,
            timeout_ns: profile.timeout_ns,
            paused_at: None,
            paused_ns: 0,
            last_now: now,
        })
    }

    pub const fn timeout_ns(self) -> Option<u64> {
        self.timeout_ns
    }

    fn observe(&mut self, now: u64) -> Result<(), LimitError> {
        if now < self.last_now {
            return Err(LimitError::ClockRegression {
                previous: self.last_now,
                current: now,
            });
        }
        self.last_now = now;
        Ok(())
    }

    pub fn pause(&mut self, now: u64) -> Result<(), LimitError> {
        self.observe(now)?;
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
        Ok(())
    }

    pub fn resume(&mut self, now: u64) -> Result<(), LimitError> {
        self.observe(now)?;
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_ns = self
                .paused_ns
                .checked_add(now - paused_at)
                .ok_or(LimitError::DeadlineOverflow)?;
        }
        Ok(())
    }

    pub fn elapsed_ns(&mut self, now: u64) -> Result<u64, LimitError> {
        self.observe(now)?;
        let paused = self.paused_ns + self.paused_at.map_or(0, |paused_at| now - paused_at);
        Ok(now.saturating_sub(self.started_at).saturating_sub(paused))
    }

    pub fn expired(&mut self, now: u64) -> Result<bool, LimitError> {
        let Some(timeout) = self.timeout_ns else {
            return Ok(false);
        };
        Ok(self.elapsed_ns(now)? >= timeout)
    }

    pub fn remaining_ns(&mut self, now: u64) -> Result<Option<u64>, LimitError> {
        let Some(timeout) = self.timeout_ns else {
            return Ok(None);
        };
        Ok(Some(timeout.saturating_sub(self.elapsed_ns(now)?)))
    }
}

/// External cancellation state. A worker gets the grace interval after the
/// first request, then the runner must force isolation if it has not stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptState {
    Running,
    Cancelling { requested_at: u64 },
    Forced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptController {
    state: InterruptState,
    grace_ns: u64,
}

impl InterruptController {
    pub fn new(profile: LimitProfile) -> Result<Self, LimitError> {
        profile.validate()?;
        Ok(Self {
            state: InterruptState::Running,
            grace_ns: profile.grace_ns,
        })
    }

    pub const fn state(self) -> InterruptState {
        self.state
    }

    pub fn request(&mut self, now: u64) -> bool {
        if matches!(self.state, InterruptState::Running) {
            self.state = InterruptState::Cancelling { requested_at: now };
            true
        } else {
            false
        }
    }

    pub fn poll(&mut self, now: u64) -> Result<InterruptState, LimitError> {
        if let InterruptState::Cancelling { requested_at } = self.state {
            if now < requested_at {
                return Err(LimitError::ClockRegression {
                    previous: requested_at,
                    current: now,
                });
            }
            if now - requested_at >= self.grace_ns {
                self.state = InterruptState::Forced;
            }
        }
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite_and_profile_identity_is_stable() {
        let profile = LimitProfile::default();
        profile.validate().unwrap();
        assert!(profile.timeout_ns().is_some());
        assert_eq!(profile.sha256().len(), 64);
        assert_eq!(
            profile.canonical_bytes(),
            LimitProfile::default().canonical_bytes()
        );
        let limits = profile.envelope_limits().unwrap();
        assert_eq!(limits.output_bytes(), profile.output());
    }

    #[test]
    fn zero_structural_limits_and_zero_timeout_are_rejected() {
        assert!(matches!(
            LimitProfile::default().with_memory(0).validate(),
            Err(LimitError::ZeroLimit(BudgetKind::Memory))
        ));
        assert!(matches!(
            LimitProfile::default().with_timeout_ns(Some(0)).validate(),
            Err(LimitError::ZeroTimeout)
        ));
        assert!(
            LimitProfile::default()
                .with_timeout_ns(None)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn ledger_preflights_all_dimensions_without_partial_charge() {
        let profile = LimitProfile::default().with_output(5).with_memory(7);
        let mut ledger = BudgetLedger::new(profile).unwrap();
        assert!(matches!(
            ledger.reserve([(BudgetKind::Output, 4), (BudgetKind::Memory, 8)]),
            Err(LimitError::Exhausted {
                kind: BudgetKind::Memory,
                ..
            })
        ));
        assert_eq!(ledger.used(BudgetKind::Output), 0);
        assert_eq!(ledger.used(BudgetKind::Memory), 0);
        let reservation = ledger
            .reserve([(BudgetKind::Output, 2), (BudgetKind::Output, 1)])
            .unwrap();
        assert_eq!(reservation.charges(), &[(BudgetKind::Output, 3)]);
        assert_eq!(ledger.remaining(BudgetKind::Output), 2);
    }

    #[test]
    fn ledger_rejects_zero_and_overflow_deltas() {
        let mut ledger = BudgetLedger::new(LimitProfile::default()).unwrap();
        assert!(matches!(
            ledger.reserve([(BudgetKind::Work, 0)]),
            Err(LimitError::InvalidDelta(BudgetKind::Work))
        ));
        assert!(matches!(
            ledger.reserve([(BudgetKind::Work, u64::MAX), (BudgetKind::Work, 1)]),
            Err(LimitError::Overflow(BudgetKind::Work))
        ));
    }

    #[test]
    fn phase_deadline_pauses_for_descendants_and_times_out_after_resume() {
        let profile = LimitProfile::default().with_timeout_ns(Some(10));
        let mut deadline = PhaseDeadline::new(profile, 100).unwrap();
        assert!(!deadline.expired(104).unwrap());
        deadline.pause(104).unwrap();
        assert!(!deadline.expired(1_000).unwrap());
        deadline.resume(1_000).unwrap();
        assert_eq!(deadline.elapsed_ns(1_005).unwrap(), 9);
        assert!(!deadline.expired(1_005).unwrap());
        assert!(deadline.expired(1_006).unwrap());
    }

    #[test]
    fn phase_deadline_supports_disabled_timeout_and_rejects_clock_regression() {
        let mut disabled =
            PhaseDeadline::new(LimitProfile::default().with_timeout_ns(None), 9).unwrap();
        assert!(!disabled.expired(100_000).unwrap());
        assert!(matches!(
            disabled.elapsed_ns(8),
            Err(LimitError::ClockRegression { .. })
        ));
        let mut deadline = PhaseDeadline::new(LimitProfile::default(), 9).unwrap();
        assert!(matches!(
            deadline.pause(8),
            Err(LimitError::ClockRegression { .. })
        ));
    }

    #[test]
    fn interrupt_controller_grants_one_grace_period_then_forces() {
        let profile = LimitProfile::default().with_grace_ns(5);
        let mut controller = InterruptController::new(profile).unwrap();
        assert!(controller.request(10));
        assert!(!controller.request(11));
        assert_eq!(
            controller.poll(14).unwrap(),
            InterruptState::Cancelling { requested_at: 10 }
        );
        assert_eq!(controller.poll(15).unwrap(), InterruptState::Forced);
        assert!(matches!(controller.poll(14), Ok(InterruptState::Forced)));
    }

    #[test]
    fn interrupt_controller_rejects_clock_regression() {
        let mut controller = InterruptController::new(LimitProfile::default()).unwrap();
        controller.request(20);
        assert!(matches!(
            controller.poll(19),
            Err(LimitError::ClockRegression {
                previous: 20,
                current: 19
            })
        ));
    }

    #[test]
    fn effective_limits_publish_every_dimension() {
        let ledger = BudgetLedger::new(LimitProfile::default()).unwrap();
        let limits = ledger.effective_limits();
        assert_eq!(limits.len(), 13);
        assert_eq!(limits["output"], LimitProfile::default().output());
        assert_eq!(limits["grace"], LimitProfile::default().grace_ns());
    }
}
