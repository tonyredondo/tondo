//! Dynamic happens-before race analysis for the hosted diagnostic trace.
//!
//! The analyzer is deliberately separate from the VM collector. The collector
//! records bounded metadata while executing; this module consumes that trace
//! and reports only races observed on executed paths. A clean report is never
//! a static proof of race freedom.

use std::collections::BTreeMap;

use super::diagnostics::{
    DiagnosticEvent, DiagnosticMemoryAccess, DiagnosticRange, DiagnosticSource,
    DiagnosticSynchronization, DiagnosticTaskState, DiagnosticTrace,
};

pub const RACE_SCHEMA: &str = "tondo-diagnostic-race/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaceConfig {
    pub max_observations: u64,
    pub max_findings: u32,
}

impl Default for RaceConfig {
    fn default() -> Self {
        Self {
            max_observations: 100_000,
            max_findings: 100_000,
        }
    }
}

impl RaceConfig {
    fn valid(self) -> bool {
        self.max_observations > 0 && self.max_findings > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaceStatus {
    Clean,
    Finding,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RaceLimitation {
    InvalidConfiguration,
    TraceTruncated,
    ObservationLimit,
    FindingLimit,
    MissingTaskLifecycle { task_id: u64 },
    MissingSynchronizationContext { task_id: u64 },
    MissingSource { sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RaceLocation {
    Shared {
        storage_id: u64,
        path_hash: u64,
    },
    Local {
        task_id: u64,
        frame: u32,
        slot: u32,
        path_hash: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaceAccess {
    pub sequence: u64,
    pub task_id: u64,
    pub thread_id: u64,
    pub access: DiagnosticMemoryAccess,
    pub range: DiagnosticRange,
    pub source: DiagnosticSource,
    pub stack: Vec<DiagnosticSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaceFinding {
    pub location: RaceLocation,
    pub first: RaceAccess,
    pub second: RaceAccess,
    pub creation_stack: Vec<DiagnosticSource>,
    pub missing_happens_before: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaceReport {
    pub format: &'static str,
    pub profile: &'static str,
    pub status: RaceStatus,
    pub observations: u64,
    pub findings: Vec<RaceFinding>,
    pub limitations: Vec<RaceLimitation>,
    pub events_seen: u64,
    pub truncated: bool,
}

impl RaceReport {
    pub fn is_clean(&self) -> bool {
        self.status == RaceStatus::Clean
    }

    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn is_unsupported(&self) -> bool {
        self.status == RaceStatus::Unsupported
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Clock(BTreeMap<u64, u64>);

impl Clock {
    fn tick(&mut self, task_id: u64) {
        let entry = self.0.entry(task_id).or_default();
        *entry = entry.saturating_add(1);
    }

    fn merge(&mut self, other: &Self) {
        for (task_id, value) in &other.0 {
            let entry = self.0.entry(*task_id).or_default();
            *entry = (*entry).max(*value);
        }
    }

    fn happens_before(&self, other: &Self) -> bool {
        let strictly_less = self
            .0
            .iter()
            .any(|(task_id, value)| *value < other.0.get(task_id).copied().unwrap_or_default());
        self.0
            .iter()
            .all(|(task_id, value)| *value <= other.0.get(task_id).copied().unwrap_or_default())
            && strictly_less
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LocationKey {
    Shared {
        storage_id: u64,
        path_hash: u64,
    },
    Local {
        task_id: u64,
        frame: u32,
        slot: u32,
        path_hash: u64,
    },
}

impl LocationKey {
    fn from_range(range: &DiagnosticRange) -> Self {
        match range.storage_id {
            Some(storage_id) => Self::Shared {
                storage_id,
                path_hash: range.path_hash,
            },
            None => Self::Local {
                task_id: range.task_id,
                frame: range.frame,
                slot: range.slot,
                path_hash: range.path_hash,
            },
        }
    }

    fn public(&self) -> RaceLocation {
        match self {
            Self::Shared {
                storage_id,
                path_hash,
            } => RaceLocation::Shared {
                storage_id: *storage_id,
                path_hash: *path_hash,
            },
            Self::Local {
                task_id,
                frame,
                slot,
                path_hash,
            } => RaceLocation::Local {
                task_id: *task_id,
                frame: *frame,
                slot: *slot,
                path_hash: *path_hash,
            },
        }
    }
}

#[derive(Debug, Clone)]
struct PriorAccess {
    access: RaceAccess,
    clock: Clock,
}

#[derive(Debug, Clone, Default)]
struct TaskState {
    clock: Clock,
    created: bool,
    creation_stack: Vec<DiagnosticSource>,
}

/// Analyze one hosted runtime trace using vector clocks.
pub fn detect_races(trace: &DiagnosticTrace) -> RaceReport {
    detect_races_with_config(trace, RaceConfig::default())
}

/// Analyze one trace with explicit observation and finding budgets.
pub fn detect_races_with_config(trace: &DiagnosticTrace, config: RaceConfig) -> RaceReport {
    let mut analyzer = Analyzer {
        config,
        tasks: BTreeMap::new(),
        completed: BTreeMap::new(),
        pending_select: BTreeMap::new(),
        prior: BTreeMap::new(),
        findings: Vec::new(),
        limitations: Vec::new(),
        observations: 0,
        current_thread: 0,
    };
    if !config.valid() {
        analyzer
            .limitations
            .push(RaceLimitation::InvalidConfiguration);
        return analyzer.finish(trace);
    }
    if trace.truncated {
        analyzer.limitations.push(RaceLimitation::TraceTruncated);
    }
    for (index, event) in trace.events.iter().enumerate() {
        let sequence = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        analyzer.process(event, sequence, trace);
    }
    analyzer.finish(trace)
}

struct Analyzer {
    config: RaceConfig,
    tasks: BTreeMap<u64, TaskState>,
    completed: BTreeMap<u64, Clock>,
    pending_select: BTreeMap<u64, Vec<u64>>,
    prior: BTreeMap<LocationKey, Vec<PriorAccess>>,
    findings: Vec<RaceFinding>,
    limitations: Vec<RaceLimitation>,
    observations: u64,
    current_thread: u64,
}

impl Analyzer {
    fn task_mut(&mut self, task_id: u64) -> &mut TaskState {
        self.tasks.entry(task_id).or_default()
    }

    fn process(&mut self, event: &DiagnosticEvent, sequence: u64, trace: &DiagnosticTrace) {
        match event {
            DiagnosticEvent::Thread { id, state } => {
                if matches!(state, super::diagnostics::DiagnosticThreadState::Started) {
                    self.current_thread = *id;
                }
            }
            DiagnosticEvent::Task {
                id,
                parent,
                state,
                stack,
            } => {
                let parent_clock = parent.as_ref().and_then(|parent| {
                    self.tasks
                        .get(parent)
                        .map(|parent_state| parent_state.clock.clone())
                });
                if matches!(state, DiagnosticTaskState::Created)
                    && let Some(parent) = parent
                    && parent_clock.is_none()
                {
                    self.limitations
                        .push(RaceLimitation::MissingTaskLifecycle { task_id: *parent });
                }
                let task = self.task_mut(*id);
                if matches!(state, DiagnosticTaskState::Created) {
                    task.created = true;
                    task.creation_stack = stack.clone();
                    if let Some(parent_clock) = &parent_clock {
                        task.clock.merge(parent_clock);
                    }
                }
                task.clock.tick(*id);
                let completion_clock = task.clock.clone();
                if matches!(state, DiagnosticTaskState::Complete) {
                    self.completed.insert(*id, completion_clock);
                }
            }
            DiagnosticEvent::Memory {
                access,
                range,
                source,
                stack,
            } => self.memory(*access, range, source, stack, sequence, trace),
            DiagnosticEvent::Synchronization {
                task_id,
                operation,
                peer,
                ..
            } => self.synchronization(*task_id, *operation, *peer),
            _ => {}
        }
    }

    fn memory(
        &mut self,
        access: DiagnosticMemoryAccess,
        range: &DiagnosticRange,
        source: &DiagnosticSource,
        stack: &[DiagnosticSource],
        sequence: u64,
        trace: &DiagnosticTrace,
    ) {
        if self.observations >= self.config.max_observations {
            self.limitations.push(RaceLimitation::ObservationLimit);
            return;
        }
        self.observations = self.observations.saturating_add(1);
        if !self
            .tasks
            .get(&range.task_id)
            .is_some_and(|task| task.created)
        {
            self.limitations.push(RaceLimitation::MissingTaskLifecycle {
                task_id: range.task_id,
            });
        }
        let source_known = trace.source_maps.iter().any(|known| known == source);
        if !source_known {
            self.limitations
                .push(RaceLimitation::MissingSource { sequence });
        }
        let task = self.task_mut(range.task_id);
        task.clock.tick(range.task_id);
        let clock = task.clock.clone();
        let access_snapshot = RaceAccess {
            sequence,
            task_id: range.task_id,
            thread_id: self.current_thread,
            access,
            range: range.clone(),
            source: source.clone(),
            stack: stack.to_vec(),
        };
        let location = LocationKey::from_range(range);
        let previous = self.prior.entry(location.clone()).or_default();
        for prior in previous.iter() {
            if prior.access.task_id == access_snapshot.task_id
                || !conflicts(prior.access.access, access_snapshot.access)
                || prior.clock.happens_before(&clock)
                || clock.happens_before(&prior.clock)
            {
                continue;
            }
            if self.findings.len() >= self.config.max_findings as usize {
                self.limitations.push(RaceLimitation::FindingLimit);
                break;
            }
            let creation_stack = self
                .tasks
                .get(&access_snapshot.task_id)
                .filter(|task| !task.creation_stack.is_empty())
                .map(|task| task.creation_stack.clone())
                .or_else(|| {
                    self.tasks
                        .get(&prior.access.task_id)
                        .map(|task| task.creation_stack.clone())
                })
                .unwrap_or_default();
            self.findings.push(RaceFinding {
                location: location.public(),
                first: prior.access.clone(),
                second: access_snapshot.clone(),
                creation_stack,
                missing_happens_before: true,
            });
        }
        previous.push(PriorAccess {
            access: access_snapshot,
            clock,
        });
    }

    fn synchronization(
        &mut self,
        task_id: u64,
        operation: DiagnosticSynchronization,
        peer: Option<u64>,
    ) {
        match operation {
            DiagnosticSynchronization::Spawn => {
                let Some(parent) = peer else {
                    self.limitations
                        .push(RaceLimitation::MissingSynchronizationContext { task_id });
                    self.task_mut(task_id).clock.tick(task_id);
                    return;
                };
                let parent_clock = self.task_mut(parent).clock.clone();
                let child = self.task_mut(task_id);
                child.clock.merge(&parent_clock);
                child.clock.tick(task_id);
            }
            DiagnosticSynchronization::Join => {
                let clock = peer
                    .and_then(|peer| self.completed.get(&peer).cloned())
                    .or_else(|| {
                        peer.and_then(|peer| self.tasks.get(&peer).map(|task| task.clock.clone()))
                    });
                if let Some(clock) = clock {
                    let task = self.task_mut(task_id);
                    task.clock.merge(&clock);
                    task.clock.tick(task_id);
                } else if peer.is_some() {
                    self.limitations
                        .push(RaceLimitation::MissingSynchronizationContext { task_id });
                    self.task_mut(task_id).clock.tick(task_id);
                } else {
                    let task = self.task_mut(task_id);
                    task.clock.tick(task_id);
                    let completion_clock = task.clock.clone();
                    self.completed.insert(task_id, completion_clock);
                }
            }
            DiagnosticSynchronization::Wake => {
                let peer_clock = peer.and_then(|peer| {
                    self.tasks
                        .get(&peer)
                        .map(|peer_state| peer_state.clock.clone())
                });
                if peer.is_some() && peer_clock.is_none() {
                    self.limitations
                        .push(RaceLimitation::MissingSynchronizationContext { task_id });
                }
                let task = self.task_mut(task_id);
                if let Some(peer_clock) = peer_clock {
                    task.clock.merge(&peer_clock);
                }
                task.clock.tick(task_id);
            }
            DiagnosticSynchronization::SelectRegister => {
                if let Some(peer) = peer {
                    self.pending_select.entry(task_id).or_default().push(peer);
                }
                self.task_mut(task_id).clock.tick(task_id);
            }
            DiagnosticSynchronization::SelectCommit => {
                let peers = self.pending_select.remove(&task_id).unwrap_or_default();
                let clocks = peers
                    .iter()
                    .filter_map(|peer| self.tasks.get(peer).map(|task| task.clock.clone()))
                    .collect::<Vec<_>>();
                let task = self.task_mut(task_id);
                for clock in clocks {
                    task.clock.merge(&clock);
                }
                task.clock.tick(task_id);
            }
            DiagnosticSynchronization::Park
            | DiagnosticSynchronization::HostStart
            | DiagnosticSynchronization::HostComplete
            | DiagnosticSynchronization::HostCancel
            | DiagnosticSynchronization::LoanReserve
            | DiagnosticSynchronization::LoanRelease => {
                let peer_clock = peer.and_then(|peer| {
                    self.tasks
                        .get(&peer)
                        .map(|peer_state| peer_state.clock.clone())
                });
                let task = self.task_mut(task_id);
                if let Some(peer_clock) = peer_clock {
                    task.clock.merge(&peer_clock);
                }
                task.clock.tick(task_id);
            }
        }
    }

    fn finish(mut self, trace: &DiagnosticTrace) -> RaceReport {
        self.limitations.sort();
        self.limitations.dedup();
        let status = if !self.limitations.is_empty() {
            RaceStatus::Unsupported
        } else if self.findings.is_empty() {
            RaceStatus::Clean
        } else {
            RaceStatus::Finding
        };
        RaceReport {
            format: RACE_SCHEMA,
            profile: "race",
            status,
            observations: self.observations,
            findings: self.findings,
            limitations: self.limitations,
            events_seen: trace.events_seen,
            truncated: trace.truncated,
        }
    }
}

fn conflicts(left: DiagnosticMemoryAccess, right: DiagnosticMemoryAccess) -> bool {
    !(left == DiagnosticMemoryAccess::Read && right == DiagnosticMemoryAccess::Read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::BytecodeSpan;
    use crate::runtime::diagnostics::{DiagnosticConfig, DiagnosticThreadState};

    fn source(function: &str, start: u32) -> DiagnosticSource {
        DiagnosticSource {
            function: function.into(),
            span: BytecodeSpan {
                file: 1,
                start,
                end: start + 1,
            },
        }
    }

    fn range(task_id: u64, storage_id: Option<u64>) -> DiagnosticRange {
        DiagnosticRange {
            task_id,
            frame: 0,
            slot: 0,
            projections: 0,
            storage_id,
            path_hash: 0,
        }
    }

    fn trace(events: Vec<DiagnosticEvent>, source: DiagnosticSource) -> DiagnosticTrace {
        DiagnosticTrace {
            format: super::super::diagnostics::DIAGNOSTIC_SCHEMA,
            config: DiagnosticConfig::default(),
            events,
            scheduler_tail: Vec::new(),
            roots: Vec::new(),
            resources: Vec::new(),
            source_maps: vec![source],
            events_seen: 0,
            truncated: false,
        }
    }

    fn memory(
        task_id: u64,
        storage_id: Option<u64>,
        access: DiagnosticMemoryAccess,
        source: &DiagnosticSource,
    ) -> DiagnosticEvent {
        DiagnosticEvent::Memory {
            access,
            range: range(task_id, storage_id),
            source: source.clone(),
            stack: vec![source.clone()],
        }
    }

    fn task(
        id: u64,
        parent: Option<u64>,
        state: DiagnosticTaskState,
        source: &DiagnosticSource,
    ) -> DiagnosticEvent {
        DiagnosticEvent::Task {
            id,
            parent,
            state,
            stack: vec![source.clone()],
        }
    }

    #[test]
    fn defaults_and_invalid_limits_are_explicit() {
        assert_eq!(RaceConfig::default().max_observations, 100_000);
        assert_eq!(RaceConfig::default().max_findings, 100_000);
        let initial_source = source("main", 0);
        let report = detect_races_with_config(
            &trace(vec![], initial_source),
            RaceConfig {
                max_observations: 0,
                ..RaceConfig::default()
            },
        );
        assert!(report.is_unsupported());
        assert_eq!(report.limitations, [RaceLimitation::InvalidConfiguration]);
        let second_source = source("main", 1);
        let report = detect_races_with_config(
            &trace(vec![], second_source),
            RaceConfig {
                max_findings: 0,
                ..RaceConfig::default()
            },
        );
        assert!(report.is_unsupported());
    }

    #[test]
    fn read_read_and_same_task_accesses_are_clean() {
        let source = source("main", 0);
        let events = vec![
            DiagnosticEvent::Thread {
                id: 0,
                state: DiagnosticThreadState::Started,
            },
            task(1, None, DiagnosticTaskState::Created, &source),
            memory(1, Some(7), DiagnosticMemoryAccess::Read, &source),
            memory(1, Some(7), DiagnosticMemoryAccess::Write, &source),
        ];
        let report = detect_races(&trace(events, source));
        assert!(report.is_clean());
        assert_eq!(report.observations, 2);
    }

    #[test]
    fn different_tasks_conflicting_shared_accesses_report_stacks() {
        let source = source("main", 10);
        let events = vec![
            task(1, None, DiagnosticTaskState::Created, &source),
            task(2, Some(1), DiagnosticTaskState::Created, &source),
            DiagnosticEvent::Synchronization {
                task_id: 2,
                operation: DiagnosticSynchronization::Spawn,
                peer: Some(1),
                source: Some(source.clone()),
            },
            memory(1, Some(9), DiagnosticMemoryAccess::Write, &source),
            memory(2, Some(9), DiagnosticMemoryAccess::Read, &source),
        ];
        let report = detect_races(&trace(events, source));
        assert_eq!(report.status, RaceStatus::Finding);
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].missing_happens_before);
        assert!(!report.findings[0].first.stack.is_empty());
        assert!(!report.findings[0].creation_stack.is_empty());
        assert!(report.has_findings());
    }

    #[test]
    fn join_and_wake_supply_happens_before_edges() {
        let source = source("main", 20);
        let events = vec![
            task(1, None, DiagnosticTaskState::Created, &source),
            task(2, Some(1), DiagnosticTaskState::Created, &source),
            DiagnosticEvent::Synchronization {
                task_id: 2,
                operation: DiagnosticSynchronization::Spawn,
                peer: Some(1),
                source: None,
            },
            memory(2, Some(4), DiagnosticMemoryAccess::Write, &source),
            task(2, Some(1), DiagnosticTaskState::Complete, &source),
            DiagnosticEvent::Synchronization {
                task_id: 1,
                operation: DiagnosticSynchronization::Join,
                peer: Some(2),
                source: None,
            },
            memory(1, Some(4), DiagnosticMemoryAccess::Read, &source),
        ];
        let report = detect_races(&trace(events, source));
        assert!(report.is_clean());
    }

    #[test]
    fn task_local_storage_does_not_alias_across_tasks() {
        let source = source("main", 30);
        let events = vec![
            task(1, None, DiagnosticTaskState::Created, &source),
            task(2, Some(1), DiagnosticTaskState::Created, &source),
            memory(1, None, DiagnosticMemoryAccess::Write, &source),
            memory(2, None, DiagnosticMemoryAccess::Read, &source),
        ];
        let report = detect_races(&trace(events, source));
        assert!(report.is_clean());
    }

    #[test]
    fn truncation_and_missing_context_fail_closed() {
        let source = source("main", 40);
        let mut trace = trace(
            vec![memory(99, Some(3), DiagnosticMemoryAccess::Write, &source)],
            source,
        );
        trace.truncated = true;
        let report = detect_races(&trace);
        assert!(report.is_unsupported());
        assert!(report.limitations.contains(&RaceLimitation::TraceTruncated));
        assert!(
            report
                .limitations
                .contains(&RaceLimitation::MissingTaskLifecycle { task_id: 99 })
        );
    }

    #[test]
    fn observation_budget_is_not_silent() {
        let source = source("main", 50);
        let events = vec![
            task(1, None, DiagnosticTaskState::Created, &source),
            memory(1, Some(1), DiagnosticMemoryAccess::Read, &source),
            memory(1, Some(1), DiagnosticMemoryAccess::Read, &source),
        ];
        let report = detect_races_with_config(
            &trace(events, source),
            RaceConfig {
                max_observations: 1,
                ..RaceConfig::default()
            },
        );
        assert!(report.is_unsupported());
        assert!(
            report
                .limitations
                .contains(&RaceLimitation::ObservationLimit)
        );
    }

    #[test]
    fn synchronization_variants_and_missing_context_are_explicit() {
        let sync_source = source("sync", 60);
        let foreign = source("foreign", 61);
        let events = vec![
            DiagnosticEvent::Thread {
                id: 7,
                state: DiagnosticThreadState::Started,
            },
            DiagnosticEvent::Thread {
                id: 8,
                state: DiagnosticThreadState::Stopped,
            },
            task(1, None, DiagnosticTaskState::Created, &sync_source),
            task(2, Some(999), DiagnosticTaskState::Created, &sync_source),
            task(3, Some(1), DiagnosticTaskState::Created, &sync_source),
            DiagnosticEvent::Synchronization {
                task_id: 4,
                operation: DiagnosticSynchronization::Spawn,
                peer: None,
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Spawn,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Park,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Wake,
                peer: Some(999),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::HostStart,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::HostComplete,
                peer: None,
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::HostCancel,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::LoanReserve,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::LoanRelease,
                peer: None,
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::SelectRegister,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::SelectRegister,
                peer: None,
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::SelectCommit,
                peer: None,
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Join,
                peer: Some(999),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Join,
                peer: Some(1),
                source: None,
            },
            DiagnosticEvent::Synchronization {
                task_id: 3,
                operation: DiagnosticSynchronization::Join,
                peer: None,
                source: None,
            },
            memory(3, Some(5), DiagnosticMemoryAccess::Move, &foreign),
        ];
        let report = detect_races(&DiagnosticTrace {
            source_maps: vec![sync_source.clone()],
            ..trace(events, sync_source)
        });
        assert!(report.is_unsupported());
        assert!(
            report
                .limitations
                .contains(&RaceLimitation::MissingTaskLifecycle { task_id: 999 })
        );
        assert!(
            report
                .limitations
                .contains(&RaceLimitation::MissingSynchronizationContext { task_id: 3 })
        );
        assert!(
            report
                .limitations
                .contains(&RaceLimitation::MissingSource { sequence: 21 })
        );
        assert_eq!(report.events_seen, 0);
    }

    #[test]
    fn finding_budget_and_location_shapes_are_closed() {
        let source = source("budget", 70);
        let events = vec![
            task(1, None, DiagnosticTaskState::Created, &source),
            task(2, None, DiagnosticTaskState::Created, &source),
            task(3, None, DiagnosticTaskState::Created, &source),
            memory(1, Some(8), DiagnosticMemoryAccess::Write, &source),
            memory(2, Some(8), DiagnosticMemoryAccess::Read, &source),
            memory(3, Some(8), DiagnosticMemoryAccess::Move, &source),
        ];
        let report = detect_races_with_config(
            &trace(events, source.clone()),
            RaceConfig {
                max_findings: 1,
                ..RaceConfig::default()
            },
        );
        assert!(report.is_unsupported());
        assert!(report.has_findings());
        assert!(report.limitations.contains(&RaceLimitation::FindingLimit));

        let mut local = range(4, None);
        local.frame = 2;
        local.slot = 3;
        local.path_hash = 9;
        assert_eq!(
            LocationKey::from_range(&local).public(),
            RaceLocation::Local {
                task_id: 4,
                frame: 2,
                slot: 3,
                path_hash: 9,
            }
        );
        assert!(conflicts(
            DiagnosticMemoryAccess::Move,
            DiagnosticMemoryAccess::Read
        ));
        assert!(!conflicts(
            DiagnosticMemoryAccess::Read,
            DiagnosticMemoryAccess::Read
        ));
    }
}
