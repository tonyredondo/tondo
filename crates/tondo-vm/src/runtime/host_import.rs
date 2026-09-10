//! Borrowed admission of a complete typed host response before heap construction.

use super::*;
use crate::bytecode::BytecodeNominalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ImportCost {
    pub objects: u32,
    pub bounded: bool,
    pub heap_bytes: u64,
    pub string_bytes: u64,
    pub frames: usize,
}

impl ImportCost {
    pub(super) fn prepared_bytes(self, limit: u64) -> Result<(u64, u64), VmError> {
        let overflow = || VmError::ResourceLimit {
            resource: "memory",
            limit,
        };
        let workspace = (self.frames as u64)
            .checked_mul(super::super::TEST_HOST_IMPORT_FRAME_BYTES)
            .and_then(|bytes| bytes.checked_add(HOST_IMPORT_ADMISSION_BYTES))
            .ok_or_else(overflow)?;
        let additional = self
            .heap_bytes
            .checked_sub(self.string_bytes)
            .and_then(|bytes| bytes.checked_add(workspace))
            .ok_or_else(overflow)?;
        Ok((additional, workspace))
    }
}

/// VM storage held before dispatching a host mutation. Detached reply bytes
/// keep their existing host admission; String payloads transfer on delivery.
#[derive(Debug)]
pub struct PreparedHostImport {
    pub(super) ty: BytecodeTypeId,
    pub(super) cost: ImportCost,
    pub(super) heap: VmMemoryCharge,
    pub(super) workspace: VmMemoryCharge,
    pub(super) objects: super::super::heap::ImportObjectReservation,
    pub(super) recipient: Option<ImportRecipient>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportRecipient {
    Pending { task: usize, call: u64 },
    Current { task: usize },
}

impl ImportRecipient {
    pub(super) fn task(self) -> usize {
        match self {
            Self::Pending { task, .. } | Self::Current { task } => task,
        }
    }
}

pub(super) trait HostImportPlanner {
    fn pause_current(&mut self) -> Result<Option<super::worker_import::PausedHostImport>, VmError>;
    fn prepare(
        &mut self,
        call: u64,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError>;
    fn prepare_current(
        &mut self,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError>;
    fn commit(&mut self, prepared: &mut [Option<PreparedHostImport>]) -> Result<(), VmError>;
}

/// Scoped access to the waiting VM's result admission. A host holds tentative
/// reservations while checking transport and storage, then publishes them
/// together before committing an operation that completes multiple calls.
pub struct VmHostImportAdmission<'a> {
    planner: Option<&'a mut dyn HostImportPlanner>,
}

impl<'a> VmHostImportAdmission<'a> {
    /// Reference hosts outside a VM retain their existing transport-only route.
    pub const fn disabled() -> Self {
        Self { planner: None }
    }

    pub(super) fn new(planner: &'a mut dyn HostImportPlanner) -> Self {
        Self {
            planner: Some(planner),
        }
    }

    pub(super) fn pause_current(
        &mut self,
    ) -> Result<Option<super::worker_import::PausedHostImport>, VmError> {
        self.planner
            .as_mut()
            .map_or(Ok(None), |planner| planner.pause_current())
    }

    /// Reserve a result without changing either endpoint. Dropping the returned
    /// token releases every tentative VM byte and heap object slot.
    pub fn prepare(
        &mut self,
        call: u64,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        self.planner
            .as_mut()
            .map_or(Ok(None), |planner| planner.prepare(call, preview))
    }

    /// Reserve the synchronous caller's result at the host's commit point.
    /// The receiving VM supplies its verified result type and original account.
    pub fn prepare_current(
        &mut self,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        self.planner
            .as_mut()
            .map_or(Ok(None), |planner| planner.prepare_current(preview))
    }

    /// Validate the complete set before publishing any reservation. Tokens are
    /// consumed only on success; failure leaves them available for RAII cleanup.
    pub fn commit(&mut self, prepared: &mut [Option<PreparedHostImport>]) -> Result<(), VmError> {
        match &mut self.planner {
            Some(planner) => planner.commit(prepared),
            None if prepared.iter().all(Option::is_none) => Ok(()),
            None => Err(VmError::invariant(
                "VM import reservation has no receiving planner",
            )),
        }
    }
}

pub(crate) const HOST_IMPORT_ADMISSION_BYTES: u64 =
    std::mem::size_of::<PreparedHostImport>() as u64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportRoute {
    HostReply,
    BlockingArgument,
}

#[derive(Clone, Copy)]
enum Shape {
    Tuple,
    Array,
    Map,
    Set,
    Some,
    Ok,
    Err,
    Ref,
    Union(BytecodeTypeId),
    Newtype(BytecodeNominalId),
    Record(BytecodeNominalId),
    Closure(BytecodeCallableId),
    Variant {
        member: u32,
        ordinal: usize,
        payload: Payload,
    },
}

#[derive(Clone, Copy)]
enum Payload {
    Unit,
    Tuple,
    Record,
}

enum Source {
    One(Option<RuntimeValue>),
    Values(std::vec::IntoIter<RuntimeValue>),
    Map {
        entries: std::vec::IntoIter<(RuntimeValue, RuntimeValue)>,
        pending: Option<RuntimeValue>,
    },
}

impl Source {
    fn next(&mut self) -> Option<RuntimeValue> {
        match self {
            Self::One(value) => value.take(),
            Self::Values(values) => values.next(),
            Self::Map { entries, pending } => {
                if let Some(value) = pending.take() {
                    return Some(value);
                }
                let (key, value) = entries.next()?;
                *pending = Some(value);
                Some(key)
            }
        }
    }
}

enum Output {
    One(Option<Value>),
    Values(Vec<Option<Value>>),
    Fields(Vec<(u32, Option<Value>)>),
    Map {
        entries: Vec<(Option<Value>, Option<Value>)>,
        key: Option<Value>,
    },
}

/// Inputs keep their original buffers. Outputs use the already admitted heap
/// storage, and partial managed values remain visible through Engine::roots.
pub(crate) struct ImportFrame {
    representation: BytecodeTypeId,
    descriptor: BytecodeTypeId,
    shape: Shape,
    source: Source,
    output: Output,
    next: usize,
}

impl ImportFrame {
    fn next_child(
        &mut self,
        engine: &Engine<'_, '_>,
    ) -> Result<Option<(BytecodeTypeId, RuntimeValue)>, VmError> {
        let Some(value) = self.source.next() else {
            return Ok(None);
        };
        let trace = engine.heap.type_descriptor(self.representation)?;
        let index = self.next;
        self.next += 1;
        let ty = match (trace, self.shape) {
            (BytecodeTraceDescriptor::Tuple { fields }, Shape::Tuple) => fields[index],
            (BytecodeTraceDescriptor::Closure { captures, .. }, Shape::Closure(_)) => {
                captures[index]
            }
            (BytecodeTraceDescriptor::Array { element }, Shape::Array)
            | (BytecodeTraceDescriptor::Set { element }, Shape::Set) => *element,
            (BytecodeTraceDescriptor::Map { key, value }, Shape::Map) => {
                if index.is_multiple_of(2) {
                    *key
                } else {
                    *value
                }
            }
            (BytecodeTraceDescriptor::Option { value }, Shape::Some)
            | (BytecodeTraceDescriptor::Result { success: value, .. }, Shape::Ok)
            | (BytecodeTraceDescriptor::Result { error: value, .. }, Shape::Err)
            | (BytecodeTraceDescriptor::Ref { value }, Shape::Ref) => *value,
            (BytecodeTraceDescriptor::Newtype { value, .. }, Shape::Newtype(_)) => *value,
            (_, Shape::Union(member)) => member,
            (BytecodeTraceDescriptor::Record { fields, .. }, Shape::Record(_)) => fields[index].ty,
            (BytecodeTraceDescriptor::Variant { variants, .. }, Shape::Variant { ordinal, .. }) => {
                match &variants[ordinal].payload {
                    BytecodeVariantPayload::Tuple(types) => types[index],
                    BytecodeVariantPayload::Record(fields) => fields[index].ty,
                    BytecodeVariantPayload::Unit => {
                        return Err(VmError::invariant("unit import has a child"));
                    }
                }
            }
            _ => {
                return Err(VmError::invariant(
                    "prepared import lost its child descriptor",
                ));
            }
        };
        Ok(Some((ty, value)))
    }

    fn accept(&mut self, engine: &Engine<'_, '_>, value: Value) -> Result<(), VmError> {
        match &mut self.output {
            Output::One(slot) => *slot = Some(value),
            Output::Values(values) => values.push(Some(value)),
            Output::Map { entries, key } => {
                if let Some(key) = key.take() {
                    entries.push((Some(key), Some(value)));
                } else {
                    *key = Some(value);
                }
            }
            Output::Fields(fields) => {
                let schema = match (
                    engine.heap.type_descriptor(self.representation)?,
                    self.shape,
                ) {
                    (BytecodeTraceDescriptor::Record { fields, .. }, Shape::Record(_)) => fields,
                    (
                        BytecodeTraceDescriptor::Variant { variants, .. },
                        Shape::Variant { ordinal, .. },
                    ) => {
                        let BytecodeVariantPayload::Record(fields) = &variants[ordinal].payload
                        else {
                            return Err(VmError::invariant("prepared record payload changed"));
                        };
                        fields
                    }
                    _ => return Err(VmError::invariant("prepared record descriptor changed")),
                };
                fields.push((schema[fields.len()].member, Some(value)));
            }
        }
        Ok(())
    }

    pub(super) fn trace_values(&self, roots: &mut Vec<Value>) {
        match &self.output {
            Output::One(value) => roots.extend(value.iter().cloned()),
            Output::Values(values) => roots.extend(values.iter().flatten().cloned()),
            Output::Fields(fields) => {
                roots.extend(fields.iter().filter_map(|(_, value)| value.clone()))
            }
            Output::Map { entries, key } => {
                roots.extend(key.iter().cloned());
                roots.extend(
                    entries
                        .iter()
                        .flat_map(|(key, value)| key.iter().chain(value).cloned()),
                );
            }
        }
    }

    fn finish(self, engine: &mut Engine<'_, '_>) -> Result<Value, VmError> {
        let object = match (self.shape, self.output) {
            (Shape::Tuple, Output::Values(values)) => HeapObject::Tuple(values),
            (Shape::Array, Output::Values(values)) => HeapObject::Array(values.into()),
            (Shape::Set, Output::Values(values)) => HeapObject::Set(values.into()),
            (Shape::Map, Output::Map { entries, key: None }) => HeapObject::Map(entries.into()),
            (Shape::Some, Output::One(value)) => HeapObject::OptionSome(value),
            (Shape::Ok, Output::One(value)) => HeapObject::ResultOk(value),
            (Shape::Err, Output::One(value)) => HeapObject::ResultErr(value),
            (Shape::Ref, Output::One(value)) => HeapObject::Ref(value),
            (Shape::Union(member), Output::One(value)) => HeapObject::Union { member, value },
            (Shape::Newtype(nominal), Output::One(value)) => HeapObject::Newtype { nominal, value },
            (Shape::Record(nominal), Output::Fields(fields)) => {
                HeapObject::Record { nominal, fields }
            }
            (Shape::Closure(callable), Output::Values(captures)) => {
                HeapObject::Closure { callable, captures }
            }
            (
                Shape::Variant {
                    member,
                    payload: Payload::Unit,
                    ..
                },
                Output::Values(values),
            ) if values.is_empty() => HeapObject::Variant {
                variant: member,
                payload: AggregatePayload::Unit,
            },
            (
                Shape::Variant {
                    member,
                    payload: Payload::Tuple,
                    ..
                },
                Output::Values(values),
            ) => HeapObject::Variant {
                variant: member,
                payload: AggregatePayload::Tuple(values),
            },
            (
                Shape::Variant {
                    member,
                    payload: Payload::Record,
                    ..
                },
                Output::Fields(fields),
            ) => HeapObject::Variant {
                variant: member,
                payload: AggregatePayload::Record(fields),
            },
            _ => {
                return Err(VmError::invariant(
                    "prepared import produced the wrong storage shape",
                ));
            }
        };
        engine.allocate_host_import(self.descriptor, object, &[])
    }
}

impl Engine<'_, '_> {
    // The inner result separates a parent frame from an unchanged leaf without
    // allocating a box for either; only the outer result represents failure.
    fn import_frame(
        &self,
        representation: BytecodeTypeId,
        descriptor: BytecodeTypeId,
        value: RuntimeValue,
    ) -> Result<Result<ImportFrame, RuntimeValue>, VmError> {
        let trace = self.heap.type_descriptor(representation)?;
        let (shape, source, output) = match value {
            RuntimeValue::Closure { callable, captures } => (
                Shape::Closure(BytecodeCallableId::new(callable)),
                Source::Values(captures.into_iter()),
                Output::Values(Vec::new()),
            ),
            RuntimeValue::Tuple(values) => (
                Shape::Tuple,
                Source::Values(values.into_iter()),
                Output::Values(Vec::new()),
            ),
            RuntimeValue::Array(values) => (
                Shape::Array,
                Source::Values(values.into_iter()),
                Output::Values(Vec::new()),
            ),
            RuntimeValue::Set(values) => (
                Shape::Set,
                Source::Values(values.into_iter()),
                Output::Values(Vec::new()),
            ),
            RuntimeValue::Map(entries) => {
                let output = Output::Map {
                    entries: Vec::with_capacity(entries.len()),
                    key: None,
                };
                (
                    Shape::Map,
                    Source::Map {
                        entries: entries.into_iter(),
                        pending: None,
                    },
                    output,
                )
            }
            RuntimeValue::OptionSome(value) => {
                (Shape::Some, Source::One(Some(*value)), Output::One(None))
            }
            RuntimeValue::ResultOk(value) => {
                (Shape::Ok, Source::One(Some(*value)), Output::One(None))
            }
            RuntimeValue::ResultErr(value) => {
                (Shape::Err, Source::One(Some(*value)), Output::One(None))
            }
            RuntimeValue::Ref(Some(value)) => {
                (Shape::Ref, Source::One(Some(*value)), Output::One(None))
            }
            RuntimeValue::Union { member, value } => (
                Shape::Union(BytecodeTypeId::new(member)),
                Source::One(Some(*value)),
                Output::One(None),
            ),
            RuntimeValue::Newtype { value, .. } => {
                let BytecodeTraceDescriptor::Newtype { nominal, .. } = trace else {
                    return Err(VmError::invariant("prepared newtype changed"));
                };
                (
                    Shape::Newtype(*nominal),
                    Source::One(Some(*value)),
                    Output::One(None),
                )
            }
            RuntimeValue::Record { values, .. } => {
                let BytecodeTraceDescriptor::Record { nominal, .. } = trace else {
                    return Err(VmError::invariant("prepared record changed"));
                };
                let output = Output::Fields(Vec::with_capacity(values.len()));
                (
                    Shape::Record(*nominal),
                    Source::Values(values.into_iter()),
                    output,
                )
            }
            RuntimeValue::Variant {
                variant, values, ..
            } => {
                let BytecodeTraceDescriptor::Variant { variants, .. } = trace else {
                    return Err(VmError::invariant("prepared variant changed"));
                };
                let schema = &variants[variant as usize];
                let (payload, output) = match schema.payload {
                    BytecodeVariantPayload::Unit => (Payload::Unit, Output::Values(Vec::new())),
                    BytecodeVariantPayload::Tuple(_) => {
                        (Payload::Tuple, Output::Values(Vec::new()))
                    }
                    BytecodeVariantPayload::Record(_) => (
                        Payload::Record,
                        Output::Fields(Vec::with_capacity(values.len())),
                    ),
                };
                (
                    Shape::Variant {
                        member: schema.member,
                        ordinal: variant as usize,
                        payload,
                    },
                    Source::Values(values.into_iter()),
                    output,
                )
            }
            value => return Ok(Err(value)),
        };
        let output = match (&source, output) {
            (Source::Values(values), Output::Values(_)) => {
                Output::Values(Vec::with_capacity(values.len()))
            }
            (_, output) => output,
        };
        Ok(Ok(ImportFrame {
            representation,
            descriptor,
            shape,
            source,
            output,
            next: 0,
        }))
    }

    pub(super) fn materialize_host_iterative(
        &mut self,
        ty: BytecodeTypeId,
        value: RuntimeValue,
    ) -> Result<Value, VmError> {
        let mut current = Some((ty, value));
        'import: loop {
            let (descriptor, value) = current.take().expect("an import cursor is ready");
            let mut representation = descriptor;
            while let BytecodeTypeKind::OpaqueResult { witness, .. } = self
                .program
                .ty(representation)
                .ok_or_else(|| VmError::invariant("import witness disappeared"))?
                .kind
            {
                representation = witness;
            }
            let resource = match &value {
                RuntimeValue::Host { kind, id } => Some((*kind, *id)),
                _ => None,
            };
            let mut result = match self.import_frame(representation, descriptor, value)? {
                Ok(mut frame) => {
                    if let Some(child) = frame.next_child(self)? {
                        self.import_frames.push(frame);
                        current = Some(child);
                        continue 'import;
                    }
                    frame.finish(self)?
                }
                Err(value) => {
                    let value = match value {
                        RuntimeValue::Function {
                            name,
                            type_arguments,
                        } => {
                            let callable = self.blocking_function_callable(&name)?;
                            Value::Function {
                                callable,
                                arguments: type_arguments
                                    .into_iter()
                                    .map(BytecodeTypeId::new)
                                    .collect(),
                            }
                        }
                        value => {
                            self.materialize_host_value_as(representation, descriptor, value)?
                        }
                    };
                    if let Some((kind, id)) = resource {
                        self.record_resource(
                            &RuntimeValue::Host { kind, id },
                            DiagnosticResourceState::Acquired,
                        )?;
                    }
                    value
                }
            };
            while let Some(mut frame) = self.import_frames.pop() {
                frame.accept(self, result)?;
                if let Some(child) = frame.next_child(self)? {
                    self.import_frames.push(frame);
                    current = Some(child);
                    continue 'import;
                }
                result = frame.finish(self)?;
            }
            return Ok(result);
        }
    }
}

enum Children<'a> {
    None,
    One(BytecodeTypeId, &'a RuntimeValue),
    Optional(BytecodeTypeId, Option<&'a RuntimeValue>),
    Variant(BytecodeTypeId, u32, PreviewPayload<'a>),
    Same(BytecodeTypeId, &'a [RuntimeValue]),
    SameParts(BytecodeTypeId, &'a [RuntimeValue], &'a [RuntimeValue]),
    Tuple(&'a [BytecodeTypeId], &'a [RuntimeValue]),
    Record(&'a [crate::bytecode::BytecodeField], &'a [RuntimeValue]),
    Map(
        BytecodeTypeId,
        BytecodeTypeId,
        &'a [(RuntimeValue, RuntimeValue)],
    ),
}

const _: () = assert!(
    std::mem::size_of::<(Children<'static>, usize)>()
        <= super::super::TEST_DETACHED_WALK_FRAME_BYTES as usize
);

impl<'a> Children<'a> {
    fn at(&self, index: usize) -> Option<Child<'a>> {
        match self {
            Self::None => None,
            Self::One(ty, value) => (index == 0).then_some((*ty, *value)),
            Self::Optional(ty, value) => {
                return (index == 0).then_some(Child::Optional(*ty, *value));
            }
            Self::Variant(ty, variant, payload) => {
                return (index == 0).then_some(Child::Variant(*ty, *variant, *payload));
            }
            Self::Same(ty, values) => values.get(index).map(|value| (*ty, value)),
            Self::SameParts(ty, first, second) => {
                let value = if index < first.len() {
                    first.get(index)
                } else {
                    second.get(index - first.len())
                };
                value.map(|value| (*ty, value))
            }
            Self::Tuple(types, values) => values.get(index).map(|value| (types[index], value)),
            Self::Record(fields, values) => {
                values.get(index).map(|value| (fields[index].ty, value))
            }
            Self::Map(key, value, entries) => entries.get(index / 2).map(|entry| {
                if index.is_multiple_of(2) {
                    (*key, &entry.0)
                } else {
                    (*value, &entry.1)
                }
            }),
        }
        .map(|(ty, value)| Child::Value(ty, value))
    }
}

enum Child<'a> {
    Value(BytecodeTypeId, &'a RuntimeValue),
    Optional(BytecodeTypeId, Option<&'a RuntimeValue>),
    Variant(BytecodeTypeId, u32, PreviewPayload<'a>),
}

#[derive(Clone, Copy)]
enum PreviewPayload<'a> {
    Value(&'a RuntimeValue),
    Optional(Option<&'a RuntimeValue>),
}

struct Node<'a> {
    bytes: u64,
    string_bytes: u64,
    children: Children<'a>,
}

impl Engine<'_, '_> {
    /// The caller collects host roots before this borrowed preview and dispatches
    /// without another collection between reservation and the host effect.
    pub(super) fn prepare_host_return_import(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        outcome: BytecodeTypeId,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        let Some(budget) = self.current_test_memory() else {
            return Ok(None);
        };
        let Some(preview) = self.host.preview_return(name, arguments)? else {
            return Ok(None);
        };
        let cost = self.host_import_preview_cost(outcome, preview, Some(&budget))?;
        self.reserve_prepared_host_import(outcome, cost, budget)
            .map(Some)
    }

    pub(super) fn prepare_polled_host_return_import(
        &mut self,
        call: u64,
        outcome: BytecodeTypeId,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        let Some(budget) = self.current_test_memory() else {
            return Ok(None);
        };
        let Some(preview) = self.host.preview_polled_return(call)? else {
            return Ok(None);
        };
        let cost = self.host_import_preview_cost(outcome, preview, Some(&budget))?;
        self.reserve_prepared_host_import(outcome, cost, budget)
            .map(Some)
    }

    fn reserve_prepared_host_import(
        &mut self,
        outcome: BytecodeTypeId,
        cost: ImportCost,
        budget: VmMemoryBudget,
    ) -> Result<PreparedHostImport, VmError> {
        let roots = self.roots(&[])?;
        reserve_prepared_import(
            &mut self.heap,
            &mut self.statistics,
            &roots,
            outcome,
            cost,
            budget,
        )
    }

    pub(super) fn host_import_types(&self) -> HostImportTypes<'_> {
        HostImportTypes {
            program: self.program,
            heap: &self.heap,
            limits: &self.limits,
            nominal_names: &self.nominal_names,
        }
    }

    pub(super) fn host_import_cost(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        budget: Option<&VmMemoryBudget>,
    ) -> Result<ImportCost, VmError> {
        self.host_import_types().host_import_cost(ty, value, budget)
    }

    pub(super) fn host_import_cost_for(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        budget: Option<&VmMemoryBudget>,
        route: ImportRoute,
    ) -> Result<ImportCost, VmError> {
        self.host_import_types()
            .host_import_cost_for(ty, value, budget, route)
    }

    pub(super) fn host_import_preview_cost(
        &self,
        ty: BytecodeTypeId,
        preview: super::super::VmHostReturnPreview<'_>,
        budget: Option<&VmMemoryBudget>,
    ) -> Result<ImportCost, VmError> {
        self.host_import_types()
            .host_import_preview_cost(ty, preview, budget)
    }

    pub(super) fn validate_prepared_host_import(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        prepared: &PreparedHostImport,
    ) -> Result<ImportCost, VmError> {
        self.host_import_types()
            .validate_prepared_host_import(ty, value, prepared)
    }

    fn blocking_function_callable(&self, name: &str) -> Result<BytecodeCallableId, VmError> {
        self.host_import_types().blocking_function_callable(name)
    }
}

/// Read-only typed layout for a borrowed response. Keeping host dispatch out
/// of this view permits admission while a host is preparing an atomic effect.
pub(super) struct HostImportTypes<'a> {
    pub program: &'a BytecodeProgram,
    pub heap: &'a Heap,
    pub limits: &'a VmLimits,
    pub nominal_names: &'a [String],
}

impl HostImportTypes<'_> {
    pub(super) fn host_import_cost(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        budget: Option<&VmMemoryBudget>,
    ) -> Result<ImportCost, VmError> {
        self.host_import_cost_for(ty, value, budget, ImportRoute::HostReply)
    }

    pub(super) fn host_import_cost_for(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        budget: Option<&VmMemoryBudget>,
        route: ImportRoute,
    ) -> Result<ImportCost, VmError> {
        let root = self.host_import_node(ty, value, route)?;
        self.host_import_cost_from_node(root, budget, route, super::super::TEST_SNAPSHOT_MAX_DEPTH)
    }

    pub(super) fn host_import_preview_cost(
        &self,
        ty: BytecodeTypeId,
        preview: super::super::VmHostReturnPreview<'_>,
        budget: Option<&VmMemoryBudget>,
    ) -> Result<ImportCost, VmError> {
        if let super::super::VmHostReturnPreview::StorageBound(outcomes) = preview {
            if outcomes.is_empty()
                || outcomes.len() > 16
                || outcomes.iter().any(|outcome| {
                    matches!(outcome, super::super::VmHostReturnPreview::StorageBound(_))
                })
            {
                return Err(VmError::Host(
                    "host storage bound requires 1..=16 non-nested outcomes".into(),
                ));
            }
            let mut maximum = ImportCost {
                objects: 0,
                bounded: true,
                heap_bytes: 0,
                string_bytes: 0,
                frames: 0,
            };
            let mut structural_bytes = 0;
            for outcome in outcomes {
                let cost = self.host_import_preview_cost(ty, *outcome, budget)?;
                maximum.objects = maximum.objects.max(cost.objects);
                maximum.frames = maximum.frames.max(cost.frames);
                maximum.string_bytes = maximum.string_bytes.max(cost.string_bytes);
                structural_bytes = structural_bytes.max(cost.heap_bytes - cost.string_bytes);
            }
            // Structural storage and transferred String payloads can peak in
            // different outcomes; subtracting maxima would under-reserve.
            maximum.heap_bytes = structural_bytes.checked_add(maximum.string_bytes).ok_or(
                VmError::ResourceLimit {
                    resource: "memory",
                    limit: self.limits.max_heap_bytes,
                },
            )?;
            return Ok(maximum);
        }
        let root = match preview {
            super::super::VmHostReturnPreview::StorageBound(_) => unreachable!("handled above"),
            super::super::VmHostReturnPreview::Value(value) => {
                self.host_import_node(ty, value, ImportRoute::HostReply)?
            }
            super::super::VmHostReturnPreview::ArrayParts { first, second } => {
                let BytecodeTraceDescriptor::Array { element } = self.heap.type_descriptor(ty)?
                else {
                    return Err(VmError::Host(
                        "array preview has a non-array result type".into(),
                    ));
                };
                let count =
                    first
                        .len()
                        .checked_add(second.len())
                        .ok_or(VmError::ResourceLimit {
                            resource: "memory",
                            limit: self.limits.max_heap_bytes,
                        })?;
                Node {
                    bytes: self
                        .host_import_storage_bytes(count, std::mem::size_of::<Option<Value>>())?,
                    string_bytes: 0,
                    children: Children::SameParts(*element, first, second),
                }
            }
            super::super::VmHostReturnPreview::Variant(variant, value) => {
                self.host_variant_import_node(ty, variant, PreviewPayload::Value(value))?
            }
            super::super::VmHostReturnPreview::OptionSome(value) => {
                self.host_optional_import_node(ty, Some(value))?
            }
            super::super::VmHostReturnPreview::ResultOkVariant(variant, value) => Node {
                bytes: std::mem::size_of::<HeapObject>() as u64,
                string_bytes: 0,
                children: Children::Variant(
                    self.host_result_import_type(ty)?,
                    variant,
                    PreviewPayload::Value(value),
                ),
            },
            super::super::VmHostReturnPreview::ResultOkVariantOption(variant, value) => Node {
                bytes: std::mem::size_of::<HeapObject>() as u64,
                string_bytes: 0,
                children: Children::Variant(
                    self.host_result_import_type(ty)?,
                    variant,
                    PreviewPayload::Optional(value),
                ),
            },
            super::super::VmHostReturnPreview::ResultOkOption(value) => {
                let success = self.host_result_import_type(ty)?;
                Node {
                    bytes: std::mem::size_of::<HeapObject>() as u64,
                    string_bytes: 0,
                    children: Children::Optional(success, value),
                }
            }
            super::super::VmHostReturnPreview::ResultOk(value) => {
                let success = self.host_result_import_type(ty)?;
                Node {
                    bytes: std::mem::size_of::<HeapObject>() as u64,
                    string_bytes: 0,
                    children: Children::One(success, value),
                }
            }
        };
        self.host_import_cost_from_node(
            root,
            budget,
            ImportRoute::HostReply,
            super::super::TEST_SNAPSHOT_MAX_DEPTH,
        )
    }

    pub(super) fn validate_prepared_host_import(
        &self,
        ty: BytecodeTypeId,
        value: &RuntimeValue,
        prepared: &PreparedHostImport,
    ) -> Result<ImportCost, VmError> {
        if ty != prepared.ty {
            return Err(VmError::Host("host reply changed its prepared type".into()));
        }
        // This traversal borrows the workspace already held by the admission.
        // Check the depth before growing its stack, including malformed replies.
        let root = self.host_import_node(ty, value, ImportRoute::HostReply)?;
        let actual = self.host_import_cost_from_node(
            root,
            None,
            ImportRoute::HostReply,
            prepared.cost.frames,
        )?;
        let fits = if prepared.cost.bounded {
            actual.objects <= prepared.cost.objects
                && actual.frames <= prepared.cost.frames
                && actual.string_bytes <= prepared.cost.string_bytes
                && actual.heap_bytes - actual.string_bytes
                    <= prepared.cost.heap_bytes - prepared.cost.string_bytes
        } else {
            actual == prepared.cost
        };
        if !fits {
            return Err(VmError::Host(
                "host reply changed its prepared storage shape".into(),
            ));
        }
        Ok(actual)
    }

    fn host_result_import_type(&self, ty: BytecodeTypeId) -> Result<BytecodeTypeId, VmError> {
        match self.heap.type_descriptor(ty)? {
            BytecodeTraceDescriptor::Result { success, .. } => Ok(*success),
            _ => Err(VmError::Host(
                "host result preview requires a Result type".into(),
            )),
        }
    }

    fn host_variant_import_node<'a>(
        &self,
        ty: BytecodeTypeId,
        variant: u32,
        payload: PreviewPayload<'a>,
    ) -> Result<Node<'a>, VmError> {
        let mismatch = || VmError::Host("host enum preview requires a single tuple payload".into());
        let BytecodeTraceDescriptor::Variant { variants, .. } = self.heap.type_descriptor(ty)?
        else {
            return Err(mismatch());
        };
        let BytecodeVariantPayload::Tuple(types) =
            &variants.get(variant as usize).ok_or_else(mismatch)?.payload
        else {
            return Err(mismatch());
        };
        let [value_type] = types.as_slice() else {
            return Err(mismatch());
        };
        Ok(Node {
            bytes: (std::mem::size_of::<HeapObject>() + std::mem::size_of::<Option<Value>>())
                as u64,
            string_bytes: 0,
            children: match payload {
                PreviewPayload::Value(value) => Children::One(*value_type, value),
                PreviewPayload::Optional(value) => Children::Optional(*value_type, value),
            },
        })
    }

    fn host_optional_import_node<'a>(
        &self,
        ty: BytecodeTypeId,
        value: Option<&'a RuntimeValue>,
    ) -> Result<Node<'a>, VmError> {
        let BytecodeTraceDescriptor::Option { value: element } = self.heap.type_descriptor(ty)?
        else {
            return Err(VmError::Host(
                "host optional preview requires an Option type".into(),
            ));
        };
        Ok(Node {
            bytes: std::mem::size_of::<HeapObject>() as u64,
            string_bytes: 0,
            children: value.map_or(Children::None, |value| Children::One(*element, value)),
        })
    }

    fn host_import_child<'a>(
        &'a self,
        child: Child<'a>,
        route: ImportRoute,
    ) -> Result<Node<'a>, VmError> {
        match child {
            Child::Value(ty, value) => self.host_import_node(ty, value, route),
            Child::Optional(ty, value) => self.host_optional_import_node(ty, value),
            Child::Variant(ty, variant, payload) => {
                self.host_variant_import_node(ty, variant, payload)
            }
        }
    }

    fn host_import_cost_from_node<'a>(
        &'a self,
        root: Node<'a>,
        budget: Option<&VmMemoryBudget>,
        route: ImportRoute,
        max_frames: usize,
    ) -> Result<ImportCost, VmError> {
        let limit = budget.map_or(self.limits.max_heap_bytes, VmMemoryBudget::limit);
        let overflow = || VmError::ResourceLimit {
            resource: "memory",
            limit,
        };
        let mut memory = budget.map(|budget| budget.reserve(0)).transpose()?;
        let mut parents: Vec<(Children<'_>, usize)> = Vec::new();
        let mut current = Some(root);
        let mut cost = ImportCost {
            objects: 0,
            bounded: false,
            heap_bytes: 0,
            string_bytes: 0,
            frames: 0,
        };
        while let Some(node) = current.take() {
            cost.heap_bytes = cost
                .heap_bytes
                .checked_add(node.bytes)
                .ok_or_else(overflow)?;
            cost.string_bytes = cost
                .string_bytes
                .checked_add(node.string_bytes)
                .ok_or_else(overflow)?;
            if node.bytes != 0 {
                cost.objects = cost.objects.checked_add(1).ok_or_else(overflow)?;
            }
            if let Some(child) = node.children.at(0) {
                if parents.len() == max_frames {
                    return Err(VmError::ResourceLimit {
                        resource: "host import depth",
                        limit: max_frames as u64,
                    });
                }
                if let Some(memory) = &mut memory {
                    memory.resize(
                        (parents.len() as u64 + 1) * super::super::TEST_DETACHED_WALK_FRAME_BYTES,
                    )?;
                }
                parents.push((node.children, 1));
                cost.frames = cost.frames.max(parents.len());
                current = Some(self.host_import_child(child, route)?);
                continue;
            }
            while let Some((children, next)) = parents.last_mut() {
                if let Some(child) = children.at(*next) {
                    *next += 1;
                    current = Some(self.host_import_child(child, route)?);
                    break;
                }
                parents.pop();
                if let Some(memory) = &mut memory {
                    memory.resize(
                        parents.len() as u64 * super::super::TEST_DETACHED_WALK_FRAME_BYTES,
                    )?;
                }
            }
        }
        Ok(cost)
    }

    fn host_import_storage_bytes(&self, count: usize, slot_bytes: usize) -> Result<u64, VmError> {
        (count as u64)
            .checked_mul(slot_bytes as u64)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<HeapObject>() as u64))
            .ok_or(VmError::ResourceLimit {
                resource: "memory",
                limit: self.limits.max_heap_bytes,
            })
    }

    fn host_import_node<'a>(
        &'a self,
        mut ty: BytecodeTypeId,
        value: &'a RuntimeValue,
        route: ImportRoute,
    ) -> Result<Node<'a>, VmError> {
        let mismatch = || {
            VmError::Host("bootstrap host result does not match its verified return type".into())
        };
        let base = std::mem::size_of::<HeapObject>() as u64;
        let slots = |count, bytes| self.host_import_storage_bytes(count, bytes);
        // An opaque result uses its verified witness representation. The
        // verifier rejects cyclic opaque witnesses before this boundary.
        while let BytecodeTypeKind::OpaqueResult { witness, .. } =
            self.program.ty(ty).ok_or_else(mismatch)?.kind
        {
            ty = witness;
        }
        let kind = &self.program.ty(ty).ok_or_else(mismatch)?.kind;
        if let RuntimeValue::Function { name, .. } = value {
            if route != ImportRoute::BlockingArgument {
                return Err(mismatch());
            }
            self.blocking_function_callable(name)?;
            return Ok(Node {
                bytes: 0,
                string_bytes: 0,
                children: Children::None,
            });
        }
        if let RuntimeValue::Host { kind: host, .. } = value {
            let bytes = match kind {
                BytecodeTypeKind::Nominal { nominal: Some(nominal), .. }
                    if self.program.nominals.get(nominal.index() as usize).and_then(|nominal| {
                        sync_collection_host_kind(nominal).or_else(|| channel_host_kind(nominal))
                            .or_else(|| encoding_host_kind(nominal)).or_else(|| executor_host_kind(nominal))
                    }) == Some(*host) => 0,
                BytecodeTypeKind::Intrinsic { constructor, .. } if runtime_host_kind(*constructor, self.program.reflection.artifact_tag) == Some(*host) => 0,
                BytecodeTypeKind::Union(members) if members.iter().any(|member| {
                    self.program.ty(*member).is_some_and(|ty| matches!(ty.kind,
                        BytecodeTypeKind::Intrinsic { constructor, .. } if runtime_host_kind(constructor, self.program.reflection.artifact_tag) == Some(*host)))
                }) => base,
                _ => return Err(mismatch()),
            };
            return Ok(Node {
                bytes,
                string_bytes: 0,
                children: Children::None,
            });
        }
        let trace = self.heap.type_descriptor(ty)?;
        let (bytes, string_bytes, children) = match (trace, value) {
            (
                BytecodeTraceDescriptor::Closure {
                    callable,
                    captures: types,
                },
                RuntimeValue::Closure {
                    callable: actual,
                    captures,
                },
            ) if route == ImportRoute::BlockingArgument
                && callable.index() == *actual
                && types.len() == captures.len() =>
            {
                (
                    slots(captures.len(), std::mem::size_of::<Option<Value>>())?,
                    0,
                    Children::Tuple(types, captures),
                )
            }
            (BytecodeTraceDescriptor::Inline, value) => {
                let valid = match (kind, value) {
                    (BytecodeTypeKind::Scalar(BytecodeScalarType::Unit), RuntimeValue::Unit)
                    | (BytecodeTypeKind::Scalar(BytecodeScalarType::Bool), RuntimeValue::Bool(_))
                    | (BytecodeTypeKind::Scalar(BytecodeScalarType::Byte), RuntimeValue::Byte(_))
                    | (BytecodeTypeKind::Scalar(BytecodeScalarType::Char), RuntimeValue::Char(_))
                    | (
                        BytecodeTypeKind::Scalar(
                            BytecodeScalarType::Float | BytecodeScalarType::Float32,
                        ),
                        RuntimeValue::Float(_),
                    )
                    | (
                        BytecodeTypeKind::Scalar(
                            BytecodeScalarType::Int
                            | BytecodeScalarType::Int8
                            | BytecodeScalarType::Int16
                            | BytecodeScalarType::Int32
                            | BytecodeScalarType::UInt8
                            | BytecodeScalarType::UInt16
                            | BytecodeScalarType::UInt32
                            | BytecodeScalarType::UInt64,
                        ),
                        RuntimeValue::Integer(_),
                    ) => true,
                    (
                        BytecodeTypeKind::Intrinsic {
                            constructor: BytecodeIntrinsicType::Duration,
                            ..
                        },
                        RuntimeValue::Integer(value),
                    ) => (i64::MIN as i128..=i64::MAX as i128).contains(value),
                    _ => false,
                };
                if !valid {
                    return Err(mismatch());
                }
                (0, 0, Children::None)
            }
            (BytecodeTraceDescriptor::String, RuntimeValue::String(text)) => (
                slots(text.capacity(), 1)?,
                text.len() as u64,
                Children::None,
            ),
            (BytecodeTraceDescriptor::Tuple { fields }, RuntimeValue::Tuple(values))
                if fields.len() == values.len() =>
            {
                (
                    slots(values.len(), std::mem::size_of::<Option<Value>>())?,
                    0,
                    Children::Tuple(fields, values),
                )
            }
            (BytecodeTraceDescriptor::Array { element }, RuntimeValue::Array(values))
            | (BytecodeTraceDescriptor::Set { element }, RuntimeValue::Set(values)) => (
                slots(values.len(), std::mem::size_of::<Option<Value>>())?,
                0,
                Children::Same(*element, values),
            ),
            (
                BytecodeTraceDescriptor::Map {
                    key,
                    value: value_type,
                },
                RuntimeValue::Map(entries),
            ) => (
                slots(
                    entries.len(),
                    std::mem::size_of::<(Option<Value>, Option<Value>)>(),
                )?,
                0,
                Children::Map(*key, *value_type, entries),
            ),
            (BytecodeTraceDescriptor::Option { .. }, RuntimeValue::OptionNone) => {
                (base, 0, Children::None)
            }
            (BytecodeTraceDescriptor::Option { value: ty }, RuntimeValue::OptionSome(value))
            | (
                BytecodeTraceDescriptor::Result { success: ty, .. },
                RuntimeValue::ResultOk(value),
            )
            | (BytecodeTraceDescriptor::Result { error: ty, .. }, RuntimeValue::ResultErr(value)) => {
                (base, 0, Children::One(*ty, value))
            }
            (BytecodeTraceDescriptor::Ref { value: ty }, RuntimeValue::Ref(value)) => (
                base,
                0,
                value
                    .as_ref()
                    .map_or(Children::None, |value| Children::One(*ty, value)),
            ),
            (BytecodeTraceDescriptor::Union { members }, RuntimeValue::Union { member, value }) => {
                let ty = members
                    .iter()
                    .find(|ty| ty.index() == *member)
                    .ok_or_else(mismatch)?;
                (base, 0, Children::One(*ty, value))
            }
            (
                BytecodeTraceDescriptor::Newtype {
                    nominal, value: ty, ..
                },
                RuntimeValue::Newtype { name, value },
            ) => {
                self.validate_host_nominal_name(*nominal, name)?;
                (base, 0, Children::One(*ty, value))
            }
            (
                BytecodeTraceDescriptor::Record {
                    nominal, fields, ..
                },
                RuntimeValue::Record { name, values },
            ) if fields.len() == values.len() => {
                self.validate_host_nominal_name(*nominal, name)?;
                (
                    slots(values.len(), std::mem::size_of::<(u32, Option<Value>)>())?,
                    0,
                    Children::Record(fields, values),
                )
            }
            (
                BytecodeTraceDescriptor::Variant {
                    nominal: Some(nominal),
                    variants,
                    ..
                },
                RuntimeValue::Variant {
                    name,
                    variant,
                    values,
                },
            ) => {
                self.validate_host_nominal_name(*nominal, name)?;
                match &variants
                    .get(*variant as usize)
                    .ok_or_else(mismatch)?
                    .payload
                {
                    BytecodeVariantPayload::Unit if values.is_empty() => (base, 0, Children::None),
                    BytecodeVariantPayload::Tuple(types) if types.len() == values.len() => (
                        slots(values.len(), std::mem::size_of::<Option<Value>>())?,
                        0,
                        Children::Tuple(types, values),
                    ),
                    BytecodeVariantPayload::Record(fields) if fields.len() == values.len() => (
                        slots(values.len(), std::mem::size_of::<(u32, Option<Value>)>())?,
                        0,
                        Children::Record(fields, values),
                    ),
                    _ => return Err(mismatch()),
                }
            }
            _ => return Err(mismatch()),
        };
        Ok(Node {
            bytes,
            string_bytes,
            children,
        })
    }

    fn blocking_function_callable(&self, name: &str) -> Result<BytecodeCallableId, VmError> {
        let (index, metadata) = self
            .program
            .callables
            .iter()
            .enumerate()
            .find(|(_, callable)| callable.name == name)
            .ok_or_else(|| {
                VmError::Host("blocking function references an unknown callable".into())
            })?;
        if metadata.closure.is_some() || metadata.implementation.is_none() {
            return Err(VmError::Host(
                "blocking function is not a bytecode callable".into(),
            ));
        }
        Ok(BytecodeCallableId::new(index as u32))
    }

    pub(super) fn validate_host_nominal_name(
        &self,
        nominal: crate::bytecode::BytecodeNominalId,
        name: &str,
    ) -> Result<(), VmError> {
        let expected = self.nominal_names.get(nominal.index() as usize);
        if expected.is_some_and(|expected| expected == name) {
            Ok(())
        } else {
            Err(VmError::Host(format!(
                "bootstrap host value names nominal `{name}`, expected `{}`",
                expected.map_or("<missing>", String::as_str)
            )))
        }
    }
}

pub(super) fn reserve_prepared_import(
    vm_heap: &mut Heap,
    statistics: &mut VmStatistics,
    roots: &[Value],
    outcome: BytecodeTypeId,
    cost: ImportCost,
    budget: VmMemoryBudget,
) -> Result<PreparedHostImport, VmError> {
    let (additional, workspace) = cost.prepared_bytes(budget.limit())?;
    vm_heap.preflight_import(
        cost.objects,
        additional,
        Some(budget.clone()),
        roots,
        statistics,
    )?;
    let mut heap = budget.reserve(additional)?;
    let workspace = heap.split_off(workspace)?;
    let objects = vm_heap.reserve_import_objects(cost.objects, roots, statistics)?;
    Ok(PreparedHostImport {
        ty: outcome,
        cost,
        heap,
        workspace,
        objects,
        recipient: None,
    })
}
