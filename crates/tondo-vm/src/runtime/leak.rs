//! Dynamic retention and resource-leak analysis for the hosted diagnostic trace.
//!
//! A tracing GC makes unreachable cycles an ordinary collection outcome, not a
//! leak.  This analyzer therefore requires quiescent root snapshots and only
//! reports managed retention when an object remains visible while the retained
//! set grows.  Affine resources are evaluated independently from the heap
//! snapshots: an acquired resource without a terminal release is actionable
//! even when the managed heap is clean.

use std::collections::{BTreeMap, BTreeSet};

use super::diagnostics::{
    DiagnosticEvent, DiagnosticHeapOperation, DiagnosticResourceState, DiagnosticRetainer,
    DiagnosticRootSnapshot, DiagnosticSource, DiagnosticTrace,
};

pub const LEAK_SCHEMA: &str = "tondo-diagnostic-leak/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeakConfig {
    pub max_observations: u64,
    pub max_findings: u32,
    /// Minimum number of completed quiescent snapshots required before a
    /// monotonic retained-set increase can be classified as sustained growth.
    pub min_growth_snapshots: u32,
}

impl Default for LeakConfig {
    fn default() -> Self {
        Self {
            max_observations: 100_000,
            max_findings: 100_000,
            min_growth_snapshots: 3,
        }
    }
}

impl LeakConfig {
    fn valid(self) -> bool {
        self.max_observations > 0 && self.max_findings > 0 && self.min_growth_snapshots >= 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakStatus {
    Clean,
    Finding,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LeakKind {
    ManagedRetention,
    AffineResource,
    NativeAllocation,
    SustainedGrowth,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LeakLimitation {
    InvalidConfiguration,
    TraceTruncated,
    ObservationLimit,
    FindingLimit,
    MissingQuiescence,
    MissingRootSnapshot { sequence: u64 },
    MissingAllocation { object_id: u64, sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakObject {
    pub object_id: u64,
    pub bytes: u64,
    pub owner_task: u64,
    pub first_event: u64,
    pub last_event: u64,
    pub first_view: u64,
    pub last_view: u64,
    pub retainers: Vec<DiagnosticRetainer>,
    pub allocation_source: Option<DiagnosticSource>,
    pub allocation_stack: Vec<DiagnosticSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakResource {
    pub id: u64,
    pub kind: String,
    pub owner_task: u64,
    pub state: DiagnosticResourceState,
    pub first_event: u64,
    pub last_event: u64,
    pub acquisition_source: Option<DiagnosticSource>,
    pub acquisition_stack: Vec<DiagnosticSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakSnapshot {
    pub sequence: u64,
    pub roots: Vec<DiagnosticRootSnapshot>,
    pub object_ids: Vec<u64>,
    pub retained_objects: u64,
    pub retained_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakFinding {
    pub kind: LeakKind,
    pub object: Option<LeakObject>,
    pub resource: Option<LeakResource>,
    pub growth: Vec<LeakSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakReport {
    pub format: &'static str,
    pub profile: &'static str,
    pub status: LeakStatus,
    pub observations: u64,
    pub snapshots: Vec<LeakSnapshot>,
    pub resources: Vec<LeakResource>,
    pub findings: Vec<LeakFinding>,
    pub limitations: Vec<LeakLimitation>,
    pub events_seen: u64,
    pub truncated: bool,
}

impl LeakReport {
    pub fn is_clean(&self) -> bool {
        self.status == LeakStatus::Clean
    }

    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn is_unsupported(&self) -> bool {
        self.status == LeakStatus::Unsupported
    }
}

#[derive(Debug, Clone)]
struct ObjectObservation {
    object_id: u64,
    bytes: u64,
    owner_task: u64,
    first_event: u64,
    last_event: u64,
    allocation_source: Option<DiagnosticSource>,
    allocation_stack: Vec<DiagnosticSource>,
    views: BTreeSet<usize>,
    retainers: BTreeMap<usize, Vec<DiagnosticRetainer>>,
}

#[derive(Debug, Clone)]
struct ResourceObservation {
    id: u64,
    kind: String,
    owner_task: u64,
    state: DiagnosticResourceState,
    first_event: u64,
    last_event: u64,
    acquisition_source: Option<DiagnosticSource>,
    acquisition_stack: Vec<DiagnosticSource>,
}

#[derive(Debug)]
struct ResourceObservationInput {
    id: u64,
    kind: String,
    state: DiagnosticResourceState,
    owner_task: u64,
    sequence: u64,
    source: Option<DiagnosticSource>,
    stack: Vec<DiagnosticSource>,
}

#[derive(Debug, Clone)]
struct PendingRoots {
    sequence: u64,
    snapshot: DiagnosticRootSnapshot,
}

#[derive(Debug)]
struct Analyzer {
    config: LeakConfig,
    objects: BTreeMap<u64, ObjectObservation>,
    resources: BTreeMap<(String, u64), ResourceObservation>,
    snapshots: Vec<LeakSnapshot>,
    pending_roots: Option<PendingRoots>,
    quiescence_open: bool,
    findings: Vec<LeakFinding>,
    limitations: Vec<LeakLimitation>,
    observations: u64,
}

/// Analyze one hosted runtime trace with the default leak budgets.
pub fn detect_leaks(trace: &DiagnosticTrace) -> LeakReport {
    detect_leaks_with_config(trace, LeakConfig::default())
}

/// Analyze one hosted runtime trace with explicit observation budgets.
pub fn detect_leaks_with_config(trace: &DiagnosticTrace, config: LeakConfig) -> LeakReport {
    let mut analyzer = Analyzer {
        config,
        objects: BTreeMap::new(),
        resources: BTreeMap::new(),
        snapshots: Vec::new(),
        pending_roots: None,
        quiescence_open: false,
        findings: Vec::new(),
        limitations: Vec::new(),
        observations: 0,
    };

    if !config.valid() {
        analyzer
            .limitations
            .push(LeakLimitation::InvalidConfiguration);
        return analyzer.finish(trace);
    }
    if trace.truncated {
        analyzer.limitations.push(LeakLimitation::TraceTruncated);
    }

    for (index, event) in trace.events.iter().enumerate() {
        let sequence = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        if !analyzer.observe(sequence) {
            break;
        }
        analyzer.process(event, sequence);
    }
    analyzer.finish(trace)
}

impl Analyzer {
    fn observe(&mut self, _sequence: u64) -> bool {
        if self.observations >= self.config.max_observations {
            self.push_limitation(LeakLimitation::ObservationLimit);
            return false;
        }
        self.observations = self.observations.saturating_add(1);
        true
    }

    fn process(&mut self, event: &DiagnosticEvent, sequence: u64) {
        match event {
            DiagnosticEvent::Heap {
                object_id,
                operation,
                bytes,
                owner_task,
                source,
                stack,
            } => {
                let entry = self
                    .objects
                    .entry(*object_id)
                    .or_insert_with(|| ObjectObservation {
                        object_id: *object_id,
                        bytes: *bytes,
                        owner_task: *owner_task,
                        first_event: sequence,
                        last_event: sequence,
                        allocation_source: source.clone(),
                        allocation_stack: stack.clone(),
                        views: BTreeSet::new(),
                        retainers: BTreeMap::new(),
                    });
                if *operation == DiagnosticHeapOperation::Allocate {
                    entry.bytes = *bytes;
                    entry.owner_task = *owner_task;
                    entry.first_event = entry.first_event.min(sequence);
                    if entry.allocation_source.is_none() {
                        entry.allocation_source = source.clone();
                    }
                    if entry.allocation_stack.is_empty() {
                        entry.allocation_stack = stack.clone();
                    }
                } else {
                    entry.bytes = *bytes;
                    entry.owner_task = *owner_task;
                }
                entry.last_event = sequence;
            }
            DiagnosticEvent::Resource {
                id,
                kind,
                state,
                owner_task,
                source,
                stack,
            } => {
                self.observe_resource(ResourceObservationInput {
                    id: *id,
                    kind: kind.clone(),
                    state: *state,
                    owner_task: *owner_task,
                    sequence,
                    source: source.clone(),
                    stack: stack.clone(),
                });
            }
            DiagnosticEvent::Roots {
                task_id,
                object_ids,
                retainers,
            } => {
                let mut ids = object_ids.clone();
                ids.sort_unstable();
                ids.dedup();
                let mut bounded_retainers = retainers.clone();
                bounded_retainers.sort();
                bounded_retainers.dedup();
                self.pending_roots = Some(PendingRoots {
                    sequence,
                    snapshot: DiagnosticRootSnapshot {
                        task_id: *task_id,
                        object_ids: ids,
                        retainers: bounded_retainers,
                    },
                });
            }
            DiagnosticEvent::Quiescence { phase, .. } => match phase {
                super::diagnostics::DiagnosticQuiescencePhase::Begin => {
                    if self.quiescence_open {
                        self.push_limitation(LeakLimitation::MissingQuiescence);
                    }
                    self.quiescence_open = true;
                    self.pending_roots = None;
                }
                super::diagnostics::DiagnosticQuiescencePhase::End => {
                    if !self.quiescence_open {
                        self.push_limitation(LeakLimitation::MissingQuiescence);
                    } else if let Some(pending) = self.pending_roots.take() {
                        self.add_snapshot(sequence, pending);
                    } else {
                        self.push_limitation(LeakLimitation::MissingRootSnapshot { sequence });
                    }
                    self.quiescence_open = false;
                }
            },
            _ => {}
        }
    }

    fn observe_resource(&mut self, input: ResourceObservationInput) {
        let ResourceObservationInput {
            id,
            kind,
            state,
            owner_task,
            sequence,
            source,
            stack,
        } = input;
        let key = (kind.clone(), id);
        let entry = self
            .resources
            .entry(key)
            .or_insert_with(|| ResourceObservation {
                id,
                kind,
                owner_task,
                state,
                first_event: sequence,
                last_event: sequence,
                acquisition_source: None,
                acquisition_stack: Vec::new(),
            });
        entry.owner_task = owner_task;
        entry.state = state;
        entry.last_event = sequence;
        if state == DiagnosticResourceState::Acquired {
            if entry.acquisition_source.is_none() {
                entry.acquisition_source = source;
            }
            if entry.acquisition_stack.is_empty() {
                entry.acquisition_stack = stack;
            }
        }
    }

    fn add_snapshot(&mut self, sequence: u64, pending: PendingRoots) {
        let snapshot_index = self.snapshots.len();
        let mut object_ids = pending.snapshot.object_ids.clone();
        object_ids.sort_unstable();
        object_ids.dedup();
        let mut retained_objects = 0_u64;
        let mut retained_bytes = 0_u64;
        for object_id in &object_ids {
            let Some(object) = self.objects.get_mut(object_id) else {
                self.push_limitation(LeakLimitation::MissingAllocation {
                    object_id: *object_id,
                    sequence: pending.sequence,
                });
                continue;
            };
            object.views.insert(snapshot_index);
            object.retainers.insert(
                snapshot_index,
                pending
                    .snapshot
                    .retainers
                    .iter()
                    .filter(|retainer| retainer.object_id == *object_id)
                    .cloned()
                    .collect(),
            );
            retained_objects = retained_objects.saturating_add(1);
            retained_bytes = retained_bytes.saturating_add(object.bytes);
        }
        self.snapshots.push(LeakSnapshot {
            sequence,
            roots: vec![pending.snapshot],
            object_ids,
            retained_objects,
            retained_bytes,
        });
    }

    fn push_limitation(&mut self, limitation: LeakLimitation) {
        if !self.limitations.contains(&limitation) {
            self.limitations.push(limitation);
        }
    }

    fn finish(mut self, trace: &DiagnosticTrace) -> LeakReport {
        if self.config.valid() {
            if self.quiescence_open {
                self.push_limitation(LeakLimitation::MissingQuiescence);
            }

            // The ledger is duplicated in the trace so callers can consume a
            // compact trace without replaying every Resource event. Merge it as a
            // fallback, while preserving the event-derived acquisition stack.
            for resource in &trace.resources {
                let key = (resource.kind.clone(), resource.id);
                let entry = self
                    .resources
                    .entry(key)
                    .or_insert_with(|| ResourceObservation {
                        id: resource.id,
                        kind: resource.kind.clone(),
                        owner_task: resource.owner_task,
                        state: resource.state,
                        first_event: resource.first_event,
                        last_event: resource.last_event,
                        acquisition_source: None,
                        acquisition_stack: Vec::new(),
                    });
                entry.owner_task = resource.owner_task;
                entry.state = resource.state;
                entry.first_event = entry.first_event.min(resource.first_event);
                entry.last_event = entry.last_event.max(resource.last_event);
            }

            if self.snapshots.is_empty() {
                self.push_limitation(LeakLimitation::MissingRootSnapshot {
                    sequence: trace.events.len() as u64,
                });
            }
        }

        let growth = sustained_growth(&self.snapshots, self.config.min_growth_snapshots);
        if growth {
            self.push_finding(LeakFinding {
                kind: LeakKind::SustainedGrowth,
                object: None,
                resource: None,
                growth: self.snapshots.clone(),
            });
            let retained_objects = self
                .objects
                .values()
                .filter(|object| object.views.len() >= 2)
                .map(|object| self.public_object(object))
                .collect::<Vec<_>>();
            for object in retained_objects {
                self.push_finding(LeakFinding {
                    kind: LeakKind::ManagedRetention,
                    object: Some(object),
                    resource: None,
                    growth: Vec::new(),
                });
            }
        }

        let resources = self
            .resources
            .values()
            .map(Self::public_resource)
            .collect::<Vec<_>>();
        for resource in &resources {
            if resource.state != DiagnosticResourceState::Acquired {
                continue;
            }
            self.push_finding(LeakFinding {
                kind: if is_native_resource(&resource.kind) {
                    LeakKind::NativeAllocation
                } else {
                    LeakKind::AffineResource
                },
                object: None,
                resource: Some(resource.clone()),
                growth: Vec::new(),
            });
        }

        self.limitations.sort();
        self.limitations.dedup();
        let status = if !self.limitations.is_empty() {
            LeakStatus::Unsupported
        } else if self.findings.is_empty() {
            LeakStatus::Clean
        } else {
            LeakStatus::Finding
        };
        LeakReport {
            format: LEAK_SCHEMA,
            profile: "leaks",
            status,
            observations: self.observations,
            snapshots: self.snapshots,
            resources,
            findings: self.findings,
            limitations: self.limitations,
            events_seen: trace.events_seen,
            truncated: trace.truncated,
        }
    }

    fn push_finding(&mut self, finding: LeakFinding) {
        if self.findings.len() >= self.config.max_findings as usize {
            self.push_limitation(LeakLimitation::FindingLimit);
            return;
        }
        self.findings.push(finding);
    }

    fn public_object(&self, object: &ObjectObservation) -> LeakObject {
        let mut retainer_map = BTreeMap::<String, DiagnosticRetainer>::new();
        for retainers in object.retainers.values() {
            for retainer in retainers {
                retainer_map
                    .entry(format!("{}:{}", retainer.owner, retainer.object_id))
                    .or_insert_with(|| retainer.clone());
            }
        }
        LeakObject {
            object_id: object.object_id,
            bytes: object.bytes,
            owner_task: object.owner_task,
            first_event: object.first_event,
            last_event: object.last_event,
            first_view: object
                .views
                .first()
                .copied()
                .map(|view| view as u64 + 1)
                .unwrap_or_default(),
            last_view: object
                .views
                .last()
                .copied()
                .map(|view| view as u64 + 1)
                .unwrap_or_default(),
            retainers: retainer_map.into_values().collect(),
            allocation_source: object.allocation_source.clone(),
            allocation_stack: object.allocation_stack.clone(),
        }
    }

    fn public_resource(resource: &ResourceObservation) -> LeakResource {
        LeakResource {
            id: resource.id,
            kind: resource.kind.clone(),
            owner_task: resource.owner_task,
            state: resource.state,
            first_event: resource.first_event,
            last_event: resource.last_event,
            acquisition_source: resource.acquisition_source.clone(),
            acquisition_stack: resource.acquisition_stack.clone(),
        }
    }
}

fn sustained_growth(snapshots: &[LeakSnapshot], minimum: u32) -> bool {
    if snapshots.len() < minimum as usize {
        return false;
    }
    snapshots.windows(2).all(|window| {
        window[1].retained_objects > window[0].retained_objects
            || window[1].retained_bytes > window[0].retained_bytes
    })
}

fn is_native_resource(kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    lower.contains("ffi") || lower.contains("native") || lower.contains("allocation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::BytecodeSpan;
    use crate::runtime::diagnostics::{
        DiagnosticConfig, DiagnosticEvent, DiagnosticHeapOperation, DiagnosticQuiescencePhase,
    };

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

    fn trace(events: Vec<DiagnosticEvent>) -> DiagnosticTrace {
        DiagnosticTrace {
            format: super::super::diagnostics::DIAGNOSTIC_SCHEMA,
            config: DiagnosticConfig::default(),
            events,
            scheduler_tail: Vec::new(),
            roots: Vec::new(),
            resources: Vec::new(),
            source_maps: Vec::new(),
            events_seen: 0,
            truncated: false,
        }
    }

    fn heap(object_id: u64, bytes: u64) -> DiagnosticEvent {
        let source = source("main", object_id as u32);
        DiagnosticEvent::Heap {
            object_id,
            operation: DiagnosticHeapOperation::Allocate,
            bytes,
            owner_task: 1,
            source: Some(source.clone()),
            stack: vec![source],
        }
    }

    fn roots(object_ids: &[u64]) -> DiagnosticEvent {
        DiagnosticEvent::Roots {
            task_id: 1,
            object_ids: object_ids.to_vec(),
            retainers: object_ids
                .iter()
                .map(|object_id| DiagnosticRetainer {
                    object_id: *object_id,
                    owner: "task:1".into(),
                })
                .collect(),
        }
    }

    fn snapshot(object_ids: &[u64]) -> Vec<DiagnosticEvent> {
        vec![
            DiagnosticEvent::Quiescence {
                task_id: 1,
                phase: DiagnosticQuiescencePhase::Begin,
            },
            roots(object_ids),
            DiagnosticEvent::Quiescence {
                task_id: 1,
                phase: DiagnosticQuiescencePhase::End,
            },
        ]
    }

    #[test]
    fn defaults_and_invalid_configuration_are_explicit() {
        assert_eq!(LeakConfig::default().min_growth_snapshots, 3);
        for config in [
            LeakConfig {
                max_observations: 0,
                ..LeakConfig::default()
            },
            LeakConfig {
                max_findings: 0,
                ..LeakConfig::default()
            },
            LeakConfig {
                min_growth_snapshots: 1,
                ..LeakConfig::default()
            },
        ] {
            let report = detect_leaks_with_config(&trace(Vec::new()), config);
            assert!(report.is_unsupported());
            assert_eq!(report.limitations, [LeakLimitation::InvalidConfiguration]);
        }
    }

    #[test]
    fn unreachable_objects_and_cycles_are_not_leaks() {
        let mut events = vec![heap(1, 32), heap(2, 64)];
        events.extend(snapshot(&[]));
        let report = detect_leaks(&trace(events));
        assert!(
            report.is_clean(),
            "unreachable objects must be collected: {report:?}"
        );
        assert!(report.findings.is_empty());
    }

    #[test]
    fn sustained_growth_reports_retention_and_growth_with_stacks() {
        let mut events = vec![heap(1, 10)];
        events.extend(snapshot(&[1]));
        events.push(heap(2, 20));
        events.extend(snapshot(&[1, 2]));
        events.push(heap(3, 30));
        events.extend(snapshot(&[1, 2, 3]));
        let report = detect_leaks(&trace(events));
        assert_eq!(report.status, LeakStatus::Finding);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == LeakKind::SustainedGrowth)
        );
        let retained = report
            .findings
            .iter()
            .filter(|finding| finding.kind == LeakKind::ManagedRetention)
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 2);
        assert_eq!(retained[0].object.as_ref().unwrap().object_id, 1);
        assert!(
            !retained[0]
                .object
                .as_ref()
                .unwrap()
                .allocation_stack
                .is_empty()
        );
        assert_eq!(report.snapshots[2].retained_bytes, 60);
    }

    #[test]
    fn resources_distinguish_affine_and_native_and_ignore_released() {
        let source = source("open", 4);
        let events = vec![
            DiagnosticEvent::Resource {
                id: 1,
                kind: "File".into(),
                state: DiagnosticResourceState::Acquired,
                owner_task: 1,
                source: Some(source.clone()),
                stack: vec![source.clone()],
            },
            DiagnosticEvent::Resource {
                id: 1,
                kind: "File".into(),
                state: DiagnosticResourceState::Released,
                owner_task: 1,
                source: None,
                stack: Vec::new(),
            },
            DiagnosticEvent::Resource {
                id: 2,
                kind: "FfiAllocation".into(),
                state: DiagnosticResourceState::Acquired,
                owner_task: 1,
                source: Some(source.clone()),
                stack: vec![source],
            },
        ];
        let mut all = events;
        all.extend(snapshot(&[]));
        let report = detect_leaks(&trace(all));
        assert_eq!(report.status, LeakStatus::Finding);
        assert!(report.findings.iter().any(|finding| {
            finding.kind == LeakKind::NativeAllocation
                && finding
                    .resource
                    .as_ref()
                    .is_some_and(|resource| resource.id == 2)
        }));
        assert!(!report.findings.iter().any(|finding| {
            finding
                .resource
                .as_ref()
                .is_some_and(|resource| resource.id == 1)
        }));
    }

    #[test]
    fn missing_quiescence_and_roots_fail_closed() {
        let no_quiescence = detect_leaks(&trace(vec![heap(1, 1)]));
        assert!(no_quiescence.is_unsupported());
        assert!(
            no_quiescence
                .limitations
                .contains(&LeakLimitation::MissingRootSnapshot { sequence: 1 })
        );

        let missing_roots = detect_leaks(&trace(vec![
            DiagnosticEvent::Quiescence {
                task_id: 1,
                phase: DiagnosticQuiescencePhase::Begin,
            },
            DiagnosticEvent::Quiescence {
                task_id: 1,
                phase: DiagnosticQuiescencePhase::End,
            },
        ]));
        assert!(missing_roots.is_unsupported());
        assert!(
            missing_roots
                .limitations
                .iter()
                .any(|limitation| matches!(limitation, LeakLimitation::MissingRootSnapshot { .. }))
        );
    }

    #[test]
    fn truncation_and_limits_are_not_silent() {
        let mut truncated_trace = trace(snapshot(&[]));
        truncated_trace.truncated = true;
        let report = detect_leaks(&truncated_trace);
        assert!(report.is_unsupported());
        assert!(report.limitations.contains(&LeakLimitation::TraceTruncated));

        let limited = detect_leaks_with_config(
            &trace(vec![heap(1, 1), heap(2, 1)]),
            LeakConfig {
                max_observations: 1,
                ..LeakConfig::default()
            },
        );
        assert!(limited.is_unsupported());
        assert!(
            limited
                .limitations
                .contains(&LeakLimitation::ObservationLimit)
        );
    }
}
