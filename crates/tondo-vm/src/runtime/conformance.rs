//! Private collector adapter for the portable conformance suite.
//!
//! This module is compiled only with the `conformance` Cargo feature. It uses
//! the same heap and tracing descriptors as ordinary VM execution, but exposes
//! no handles, addresses, object layouts, or mutation primitive.

use crate::bytecode::{BytecodeCallableId, BytecodeTraceDescriptor, BytecodeTypeId};

use super::heap::{Heap, HeapHandle, HeapObject};
use super::value::Value;
use super::{VmError, VmLimits, VmStatistics};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScenario {
    ReachableRoots,
    UnreachableCycles,
    SustainedPressure,
    RetryBeforeOom,
}

impl MemoryScenario {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ReachableRoots => "reachable-roots",
            Self::UnreachableCycles => "unreachable-cycles",
            Self::SustainedPressure => "sustained-pressure",
            Self::RetryBeforeOom => "retry-before-oom",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryObservation {
    pub scenario: &'static str,
    pub collections: u64,
    pub reclaimed_objects: u64,
    pub peak_live_objects: u32,
    pub roots_preserved: bool,
    pub cycles_reclaimed: bool,
    pub retry_before_success: bool,
    pub retry_before_oom: bool,
}

pub fn run_memory_scenario(scenario: MemoryScenario) -> Result<MemoryObservation, VmError> {
    match scenario {
        MemoryScenario::ReachableRoots => reachable_roots(),
        MemoryScenario::UnreachableCycles => unreachable_cycles(),
        MemoryScenario::SustainedPressure => sustained_pressure(),
        MemoryScenario::RetryBeforeOom => retry_before_oom(),
    }
}

fn descriptors() -> Vec<BytecodeTraceDescriptor> {
    vec![
        BytecodeTraceDescriptor::String,
        BytecodeTraceDescriptor::Ref {
            value: BytecodeTypeId::new(1),
        },
        BytecodeTraceDescriptor::Array {
            element: BytecodeTypeId::new(3),
        },
        BytecodeTraceDescriptor::Closure {
            callable: BytecodeCallableId::new(7),
            captures: vec![BytecodeTypeId::new(1)],
        },
    ]
}

fn limits() -> VmLimits {
    VmLimits {
        max_heap_objects: 16,
        max_heap_bytes: 32 * 1024,
        initial_gc_threshold: 1,
        ..VmLimits::default()
    }
}

fn reachable_roots() -> Result<MemoryObservation, VmError> {
    let mut heap = Heap::new(limits(), descriptors());
    let mut statistics = VmStatistics::default();
    let text = heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("rooted".into()),
        &[],
        &mut statistics,
    )?;
    let reference = heap.allocate(
        BytecodeTypeId::new(1),
        HeapObject::Ref(Some(Value::Heap(text))),
        &[Value::Heap(text)],
        &mut statistics,
    )?;
    heap.collect(&[Value::Heap(reference)], &mut statistics)?;
    let roots_preserved = heap.get(reference).is_ok() && heap.get(text).is_ok();
    heap.collect(&[], &mut statistics)?;
    let reclaimed = heap.get(reference).is_err() && heap.get(text).is_err();
    Ok(observation(
        MemoryScenario::ReachableRoots,
        statistics,
        roots_preserved,
        reclaimed,
        false,
        false,
    ))
}

fn unreachable_cycles() -> Result<MemoryObservation, VmError> {
    let mut heap = Heap::new(limits(), descriptors());
    let mut statistics = VmStatistics::default();
    let first = heap.allocate(
        BytecodeTypeId::new(1),
        HeapObject::Ref(None),
        &[],
        &mut statistics,
    )?;
    let second = heap.allocate(
        BytecodeTypeId::new(1),
        HeapObject::Ref(Some(Value::Heap(first))),
        &[Value::Heap(first)],
        &mut statistics,
    )?;
    heap.replace(
        first,
        HeapObject::Ref(Some(Value::Heap(second))),
        &[Value::Heap(first), Value::Heap(second)],
        &mut statistics,
    )?;
    heap.collect(&[Value::Heap(first)], &mut statistics)?;
    let roots_preserved = heap.get(first).is_ok() && heap.get(second).is_ok();
    heap.collect(&[], &mut statistics)?;
    let cycles_reclaimed = heap.get(first).is_err() && heap.get(second).is_err();
    Ok(observation(
        MemoryScenario::UnreachableCycles,
        statistics,
        roots_preserved,
        cycles_reclaimed,
        false,
        false,
    ))
}

struct PressureAdapter {
    heap: Heap,
    roots: Vec<Value>,
    statistics: VmStatistics,
}

impl PressureAdapter {
    fn new() -> Self {
        Self {
            heap: Heap::new(limits(), descriptors()),
            roots: Vec::new(),
            statistics: VmStatistics::default(),
        }
    }

    fn active_roots(&self, temporary: &[HeapHandle]) -> Vec<Value> {
        self.roots
            .iter()
            .cloned()
            .chain(temporary.iter().copied().map(Value::Heap))
            .collect()
    }

    fn allocate(
        &mut self,
        descriptor: BytecodeTypeId,
        object: HeapObject,
        temporary: &[HeapHandle],
    ) -> Result<HeapHandle, VmError> {
        let roots = self.active_roots(temporary);
        self.heap
            .allocate(descriptor, object, &roots, &mut self.statistics)
    }

    fn replace(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
        temporary: &[HeapHandle],
    ) -> Result<(), VmError> {
        let roots = self.active_roots(temporary);
        self.heap
            .replace(handle, object, &roots, &mut self.statistics)
    }

    fn cycle(&mut self) -> Result<[HeapHandle; 3], VmError> {
        let reference = self.allocate(BytecodeTypeId::new(1), HeapObject::Ref(None), &[])?;
        let closure = self.allocate(
            BytecodeTypeId::new(3),
            HeapObject::Closure {
                callable: BytecodeCallableId::new(7),
                captures: vec![Some(Value::Heap(reference))],
            },
            &[reference],
        )?;
        let array = self.allocate(
            BytecodeTypeId::new(2),
            HeapObject::Array(vec![Some(Value::Heap(closure))].into()),
            &[reference, closure],
        )?;
        self.replace(
            reference,
            HeapObject::Ref(Some(Value::Heap(array))),
            &[reference, closure, array],
        )?;
        Ok([reference, array, closure])
    }

    fn pressure(&mut self, count: usize) -> Result<(), VmError> {
        for index in 0..count {
            self.allocate(
                BytecodeTypeId::new(0),
                HeapObject::String(format!("pressure-{index}")),
                &[],
            )?;
        }
        Ok(())
    }
}

fn sustained_pressure() -> Result<MemoryObservation, VmError> {
    let mut adapter = PressureAdapter::new();
    let retained = adapter.cycle()?;
    adapter.roots.push(Value::Heap(retained[0]));
    let mut garbage_reclaimed = true;
    for _ in 0..32 {
        let garbage = adapter.cycle()?;
        adapter.pressure(8)?;
        garbage_reclaimed &= garbage
            .iter()
            .all(|handle| adapter.heap.get(*handle).is_err());
        if !retained
            .iter()
            .all(|handle| adapter.heap.get(*handle).is_ok())
        {
            return Err(VmError::invariant(
                "conformance pressure lost a reachable cycle",
            ));
        }
    }
    let roots_preserved = retained
        .iter()
        .all(|handle| adapter.heap.get(*handle).is_ok());
    adapter.roots.clear();
    adapter.pressure(8)?;
    let cycles_reclaimed = garbage_reclaimed
        && retained
            .iter()
            .all(|handle| adapter.heap.get(*handle).is_err());
    Ok(observation(
        MemoryScenario::SustainedPressure,
        adapter.statistics,
        roots_preserved,
        cycles_reclaimed,
        false,
        false,
    ))
}

fn retry_before_oom() -> Result<MemoryObservation, VmError> {
    let retry_limits = VmLimits {
        max_heap_objects: 2,
        max_heap_bytes: 16 * 1024,
        initial_gc_threshold: 2,
        ..VmLimits::default()
    };
    let mut statistics = VmStatistics::default();
    let mut heap = Heap::new(retry_limits, vec![BytecodeTraceDescriptor::String]);
    let first = heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("first".into()),
        &[],
        &mut statistics,
    )?;
    let second = heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("second".into()),
        &[],
        &mut statistics,
    )?;
    let replacement = heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("replacement".into()),
        &[],
        &mut statistics,
    )?;
    let retry_before_success = statistics.collections == 1
        && heap.get(first).is_err()
        && heap.get(second).is_err()
        && heap.get(replacement).is_ok();

    let mut oom_statistics = VmStatistics::default();
    let mut oom_heap = Heap::new(retry_limits, vec![BytecodeTraceDescriptor::String]);
    let first = oom_heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("first".into()),
        &[],
        &mut oom_statistics,
    )?;
    let second = oom_heap.allocate(
        BytecodeTypeId::new(0),
        HeapObject::String("second".into()),
        &[],
        &mut oom_statistics,
    )?;
    let error = oom_heap
        .allocate(
            BytecodeTypeId::new(0),
            HeapObject::String("rejected".into()),
            &[Value::Heap(first), Value::Heap(second)],
            &mut oom_statistics,
        )
        .expect_err("two retained objects must exhaust a two-object heap");
    let retry_before_oom = matches!(error, VmError::OutOfMemory { .. })
        && oom_statistics.collections == 1
        && oom_heap.get(first).is_ok()
        && oom_heap.get(second).is_ok();

    statistics.collections += oom_statistics.collections;
    statistics.reclaimed_objects += oom_statistics.reclaimed_objects;
    statistics.peak_live_objects = statistics
        .peak_live_objects
        .max(oom_statistics.peak_live_objects);
    Ok(observation(
        MemoryScenario::RetryBeforeOom,
        statistics,
        true,
        true,
        retry_before_success,
        retry_before_oom,
    ))
}

fn observation(
    scenario: MemoryScenario,
    statistics: VmStatistics,
    roots_preserved: bool,
    cycles_reclaimed: bool,
    retry_before_success: bool,
    retry_before_oom: bool,
) -> MemoryObservation {
    MemoryObservation {
        scenario: scenario.name(),
        collections: statistics.collections,
        reclaimed_objects: statistics.reclaimed_objects,
        peak_live_objects: statistics.peak_live_objects,
        roots_preserved,
        cycles_reclaimed,
        retry_before_success,
        retry_before_oom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_private_memory_scenario_uses_the_real_collector() {
        for scenario in [
            MemoryScenario::ReachableRoots,
            MemoryScenario::UnreachableCycles,
            MemoryScenario::SustainedPressure,
            MemoryScenario::RetryBeforeOom,
        ] {
            let observation = run_memory_scenario(scenario).unwrap();
            assert!(observation.collections > 0);
            assert!(observation.roots_preserved);
            assert!(observation.cycles_reclaimed);
        }
    }
}
