//! Bounded, runtime-private observations used by the dynamic diagnostic tools.
//!
//! This module deliberately does not expose a Tondo or standard-library API.
//! The collector is opt-in at the Rust VM boundary and keeps only metadata:
//! logical ranges, task identities, source spans, object identities and
//! resource states. User payloads never enter the trace.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::bytecode::BytecodeSpan;

use super::VmError;

pub const DIAGNOSTIC_SCHEMA: &str = "tondo-diagnostic-runtime/1";

/// Runtime limits from `testing/diagnostic-tooling.json` that apply to the VM
/// collector. The report and dump byte budgets are enforced by their writers,
/// not by this in-memory event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticConfig {
    pub max_events: u64,
    pub max_stack_depth: u32,
    pub max_retainers_per_object: u32,
    pub max_scheduler_tail_events: u32,
}

impl Default for DiagnosticConfig {
    fn default() -> Self {
        Self {
            max_events: 1_000_000,
            max_stack_depth: 256,
            max_retainers_per_object: 256,
            max_scheduler_tail_events: 4_096,
        }
    }
}

impl DiagnosticConfig {
    pub fn validate(self) -> Result<Self, VmError> {
        if self.max_events == 0 {
            return Err(VmError::InvalidLimits(
                "diagnostic max_events must be positive",
            ));
        }
        if self.max_stack_depth == 0 {
            return Err(VmError::InvalidLimits(
                "diagnostic max_stack_depth must be positive",
            ));
        }
        if self.max_retainers_per_object == 0 {
            return Err(VmError::InvalidLimits(
                "diagnostic max_retainers_per_object must be positive",
            ));
        }
        if self.max_scheduler_tail_events == 0 {
            return Err(VmError::InvalidLimits(
                "diagnostic max_scheduler_tail_events must be positive",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticTaskState {
    Created,
    Runnable,
    Running,
    Waiting,
    CancelRequested,
    Complete,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticThreadState {
    Started,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticMemoryAccess {
    Read,
    Write,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticSynchronization {
    Spawn,
    Park,
    Wake,
    Join,
    HostStart,
    HostComplete,
    HostCancel,
    LoanReserve,
    LoanRelease,
    SelectRegister,
    SelectCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticHeapOperation {
    Allocate,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticResourceState {
    Acquired,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticSchedulerOperation {
    Enqueue,
    Switch,
    Park,
    Wake,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticQuiescencePhase {
    Begin,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticSource {
    pub function: String,
    pub span: BytecodeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticRange {
    pub task_id: u64,
    pub frame: u32,
    pub slot: u32,
    pub projections: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticRetainer {
    pub object_id: u64,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticEvent {
    Thread {
        id: u64,
        state: DiagnosticThreadState,
    },
    Task {
        id: u64,
        parent: Option<u64>,
        state: DiagnosticTaskState,
    },
    Memory {
        access: DiagnosticMemoryAccess,
        range: DiagnosticRange,
        source: DiagnosticSource,
    },
    Synchronization {
        task_id: u64,
        operation: DiagnosticSynchronization,
        peer: Option<u64>,
        source: Option<DiagnosticSource>,
    },
    Heap {
        object_id: u64,
        operation: DiagnosticHeapOperation,
        bytes: u64,
        owner_task: u64,
    },
    Roots {
        task_id: u64,
        object_ids: Vec<u64>,
        retainers: Vec<DiagnosticRetainer>,
    },
    Resource {
        id: u64,
        kind: String,
        state: DiagnosticResourceState,
        owner_task: u64,
    },
    Scheduler {
        task_id: u64,
        operation: DiagnosticSchedulerOperation,
        queue_len: u32,
    },
    Quiescence {
        task_id: u64,
        phase: DiagnosticQuiescencePhase,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticResource {
    pub id: u64,
    pub kind: String,
    pub owner_task: u64,
    pub state: DiagnosticResourceState,
    pub first_event: u64,
    pub last_event: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticRootSnapshot {
    pub task_id: u64,
    pub object_ids: Vec<u64>,
    pub retainers: Vec<DiagnosticRetainer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticTrace {
    pub format: &'static str,
    pub config: DiagnosticConfig,
    pub events: Vec<DiagnosticEvent>,
    pub scheduler_tail: Vec<DiagnosticEvent>,
    pub roots: Vec<DiagnosticRootSnapshot>,
    pub resources: Vec<DiagnosticResource>,
    pub source_maps: Vec<DiagnosticSource>,
    pub events_seen: u64,
    pub truncated: bool,
}

impl DiagnosticTrace {
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn task_ids(&self) -> Vec<u64> {
        let mut ids = self
            .events
            .iter()
            .filter_map(|event| match event {
                DiagnosticEvent::Task { id, .. } => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// Mutable collector kept behind `Option` in the VM. When diagnostics are not
/// requested, the hot path does not even construct this value.
#[derive(Debug)]
pub(super) struct DiagnosticSession {
    config: DiagnosticConfig,
    events: Vec<DiagnosticEvent>,
    scheduler_tail: VecDeque<DiagnosticEvent>,
    roots: Vec<DiagnosticRootSnapshot>,
    resources: BTreeMap<(String, u64), DiagnosticResource>,
    source_maps: Vec<DiagnosticSource>,
    source_keys: BTreeSet<(String, BytecodeSpan)>,
    events_seen: u64,
    truncated: bool,
}

impl DiagnosticSession {
    pub(super) fn new(config: DiagnosticConfig) -> Result<Self, VmError> {
        let config = config.validate()?;
        Ok(Self {
            config,
            events: Vec::new(),
            scheduler_tail: VecDeque::new(),
            roots: Vec::new(),
            resources: BTreeMap::new(),
            source_maps: Vec::new(),
            source_keys: BTreeSet::new(),
            events_seen: 0,
            truncated: false,
        })
    }

    fn emit(&mut self, event: DiagnosticEvent) -> Result<(), VmError> {
        if self.events_seen >= self.config.max_events {
            self.truncated = true;
            return Err(VmError::ResourceLimit {
                resource: "diagnostic events",
                limit: self.config.max_events,
            });
        }
        self.events_seen += 1;
        if let DiagnosticEvent::Scheduler { .. } = event {
            self.scheduler_tail.push_back(event.clone());
            while self.scheduler_tail.len() > self.config.max_scheduler_tail_events as usize {
                self.scheduler_tail.pop_front();
                self.truncated = true;
            }
        }
        self.events.push(event);
        Ok(())
    }

    pub(super) fn thread(&mut self, id: u64, state: DiagnosticThreadState) -> Result<(), VmError> {
        self.emit(DiagnosticEvent::Thread { id, state })
    }

    pub(super) fn task(
        &mut self,
        id: u64,
        parent: Option<u64>,
        state: DiagnosticTaskState,
    ) -> Result<(), VmError> {
        self.emit(DiagnosticEvent::Task { id, parent, state })
    }

    pub(super) fn memory(
        &mut self,
        access: DiagnosticMemoryAccess,
        range: DiagnosticRange,
        source: DiagnosticSource,
    ) -> Result<(), VmError> {
        if self
            .source_keys
            .insert((source.function.clone(), source.span))
        {
            self.source_maps.push(source.clone());
        }
        self.emit(DiagnosticEvent::Memory {
            access,
            range,
            source,
        })
    }

    pub(super) fn synchronization(
        &mut self,
        task_id: u64,
        operation: DiagnosticSynchronization,
        peer: Option<u64>,
        source: Option<DiagnosticSource>,
    ) -> Result<(), VmError> {
        if let Some(source) = &source
            && self
                .source_keys
                .insert((source.function.clone(), source.span))
        {
            self.source_maps.push(source.clone());
        }
        self.emit(DiagnosticEvent::Synchronization {
            task_id,
            operation,
            peer,
            source,
        })
    }

    pub(super) fn heap(
        &mut self,
        object_id: u64,
        operation: DiagnosticHeapOperation,
        bytes: u64,
        owner_task: u64,
    ) -> Result<(), VmError> {
        self.emit(DiagnosticEvent::Heap {
            object_id,
            operation,
            bytes,
            owner_task,
        })
    }

    pub(super) fn roots(
        &mut self,
        task_id: u64,
        objects: impl IntoIterator<Item = (u64, String)>,
    ) -> Result<(), VmError> {
        let mut object_ids = Vec::new();
        let mut retainers = Vec::new();
        for (object_id, owner) in objects {
            object_ids.push(object_id);
            if retainers.len() < self.config.max_retainers_per_object as usize {
                retainers.push(DiagnosticRetainer { object_id, owner });
            } else {
                self.truncated = true;
            }
        }
        object_ids.sort_unstable();
        object_ids.dedup();
        let snapshot = DiagnosticRootSnapshot {
            task_id,
            object_ids: object_ids.clone(),
            retainers: retainers.clone(),
        };
        self.roots.push(snapshot);
        self.emit(DiagnosticEvent::Roots {
            task_id,
            object_ids,
            retainers,
        })
    }

    pub(super) fn resource(
        &mut self,
        id: u64,
        kind: impl Into<String>,
        state: DiagnosticResourceState,
        owner_task: u64,
    ) -> Result<(), VmError> {
        let kind = kind.into();
        self.emit(DiagnosticEvent::Resource {
            id,
            kind: kind.clone(),
            state,
            owner_task,
        })?;
        let event = self.events_seen;
        let entry =
            self.resources
                .entry((kind.clone(), id))
                .or_insert_with(|| DiagnosticResource {
                    id,
                    kind,
                    owner_task,
                    state,
                    first_event: event,
                    last_event: event,
                });
        entry.state = state;
        entry.owner_task = owner_task;
        entry.last_event = event;
        Ok(())
    }

    pub(super) fn scheduler(
        &mut self,
        task_id: u64,
        operation: DiagnosticSchedulerOperation,
        queue_len: usize,
    ) -> Result<(), VmError> {
        self.emit(DiagnosticEvent::Scheduler {
            task_id,
            operation,
            queue_len: u32::try_from(queue_len).unwrap_or(u32::MAX),
        })
    }

    pub(super) fn quiescence(
        &mut self,
        task_id: u64,
        phase: DiagnosticQuiescencePhase,
    ) -> Result<(), VmError> {
        self.emit(DiagnosticEvent::Quiescence { task_id, phase })
    }

    pub(super) fn finish(self) -> DiagnosticTrace {
        DiagnosticTrace {
            format: DIAGNOSTIC_SCHEMA,
            config: self.config,
            events: self.events,
            scheduler_tail: self.scheduler_tail.into_iter().collect(),
            roots: self.roots,
            resources: self.resources.into_values().collect(),
            source_maps: self.source_maps,
            events_seen: self.events_seen,
            truncated: self.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DiagnosticSource {
        DiagnosticSource {
            function: "main".into(),
            span: BytecodeSpan {
                file: 2,
                start: 10,
                end: 12,
            },
        }
    }

    #[test]
    fn defaults_match_the_d0_runtime_budgets() {
        let config = DiagnosticConfig::default();
        assert_eq!(config.max_events, 1_000_000);
        assert_eq!(config.max_stack_depth, 256);
        assert_eq!(config.max_retainers_per_object, 256);
        assert_eq!(config.max_scheduler_tail_events, 4_096);
    }

    #[test]
    fn invalid_zero_budgets_are_rejected_before_execution() {
        for config in [
            DiagnosticConfig {
                max_events: 0,
                ..DiagnosticConfig::default()
            },
            DiagnosticConfig {
                max_stack_depth: 0,
                ..DiagnosticConfig::default()
            },
            DiagnosticConfig {
                max_retainers_per_object: 0,
                ..DiagnosticConfig::default()
            },
            DiagnosticConfig {
                max_scheduler_tail_events: 0,
                ..DiagnosticConfig::default()
            },
        ] {
            assert!(DiagnosticSession::new(config).is_err());
        }
    }

    #[test]
    fn event_budget_fails_closed_and_records_truncation() {
        let mut session = DiagnosticSession::new(DiagnosticConfig {
            max_events: 1,
            ..DiagnosticConfig::default()
        })
        .unwrap();
        session.thread(0, DiagnosticThreadState::Started).unwrap();
        let error = session
            .thread(0, DiagnosticThreadState::Stopped)
            .unwrap_err();
        assert!(matches!(
            error,
            VmError::ResourceLimit {
                resource: "diagnostic events",
                limit: 1
            }
        ));
        assert!(session.truncated);
        assert_eq!(session.events_seen, 1);
    }

    #[test]
    fn scheduler_tail_is_bounded_without_losing_the_main_event_stream() {
        let mut session = DiagnosticSession::new(DiagnosticConfig {
            max_scheduler_tail_events: 2,
            ..DiagnosticConfig::default()
        })
        .unwrap();
        for task_id in 1..=3 {
            session
                .scheduler(
                    task_id,
                    DiagnosticSchedulerOperation::Switch,
                    task_id as usize,
                )
                .unwrap();
        }
        let trace = session.finish();
        assert_eq!(trace.events.len(), 3);
        assert_eq!(trace.scheduler_tail.len(), 2);
        assert!(trace.truncated);
        assert!(matches!(
            trace.scheduler_tail[0],
            DiagnosticEvent::Scheduler { task_id: 2, .. }
        ));
    }

    #[test]
    fn roots_retain_deterministic_bounded_identity_metadata() {
        let mut session = DiagnosticSession::new(DiagnosticConfig {
            max_retainers_per_object: 1,
            ..DiagnosticConfig::default()
        })
        .unwrap();
        session
            .roots(4, [(9, "task:4".to_owned()), (2, "temporary".to_owned())])
            .unwrap();
        let trace = session.finish();
        assert_eq!(trace.roots[0].object_ids, [2, 9]);
        assert_eq!(trace.roots[0].retainers.len(), 1);
        assert!(trace.truncated);
    }

    #[test]
    fn source_maps_are_deduplicated_and_resources_keep_terminal_state() {
        let mut session = DiagnosticSession::new(DiagnosticConfig::default()).unwrap();
        let source = source();
        let range = DiagnosticRange {
            task_id: 1,
            frame: 0,
            slot: 2,
            projections: 1,
        };
        session
            .memory(DiagnosticMemoryAccess::Read, range.clone(), source.clone())
            .unwrap();
        session
            .memory(DiagnosticMemoryAccess::Write, range, source)
            .unwrap();
        session
            .resource(7, "File", DiagnosticResourceState::Acquired, 1)
            .unwrap();
        session
            .resource(7, "File", DiagnosticResourceState::Released, 1)
            .unwrap();
        let trace = session.finish();
        assert_eq!(trace.source_maps.len(), 1);
        assert_eq!(trace.resources.len(), 1);
        assert_eq!(trace.resources[0].state, DiagnosticResourceState::Released);
        assert_eq!(trace.resources[0].first_event, 3);
        assert_eq!(trace.resources[0].last_event, 4);
    }
}
