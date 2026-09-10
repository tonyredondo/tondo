use std::collections::BTreeSet;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::bytecode::{
    BytecodeCallableId, BytecodeCursorMode, BytecodeNominalId, BytecodeRangeKind,
    BytecodeTraceDescriptor, BytecodeTypeId, BytecodeVariantPayload,
};

use super::value::{AggregatePayload, Value};
use super::{VmError, VmHostRoots, VmLimits, VmMemoryBudget, VmMemoryCharge, VmStatistics};

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
    pub(super) const fn diagnostic_id(self) -> u64 {
        ((self.index as u64) << 32) | (self.generation as u64)
    }

    #[cfg(test)]
    pub(super) const fn for_test(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

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
    /// Internal cursor state for a hosted `std.sync` collection.  The
    /// collection itself remains the source value; only the finite structural
    /// cutoff and the last birth generation are retained.
    Sync {
        cutoff: u64,
        last: u64,
        descending: bool,
    },
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
    charge: Option<VmMemoryCharge>,
}

#[derive(Debug)]
pub(super) struct ImportObjectReservation {
    reserved: Arc<AtomicU32>,
    remaining: u32,
}

impl Drop for ImportObjectReservation {
    fn drop(&mut self) {
        self.reserved.fetch_sub(self.remaining, Ordering::AcqRel);
    }
}

/// Object capacity exported only while a blocking worker is stopped inside
/// its scoped host call. The worker must not allocate until the context returns.
/// Other threads can release reservations, but cannot allocate in that heap.
#[derive(Debug)]
pub(super) struct PausedImportCapacity {
    reserved: Arc<AtomicU32>,
    live_objects: u32,
    live_bytes: u64,
    limit: u32,
}

impl PausedImportCapacity {
    pub(super) fn reserve(&mut self, objects: u32) -> Result<ImportObjectReservation, VmError> {
        if self
            .live_objects
            .checked_add(self.reserved.load(Ordering::Acquire))
            .and_then(|total| total.checked_add(objects))
            .is_none_or(|total| total > self.limit)
        {
            return Err(VmError::OutOfMemory {
                live_objects: self.live_objects,
                live_bytes: self.live_bytes,
            });
        }
        self.reserved.fetch_add(objects, Ordering::AcqRel);
        Ok(ImportObjectReservation {
            reserved: self.reserved.clone(),
            remaining: objects,
        })
    }

    pub(super) fn owns(&self, reservation: &ImportObjectReservation) -> bool {
        Arc::ptr_eq(&self.reserved, &reservation.reserved)
    }
}

#[derive(Debug)]
pub(super) struct Heap {
    descriptors: Vec<BytecodeTraceDescriptor>,
    slots: Vec<HeapSlot>,
    free: Vec<u32>,
    live_objects: u32,
    reserved_objects: Arc<AtomicU32>,
    live_bytes: u64,
    next_collection: u32,
    limits: VmLimits,
    budget: Option<VmMemoryBudget>,
}

struct CapacityDemand<'object> {
    objects: u32,
    bytes: u64,
    threshold_reached: bool,
    protected: Option<HeapHandle>,
    pending: Option<(BytecodeTypeId, &'object HeapObject)>,
    budget: Option<VmMemoryBudget>,
}

impl Heap {
    pub(super) fn new(limits: VmLimits, descriptors: Vec<BytecodeTraceDescriptor>) -> Self {
        Self {
            descriptors,
            slots: Vec::new(),
            free: Vec::new(),
            live_objects: 0,
            reserved_objects: Arc::new(AtomicU32::new(0)),
            live_bytes: 0,
            next_collection: limits.initial_gc_threshold.min(limits.max_heap_objects),
            limits,
            budget: None,
        }
    }

    pub(super) fn set_budget(&mut self, budget: Option<VmMemoryBudget>) {
        self.budget = budget;
    }

    pub(super) fn type_descriptor(
        &self,
        descriptor: BytecodeTypeId,
    ) -> Result<&BytecodeTraceDescriptor, VmError> {
        self.descriptors
            .get(descriptor.index() as usize)
            .ok_or_else(|| VmError::invariant("heap type descriptor is missing"))
    }

    pub(super) fn allocate(
        &mut self,
        descriptor: BytecodeTypeId,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<HeapHandle, VmError> {
        self.allocate_with_charge(descriptor, object, roots, statistics, None)
    }

    /// Check a complete response before constructing any of its heap objects.
    /// With a phase account, bytes are the additional reservation after moving
    /// already admitted String payloads; otherwise they are full heap storage.
    pub(super) fn preflight_import(
        &mut self,
        objects: u32,
        bytes: u64,
        budget: Option<VmMemoryBudget>,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        self.ensure_capacity(
            CapacityDemand {
                objects,
                bytes,
                threshold_reached: false,
                protected: None,
                pending: None,
                budget,
            },
            roots,
            statistics,
        )
    }

    /// Hold object capacity while a prepared host result waits for delivery.
    /// Other tasks can allocate only outside these held slots. Dropping a
    /// cancelled or rejected result releases its unconsumed capacity.
    pub(super) fn reserve_import_objects(
        &mut self,
        objects: u32,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<ImportObjectReservation, VmError> {
        self.ensure_capacity(
            CapacityDemand {
                objects,
                bytes: 0,
                threshold_reached: false,
                protected: None,
                pending: None,
                budget: self.budget.clone(),
            },
            roots,
            statistics,
        )?;
        // Only this heap can increase the counter; concurrently dropped
        // reservations can only make the checked capacity larger.
        self.reserved_objects.fetch_add(objects, Ordering::AcqRel);
        Ok(ImportObjectReservation {
            reserved: self.reserved_objects.clone(),
            remaining: objects,
        })
    }

    pub(super) fn pause_import_capacity(
        &mut self,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<PausedImportCapacity, VmError> {
        // The servicing thread cannot collect the worker's heap. Reclaim its
        // garbage while its verified roots and exclusive heap access are here.
        self.collect(roots, statistics)?;
        Ok(PausedImportCapacity {
            reserved: self.reserved_objects.clone(),
            live_objects: self.live_objects,
            live_bytes: self.live_bytes,
            limit: self.limits.max_heap_objects,
        })
    }

    pub(super) fn consume_import_object(
        &self,
        reservation: &mut ImportObjectReservation,
    ) -> Result<(), VmError> {
        if !self.owns_import_reservation(reservation) || reservation.remaining == 0 {
            return Err(VmError::invariant(
                "import object reservation is foreign or exhausted",
            ));
        }
        reservation.remaining -= 1;
        self.reserved_objects.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }

    pub(super) fn owns_import_reservation(&self, reservation: &ImportObjectReservation) -> bool {
        Arc::ptr_eq(&self.reserved_objects, &reservation.reserved)
    }

    /// An imported payload already owns its reservation. Admit only the
    /// additional heap storage and move the existing charge into the slot.
    pub(super) fn allocate_with_charge(
        &mut self,
        descriptor: BytecodeTypeId,
        object: HeapObject,
        roots: &[Value],
        statistics: &mut VmStatistics,
        charge: Option<VmMemoryCharge>,
    ) -> Result<HeapHandle, VmError> {
        Self::visit_object(&self.descriptors, descriptor, &object, |_| {})?;
        let bytes = object.estimated_bytes();
        let admitted = charge.as_ref().map_or(0, VmMemoryCharge::bytes);
        let growth = bytes.checked_sub(admitted).ok_or_else(|| {
            VmError::invariant("imported charge exceeds the heap object's storage")
        })?;
        let budget = charge
            .as_ref()
            .map(|charge| charge.budget().clone())
            .or_else(|| self.budget.clone());
        self.ensure_capacity(
            CapacityDemand {
                objects: 1,
                bytes: growth,
                threshold_reached: self.live_objects >= self.next_collection,
                protected: None,
                pending: Some((descriptor, &object)),
                budget: budget.clone(),
            },
            roots,
            statistics,
        )?;

        let charge = match charge {
            Some(mut charge) => {
                charge.resize(bytes)?;
                Some(charge)
            }
            None => budget
                .as_ref()
                .map(|budget| budget.reserve(bytes))
                .transpose()?,
        };

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
            slot.charge = charge;
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
                charge,
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
        let budget = self.slots[handle.index as usize]
            .charge
            .as_ref()
            .map(|charge| charge.budget().clone());
        self.ensure_capacity(
            CapacityDemand {
                objects: 0,
                bytes: growth,
                threshold_reached: false,
                protected: Some(handle),
                pending: Some((descriptor, &object)),
                budget,
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
        if let Some(charge) = &mut slot.charge {
            charge.resize(new_bytes)?;
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

    pub(super) fn trace_host_roots(
        &self,
        roots: &[Value],
        pending: Option<(BytecodeTypeId, &HeapObject)>,
        output: &mut VmHostRoots,
    ) -> Result<(), VmError> {
        let mut work = roots.to_vec();
        if let Some((descriptor, object)) = pending {
            Self::visit_object(&self.descriptors, descriptor, object, |value| {
                work.push(value.clone())
            })?;
        }
        let mut visited = BTreeSet::new();
        while let Some(value) = work.pop() {
            match value {
                Value::Host(value) => value.trace_host_roots(output),
                Value::Heap(handle) if visited.insert(handle) => {
                    Self::visit_object(
                        &self.descriptors,
                        self.descriptor(handle)?,
                        self.get(handle)?,
                        |value| work.push(value.clone()),
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn ensure_capacity(
        &mut self,
        demand: CapacityDemand<'_>,
        roots: &[Value],
        statistics: &mut VmStatistics,
    ) -> Result<(), VmError> {
        // Threshold or capacity pressure permits at most one full collection.
        // The protected handle keeps a replacement target stable until publication.
        if demand.threshold_reached
            || !self.has_capacity(demand.objects, demand.bytes, demand.budget.as_ref())
        {
            self.collect_with_pending(roots, demand.protected, demand.pending, statistics)?;
        }
        if self.has_capacity(demand.objects, demand.bytes, demand.budget.as_ref()) {
            Ok(())
        } else {
            Err(VmError::OutOfMemory {
                live_objects: self.live_objects,
                live_bytes: self.live_bytes,
            })
        }
    }

    fn has_capacity(
        &self,
        additional_objects: u32,
        additional_bytes: u64,
        budget: Option<&VmMemoryBudget>,
    ) -> bool {
        self.live_objects
            .checked_add(self.reserved_objects.load(Ordering::Acquire))
            .and_then(|total| total.checked_add(additional_objects))
            .is_some_and(|total| total <= self.limits.max_heap_objects)
            && budget.map_or_else(
                || {
                    self.live_bytes
                        .checked_add(additional_bytes)
                        .is_some_and(|total| total <= self.limits.max_heap_bytes)
                },
                |budget| budget.can_reserve(additional_bytes),
            )
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
                slot.charge = None;
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

    #[cfg(test)]
    pub(super) fn clear_type_descriptors_for_test(&mut self) {
        self.descriptors.clear();
    }

    #[cfg(test)]
    pub(super) fn set_max_heap_objects_for_test(&mut self, limit: u32) {
        self.limits.max_heap_objects = limit;
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
                        IteratorAdapter::Sync { .. } | IteratorAdapter::Take { .. } => {}
                        IteratorAdapter::Map { callback, .. }
                        | IteratorAdapter::Filter { callback, .. } => visit(callback),
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
    fn imported_heap_storage_keeps_its_owner_through_growth_and_collection() {
        let mut heap = Heap::new(VmLimits::default(), vec![BytecodeTraceDescriptor::String]);
        let mut statistics = VmStatistics::default();
        let owner = VmMemoryBudget::new(1024);
        let sibling = VmMemoryBudget::new(0);
        heap.set_budget(Some(sibling.clone()));
        let object = HeapObject::String("imported".into());
        let original_bytes = object.estimated_bytes();
        let handle = heap
            .allocate_with_charge(
                BytecodeTypeId::new(0),
                object,
                &[],
                &mut statistics,
                Some(owner.reserve(1).unwrap()),
            )
            .unwrap();
        assert_eq!(owner.live_bytes(), original_bytes);
        assert_eq!(sibling.live_bytes(), 0);
        let roots = [Value::Heap(handle)];
        let grown = HeapObject::String("imported and extended".into());
        let grown_bytes = grown.estimated_bytes();
        heap.replace(handle, grown, &roots, &mut statistics)
            .unwrap();
        assert_eq!(owner.live_bytes(), grown_bytes);
        assert_eq!(sibling.live_bytes(), 0);
        assert!(matches!(
            heap.replace(
                handle,
                HeapObject::String("x".repeat(2048)),
                &roots,
                &mut statistics
            ),
            Err(VmError::OutOfMemory { .. })
        ));
        assert!(
            matches!(heap.get(handle).unwrap(), HeapObject::String(text) if text == "imported and extended")
        );
        assert_eq!(owner.live_bytes(), grown_bytes);
        heap.collect(&roots, &mut statistics).unwrap();
        assert_eq!(owner.live_bytes(), grown_bytes);
        heap.collect(&[], &mut statistics).unwrap();
        assert_eq!(owner.live_bytes(), 0);
        assert_eq!(sibling.live_bytes(), 0);
        assert_eq!(heap.live_objects, 0);
    }

    #[test]
    fn rejected_import_charges_and_collected_roots_do_not_publish_objects() {
        let mut heap = Heap::new(VmLimits::default(), vec![BytecodeTraceDescriptor::String]);
        let mut statistics = VmStatistics::default();
        let object = HeapObject::String("payload".into());
        let budget = VmMemoryBudget::new(1024);
        let excessive = budget.reserve(object.estimated_bytes() + 1).unwrap();
        assert!(matches!(
            heap.allocate_with_charge(BytecodeTypeId::new(0), object, &[], &mut statistics, Some(excessive)),
            Err(VmError::Invariant(message)) if message.contains("imported charge exceeds")
        ));
        assert_eq!(budget.live_bytes(), 0);
        assert_eq!(heap.live_objects, 0);
        let original = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("first".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        heap.collect(&[], &mut statistics).unwrap();
        assert!(matches!(
            heap.collect(&[Value::Heap(original)], &mut statistics),
            Err(VmError::Invariant(message)) if message.contains("collected object")
        ));
        let replacement = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("second".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        assert_ne!(original, replacement);
        assert!(matches!(
            heap.collect(&[Value::Heap(original)], &mut statistics),
            Err(VmError::Invariant(message)) if message.contains("stale heap handle")
        ));
        assert!(heap.get(replacement).is_ok());
        heap.collect(&[Value::Heap(replacement)], &mut statistics)
            .unwrap();
        assert_eq!(heap.live_objects, 1);
    }

    #[test]
    fn prepared_import_object_slots_survive_other_allocations_and_release_on_drop() {
        let limits = VmLimits {
            max_heap_objects: 3,
            ..VmLimits::default()
        };
        let mut heap = Heap::new(limits, vec![BytecodeTraceDescriptor::String]);
        let mut stats = VmStatistics::default();
        let mut reservation = heap.reserve_import_objects(2, &[], &mut stats).unwrap();
        let first = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("first".into()),
                &[],
                &mut stats,
            )
            .unwrap();
        let mut roots = vec![Value::Heap(first)];
        assert!(matches!(
            heap.allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("no slot".into()),
                &roots,
                &mut stats
            ),
            Err(VmError::OutOfMemory { .. })
        ));
        assert_eq!(heap.live_objects, 1);
        assert_eq!(heap.reserved_objects.load(Ordering::Acquire), 2);
        let foreign = Heap::new(limits, vec![BytecodeTraceDescriptor::String]);
        assert!(foreign.consume_import_object(&mut reservation).is_err());
        assert_eq!(reservation.remaining, 2);
        for expected in [2, 3] {
            heap.consume_import_object(&mut reservation).unwrap();
            let object = heap
                .allocate(
                    BytecodeTypeId::new(0),
                    HeapObject::String("imported".into()),
                    &roots,
                    &mut stats,
                )
                .unwrap();
            roots.push(Value::Heap(object));
            assert_eq!(heap.live_objects, expected);
        }
        assert!(heap.consume_import_object(&mut reservation).is_err());
        drop(reservation);
        assert_eq!(heap.reserved_objects.load(Ordering::Acquire), 0);
        roots.clear();
        let cancelled = heap.reserve_import_objects(3, &roots, &mut stats).unwrap();
        assert_eq!(heap.live_objects, 0);
        assert!(matches!(
            heap.reserve_import_objects(1, &roots, &mut stats),
            Err(VmError::OutOfMemory { .. })
        ));
        drop(cancelled);
        assert_eq!(heap.reserved_objects.load(Ordering::Acquire), 0);
        let after_drop = heap.reserve_import_objects(3, &roots, &mut stats).unwrap();
        let counter = heap.reserved_objects.clone();
        drop(heap);
        drop(after_drop);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn recycled_generation_never_exposes_the_reserved_zero_handle() {
        let mut heap = Heap::new(VmLimits::default(), vec![BytecodeTraceDescriptor::String]);
        heap.free.push(1);
        assert!(matches!(
            heap.allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("invalid-free-slot".into()),
                &[],
                &mut VmStatistics::default(),
            ),
            Err(VmError::Invariant(message))
                if message == "heap free list contains an invalid slot"
        ));

        heap.slots.push(HeapSlot {
            generation: u32::MAX,
            marked: false,
            descriptor: BytecodeTypeId::new(0),
            object: None,
            bytes: 0,
            charge: None,
        });
        heap.free.push(0);

        let handle = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("recycled".into()),
                &[],
                &mut VmStatistics::default(),
            )
            .unwrap();

        assert_eq!(handle.index, 0);
        assert_eq!(handle.generation, 1);
    }

    #[test]
    fn diagnostic_id_is_stable_for_a_heap_handle() {
        let handle = HeapHandle {
            index: 0x1234_5678,
            generation: 0x9abc_def0,
        };

        assert_eq!(handle.diagnostic_id(), 0x1234_5678_9abc_def0);
    }

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
