use std::collections::BTreeSet;
use std::mem;

use crate::bytecode::{
    BytecodeCallableId, BytecodeCursorMode, BytecodeNominalId, BytecodeRangeKind, BytecodeTypeId,
};

use super::value::{AggregatePayload, Value};
use super::{VmError, VmLimits, VmStatistics};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct HeapHandle {
    index: u32,
    generation: u32,
}

impl HeapHandle {
    #[cfg(test)]
    pub(super) const fn index(self) -> u32 {
        self.index
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum HeapObject {
    String(String),
    Tuple(Vec<Option<Value>>),
    Array(Vec<Option<Value>>),
    Map(Vec<(Option<Value>, Option<Value>)>),
    Set(Vec<Option<Value>>),
    Closure {
        callable: BytecodeCallableId,
        captures: Vec<Option<Value>>,
    },
    Newtype {
        nominal: BytecodeNominalId,
        value: Option<Value>,
    },
    Record {
        nominal: BytecodeNominalId,
        fields: Vec<(u32, Option<Value>)>,
    },
    Variant {
        variant: u32,
        payload: AggregatePayload,
    },
    OptionNone,
    OptionSome(Option<Value>),
    ResultOk(Option<Value>),
    ResultErr(Option<Value>),
    Union {
        member: BytecodeTypeId,
        value: Option<Value>,
    },
    Range {
        kind: BytecodeRangeKind,
        start: Option<Value>,
        end: Option<Value>,
    },
    Iterator {
        mode: BytecodeCursorMode,
        source: Option<Value>,
        next: usize,
    },
    #[allow(dead_code)]
    Ref(Option<Value>),
}

impl HeapObject {
    fn trace_values(&self, output: &mut Vec<Value>) {
        match self {
            Self::String(_) | Self::OptionNone => {}
            Self::Tuple(values) | Self::Array(values) | Self::Set(values) => {
                output.extend(values.iter().flatten().cloned());
            }
            Self::Closure { captures, .. } => {
                output.extend(captures.iter().flatten().cloned());
            }
            Self::Map(entries) => {
                for (key, value) in entries {
                    output.extend(key.iter().cloned());
                    output.extend(value.iter().cloned());
                }
            }
            Self::Newtype { value, .. }
            | Self::OptionSome(value)
            | Self::ResultOk(value)
            | Self::ResultErr(value)
            | Self::Union { value, .. }
            | Self::Ref(value) => output.extend(value.iter().cloned()),
            Self::Record { fields, .. } => {
                output.extend(fields.iter().filter_map(|(_, value)| value.clone()));
            }
            Self::Variant { payload, .. } => payload.trace_values(output),
            Self::Range { start, end, .. } => {
                output.extend(start.iter().cloned());
                output.extend(end.iter().cloned());
            }
            Self::Iterator { source, .. } => output.extend(source.iter().cloned()),
        }
    }

    fn estimated_bytes(&self) -> u64 {
        let base = mem::size_of::<Self>() as u64;
        let value = mem::size_of::<Option<Value>>() as u64;
        base.saturating_add(match self {
            Self::String(text) => text.capacity() as u64,
            Self::Tuple(values) | Self::Array(values) | Self::Set(values) => {
                (values.capacity() as u64).saturating_mul(value)
            }
            Self::Closure { captures, .. } => (captures.capacity() as u64).saturating_mul(value),
            Self::Map(entries) => (entries.capacity() as u64)
                .saturating_mul((mem::size_of::<(Option<Value>, Option<Value>)>()) as u64),
            Self::Record { fields, .. } => (fields.capacity() as u64)
                .saturating_mul(mem::size_of::<(u32, Option<Value>)>() as u64),
            Self::Variant { payload, .. } => match payload {
                AggregatePayload::Unit => 0,
                AggregatePayload::Tuple(values) => (values.capacity() as u64).saturating_mul(value),
                AggregatePayload::Record(fields) => (fields.capacity() as u64)
                    .saturating_mul(mem::size_of::<(u32, Option<Value>)>() as u64),
            },
            Self::Newtype { .. }
            | Self::OptionNone
            | Self::OptionSome(_)
            | Self::ResultOk(_)
            | Self::ResultErr(_)
            | Self::Union { .. }
            | Self::Range { .. }
            | Self::Iterator { .. }
            | Self::Ref(_) => 0,
        })
    }
}

#[derive(Debug)]
struct HeapSlot {
    generation: u32,
    marked: bool,
    object: Option<HeapObject>,
    bytes: u64,
}

#[derive(Debug)]
pub(super) struct Heap {
    slots: Vec<HeapSlot>,
    free: Vec<u32>,
    live_objects: u32,
    live_bytes: u64,
    next_collection: u32,
    limits: VmLimits,
}

impl Heap {
    pub(super) fn new(limits: VmLimits) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live_objects: 0,
            live_bytes: 0,
            next_collection: limits.initial_gc_threshold.min(limits.max_heap_objects),
            limits,
        }
    }

    pub(super) fn allocate(
        &mut self,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<HeapHandle, VmError> {
        let bytes = object.estimated_bytes();
        if self.live_objects >= self.next_collection
            || self.live_objects >= self.limits.max_heap_objects
            || self.live_bytes.saturating_add(bytes) > self.limits.max_heap_bytes
        {
            self.collect_with_pending(roots, Some(&object), statistics)?;
        }
        if self.live_objects >= self.limits.max_heap_objects
            || self.live_bytes.saturating_add(bytes) > self.limits.max_heap_bytes
        {
            return Err(VmError::OutOfMemory {
                live_objects: self.live_objects,
                live_bytes: self.live_bytes,
            });
        }

        let handle = if let Some(index) = self.free.pop() {
            let slot = self
                .slots
                .get_mut(index as usize)
                .ok_or_else(|| VmError::invariant("heap free list contains an invalid slot"))?;
            slot.generation = slot.generation.wrapping_add(1);
            if slot.generation == 0 {
                slot.generation = 1;
            }
            slot.object = Some(object);
            slot.bytes = bytes;
            HeapHandle {
                index,
                generation: slot.generation,
            }
        } else {
            let index = u32::try_from(self.slots.len())
                .map_err(|_| VmError::invariant("heap slot index exceeds u32"))?;
            self.slots.push(HeapSlot {
                generation: 1,
                marked: false,
                object: Some(object),
                bytes,
            });
            HeapHandle {
                index,
                generation: 1,
            }
        };
        self.live_objects += 1;
        self.live_bytes = self.live_bytes.saturating_add(bytes);
        statistics.allocations = statistics.allocations.saturating_add(1);
        statistics.peak_live_objects = statistics.peak_live_objects.max(self.live_objects);
        statistics.peak_live_bytes = statistics.peak_live_bytes.max(self.live_bytes);
        Ok(handle)
    }

    pub(super) fn get(&self, handle: HeapHandle) -> Result<&HeapObject, VmError> {
        let slot = self
            .slots
            .get(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)
            .ok_or_else(|| VmError::invariant("stale or invalid heap handle"))?;
        slot.object
            .as_ref()
            .ok_or_else(|| VmError::invariant("heap handle refers to a collected object"))
    }

    pub(super) fn replace(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        let old_bytes = self.get(handle)?.estimated_bytes();
        let new_bytes = object.estimated_bytes();
        let growth = new_bytes.saturating_sub(old_bytes);
        if self.live_bytes.saturating_add(growth) > self.limits.max_heap_bytes {
            self.collect_with_pending(roots, Some(&object), statistics)?;
        }
        if self.live_bytes.saturating_add(growth) > self.limits.max_heap_bytes {
            return Err(VmError::OutOfMemory {
                live_objects: self.live_objects,
                live_bytes: self.live_bytes,
            });
        }
        let slot = self
            .slots
            .get_mut(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)
            .ok_or_else(|| VmError::invariant("stale heap handle during mutation"))?;
        if slot.object.is_none() {
            return Err(VmError::invariant(
                "collected heap handle used during mutation",
            ));
        }
        self.live_bytes = self.live_bytes.saturating_sub(slot.bytes);
        slot.bytes = new_bytes;
        slot.object = Some(object);
        self.live_bytes = self.live_bytes.saturating_add(new_bytes);
        statistics.peak_live_bytes = statistics.peak_live_bytes.max(self.live_bytes);
        Ok(())
    }

    pub(super) fn collect(
        &mut self,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        self.collect_with_pending(roots, None, statistics)
    }

    fn collect_with_pending(
        &mut self,
        roots: &[Value],
        pending: Option<&HeapObject>,
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        for slot in &mut self.slots {
            slot.marked = false;
        }
        let mut work = roots.to_vec();
        if let Some(object) = pending {
            object.trace_values(&mut work);
        }
        let mut visited = BTreeSet::new();
        while let Some(value) = work.pop() {
            let Some(handle) = value.heap_handle() else {
                continue;
            };
            if !visited.insert(handle) {
                continue;
            }
            let slot = self
                .slots
                .get_mut(handle.index as usize)
                .filter(|slot| slot.generation == handle.generation)
                .ok_or_else(|| VmError::invariant("GC root contains a stale heap handle"))?;
            let object = slot
                .object
                .as_ref()
                .ok_or_else(|| VmError::invariant("GC root refers to a collected object"))?;
            slot.marked = true;
            object.trace_values(&mut work);
        }

        let before = self.live_objects;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.object.is_some() && !slot.marked {
                slot.object = None;
                self.live_objects -= 1;
                self.live_bytes = self.live_bytes.saturating_sub(slot.bytes);
                slot.bytes = 0;
                self.free.push(index as u32);
            }
        }
        let doubled = self.live_objects.saturating_mul(2).max(1);
        self.next_collection = doubled
            .max(
                self.limits
                    .initial_gc_threshold
                    .min(self.limits.max_heap_objects),
            )
            .min(self.limits.max_heap_objects);
        statistics.collections = statistics.collections.saturating_add(1);
        statistics.reclaimed_objects = statistics
            .reclaimed_objects
            .saturating_add(u64::from(before - self.live_objects));
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn live_objects(&self) -> u32 {
        self.live_objects
    }
}
