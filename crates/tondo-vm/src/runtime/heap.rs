use std::collections::BTreeSet;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::bytecode::{
    BytecodeCallableId, BytecodeCursorMode, BytecodeNominalId, BytecodeRangeKind,
    BytecodeTraceDescriptor, BytecodeTypeId, BytecodeVariantPayload,
};

use super::value::{AggregatePayload, Value};
use super::{VmError, VmLimits, VmStatistics};

#[derive(Debug, Clone)]
pub(super) struct SharedBuffer<T>(Arc<Vec<T>>);

impl<T> SharedBuffer<T> {
    pub(super) fn is_unique(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    fn storage_id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl<T> From<Vec<T>> for SharedBuffer<T> {
    fn from(values: Vec<T>) -> Self {
        Self(Arc::new(values))
    }
}

impl<T> FromIterator<T> for SharedBuffer<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Self::from(values.into_iter().collect::<Vec<_>>())
    }
}

impl<T> Deref for SharedBuffer<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Clone> DerefMut for SharedBuffer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: PartialEq> PartialEq for SharedBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Clone> IntoIterator for SharedBuffer<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        Arc::try_unwrap(self.0)
            .unwrap_or_else(|shared| shared.as_ref().clone())
            .into_iter()
    }
}

impl<'buffer, T> IntoIterator for &'buffer SharedBuffer<T> {
    type Item = &'buffer T;
    type IntoIter = std::slice::Iter<'buffer, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'buffer, T: Clone> IntoIterator for &'buffer mut SharedBuffer<T> {
    type Item = &'buffer mut T;
    type IntoIter = std::slice::IterMut<'buffer, T>;

    fn into_iter(self) -> Self::IntoIter {
        Arc::make_mut(&mut self.0).iter_mut()
    }
}

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
    Array(SharedBuffer<Option<Value>>),
    Map(SharedBuffer<(Option<Value>, Option<Value>)>),
    Set(SharedBuffer<Option<Value>>),
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
        adapter: Option<IteratorAdapter>,
    },
    Ref(Option<Value>),
}

/// Runtime state for the lazy `std.iter` adapters.  The adapter keeps the
/// source cursor and callback as managed values owned by the outer cursor; no
/// collection is materialized until `collect` is requested.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum IteratorAdapter {
    Map {
        callback: Value,
        source_item: BytecodeTypeId,
    },
    Filter {
        callback: Value,
        source_item: BytecodeTypeId,
    },
    Take {
        remaining: usize,
        source_item: BytecodeTypeId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionStorageKind {
    Array,
    Map,
    Set,
}

impl HeapObject {
    fn collection_storage(&self) -> Option<(CollectionStorageKind, usize, bool)> {
        match self {
            Self::Array(values) => Some((
                CollectionStorageKind::Array,
                values.storage_id(),
                values.is_unique(),
            )),
            Self::Map(entries) => Some((
                CollectionStorageKind::Map,
                entries.storage_id(),
                entries.is_unique(),
            )),
            Self::Set(values) => Some((
                CollectionStorageKind::Set,
                values.storage_id(),
                values.is_unique(),
            )),
            _ => None,
        }
    }

    pub(super) fn estimated_bytes(&self) -> u64 {
        let base = mem::size_of::<Self>() as u64;
        let value = mem::size_of::<Option<Value>>() as u64;
        base.saturating_add(match self {
            Self::String(text) => text.capacity() as u64,
            Self::Tuple(values) => (values.capacity() as u64).saturating_mul(value),
            Self::Array(values) | Self::Set(values) => {
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
    descriptor: BytecodeTypeId,
    object: Option<HeapObject>,
    bytes: u64,
}

#[derive(Debug)]
pub(super) struct Heap {
    descriptors: Vec<BytecodeTraceDescriptor>,
    slots: Vec<HeapSlot>,
    free: Vec<u32>,
    live_objects: u32,
    live_bytes: u64,
    next_collection: u32,
    limits: VmLimits,
}

struct CapacityDemand<'object> {
    objects: u32,
    bytes: u64,
    threshold_reached: bool,
    protected: Option<HeapHandle>,
    pending: Option<(BytecodeTypeId, &'object HeapObject)>,
}

impl Heap {
    pub(super) fn new(limits: VmLimits, descriptors: Vec<BytecodeTraceDescriptor>) -> Self {
        Self {
            descriptors,
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
        descriptor: BytecodeTypeId,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<HeapHandle, VmError> {
        Self::visit_object(&self.descriptors, descriptor, &object, |_| {})?;
        let bytes = object.estimated_bytes();
        self.ensure_capacity(
            CapacityDemand {
                objects: 1,
                bytes,
                threshold_reached: self.live_objects >= self.next_collection,
                protected: None,
                pending: Some((descriptor, &object)),
            },
            roots,
            statistics,
        )?;

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
            slot.descriptor = descriptor;
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
                descriptor,
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

    pub(super) fn descriptor(&self, handle: HeapHandle) -> Result<BytecodeTypeId, VmError> {
        let slot = self
            .slots
            .get(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)
            .ok_or_else(|| VmError::invariant("stale or invalid heap handle"))?;
        if slot.object.is_none() {
            return Err(VmError::invariant(
                "heap handle refers to a collected object",
            ));
        }
        Ok(slot.descriptor)
    }

    pub(super) fn replace(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        let descriptor = self.descriptor(handle)?;
        Self::visit_object(&self.descriptors, descriptor, &object, |_| {})?;
        let old_object = self.get(handle)?;
        let old_bytes = old_object.estimated_bytes();
        let new_bytes = object.estimated_bytes();
        let new_storage = object.collection_storage();
        let growth = new_bytes.saturating_sub(old_bytes);
        self.ensure_capacity(
            CapacityDemand {
                objects: 0,
                bytes: growth,
                threshold_reached: false,
                protected: Some(handle),
                pending: Some((descriptor, &object)),
            },
            roots,
            statistics,
        )?;
        let old_storage = self.get(handle)?.collection_storage();
        let detached_shared_buffer = matches!((old_storage, new_storage),
            (
                Some((old_kind, old_id, false)),
                Some((new_kind, new_id, _)),
            ) if old_kind == new_kind && old_id != new_id
        );
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
        if detached_shared_buffer {
            statistics.collection_buffer_detaches =
                statistics.collection_buffer_detaches.saturating_add(1);
        }
        statistics.peak_live_bytes = statistics.peak_live_bytes.max(self.live_bytes);
        Ok(())
    }

    pub(super) fn collect(
        &mut self,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        self.collect_with_pending(roots, None, None, statistics)
    }

    fn ensure_capacity(
        &mut self,
        demand: CapacityDemand<'_>,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        // Threshold or capacity pressure permits at most one full collection.
        // The protected handle keeps a replacement target stable until publication.
        if demand.threshold_reached || !self.has_capacity(demand.objects, demand.bytes) {
            self.collect_with_pending(roots, demand.protected, demand.pending, statistics)?;
        }
        if self.has_capacity(demand.objects, demand.bytes) {
            Ok(())
        } else {
            Err(VmError::OutOfMemory {
                live_objects: self.live_objects,
                live_bytes: self.live_bytes,
            })
        }
    }

    fn has_capacity(&self, additional_objects: u32, additional_bytes: u64) -> bool {
        self.live_objects
            .checked_add(additional_objects)
            .is_some_and(|total| total <= self.limits.max_heap_objects)
            && self
                .live_bytes
                .checked_add(additional_bytes)
                .is_some_and(|total| total <= self.limits.max_heap_bytes)
    }

    fn collect_with_pending(
        &mut self,
        roots: &[Value],
        protected: Option<HeapHandle>,
        pending: Option<(BytecodeTypeId, &HeapObject)>,
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        for slot in &mut self.slots {
            slot.marked = false;
        }
        let mut work = roots.to_vec();
        if let Some(handle) = protected {
            work.push(Value::Heap(handle));
        }
        if let Some((descriptor, object)) = pending {
            Self::visit_object(&self.descriptors, descriptor, object, |value| {
                work.push(value.clone());
            })?;
        }
        let mut visited = BTreeSet::new();
        let descriptors = &self.descriptors;
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
            let descriptor = slot.descriptor;
            Self::visit_object(descriptors, descriptor, object, |value| {
                work.push(value.clone());
            })?;
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

    fn visit_object(
        descriptors: &[BytecodeTraceDescriptor],
        descriptor: BytecodeTypeId,
        object: &HeapObject,
        mut visit: impl FnMut(&Value),
    ) -> Result<(), VmError> {
        let descriptor = descriptors
            .get(descriptor.index() as usize)
            .ok_or_else(|| VmError::invariant("heap object has an unknown trace descriptor"))?;
        match (descriptor, object) {
            (BytecodeTraceDescriptor::String, HeapObject::String(_))
            | (BytecodeTraceDescriptor::Option { .. }, HeapObject::OptionNone) => {}
            (BytecodeTraceDescriptor::Tuple { fields }, HeapObject::Tuple(values))
                if fields.len() == values.len() =>
            {
                visit_optional_values(values, &mut visit);
            }
            (BytecodeTraceDescriptor::Array { .. }, HeapObject::Array(values))
            | (BytecodeTraceDescriptor::Set { .. }, HeapObject::Set(values)) => {
                visit_optional_values(values, &mut visit);
            }
            (BytecodeTraceDescriptor::Map { .. }, HeapObject::Map(entries)) => {
                for (key, value) in entries {
                    visit_optional_value(key, &mut visit);
                    visit_optional_value(value, &mut visit);
                }
            }
            (
                BytecodeTraceDescriptor::Closure {
                    callable,
                    captures: expected,
                },
                HeapObject::Closure {
                    callable: actual,
                    captures,
                },
            ) if callable == actual && expected.len() == captures.len() => {
                visit_optional_values(captures, &mut visit);
            }
            (
                BytecodeTraceDescriptor::Newtype {
                    nominal: expected, ..
                },
                HeapObject::Newtype {
                    nominal: actual,
                    value,
                },
            ) if expected == actual => visit_optional_value(value, &mut visit),
            (
                BytecodeTraceDescriptor::Record {
                    nominal: expected,
                    fields: expected_fields,
                    ..
                },
                HeapObject::Record {
                    nominal: actual,
                    fields,
                },
            ) if expected == actual
                && expected_fields.len() == fields.len()
                && expected_fields
                    .iter()
                    .zip(fields)
                    .all(|(expected, (actual, _))| expected.member == *actual) =>
            {
                for (_, value) in fields {
                    visit_optional_value(value, &mut visit);
                }
            }
            (
                BytecodeTraceDescriptor::Variant { variants, .. },
                HeapObject::Variant { variant, payload },
            ) => {
                let expected = variants
                    .iter()
                    .find(|candidate| candidate.member == *variant)
                    .ok_or_else(|| {
                        VmError::invariant("heap variant is absent from its trace descriptor")
                    })?;
                visit_payload(&expected.payload, payload, &mut visit)?;
            }
            (BytecodeTraceDescriptor::Option { .. }, HeapObject::OptionSome(value))
            | (BytecodeTraceDescriptor::Result { .. }, HeapObject::ResultOk(value))
            | (BytecodeTraceDescriptor::Result { .. }, HeapObject::ResultErr(value))
            | (BytecodeTraceDescriptor::Ref { .. }, HeapObject::Ref(value)) => {
                visit_optional_value(value, &mut visit);
            }
            (BytecodeTraceDescriptor::Union { members }, HeapObject::Union { member, value })
                if members.contains(member) =>
            {
                visit_optional_value(value, &mut visit)
            }
            (BytecodeTraceDescriptor::Range { .. }, HeapObject::Range { start, end, .. }) => {
                visit_optional_value(start, &mut visit);
                visit_optional_value(end, &mut visit);
            }
            (
                BytecodeTraceDescriptor::Cursor { mode: expected, .. },
                HeapObject::Iterator {
                    mode: actual,
                    source,
                    adapter,
                    ..
                },
            ) if expected == actual => {
                visit_optional_value(source, &mut visit);
                if let Some(adapter) = adapter {
                    match adapter {
                        IteratorAdapter::Map { callback, .. }
                        | IteratorAdapter::Filter { callback, .. } => visit(callback),
                        IteratorAdapter::Take { .. } => {}
                    }
                }
            }
            _ => {
                return Err(VmError::invariant(
                    "heap object does not match its verified trace descriptor",
                ));
            }
        }
        Ok(())
    }
}

fn visit_optional_values(values: &[Option<Value>], visit: &mut impl FnMut(&Value)) {
    for value in values.iter().flatten() {
        visit(value);
    }
}

fn visit_optional_value(value: &Option<Value>, visit: &mut impl FnMut(&Value)) {
    if let Some(value) = value {
        visit(value);
    }
}

fn visit_payload(
    descriptor: &BytecodeVariantPayload,
    payload: &AggregatePayload,
    visit: &mut impl FnMut(&Value),
) -> Result<(), VmError> {
    match (descriptor, payload) {
        (BytecodeVariantPayload::Unit, AggregatePayload::Unit) => {}
        (BytecodeVariantPayload::Tuple(expected), AggregatePayload::Tuple(values))
            if expected.len() == values.len() =>
        {
            visit_optional_values(values, visit);
        }
        (BytecodeVariantPayload::Record(expected), AggregatePayload::Record(fields))
            if expected.len() == fields.len()
                && expected
                    .iter()
                    .zip(fields)
                    .all(|(expected, (actual, _))| expected.member == *actual) =>
        {
            for (_, value) in fields {
                visit_optional_value(value, visit);
            }
        }
        _ => {
            return Err(VmError::invariant(
                "heap variant payload does not match its trace descriptor",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::bytecode::BytecodeVariant;

    use super::*;

    #[test]
    fn iterator_adapters_trace_source_and_callbacks() {
        let descriptors = vec![BytecodeTraceDescriptor::Cursor {
            mode: BytecodeCursorMode::Own,
            collection: BytecodeTypeId::new(0),
        }];
        let source = Value::Heap(HeapHandle {
            index: 1,
            generation: 0,
        });
        let callback = Value::Heap(HeapHandle {
            index: 2,
            generation: 0,
        });

        for adapter in [
            IteratorAdapter::Map {
                callback: callback.clone(),
                source_item: BytecodeTypeId::new(0),
            },
            IteratorAdapter::Filter {
                callback: callback.clone(),
                source_item: BytecodeTypeId::new(0),
            },
            IteratorAdapter::Take {
                remaining: 2,
                source_item: BytecodeTypeId::new(0),
            },
        ] {
            let object = HeapObject::Iterator {
                mode: BytecodeCursorMode::Own,
                source: Some(source.clone()),
                next: 0,
                adapter: Some(adapter),
            };
            let mut roots = Vec::new();
            Heap::visit_object(&descriptors, BytecodeTypeId::new(0), &object, |value| {
                roots.push(value.clone())
            })
            .unwrap();

            let expected = match object {
                HeapObject::Iterator {
                    adapter: Some(IteratorAdapter::Take { .. }),
                    ..
                } => vec![source.clone()],
                _ => vec![source.clone(), callback.clone()],
            };
            assert_eq!(roots, expected);
        }

        let descriptors = vec![BytecodeTraceDescriptor::Variant {
            nominal: None,
            arguments: Vec::new(),
            variants: vec![BytecodeVariant {
                member: 1,
                payload: BytecodeVariantPayload::Unit,
            }],
        }];
        let invalid = HeapObject::Variant {
            variant: 2,
            payload: AggregatePayload::Unit,
        };
        assert!(
            Heap::visit_object(&descriptors, BytecodeTypeId::new(0), &invalid, |_| {},).is_err()
        );
    }
}
