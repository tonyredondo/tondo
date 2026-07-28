use std::cell::{Cell, OnceCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::literal;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeVerificationLimits {
    pub max_dataflow_steps: u64,
}

impl Default for BytecodeVerificationLimits {
    fn default() -> Self {
        Self {
            max_dataflow_steps: 32_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeVerificationError {
    context: String,
    message: String,
    resource_limit: bool,
}

impl BytecodeVerificationError {
    fn new(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
            resource_limit: false,
        }
    }

    fn resource_limit(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
            resource_limit: true,
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_resource_limit(&self) -> bool {
        self.resource_limit
    }
}

impl fmt::Display for BytecodeVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "bytecode invariant failed in {}: {}",
            self.context, self.message
        )
    }
}

impl Error for BytecodeVerificationError {}

pub fn verify_bytecode(program: &BytecodeProgram) -> Result<(), BytecodeVerificationError> {
    verify_bytecode_with_limits(program, BytecodeVerificationLimits::default())
}

pub fn verify_bytecode_with_limits(
    program: &BytecodeProgram,
    limits: BytecodeVerificationLimits,
) -> Result<(), BytecodeVerificationError> {
    verify_bytecode_with_trace_metadata(program, limits).map(drop)
}

pub(crate) fn verify_bytecode_with_trace_metadata(
    program: &BytecodeProgram,
    limits: BytecodeVerificationLimits,
) -> Result<BytecodeTraceMetadata, BytecodeVerificationError> {
    Verifier {
        program,
        limits,
        dataflow_steps: Cell::new(0),
        capabilities: OnceCell::new(),
        terminals: OnceCell::new(),
    }
    .verify()?;
    derive_trace_metadata(program)
}

/// Derives `Discard` for closed bytecode types from the same structural graph
/// used by verification. The compiler uses this after monomorphization so a
/// closure protocol row reflects concrete captures rather than open HIR
/// binders. Full bytecode verification remains the admission boundary.
pub fn derive_discard_capabilities(
    program: &BytecodeProgram,
    types: &[BytecodeTypeId],
) -> Result<Vec<bool>, BytecodeVerificationError> {
    let analysis = CapabilityAnalysis::new(program)?;
    types
        .iter()
        .map(|ty| analysis.status(program, *ty, ClosedCapability::Discard))
        .collect()
}

/// Derives `Copy` for closed bytecode types from the same structural graph
/// used by verification. The compiler uses this after monomorphization to
/// replace a source-generic defer guard with a concrete value snapshot.
pub fn derive_copy_capabilities(
    program: &BytecodeProgram,
    types: &[BytecodeTypeId],
) -> Result<Vec<bool>, BytecodeVerificationError> {
    let analysis = CapabilityAnalysis::new(program)?;
    types
        .iter()
        .map(|ty| analysis.status(program, *ty, ClosedCapability::Copy))
        .collect()
}

/// Derives the closed terminal status of concrete bytecode types.
///
/// This is independent of source HIR metadata so malformed lowering cannot
/// erase a terminal token before cleanup analysis.
pub fn derive_terminal_statuses(
    program: &BytecodeProgram,
    types: &[BytecodeTypeId],
) -> Result<Vec<BytecodeTerminalStatus>, BytecodeVerificationError> {
    let analysis = TerminalAnalysis::new(program)?;
    types
        .iter()
        .map(|ty| analysis.status(program, *ty))
        .collect()
}

/// Derives precise heap-edge and frame-root descriptors from the closed
/// bytecode catalog.
///
/// The returned metadata is runtime-owned proof material, not compiler input:
/// malformed references, duplicate closure environments, and cyclic opaque
/// representations are rejected while deriving it.
pub fn derive_trace_metadata(
    program: &BytecodeProgram,
) -> Result<BytecodeTraceMetadata, BytecodeVerificationError> {
    TraceMetadataAnalysis::new(program)?.finish()
}

struct TraceMetadataAnalysis<'a> {
    program: &'a BytecodeProgram,
    types: Vec<Option<BytecodeTraceDescriptor>>,
    closures: BTreeMap<BytecodeTypeId, (BytecodeCallableId, Vec<BytecodeTypeId>)>,
    visiting: BTreeSet<BytecodeTypeId>,
}

impl<'a> TraceMetadataAnalysis<'a> {
    fn new(program: &'a BytecodeProgram) -> Result<Self, BytecodeVerificationError> {
        if program.types.len() > u32::MAX as usize
            || program.callables.len() > u32::MAX as usize
            || program.functions.len() > u32::MAX as usize
        {
            return Err(BytecodeVerificationError::new(
                "trace metadata",
                "catalog exceeds the trace descriptor index space",
            ));
        }
        for (index, ty) in program.types.iter().enumerate() {
            let context = format!("type#{index}");
            for child in bytecode_type_children(&ty.kind) {
                Self::require_type(program, child, &context)?;
            }
            if let BytecodeTypeKind::OpaqueResult { witness, .. } = &ty.kind {
                Self::require_type(program, *witness, &context)?;
            }
            if let BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } = &ty.kind
                && arguments.len() != constructor.arity()
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "intrinsic trace descriptor has the wrong generic arity",
                ));
            }
        }
        for (index, nominal) in program.nominals.iter().enumerate() {
            let context = format!("nominal#{index}");
            match &nominal.shape {
                BytecodeNominalShape::Newtype { underlying } => {
                    Self::require_type(program, *underlying, &context)?;
                }
                BytecodeNominalShape::Record { fields } => {
                    for field in fields {
                        Self::require_type(program, field.ty, &context)?;
                    }
                }
                BytecodeNominalShape::Enum { variants } => {
                    for variant in variants {
                        match &variant.payload {
                            BytecodeVariantPayload::Unit => {}
                            BytecodeVariantPayload::Tuple(fields) => {
                                for field in fields {
                                    Self::require_type(program, *field, &context)?;
                                }
                            }
                            BytecodeVariantPayload::Record(fields) => {
                                for field in fields {
                                    Self::require_type(program, field.ty, &context)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut closures = BTreeMap::new();
        for (index, callable) in program.callables.iter().enumerate() {
            let Some(closure) = &callable.closure else {
                continue;
            };
            let context = format!("callable#{index}");
            Self::require_type(program, closure.environment, &context)?;
            if !matches!(
                program.ty(closure.environment).map(|ty| &ty.kind),
                Some(BytecodeTypeKind::Generated { .. })
            ) {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "closure trace descriptor requires a generated environment type",
                ));
            }
            if closures
                .insert(
                    closure.environment,
                    (
                        BytecodeCallableId::new(index as u32),
                        closure.captures.clone(),
                    ),
                )
                .is_some()
            {
                return Err(BytecodeVerificationError::new(
                    format!("type#{}", closure.environment.index()),
                    "generated trace descriptor has duplicate closure environments",
                ));
            }
            for capture in &closure.captures {
                Self::require_type(program, *capture, &context)?;
            }
        }
        for (index, function) in program.functions.iter().enumerate() {
            let context = format!("function#{index}");
            for slot in &function.slots {
                Self::require_type(program, slot.ty, &context)?;
            }
        }
        Ok(Self {
            program,
            types: vec![None; program.types.len()],
            closures,
            visiting: BTreeSet::new(),
        })
    }

    fn require_type(
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if program.types.get(ty.index() as usize).is_none() {
            return Err(BytecodeVerificationError::new(
                context,
                "trace descriptor references an unknown type",
            ));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<BytecodeTraceMetadata, BytecodeVerificationError> {
        for index in 0..self.program.types.len() {
            self.descriptor(BytecodeTypeId::new(index as u32))?;
        }
        let types = self
            .types
            .into_iter()
            .map(|descriptor| {
                descriptor.ok_or_else(|| {
                    BytecodeVerificationError::new(
                        "trace metadata",
                        "type has no derived trace descriptor",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let frames = self
            .program
            .functions
            .iter()
            .enumerate()
            .map(|(index, function)| BytecodeFrameTraceDescriptor {
                function: BytecodeFunctionId::new(index as u32),
                slots: function.slots.iter().map(|slot| slot.ty).collect(),
            })
            .collect();
        Ok(BytecodeTraceMetadata { types, frames })
    }

    fn descriptor(
        &mut self,
        id: BytecodeTypeId,
    ) -> Result<BytecodeTraceDescriptor, BytecodeVerificationError> {
        let index = id.index() as usize;
        if let Some(descriptor) = self.types.get(index).and_then(Clone::clone) {
            return Ok(descriptor);
        }
        if !self.visiting.insert(id) {
            return Err(BytecodeVerificationError::new(
                format!("type#{}", id.index()),
                "opaque representation forms a trace descriptor cycle",
            ));
        }
        let kind = self
            .program
            .types
            .get(index)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    format!("type#{}", id.index()),
                    "trace descriptor references an unknown type",
                )
            })?
            .kind
            .clone();
        let context = format!("type#{}", id.index());
        let descriptor = match kind {
            BytecodeTypeKind::Scalar(BytecodeScalarType::String) => BytecodeTraceDescriptor::String,
            BytecodeTypeKind::Scalar(_)
            | BytecodeTypeKind::Function(_)
            | BytecodeTypeKind::GenericParameter(_) => BytecodeTraceDescriptor::Inline,
            BytecodeTypeKind::Nominal {
                nominal: Some(nominal),
                arguments,
                ..
            } => {
                let metadata = self
                    .program
                    .nominals
                    .get(nominal.index() as usize)
                    .ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "trace descriptor references an unknown nominal",
                        )
                    })?
                    .clone();
                match metadata.shape {
                    BytecodeNominalShape::Newtype { underlying } => {
                        BytecodeTraceDescriptor::Newtype {
                            nominal,
                            arguments,
                            value: underlying,
                        }
                    }
                    BytecodeNominalShape::Record { fields } => BytecodeTraceDescriptor::Record {
                        nominal,
                        arguments,
                        fields,
                    },
                    BytecodeNominalShape::Enum { variants } => BytecodeTraceDescriptor::Variant {
                        nominal: Some(nominal),
                        arguments,
                        variants,
                    },
                }
            }
            BytecodeTypeKind::Nominal { nominal: None, .. } => BytecodeTraceDescriptor::Inline,
            BytecodeTypeKind::Tuple(fields) => BytecodeTraceDescriptor::Tuple { fields },
            BytecodeTypeKind::Option(value) => BytecodeTraceDescriptor::Option { value },
            BytecodeTypeKind::Result { success, error } => {
                BytecodeTraceDescriptor::Result { success, error }
            }
            BytecodeTypeKind::Union(members) => BytecodeTraceDescriptor::Union { members },
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => match constructor {
                BytecodeIntrinsicType::Array => BytecodeTraceDescriptor::Array {
                    element: arguments.first().copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "array trace descriptor has no element type",
                        )
                    })?,
                },
                BytecodeIntrinsicType::Map => BytecodeTraceDescriptor::Map {
                    key: arguments.first().copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "map trace descriptor has no key type",
                        )
                    })?,
                    value: arguments.get(1).copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "map trace descriptor has no value type",
                        )
                    })?,
                },
                BytecodeIntrinsicType::Set => BytecodeTraceDescriptor::Set {
                    element: arguments.first().copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "set trace descriptor has no element type",
                        )
                    })?,
                },
                BytecodeIntrinsicType::Range => BytecodeTraceDescriptor::Range {
                    element: arguments.first().copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "range trace descriptor has no element type",
                        )
                    })?,
                },
                BytecodeIntrinsicType::Ref => BytecodeTraceDescriptor::Ref {
                    value: arguments.first().copied().ok_or_else(|| {
                        BytecodeVerificationError::new(
                            &context,
                            "reference trace descriptor has no value type",
                        )
                    })?,
                },
                BytecodeIntrinsicType::NumericConversionError => BytecodeTraceDescriptor::Variant {
                    nominal: None,
                    arguments: Vec::new(),
                    variants: BytecodeNumericConversionError::ALL
                        .into_iter()
                        .map(|variant| BytecodeVariant {
                            member: variant.index(),
                            payload: BytecodeVariantPayload::Unit,
                        })
                        .collect(),
                },
                BytecodeIntrinsicType::Pointer
                | BytecodeIntrinsicType::Join
                | BytecodeIntrinsicType::Command
                | BytecodeIntrinsicType::Pipeline => BytecodeTraceDescriptor::Inline,
            },
            BytecodeTypeKind::OpaqueResult { witness, .. } => self.opaque_descriptor(witness)?,
            BytecodeTypeKind::Generated { .. } => self
                .closures
                .get(&id)
                .map(|(callable, captures)| BytecodeTraceDescriptor::Closure {
                    callable: *callable,
                    captures: captures.clone(),
                })
                .unwrap_or(BytecodeTraceDescriptor::Inline),
            BytecodeTypeKind::Cursor { mode, collection } => {
                BytecodeTraceDescriptor::Cursor { mode, collection }
            }
        };
        self.visiting.remove(&id);
        self.types[index] = Some(descriptor.clone());
        Ok(descriptor)
    }

    fn opaque_descriptor(
        &mut self,
        witness: BytecodeTypeId,
    ) -> Result<BytecodeTraceDescriptor, BytecodeVerificationError> {
        let mut current = witness;
        let mut chain = Vec::new();
        let descriptor = loop {
            if let Some(descriptor) = self
                .types
                .get(current.index() as usize)
                .and_then(Clone::clone)
            {
                break descriptor;
            }
            let kind = self
                .program
                .ty(current)
                .ok_or_else(|| {
                    BytecodeVerificationError::new(
                        format!("type#{}", current.index()),
                        "trace descriptor references an unknown opaque witness",
                    )
                })?
                .kind
                .clone();
            let BytecodeTypeKind::OpaqueResult { witness, .. } = kind else {
                break self.descriptor(current)?;
            };
            if !self.visiting.insert(current) {
                return Err(BytecodeVerificationError::new(
                    format!("type#{}", current.index()),
                    "opaque representation forms a trace descriptor cycle",
                ));
            }
            chain.push(current);
            current = witness;
        };
        for opaque in chain.into_iter().rev() {
            self.visiting.remove(&opaque);
            self.types[opaque.index() as usize] = Some(descriptor.clone());
        }
        Ok(descriptor)
    }
}

struct Verifier<'a> {
    program: &'a BytecodeProgram,
    limits: BytecodeVerificationLimits,
    dataflow_steps: Cell<u64>,
    capabilities: OnceCell<CapabilityAnalysis>,
    terminals: OnceCell<TerminalAnalysis>,
}

struct CallVerification<'a> {
    callee: &'a BytecodeOperand,
    arguments: &'a [BytecodeCallArgument],
    signature: BytecodeTypeId,
    protocol: BytecodeCallProtocol,
    outcome: BytecodeTypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationContext {
    Immediate,
    Deferred,
    Await,
    Spawn,
}

impl OperationContext {
    fn expects_async(self) -> bool {
        matches!(self, Self::Await | Self::Spawn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ClosedCapability {
    Copy,
    Discard,
    Equatable,
    Key,
    Send,
    Share,
}

impl ClosedCapability {
    const ALL: [Self; 6] = [
        Self::Copy,
        Self::Discard,
        Self::Equatable,
        Self::Key,
        Self::Send,
        Self::Share,
    ];

    const fn exposed_by(self, capabilities: BytecodeCapabilitySet) -> bool {
        match self {
            Self::Copy => capabilities.copy,
            Self::Discard => capabilities.discard,
            Self::Equatable => capabilities.equatable,
            Self::Key => capabilities.key,
            Self::Send => capabilities.send,
            Self::Share => capabilities.share,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapabilityRequirement {
    possible: bool,
    parameters: BTreeSet<(u32, ClosedCapability)>,
}

impl Default for CapabilityRequirement {
    fn default() -> Self {
        Self {
            possible: true,
            parameters: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
struct CapabilityNode {
    possible: bool,
    dependencies: Vec<(BytecodeTypeId, ClosedCapability)>,
}

#[derive(Debug)]
struct CapabilityAnalysis {
    summaries: BTreeMap<(BytecodeNominalId, ClosedCapability), CapabilityRequirement>,
}

impl CapabilityAnalysis {
    fn new(program: &BytecodeProgram) -> Result<Self, BytecodeVerificationError> {
        let mut summaries = program
            .nominals
            .iter()
            .enumerate()
            .flat_map(|(index, _)| {
                let nominal = BytecodeNominalId::new(index as u32);
                ClosedCapability::ALL.into_iter().map(move |capability| {
                    ((nominal, capability), CapabilityRequirement::default())
                })
            })
            .collect::<BTreeMap<_, _>>();
        loop {
            let mut changes = Vec::new();
            for (index, nominal) in program.nominals.iter().enumerate() {
                let nominal_id = BytecodeNominalId::new(index as u32);
                let roots = nominal_type_roots(&nominal.shape);
                for capability in ClosedCapability::ALL {
                    let next = capability_requirement(program, &roots, capability, &summaries)?;
                    if summaries[&(nominal_id, capability)] != next {
                        changes.push(((nominal_id, capability), next));
                    }
                }
            }
            if changes.is_empty() {
                break;
            }
            for (key, requirement) in changes {
                summaries.insert(key, requirement);
            }
        }
        Ok(Self { summaries })
    }

    fn status(
        &self,
        program: &BytecodeProgram,
        root: BytecodeTypeId,
        capability: ClosedCapability,
    ) -> Result<bool, BytecodeVerificationError> {
        let mut nodes = BTreeMap::new();
        let mut pending = vec![(root, capability)];
        while let Some(key @ (ty, capability)) = pending.pop() {
            if nodes.contains_key(&key) {
                continue;
            }
            let mut node = self.node(program, ty, capability)?;
            node.dependencies.sort_unstable();
            node.dependencies.dedup();
            pending.extend(node.dependencies.iter().copied());
            nodes.insert(key, node);
        }
        let mut statuses = nodes
            .iter()
            .map(|(key, node)| (*key, node.possible))
            .collect::<BTreeMap<_, _>>();
        loop {
            let mut changed = false;
            for (key, node) in &nodes {
                let next = node.possible
                    && node
                        .dependencies
                        .iter()
                        .all(|dependency| statuses[dependency]);
                let current = statuses
                    .get_mut(key)
                    .expect("every capability node has a status");
                if *current != next {
                    *current = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Ok(statuses[&(root, capability)])
    }

    fn node(
        &self,
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
        capability: ClosedCapability,
    ) -> Result<CapabilityNode, BytecodeVerificationError> {
        let kind = &program
            .ty(ty)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    "capability graph",
                    format!("references unknown type#{}", ty.index()),
                )
            })?
            .kind;
        let node = match kind {
            BytecodeTypeKind::Scalar(scalar) => {
                fixed_capability(scalar_capability(*scalar, capability))
            }
            BytecodeTypeKind::Function(_) => fixed_capability(function_capability(capability)),
            BytecodeTypeKind::Tuple(items) | BytecodeTypeKind::Union(items) => {
                same_capability(items, capability)
            }
            BytecodeTypeKind::Option(item) => dependent_capability(vec![(*item, capability)]),
            BytecodeTypeKind::Result { success, error } => {
                dependent_capability(vec![(*success, capability), (*error, capability)])
            }
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => intrinsic_capability(*constructor, arguments, capability),
            BytecodeTypeKind::Nominal {
                nominal, arguments, ..
            } => {
                let Some(nominal) = nominal else {
                    return Ok(fixed_capability(false));
                };
                let requirement = self.summaries.get(&(*nominal, capability)).ok_or_else(|| {
                    BytecodeVerificationError::new(
                        "capability graph",
                        format!("references unknown nominal#{}", nominal.index()),
                    )
                })?;
                let mut dependencies = Vec::with_capacity(requirement.parameters.len());
                for (position, required) in &requirement.parameters {
                    let Some(argument) = arguments.get(*position as usize) else {
                        return Ok(fixed_capability(false));
                    };
                    dependencies.push((*argument, *required));
                }
                CapabilityNode {
                    possible: requirement.possible,
                    dependencies,
                }
            }
            BytecodeTypeKind::GenericParameter(_) => fixed_capability(true),
            BytecodeTypeKind::OpaqueResult { capabilities, .. } => {
                fixed_capability(capability.exposed_by(*capabilities))
            }
            BytecodeTypeKind::Generated { .. } => generated_capability(program, ty, capability),
            BytecodeTypeKind::Cursor { mode, collection } => {
                cursor_capability(*mode, *collection, capability)
            }
        };
        Ok(node)
    }
}

fn capability_requirement(
    program: &BytecodeProgram,
    roots: &[BytecodeTypeId],
    capability: ClosedCapability,
    summaries: &BTreeMap<(BytecodeNominalId, ClosedCapability), CapabilityRequirement>,
) -> Result<CapabilityRequirement, BytecodeVerificationError> {
    let mut requirement = CapabilityRequirement::default();
    let mut pending = roots
        .iter()
        .copied()
        .map(|ty| (ty, capability))
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(key @ (ty, capability)) = pending.pop() {
        if !visited.insert(key) {
            continue;
        }
        let kind = &program
            .ty(ty)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    "capability graph",
                    format!("references unknown type#{}", ty.index()),
                )
            })?
            .kind;
        match kind {
            BytecodeTypeKind::Scalar(scalar) => {
                requirement.possible &= scalar_capability(*scalar, capability);
            }
            BytecodeTypeKind::Function(_) => {
                requirement.possible &= function_capability(capability);
            }
            BytecodeTypeKind::Tuple(items) | BytecodeTypeKind::Union(items) => {
                pending.extend(items.iter().copied().map(|item| (item, capability)));
            }
            BytecodeTypeKind::Option(item) => pending.push((*item, capability)),
            BytecodeTypeKind::Result { success, error } => {
                pending.push((*success, capability));
                pending.push((*error, capability));
            }
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                let node = intrinsic_capability(*constructor, arguments, capability);
                requirement.possible &= node.possible;
                pending.extend(node.dependencies);
            }
            BytecodeTypeKind::Nominal {
                nominal, arguments, ..
            } => {
                let Some(nominal) = nominal else {
                    requirement.possible = false;
                    continue;
                };
                let summary = summaries.get(&(*nominal, capability)).ok_or_else(|| {
                    BytecodeVerificationError::new(
                        "capability graph",
                        format!("references unknown nominal#{}", nominal.index()),
                    )
                })?;
                requirement.possible &= summary.possible;
                for (position, required) in &summary.parameters {
                    if let Some(argument) = arguments.get(*position as usize) {
                        pending.push((*argument, *required));
                    } else {
                        requirement.possible = false;
                    }
                }
            }
            BytecodeTypeKind::GenericParameter(position) => {
                requirement.parameters.insert((*position, capability));
            }
            BytecodeTypeKind::OpaqueResult { capabilities, .. } => {
                requirement.possible &= capability.exposed_by(*capabilities);
            }
            BytecodeTypeKind::Generated { .. } => {
                let node = generated_capability(program, ty, capability);
                requirement.possible &= node.possible;
                pending.extend(node.dependencies);
            }
            BytecodeTypeKind::Cursor { mode, collection } => {
                let node = cursor_capability(*mode, *collection, capability);
                requirement.possible &= node.possible;
                pending.extend(node.dependencies);
            }
        }
    }
    Ok(requirement)
}

fn nominal_type_roots(shape: &BytecodeNominalShape) -> Vec<BytecodeTypeId> {
    match shape {
        BytecodeNominalShape::Newtype { underlying } => vec![*underlying],
        BytecodeNominalShape::Record { fields } => fields.iter().map(|field| field.ty).collect(),
        BytecodeNominalShape::Enum { variants } => variants
            .iter()
            .flat_map(|variant| match &variant.payload {
                BytecodeVariantPayload::Unit => Vec::new(),
                BytecodeVariantPayload::Tuple(items) => items.clone(),
                BytecodeVariantPayload::Record(fields) => {
                    fields.iter().map(|field| field.ty).collect()
                }
            })
            .collect(),
    }
}

fn generated_capability(
    program: &BytecodeProgram,
    ty: BytecodeTypeId,
    capability: ClosedCapability,
) -> CapabilityNode {
    let captures = program.callables.iter().find_map(|callable| {
        callable
            .closure
            .as_ref()
            .filter(|closure| closure.environment == ty)
            .map(|closure| closure.captures.as_slice())
    });
    let Some(captures) = captures else {
        return fixed_capability(false);
    };
    match capability {
        ClosedCapability::Copy
        | ClosedCapability::Discard
        | ClosedCapability::Send
        | ClosedCapability::Share => same_capability(captures, capability),
        ClosedCapability::Equatable | ClosedCapability::Key => fixed_capability(false),
    }
}

fn fixed_capability(possible: bool) -> CapabilityNode {
    CapabilityNode {
        possible,
        dependencies: Vec::new(),
    }
}

fn dependent_capability(dependencies: Vec<(BytecodeTypeId, ClosedCapability)>) -> CapabilityNode {
    CapabilityNode {
        possible: true,
        dependencies,
    }
}

fn same_capability(arguments: &[BytecodeTypeId], capability: ClosedCapability) -> CapabilityNode {
    dependent_capability(
        arguments
            .iter()
            .copied()
            .map(|argument| (argument, capability))
            .collect(),
    )
}

fn scalar_capability(scalar: BytecodeScalarType, capability: ClosedCapability) -> bool {
    capability != ClosedCapability::Key
        || !matches!(
            scalar,
            BytecodeScalarType::Float | BytecodeScalarType::Float32
        )
}

fn function_capability(capability: ClosedCapability) -> bool {
    matches!(
        capability,
        ClosedCapability::Copy
            | ClosedCapability::Discard
            | ClosedCapability::Send
            | ClosedCapability::Share
    )
}

fn cursor_capability(
    mode: BytecodeCursorMode,
    collection: BytecodeTypeId,
    capability: ClosedCapability,
) -> CapabilityNode {
    match (mode, capability) {
        (_, ClosedCapability::Equatable | ClosedCapability::Key) => fixed_capability(false),
        (BytecodeCursorMode::Ref, ClosedCapability::Copy | ClosedCapability::Discard) => {
            fixed_capability(true)
        }
        (BytecodeCursorMode::Ref, ClosedCapability::Send | ClosedCapability::Share) => {
            dependent_capability(vec![
                (collection, ClosedCapability::Send),
                (collection, ClosedCapability::Share),
            ])
        }
        (BytecodeCursorMode::Mut, ClosedCapability::Discard) => fixed_capability(true),
        (
            BytecodeCursorMode::Mut,
            ClosedCapability::Copy | ClosedCapability::Send | ClosedCapability::Share,
        ) => fixed_capability(false),
        (BytecodeCursorMode::Own, capability) => {
            dependent_capability(vec![(collection, capability)])
        }
    }
}

fn intrinsic_capability(
    constructor: BytecodeIntrinsicType,
    arguments: &[BytecodeTypeId],
    capability: ClosedCapability,
) -> CapabilityNode {
    match constructor {
        BytecodeIntrinsicType::Array => {
            if capability == ClosedCapability::Key {
                fixed_capability(false)
            } else {
                same_capability(arguments, capability)
            }
        }
        BytecodeIntrinsicType::Map => match capability {
            ClosedCapability::Key => fixed_capability(false),
            ClosedCapability::Copy => dependent_capability(vec![
                (arguments[0], ClosedCapability::Key),
                (arguments[1], ClosedCapability::Copy),
            ]),
            ClosedCapability::Discard
            | ClosedCapability::Equatable
            | ClosedCapability::Send
            | ClosedCapability::Share => same_capability(arguments, capability),
        },
        BytecodeIntrinsicType::Set => match capability {
            ClosedCapability::Key => fixed_capability(false),
            ClosedCapability::Copy => {
                dependent_capability(vec![(arguments[0], ClosedCapability::Key)])
            }
            ClosedCapability::Discard
            | ClosedCapability::Equatable
            | ClosedCapability::Send
            | ClosedCapability::Share => same_capability(arguments, capability),
        },
        BytecodeIntrinsicType::Range => {
            if matches!(
                capability,
                ClosedCapability::Equatable | ClosedCapability::Key
            ) {
                fixed_capability(false)
            } else {
                same_capability(arguments, capability)
            }
        }
        BytecodeIntrinsicType::Ref => match capability {
            ClosedCapability::Copy
            | ClosedCapability::Discard
            | ClosedCapability::Equatable
            | ClosedCapability::Key => fixed_capability(true),
            ClosedCapability::Send | ClosedCapability::Share => dependent_capability(vec![
                (arguments[0], ClosedCapability::Send),
                (arguments[0], ClosedCapability::Share),
            ]),
        },
        BytecodeIntrinsicType::Pointer => fixed_capability(matches!(
            capability,
            ClosedCapability::Copy | ClosedCapability::Discard
        )),
        BytecodeIntrinsicType::Join => fixed_capability(false),
        BytecodeIntrinsicType::Command | BytecodeIntrinsicType::Pipeline => {
            fixed_capability(matches!(
                capability,
                ClosedCapability::Copy
                    | ClosedCapability::Discard
                    | ClosedCapability::Send
                    | ClosedCapability::Share
            ))
        }
        BytecodeIntrinsicType::NumericConversionError => fixed_capability(true),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalRequirement {
    floor: BytecodeTerminalStatus,
    parameters: BTreeSet<u32>,
}

impl Default for TerminalRequirement {
    fn default() -> Self {
        Self {
            floor: BytecodeTerminalStatus::Absent,
            parameters: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
struct TerminalNode {
    floor: BytecodeTerminalStatus,
    dependencies: Vec<BytecodeTypeId>,
}

#[derive(Debug)]
struct TerminalAnalysis {
    summaries: BTreeMap<BytecodeNominalId, TerminalRequirement>,
}

impl TerminalAnalysis {
    fn new(program: &BytecodeProgram) -> Result<Self, BytecodeVerificationError> {
        let mut summaries = program
            .nominals
            .iter()
            .enumerate()
            .map(|(index, _)| {
                (
                    BytecodeNominalId::new(index as u32),
                    TerminalRequirement::default(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        loop {
            let mut changes = Vec::new();
            for (index, nominal) in program.nominals.iter().enumerate() {
                let nominal_id = BytecodeNominalId::new(index as u32);
                let roots = nominal_type_roots(&nominal.shape);
                let next = terminal_requirement(program, &roots, &summaries)?;
                if summaries[&nominal_id] != next {
                    changes.push((nominal_id, next));
                }
            }
            if changes.is_empty() {
                break;
            }
            for (nominal, requirement) in changes {
                summaries.insert(nominal, requirement);
            }
        }
        let analysis = Self { summaries };
        for (index, ty) in program.types.iter().enumerate() {
            let BytecodeTypeKind::OpaqueResult { witness, .. } = &ty.kind else {
                continue;
            };
            if analysis.status(program, *witness)? != BytecodeTerminalStatus::Absent {
                return Err(BytecodeVerificationError::new(
                    format!("type#{index}"),
                    "opaque result witness retains a terminal obligation",
                ));
            }
        }
        Ok(analysis)
    }

    fn status(
        &self,
        program: &BytecodeProgram,
        root: BytecodeTypeId,
    ) -> Result<BytecodeTerminalStatus, BytecodeVerificationError> {
        let mut nodes = BTreeMap::new();
        let mut pending = vec![root];
        while let Some(ty) = pending.pop() {
            if nodes.contains_key(&ty) {
                continue;
            }
            let mut node = self.node(program, ty)?;
            node.dependencies.sort_unstable();
            node.dependencies.dedup();
            pending.extend(node.dependencies.iter().copied());
            nodes.insert(ty, node);
        }
        let mut statuses = nodes
            .iter()
            .map(|(ty, node)| (*ty, node.floor))
            .collect::<BTreeMap<_, _>>();
        let mut users = nodes
            .keys()
            .copied()
            .map(|ty| (ty, Vec::new()))
            .collect::<BTreeMap<_, Vec<BytecodeTypeId>>>();
        for (user, node) in &nodes {
            for dependency in &node.dependencies {
                users
                    .get_mut(dependency)
                    .expect("all bytecode terminal dependencies are indexed")
                    .push(*user);
            }
        }
        let mut changed = statuses
            .iter()
            .filter_map(|(ty, status)| (*status != BytecodeTerminalStatus::Absent).then_some(*ty))
            .collect::<BTreeSet<_>>();
        while let Some(dependency) = changed.pop_first() {
            for user in &users[&dependency] {
                let node = &nodes[user];
                let next = node
                    .dependencies
                    .iter()
                    .fold(node.floor, |status, dependency| {
                        status.max(statuses[dependency])
                    });
                let current = statuses
                    .get_mut(user)
                    .expect("all bytecode terminal graph users have a status");
                if next > *current {
                    *current = next;
                    changed.insert(*user);
                }
            }
        }
        Ok(statuses[&root])
    }

    fn node(
        &self,
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
    ) -> Result<TerminalNode, BytecodeVerificationError> {
        let kind = &program
            .ty(ty)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    "terminal graph",
                    format!("references unknown type#{}", ty.index()),
                )
            })?
            .kind;
        let node = match kind {
            BytecodeTypeKind::Scalar(_) | BytecodeTypeKind::Function(_) => {
                fixed_terminal(BytecodeTerminalStatus::Absent)
            }
            BytecodeTypeKind::Tuple(items) | BytecodeTypeKind::Union(items) => {
                dependent_terminal(items.clone())
            }
            BytecodeTypeKind::Option(item) => dependent_terminal(vec![*item]),
            BytecodeTypeKind::Result { success, error } => {
                dependent_terminal(vec![*success, *error])
            }
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => intrinsic_terminal(*constructor, arguments),
            BytecodeTypeKind::Nominal {
                nominal, arguments, ..
            } => {
                let Some(nominal) = nominal else {
                    return Ok(fixed_terminal(BytecodeTerminalStatus::Potential));
                };
                let summary = self.summaries.get(nominal).ok_or_else(|| {
                    BytecodeVerificationError::new(
                        "terminal graph",
                        format!("references unknown nominal#{}", nominal.index()),
                    )
                })?;
                let mut dependencies = Vec::with_capacity(summary.parameters.len());
                for position in &summary.parameters {
                    let Some(argument) = arguments.get(*position as usize) else {
                        return Ok(fixed_terminal(BytecodeTerminalStatus::Potential));
                    };
                    dependencies.push(*argument);
                }
                TerminalNode {
                    floor: summary.floor,
                    dependencies,
                }
            }
            BytecodeTypeKind::GenericParameter(_) => {
                fixed_terminal(BytecodeTerminalStatus::Potential)
            }
            BytecodeTypeKind::OpaqueResult { .. } => fixed_terminal(BytecodeTerminalStatus::Absent),
            BytecodeTypeKind::Generated { .. } => generated_terminal(program, ty),
            BytecodeTypeKind::Cursor { mode, collection } => match mode {
                BytecodeCursorMode::Own => dependent_terminal(vec![*collection]),
                BytecodeCursorMode::Ref | BytecodeCursorMode::Mut => {
                    fixed_terminal(BytecodeTerminalStatus::Absent)
                }
            },
        };
        Ok(node)
    }
}

fn terminal_requirement(
    program: &BytecodeProgram,
    roots: &[BytecodeTypeId],
    summaries: &BTreeMap<BytecodeNominalId, TerminalRequirement>,
) -> Result<TerminalRequirement, BytecodeVerificationError> {
    let mut requirement = TerminalRequirement::default();
    let mut pending = roots.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        let kind = &program
            .ty(ty)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    "terminal graph",
                    format!("references unknown type#{}", ty.index()),
                )
            })?
            .kind;
        match kind {
            BytecodeTypeKind::Scalar(_) | BytecodeTypeKind::Function(_) => {}
            BytecodeTypeKind::Tuple(items) | BytecodeTypeKind::Union(items) => {
                pending.extend(items);
            }
            BytecodeTypeKind::Option(item) => pending.push(*item),
            BytecodeTypeKind::Result { success, error } => {
                pending.push(*success);
                pending.push(*error);
            }
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                let node = intrinsic_terminal(*constructor, arguments);
                requirement.floor = requirement.floor.max(node.floor);
                pending.extend(node.dependencies);
            }
            BytecodeTypeKind::Nominal {
                nominal, arguments, ..
            } => {
                let Some(nominal) = nominal else {
                    requirement.floor = requirement.floor.max(BytecodeTerminalStatus::Potential);
                    continue;
                };
                let summary = summaries.get(nominal).ok_or_else(|| {
                    BytecodeVerificationError::new(
                        "terminal graph",
                        format!("references unknown nominal#{}", nominal.index()),
                    )
                })?;
                requirement.floor = requirement.floor.max(summary.floor);
                for position in &summary.parameters {
                    if let Some(argument) = arguments.get(*position as usize) {
                        pending.push(*argument);
                    } else {
                        requirement.floor =
                            requirement.floor.max(BytecodeTerminalStatus::Potential);
                    }
                }
            }
            BytecodeTypeKind::GenericParameter(position) => {
                requirement.parameters.insert(*position);
            }
            BytecodeTypeKind::OpaqueResult { .. } => {}
            BytecodeTypeKind::Generated { .. } => {
                let node = generated_terminal(program, ty);
                requirement.floor = requirement.floor.max(node.floor);
                pending.extend(node.dependencies);
            }
            BytecodeTypeKind::Cursor { mode, collection } => {
                if *mode == BytecodeCursorMode::Own {
                    pending.push(*collection);
                }
            }
        }
    }
    Ok(requirement)
}

fn intrinsic_terminal(
    constructor: BytecodeIntrinsicType,
    arguments: &[BytecodeTypeId],
) -> TerminalNode {
    if constructor.terminal_contract().is_some() {
        return fixed_terminal(BytecodeTerminalStatus::Present);
    }
    match constructor {
        BytecodeIntrinsicType::Array
        | BytecodeIntrinsicType::Map
        | BytecodeIntrinsicType::Set
        | BytecodeIntrinsicType::Range => dependent_terminal(arguments.to_vec()),
        BytecodeIntrinsicType::Ref
        | BytecodeIntrinsicType::Pointer
        | BytecodeIntrinsicType::Command
        | BytecodeIntrinsicType::Pipeline
        | BytecodeIntrinsicType::NumericConversionError => {
            fixed_terminal(BytecodeTerminalStatus::Absent)
        }
        BytecodeIntrinsicType::Join => {
            unreachable!("registered bytecode terminal roots return above")
        }
    }
}

fn generated_terminal(program: &BytecodeProgram, ty: BytecodeTypeId) -> TerminalNode {
    let captures = program.callables.iter().find_map(|callable| {
        callable
            .closure
            .as_ref()
            .filter(|closure| closure.environment == ty)
            .map(|closure| closure.captures.clone())
    });
    match captures {
        Some(captures) => dependent_terminal(captures),
        None => fixed_terminal(BytecodeTerminalStatus::Potential),
    }
}

fn fixed_terminal(floor: BytecodeTerminalStatus) -> TerminalNode {
    TerminalNode {
        floor,
        dependencies: Vec::new(),
    }
}

fn dependent_terminal(dependencies: Vec<BytecodeTypeId>) -> TerminalNode {
    TerminalNode {
        floor: BytecodeTerminalStatus::Absent,
        dependencies,
    }
}

impl Verifier<'_> {
    fn verify(&self) -> Result<(), BytecodeVerificationError> {
        self.verify_types()?;
        self.verify_opaque_types()?;
        self.verify_nominals()?;
        self.capabilities
            .set(CapabilityAnalysis::new(self.program)?)
            .expect("capability analysis is initialized once");
        self.terminals
            .set(TerminalAnalysis::new(self.program)?)
            .expect("terminal analysis is initialized once");
        self.verify_opaque_capabilities()?;
        self.verify_terminal_types()?;
        self.verify_type_formations()?;
        self.verify_callables()?;
        self.verify_constants()?;
        self.verify_function_implementations()?;
        for (index, function) in self.program.functions.iter().enumerate() {
            self.verify_function(BytecodeFunctionId::new(index as u32), function)?;
        }
        Ok(())
    }

    fn capability(
        &self,
        ty: BytecodeTypeId,
        capability: ClosedCapability,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        self.capabilities
            .get()
            .expect("capabilities are initialized after type verification")
            .status(self.program, ty, capability)
            .map_err(|error| BytecodeVerificationError::new(context, error.message))
    }

    fn terminal_status(
        &self,
        ty: BytecodeTypeId,
        context: &str,
    ) -> Result<BytecodeTerminalStatus, BytecodeVerificationError> {
        self.terminals
            .get()
            .expect("terminal analysis is initialized after type verification")
            .status(self.program, ty)
            .map_err(|error| BytecodeVerificationError::new(context, error.message))
    }

    fn verify_terminal_types(&self) -> Result<(), BytecodeVerificationError> {
        for (index, _) in self.program.types.iter().enumerate() {
            let id = BytecodeTypeId::new(index as u32);
            let context = format!("type#{index}");
            let terminal = self.terminal_status(id, &context)?;
            if terminal == BytecodeTerminalStatus::Present {
                if self.capability(id, ClosedCapability::Copy, &context)? {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "terminal type satisfies Copy",
                    ));
                }
                if self.capability(id, ClosedCapability::Discard, &context)? {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "terminal type satisfies Discard",
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_type_formations(&self) -> Result<(), BytecodeVerificationError> {
        for (index, ty) in self.program.types.iter().enumerate() {
            let BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } = &ty.kind
            else {
                continue;
            };
            let requirement = match constructor {
                BytecodeIntrinsicType::Map => {
                    Some((arguments[0], ClosedCapability::Key, "Map key"))
                }
                BytecodeIntrinsicType::Set => {
                    Some((arguments[0], ClosedCapability::Key, "Set key"))
                }
                BytecodeIntrinsicType::Ref => {
                    Some((arguments[0], ClosedCapability::Discard, "Ref target"))
                }
                BytecodeIntrinsicType::Array
                | BytecodeIntrinsicType::Range
                | BytecodeIntrinsicType::Pointer
                | BytecodeIntrinsicType::Join
                | BytecodeIntrinsicType::Command
                | BytecodeIntrinsicType::Pipeline
                | BytecodeIntrinsicType::NumericConversionError => None,
            };
            if let Some((required, capability, label)) = requirement {
                let context = format!("type#{index}");
                if !self.capability(required, capability, &context)? {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!("{label} does not satisfy its closed capability contract"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_opaque_types(&self) -> Result<(), BytecodeVerificationError> {
        let mut families = BTreeSet::new();
        let mut opaque = Vec::new();
        let mut adjacency = vec![Vec::new(); self.program.types.len()];
        for (index, ty) in self.program.types.iter().enumerate() {
            let BytecodeTypeKind::OpaqueResult {
                identity,
                arguments,
                witness,
                capabilities,
            } = &ty.kind
            else {
                continue;
            };
            let context = format!("type#{index}");
            if !capabilities.discard
                || (capabilities.key && !(capabilities.copy && capabilities.equatable))
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "opaque result has a non-normalized published capability set",
                ));
            }
            if !families.insert((identity.as_str(), arguments.as_slice())) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "opaque result family and arguments are duplicated",
                ));
            }
            for root in arguments.iter().chain([witness]) {
                if self.type_contains_generic_parameter(*root, &context)? {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "executable opaque result retains a generic parameter",
                    ));
                }
            }
            if self.is_scalar(*witness, BytecodeScalarType::Never) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "opaque result witness is Never",
                ));
            }
            let id = BytecodeTypeId::new(index as u32);
            opaque.push(id);
            adjacency[index] = self.opaque_dependencies(*witness, &context)?;
        }

        let mut state = vec![0_u8; self.program.types.len()];
        for start in opaque {
            if state[start.index() as usize] != 0 {
                continue;
            }
            let mut pending = vec![(start, false)];
            while let Some((current, expanded)) = pending.pop() {
                let index = current.index() as usize;
                if expanded {
                    state[index] = 2;
                    continue;
                }
                match state[index] {
                    2 => continue,
                    1 => {
                        return Err(BytecodeVerificationError::new(
                            format!("type#{}", start.index()),
                            "opaque result representations form a cycle",
                        ));
                    }
                    _ => {}
                }
                state[index] = 1;
                pending.push((current, true));
                for dependency in adjacency[index].iter().rev() {
                    let dependency_index = dependency.index() as usize;
                    if state[dependency_index] == 1 {
                        return Err(BytecodeVerificationError::new(
                            format!("type#{}", start.index()),
                            "opaque result representations form a cycle",
                        ));
                    }
                    if state[dependency_index] == 0 {
                        pending.push((*dependency, false));
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_opaque_capabilities(&self) -> Result<(), BytecodeVerificationError> {
        for (index, ty) in self.program.types.iter().enumerate() {
            let BytecodeTypeKind::OpaqueResult {
                witness,
                capabilities,
                ..
            } = &ty.kind
            else {
                continue;
            };
            let context = format!("type#{index}");
            for capability in ClosedCapability::ALL {
                if capability.exposed_by(*capabilities)
                    && !self.capability(*witness, capability, &context)?
                {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        format!(
                            "opaque result publishes {capability:?} without a matching witness capability"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn type_contains_generic_parameter(
        &self,
        root: BytecodeTypeId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            let kind = &self.ty(ty, context)?.kind;
            if matches!(kind, BytecodeTypeKind::GenericParameter(_)) {
                return Ok(true);
            }
            pending.extend(bytecode_type_children(kind));
        }
        Ok(false)
    }

    fn opaque_dependencies(
        &self,
        witness: BytecodeTypeId,
        context: &str,
    ) -> Result<Vec<BytecodeTypeId>, BytecodeVerificationError> {
        let mut dependencies = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = vec![witness];
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            let kind = &self.ty(ty, context)?.kind;
            if matches!(kind, BytecodeTypeKind::OpaqueResult { .. }) {
                dependencies.insert(ty);
            } else {
                pending.extend(bytecode_type_children(kind));
            }
        }
        Ok(dependencies.into_iter().collect())
    }

    fn verify_types(&self) -> Result<(), BytecodeVerificationError> {
        let mut names = BTreeSet::new();
        for (index, ty) in self.program.types.iter().enumerate() {
            let context = format!("type#{index}");
            if ty.name.is_empty() || !names.insert(ty.name.as_str()) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "type name is empty or duplicated",
                ));
            }
            match &ty.kind {
                BytecodeTypeKind::Scalar(_) | BytecodeTypeKind::GenericParameter(_) => {}
                BytecodeTypeKind::Nominal {
                    nominal,
                    identity,
                    arguments,
                } => {
                    if identity.is_empty() {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "nominal identity is empty",
                        ));
                    }
                    self.verify_type_ids(arguments, &context)?;
                    if let Some(id) = nominal {
                        let metadata = self.nominal(*id, &context)?;
                        if metadata.identity != *identity
                            || metadata.generic_arity as usize != arguments.len()
                        {
                            return Err(BytecodeVerificationError::new(
                                &context,
                                "nominal identity or generic arity differs from its metadata",
                            ));
                        }
                    }
                }
                BytecodeTypeKind::Tuple(items) => {
                    if items.len() < 2 {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "tuple type has fewer than two items",
                        ));
                    }
                    self.verify_type_ids(items, &context)?;
                }
                BytecodeTypeKind::Function(function) => {
                    for parameter in &function.parameters {
                        self.ty(parameter.ty, &context)?;
                    }
                    if let Some(variadic) = function.variadic {
                        self.ty(variadic, &context)?;
                    }
                    self.ty(function.outcome, &context)?;
                }
                BytecodeTypeKind::Option(item) => {
                    self.ty(*item, &context)?;
                }
                BytecodeTypeKind::Result { success, error } => {
                    self.ty(*success, &context)?;
                    self.ty(*error, &context)?;
                }
                BytecodeTypeKind::Union(members) => {
                    if members.len() < 2 {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "union type has fewer than two members",
                        ));
                    }
                    self.verify_type_ids(members, &context)?;
                    for pair in members.windows(2) {
                        if self.type_name(pair[0])? >= self.type_name(pair[1])? {
                            return Err(BytecodeVerificationError::new(
                                &context,
                                "union members are not in unique canonical order",
                            ));
                        }
                    }
                }
                BytecodeTypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => {
                    if arguments.len() != constructor.arity() {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "intrinsic type has the wrong arity",
                        ));
                    }
                    self.verify_type_ids(arguments, &context)?;
                }
                BytecodeTypeKind::OpaqueResult {
                    identity,
                    arguments,
                    witness,
                    ..
                } => {
                    if identity.is_empty() {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "opaque result identity is empty",
                        ));
                    }
                    self.verify_type_ids(arguments, &context)?;
                    self.ty(*witness, &context)?;
                }
                BytecodeTypeKind::Generated {
                    identity,
                    arguments,
                } => {
                    if identity.is_empty() {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "generated type identity is empty",
                        ));
                    }
                    self.verify_type_ids(arguments, &context)?;
                }
                BytecodeTypeKind::Cursor { collection, .. } => {
                    self.ty(*collection, &context)?;
                }
            }
        }
        Ok(())
    }

    fn verify_nominals(&self) -> Result<(), BytecodeVerificationError> {
        let mut identities = BTreeSet::new();
        for (index, nominal) in self.program.nominals.iter().enumerate() {
            let context = format!("nominal#{index}");
            if nominal.name.is_empty()
                || nominal.identity.is_empty()
                || !identities.insert(nominal.identity.as_str())
            {
                return Err(BytecodeVerificationError::new(
                    context,
                    "nominal name or identity is empty or duplicated",
                ));
            }
            match &nominal.shape {
                BytecodeNominalShape::Newtype { underlying } => {
                    self.ty(*underlying, &context)?;
                }
                BytecodeNominalShape::Record { fields } => {
                    self.verify_fields(fields, &context)?;
                }
                BytecodeNominalShape::Enum { variants } => {
                    let mut members = BTreeSet::new();
                    for variant in variants {
                        if !members.insert(variant.member) {
                            return Err(BytecodeVerificationError::new(
                                &context,
                                "enum variant member is duplicated",
                            ));
                        }
                        match &variant.payload {
                            BytecodeVariantPayload::Unit => {}
                            BytecodeVariantPayload::Tuple(items) => {
                                self.verify_type_ids(items, &context)?;
                            }
                            BytecodeVariantPayload::Record(fields) => {
                                self.verify_fields(fields, &context)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_fields(
        &self,
        fields: &[BytecodeField],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let mut members = BTreeSet::new();
        for field in fields {
            if !members.insert(field.member) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "field member is duplicated",
                ));
            }
            self.ty(field.ty, context)?;
        }
        Ok(())
    }

    fn verify_callables(&self) -> Result<(), BytecodeVerificationError> {
        let mut names = BTreeSet::new();
        let mut closure_environments = BTreeSet::new();
        for (index, callable) in self.program.callables.iter().enumerate() {
            let context = format!("callable#{index}");
            if callable.name.is_empty() || !names.insert(callable.name.as_str()) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "callable name is empty or duplicated",
                ));
            }
            self.ty(callable.outcome, &context)?;
            let BytecodeTypeKind::Function(function) =
                &self.ty(callable.function_type, &context)?.kind
            else {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "callable function_type is not a function",
                ));
            };
            if function.outcome != callable.outcome {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "callable outcome differs from its function type",
                ));
            }
            let variadics = callable
                .parameters
                .iter()
                .filter(|parameter| parameter.variadic_element.is_some())
                .count();
            if variadics > 1
                || callable.parameters.iter().filter(|p| p.receiver).count() > 1
                || callable
                    .parameters
                    .iter()
                    .enumerate()
                    .any(|(position, parameter)| {
                        parameter.variadic_element.is_some()
                            && position + 1 != callable.parameters.len()
                    })
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "callable receiver or variadic shape is invalid",
                ));
            }
            let mut fixed = function.parameters.iter();
            for parameter in &callable.parameters {
                self.ty(parameter.ty, &context)?;
                if let Some(element) = parameter.variadic_element {
                    self.ty(element, &context)?;
                    if function.variadic != Some(element) {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "callable variadic element differs from its function type",
                        ));
                    }
                    continue;
                }
                let Some(expected) = fixed.next() else {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "callable has excess fixed parameters",
                    ));
                };
                if expected.mode != parameter.mode || expected.ty != parameter.ty {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "callable parameter differs from its function type",
                    ));
                }
            }
            if fixed.next().is_some() || (variadics == 0) != function.variadic.is_none() {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "callable function type has excess parameters",
                ));
            }
            if function.is_async
                && callable.parameters.iter().any(|parameter| {
                    matches!(
                        parameter.mode,
                        BytecodeParameterMode::Mut | BytecodeParameterMode::Var
                    )
                })
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "async callable has an exclusive parameter",
                ));
            }
            if callable.implementation.is_none()
                && callable
                    .parameters
                    .iter()
                    .any(|parameter| parameter.mode != BytecodeParameterMode::Value)
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "host callable ABI cannot receive borrowed parameters",
                ));
            }
            if let Some(function) = callable.implementation {
                self.function(function, &context)?;
            }
            if let Some(closure) = &callable.closure {
                let discard =
                    self.capability(closure.environment, ClosedCapability::Discard, &context)?;
                if callable.implementation.is_none()
                    || !closure_environments.insert(closure.environment)
                    || !matches!(
                        self.ty(closure.environment, &context)?.kind,
                        BytecodeTypeKind::Generated { .. }
                    )
                    || (closure.protocols.call && !closure.protocols.call_mut)
                    || (discard && closure.protocols.call_mut && !closure.protocols.call_once)
                {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "closure callable metadata is inconsistent",
                    ));
                }
                self.verify_type_ids(&closure.captures, &context)?;
                let derived = self.derive_closure_protocols(
                    BytecodeCallableId::new(index as u32),
                    callable,
                    function.is_async,
                    &context,
                )?;
                if closure.protocols != derived {
                    return Err(BytecodeVerificationError::new(
                        &context,
                        "closure protocols differ from the implementation body",
                    ));
                }
            }
        }
        Ok(())
    }

    fn derive_closure_protocols(
        &self,
        callable_id: BytecodeCallableId,
        callable: &BytecodeCallable,
        is_async: bool,
        context: &str,
    ) -> Result<BytecodeClosureProtocols, BytecodeVerificationError> {
        let implementation = callable
            .implementation
            .ok_or_else(|| BytecodeVerificationError::new(context, "closure has no body"))?;
        let function = self.function(implementation, context)?;
        let writes_capture = function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    &instruction.kind,
                    BytecodeInstructionKind::Store { destination, .. }
                        if closure_capture_place(function, callable_id, destination)
                )
            }) || matches!(
                &block.terminator.kind,
                BytecodeTerminatorKind::Invoke {
                    operation:
                        BytecodeOperation {
                            kind: BytecodeOperationKind::Call {
                                callee,
                                arguments,
                                protocol,
                                ..
                            },
                            ..
                        },
                    ..
                } if (*protocol == BytecodeCallProtocol::CallMut
                    && operand_place(function, callee).is_some_and(|place| {
                        closure_capture_place(function, callable_id, place)
                    }))
                    || arguments.iter().any(|argument| {
                        matches!(argument.mode, BytecodeParameterMode::Mut | BytecodeParameterMode::Var)
                            && operand_place(function, &argument.value).is_some_and(|place| {
                                closure_capture_place(function, callable_id, place)
                            })
                    })
            )
        });
        let moves_capture = function.blocks.iter().any(|block| {
            local_events(function, block).into_iter().any(|event| {
                matches!(
                    event,
                    LocalEvent::Move(access)
                        if closure_capture_access(function, callable_id, &access)
                )
            })
        });
        let mut required_transfers = BTreeSet::new();
        let closure = callable
            .closure
            .as_ref()
            .expect("closure protocol derivation receives closure metadata");
        for (index, capture) in closure.captures.iter().enumerate() {
            if !self.capability(*capture, ClosedCapability::Discard, context)? {
                required_transfers.insert(u32::try_from(index).map_err(|_| {
                    BytecodeVerificationError::new(
                        context,
                        "closure capture index exceeds bytecode limits",
                    )
                })?);
            }
        }
        let transferred_on_all_returns = self.closure_captures_transferred_on_all_returns(
            function,
            callable_id,
            closure.captures.len(),
            context,
        )?;
        Ok(BytecodeClosureProtocols {
            call: !writes_capture && !moves_capture,
            call_mut: !moves_capture && (!is_async || !writes_capture),
            call_once: required_transfers.is_subset(&transferred_on_all_returns),
        })
    }

    fn closure_captures_transferred_on_all_returns(
        &self,
        function: &BytecodeFunction,
        callable: BytecodeCallableId,
        capture_count: usize,
        context: &str,
    ) -> Result<BTreeSet<u32>, BytecodeVerificationError> {
        let all = (0..capture_count)
            .map(|index| {
                u32::try_from(index).map_err(|_| {
                    BytecodeVerificationError::new(
                        context,
                        "closure capture index exceeds bytecode limits",
                    )
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut incoming = vec![None::<BTreeSet<u32>>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(BTreeSet::new());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        let mut returns = None::<BTreeSet<u32>>;

        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let Some(mut state) = incoming[block_id.index() as usize].clone() else {
                continue;
            };
            let block = &function.blocks[block_id.index() as usize];
            if block.kind != BytecodeBlockKind::Normal {
                continue;
            }
            for event in local_events(function, block) {
                match event {
                    LocalEvent::Move(access) => {
                        if let Some(index) =
                            closure_capture_transfer_index(function, callable, &access)
                        {
                            state.insert(index);
                        }
                    }
                    LocalEvent::Write(access) => {
                        if let Some(index) =
                            closure_capture_access_index(function, callable, &access)
                        {
                            state.remove(&index);
                        }
                    }
                    LocalEvent::Read(_)
                    | LocalEvent::Resolve(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => {}
                }
            }
            if matches!(block.terminator.kind, BytecodeTerminatorKind::Return) {
                intersect_optional_capture_set(&mut returns, state);
                continue;
            }
            for edge in successor_edges(&block.terminator.kind) {
                if function.blocks[edge.target.index() as usize].kind != BytecodeBlockKind::Normal {
                    continue;
                }
                let mut edge_state = state.clone();
                if let Some(index) = edge.writes.as_ref().and_then(|place| {
                    closure_capture_access_index(
                        function,
                        callable,
                        &LocalAccess::from_place(place),
                    )
                }) {
                    edge_state.remove(&index);
                }
                let changed = intersect_incoming_capture_set(
                    &mut incoming[edge.target.index() as usize],
                    edge_state,
                );
                if changed && !queued[edge.target.index() as usize] {
                    queued[edge.target.index() as usize] = true;
                    queue.push_back(edge.target);
                }
            }
        }

        Ok(returns.unwrap_or(all))
    }

    fn verify_constants(&self) -> Result<(), BytecodeVerificationError> {
        let mut names = BTreeSet::new();
        for (index, constant) in self.program.constants.iter().enumerate() {
            let context = format!("constant#{index}");
            if constant.name.is_empty() || !names.insert(constant.name.as_str()) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "constant name is empty or duplicated",
                ));
            }
            self.verify_constant_value(&constant.value, &context)?;
        }
        Ok(())
    }

    fn verify_constant_value(
        &self,
        value: &BytecodeConstantValue,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let ty = &self.ty(value.ty, context)?.kind;
        match &value.kind {
            BytecodeConstantValueKind::Unit
                if matches!(ty, BytecodeTypeKind::Scalar(BytecodeScalarType::Unit)) => {}
            BytecodeConstantValueKind::Bool(_)
                if matches!(ty, BytecodeTypeKind::Scalar(BytecodeScalarType::Bool)) => {}
            BytecodeConstantValueKind::Integer(integer) => {
                let BytecodeTypeKind::Scalar(scalar) = ty else {
                    return Err(constant_shape_error(context));
                };
                if !integer_value_fits(*integer, *scalar) {
                    return Err(constant_shape_error(context));
                }
            }
            BytecodeConstantValueKind::Float(bits) => {
                let BytecodeTypeKind::Scalar(scalar) = ty else {
                    return Err(constant_shape_error(context));
                };
                if !float_bits_fit_scalar(*bits, *scalar) {
                    return Err(constant_shape_error(context));
                }
            }
            BytecodeConstantValueKind::Char(_)
                if matches!(ty, BytecodeTypeKind::Scalar(BytecodeScalarType::Char)) => {}
            BytecodeConstantValueKind::String(_)
                if matches!(ty, BytecodeTypeKind::Scalar(BytecodeScalarType::String)) => {}
            BytecodeConstantValueKind::Function {
                callable,
                arguments,
            } => {
                let callable = self.callable(*callable, context)?;
                self.verify_type_ids(arguments, context)?;
                if callable.closure.is_some()
                    || arguments.len() != callable.generic_arity as usize
                    || !self.representation_matches_substitution(
                        callable.function_type,
                        value.ty,
                        arguments,
                        context,
                    )?
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "constant function value does not match its callable",
                    ));
                }
            }
            BytecodeConstantValueKind::Tuple(values) => {
                let BytecodeTypeKind::Tuple(items) = ty else {
                    return Err(constant_shape_error(context));
                };
                self.verify_constant_sequence(values, items, context)?;
            }
            BytecodeConstantValueKind::Array(values) => {
                let element =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Array, 0, context)?;
                self.verify_constant_repeated(values, element, context)?;
            }
            BytecodeConstantValueKind::Map(entries) => {
                let key =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Map, 0, context)?;
                let item =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Map, 1, context)?;
                for (entry_key, entry_value) in entries {
                    if entry_key.ty != key || entry_value.ty != item {
                        return Err(constant_shape_error(context));
                    }
                    self.verify_constant_value(entry_key, context)?;
                    self.verify_constant_value(entry_value, context)?;
                }
            }
            BytecodeConstantValueKind::Set(values) => {
                let element =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Set, 0, context)?;
                self.verify_constant_repeated(values, element, context)?;
            }
            BytecodeConstantValueKind::Newtype {
                nominal,
                value: item,
            } => {
                let (actual_nominal, arguments, metadata) =
                    self.nominal_instance(value.ty, context)?;
                let BytecodeNominalShape::Newtype { underlying } = metadata.shape else {
                    return Err(constant_shape_error(context));
                };
                if *nominal != actual_nominal
                    || !self.type_matches_substitution(underlying, item.ty, arguments, context)?
                {
                    return Err(constant_shape_error(context));
                }
                self.verify_constant_value(item, context)?;
            }
            BytecodeConstantValueKind::Record { nominal, fields } => {
                let (actual_nominal, arguments, metadata) =
                    self.nominal_instance(value.ty, context)?;
                let BytecodeNominalShape::Record { fields: declared } = &metadata.shape else {
                    return Err(constant_shape_error(context));
                };
                if *nominal != actual_nominal || fields.len() != declared.len() {
                    return Err(constant_shape_error(context));
                }
                for ((member, field), declaration) in fields.iter().zip(declared) {
                    if *member != declaration.member
                        || !self.type_matches_substitution(
                            declaration.ty,
                            field.ty,
                            arguments,
                            context,
                        )?
                    {
                        return Err(constant_shape_error(context));
                    }
                    self.verify_constant_value(field, context)?;
                }
            }
            BytecodeConstantValueKind::Variant { variant, payload } => {
                if matches!(
                    ty,
                    BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::NumericConversionError,
                        arguments,
                    } if arguments.is_empty()
                ) {
                    if BytecodeNumericConversionError::from_index(*variant).is_none()
                        || !matches!(payload, BytecodeConstantVariantValue::Unit)
                    {
                        return Err(constant_shape_error(context));
                    }
                } else {
                    let (_, arguments, metadata) = self.nominal_instance(value.ty, context)?;
                    let BytecodeNominalShape::Enum { variants } = &metadata.shape else {
                        return Err(constant_shape_error(context));
                    };
                    let Some(declaration) = variants.iter().find(|item| item.member == *variant)
                    else {
                        return Err(constant_shape_error(context));
                    };
                    self.verify_constant_variant(
                        payload,
                        &declaration.payload,
                        arguments,
                        context,
                    )?;
                }
            }
            BytecodeConstantValueKind::OptionNone if matches!(ty, BytecodeTypeKind::Option(_)) => {}
            BytecodeConstantValueKind::OptionSome(item) => {
                let BytecodeTypeKind::Option(expected) = ty else {
                    return Err(constant_shape_error(context));
                };
                if item.ty != *expected {
                    return Err(constant_shape_error(context));
                }
                self.verify_constant_value(item, context)?;
            }
            BytecodeConstantValueKind::ResultOk(item) => {
                let BytecodeTypeKind::Result { success, .. } = ty else {
                    return Err(constant_shape_error(context));
                };
                if item.ty != *success {
                    return Err(constant_shape_error(context));
                }
                self.verify_constant_value(item, context)?;
            }
            BytecodeConstantValueKind::ResultErr(item) => {
                let BytecodeTypeKind::Result { error, .. } = ty else {
                    return Err(constant_shape_error(context));
                };
                if item.ty != *error {
                    return Err(constant_shape_error(context));
                }
                self.verify_constant_value(item, context)?;
            }
            BytecodeConstantValueKind::Range { start, end, .. } => {
                let item =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Range, 0, context)?;
                if start.ty != item || end.ty != item {
                    return Err(constant_shape_error(context));
                }
                self.verify_constant_value(start, context)?;
                self.verify_constant_value(end, context)?;
            }
            _ => return Err(constant_shape_error(context)),
        }
        Ok(())
    }

    fn verify_constant_sequence(
        &self,
        values: &[BytecodeConstantValue],
        types: &[BytecodeTypeId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if values.len() != types.len() {
            return Err(constant_shape_error(context));
        }
        for (value, ty) in values.iter().zip(types) {
            if value.ty != *ty {
                return Err(constant_shape_error(context));
            }
            self.verify_constant_value(value, context)?;
        }
        Ok(())
    }

    fn verify_constant_repeated(
        &self,
        values: &[BytecodeConstantValue],
        ty: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        for value in values {
            if value.ty != ty {
                return Err(constant_shape_error(context));
            }
            self.verify_constant_value(value, context)?;
        }
        Ok(())
    }

    fn verify_constant_variant(
        &self,
        value: &BytecodeConstantVariantValue,
        declaration: &BytecodeVariantPayload,
        arguments: &[BytecodeTypeId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        match (value, declaration) {
            (BytecodeConstantVariantValue::Unit, BytecodeVariantPayload::Unit) => Ok(()),
            (BytecodeConstantVariantValue::Tuple(values), BytecodeVariantPayload::Tuple(types))
                if values.len() == types.len() =>
            {
                for (value, template) in values.iter().zip(types) {
                    if !self.type_matches_substitution(*template, value.ty, arguments, context)? {
                        return Err(constant_shape_error(context));
                    }
                    self.verify_constant_value(value, context)?;
                }
                Ok(())
            }
            (
                BytecodeConstantVariantValue::Record(values),
                BytecodeVariantPayload::Record(fields),
            ) if values.len() == fields.len() => {
                for ((member, value), field) in values.iter().zip(fields) {
                    if *member != field.member
                        || !self
                            .type_matches_substitution(field.ty, value.ty, arguments, context)?
                    {
                        return Err(constant_shape_error(context));
                    }
                    self.verify_constant_value(value, context)?;
                }
                Ok(())
            }
            _ => Err(constant_shape_error(context)),
        }
    }

    fn nominal_instance(
        &self,
        ty: BytecodeTypeId,
        context: &str,
    ) -> Result<(BytecodeNominalId, &[BytecodeTypeId], &BytecodeNominal), BytecodeVerificationError>
    {
        let BytecodeTypeKind::Nominal {
            nominal: Some(nominal),
            arguments,
            ..
        } = &self.ty(ty, context)?.kind
        else {
            return Err(BytecodeVerificationError::new(
                context,
                "expected a local nominal type",
            ));
        };
        Ok((*nominal, arguments, self.nominal(*nominal, context)?))
    }

    fn intrinsic_argument(
        &self,
        ty: BytecodeTypeId,
        constructor: BytecodeIntrinsicType,
        index: usize,
        context: &str,
    ) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        let BytecodeTypeKind::Intrinsic {
            constructor: actual,
            arguments,
        } = &self.ty(ty, context)?.kind
        else {
            return Err(BytecodeVerificationError::new(
                context,
                "expected an intrinsic type",
            ));
        };
        if *actual != constructor {
            return Err(BytecodeVerificationError::new(
                context,
                "intrinsic constructor is inconsistent",
            ));
        }
        arguments
            .get(index)
            .copied()
            .ok_or_else(|| BytecodeVerificationError::new(context, "intrinsic argument is absent"))
    }

    fn type_matches_substitution(
        &self,
        template: BytecodeTypeId,
        actual: BytecodeTypeId,
        arguments: &[BytecodeTypeId],
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        self.type_matches_substitution_with_representation(
            template, actual, arguments, false, context,
        )
    }

    fn representation_matches_substitution(
        &self,
        template: BytecodeTypeId,
        actual: BytecodeTypeId,
        arguments: &[BytecodeTypeId],
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        self.type_matches_substitution_with_representation(
            template, actual, arguments, true, context,
        )
    }

    fn type_matches_substitution_with_representation(
        &self,
        template: BytecodeTypeId,
        actual: BytecodeTypeId,
        arguments: &[BytecodeTypeId],
        reveal_opaque: bool,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let mut pending = vec![(template, actual)];
        let mut visited = BTreeSet::new();
        while let Some((template, actual)) = pending.pop() {
            if template == actual {
                continue;
            }
            if !visited.insert((template, actual)) {
                return Ok(false);
            }
            let template_kind = &self.ty(template, context)?.kind;
            if let BytecodeTypeKind::GenericParameter(position) = template_kind {
                let Some(substituted) = arguments.get(*position as usize).copied() else {
                    return Ok(false);
                };
                if reveal_opaque {
                    pending.push((substituted, actual));
                } else if substituted != actual {
                    return Ok(false);
                }
                continue;
            }
            let actual_kind = &self.ty(actual, context)?.kind;
            if reveal_opaque {
                if let BytecodeTypeKind::OpaqueResult { witness, .. } = template_kind {
                    pending.push((*witness, actual));
                    continue;
                }
                if let BytecodeTypeKind::OpaqueResult { witness, .. } = actual_kind {
                    pending.push((template, *witness));
                    continue;
                }
            }
            match (template_kind, actual_kind) {
                (BytecodeTypeKind::Scalar(left), BytecodeTypeKind::Scalar(right))
                    if left == right => {}
                (
                    BytecodeTypeKind::Nominal {
                        identity: left_identity,
                        arguments: left,
                        ..
                    },
                    BytecodeTypeKind::Nominal {
                        identity: right_identity,
                        arguments: right,
                        ..
                    },
                ) if left_identity == right_identity && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (BytecodeTypeKind::Tuple(left), BytecodeTypeKind::Tuple(right))
                | (BytecodeTypeKind::Union(left), BytecodeTypeKind::Union(right))
                    if left.len() == right.len() =>
                {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (BytecodeTypeKind::Option(left), BytecodeTypeKind::Option(right)) => {
                    pending.push((*left, *right));
                }
                (
                    BytecodeTypeKind::Result {
                        success: left_success,
                        error: left_error,
                    },
                    BytecodeTypeKind::Result {
                        success: right_success,
                        error: right_error,
                    },
                ) => {
                    pending.push((*left_success, *right_success));
                    pending.push((*left_error, *right_error));
                }
                (
                    BytecodeTypeKind::Intrinsic {
                        constructor: left_constructor,
                        arguments: left,
                    },
                    BytecodeTypeKind::Intrinsic {
                        constructor: right_constructor,
                        arguments: right,
                    },
                ) if left_constructor == right_constructor && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (BytecodeTypeKind::Function(left), BytecodeTypeKind::Function(right))
                    if left.is_async == right.is_async
                        && left.is_unsafe == right.is_unsafe
                        && left.parameters.len() == right.parameters.len()
                        && left.variadic.is_some() == right.variadic.is_some() =>
                {
                    for (left, right) in left.parameters.iter().zip(&right.parameters) {
                        if left.mode != right.mode {
                            return Ok(false);
                        }
                        pending.push((left.ty, right.ty));
                    }
                    if let (Some(left), Some(right)) = (left.variadic, right.variadic) {
                        pending.push((left, right));
                    }
                    pending.push((left.outcome, right.outcome));
                }
                (
                    BytecodeTypeKind::OpaqueResult {
                        identity: left_identity,
                        arguments: left,
                        witness: left_witness,
                        capabilities: left_capabilities,
                    },
                    BytecodeTypeKind::OpaqueResult {
                        identity: right_identity,
                        arguments: right,
                        witness: right_witness,
                        capabilities: right_capabilities,
                    },
                ) if left_identity == right_identity
                    && left.len() == right.len()
                    && left_witness == right_witness
                    && left_capabilities == right_capabilities =>
                {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (
                    BytecodeTypeKind::Generated {
                        identity: left_identity,
                        arguments: left,
                    },
                    BytecodeTypeKind::Generated {
                        identity: right_identity,
                        arguments: right,
                    },
                ) if left_identity == right_identity && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (
                    BytecodeTypeKind::Cursor {
                        mode: left_mode,
                        collection: left,
                    },
                    BytecodeTypeKind::Cursor {
                        mode: right_mode,
                        collection: right,
                    },
                ) if left_mode == right_mode => pending.push((*left, *right)),
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn verify_function(
        &self,
        id: BytecodeFunctionId,
        function: &BytecodeFunction,
    ) -> Result<(), BytecodeVerificationError> {
        let context = format!("function#{}", id.index());
        let callable = self.callable(function.callable, &context)?;
        if function.source.start > function.source.end {
            return Err(BytecodeVerificationError::new(
                &context,
                "function source span is reversed",
            ));
        }
        if function.types.is_empty()
            || function
                .types
                .windows(2)
                .any(|pair| pair[0].index() >= pair[1].index())
        {
            return Err(BytecodeVerificationError::new(
                &context,
                "function type table is empty, duplicated, or unordered",
            ));
        }
        self.verify_type_ids(&function.types, &context)?;
        if function.spans.is_empty()
            || function.spans.windows(2).any(|pair| pair[0] >= pair[1])
            || function
                .spans
                .iter()
                .any(|span| span.file != function.source.file || span.start > span.end)
        {
            return Err(BytecodeVerificationError::new(
                &context,
                "function span table is empty, invalid, duplicated, unordered, or cross-file",
            ));
        }
        if function.slots.is_empty() || function.blocks.is_empty() {
            return Err(BytecodeVerificationError::new(
                &context,
                "function has no slots or blocks",
            ));
        }
        let return_slot = self.slot(function, function.return_slot, &context)?;
        if return_slot.kind != BytecodeSlotKind::Return || return_slot.ty != callable.outcome {
            return Err(BytecodeVerificationError::new(
                &context,
                "return slot kind or type differs from callable outcome",
            ));
        }
        let mut return_count = 0;
        let mut parameter_count = 0;
        let mut user_locals = BTreeSet::new();
        for (index, slot) in function.slots.iter().enumerate() {
            self.function_type(function, slot.ty, &context)?;
            self.span(function, slot.span, &context)?;
            match slot.kind {
                BytecodeSlotKind::Return => return_count += 1,
                BytecodeSlotKind::Parameter { index: parameter } => {
                    if parameter as usize != parameter_count {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "parameter slot indices are not contiguous",
                        ));
                    }
                    parameter_count += 1;
                    if function.parameters.get(parameter as usize)
                        != Some(&BytecodeSlotId::new(index as u32))
                    {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "parameter slot table differs from slot metadata",
                        ));
                    }
                }
                BytecodeSlotKind::User { local } => {
                    if !user_locals.insert(local) {
                        return Err(BytecodeVerificationError::new(
                            &context,
                            "user local identity is duplicated",
                        ));
                    }
                }
                BytecodeSlotKind::Temporary => {}
            }
        }
        let environment_count = usize::from(callable.closure.is_some());
        let expected_parameters = callable.parameters.len() + environment_count;
        if return_count != 1
            || parameter_count != expected_parameters
            || function.parameters.len() != expected_parameters
        {
            return Err(BytecodeVerificationError::new(
                &context,
                "return or parameter slot count is inconsistent",
            ));
        }
        if let Some(closure) = &callable.closure {
            let slot = self.slot(function, function.parameters[0], &context)?;
            if slot.ty != closure.environment
                || slot.kind != (BytecodeSlotKind::Parameter { index: 0 })
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "closure environment slot differs from callable metadata",
                ));
            }
        }
        for (position, (slot, parameter)) in function
            .parameters
            .iter()
            .skip(environment_count)
            .zip(&callable.parameters)
            .enumerate()
        {
            let slot = self.slot(function, *slot, &context)?;
            if slot.ty != parameter.ty
                || slot.kind
                    != (BytecodeSlotKind::Parameter {
                        index: (position + environment_count) as u32,
                    })
            {
                return Err(BytecodeVerificationError::new(
                    &context,
                    "parameter slot type or position differs from callable metadata",
                ));
            }
        }
        for (index, loan) in function.loans.iter().enumerate() {
            let loan_context = format!("{context} loan#{index}");
            if loan.mode == BytecodeParameterMode::Value {
                return Err(BytecodeVerificationError::new(
                    &loan_context,
                    "loan metadata uses the owning value mode",
                ));
            }
            match loan.kind {
                BytecodeLoanKind::CallLocal => {}
                BytecodeLoanKind::Region => {}
            }
            if let Some(source) = loan.place.source_loan
                && source.index() as usize >= index
            {
                return Err(BytecodeVerificationError::new(
                    &loan_context,
                    "loan source region is not an earlier acyclic reservation",
                ));
            }
            self.verify_place(function, &loan.place, &loan_context)?;
            if loan.mode != BytecodeParameterMode::Ref && place_contains_ref_value(&loan.place) {
                return Err(BytecodeVerificationError::new(
                    &loan_context,
                    "`Ref[T].value` permits only shared `ref` loans",
                ));
            }
            if loan.kind == BytecodeLoanKind::Region
                && matches!(
                    loan.mode,
                    BytecodeParameterMode::Mut | BytecodeParameterMode::Var
                )
            {
                self.verify_exclusive_iterator_loan_path(function, &loan.place, &loan_context)?;
            }
        }
        if function.entry == function.unwind
            || self.block(function, function.entry, &context)?.kind != BytecodeBlockKind::Normal
            || self.block(function, function.unwind, &context)?.kind != BytecodeBlockKind::Cleanup
            || !matches!(
                self.block(function, function.unwind, &context)?
                    .terminator
                    .kind,
                BytecodeTerminatorKind::ResumePanic
            )
        {
            return Err(BytecodeVerificationError::new(
                &context,
                "entry and unwind blocks do not have their required distinct shapes",
            ));
        }
        for (block_index, block) in function.blocks.iter().enumerate() {
            let block_context = format!("{context} block#{block_index}");
            for instruction in &block.instructions {
                self.span(function, instruction.span, &block_context)?;
                if block.kind == BytecodeBlockKind::Cleanup
                    && matches!(
                        instruction.kind,
                        BytecodeInstructionKind::ReserveLoan(_)
                            | BytecodeInstructionKind::ReleaseLoan(_)
                    )
                {
                    return Err(BytecodeVerificationError::new(
                        &block_context,
                        "cleanup block manipulates a loan reservation",
                    ));
                }
                self.verify_instruction(function, block.kind, instruction, &block_context)?;
            }
            self.span(function, block.terminator.span, &block_context)?;
            self.verify_terminator(function, block, &block_context)?;
        }
        self.verify_control_and_dataflow(function, &context)?;
        self.verify_defer_flow(function, &context)?;
        self.verify_task_scope_flow(function, &context)?;
        self.verify_suspension_liveness(function, &context)?;
        Ok(())
    }

    fn verify_task_scope_flow(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let mut incoming = vec![None::<Vec<BytecodeScopeId>>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(Vec::new());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let Some(mut scopes) = incoming[block_id.index() as usize].clone() else {
                continue;
            };
            let block = &function.blocks[block_id.index() as usize];
            if block.kind != BytecodeBlockKind::Normal {
                continue;
            }
            let block_context = format!("{context} block#{}", block_id.index());
            for instruction in &block.instructions {
                if let BytecodeInstructionKind::EnterTaskScope { scope } = instruction.kind {
                    if scopes.contains(&scope) {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "task scope is re-entered before its previous extent is drained",
                        ));
                    }
                    scopes.push(scope);
                }
            }
            if let BytecodeTerminatorKind::Spawn { scope, .. } = &block.terminator.kind
                && scopes.last() != Some(scope)
            {
                return Err(BytecodeVerificationError::new(
                    &block_context,
                    "spawn is not owned by the innermost active task scope",
                ));
            }
            if let BytecodeTerminatorKind::DrainScopes { task_scopes, .. } = &block.terminator.kind
            {
                let start = scopes.len().checked_sub(task_scopes.len()).ok_or_else(|| {
                    BytecodeVerificationError::new(
                        &block_context,
                        "structured drain removes more task scopes than are active",
                    )
                })?;
                if scopes[start..] != task_scopes[..] {
                    return Err(BytecodeVerificationError::new(
                        &block_context,
                        "structured drain does not remove the exact active task-scope suffix",
                    ));
                }
                scopes.truncate(start);
            }
            if matches!(block.terminator.kind, BytecodeTerminatorKind::Return) && !scopes.is_empty()
            {
                return Err(BytecodeVerificationError::new(
                    &block_context,
                    "normal return abandons active task scopes",
                ));
            }
            for edge in successor_edges(&block.terminator.kind) {
                if function.blocks[edge.target.index() as usize].kind != BytecodeBlockKind::Normal {
                    continue;
                }
                let target = edge.target.index() as usize;
                match &incoming[target] {
                    Some(previous) if previous != &scopes => {
                        return Err(BytecodeVerificationError::new(
                            format!("{context} block#{}", edge.target.index()),
                            "control-flow predecessors disagree about active task scopes",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        incoming[target] = Some(scopes.clone());
                        if !queued[target] {
                            queued[target] = true;
                            queue.push_back(edge.target);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_suspension_liveness(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let mut uses = vec![BTreeSet::<BytecodeSlotId>::new(); function.blocks.len()];
        let mut definitions = vec![BTreeSet::<BytecodeSlotId>::new(); function.blocks.len()];
        for (index, block) in function.blocks.iter().enumerate() {
            for event in local_events(function, block) {
                match event {
                    LocalEvent::Read(access)
                    | LocalEvent::Resolve(access)
                    | LocalEvent::WriteAccess(access) => {
                        if !definitions[index].contains(&access.slot) {
                            uses[index].insert(access.slot);
                        }
                    }
                    LocalEvent::Move(access) => {
                        if !definitions[index].contains(&access.slot) {
                            uses[index].insert(access.slot);
                        }
                        if access.path.is_empty() {
                            definitions[index].insert(access.slot);
                        }
                    }
                    LocalEvent::Write(access) => {
                        if !access.path.is_empty() && !definitions[index].contains(&access.slot) {
                            uses[index].insert(access.slot);
                        }
                        if access.path.is_empty() {
                            definitions[index].insert(access.slot);
                        }
                    }
                    LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => {
                        definitions[index].insert(slot);
                    }
                }
            }
        }
        let mut live_in = vec![BTreeSet::<BytecodeSlotId>::new(); function.blocks.len()];
        let mut live_out = live_in.clone();
        loop {
            self.consume_dataflow_step(context)?;
            let mut changed = false;
            for index in (0..function.blocks.len()).rev() {
                let mut outgoing = BTreeSet::new();
                for edge in successor_edges(&function.blocks[index].terminator.kind) {
                    let mut edge_live = live_in[edge.target.index() as usize].clone();
                    if let Some(destination) = edge.writes
                        && destination.projections.is_empty()
                    {
                        edge_live.remove(&destination.slot);
                    }
                    outgoing.extend(edge_live);
                }
                let mut incoming = uses[index].clone();
                incoming.extend(
                    outgoing
                        .iter()
                        .filter(|slot| !definitions[index].contains(slot))
                        .copied(),
                );
                if live_out[index] != outgoing {
                    live_out[index] = outgoing;
                    changed = true;
                }
                if live_in[index] != incoming {
                    live_in[index] = incoming;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            if !matches!(
                block.terminator.kind,
                BytecodeTerminatorKind::Await { .. } | BytecodeTerminatorKind::DrainScopes { .. }
            ) {
                continue;
            }
            let block_context = format!("{context} block#{index}");
            for slot in &live_out[index] {
                let ty = self.slot(function, *slot, &block_context)?.ty;
                if matches!(
                    self.ty(ty, &block_context)?.kind,
                    BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Join,
                        ..
                    }
                ) {
                    continue;
                }
                if !self.capability(ty, ClosedCapability::Send, &block_context)? {
                    return Err(BytecodeVerificationError::new(
                        &block_context,
                        format!("live slot#{} is not Send across suspension", slot.index()),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_instruction(
        &self,
        function: &BytecodeFunction,
        block_kind: BytecodeBlockKind,
        instruction: &BytecodeInstruction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        match &instruction.kind {
            BytecodeInstructionKind::StorageLive(slot)
            | BytecodeInstructionKind::StorageDead(slot) => {
                self.slot(function, *slot, context)?;
            }
            BytecodeInstructionKind::ReserveLoan(loan)
            | BytecodeInstructionKind::ReleaseLoan(loan) => {
                self.loan(function, *loan, context)?;
            }
            BytecodeInstructionKind::EnterTaskScope { .. } => {
                if block_kind != BytecodeBlockKind::Normal
                    || !self.function_is_async(function, context)?
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "task scope is entered outside ordinary async code",
                    ));
                }
            }
            BytecodeInstructionKind::Store { destination, value } => {
                self.verify_place(function, destination, context)?;
                self.verify_rvalue(function, value, context)?;
                if place_contains_ref_value(destination) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "`Ref[T].value` is a read-only projection",
                    ));
                }
                if destination.ty != value.ty {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "store destination and rvalue types differ",
                    ));
                }
            }
            BytecodeInstructionKind::RegisterDefer { action, guard, .. } => {
                if block_kind != BytecodeBlockKind::Normal {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "cleanup block registers another defer",
                    ));
                }
                if let BytecodeOperationKind::Call {
                    callee, protocol, ..
                } = &action.kind
                    && !self.capability(callee.ty, ClosedCapability::Copy, context)?
                    && *protocol != BytecodeCallProtocol::CallOnce
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "a non-Copy deferred callee does not use CallOnce",
                    ));
                }
                self.verify_operation(function, action, OperationContext::Deferred, context)?;
                if !self.is_scalar(action.ty, BytecodeScalarType::Unit)
                    || !matches!(
                        action.kind,
                        BytecodeOperationKind::Call { .. }
                            | BytecodeOperationKind::Assert { .. }
                            | BytecodeOperationKind::BootstrapHostCall { .. }
                    )
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer entry is not an infallible Unit invocation",
                    ));
                }
                if operation_operands(action).iter().any(|operand| {
                    matches!(
                        operand.kind,
                        BytecodeOperandKind::Borrow(_) | BytecodeOperandKind::Loan(_)
                    )
                }) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer entry retains a borrowed operand",
                    ));
                }
                if let BytecodeOperationKind::Call { arguments, .. } = &action.kind
                    && arguments
                        .iter()
                        .any(|argument| argument.mode != BytecodeParameterMode::Value)
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer call retains a non-value argument mode",
                    ));
                }
                let operands = operation_operands(action);
                let mut affine = Vec::new();
                for operand in &operands {
                    if self.capability(operand.ty, ClosedCapability::Copy, context)? {
                        if matches!(operand.kind, BytecodeOperandKind::Move(_)) {
                            return Err(BytecodeVerificationError::new(
                                context,
                                "defer must snapshot a Copy operand instead of moving it",
                            ));
                        }
                    } else {
                        affine.push(*operand);
                    }
                }
                if affine.len() > 1 || guard.is_some() != (affine.len() == 1) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer entry does not have exactly one guard for its affine operand",
                    ));
                }
                if let Some(guard) = guard {
                    self.verify_place(function, guard, context)?;
                    if !is_complete_defer_owner_place(guard) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "registered defer guard is not one complete owner place",
                        ));
                    }
                    if !matches!(
                        &affine[0].kind,
                        BytecodeOperandKind::Move(place) if place == guard
                    ) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "defer guard does not match exactly one moved invocation operand",
                        ));
                    }
                }
            }
            BytecodeInstructionKind::RegisterFallback { owner, .. } => {
                if block_kind != BytecodeBlockKind::Normal {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "cleanup block registers a terminal fallback",
                    ));
                }
                self.verify_place(function, owner, context)?;
                if owner.source_loan.is_some()
                    || self.terminal_status(owner.ty, context)? != BytecodeTerminalStatus::Present
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "terminal fallback owner is borrowed or has no closed terminal token",
                    ));
                }
            }
            BytecodeInstructionKind::RetargetCleanup { from, to } => {
                if block_kind != BytecodeBlockKind::Normal {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "cleanup block retargets a defer guard",
                    ));
                }
                self.verify_place(function, from, context)?;
                self.verify_place(function, to, context)?;
                if from.ty != to.ty
                    || !is_complete_defer_owner_place(from)
                    || !(is_complete_defer_owner_place(to) || is_iterator_defer_target(to))
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer retarget does not preserve one complete owner place",
                    ));
                }
            }
            BytecodeInstructionKind::DisarmCleanup(place) => {
                if block_kind != BytecodeBlockKind::Normal {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "cleanup block explicitly disarms a defer guard",
                    ));
                }
                self.verify_place(function, place, context)?;
            }
        }
        Ok(())
    }

    fn verify_place(
        &self,
        function: &BytecodeFunction,
        place: &BytecodePlace,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        self.function_type(function, place.ty, context)?;
        let mut current = self.slot(function, place.slot, context)?.ty;
        for (position, projection) in place.projections.iter().enumerate() {
            self.function_type(function, projection.ty, context)?;
            if let BytecodeProjectionKind::IteratorElement { index } = &projection.kind {
                let base = BytecodePlace {
                    slot: place.slot,
                    ty: current,
                    projections: place.projections[..position].to_vec(),
                    source_loan: place.source_loan,
                };
                self.verify_iterator_element_origin(function, &base, *index, context)?;
            }
            let expected = match &projection.kind {
                BytecodeProjectionKind::ClosureCapture { callable, index } => {
                    let callable = self.callable(*callable, context)?;
                    let closure = callable
                        .closure
                        .as_ref()
                        .ok_or_else(|| projection_error(context))?;
                    if closure.environment != current {
                        return Err(projection_error(context));
                    }
                    *closure
                        .captures
                        .get(*index as usize)
                        .ok_or_else(|| projection_error(context))?
                }
                BytecodeProjectionKind::Field(member) => {
                    let (_, arguments, metadata) = self.nominal_instance(current, context)?;
                    let BytecodeNominalShape::Record { fields } = &metadata.shape else {
                        return Err(projection_error(context));
                    };
                    let field = fields
                        .iter()
                        .find(|field| field.member == *member)
                        .ok_or_else(|| projection_error(context))?;
                    if !self.type_matches_substitution(
                        field.ty,
                        projection.ty,
                        arguments,
                        context,
                    )? {
                        return Err(projection_error(context));
                    }
                    projection.ty
                }
                BytecodeProjectionKind::TupleField(index) => {
                    let BytecodeTypeKind::Tuple(items) = &self.ty(current, context)?.kind else {
                        return Err(projection_error(context));
                    };
                    *items
                        .get(*index as usize)
                        .ok_or_else(|| projection_error(context))?
                }
                BytecodeProjectionKind::NewtypeValue => {
                    let (_, arguments, metadata) = self.nominal_instance(current, context)?;
                    let BytecodeNominalShape::Newtype { underlying } = &metadata.shape else {
                        return Err(projection_error(context));
                    };
                    if !self.type_matches_substitution(
                        *underlying,
                        projection.ty,
                        arguments,
                        context,
                    )? {
                        return Err(projection_error(context));
                    }
                    projection.ty
                }
                BytecodeProjectionKind::RefValue => {
                    self.intrinsic_argument(current, BytecodeIntrinsicType::Ref, 0, context)?
                }
                BytecodeProjectionKind::VariantTuple { variant, index } => {
                    let (_, arguments, metadata) = self.nominal_instance(current, context)?;
                    let declaration = enum_variant(metadata, *variant, context)?;
                    let BytecodeVariantPayload::Tuple(items) = &declaration.payload else {
                        return Err(projection_error(context));
                    };
                    let template = *items
                        .get(*index as usize)
                        .ok_or_else(|| projection_error(context))?;
                    if !self.type_matches_substitution(
                        template,
                        projection.ty,
                        arguments,
                        context,
                    )? {
                        return Err(projection_error(context));
                    }
                    projection.ty
                }
                BytecodeProjectionKind::VariantField { variant, field } => {
                    let (_, arguments, metadata) = self.nominal_instance(current, context)?;
                    let declaration = enum_variant(metadata, *variant, context)?;
                    let BytecodeVariantPayload::Record(fields) = &declaration.payload else {
                        return Err(projection_error(context));
                    };
                    let template = fields
                        .iter()
                        .find(|candidate| candidate.member == *field)
                        .map(|field| field.ty)
                        .ok_or_else(|| projection_error(context))?;
                    if !self.type_matches_substitution(
                        template,
                        projection.ty,
                        arguments,
                        context,
                    )? {
                        return Err(projection_error(context));
                    }
                    projection.ty
                }
                BytecodeProjectionKind::OptionValue => {
                    let BytecodeTypeKind::Option(item) = self.ty(current, context)?.kind else {
                        return Err(projection_error(context));
                    };
                    item
                }
                BytecodeProjectionKind::ResultOkValue => {
                    let BytecodeTypeKind::Result { success, .. } = self.ty(current, context)?.kind
                    else {
                        return Err(projection_error(context));
                    };
                    success
                }
                BytecodeProjectionKind::ResultErrValue => {
                    let BytecodeTypeKind::Result { error, .. } = self.ty(current, context)?.kind
                    else {
                        return Err(projection_error(context));
                    };
                    error
                }
                BytecodeProjectionKind::UnionValue(member) => {
                    let BytecodeTypeKind::Union(members) = &self.ty(current, context)?.kind else {
                        return Err(projection_error(context));
                    };
                    if !members.contains(member) {
                        return Err(projection_error(context));
                    }
                    *member
                }
                BytecodeProjectionKind::ArrayPatternIndex(_) => {
                    self.intrinsic_argument(current, BytecodeIntrinsicType::Array, 0, context)?
                }
                BytecodeProjectionKind::ArrayPatternRest { start, suffix } => {
                    start
                        .checked_add(*suffix)
                        .ok_or_else(|| projection_error(context))?;
                    let _ =
                        self.intrinsic_argument(current, BytecodeIntrinsicType::Array, 0, context)?;
                    current
                }
                BytecodeProjectionKind::IteratorElement { index } => {
                    let slot = self.slot(function, *index, context)?;
                    if !self.is_scalar(slot.ty, BytecodeScalarType::Int) {
                        return Err(projection_error(context));
                    }
                    let expected = self
                        .borrowed_collection_item_type(current, context)?
                        .ok_or_else(|| projection_error(context))?;
                    if expected != projection.ty {
                        return Err(projection_error(context));
                    }
                    expected
                }
                BytecodeProjectionKind::IteratorSource => {
                    let BytecodeTypeKind::Cursor { collection, .. } =
                        self.ty(current, context)?.kind
                    else {
                        return Err(projection_error(context));
                    };
                    if collection != projection.ty {
                        return Err(projection_error(context));
                    }
                    collection
                }
                BytecodeProjectionKind::Index { index, access } => {
                    if *access == BytecodeIndexAccess::String {
                        return Err(projection_error(context));
                    }
                    let index = self.slot(function, *index, context)?.ty;
                    self.index_result(current, index, *access, context)?
                }
                BytecodeProjectionKind::Slice { start, end, step } => {
                    let _ =
                        self.intrinsic_argument(current, BytecodeIntrinsicType::Array, 0, context)?;
                    for slot in start.iter().chain(end).chain(step) {
                        if !self.is_scalar(
                            self.slot(function, *slot, context)?.ty,
                            BytecodeScalarType::Int,
                        ) {
                            return Err(projection_error(context));
                        }
                    }
                    current
                }
            };
            if expected != projection.ty {
                return Err(projection_error(context));
            }
            current = expected;
        }
        if current != place.ty {
            return Err(projection_error(context));
        }
        if let Some(source) = place.source_loan {
            let source = self.loan(function, source, context)?;
            if source.kind != BytecodeLoanKind::Region {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place source is not a region loan",
                ));
            }
            let source = LocalAccess::from_place(&source.place);
            let access = LocalAccess::from_place(place);
            if source.slot != access.slot || !move_path_is_prefix(&source.path, &access.path) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place escapes the source region's reserved path",
                ));
            }
        }
        Ok(())
    }

    fn verify_operand(
        &self,
        function: &BytecodeFunction,
        operand: &BytecodeOperand,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        self.function_type(function, operand.ty, context)?;
        match &operand.kind {
            BytecodeOperandKind::Constant(value) => match value {
                BytecodeConstant::Unit if self.is_scalar(operand.ty, BytecodeScalarType::Unit) => {}
                BytecodeConstant::Bool(_)
                    if self.is_scalar(operand.ty, BytecodeScalarType::Bool) => {}
                BytecodeConstant::Integer(spelling) => {
                    let BytecodeTypeKind::Scalar(scalar) = &self.ty(operand.ty, context)?.kind
                    else {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate integer constant has a non-scalar type",
                        ));
                    };
                    let Some(value) = literal::integer(spelling) else {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate integer constant is malformed",
                        ));
                    };
                    if !integer_value_fits(value, *scalar) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            format!("immediate integer constant `{spelling}` exceeds `{scalar:?}`"),
                        ));
                    }
                }
                BytecodeConstant::Float(spelling) => {
                    let BytecodeTypeKind::Scalar(scalar) = &self.ty(operand.ty, context)?.kind
                    else {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate float constant has a non-scalar type",
                        ));
                    };
                    let single_precision = match scalar {
                        BytecodeScalarType::Float32 => true,
                        BytecodeScalarType::Float => false,
                        _ => {
                            return Err(BytecodeVerificationError::new(
                                context,
                                "immediate float constant has a non-float type",
                            ));
                        }
                    };
                    if literal::float(spelling, single_precision).is_none() {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate float constant is malformed, overflows, or has the wrong suffix",
                        ));
                    }
                }
                BytecodeConstant::Char(spelling) => {
                    if !self.is_scalar(operand.ty, BytecodeScalarType::Char) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate character constant has a non-Char type",
                        ));
                    }
                    if literal::character(spelling).is_none() {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate character constant is malformed",
                        ));
                    }
                }
                BytecodeConstant::String(spelling) => {
                    if !self.is_scalar(operand.ty, BytecodeScalarType::String) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate string constant has a non-String type",
                        ));
                    }
                    if literal::string(spelling).is_none() {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "immediate string constant is malformed",
                        ));
                    }
                }
                BytecodeConstant::Named(id) => {
                    let constant =
                        self.program
                            .constants
                            .get(id.index() as usize)
                            .ok_or_else(|| {
                                BytecodeVerificationError::new(
                                    context,
                                    format!("references unknown constant#{}", id.index()),
                                )
                            })?;
                    if constant.value.ty != operand.ty {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "named constant operand has the wrong type",
                        ));
                    }
                }
                _ => {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "immediate constant kind does not match its type",
                    ));
                }
            },
            BytecodeOperandKind::Copy(place)
            | BytecodeOperandKind::Move(place)
            | BytecodeOperandKind::Borrow(place) => {
                self.verify_place(function, place, context)?;
                if matches!(operand.kind, BytecodeOperandKind::Move(_))
                    && place_contains_ref_value(place)
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "`Ref[T].value` cannot be moved out of its identity cell",
                    ));
                }
                if place.ty != operand.ty {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "place operand changes its type",
                    ));
                }
                if matches!(operand.kind, BytecodeOperandKind::Copy(_))
                    && !self.capability(operand.ty, ClosedCapability::Copy, context)?
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "Copy operand type does not satisfy its closed Copy contract",
                    ));
                }
            }
            BytecodeOperandKind::Loan(loan) => {
                let loan = self.loan(function, *loan, context)?;
                if loan.kind != BytecodeLoanKind::CallLocal {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "region loan cannot be consumed as a call argument",
                    ));
                }
                if loan.place.ty != operand.ty {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "loan operand changes its reserved place type",
                    ));
                }
            }
            BytecodeOperandKind::Function {
                callable,
                arguments,
            } => {
                let callable = self.callable(*callable, context)?;
                for argument in arguments {
                    self.function_type(function, *argument, context)?;
                }
                if callable.closure.is_some()
                    || arguments.len() != callable.generic_arity as usize
                    || !self.representation_matches_substitution(
                        callable.function_type,
                        operand.ty,
                        arguments,
                        context,
                    )?
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "function operand does not match its callable specialization",
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_rvalue(
        &self,
        function: &BytecodeFunction,
        value: &BytecodeRvalue,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if rvalue_contains_invalid_borrow(value) {
            return Err(BytecodeVerificationError::new(
                context,
                "borrow escapes its permitted immediate observation",
            ));
        }
        self.function_type(function, value.ty, context)?;
        match &value.kind {
            BytecodeRvalueKind::Use(operand) => {
                self.verify_operand(function, operand, context)?;
                if operand.ty != value.ty {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeRvalueKind::Prefix { operator, operand } => {
                self.verify_operand(function, operand, context)?;
                self.verify_prefix(*operator, operand.ty, value.ty, context)?;
                if self.prefix_requires_checked(*operator, operand.ty) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "potentially panicking prefix operation is not Invoke",
                    ));
                }
            }
            BytecodeRvalueKind::Binary {
                operator,
                left,
                right,
            } => {
                self.verify_operand(function, left, context)?;
                self.verify_operand(function, right, context)?;
                self.verify_binary(*operator, left.ty, right.ty, value.ty, context)?;
                if self.binary_requires_checked(*operator, left.ty, right.ty) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "potentially panicking binary operation is not Invoke",
                    ));
                }
            }
            BytecodeRvalueKind::Construct { shape, values } => {
                for operand in values {
                    self.verify_operand(function, operand, context)?;
                }
                self.verify_aggregate(shape, values, value.ty, context)?;
            }
            BytecodeRvalueKind::RecordUpdate { base, fields } => {
                self.verify_operand(function, base, context)?;
                if base.ty != value.ty {
                    return Err(rvalue_error(context));
                }
                let (_, arguments, metadata) = self.nominal_instance(value.ty, context)?;
                let BytecodeNominalShape::Record { fields: declared } = &metadata.shape else {
                    return Err(rvalue_error(context));
                };
                let mut seen = BTreeSet::new();
                for (member, operand) in fields {
                    self.verify_operand(function, operand, context)?;
                    let Some(field) = declared.iter().find(|field| field.member == *member) else {
                        return Err(rvalue_error(context));
                    };
                    if !seen.insert(*member)
                        || !self
                            .type_matches_substitution(field.ty, operand.ty, arguments, context)?
                    {
                        return Err(rvalue_error(context));
                    }
                }
            }
            BytecodeRvalueKind::Coerce {
                kind,
                value: operand,
            } => {
                self.verify_operand(function, operand, context)?;
                if self.assignability(operand.ty, value.ty, context)? != Some(*kind)
                    || *kind == BytecodeCoercion::Exact
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeRvalueKind::NumericConversion {
                target,
                conversion,
                value: operand,
            } => {
                self.verify_operand(function, operand, context)?;
                self.verify_numeric_conversion(
                    operand.ty,
                    *target,
                    *conversion,
                    value.ty,
                    context,
                )?;
            }
            BytecodeRvalueKind::Range { start, end, .. } => {
                self.verify_operand(function, start, context)?;
                self.verify_operand(function, end, context)?;
                let element =
                    self.intrinsic_argument(value.ty, BytecodeIntrinsicType::Range, 0, context)?;
                if start.ty != end.ty || start.ty != element {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeRvalueKind::Contains {
                kind,
                item,
                container,
            } => {
                self.verify_operand(function, item, context)?;
                self.verify_operand(function, container, context)?;
                self.verify_contains(*kind, item.ty, container.ty, value.ty, context)?;
            }
            BytecodeRvalueKind::MapRemove { map, key } => {
                self.verify_place(function, map, context)?;
                self.verify_operand(function, key, context)?;
                let map_key =
                    self.intrinsic_argument(map.ty, BytecodeIntrinsicType::Map, 0, context)?;
                let map_value =
                    self.intrinsic_argument(map.ty, BytecodeIntrinsicType::Map, 1, context)?;
                let BytecodeTypeKind::Option(result_value) = self.ty(value.ty, context)?.kind
                else {
                    return Err(rvalue_error(context));
                };
                let source = map
                    .source_loan
                    .and_then(|source| function.loans.get(source.index() as usize));
                if key.ty != map_key
                    || result_value != map_value
                    || !source.is_some_and(|source| {
                        source.kind == BytecodeLoanKind::Region
                            && source.mode == BytecodeParameterMode::Var
                            && same_place_path(&source.place, map)
                    })
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeRvalueKind::Interpolate { segments, values } => {
                if !self.is_scalar(value.ty, BytecodeScalarType::String)
                    || segments.len() != values.len() + 1
                {
                    return Err(rvalue_error(context));
                }
                for operand in values {
                    self.verify_operand(function, operand, context)?;
                    if !self.is_scalar(operand.ty, BytecodeScalarType::String) {
                        return Err(rvalue_error(context));
                    }
                }
            }
            BytecodeRvalueKind::Length(operand) => {
                self.verify_operand(function, operand, context)?;
                if !self.is_scalar(value.ty, BytecodeScalarType::Int)
                    || (!self.is_scalar(operand.ty, BytecodeScalarType::String)
                        && self
                            .intrinsic_argument(
                                operand.ty,
                                BytecodeIntrinsicType::Array,
                                0,
                                context,
                            )
                            .is_err())
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeRvalueKind::IteratorState(source) => {
                self.verify_operand(function, source, context)?;
                let BytecodeTypeKind::Cursor { mode, collection } =
                    &self.ty(value.ty, context)?.kind
                else {
                    return Err(rvalue_error(context));
                };
                let borrows = matches!(source.kind, BytecodeOperandKind::Borrow(_));
                if *collection != source.ty
                    || (*mode != BytecodeCursorMode::Own) != borrows
                    || self
                        .iterated_collection_item_type(source.ty, context)?
                        .is_none()
                {
                    return Err(rvalue_error(context));
                }
            }
        }
        Ok(())
    }

    fn verify_aggregate(
        &self,
        shape: &BytecodeAggregateKind,
        values: &[BytecodeOperand],
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        match shape {
            BytecodeAggregateKind::Tuple => {
                let BytecodeTypeKind::Tuple(items) = &self.ty(result, context)?.kind else {
                    return Err(rvalue_error(context));
                };
                self.verify_operand_types(values, items, context)?;
            }
            BytecodeAggregateKind::Array => {
                let item =
                    self.intrinsic_argument(result, BytecodeIntrinsicType::Array, 0, context)?;
                self.verify_repeated_operands(values, item, context)?;
            }
            BytecodeAggregateKind::Set => {
                let item =
                    self.intrinsic_argument(result, BytecodeIntrinsicType::Set, 0, context)?;
                self.verify_repeated_operands(values, item, context)?;
            }
            BytecodeAggregateKind::Closure { callable, captures } => {
                let callable = self.callable(*callable, context)?;
                let closure = callable
                    .closure
                    .as_ref()
                    .ok_or_else(|| rvalue_error(context))?;
                if closure.environment != result
                    || closure.captures != *captures
                    || captures.len() != values.len()
                    || captures
                        .iter()
                        .zip(values)
                        .any(|(expected, value)| *expected != value.ty)
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeAggregateKind::Newtype { nominal } => {
                let (actual, arguments, metadata) = self.nominal_instance(result, context)?;
                let BytecodeNominalShape::Newtype { underlying } = &metadata.shape else {
                    return Err(rvalue_error(context));
                };
                if actual != *nominal
                    || values.len() != 1
                    || !self.type_matches_substitution(
                        *underlying,
                        values[0].ty,
                        arguments,
                        context,
                    )?
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeAggregateKind::Ref => {
                let target =
                    self.intrinsic_argument(result, BytecodeIntrinsicType::Ref, 0, context)?;
                self.verify_operand_types(values, &[target], context)?;
            }
            BytecodeAggregateKind::Record { nominal, fields } => {
                let (actual, arguments, metadata) = self.nominal_instance(result, context)?;
                let BytecodeNominalShape::Record { fields: declared } = &metadata.shape else {
                    return Err(rvalue_error(context));
                };
                if actual != *nominal
                    || fields.len() != declared.len()
                    || values.len() != declared.len()
                {
                    return Err(rvalue_error(context));
                }
                for ((member, value), declaration) in fields.iter().zip(values).zip(declared) {
                    if *member != declaration.member
                        || !self.type_matches_substitution(
                            declaration.ty,
                            value.ty,
                            arguments,
                            context,
                        )?
                    {
                        return Err(rvalue_error(context));
                    }
                }
            }
            BytecodeAggregateKind::Variant { variant, fields } => {
                if self.numeric_conversion_error_variant(result, *variant, context)? {
                    if !fields.is_empty() || !values.is_empty() {
                        return Err(rvalue_error(context));
                    }
                } else {
                    let (_, arguments, metadata) = self.nominal_instance(result, context)?;
                    let declaration = enum_variant(metadata, *variant, context)?;
                    match &declaration.payload {
                        BytecodeVariantPayload::Unit if fields.is_empty() && values.is_empty() => {}
                        BytecodeVariantPayload::Tuple(items)
                            if fields.len() == items.len()
                                && fields.iter().all(Option::is_none)
                                && values.len() == items.len() =>
                        {
                            for (template, value) in items.iter().zip(values) {
                                if !self.type_matches_substitution(
                                    *template, value.ty, arguments, context,
                                )? {
                                    return Err(rvalue_error(context));
                                }
                            }
                        }
                        BytecodeVariantPayload::Record(declared)
                            if fields.len() == declared.len() && values.len() == declared.len() =>
                        {
                            for ((member, value), declaration) in
                                fields.iter().zip(values).zip(declared)
                            {
                                if *member != Some(declaration.member)
                                    || !self.type_matches_substitution(
                                        declaration.ty,
                                        value.ty,
                                        arguments,
                                        context,
                                    )?
                                {
                                    return Err(rvalue_error(context));
                                }
                            }
                        }
                        _ => return Err(rvalue_error(context)),
                    }
                }
            }
            BytecodeAggregateKind::OptionNone => {
                if !values.is_empty()
                    || !matches!(self.ty(result, context)?.kind, BytecodeTypeKind::Option(_))
                {
                    return Err(rvalue_error(context));
                }
            }
            BytecodeAggregateKind::OptionSome => {
                let BytecodeTypeKind::Option(item) = self.ty(result, context)?.kind else {
                    return Err(rvalue_error(context));
                };
                self.verify_operand_types(values, &[item], context)?;
            }
            BytecodeAggregateKind::ResultOk => {
                let BytecodeTypeKind::Result { success, .. } = self.ty(result, context)?.kind
                else {
                    return Err(rvalue_error(context));
                };
                self.verify_operand_types(values, &[success], context)?;
            }
            BytecodeAggregateKind::ResultErr => {
                let BytecodeTypeKind::Result { error, .. } = self.ty(result, context)?.kind else {
                    return Err(rvalue_error(context));
                };
                self.verify_operand_types(values, &[error], context)?;
            }
        }
        Ok(())
    }

    fn verify_operand_types(
        &self,
        values: &[BytecodeOperand],
        expected: &[BytecodeTypeId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if values.len() != expected.len()
            || values
                .iter()
                .zip(expected)
                .any(|(value, expected)| value.ty != *expected)
        {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn verify_repeated_operands(
        &self,
        values: &[BytecodeOperand],
        expected: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if values.iter().any(|value| value.ty != expected) {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn verify_prefix(
        &self,
        operator: BytecodePrefixOperator,
        operand: BytecodeTypeId,
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let BytecodeTypeKind::Scalar(scalar) = self.ty(operand, context)?.kind else {
            return Err(rvalue_error(context));
        };
        let valid = match operator {
            BytecodePrefixOperator::LogicalNot => scalar == BytecodeScalarType::Bool,
            BytecodePrefixOperator::Negate => is_signed_integer(scalar) || is_float(scalar),
            BytecodePrefixOperator::BitwiseNot => {
                is_integer(scalar) || scalar == BytecodeScalarType::Byte
            }
        };
        let expected = if operator == BytecodePrefixOperator::LogicalNot {
            self.scalar_id(BytecodeScalarType::Bool, context)?
        } else {
            operand
        };
        if !valid || result != expected {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn verify_binary(
        &self,
        operator: BytecodeBinaryOperator,
        left: BytecodeTypeId,
        right: BytecodeTypeId,
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if !self.binary_result_matches(operator, left, right, result, context)? {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn binary_result_matches(
        &self,
        operator: BytecodeBinaryOperator,
        left: BytecodeTypeId,
        right: BytecodeTypeId,
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let arithmetic = matches!(
            operator,
            BytecodeBinaryOperator::Add
                | BytecodeBinaryOperator::Subtract
                | BytecodeBinaryOperator::Multiply
                | BytecodeBinaryOperator::Divide
                | BytecodeBinaryOperator::Remainder
        );
        let left_array = self.array_element(left);
        let right_array = self.array_element(right);
        if arithmetic && (left_array.is_some() || right_array.is_some()) {
            let Some(result_element) = self.array_element(result) else {
                return Ok(false);
            };
            return self.binary_result_matches(
                operator,
                left_array.unwrap_or(left),
                right_array.unwrap_or(right),
                result_element,
                context,
            );
        }
        let left_scalar = scalar_kind(self.ty(left, context)?);
        let right_scalar = scalar_kind(self.ty(right, context)?);
        if left != right
            && !matches!(
                operator,
                BytecodeBinaryOperator::ShiftLeft | BytecodeBinaryOperator::ShiftRight
            )
        {
            return Ok(false);
        }
        let valid = match operator {
            BytecodeBinaryOperator::Multiply
            | BytecodeBinaryOperator::Divide
            | BytecodeBinaryOperator::Add
            | BytecodeBinaryOperator::Subtract => left_scalar.is_some_and(is_arithmetic),
            BytecodeBinaryOperator::Remainder => left_scalar.is_some_and(is_integer),
            BytecodeBinaryOperator::ShiftLeft | BytecodeBinaryOperator::ShiftRight => {
                left_scalar
                    .is_some_and(|scalar| is_integer(scalar) || scalar == BytecodeScalarType::Byte)
                    && right_scalar.is_some_and(is_integer)
            }
            BytecodeBinaryOperator::BitwiseAnd
            | BytecodeBinaryOperator::BitwiseXor
            | BytecodeBinaryOperator::BitwiseOr => left_scalar
                .is_some_and(|scalar| is_integer(scalar) || scalar == BytecodeScalarType::Byte),
            BytecodeBinaryOperator::Less
            | BytecodeBinaryOperator::LessEqual
            | BytecodeBinaryOperator::Greater
            | BytecodeBinaryOperator::GreaterEqual => left_scalar.is_some_and(is_relational),
            BytecodeBinaryOperator::Equal | BytecodeBinaryOperator::NotEqual => {
                self.capability(left, ClosedCapability::Equatable, context)?
            }
            BytecodeBinaryOperator::LogicalAnd | BytecodeBinaryOperator::LogicalOr => {
                left_scalar == Some(BytecodeScalarType::Bool)
            }
        };
        if !valid {
            return Ok(false);
        }
        let expected = if matches!(
            operator,
            BytecodeBinaryOperator::Less
                | BytecodeBinaryOperator::LessEqual
                | BytecodeBinaryOperator::Greater
                | BytecodeBinaryOperator::GreaterEqual
                | BytecodeBinaryOperator::Equal
                | BytecodeBinaryOperator::NotEqual
                | BytecodeBinaryOperator::LogicalAnd
                | BytecodeBinaryOperator::LogicalOr
        ) {
            self.scalar_id(BytecodeScalarType::Bool, context)?
        } else {
            left
        };
        Ok(result == expected)
    }

    fn prefix_requires_checked(
        &self,
        operator: BytecodePrefixOperator,
        operand: BytecodeTypeId,
    ) -> bool {
        operator == BytecodePrefixOperator::Negate
            && matches!(
                self.program.ty(operand).map(|ty| &ty.kind),
                Some(BytecodeTypeKind::Scalar(
                    BytecodeScalarType::Int
                        | BytecodeScalarType::Int8
                        | BytecodeScalarType::Int16
                        | BytecodeScalarType::Int32
                ))
            )
    }

    fn binary_requires_checked(
        &self,
        operator: BytecodeBinaryOperator,
        left: BytecodeTypeId,
        right: BytecodeTypeId,
    ) -> bool {
        matches!(
            operator,
            BytecodeBinaryOperator::Multiply
                | BytecodeBinaryOperator::Divide
                | BytecodeBinaryOperator::Remainder
                | BytecodeBinaryOperator::Add
                | BytecodeBinaryOperator::Subtract
                | BytecodeBinaryOperator::ShiftLeft
                | BytecodeBinaryOperator::ShiftRight
        ) && (self.array_element(left).is_some()
            || self.array_element(right).is_some()
            || !matches!(
                self.program.ty(left).map(|ty| &ty.kind),
                Some(BytecodeTypeKind::Scalar(
                    BytecodeScalarType::Float | BytecodeScalarType::Float32
                ))
            ))
    }

    fn assignability(
        &self,
        actual: BytecodeTypeId,
        expected: BytecodeTypeId,
        context: &str,
    ) -> Result<Option<BytecodeCoercion>, BytecodeVerificationError> {
        if actual == expected {
            return Ok(Some(BytecodeCoercion::Exact));
        }
        if self.is_scalar(actual, BytecodeScalarType::Never) {
            return Ok(Some(BytecodeCoercion::Diverging));
        }
        if self.callable_erasure_matches(actual, expected, context)? {
            return Ok(Some(BytecodeCoercion::CallableErasure));
        }
        if self.opaque_coercion_matches(actual, expected, context)? {
            return Ok(Some(BytecodeCoercion::Opaque));
        }
        if let BytecodeTypeKind::Union(expected_members) = &self.ty(expected, context)?.kind {
            let actual_members = match &self.ty(actual, context)?.kind {
                BytecodeTypeKind::Union(members) => members.as_slice(),
                _ => std::slice::from_ref(&actual),
            };
            if actual_members
                .iter()
                .all(|member| expected_members.contains(member))
            {
                return Ok(Some(if actual_members.len() == 1 {
                    BytecodeCoercion::UnionInjection
                } else {
                    BytecodeCoercion::UnionWidening
                }));
            }
        }
        if matches!(&self.ty(expected, context)?.kind, BytecodeTypeKind::Option(item) if *item == actual)
        {
            return Ok(Some(BytecodeCoercion::OptionLift));
        }
        Ok(None)
    }

    fn callable_erasure_matches(
        &self,
        actual: BytecodeTypeId,
        expected: BytecodeTypeId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        if !matches!(
            self.ty(expected, context)?.kind,
            BytecodeTypeKind::Function(_)
        ) {
            return Ok(false);
        }
        let Some(callable) = self.closure_callable_for_type(actual, context)? else {
            return Ok(false);
        };
        let closure = callable
            .closure
            .as_ref()
            .expect("closure lookup only returns closure callables");
        if callable.function_type != expected || !closure.protocols.call {
            return Ok(false);
        }
        for capture in &closure.captures {
            for capability in [
                ClosedCapability::Copy,
                ClosedCapability::Send,
                ClosedCapability::Share,
            ] {
                if !self.capability(*capture, capability, context)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn opaque_coercion_matches(
        &self,
        actual: BytecodeTypeId,
        expected: BytecodeTypeId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        if matches!(
            &self.ty(expected, context)?.kind,
            BytecodeTypeKind::OpaqueResult { witness, .. } if *witness == actual
        ) {
            return Ok(true);
        }
        let (
            BytecodeTypeKind::Result {
                success: actual_success,
                error: actual_error,
            },
            BytecodeTypeKind::Result {
                success: expected_success,
                error: expected_error,
            },
        ) = (
            &self.ty(actual, context)?.kind,
            &self.ty(expected, context)?.kind,
        )
        else {
            return Ok(false);
        };
        Ok(actual_error == expected_error
            && matches!(
                &self.ty(*expected_success, context)?.kind,
                BytecodeTypeKind::OpaqueResult { witness, .. } if witness == actual_success
            ))
    }

    fn verify_numeric_conversion(
        &self,
        source: BytecodeTypeId,
        target: BytecodeScalarType,
        conversion: BytecodeNumericConversion,
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let Some(source) = scalar_kind(self.ty(source, context)?) else {
            return Err(rvalue_error(context));
        };
        if classify_numeric_conversion(source, target) != Some(conversion) {
            return Err(rvalue_error(context));
        }
        let target_type = self.scalar_id(target, context)?;
        let valid_result = if conversion == BytecodeNumericConversion::Checked {
            matches!(
                &self.ty(result, context)?.kind,
                BytecodeTypeKind::Result { success, error }
                    if *success == target_type
                        && matches!(
                            &self.ty(*error, context)?.kind,
                            BytecodeTypeKind::Intrinsic {
                                constructor: BytecodeIntrinsicType::NumericConversionError,
                                arguments,
                            } if arguments.is_empty()
                        )
            )
        } else {
            result == target_type
        };
        if !valid_result {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn verify_contains(
        &self,
        kind: BytecodeContainmentKind,
        item: BytecodeTypeId,
        container: BytecodeTypeId,
        result: BytecodeTypeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let expected = match kind {
            BytecodeContainmentKind::Array => {
                self.intrinsic_argument(container, BytecodeIntrinsicType::Array, 0, context)?
            }
            BytecodeContainmentKind::MapKey => {
                self.intrinsic_argument(container, BytecodeIntrinsicType::Map, 0, context)?
            }
            BytecodeContainmentKind::Set => {
                self.intrinsic_argument(container, BytecodeIntrinsicType::Set, 0, context)?
            }
            BytecodeContainmentKind::Range => {
                self.intrinsic_argument(container, BytecodeIntrinsicType::Range, 0, context)?
            }
            BytecodeContainmentKind::StringChar => {
                if !self.is_scalar(container, BytecodeScalarType::String) {
                    return Err(rvalue_error(context));
                }
                self.scalar_id(BytecodeScalarType::Char, context)?
            }
        };
        if item != expected || !self.is_scalar(result, BytecodeScalarType::Bool) {
            return Err(rvalue_error(context));
        }
        let required = match kind {
            BytecodeContainmentKind::Array => Some(ClosedCapability::Equatable),
            BytecodeContainmentKind::MapKey | BytecodeContainmentKind::Set => {
                Some(ClosedCapability::Key)
            }
            BytecodeContainmentKind::Range | BytecodeContainmentKind::StringChar => None,
        };
        if let Some(capability) = required
            && !self.capability(expected, capability, context)?
        {
            return Err(rvalue_error(context));
        }
        Ok(())
    }

    fn index_result(
        &self,
        base: BytecodeTypeId,
        index: BytecodeTypeId,
        access: BytecodeIndexAccess,
        context: &str,
    ) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        match access {
            BytecodeIndexAccess::Array => {
                if !self.is_scalar(index, BytecodeScalarType::Int) {
                    return Err(projection_error(context));
                }
                self.intrinsic_argument(base, BytecodeIntrinsicType::Array, 0, context)
            }
            BytecodeIndexAccess::String => {
                if !self.is_scalar(base, BytecodeScalarType::String)
                    || !self.is_scalar(index, BytecodeScalarType::Int)
                {
                    return Err(projection_error(context));
                }
                self.find_type(
                    |kind| matches!(kind, BytecodeTypeKind::Scalar(BytecodeScalarType::Char)),
                    context,
                )
            }
            BytecodeIndexAccess::MapLookup | BytecodeIndexAccess::MapEntry => {
                let key = self.intrinsic_argument(base, BytecodeIntrinsicType::Map, 0, context)?;
                let value =
                    self.intrinsic_argument(base, BytecodeIntrinsicType::Map, 1, context)?;
                if index != key {
                    return Err(projection_error(context));
                }
                if access == BytecodeIndexAccess::MapEntry {
                    Ok(value)
                } else {
                    if !self.capability(value, ClosedCapability::Copy, context)? {
                        return Err(projection_error(context));
                    }
                    self.find_type(
                        |kind| matches!(kind, BytecodeTypeKind::Option(item) if *item == value),
                        context,
                    )
                }
            }
        }
    }

    fn iterated_item_type(
        &self,
        cursor: BytecodeTypeId,
        context: &str,
    ) -> Result<Option<BytecodeTypeId>, BytecodeVerificationError> {
        let BytecodeTypeKind::Cursor { collection, .. } = self.ty(cursor, context)?.kind else {
            return Ok(None);
        };
        self.iterated_collection_item_type(collection, context)
    }

    fn borrowed_collection_item_type(
        &self,
        collection: BytecodeTypeId,
        context: &str,
    ) -> Result<Option<BytecodeTypeId>, BytecodeVerificationError> {
        let item = match &self.ty(collection, context)?.kind {
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Array | BytecodeIntrinsicType::Set,
                arguments,
            } => arguments.first().copied(),
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Map,
                arguments,
            } => Some(self.find_type(
                |kind| matches!(kind, BytecodeTypeKind::Tuple(items) if items == arguments),
                context,
            )?),
            _ => None,
        };
        Ok(item)
    }

    fn verify_exclusive_iterator_loan_path(
        &self,
        function: &BytecodeFunction,
        place: &BytecodePlace,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let mut current = self.slot(function, place.slot, context)?.ty;
        for (index, projection) in place.projections.iter().enumerate() {
            if matches!(
                projection.kind,
                BytecodeProjectionKind::IteratorElement { .. }
            ) {
                match &self.ty(current, context)?.kind {
                    BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Array,
                        ..
                    } => {}
                    BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Map,
                        ..
                    } if matches!(
                        place.projections.get(index + 1).map(|next| &next.kind),
                        Some(BytecodeProjectionKind::TupleField(1))
                    ) => {}
                    BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Map,
                        ..
                    } => {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "exclusive Map iterator loan does not project through its value",
                        ));
                    }
                    _ => {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "exclusive iterator loan has a non-mutable collection source",
                        ));
                    }
                }
                return Ok(());
            }
            current = projection.ty;
        }
        Ok(())
    }

    fn verify_borrowed_iterator_origin(
        &self,
        function: &BytecodeFunction,
        state: &BytecodePlace,
        destination: &BytecodePlace,
        source: &BytecodePlace,
        mode: BytecodeCursorMode,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if !state.projections.is_empty()
            || state.source_loan.is_some()
            || !destination.projections.is_empty()
            || destination.source_loan.is_some()
            || self.slot(function, state.slot, context)?.kind != BytecodeSlotKind::Temporary
            || self.slot(function, destination.slot, context)?.kind != BytecodeSlotKind::Temporary
        {
            return Err(terminator_error(context));
        }
        let loan = self.loan(
            function,
            source
                .source_loan
                .ok_or_else(|| terminator_error(context))?,
            context,
        )?;
        let expected_mode = match mode {
            BytecodeCursorMode::Ref => BytecodeParameterMode::Ref,
            BytecodeCursorMode::Mut => BytecodeParameterMode::Mut,
            BytecodeCursorMode::Own => return Err(terminator_error(context)),
        };
        if loan.kind != BytecodeLoanKind::Region
            || loan.mode != expected_mode
            || !same_place_path(&loan.place, source)
        {
            return Err(terminator_error(context));
        }
        let mut state_definitions = 0_u32;
        let mut position_definitions = 0_u32;
        for block in &function.blocks {
            self.consume_dataflow_step(context)?;
            for instruction in &block.instructions {
                self.consume_dataflow_step(context)?;
                let BytecodeInstructionKind::Store {
                    destination: assigned,
                    value,
                } = &instruction.kind
                else {
                    continue;
                };
                if assigned.slot == destination.slot {
                    return Err(terminator_error(context));
                }
                if assigned.slot == state.slot {
                    let matches_origin = assigned.projections.is_empty()
                        && assigned.source_loan.is_none()
                        && matches!(
                            &value.kind,
                            BytecodeRvalueKind::IteratorState(BytecodeOperand {
                                kind: BytecodeOperandKind::Borrow(origin),
                                ..
                            }) if origin == source
                        );
                    if !matches_origin {
                        return Err(terminator_error(context));
                    }
                    state_definitions = state_definitions.saturating_add(1);
                }
            }
            match &block.terminator.kind {
                BytecodeTerminatorKind::IteratorNext {
                    state: candidate_state,
                    destination: assigned,
                    borrowed_source: Some(candidate_source),
                    ..
                } if assigned.slot == destination.slot => {
                    if candidate_state != state
                        || assigned != destination
                        || candidate_source != source
                    {
                        return Err(terminator_error(context));
                    }
                    position_definitions = position_definitions.saturating_add(1);
                }
                BytecodeTerminatorKind::IteratorNext {
                    destination: assigned,
                    ..
                }
                | BytecodeTerminatorKind::Invoke {
                    destination: Some(assigned),
                    ..
                } if assigned.slot == destination.slot => {
                    return Err(terminator_error(context));
                }
                BytecodeTerminatorKind::IteratorNext {
                    destination: assigned,
                    ..
                }
                | BytecodeTerminatorKind::Invoke {
                    destination: Some(assigned),
                    ..
                } if assigned.slot == state.slot => {
                    return Err(terminator_error(context));
                }
                _ => {}
            }
        }
        if state_definitions != 1 || position_definitions != 1 {
            return Err(terminator_error(context));
        }
        Ok(())
    }

    fn verify_iterator_element_origin(
        &self,
        function: &BytecodeFunction,
        base: &BytecodePlace,
        index: BytecodeSlotId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let origin_loan = base.source_loan.ok_or_else(|| projection_error(context))?;
        let mut producers = 0_usize;
        for block in &function.blocks {
            self.consume_dataflow_step(context)?;
            let BytecodeTerminatorKind::IteratorNext {
                destination,
                borrowed_source: Some(source),
                ..
            } = &block.terminator.kind
            else {
                continue;
            };
            if destination.slot == index
                && destination.projections.is_empty()
                && destination.source_loan.is_none()
                && same_place_path(base, source)
            {
                let expected_loan = source
                    .source_loan
                    .ok_or_else(|| projection_error(context))?;
                if !self.region_loan_descends_from(function, origin_loan, expected_loan, context)? {
                    return Err(projection_error(context));
                }
                producers = producers.saturating_add(1);
            }
        }
        if producers != 1 {
            return Err(projection_error(context));
        }
        Ok(())
    }

    fn region_loan_descends_from(
        &self,
        function: &BytecodeFunction,
        mut candidate: BytecodeLoanId,
        ancestor: BytecodeLoanId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let mut seen = BTreeSet::new();
        loop {
            self.consume_dataflow_step(context)?;
            if candidate == ancestor {
                return Ok(true);
            }
            if !seen.insert(candidate) {
                return Err(projection_error(context));
            }
            let loan = self.loan(function, candidate, context)?;
            if loan.kind != BytecodeLoanKind::Region {
                return Ok(false);
            }
            let Some(parent) = loan.place.source_loan else {
                return Ok(false);
            };
            candidate = parent;
        }
    }

    fn iterated_collection_item_type(
        &self,
        collection: BytecodeTypeId,
        context: &str,
    ) -> Result<Option<BytecodeTypeId>, BytecodeVerificationError> {
        let result = match &self.ty(collection, context)?.kind {
            BytecodeTypeKind::Intrinsic {
                constructor:
                    BytecodeIntrinsicType::Array
                    | BytecodeIntrinsicType::Set
                    | BytecodeIntrinsicType::Range,
                arguments,
            } => arguments.first().copied(),
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Map,
                arguments,
            } => Some(self.find_type(
                |kind| matches!(kind, BytecodeTypeKind::Tuple(items) if items == arguments),
                context,
            )?),
            BytecodeTypeKind::Scalar(BytecodeScalarType::String) => {
                Some(self.scalar_id(BytecodeScalarType::Char, context)?)
            }
            _ => None,
        };
        Ok(result)
    }

    fn scalar_id(
        &self,
        scalar: BytecodeScalarType,
        context: &str,
    ) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        self.find_type(
            |kind| matches!(kind, BytecodeTypeKind::Scalar(candidate) if *candidate == scalar),
            context,
        )
    }

    fn find_type(
        &self,
        predicate: impl Fn(&BytecodeTypeKind) -> bool,
        context: &str,
    ) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        self.program
            .types
            .iter()
            .position(|ty| predicate(&ty.kind))
            .map(|index| BytecodeTypeId::new(index as u32))
            .ok_or_else(|| BytecodeVerificationError::new(context, "required type is absent"))
    }

    fn array_element(&self, ty: BytecodeTypeId) -> Option<BytecodeTypeId> {
        match self.program.ty(ty).map(|ty| &ty.kind) {
            Some(BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Array,
                arguments,
            }) => arguments.first().copied(),
            _ => None,
        }
    }

    fn is_scalar(&self, ty: BytecodeTypeId, scalar: BytecodeScalarType) -> bool {
        matches!(
            self.program.ty(ty).map(|ty| &ty.kind),
            Some(BytecodeTypeKind::Scalar(candidate)) if *candidate == scalar
        )
    }

    fn function_is_async(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let callable = self.callable(function.callable, context)?;
        let BytecodeTypeKind::Function(signature) = &self.ty(callable.function_type, context)?.kind
        else {
            return Err(BytecodeVerificationError::new(
                context,
                "function callable has a non-function signature",
            ));
        };
        Ok(signature.is_async)
    }

    fn join_logical_outcome(
        &self,
        join: BytecodeTypeId,
        context: &str,
    ) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        let BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Join,
            arguments,
        } = &self.ty(join, context)?.kind
        else {
            return Err(BytecodeVerificationError::new(
                context,
                "await operand is not Join",
            ));
        };
        let [success, error] = arguments.as_slice() else {
            return Err(BytecodeVerificationError::new(
                context,
                "Join has the wrong intrinsic arity",
            ));
        };
        if self.is_scalar(*error, BytecodeScalarType::Never) {
            return Ok(*success);
        }
        self.find_type(
            |kind| {
                matches!(
                    kind,
                    BytecodeTypeKind::Result {
                        success: candidate_success,
                        error: candidate_error,
                    } if candidate_success == success && candidate_error == error
                )
            },
            context,
        )
    }

    fn is_join_for_outcome(
        &self,
        join: BytecodeTypeId,
        outcome: BytecodeTypeId,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        let BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Join,
            arguments,
        } = &self.ty(join, context)?.kind
        else {
            return Ok(false);
        };
        let (success, error) = match &self.ty(outcome, context)?.kind {
            BytecodeTypeKind::Result { success, error } => (*success, *error),
            _ => (
                outcome,
                self.find_type(
                    |kind| matches!(kind, BytecodeTypeKind::Scalar(BytecodeScalarType::Never)),
                    context,
                )?,
            ),
        };
        Ok(arguments.as_slice() == [success, error])
    }

    fn verify_spawn_transfer(
        &self,
        operation: &BytecodeOperation,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let BytecodeOperationKind::Call {
            callee, arguments, ..
        } = &operation.kind
        else {
            return Err(operation_error(context));
        };
        if !self.capability(callee.ty, ClosedCapability::Send, context)? {
            return Err(BytecodeVerificationError::new(
                context,
                "spawn callee is not Send",
            ));
        }
        for argument in arguments {
            let send = self.capability(argument.value.ty, ClosedCapability::Send, context)?;
            let share = self.capability(argument.value.ty, ClosedCapability::Share, context)?;
            if !send
                || argument.mode == BytecodeParameterMode::Ref && !share
                || matches!(
                    argument.mode,
                    BytecodeParameterMode::Mut | BytecodeParameterMode::Var
                )
            {
                return Err(BytecodeVerificationError::new(
                    context,
                    "spawn argument violates Send/Share or exclusive-loan rules",
                ));
            }
        }
        let (success, error) = match &self.ty(operation.ty, context)?.kind {
            BytecodeTypeKind::Result { success, error } => (*success, *error),
            _ => (
                operation.ty,
                self.find_type(
                    |kind| matches!(kind, BytecodeTypeKind::Scalar(BytecodeScalarType::Never)),
                    context,
                )?,
            ),
        };
        if !self.capability(success, ClosedCapability::Send, context)?
            || !self.capability(error, ClosedCapability::Send, context)?
        {
            return Err(BytecodeVerificationError::new(
                context,
                "spawn result or error is not Send",
            ));
        }
        Ok(())
    }

    fn verify_operation(
        &self,
        function: &BytecodeFunction,
        operation: &BytecodeOperation,
        operation_context: OperationContext,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if operation_contains_invalid_borrow(operation) {
            return Err(BytecodeVerificationError::new(
                context,
                "borrow escapes its permitted immediate operation",
            ));
        }
        self.function_type(function, operation.ty, context)?;
        if operation_context.expects_async()
            && !matches!(operation.kind, BytecodeOperationKind::Call { .. })
        {
            return Err(BytecodeVerificationError::new(
                context,
                "async initiation does not contain exactly one call operation",
            ));
        }
        match &operation.kind {
            BytecodeOperationKind::CheckedPrefix { operator, operand } => {
                self.verify_operand(function, operand, context)?;
                self.verify_prefix(*operator, operand.ty, operation.ty, context)?;
                if !self.prefix_requires_checked(*operator, operand.ty) {
                    return Err(operation_error(context));
                }
            }
            BytecodeOperationKind::CheckedBinary {
                operator,
                left,
                right,
            } => {
                self.verify_operand(function, left, context)?;
                self.verify_operand(function, right, context)?;
                self.verify_binary(*operator, left.ty, right.ty, operation.ty, context)?;
                if !self.binary_requires_checked(*operator, left.ty, right.ty) {
                    return Err(operation_error(context));
                }
            }
            BytecodeOperationKind::ArraySequence {
                kind,
                array,
                argument,
            } => {
                self.verify_operand(function, array, context)?;
                self.verify_operand(function, argument, context)?;
                let element = self.intrinsic_argument(
                    operation.ty,
                    BytecodeIntrinsicType::Array,
                    0,
                    context,
                )?;
                let expected = match kind {
                    BytecodeArraySequenceKind::Concat => operation.ty,
                    BytecodeArraySequenceKind::Repeat => self.find_type(
                        |ty| matches!(ty, BytecodeTypeKind::Scalar(BytecodeScalarType::Int)),
                        context,
                    )?,
                };
                if array.ty != operation.ty
                    || !matches!(array.kind, BytecodeOperandKind::Borrow(_))
                    || argument.ty != expected
                    || !self.capability(element, ClosedCapability::Copy, context)?
                {
                    return Err(operation_error(context));
                }
            }
            BytecodeOperationKind::BuildMap { entries, .. } => {
                let key =
                    self.intrinsic_argument(operation.ty, BytecodeIntrinsicType::Map, 0, context)?;
                let value =
                    self.intrinsic_argument(operation.ty, BytecodeIntrinsicType::Map, 1, context)?;
                for (entry_key, entry_value) in entries {
                    self.verify_operand(function, entry_key, context)?;
                    self.verify_operand(function, entry_value, context)?;
                    if entry_key.ty != key || entry_value.ty != value {
                        return Err(operation_error(context));
                    }
                }
            }
            BytecodeOperationKind::Index {
                base,
                index,
                access,
                against,
            } => {
                self.verify_operand(function, base, context)?;
                self.verify_operand(function, index, context)?;
                if self.index_result(base.ty, index.ty, *access, context)? != operation.ty {
                    return Err(operation_error(context));
                }
                if *access == BytecodeIndexAccess::String && !against.is_empty() {
                    return Err(operation_error(context));
                }
                self.verify_runtime_conflict_ids(function, against, context)?;
                let _ = operation_access_place(operation, context)?;
            }
            BytecodeOperationKind::Slice {
                base,
                bounds,
                against,
            } => {
                self.verify_operand(function, base, context)?;
                for bound in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                    self.verify_operand(function, bound, context)?;
                    if !self.is_scalar(bound.ty, BytecodeScalarType::Int) {
                        return Err(operation_error(context));
                    }
                }
                let is_array = self
                    .intrinsic_argument(base.ty, BytecodeIntrinsicType::Array, 0, context)
                    .is_ok();
                let is_string = self.is_scalar(base.ty, BytecodeScalarType::String);
                if operation.ty != base.ty || !(is_array || is_string) {
                    return Err(operation_error(context));
                }
                if is_string && !against.is_empty() {
                    return Err(operation_error(context));
                }
                if is_array && !self.capability(operation.ty, ClosedCapability::Copy, context)? {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "slice operation materializes a non-Copy Array",
                    ));
                }
                self.verify_runtime_conflict_ids(function, against, context)?;
                let _ = operation_access_place(operation, context)?;
            }
            BytecodeOperationKind::Call {
                callee,
                arguments,
                signature,
                protocol,
            } => {
                self.verify_operand(function, callee, context)?;
                for argument in arguments {
                    self.verify_operand(function, &argument.value, context)?;
                }
                self.verify_call(
                    function,
                    CallVerification {
                        callee,
                        arguments,
                        signature: *signature,
                        protocol: *protocol,
                        outcome: operation.ty,
                    },
                    operation_context,
                    context,
                )?;
            }
            BytecodeOperationKind::Display { argument } => {
                self.verify_operand(function, &argument.value, context)?;
                let BytecodeOperandKind::Loan(loan) = argument.value.kind else {
                    return Err(operation_error(context));
                };
                let loan = self.loan(function, loan, context)?;
                if operation_context != OperationContext::Immediate
                    || argument.mode != BytecodeParameterMode::Ref
                    || argument.target != BytecodeCallArgumentTarget::Receiver
                    || loan.mode != BytecodeParameterMode::Ref
                    || !self.is_intrinsic_display_type(argument.value.ty)
                    || !self.is_scalar(operation.ty, BytecodeScalarType::String)
                {
                    return Err(operation_error(context));
                }
            }
            BytecodeOperationKind::ExplicitPanic { message } => {
                self.verify_operand(function, message, context)?;
                if !self.is_scalar(message.ty, BytecodeScalarType::String)
                    || !self.is_scalar(operation.ty, BytecodeScalarType::Never)
                {
                    return Err(operation_error(context));
                }
            }
            BytecodeOperationKind::Assert {
                condition,
                condition_repr,
                message_parts,
            } => {
                self.verify_operand(function, condition, context)?;
                if !self.is_scalar(condition.ty, BytecodeScalarType::Bool)
                    || !self.is_scalar(operation.ty, BytecodeScalarType::Unit)
                    || condition_repr.is_empty()
                {
                    return Err(operation_error(context));
                }
                for part in message_parts {
                    self.verify_operand(function, &part.value, context)?;
                    if part.spread {
                        let element = self.intrinsic_argument(
                            part.value.ty,
                            BytecodeIntrinsicType::Array,
                            0,
                            context,
                        )?;
                        if !self.is_scalar(element, BytecodeScalarType::String) {
                            return Err(operation_error(context));
                        }
                    } else if !self.is_scalar(part.value.ty, BytecodeScalarType::String) {
                        return Err(operation_error(context));
                    }
                }
            }
            BytecodeOperationKind::BootstrapHostCall {
                function: host_function,
                arguments,
            } => {
                for argument in arguments {
                    self.verify_operand(function, argument, context)?;
                }
                if !matches!(host_function, BytecodeBootstrapHostFunction::ConsolePrint)
                    || arguments.len() != 1
                    || !self.is_scalar(arguments[0].ty, BytecodeScalarType::String)
                    || !self.is_scalar(operation.ty, BytecodeScalarType::Unit)
                {
                    return Err(operation_error(context));
                }
            }
        }
        Ok(())
    }

    fn verify_call(
        &self,
        bytecode_function: &BytecodeFunction,
        call: CallVerification<'_>,
        operation_context: OperationContext,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let CallVerification {
            callee,
            arguments,
            signature,
            protocol,
            outcome,
        } = call;
        let BytecodeTypeKind::Function(function_type) = &self.ty(signature, context)?.kind else {
            return Err(operation_error(context));
        };
        if function_type.is_async != operation_context.expects_async() || function_type.is_unsafe {
            return Err(BytecodeVerificationError::new(
                context,
                "call effects differ from their bytecode initiation context",
            ));
        }
        if function_type.outcome != outcome {
            return Err(operation_error(context));
        }
        match &self.ty(callee.ty, context)?.kind {
            BytecodeTypeKind::Function(_) => {
                if callee.ty != signature || protocol != BytecodeCallProtocol::Call {
                    return Err(operation_error(context));
                }
            }
            BytecodeTypeKind::Generated { .. } | BytecodeTypeKind::OpaqueResult { .. } => {
                let (concrete_signature, callable) =
                    self.concrete_callable_for_type(callee.ty, context)?;
                let expected = match callable.and_then(|callable| callable.closure.as_ref()) {
                    None => Some(BytecodeCallProtocol::Call),
                    Some(closure)
                        if operation_context == OperationContext::Spawn
                            && closure.protocols.call_once
                            && !matches!(callee.kind, BytecodeOperandKind::Borrow(_)) =>
                    {
                        Some(BytecodeCallProtocol::CallOnce)
                    }
                    Some(closure)
                        if operation_context == OperationContext::Deferred
                            && !self.capability(callee.ty, ClosedCapability::Copy, context)?
                            && closure.protocols.call_once
                            && !matches!(callee.kind, BytecodeOperandKind::Borrow(_)) =>
                    {
                        Some(BytecodeCallProtocol::CallOnce)
                    }
                    Some(closure) if closure.protocols.call => Some(BytecodeCallProtocol::Call),
                    Some(closure)
                        if closure.protocols.call_mut
                            && matches!(callee.kind, BytecodeOperandKind::Borrow(_)) =>
                    {
                        Some(BytecodeCallProtocol::CallMut)
                    }
                    Some(closure)
                        if closure.protocols.call_once
                            && !matches!(callee.kind, BytecodeOperandKind::Borrow(_)) =>
                    {
                        Some(BytecodeCallProtocol::CallOnce)
                    }
                    Some(_) => None,
                };
                if concrete_signature != signature || expected != Some(protocol) {
                    return Err(operation_error(context));
                }
            }
            _ => return Err(operation_error(context)),
        }
        if protocol == BytecodeCallProtocol::CallMut
            && !matches!(callee.kind, BytecodeOperandKind::Borrow(_))
            || protocol == BytecodeCallProtocol::CallOnce
                && matches!(callee.kind, BytecodeOperandKind::Borrow(_))
        {
            return Err(operation_error(context));
        }
        let callable = match callee.kind {
            BytecodeOperandKind::Function { callable, .. } => {
                let callable = self.callable(callable, context)?;
                if callable.closure.is_some() {
                    return Err(operation_error(context));
                }
                Some(callable)
            }
            _ => None,
        };
        let mut fixed = Vec::new();
        let mut receiver = None;
        if let Some(callable) = callable {
            let mut concrete = function_type.parameters.iter();
            for (source_index, parameter) in callable.parameters.iter().enumerate() {
                if parameter.variadic_element.is_some() {
                    continue;
                }
                let concrete = concrete.next().ok_or_else(|| operation_error(context))?;
                let association = if parameter.receiver {
                    BytecodeCallArgumentTarget::Receiver
                } else {
                    BytecodeCallArgumentTarget::Fixed(source_index as u32)
                };
                let item = (association, concrete.mode, concrete.ty);
                if parameter.receiver {
                    if receiver.replace(item).is_some() {
                        return Err(operation_error(context));
                    }
                } else {
                    fixed.push(item);
                }
            }
            if concrete.next().is_some() {
                return Err(operation_error(context));
            }
        } else {
            fixed.extend(
                function_type
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| {
                        (
                            BytecodeCallArgumentTarget::Fixed(index as u32),
                            parameter.mode,
                            parameter.ty,
                        )
                    }),
            );
        }
        let mut provided = Vec::new();
        let mut spread = false;
        for (position, argument) in arguments.iter().enumerate() {
            let expected = match argument.target {
                BytecodeCallArgumentTarget::Receiver => receiver,
                BytecodeCallArgumentTarget::Fixed(index) => fixed
                    .iter()
                    .find(|(target, _, _)| *target == BytecodeCallArgumentTarget::Fixed(index))
                    .copied(),
                BytecodeCallArgumentTarget::VariadicElement => function_type
                    .variadic
                    .map(|ty| (argument.target, BytecodeParameterMode::Value, ty)),
                BytecodeCallArgumentTarget::VariadicSpread => {
                    if spread || position + 1 != arguments.len() {
                        return Err(operation_error(context));
                    }
                    spread = true;
                    let element = function_type
                        .variadic
                        .ok_or_else(|| operation_error(context))?;
                    let valid = matches!(
                        &self.ty(argument.value.ty, context)?.kind,
                        BytecodeTypeKind::Intrinsic {
                            constructor: BytecodeIntrinsicType::Array,
                            arguments,
                        } if arguments == &[element]
                    );
                    if !valid || argument.mode != BytecodeParameterMode::Value {
                        return Err(operation_error(context));
                    }
                    continue;
                }
            }
            .ok_or_else(|| operation_error(context))?;
            if matches!(
                argument.target,
                BytecodeCallArgumentTarget::Receiver | BytecodeCallArgumentTarget::Fixed(_)
            ) && provided.contains(&argument.target)
            {
                return Err(operation_error(context));
            }
            if matches!(
                argument.target,
                BytecodeCallArgumentTarget::Receiver | BytecodeCallArgumentTarget::Fixed(_)
            ) {
                provided.push(argument.target);
            }
            if argument.mode != expected.1 || argument.value.ty != expected.2 {
                return Err(operation_error(context));
            }
            let loans = matches!(argument.value.kind, BytecodeOperandKind::Loan(_));
            if (argument.mode == BytecodeParameterMode::Value) == loans {
                return Err(operation_error(context));
            }
            if let BytecodeOperandKind::Loan(loan) = argument.value.kind {
                let loan = self.loan(bytecode_function, loan, context)?;
                if loan.mode != argument.mode || loan.place.ty != argument.value.ty {
                    return Err(operation_error(context));
                }
            }
        }
        if provided.len() != fixed.len() + usize::from(receiver.is_some()) {
            return Err(operation_error(context));
        }
        Ok(())
    }

    fn is_intrinsic_display_type(&self, ty: BytecodeTypeId) -> bool {
        let mut ty = ty;
        let mut remaining = self.program.types.len();
        loop {
            if remaining == 0 {
                return false;
            }
            remaining -= 1;
            match self.program.ty(ty).map(|ty| &ty.kind) {
                Some(BytecodeTypeKind::Scalar(scalar)) => {
                    return *scalar != BytecodeScalarType::Never;
                }
                Some(BytecodeTypeKind::OpaqueResult { witness, .. }) => ty = *witness,
                _ => return false,
            }
        }
    }

    fn closure_callable_for_type(
        &self,
        mut ty: BytecodeTypeId,
        context: &str,
    ) -> Result<Option<&BytecodeCallable>, BytecodeVerificationError> {
        loop {
            match &self.ty(ty, context)?.kind {
                BytecodeTypeKind::OpaqueResult { witness, .. } => ty = *witness,
                BytecodeTypeKind::Generated { .. } => {
                    return Ok(self.program.callables.iter().find(|callable| {
                        callable
                            .closure
                            .as_ref()
                            .is_some_and(|closure| closure.environment == ty)
                    }));
                }
                _ => return Ok(None),
            }
        }
    }

    fn concrete_callable_for_type(
        &self,
        mut ty: BytecodeTypeId,
        context: &str,
    ) -> Result<(BytecodeTypeId, Option<&BytecodeCallable>), BytecodeVerificationError> {
        loop {
            match &self.ty(ty, context)?.kind {
                BytecodeTypeKind::OpaqueResult { witness, .. } => ty = *witness,
                BytecodeTypeKind::Function(_) => return Ok((ty, None)),
                BytecodeTypeKind::Generated { .. } => {
                    let callable = self
                        .program
                        .callables
                        .iter()
                        .find(|callable| {
                            callable
                                .closure
                                .as_ref()
                                .is_some_and(|closure| closure.environment == ty)
                        })
                        .ok_or_else(|| operation_error(context))?;
                    return Ok((callable.function_type, Some(callable)));
                }
                _ => return Err(operation_error(context)),
            }
        }
    }

    fn verify_terminator(
        &self,
        function: &BytecodeFunction,
        block: &BytecodeBlock,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        match &block.terminator.kind {
            BytecodeTerminatorKind::Goto { target } => {
                self.edge_target(function, block.kind, *target, context)?;
            }
            BytecodeTerminatorKind::BranchBool {
                condition,
                if_true,
                if_false,
            } => {
                if block.kind != BytecodeBlockKind::Normal
                    || operand_is_borrow(condition)
                    || operand_is_loan(condition)
                {
                    return Err(terminator_error(context));
                }
                self.verify_operand(function, condition, context)?;
                if !self.is_scalar(condition.ty, BytecodeScalarType::Bool) {
                    return Err(terminator_error(context));
                }
                self.normal_target(function, *if_true, context)?;
                self.normal_target(function, *if_false, context)?;
            }
            BytecodeTerminatorKind::BranchTag {
                value,
                cases,
                otherwise,
            } => {
                if block.kind != BytecodeBlockKind::Normal
                    || cases.is_empty()
                    || operand_is_loan(value)
                {
                    return Err(terminator_error(context));
                }
                self.verify_operand(function, value, context)?;
                let mut tags = BTreeSet::new();
                for (tag, target) in cases {
                    if !tags.insert(*tag) || !self.tag_matches(value.ty, *tag, context)? {
                        return Err(terminator_error(context));
                    }
                    self.normal_target(function, *target, context)?;
                }
                self.normal_target(function, *otherwise, context)?;
            }
            BytecodeTerminatorKind::Invoke {
                operation,
                destination,
                target,
                unwind,
            } => {
                if block.kind != BytecodeBlockKind::Normal {
                    return Err(terminator_error(context));
                }
                self.verify_operation(function, operation, OperationContext::Immediate, context)?;
                match (destination, target) {
                    (Some(destination), Some(target)) => {
                        self.verify_place(function, destination, context)?;
                        if place_contains_ref_value(destination) {
                            return Err(BytecodeVerificationError::new(
                                context,
                                "`Ref[T].value` is a read-only projection",
                            ));
                        }
                        if destination.ty != operation.ty
                            || self.is_scalar(operation.ty, BytecodeScalarType::Never)
                        {
                            return Err(terminator_error(context));
                        }
                        self.normal_target(function, *target, context)?;
                    }
                    (None, None) if self.is_scalar(operation.ty, BytecodeScalarType::Never) => {}
                    _ => return Err(terminator_error(context)),
                }
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::Await {
                awaitable,
                destination,
                target,
                unwind,
            } => {
                if block.kind != BytecodeBlockKind::Normal
                    || !self.function_is_async(function, context)?
                {
                    return Err(terminator_error(context));
                }
                self.verify_place(function, destination, context)?;
                if place_contains_ref_value(destination) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "`Ref[T].value` is a read-only projection",
                    ));
                }
                let expected = match awaitable {
                    BytecodeAwaitable::Call(operation) => {
                        self.verify_operation(
                            function,
                            operation,
                            OperationContext::Await,
                            context,
                        )?;
                        operation.ty
                    }
                    BytecodeAwaitable::Join(join) => {
                        self.verify_operand(function, join, context)?;
                        if !matches!(join.kind, BytecodeOperandKind::Move(_)) {
                            return Err(BytecodeVerificationError::new(
                                context,
                                "await must consume its affine Join operand",
                            ));
                        }
                        self.join_logical_outcome(join.ty, context)?
                    }
                };
                if destination.ty != expected {
                    return Err(terminator_error(context));
                }
                self.normal_target(function, *target, context)?;
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::Spawn {
                operation,
                destination,
                target,
                unwind,
                ..
            } => {
                if block.kind != BytecodeBlockKind::Normal
                    || !self.function_is_async(function, context)?
                {
                    return Err(terminator_error(context));
                }
                self.verify_operation(function, operation, OperationContext::Spawn, context)?;
                self.verify_spawn_transfer(operation, context)?;
                self.verify_place(function, destination, context)?;
                if place_contains_ref_value(destination)
                    || !self.is_join_for_outcome(destination.ty, operation.ty, context)?
                {
                    return Err(terminator_error(context));
                }
                self.normal_target(function, *target, context)?;
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::IteratorNext {
                state,
                destination,
                borrowed_source,
                exhaustion_guard,
                has_value,
                exhausted,
                unwind,
            } => {
                if block.kind != BytecodeBlockKind::Normal {
                    return Err(terminator_error(context));
                }
                self.verify_place(function, state, context)?;
                self.verify_place(function, destination, context)?;
                if place_contains_ref_value(destination) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "`Ref[T].value` is a read-only projection",
                    ));
                }
                let BytecodeTypeKind::Cursor { mode, collection } =
                    self.ty(state.ty, context)?.kind
                else {
                    return Err(terminator_error(context));
                };
                match mode {
                    BytecodeCursorMode::Own => {
                        if borrowed_source.is_some()
                            || self.iterated_item_type(state.ty, context)? != Some(destination.ty)
                        {
                            return Err(terminator_error(context));
                        }
                        let terminal = self.terminal_status(collection, context)?
                            == BytecodeTerminalStatus::Present;
                        if terminal != exhaustion_guard.is_some() {
                            return Err(terminator_error(context));
                        }
                        if let Some(guard) = exhaustion_guard {
                            if has_value == exhausted {
                                return Err(terminator_error(context));
                            }
                            self.verify_place(function, guard, context)?;
                            let mut expected = state.clone();
                            expected.ty = collection;
                            expected.projections.push(BytecodeProjection {
                                ty: collection,
                                kind: BytecodeProjectionKind::IteratorSource,
                            });
                            if guard != &expected {
                                return Err(terminator_error(context));
                            }
                        }
                    }
                    BytecodeCursorMode::Ref | BytecodeCursorMode::Mut => {
                        if exhaustion_guard.is_some() {
                            return Err(terminator_error(context));
                        }
                        let source = borrowed_source
                            .as_ref()
                            .ok_or_else(|| terminator_error(context))?;
                        self.verify_place(function, source, context)?;
                        if mode == BytecodeCursorMode::Mut
                            && !matches!(
                                self.ty(collection, context)?.kind,
                                BytecodeTypeKind::Intrinsic {
                                    constructor: BytecodeIntrinsicType::Array
                                        | BytecodeIntrinsicType::Map,
                                    ..
                                }
                            )
                        {
                            return Err(terminator_error(context));
                        }
                        if source.ty != collection
                            || !self.is_scalar(destination.ty, BytecodeScalarType::Int)
                            || source.source_loan.is_none()
                            || self
                                .borrowed_collection_item_type(collection, context)?
                                .is_none()
                        {
                            return Err(terminator_error(context));
                        }
                        self.verify_borrowed_iterator_origin(
                            function,
                            state,
                            destination,
                            source,
                            mode,
                            context,
                        )?;
                    }
                }
                self.normal_target(function, *has_value, context)?;
                self.normal_target(function, *exhausted, context)?;
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
                for_write,
                target,
                unwind,
            } => {
                if block.kind != BytecodeBlockKind::Normal
                    || places.is_empty()
                    || places.len() != replacements.len()
                    || places.len() != against.len()
                {
                    return Err(terminator_error(context));
                }
                let mut unique = Vec::new();
                for ((place, replacement), against) in places.iter().zip(replacements).zip(against)
                {
                    self.verify_place(function, place, context)?;
                    if *for_write && place_contains_ref_value(place) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            "`Ref[T].value` is a read-only projection",
                        ));
                    }
                    if unique.contains(place) {
                        return Err(terminator_error(context));
                    }
                    unique.push(place.clone());
                    let mut previous = None;
                    for loan in against {
                        self.loan(function, *loan, context)?;
                        if previous.is_some_and(|previous| previous >= *loan) {
                            return Err(terminator_error(context));
                        }
                        previous = Some(*loan);
                    }
                    match (*for_write, replacement) {
                        (false, None) => {}
                        (true, Some(replacement)) => {
                            self.verify_operand(function, replacement, context)?;
                            if replacement.ty != place.ty
                                || !matches!(replacement.kind, BytecodeOperandKind::Borrow(_))
                            {
                                return Err(terminator_error(context));
                            }
                        }
                        _ => return Err(terminator_error(context)),
                    }
                }
                self.normal_target(function, *target, context)?;
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::ValidateLoan {
                loan,
                against,
                target,
                unwind,
            } => {
                if block.kind != BytecodeBlockKind::Normal {
                    return Err(terminator_error(context));
                }
                let metadata = self.loan(function, *loan, context)?;
                if !place_requires_loan_validation(&metadata.place) {
                    return Err(terminator_error(context));
                }
                let mut previous = None;
                for candidate in against {
                    self.loan(function, *candidate, context)?;
                    if candidate == loan || previous.is_some_and(|previous| previous >= *candidate)
                    {
                        return Err(terminator_error(context));
                    }
                    previous = Some(*candidate);
                }
                let target_block = self.block(function, *target, context)?;
                if target_block.kind != BytecodeBlockKind::Normal
                    || !matches!(
                        target_block.instructions.first().map(|instruction| &instruction.kind),
                        Some(BytecodeInstructionKind::ReserveLoan(candidate)) if candidate == loan
                    )
                {
                    return Err(terminator_error(context));
                }
                self.cleanup_target(function, *unwind, context)?;
            }
            BytecodeTerminatorKind::DrainDefers {
                scopes,
                target,
                unwind,
            } => {
                if scopes.is_empty()
                    || scopes.iter().copied().collect::<BTreeSet<_>>().len() != scopes.len()
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer drain has an empty or duplicate scope set",
                    ));
                }
                self.edge_target(function, block.kind, *target, context)?;
                self.cleanup_target(function, *unwind, context)?;
                if !block.instructions.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "defer drain block contains ordinary instructions",
                    ));
                }
            }
            BytecodeTerminatorKind::DrainScopes {
                task_scopes,
                defer_scopes,
                target,
                unwind,
            } => {
                if task_scopes.is_empty() && defer_scopes.is_empty()
                    || task_scopes.iter().copied().collect::<BTreeSet<_>>().len()
                        != task_scopes.len()
                    || defer_scopes.iter().copied().collect::<BTreeSet<_>>().len()
                        != defer_scopes.len()
                    || !task_scopes.is_empty() && !self.function_is_async(function, context)?
                {
                    return Err(terminator_error(context));
                }
                self.edge_target(function, block.kind, *target, context)?;
                self.cleanup_target(function, *unwind, context)?;
                if !block.instructions.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "structured drain block contains ordinary instructions",
                    ));
                }
            }
            BytecodeTerminatorKind::DrainUnwind { target } => {
                if block.kind != BytecodeBlockKind::Cleanup
                    || *target != function.unwind
                    || !block.instructions.is_empty()
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "unwind drain is not an empty cleanup block targeting the function unwind",
                    ));
                }
                self.cleanup_target(function, *target, context)?;
            }
            BytecodeTerminatorKind::Return => {
                if block.kind != BytecodeBlockKind::Normal
                    || self.is_scalar(
                        self.slot(function, function.return_slot, context)?.ty,
                        BytecodeScalarType::Never,
                    )
                {
                    return Err(terminator_error(context));
                }
            }
            BytecodeTerminatorKind::ResumePanic => {
                if block.kind != BytecodeBlockKind::Cleanup {
                    return Err(terminator_error(context));
                }
            }
            BytecodeTerminatorKind::Unreachable => {}
        }
        Ok(())
    }

    fn numeric_conversion_error_variant(
        &self,
        ty: BytecodeTypeId,
        variant: u32,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        Ok(matches!(
            &self.ty(ty, context)?.kind,
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::NumericConversionError,
                arguments,
            } if arguments.is_empty()
                && BytecodeNumericConversionError::from_index(variant).is_some()
        ))
    }

    fn tag_matches(
        &self,
        ty: BytecodeTypeId,
        tag: BytecodeTag,
        context: &str,
    ) -> Result<bool, BytecodeVerificationError> {
        Ok(match tag {
            BytecodeTag::OptionNone | BytecodeTag::OptionSome => {
                matches!(self.ty(ty, context)?.kind, BytecodeTypeKind::Option(_))
            }
            BytecodeTag::ResultOk | BytecodeTag::ResultErr => {
                matches!(self.ty(ty, context)?.kind, BytecodeTypeKind::Result { .. })
            }
            BytecodeTag::Variant(member) => {
                if self.numeric_conversion_error_variant(ty, member, context)? {
                    true
                } else {
                    let (_, _, metadata) = self.nominal_instance(ty, context)?;
                    matches!(&metadata.shape, BytecodeNominalShape::Enum { variants } if variants.iter().any(|variant| variant.member == member))
                }
            }
            BytecodeTag::Union(member) => {
                self.ty(member, context)?;
                matches!(&self.ty(ty, context)?.kind, BytecodeTypeKind::Union(members) if members.contains(&member))
            }
        })
    }

    fn edge_target(
        &self,
        function: &BytecodeFunction,
        source: BytecodeBlockKind,
        target: BytecodeBlockId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let target_kind = self.block(function, target, context)?.kind;
        if source != target_kind {
            return Err(terminator_error(context));
        }
        Ok(())
    }

    fn normal_target(
        &self,
        function: &BytecodeFunction,
        target: BytecodeBlockId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if self.block(function, target, context)?.kind != BytecodeBlockKind::Normal {
            return Err(terminator_error(context));
        }
        Ok(())
    }

    fn cleanup_target(
        &self,
        function: &BytecodeFunction,
        target: BytecodeBlockId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if self.block(function, target, context)?.kind != BytecodeBlockKind::Cleanup {
            return Err(terminator_error(context));
        }
        Ok(())
    }

    fn verify_control_and_dataflow(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let events = function
            .blocks
            .iter()
            .map(|block| local_events(function, block))
            .collect::<Vec<_>>();
        let successors = function
            .blocks
            .iter()
            .map(|block| successor_edges(&block.terminator.kind))
            .collect::<Vec<_>>();
        let mut predecessors =
            vec![Vec::<(BytecodeBlockId, SuccessorEdge)>::new(); function.blocks.len()];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target.index() as usize]
                    .push((BytecodeBlockId::new(source as u32), edge.clone()));
            }
        }
        if !predecessors[function.entry.index() as usize].is_empty() {
            return Err(BytecodeVerificationError::new(
                context,
                "entry block has an incoming edge",
            ));
        }
        let mut reachable = vec![false; function.blocks.len()];
        let mut queue = VecDeque::from([function.entry]);
        reachable[function.entry.index() as usize] = true;
        while let Some(block) = queue.pop_front() {
            for edge in &successors[block.index() as usize] {
                let index = edge.target.index() as usize;
                if !reachable[index] {
                    reachable[index] = true;
                    queue.push_back(edge.target);
                }
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            if reachable[index] || BytecodeBlockId::new(index as u32) == function.unwind {
                continue;
            }
            if !block.instructions.is_empty()
                || !matches!(block.terminator.kind, BytecodeTerminatorKind::Unreachable)
            {
                return Err(BytecodeVerificationError::new(
                    context,
                    format!("unreachable block#{index} contains executable bytecode"),
                ));
            }
        }
        let managed = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => Some(*slot),
                LocalEvent::Read(_)
                | LocalEvent::Resolve(_)
                | LocalEvent::Move(_)
                | LocalEvent::Write(_)
                | LocalEvent::WriteAccess(_) => None,
            })
            .collect::<BTreeSet<_>>();
        let mut relevant = events
            .iter()
            .flatten()
            .map(|event| match event {
                LocalEvent::Read(access)
                | LocalEvent::Resolve(access)
                | LocalEvent::Move(access)
                | LocalEvent::Write(access)
                | LocalEvent::WriteAccess(access) => access.slot,
                LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => *slot,
            })
            .collect::<BTreeSet<_>>();
        relevant.insert(function.return_slot);
        for edges in &successors {
            relevant.extend(
                edges
                    .iter()
                    .filter_map(|edge| edge.writes.as_ref().map(|place| place.slot)),
            );
        }
        for slot in relevant {
            self.verify_slot_flow(
                function,
                slot,
                &events,
                &successors,
                &predecessors,
                &reachable,
                managed.contains(&slot),
                context,
            )?;
        }
        self.verify_loan_flow(function, &reachable, context)?;
        self.verify_tag_refinements(function, &successors, &reachable, context)?;
        Ok(())
    }

    fn verify_defer_flow(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        self.verify_fallback_coverage(function, context)?;
        let entry = self.block(function, function.entry, context)?;
        let registered_entry_owners = entry
            .instructions
            .iter()
            .take_while(|instruction| {
                matches!(
                    instruction.kind,
                    BytecodeInstructionKind::RegisterFallback { .. }
                )
            })
            .filter_map(|instruction| match &instruction.kind {
                BytecodeInstructionKind::RegisterFallback { owner, .. } => {
                    Some(LocalAccess::from_place(owner))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for owner in self.terminal_entry_owners(function, context)? {
            if !registered_entry_owners.contains(&LocalAccess::from_place(&owner)) {
                return Err(BytecodeVerificationError::new(
                    context,
                    format!(
                        "terminal entry owner rooted at slot#{} has no entry fallback registration",
                        owner.slot.index()
                    ),
                ));
            }
        }
        let registered_scopes = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.kind {
                BytecodeInstructionKind::RegisterDefer { scope, .. } => Some(*scope),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for (index, block) in function.blocks.iter().enumerate() {
            let scopes = match &block.terminator.kind {
                BytecodeTerminatorKind::DrainDefers { scopes, .. } => scopes,
                BytecodeTerminatorKind::DrainScopes { defer_scopes, .. } => defer_scopes,
                _ => continue,
            };
            if scopes
                .iter()
                .any(|scope| !registered_scopes.contains(scope))
            {
                return Err(BytecodeVerificationError::new(
                    format!("{context} block#{index}"),
                    "defer drain references a scope with no registration",
                ));
            }
        }
        let successors = function
            .blocks
            .iter()
            .map(|block| successor_edges(&block.terminator.kind))
            .collect::<Vec<_>>();
        let mut incoming = vec![BTreeSet::<DeferFlowState>::new(); function.blocks.len()];
        incoming[function.entry.index() as usize].insert(DeferFlowState::default());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;

        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            let block = &function.blocks[block_id.index() as usize];
            let block_context = format!("{context} block#{}", block_id.index());
            let block_events = local_events(function, block);
            let mut outgoing = BTreeSet::new();
            for mut state in incoming[block_id.index() as usize].clone() {
                self.consume_dataflow_step(context)?;
                apply_consumed_defer_events(&mut state, &block_events, &block_context)?;
                for (index, instruction) in block.instructions.iter().enumerate() {
                    let instruction_context = format!("{block_context} instruction#{index}");
                    let advances_pending = match &instruction.kind {
                        BytecodeInstructionKind::RetargetCleanup { from, .. } => state
                            .pending_moves
                            .contains_key(&LocalAccess::from_place(from)),
                        BytecodeInstructionKind::DisarmCleanup(place) => state
                            .pending_moves
                            .contains_key(&LocalAccess::from_place(place)),
                        _ => false,
                    };
                    if !state.pending_moves.is_empty() && !advances_pending {
                        return Err(BytecodeVerificationError::new(
                            instruction_context,
                            "guarded move is not immediately followed by its defer transition",
                        ));
                    }
                    match &instruction.kind {
                        BytecodeInstructionKind::RegisterDefer { scope, guard, .. } => {
                            state.activate_scope(*scope, &instruction_context)?;
                            let registration = (block_id, index);
                            if state
                                .registrations
                                .insert(
                                    registration,
                                    ActiveCleanupRegistration {
                                        scope: *scope,
                                        kind: CleanupEntryKind::Explicit,
                                    },
                                )
                                .is_some()
                            {
                                return Err(BytecodeVerificationError::new(
                                    instruction_context,
                                    "defer registration is re-executed before it is drained or disarmed",
                                ));
                            }
                            if let Some(guard) = guard {
                                let terminal = self
                                    .terminal_status(guard.ty, &instruction_context)?
                                    == BytecodeTerminalStatus::Present;
                                let guard = LocalAccess::from_place(guard);
                                let replaced = state
                                    .guards
                                    .iter()
                                    .filter_map(|(existing, active)| {
                                        (active.kind == CleanupEntryKind::Fallback
                                            && local_access_contains(&guard, existing))
                                        .then_some((existing.clone(), *active))
                                    })
                                    .collect::<Vec<_>>();
                                if terminal && replaced.len() != 1 {
                                    return Err(BytecodeVerificationError::new(
                                        instruction_context,
                                        "terminal explicit cleanup guard does not replace exactly one fallback",
                                    ));
                                }
                                for (existing, active) in replaced {
                                    state.guards.remove(&existing);
                                    state.registrations.remove(&active.registration);
                                }
                                if state
                                    .guards
                                    .keys()
                                    .any(|existing| local_accesses_overlap(existing, &guard))
                                {
                                    return Err(BytecodeVerificationError::new(
                                        instruction_context,
                                        "defer registration overlaps an already-active guard",
                                    ));
                                }
                                state.guards.insert(
                                    guard,
                                    ActiveDeferGuard {
                                        scope: *scope,
                                        registration,
                                        kind: CleanupEntryKind::Explicit,
                                    },
                                );
                            } else {
                                state.unguarded_scopes.insert(*scope);
                            }
                        }
                        BytecodeInstructionKind::RegisterFallback { scope, owner } => {
                            let owner = LocalAccess::from_place(owner);
                            if state.guards.iter().any(|(existing, active)| {
                                active.kind == CleanupEntryKind::Explicit
                                    && local_accesses_overlap(existing, &owner)
                            }) {
                                return Err(BytecodeVerificationError::new(
                                    instruction_context,
                                    "terminal fallback overlaps an active explicit cleanup guard",
                                ));
                            }
                            if state.guards.iter().any(|(existing, active)| {
                                active.kind == CleanupEntryKind::Fallback
                                    && local_access_contains(existing, &owner)
                            }) {
                                continue;
                            }
                            let folded = state
                                .guards
                                .iter()
                                .filter_map(|(existing, active)| {
                                    (active.kind == CleanupEntryKind::Fallback
                                        && local_access_contains(&owner, existing))
                                    .then_some((existing.clone(), *active))
                                })
                                .collect::<Vec<_>>();
                            for (existing, active) in folded {
                                state.guards.remove(&existing);
                                state.registrations.remove(&active.registration);
                            }
                            let registration = (block_id, index);
                            if state
                                .registrations
                                .insert(
                                    registration,
                                    ActiveCleanupRegistration {
                                        scope: *scope,
                                        kind: CleanupEntryKind::Fallback,
                                    },
                                )
                                .is_some()
                            {
                                return Err(BytecodeVerificationError::new(
                                    instruction_context,
                                    "terminal fallback registration is re-executed while active",
                                ));
                            }
                            state.guards.insert(
                                owner,
                                ActiveDeferGuard {
                                    scope: *scope,
                                    registration,
                                    kind: CleanupEntryKind::Fallback,
                                },
                            );
                        }
                        BytecodeInstructionKind::RetargetCleanup { from, to } => {
                            let from = LocalAccess::from_place(from);
                            let to = LocalAccess::from_place(to);
                            if let Some(guard) = state.guards.remove(&from) {
                                if state.pending_moves.remove(&from)
                                    != Some(PendingDeferTransition::Retarget)
                                {
                                    return Err(BytecodeVerificationError::new(
                                        instruction_context,
                                        "defer guard retarget is not backed by an immediate move",
                                    ));
                                }
                                if state
                                    .guards
                                    .keys()
                                    .any(|existing| local_accesses_overlap(existing, &to))
                                {
                                    return Err(BytecodeVerificationError::new(
                                        instruction_context,
                                        "defer guard retarget overlaps another active guard",
                                    ));
                                }
                                state.guards.insert(to, guard);
                            }
                        }
                        BytecodeInstructionKind::DisarmCleanup(place) => {
                            let place = LocalAccess::from_place(place);
                            let pending = state.pending_moves.remove(&place);
                            if let Some(guard) = state.guards.remove(&place) {
                                let confirmed = match guard.kind {
                                    CleanupEntryKind::Explicit => defer_disarm_is_confirmed(
                                        function,
                                        block,
                                        index,
                                        &place,
                                        guard.scope,
                                        pending,
                                    ),
                                    CleanupEntryKind::Fallback => {
                                        pending == Some(PendingDeferTransition::Disarm)
                                            || (place.slot == function.return_slot
                                                && place.path.is_empty()
                                                && matches!(
                                                    block.terminator.kind,
                                                    BytecodeTerminatorKind::Return
                                                ))
                                            || (pending.is_none()
                                                && defer_disarm_is_confirmed(
                                                    function,
                                                    block,
                                                    index,
                                                    &place,
                                                    guard.scope,
                                                    None,
                                                ))
                                    }
                                };
                                if !confirmed {
                                    return Err(BytecodeVerificationError::new(
                                        instruction_context,
                                        format!(
                                            "{:?} cleanup guard is disarmed without an immediate consuming handoff or scope exit (pending {pending:?}, terminator {:?})",
                                            guard.kind, block.terminator.kind
                                        ),
                                    ));
                                }
                                state.registrations.remove(&guard.registration);
                                state.remove_inactive_scope(guard.scope);
                            }
                        }
                        BytecodeInstructionKind::Store { destination, value } => {
                            let destination_place = destination;
                            let destination = LocalAccess::from_place(destination_place);
                            if state.guards.iter().any(|(guard, active)| {
                                local_accesses_overlap(guard, &destination)
                                    && (active.kind == CleanupEntryKind::Explicit
                                        || !local_access_contains(guard, &destination)
                                        || guard == &destination)
                            }) {
                                return Err(BytecodeVerificationError::new(
                                    instruction_context,
                                    "store overwrites an active cleanup guard",
                                ));
                            }
                            let mut events = Vec::new();
                            push_rvalue_events(value, &mut events);
                            for access in events.into_iter().filter_map(|event| match event {
                                LocalEvent::Move(access) => Some(access),
                                _ => None,
                            }) {
                                let overlapping = state
                                    .guards
                                    .iter()
                                    .filter(|(guard, _)| local_accesses_overlap(guard, &access))
                                    .map(|(guard, active)| (guard.clone(), *active))
                                    .collect::<Vec<_>>();
                                for (guard_place, guard) in overlapping {
                                    if guard_place != access {
                                        if guard.kind == CleanupEntryKind::Fallback
                                            && local_access_contains(&guard_place, &access)
                                        {
                                            continue;
                                        }
                                        return Err(BytecodeVerificationError::new(
                                            &instruction_context,
                                            "store partially moves an explicit guard or embeds a fallback owner",
                                        ));
                                    }
                                    let transition = match guard.kind {
                                        CleanupEntryKind::Explicit => defer_assignment_transition(
                                            function,
                                            block,
                                            destination_place,
                                            value,
                                            &access,
                                            guard.scope,
                                        )
                                        .ok_or_else(|| {
                                            BytecodeVerificationError::new(
                                                &instruction_context,
                                                "store embeds an active defer guard instead of retargeting or handing it off",
                                            )
                                        })?,
                                        CleanupEntryKind::Fallback => {
                                            if assignment_directly_moves(value, &access) {
                                                PendingDeferTransition::Retarget
                                            } else {
                                                PendingDeferTransition::Disarm
                                            }
                                        }
                                    };
                                    if state
                                        .pending_moves
                                        .insert(access.clone(), transition)
                                        .is_some()
                                    {
                                        return Err(BytecodeVerificationError::new(
                                            &instruction_context,
                                            "one cleanup owner is moved more than once by one store",
                                        ));
                                    }
                                }
                            }
                        }
                        BytecodeInstructionKind::StorageLive(slot)
                        | BytecodeInstructionKind::StorageDead(slot) => {
                            if state.guards.keys().any(|guard| guard.slot == *slot) {
                                return Err(BytecodeVerificationError::new(
                                    instruction_context,
                                    "storage lifetime crosses an active defer guard",
                                ));
                            }
                        }
                        BytecodeInstructionKind::ReserveLoan(_)
                        | BytecodeInstructionKind::ReleaseLoan(_)
                        | BytecodeInstructionKind::EnterTaskScope { .. } => {}
                    }
                }
                if !state.pending_moves.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        &block_context,
                        "guarded move reaches a terminator without retarget or disarm",
                    ));
                }

                let mut terminator_events = Vec::new();
                let await_join_owner = match &block.terminator.kind {
                    BytecodeTerminatorKind::Await {
                        awaitable:
                            BytecodeAwaitable::Join(BytecodeOperand {
                                kind: BytecodeOperandKind::Move(place),
                                ..
                            }),
                        ..
                    } => Some(LocalAccess::from_place(place)),
                    _ => None,
                };
                match &block.terminator.kind {
                    BytecodeTerminatorKind::BranchBool { condition, .. }
                    | BytecodeTerminatorKind::BranchTag {
                        value: condition, ..
                    } => push_operand_events(condition, &mut terminator_events),
                    BytecodeTerminatorKind::Invoke { operation, .. }
                    | BytecodeTerminatorKind::Spawn { operation, .. } => {
                        push_operation_events(operation, &mut terminator_events);
                    }
                    BytecodeTerminatorKind::Await { awaitable, .. } => match awaitable {
                        BytecodeAwaitable::Call(operation) => {
                            push_operation_events(operation, &mut terminator_events);
                        }
                        BytecodeAwaitable::Join(join) => {
                            push_operand_events(join, &mut terminator_events);
                        }
                    },
                    BytecodeTerminatorKind::ValidatePlaces { replacements, .. } => {
                        for replacement in replacements.iter().flatten() {
                            push_operand_events(replacement, &mut terminator_events);
                        }
                    }
                    BytecodeTerminatorKind::Goto { .. }
                    | BytecodeTerminatorKind::IteratorNext { .. }
                    | BytecodeTerminatorKind::ValidateLoan { .. }
                    | BytecodeTerminatorKind::DrainDefers { .. }
                    | BytecodeTerminatorKind::DrainScopes { .. }
                    | BytecodeTerminatorKind::DrainUnwind { .. }
                    | BytecodeTerminatorKind::Return
                    | BytecodeTerminatorKind::ResumePanic
                    | BytecodeTerminatorKind::Unreachable => {}
                }
                for access in terminator_events
                    .into_iter()
                    .filter_map(|event| match event {
                        LocalEvent::Move(access) => Some(access),
                        _ => None,
                    })
                {
                    let overlaps = state
                        .guards
                        .iter()
                        .filter(|(guard, _)| local_accesses_overlap(guard, &access))
                        .map(|(guard, entry)| (guard.clone(), *entry))
                        .collect::<Vec<_>>();
                    if overlaps.len() == 1
                        && await_join_owner.as_ref() == Some(&access)
                        && overlaps[0].0 == access
                        && overlaps[0].1.kind == CleanupEntryKind::Fallback
                    {
                        let guard = state
                            .guards
                            .remove(&access)
                            .expect("the exact await Join guard was just observed");
                        state.registrations.remove(&guard.registration);
                        state.remove_inactive_scope(guard.scope);
                    } else if !overlaps.is_empty() {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "terminator moves an active defer guard without disarming it",
                        ));
                    }
                }

                if let BytecodeTerminatorKind::DrainDefers { scopes, .. } = &block.terminator.kind {
                    state.drain(scopes, &block_context)?;
                }
                if let BytecodeTerminatorKind::DrainScopes { defer_scopes, .. } =
                    &block.terminator.kind
                {
                    state.drain(defer_scopes, &block_context)?;
                }
                if matches!(
                    block.terminator.kind,
                    BytecodeTerminatorKind::DrainUnwind { .. }
                ) {
                    state.drain_unwind();
                }
                match block.terminator.kind {
                    BytecodeTerminatorKind::Return => {
                        state.finish_normal(&block_context)?;
                    }
                    BytecodeTerminatorKind::ResumePanic if !state.is_empty() => {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "panic resume abandons registered cleanup entries",
                        ));
                    }
                    _ => {}
                }
                outgoing.insert(state);
            }

            for edge in &successors[block_id.index() as usize] {
                let target = edge.target.index() as usize;
                let mut changed = false;
                for state in &outgoing {
                    let mut state = state.clone();
                    if let Some(destination) = &edge.writes {
                        let destination = LocalAccess::from_place(destination);
                        if state
                            .guards
                            .keys()
                            .any(|guard| local_accesses_overlap(guard, &destination))
                        {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "terminator overwrites an active defer guard",
                            ));
                        }
                        apply_consumed_defer_events(
                            &mut state,
                            &[LocalEvent::Write(destination)],
                            &block_context,
                        )?;
                    }
                    if let BytecodeTerminatorKind::IteratorNext {
                        exhaustion_guard: Some(place),
                        exhausted,
                        ..
                    } = &block.terminator.kind
                        && edge.target == *exhausted
                    {
                        let place = LocalAccess::from_place(place);
                        if let Some(guard) = state.guards.remove(&place) {
                            state.registrations.remove(&guard.registration);
                            state.remove_inactive_scope(guard.scope);
                        }
                    }
                    changed |= incoming[target].insert(state);
                }
                if changed && !queued[target] {
                    queued[target] = true;
                    queue.push_back(edge.target);
                }
            }
        }
        Ok(())
    }

    fn verify_fallback_coverage(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        for (block_index, block) in function.blocks.iter().enumerate() {
            let block_context = format!("{context} block#{block_index}");
            for (index, instruction) in block.instructions.iter().enumerate() {
                let BytecodeInstructionKind::Store { destination, value } = &instruction.kind
                else {
                    continue;
                };
                if self.terminal_status(destination.ty, &block_context)?
                    != BytecodeTerminalStatus::Present
                {
                    continue;
                }
                if let Some((from, to)) = store_cleanup_transfer(destination, value) {
                    if !matches!(
                        block.instructions.get(index + 1).map(|instruction| &instruction.kind),
                        Some(BytecodeInstructionKind::RetargetCleanup {
                            from: actual_from,
                            to: actual_to,
                        }) if actual_from == &from && actual_to == &to
                    ) {
                        return Err(BytecodeVerificationError::new(
                            format!("{block_context} instruction#{index}"),
                            "terminal store has no immediate cleanup retarget",
                        ));
                    }
                    continue;
                }
                let mut next = index + 1;
                while matches!(
                    block
                        .instructions
                        .get(next)
                        .map(|instruction| &instruction.kind),
                    Some(BytecodeInstructionKind::DisarmCleanup(_))
                ) {
                    next += 1;
                }
                if !matches!(
                    block.instructions.get(next).map(|instruction| &instruction.kind),
                    Some(BytecodeInstructionKind::RegisterFallback { owner, .. })
                        if owner == destination
                ) {
                    return Err(BytecodeVerificationError::new(
                        format!("{block_context} instruction#{index}"),
                        "terminal store result has no immediate fallback registration",
                    ));
                }
            }
            match &block.terminator.kind {
                BytecodeTerminatorKind::Invoke {
                    destination: Some(destination),
                    target: Some(target),
                    ..
                } if self.terminal_status(destination.ty, &block_context)?
                    == BytecodeTerminalStatus::Present =>
                {
                    let target = self.block(function, *target, &block_context)?;
                    if !matches!(
                        target.instructions.first().map(|instruction| &instruction.kind),
                        Some(BytecodeInstructionKind::RegisterFallback { owner, .. })
                            if owner == destination
                    ) {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "terminal invocation result edge has no fallback registration",
                        ));
                    }
                }
                BytecodeTerminatorKind::IteratorNext {
                    destination,
                    has_value,
                    ..
                } if self.terminal_status(destination.ty, &block_context)?
                    == BytecodeTerminalStatus::Present =>
                {
                    let target = self.block(function, *has_value, &block_context)?;
                    if !matches!(
                        target.instructions.first().map(|instruction| &instruction.kind),
                        Some(BytecodeInstructionKind::RegisterFallback { owner, .. })
                            if owner == destination
                    ) {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "terminal iterator value edge has no fallback registration",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn terminal_entry_owners(
        &self,
        function: &BytecodeFunction,
        context: &str,
    ) -> Result<Vec<BytecodePlace>, BytecodeVerificationError> {
        let callable = self.program.callable(function.callable).ok_or_else(|| {
            BytecodeVerificationError::new(context, "function references an unknown callable")
        })?;
        let hidden_environment = usize::from(callable.closure.is_some());
        let mut candidates = Vec::new();
        if let Some(closure) = &callable.closure {
            let environment = function.parameters.first().copied().ok_or_else(|| {
                BytecodeVerificationError::new(
                    context,
                    "closure function has no hidden environment parameter",
                )
            })?;
            for (index, ty) in closure.captures.iter().copied().enumerate() {
                candidates.push(BytecodePlace {
                    slot: environment,
                    ty,
                    projections: vec![BytecodeProjection {
                        ty,
                        kind: BytecodeProjectionKind::ClosureCapture {
                            callable: function.callable,
                            index: index as u32,
                        },
                    }],
                    source_loan: None,
                });
            }
        }
        for (index, slot) in function
            .parameters
            .iter()
            .copied()
            .skip(hidden_environment)
            .enumerate()
        {
            let parameter = callable.parameters.get(index).ok_or_else(|| {
                BytecodeVerificationError::new(
                    context,
                    "function parameter index exceeds its callable signature",
                )
            })?;
            if parameter.mode == BytecodeParameterMode::Value && !parameter.receiver {
                candidates.push(BytecodePlace {
                    slot,
                    ty: self.slot(function, slot, context)?.ty,
                    projections: Vec::new(),
                    source_loan: None,
                });
            }
        }
        candidates
            .into_iter()
            .filter_map(|owner| {
                self.terminal_status(owner.ty, context)
                    .map(|status| (status == BytecodeTerminalStatus::Present).then_some(owner))
                    .transpose()
            })
            .collect()
    }

    fn verify_loan_flow(
        &self,
        function: &BytecodeFunction,
        reachable: &[bool],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let events = function
            .blocks
            .iter()
            .map(|block| bytecode_loan_events(function, block))
            .collect::<Vec<_>>();
        let static_integers = static_integer_slots(self.program, function);
        let mut reservations = vec![0_u32; function.loans.len()];
        let mut validations = vec![0_u32; function.loans.len()];
        let mut consumptions = vec![0_u32; function.loans.len()];
        for block_events in &events {
            for event in block_events {
                match event {
                    LoanEvent::Reserve(loan) => {
                        let count =
                            reservations.get_mut(loan.index() as usize).ok_or_else(|| {
                                BytecodeVerificationError::new(context, "reserves an unknown loan")
                            })?;
                        *count = count.saturating_add(1);
                    }
                    LoanEvent::Consume(loans) => {
                        for loan in loans {
                            let count =
                                consumptions.get_mut(loan.index() as usize).ok_or_else(|| {
                                    BytecodeVerificationError::new(
                                        context,
                                        "consumes an unknown loan",
                                    )
                                })?;
                            *count = count.saturating_add(1);
                        }
                    }
                    LoanEvent::Local(_) | LoanEvent::Release(_) => {}
                }
            }
        }
        for block in &function.blocks {
            if let BytecodeTerminatorKind::ValidateLoan { loan, .. } = &block.terminator.kind {
                let count = validations.get_mut(loan.index() as usize).ok_or_else(|| {
                    BytecodeVerificationError::new(context, "validates an unknown loan")
                })?;
                *count = count.saturating_add(1);
            }
        }
        for index in 0..function.loans.len() {
            let loan = &function.loans[index];
            let valid_consumptions = match loan.kind {
                BytecodeLoanKind::CallLocal => consumptions[index] <= 1,
                BytecodeLoanKind::Region => consumptions[index] == 0,
            };
            let expected_validations = u32::from(place_requires_loan_validation(&loan.place));
            if reservations[index] != 1
                || validations[index] != expected_validations
                || !valid_consumptions
            {
                return Err(BytecodeVerificationError::new(
                    format!("{context} loan#{index}"),
                    format!(
                        "has {} validations, {} reservations, and {} call consumptions, which violates its {:?} contract",
                        validations[index], reservations[index], consumptions[index], loan.kind
                    ),
                ));
            }
        }

        let mut incoming = vec![None::<LoanFlowState>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(LoanFlowState::default());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let mut state = incoming[block_id.index() as usize]
                .clone()
                .expect("queued loan-flow blocks have an incoming state");
            let block_context = format!("{context} block#{}", block_id.index());
            for event in &events[block_id.index() as usize] {
                self.apply_loan_event(
                    function,
                    &static_integers,
                    &mut state,
                    event,
                    &block_context,
                )?;
            }
            let block = &function.blocks[block_id.index() as usize];
            let mut propagate = |target: BytecodeBlockId,
                                 edge_state: LoanFlowState|
             -> Result<(), BytecodeVerificationError> {
                let target_index = target.index() as usize;
                if !reachable[target_index] {
                    return Ok(());
                }
                match &incoming[target_index] {
                    Some(existing) if existing != &edge_state => {
                        return Err(BytecodeVerificationError::new(
                            format!("{context} block#{}", target.index()),
                            "control-flow predecessors disagree about active loans",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        incoming[target_index] = Some(edge_state);
                        if !queued[target_index] {
                            queued[target_index] = true;
                            queue.push_back(target);
                        }
                    }
                }
                Ok(())
            };
            if !state.accesses.is_empty() {
                return Err(BytecodeVerificationError::new(
                    &block_context,
                    "runtime place proof is not consumed by the immediate access",
                ));
            }
            match &block.terminator.kind {
                BytecodeTerminatorKind::Goto { target } => propagate(*target, state)?,
                BytecodeTerminatorKind::BranchBool {
                    if_true, if_false, ..
                } => {
                    propagate(*if_true, state.clone())?;
                    propagate(*if_false, state)?;
                }
                BytecodeTerminatorKind::BranchTag {
                    cases, otherwise, ..
                } => {
                    for (_, target) in cases {
                        propagate(*target, state.clone())?;
                    }
                    propagate(*otherwise, state)?;
                }
                BytecodeTerminatorKind::Invoke {
                    operation,
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    if let Some((place, against)) =
                        operation_access_place(operation, &block_context)?
                    {
                        let expected = self.runtime_place_conflicts(
                            function,
                            &static_integers,
                            &state.active,
                            &place,
                            false,
                            &block_context,
                        )?;
                        if against != expected {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "indexed operation runtime proof differs from active dynamic conflicts",
                            ));
                        }
                    }
                    if let Some(target) = target {
                        let normal = state.clone();
                        if let Some(destination) = destination {
                            self.verify_loan_local_access(
                                function,
                                &static_integers,
                                &normal.active,
                                &LocalEvent::Write(LocalAccess::from_place(destination)),
                                None,
                                &block_context,
                            )?;
                        }
                        propagate(*target, normal)?;
                    }
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::Await {
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    for loan in &state.active {
                        if self.loan(function, *loan, &block_context)?.mode
                            != BytecodeParameterMode::Ref
                        {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "exclusive loan crosses an await suspension",
                            ));
                        }
                    }
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::Spawn {
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::IteratorNext {
                    destination,
                    borrowed_source,
                    has_value,
                    exhausted,
                    unwind,
                    ..
                } => {
                    let source_chain = borrowed_source
                        .as_ref()
                        .map(|source| self.place_source_chain(function, source, &block_context))
                        .transpose()?
                        .unwrap_or_default();
                    for id in &state.active {
                        let loan = self.loan(function, *id, &block_context)?;
                        if loan.kind != BytecodeLoanKind::Region
                            || (loan.mode != BytecodeParameterMode::Ref
                                && !source_chain.contains(id))
                        {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "only shared regions or the iterator source chain may cross an iterator boundary",
                            ));
                        }
                    }
                    let has_value_state = state.clone();
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &has_value_state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*has_value, has_value_state)?;
                    propagate(*exhausted, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::ValidatePlaces {
                    places,
                    against,
                    for_write,
                    target,
                    unwind,
                    ..
                } => {
                    let expected = places
                        .iter()
                        .map(|place| {
                            self.runtime_place_conflicts(
                                function,
                                &static_integers,
                                &state.active,
                                place,
                                *for_write,
                                &block_context,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if against != &expected {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            "place validation runtime proof differs from active dynamic conflicts",
                        ));
                    }
                    for (place, loans) in places.iter().zip(expected) {
                        if loans.is_empty() {
                            continue;
                        }
                        let key = ValidatedAccess {
                            access: LocalAccess::from_place(place),
                            for_write: *for_write,
                        };
                        if state.accesses.insert(key, loans).is_some() {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "place validation duplicates a pending runtime access proof",
                            ));
                        }
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::ValidateLoan {
                    loan,
                    against,
                    target,
                    unwind,
                } => {
                    let expected = self.runtime_loan_conflicts(
                        function,
                        &static_integers,
                        &state.active,
                        *loan,
                        &block_context,
                    )?;
                    if against != &expected {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            format!(
                                "loan#{} runtime proof lists {:?}, expected {:?}",
                                loan.index(),
                                against.iter().map(|loan| loan.index()).collect::<Vec<_>>(),
                                expected.iter().map(|loan| loan.index()).collect::<Vec<_>>()
                            ),
                        ));
                    }
                    if state.active.contains(loan)
                        || state.validated.insert(*loan, expected).is_some()
                    {
                        return Err(BytecodeVerificationError::new(
                            &block_context,
                            format!("validates already-active or pending loan#{}", loan.index()),
                        ));
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::DrainDefers { target, unwind, .. } => {
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::DrainScopes { target, unwind, .. } => {
                    for loan in &state.active {
                        if self.loan(function, *loan, &block_context)?.mode
                            != BytecodeParameterMode::Ref
                        {
                            return Err(BytecodeVerificationError::new(
                                &block_context,
                                "exclusive loan crosses structured scope suspension",
                            ));
                        }
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::DrainUnwind { target } => {
                    propagate(*target, LoanFlowState::default())?;
                }
                BytecodeTerminatorKind::Return => {
                    if !state.active.is_empty()
                        || !state.validated.is_empty()
                        || !state.accesses.is_empty()
                    {
                        return Err(BytecodeVerificationError::new(
                            block_context,
                            "return abandons active loans without explicit release",
                        ));
                    }
                }
                BytecodeTerminatorKind::ResumePanic | BytecodeTerminatorKind::Unreachable => {}
            }
        }
        Ok(())
    }

    fn apply_loan_event(
        &self,
        function: &BytecodeFunction,
        static_integers: &BTreeMap<BytecodeSlotId, u64>,
        state: &mut LoanFlowState,
        event: &LoanEvent,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        match event {
            LoanEvent::Local(event) => {
                self.verify_runtime_proof_inputs_stable(function, state, event, context)?;
                let key = match event {
                    LocalEvent::Read(access) => Some(ValidatedAccess {
                        access: access.clone(),
                        for_write: false,
                    }),
                    LocalEvent::Move(access) | LocalEvent::Write(access) => Some(ValidatedAccess {
                        access: access.clone(),
                        for_write: true,
                    }),
                    LocalEvent::Resolve(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => None,
                };
                let proof = key.as_ref().and_then(|key| state.accesses.remove(key));
                self.verify_loan_local_access(
                    function,
                    static_integers,
                    &state.active,
                    event,
                    proof.as_deref(),
                    context,
                )
            }
            LoanEvent::Reserve(id) => {
                if !state.accesses.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "reserves a loan while a runtime access proof is pending",
                    ));
                }
                let loan = self.loan(function, *id, context)?;
                self.verify_reborrow_mode(function, loan, context)?;
                if state.active.contains(id) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!("reserves already-active loan#{}", id.index()),
                    ));
                }
                let proof = state.validated.remove(id);
                if place_requires_loan_validation(&loan.place) != proof.is_some() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!(
                            "loan#{} reservation disagrees with its required validation",
                            id.index()
                        ),
                    ));
                }
                let source_chain = self.place_source_chain(function, &loan.place, context)?;
                for active in state.active.iter().copied() {
                    self.consume_dataflow_step(context)?;
                    if source_chain.contains(&active) {
                        continue;
                    }
                    let existing = self.loan(function, active, context)?;
                    if loan.mode == BytecodeParameterMode::Ref
                        && existing.mode == BytecodeParameterMode::Ref
                    {
                        continue;
                    }
                    let relation =
                        loan_place_relation(&loan.place, &existing.place, static_integers);
                    match relation {
                        StaticRegionRelation::Disjoint => {}
                        StaticRegionRelation::Runtime
                            if proof
                                .as_ref()
                                .is_some_and(|against| against.contains(&active)) => {}
                        StaticRegionRelation::Runtime | StaticRegionRelation::Overlap => {
                            return Err(BytecodeVerificationError::new(
                                context,
                                format!(
                                    "loan#{} lacks a valid proof against incompatible active loan#{}",
                                    id.index(),
                                    active.index()
                                ),
                            ));
                        }
                    }
                }
                state.active.insert(*id);
                Ok(())
            }
            LoanEvent::Release(loan) => {
                if !state.validated.is_empty() || !state.accesses.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "releases a loan while another reservation proof is pending",
                    ));
                }
                if !state.active.contains(loan) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!("releases inactive loan#{}", loan.index()),
                    ));
                }
                if let Some(dependent) =
                    self.active_dependent_loan(function, &state.active, *loan, context)?
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!(
                            "releases source region loan#{} while dependent loan#{} remains active",
                            loan.index(),
                            dependent.index()
                        ),
                    ));
                }
                state.active.remove(loan);
                Ok(())
            }
            LoanEvent::Consume(loans) => {
                if !state.validated.is_empty() || !state.accesses.is_empty() {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "consumes loans while a runtime proof is pending",
                    ));
                }
                let mut seen = BTreeSet::new();
                for loan in loans {
                    let metadata = self.loan(function, *loan, context)?;
                    if metadata.kind != BytecodeLoanKind::CallLocal {
                        return Err(BytecodeVerificationError::new(
                            context,
                            format!("call consumes region loan#{}", loan.index()),
                        ));
                    }
                    self.verify_source_loan_access(
                        function,
                        &state.active,
                        &LocalAccess::from_place(&metadata.place),
                        "read",
                        context,
                    )?;
                    if !seen.insert(*loan) || !state.active.remove(loan) {
                        return Err(BytecodeVerificationError::new(
                            context,
                            format!("consumes inactive loan#{}", loan.index()),
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn verify_runtime_proof_inputs_stable(
        &self,
        function: &BytecodeFunction,
        state: &LoanFlowState,
        event: &LocalEvent,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let changed = match event {
            LocalEvent::Move(access)
            | LocalEvent::Write(access)
            | LocalEvent::WriteAccess(access) => Some(access.slot),
            LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => Some(*slot),
            LocalEvent::Read(_) | LocalEvent::Resolve(_) => None,
        };
        let Some(changed) = changed else {
            return Ok(());
        };
        let access_input_changed = state.accesses.keys().any(|validated| {
            move_path_runtime_inputs(&validated.access.path).any(|slot| slot == changed)
        });
        let loan_input_changed =
            state
                .validated
                .keys()
                .try_fold(false, |changed_input, loan| {
                    if changed_input {
                        return Ok(true);
                    }
                    let loan = self.loan(function, *loan, context)?;
                    Ok(
                        move_path_runtime_inputs(&LocalAccess::from_place(&loan.place).path)
                            .any(|slot| slot == changed),
                    )
                })?;
        if access_input_changed || loan_input_changed {
            return Err(BytecodeVerificationError::new(
                context,
                format!(
                    "changes slot#{} while it is an input to a pending runtime-overlap proof",
                    changed.index()
                ),
            ));
        }
        Ok(())
    }

    fn runtime_loan_conflicts(
        &self,
        function: &BytecodeFunction,
        static_integers: &BTreeMap<BytecodeSlotId, u64>,
        active: &BTreeSet<BytecodeLoanId>,
        candidate: BytecodeLoanId,
        context: &str,
    ) -> Result<Vec<BytecodeLoanId>, BytecodeVerificationError> {
        let loan = self.loan(function, candidate, context)?;
        if !place_requires_loan_validation(&loan.place) {
            return Err(BytecodeVerificationError::new(
                context,
                format!(
                    "loan#{} has no runtime-resolvable collection projection",
                    candidate.index()
                ),
            ));
        }
        let mut against = Vec::new();
        let source_chain = self.place_source_chain(function, &loan.place, context)?;
        for active in active.iter().copied() {
            self.consume_dataflow_step(context)?;
            if source_chain.contains(&active) {
                continue;
            }
            let existing = self.loan(function, active, context)?;
            if loan.mode == BytecodeParameterMode::Ref
                && existing.mode == BytecodeParameterMode::Ref
            {
                continue;
            }
            match loan_place_relation(&loan.place, &existing.place, static_integers) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime => against.push(active),
                StaticRegionRelation::Overlap => {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!(
                            "loan#{} statically overlaps incompatible active loan#{}",
                            candidate.index(),
                            active.index()
                        ),
                    ));
                }
            }
        }
        Ok(against)
    }

    fn runtime_place_conflicts(
        &self,
        function: &BytecodeFunction,
        static_integers: &BTreeMap<BytecodeSlotId, u64>,
        active: &BTreeSet<BytecodeLoanId>,
        place: &BytecodePlace,
        for_write: bool,
        context: &str,
    ) -> Result<Vec<BytecodeLoanId>, BytecodeVerificationError> {
        let mut against = Vec::new();
        let source_chain = self.place_source_chain(function, place, context)?;
        for active in active.iter().copied() {
            self.consume_dataflow_step(context)?;
            if source_chain.contains(&active) {
                continue;
            }
            let existing = self.loan(function, active, context)?;
            if !for_write && existing.mode == BytecodeParameterMode::Ref {
                continue;
            }
            match loan_place_relation(place, &existing.place, static_integers) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime => against.push(active),
                StaticRegionRelation::Overlap => {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!(
                            "place validation statically overlaps active loan#{}",
                            active.index()
                        ),
                    ));
                }
            }
        }
        Ok(against)
    }

    fn place_source_chain(
        &self,
        function: &BytecodeFunction,
        place: &BytecodePlace,
        context: &str,
    ) -> Result<BTreeSet<BytecodeLoanId>, BytecodeVerificationError> {
        let mut chain = BTreeSet::new();
        let mut source = place.source_loan;
        while let Some(id) = source {
            if !chain.insert(id) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            source = self.loan(function, id, context)?.place.source_loan;
        }
        Ok(chain)
    }

    fn active_dependent_loan(
        &self,
        function: &BytecodeFunction,
        state: &BTreeSet<BytecodeLoanId>,
        source: BytecodeLoanId,
        context: &str,
    ) -> Result<Option<BytecodeLoanId>, BytecodeVerificationError> {
        for candidate in state
            .iter()
            .copied()
            .filter(|candidate| *candidate != source)
        {
            let mut parent = self.loan(function, candidate, context)?.place.source_loan;
            let mut seen = BTreeSet::new();
            while let Some(id) = parent {
                if id == source {
                    return Ok(Some(candidate));
                }
                if !seen.insert(id) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "loan source region chain contains a cycle",
                    ));
                }
                parent = self.loan(function, id, context)?.place.source_loan;
            }
        }
        Ok(None)
    }

    fn verify_loan_local_access(
        &self,
        function: &BytecodeFunction,
        static_integers: &BTreeMap<BytecodeSlotId, u64>,
        state: &BTreeSet<BytecodeLoanId>,
        event: &LocalEvent,
        proof: Option<&[BytecodeLoanId]>,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let (access, access_kind) = match event {
            LocalEvent::Read(access) => (Some(access), "read"),
            LocalEvent::Resolve(access) => {
                self.verify_source_loan_access(function, state, access, "read", context)?;
                return Ok(());
            }
            LocalEvent::Move(access) => (Some(access), "move"),
            LocalEvent::Write(access) | LocalEvent::WriteAccess(access) => (Some(access), "write"),
            LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => {
                let access = LocalAccess {
                    slot: *slot,
                    path: Vec::new(),
                    source_loan: None,
                };
                return self.verify_active_loan_access(
                    function,
                    static_integers,
                    state,
                    ClassifiedLocalAccess {
                        access: &access,
                        kind: "storage change",
                    },
                    None,
                    context,
                );
            }
        };
        let access = access.expect("access events carry a place");
        self.verify_source_loan_access(function, state, access, access_kind, context)?;
        if let Some(mode) = self.parameter_mode(function, access.slot, context)? {
            if access_kind == "move" && mode != BytecodeParameterMode::Value {
                return Err(BytecodeVerificationError::new(
                    context,
                    "moves content out of a borrowed parameter",
                ));
            }
            if access_kind == "write" && mode == BytecodeParameterMode::Ref {
                return Err(BytecodeVerificationError::new(
                    context,
                    "writes through a shared `ref` parameter",
                ));
            }
        }
        self.verify_active_loan_access(
            function,
            static_integers,
            state,
            ClassifiedLocalAccess {
                access,
                kind: access_kind,
            },
            proof,
            context,
        )
    }

    fn verify_source_loan_access(
        &self,
        function: &BytecodeFunction,
        state: &BTreeSet<BytecodeLoanId>,
        access: &LocalAccess,
        access_kind: &str,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let Some(mut source) = access.source_loan else {
            return Ok(());
        };
        if access_kind == "move" {
            return Err(BytecodeVerificationError::new(
                context,
                "move transfers content out of a region reference",
            ));
        }
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(source) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            if !state.contains(&source) {
                return Err(BytecodeVerificationError::new(
                    context,
                    format!("read uses inactive source region loan#{}", source.index()),
                ));
            }
            let loan = self.loan(function, source, context)?;
            if loan.kind != BytecodeLoanKind::Region {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place source is not a region loan",
                ));
            }
            if access_kind == "write" && loan.mode == BytecodeParameterMode::Ref {
                return Err(BytecodeVerificationError::new(
                    context,
                    "write uses a shared region reference",
                ));
            }
            let Some(parent) = loan.place.source_loan else {
                return Ok(());
            };
            source = parent;
        }
    }

    fn verify_active_loan_access(
        &self,
        function: &BytecodeFunction,
        static_integers: &BTreeMap<BytecodeSlotId, u64>,
        state: &BTreeSet<BytecodeLoanId>,
        access: ClassifiedLocalAccess<'_>,
        proof: Option<&[BytecodeLoanId]>,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let ClassifiedLocalAccess { access, kind } = access;
        let mut source_chain = BTreeSet::new();
        let mut source = access.source_loan;
        while let Some(id) = source {
            if !source_chain.insert(id) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            source = self.loan(function, id, context)?.place.source_loan;
        }
        for active in state.iter().copied() {
            self.consume_dataflow_step(context)?;
            let loan = self.loan(function, active, context)?;
            let loan_access = LocalAccess::from_place(&loan.place);
            if source_chain.contains(&active)
                || access.slot != loan_access.slot
                || kind == "read" && loan.mode == BytecodeParameterMode::Ref
            {
                continue;
            }
            match loan_paths_relation(&access.path, &loan_access.path, static_integers) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime
                    if proof.is_some_and(|proof| proof.contains(&active)) => {}
                StaticRegionRelation::Runtime | StaticRegionRelation::Overlap => {
                    return Err(BytecodeVerificationError::new(
                        context,
                        format!(
                            "{kind} overlaps active loan#{} ({:?})",
                            active.index(),
                            loan.mode
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_reborrow_mode(
        &self,
        function: &BytecodeFunction,
        loan: &BytecodeLoan,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let Some(source) = self.loan_source_mode(function, loan, context)? else {
            return Ok(());
        };
        let compatible = match loan.mode {
            BytecodeParameterMode::Value => false,
            BytecodeParameterMode::Ref => true,
            BytecodeParameterMode::Mut => matches!(
                source,
                BytecodeParameterMode::Mut | BytecodeParameterMode::Var
            ),
            BytecodeParameterMode::Var => {
                source == BytecodeParameterMode::Var
                    || source == BytecodeParameterMode::Mut
                        && loan.place.is_structurally_replaceable()
            }
        };
        if compatible {
            Ok(())
        } else {
            Err(BytecodeVerificationError::new(
                context,
                "loan requests stronger permissions than its borrowed parameter source",
            ))
        }
    }

    fn loan_source_mode(
        &self,
        function: &BytecodeFunction,
        loan: &BytecodeLoan,
        context: &str,
    ) -> Result<Option<BytecodeParameterMode>, BytecodeVerificationError> {
        if let Some(source) = loan.place.source_loan {
            let source = self.loan(function, source, context)?;
            if source.kind != BytecodeLoanKind::Region {
                return Err(BytecodeVerificationError::new(
                    context,
                    "reborrow source is not a region loan",
                ));
            }
            return Ok(Some(source.mode));
        }
        let callable = self.callable(function.callable, context)?;
        if callable.closure.is_some()
            && function.parameters.first() == Some(&loan.place.slot)
            && let Some(BytecodeProjectionKind::ClosureCapture {
                callable: projected,
                ..
            }) = loan
                .place
                .projections
                .first()
                .map(|projection| &projection.kind)
        {
            if *projected != function.callable {
                return Err(BytecodeVerificationError::new(
                    context,
                    "loan capture projection belongs to a different closure",
                ));
            }
            // The invocation owns its environment. Source-level capture
            // mutability was proved before bytecode lowering, while the
            // derived closure protocol proves exclusive access to stateful
            // bodies at this representation boundary.
            return Ok(Some(BytecodeParameterMode::Var));
        }
        self.parameter_mode(function, loan.place.slot, context)
    }

    fn parameter_mode(
        &self,
        function: &BytecodeFunction,
        slot: BytecodeSlotId,
        context: &str,
    ) -> Result<Option<BytecodeParameterMode>, BytecodeVerificationError> {
        let BytecodeSlotKind::Parameter { index } = self.slot(function, slot, context)?.kind else {
            return Ok(None);
        };
        let callable = self.callable(function.callable, context)?;
        if callable.closure.is_some() && index == 0 {
            return Ok(Some(BytecodeParameterMode::Value));
        }
        let offset = u32::from(callable.closure.is_some());
        callable
            .parameters
            .get((index - offset) as usize)
            .map(|parameter| Some(parameter.mode))
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    context,
                    "parameter slot has no matching callable parameter mode",
                )
            })
    }

    fn verify_tag_refinements(
        &self,
        function: &BytecodeFunction,
        successors: &[Vec<SuccessorEdge>],
        reachable: &[bool],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let events = function
            .blocks
            .iter()
            .map(|block| tag_events(function, block))
            .collect::<Vec<_>>();
        let mut facts = Vec::<TagFact>::new();
        for fact in events.iter().flatten().filter_map(|event| match event {
            TagEvent::Require(fact) => Some(fact),
            TagEvent::Write(_) => None,
        }) {
            if !facts.contains(fact) {
                facts.push(fact.clone());
            }
        }
        if facts.is_empty() {
            return Ok(());
        }
        let mut predecessors =
            vec![Vec::<(BytecodeBlockId, SuccessorEdge)>::new(); function.blocks.len()];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target.index() as usize]
                    .push((BytecodeBlockId::new(source as u32), edge.clone()));
            }
        }
        for fact in facts {
            let mut incoming = vec![true; function.blocks.len()];
            incoming[function.entry.index() as usize] = false;
            let mut queue = (0..function.blocks.len())
                .filter(|index| reachable[*index] && *index != function.entry.index() as usize)
                .map(|index| BytecodeBlockId::new(index as u32))
                .collect::<VecDeque<_>>();
            let mut queued = reachable.to_vec();
            queued[function.entry.index() as usize] = false;
            while let Some(block) = queue.pop_front() {
                queued[block.index() as usize] = false;
                self.consume_dataflow_step(context)?;
                let mut state = true;
                let mut found = false;
                for (predecessor, edge) in &predecessors[block.index() as usize] {
                    if !reachable[predecessor.index() as usize] {
                        continue;
                    }
                    let mut edge_state = transfer_tag(
                        incoming[predecessor.index() as usize],
                        &events[predecessor.index() as usize],
                        &fact,
                    );
                    if edge
                        .writes
                        .as_ref()
                        .is_some_and(|write| places_may_overlap(write, &fact.place))
                    {
                        edge_state = false;
                    }
                    if edge.refinement.as_ref() == Some(&fact) {
                        edge_state = true;
                    }
                    state &= edge_state;
                    found = true;
                }
                if !found {
                    continue;
                }
                let index = block.index() as usize;
                if incoming[index] != state {
                    incoming[index] = state;
                    for edge in &successors[index] {
                        let next = edge.target.index() as usize;
                        if reachable[next] && edge.target != function.entry && !queued[next] {
                            queued[next] = true;
                            queue.push_back(edge.target);
                        }
                    }
                }
            }
            for (block_index, block_events) in events.iter().enumerate() {
                if !reachable[block_index] {
                    continue;
                }
                let mut state = incoming[block_index];
                for event in block_events {
                    match event {
                        TagEvent::Require(required) if required == &fact => {
                            if !state {
                                return Err(BytecodeVerificationError::new(
                                    format!("{context} block#{block_index}"),
                                    format!(
                                        "projects {:?} without a dominating matching BranchTag",
                                        fact.tag
                                    ),
                                ));
                            }
                        }
                        TagEvent::Write(write) if places_may_overlap(write, &fact.place) => {
                            state = false;
                        }
                        TagEvent::Require(_) | TagEvent::Write(_) => {}
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_slot_flow(
        &self,
        function: &BytecodeFunction,
        slot: BytecodeSlotId,
        events: &[Vec<LocalEvent>],
        successors: &[Vec<SuccessorEdge>],
        predecessors: &[Vec<(BytecodeBlockId, SuccessorEdge)>],
        reachable: &[bool],
        managed_storage: bool,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let slot_kind = self.slot(function, slot, context)?.kind;
        if managed_storage
            && matches!(
                slot_kind,
                BytecodeSlotKind::Return | BytecodeSlotKind::Parameter { .. }
            )
        {
            return Err(BytecodeVerificationError::new(
                context,
                format!(
                    "slot#{} has function-wide storage but explicit lifetime instructions",
                    slot.index()
                ),
            ));
        }
        let mut initial_unavailable = BTreeSet::new();
        if !matches!(slot_kind, BytecodeSlotKind::Parameter { .. }) {
            initial_unavailable.insert(Vec::new());
        }
        let initial = LocalState {
            live: !managed_storage,
            unavailable: initial_unavailable,
        };
        let top = LocalState {
            live: true,
            unavailable: BTreeSet::new(),
        };
        let mut incoming = vec![top.clone(); function.blocks.len()];
        incoming[function.entry.index() as usize] = initial;
        let mut queue = (0..function.blocks.len())
            .filter(|index| reachable[*index] && *index != function.entry.index() as usize)
            .map(|index| BytecodeBlockId::new(index as u32))
            .collect::<VecDeque<_>>();
        let mut queued = reachable.to_vec();
        queued[function.entry.index() as usize] = false;
        while let Some(block) = queue.pop_front() {
            queued[block.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let mut state = top.clone();
            let mut found = false;
            for (predecessor, edge) in &predecessors[block.index() as usize] {
                if !reachable[predecessor.index() as usize] {
                    continue;
                }
                let mut edge_state = transfer_local(
                    incoming[predecessor.index() as usize].clone(),
                    &events[predecessor.index() as usize],
                    slot,
                );
                if let Some(write) = edge.writes.as_ref().filter(|place| place.slot == slot)
                    && edge_state.live
                {
                    write_path_unchecked(
                        &mut edge_state.unavailable,
                        &LocalAccess::from_place(write).path,
                    );
                }
                state.live &= edge_state.live;
                state.unavailable.extend(edge_state.unavailable);
                found = true;
            }
            if !found {
                continue;
            }
            let index = block.index() as usize;
            if incoming[index] != state {
                incoming[index] = state;
                for edge in &successors[index] {
                    let next = edge.target.index() as usize;
                    if reachable[next] && edge.target != function.entry && !queued[next] {
                        queued[next] = true;
                        queue.push_back(edge.target);
                    }
                }
            }
        }
        for (block_index, block_events) in events.iter().enumerate() {
            if !reachable[block_index] {
                continue;
            }
            let mut state = incoming[block_index].clone();
            for event in block_events {
                match event {
                    LocalEvent::Read(access) | LocalEvent::Resolve(access)
                        if access.slot == slot =>
                    {
                        if !state.live || !path_is_available(&state.unavailable, &access.path) {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                unavailable_read_message(slot, &access.path),
                            ));
                        }
                    }
                    LocalEvent::Move(access) if access.slot == slot => {
                        if !state.live || !path_is_available(&state.unavailable, &access.path) {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                unavailable_move_message(slot, &access.path),
                            ));
                        }
                        move_path_unchecked(&mut state.unavailable, access.path.clone());
                    }
                    LocalEvent::WriteAccess(access) if access.slot == slot => {
                        if !state.live
                            || !path_parent_is_available(&state.unavailable, &access.path)
                        {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                format!(
                                    "resolves a write through unavailable slot#{}",
                                    slot.index()
                                ),
                            ));
                        }
                    }
                    LocalEvent::Write(access) if access.slot == slot => {
                        if !state.live {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                format!("writes slot#{} outside its lifetime", slot.index()),
                            ));
                        }
                        if !path_parent_is_available(&state.unavailable, &access.path) {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                format!("writes through unavailable slot#{}", slot.index()),
                            ));
                        }
                        write_path_unchecked(&mut state.unavailable, &access.path);
                    }
                    LocalEvent::StorageLive(event_slot) if *event_slot == slot => {
                        state.live = true;
                        state.unavailable.clear();
                        state.unavailable.insert(Vec::new());
                    }
                    LocalEvent::StorageDead(event_slot) if *event_slot == slot => {
                        if !state.live {
                            return Err(BytecodeVerificationError::new(
                                format!("{context} block#{block_index}"),
                                format!("ends dead storage for slot#{}", slot.index()),
                            ));
                        }
                        state.live = false;
                        state.unavailable.clear();
                        state.unavailable.insert(Vec::new());
                    }
                    LocalEvent::Read(_)
                    | LocalEvent::Resolve(_)
                    | LocalEvent::Move(_)
                    | LocalEvent::Write(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => {}
                }
            }
        }
        Ok(())
    }

    fn consume_dataflow_step(&self, context: &str) -> Result<(), BytecodeVerificationError> {
        let next = self.dataflow_steps.get().saturating_add(1);
        if next > self.limits.max_dataflow_steps {
            return Err(BytecodeVerificationError::resource_limit(
                context,
                format!(
                    "verification exceeded its {}-step dataflow budget",
                    self.limits.max_dataflow_steps
                ),
            ));
        }
        self.dataflow_steps.set(next);
        Ok(())
    }

    fn function_type(
        &self,
        function: &BytecodeFunction,
        ty: BytecodeTypeId,
        context: &str,
    ) -> Result<&BytecodeType, BytecodeVerificationError> {
        if function.types.binary_search(&ty).is_err() {
            return Err(BytecodeVerificationError::new(
                context,
                format!("type#{} is absent from the function type table", ty.index()),
            ));
        }
        self.ty(ty, context)
    }

    fn slot<'a>(
        &self,
        function: &'a BytecodeFunction,
        id: BytecodeSlotId,
        context: &str,
    ) -> Result<&'a BytecodeSlot, BytecodeVerificationError> {
        function.slot(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown slot#{}", id.index()),
            )
        })
    }

    fn loan<'a>(
        &self,
        function: &'a BytecodeFunction,
        id: BytecodeLoanId,
        context: &str,
    ) -> Result<&'a BytecodeLoan, BytecodeVerificationError> {
        function.loans.get(id.index() as usize).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown loan#{}", id.index()),
            )
        })
    }

    fn verify_runtime_conflict_ids(
        &self,
        function: &BytecodeFunction,
        loans: &[BytecodeLoanId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let mut previous = None;
        for loan in loans {
            self.loan(function, *loan, context)?;
            if previous.is_some_and(|previous| previous >= *loan) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "runtime conflict IDs are not unique and canonical",
                ));
            }
            previous = Some(*loan);
        }
        Ok(())
    }

    fn block<'a>(
        &self,
        function: &'a BytecodeFunction,
        id: BytecodeBlockId,
        context: &str,
    ) -> Result<&'a BytecodeBlock, BytecodeVerificationError> {
        function.block(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown block#{}", id.index()),
            )
        })
    }

    fn span(
        &self,
        function: &BytecodeFunction,
        id: BytecodeSpanId,
        context: &str,
    ) -> Result<BytecodeSpan, BytecodeVerificationError> {
        function.span(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown span#{}", id.index()),
            )
        })
    }

    fn verify_function_implementations(&self) -> Result<(), BytecodeVerificationError> {
        let mut implementations = BTreeSet::new();
        for (index, function) in self.program.functions.iter().enumerate() {
            let id = BytecodeFunctionId::new(index as u32);
            let context = format!("function#{index}");
            let callable = self.callable(function.callable, &context)?;
            if callable.implementation != Some(id) || !implementations.insert(function.callable) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "function and callable implementation links are inconsistent",
                ));
            }
        }
        for (index, callable) in self.program.callables.iter().enumerate() {
            if callable.implementation.is_some()
                && !implementations.contains(&BytecodeCallableId::new(index as u32))
            {
                return Err(BytecodeVerificationError::new(
                    format!("callable#{index}"),
                    "callable implementation has no function body",
                ));
            }
        }
        Ok(())
    }

    fn verify_type_ids(
        &self,
        types: &[BytecodeTypeId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        for ty in types {
            self.ty(*ty, context)?;
        }
        Ok(())
    }

    fn ty(
        &self,
        id: BytecodeTypeId,
        context: &str,
    ) -> Result<&BytecodeType, BytecodeVerificationError> {
        self.program.ty(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown type#{}", id.index()),
            )
        })
    }

    fn type_name(&self, id: BytecodeTypeId) -> Result<&str, BytecodeVerificationError> {
        Ok(&self.ty(id, "type ordering")?.name)
    }

    fn nominal(
        &self,
        id: BytecodeNominalId,
        context: &str,
    ) -> Result<&BytecodeNominal, BytecodeVerificationError> {
        self.program
            .nominals
            .get(id.index() as usize)
            .ok_or_else(|| {
                BytecodeVerificationError::new(
                    context,
                    format!("references unknown nominal#{}", id.index()),
                )
            })
    }

    fn callable(
        &self,
        id: BytecodeCallableId,
        context: &str,
    ) -> Result<&BytecodeCallable, BytecodeVerificationError> {
        self.program.callable(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown callable#{}", id.index()),
            )
        })
    }

    fn function(
        &self,
        id: BytecodeFunctionId,
        context: &str,
    ) -> Result<&BytecodeFunction, BytecodeVerificationError> {
        self.program.function(id).ok_or_else(|| {
            BytecodeVerificationError::new(
                context,
                format!("references unknown function#{}", id.index()),
            )
        })
    }
}

fn constant_shape_error(context: &str) -> BytecodeVerificationError {
    BytecodeVerificationError::new(context, "constant value does not match its declared type")
}

fn projection_error(context: &str) -> BytecodeVerificationError {
    BytecodeVerificationError::new(context, "place projection is invalid for its base type")
}

fn rvalue_error(context: &str) -> BytecodeVerificationError {
    BytecodeVerificationError::new(
        context,
        "rvalue operands, shape, or result type are invalid",
    )
}

fn operation_error(context: &str) -> BytecodeVerificationError {
    BytecodeVerificationError::new(
        context,
        "fallible operation operands, association, or result type are invalid",
    )
}

fn terminator_error(context: &str) -> BytecodeVerificationError {
    BytecodeVerificationError::new(context, "terminator edge or block kind is invalid")
}

fn enum_variant<'a>(
    nominal: &'a BytecodeNominal,
    member: u32,
    context: &str,
) -> Result<&'a BytecodeVariant, BytecodeVerificationError> {
    let BytecodeNominalShape::Enum { variants } = &nominal.shape else {
        return Err(projection_error(context));
    };
    variants
        .iter()
        .find(|variant| variant.member == member)
        .ok_or_else(|| projection_error(context))
}

fn scalar_kind(ty: &BytecodeType) -> Option<BytecodeScalarType> {
    match ty.kind {
        BytecodeTypeKind::Scalar(scalar) => Some(scalar),
        _ => None,
    }
}

fn bytecode_type_children(kind: &BytecodeTypeKind) -> Vec<BytecodeTypeId> {
    match kind {
        BytecodeTypeKind::Nominal { arguments, .. }
        | BytecodeTypeKind::Tuple(arguments)
        | BytecodeTypeKind::Union(arguments)
        | BytecodeTypeKind::Intrinsic { arguments, .. }
        | BytecodeTypeKind::Generated { arguments, .. }
        | BytecodeTypeKind::OpaqueResult { arguments, .. } => arguments.clone(),
        BytecodeTypeKind::Function(function) => function
            .parameters
            .iter()
            .map(|parameter| parameter.ty)
            .chain(function.variadic)
            .chain([function.outcome])
            .collect(),
        BytecodeTypeKind::Option(item) => vec![*item],
        BytecodeTypeKind::Result { success, error } => vec![*success, *error],
        BytecodeTypeKind::Cursor { collection, .. } => vec![*collection],
        BytecodeTypeKind::Scalar(_) | BytecodeTypeKind::GenericParameter(_) => Vec::new(),
    }
}

fn integer_value_fits(value: i128, scalar: BytecodeScalarType) -> bool {
    let Some(NumericShape::Integer(shape)) = numeric_shape(scalar) else {
        return false;
    };
    if shape.signed {
        let magnitude = 1_i128 << (shape.bits - 1);
        (-magnitude..=magnitude - 1).contains(&value)
    } else {
        value >= 0 && (value as u128) < (1_u128 << shape.bits)
    }
}

fn float_bits_fit_scalar(bits: u64, scalar: BytecodeScalarType) -> bool {
    match scalar {
        BytecodeScalarType::Float => true,
        BytecodeScalarType::Float32 => {
            let value = f64::from_bits(bits);
            value.is_nan() || f64::from(value as f32).to_bits() == bits
        }
        _ => false,
    }
}

fn is_integer(scalar: BytecodeScalarType) -> bool {
    matches!(
        scalar,
        BytecodeScalarType::Int
            | BytecodeScalarType::Int8
            | BytecodeScalarType::Int16
            | BytecodeScalarType::Int32
            | BytecodeScalarType::UInt8
            | BytecodeScalarType::UInt16
            | BytecodeScalarType::UInt32
            | BytecodeScalarType::UInt64
    )
}

fn is_signed_integer(scalar: BytecodeScalarType) -> bool {
    matches!(
        scalar,
        BytecodeScalarType::Int
            | BytecodeScalarType::Int8
            | BytecodeScalarType::Int16
            | BytecodeScalarType::Int32
    )
}

fn is_float(scalar: BytecodeScalarType) -> bool {
    matches!(
        scalar,
        BytecodeScalarType::Float | BytecodeScalarType::Float32
    )
}

fn is_arithmetic(scalar: BytecodeScalarType) -> bool {
    is_integer(scalar) || is_float(scalar)
}

fn is_relational(scalar: BytecodeScalarType) -> bool {
    is_arithmetic(scalar)
        || matches!(
            scalar,
            BytecodeScalarType::Byte | BytecodeScalarType::Char | BytecodeScalarType::String
        )
}

#[derive(Debug, Clone, Copy)]
enum NumericShape {
    Integer(IntegerShape),
    Float(u8),
}

#[derive(Debug, Clone, Copy)]
struct IntegerShape {
    signed: bool,
    bits: u8,
}

fn classify_numeric_conversion(
    source: BytecodeScalarType,
    target: BytecodeScalarType,
) -> Option<BytecodeNumericConversion> {
    if source == target {
        return numeric_shape(source).map(|_| BytecodeNumericConversion::Identity);
    }
    match (numeric_shape(source)?, numeric_shape(target)?) {
        (NumericShape::Integer(source), NumericShape::Integer(target)) => {
            Some(if integer_range_contains(target, source) {
                BytecodeNumericConversion::Total
            } else {
                BytecodeNumericConversion::Checked
            })
        }
        (NumericShape::Integer(_), NumericShape::Float(_)) => {
            Some(BytecodeNumericConversion::Total)
        }
        (NumericShape::Float(32), NumericShape::Float(64)) => {
            Some(BytecodeNumericConversion::Total)
        }
        (NumericShape::Float(_), NumericShape::Float(_))
        | (NumericShape::Float(_), NumericShape::Integer(_)) => {
            Some(BytecodeNumericConversion::Checked)
        }
    }
}

fn numeric_shape(scalar: BytecodeScalarType) -> Option<NumericShape> {
    Some(match scalar {
        BytecodeScalarType::Byte | BytecodeScalarType::UInt8 => {
            NumericShape::Integer(IntegerShape {
                signed: false,
                bits: 8,
            })
        }
        BytecodeScalarType::UInt16 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 16,
        }),
        BytecodeScalarType::UInt32 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 32,
        }),
        BytecodeScalarType::UInt64 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 64,
        }),
        BytecodeScalarType::Int8 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 8,
        }),
        BytecodeScalarType::Int16 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 16,
        }),
        BytecodeScalarType::Int32 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 32,
        }),
        BytecodeScalarType::Int => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 64,
        }),
        BytecodeScalarType::Float32 => NumericShape::Float(32),
        BytecodeScalarType::Float => NumericShape::Float(64),
        BytecodeScalarType::Bool
        | BytecodeScalarType::Char
        | BytecodeScalarType::String
        | BytecodeScalarType::Unit
        | BytecodeScalarType::Never => return None,
    })
}

fn integer_range_contains(target: IntegerShape, source: IntegerShape) -> bool {
    match (target.signed, source.signed) {
        (true, true) | (false, false) => target.bits >= source.bits,
        (true, false) => target.bits > source.bits,
        (false, true) => false,
    }
}

fn operand_place<'a>(
    function: &'a BytecodeFunction,
    operand: &'a BytecodeOperand,
) -> Option<&'a BytecodePlace> {
    match &operand.kind {
        BytecodeOperandKind::Copy(place)
        | BytecodeOperandKind::Move(place)
        | BytecodeOperandKind::Borrow(place) => Some(place),
        BytecodeOperandKind::Loan(loan) => function
            .loans
            .get(loan.index() as usize)
            .map(|loan| &loan.place),
        BytecodeOperandKind::Constant(_) | BytecodeOperandKind::Function { .. } => None,
    }
}

fn operand_is_borrow(operand: &BytecodeOperand) -> bool {
    matches!(operand.kind, BytecodeOperandKind::Borrow(_))
}

fn operand_is_loan(operand: &BytecodeOperand) -> bool {
    matches!(operand.kind, BytecodeOperandKind::Loan(_))
}

fn rvalue_contains_invalid_borrow(value: &BytecodeRvalue) -> bool {
    let escapes =
        |operand: &BytecodeOperand| operand_is_borrow(operand) || operand_is_loan(operand);
    match &value.kind {
        BytecodeRvalueKind::Use(value)
        | BytecodeRvalueKind::Prefix { operand: value, .. }
        | BytecodeRvalueKind::Coerce { value, .. }
        | BytecodeRvalueKind::NumericConversion { value, .. } => escapes(value),
        BytecodeRvalueKind::Binary {
            left,
            right,
            operator: BytecodeBinaryOperator::Equal | BytecodeBinaryOperator::NotEqual,
        } => operand_is_loan(left) || operand_is_loan(right),
        BytecodeRvalueKind::Contains {
            item, container, ..
        } => operand_is_loan(item) || operand_is_loan(container),
        BytecodeRvalueKind::MapRemove { key, .. } => escapes(key),
        BytecodeRvalueKind::Interpolate { values, .. } => values.iter().any(escapes),
        BytecodeRvalueKind::Length(operand) | BytecodeRvalueKind::IteratorState(operand) => {
            operand_is_loan(operand)
        }
        BytecodeRvalueKind::Binary { left, right, .. }
        | BytecodeRvalueKind::Range {
            start: left,
            end: right,
            ..
        } => escapes(left) || escapes(right),
        BytecodeRvalueKind::Construct { values, .. } => values.iter().any(escapes),
        BytecodeRvalueKind::RecordUpdate { base, fields } => {
            escapes(base) || fields.iter().any(|(_, value)| escapes(value))
        }
    }
}

fn operation_contains_invalid_borrow(operation: &BytecodeOperation) -> bool {
    let escapes =
        |operand: &BytecodeOperand| operand_is_borrow(operand) || operand_is_loan(operand);
    match &operation.kind {
        BytecodeOperationKind::CheckedPrefix { operand, .. }
        | BytecodeOperationKind::ExplicitPanic { message: operand } => escapes(operand),
        BytecodeOperationKind::CheckedBinary { left, right, .. } => escapes(left) || escapes(right),
        BytecodeOperationKind::ArraySequence {
            array, argument, ..
        } => operand_is_loan(array) || escapes(argument),
        BytecodeOperationKind::BuildMap { entries, .. } => entries
            .iter()
            .any(|(key, value)| escapes(key) || escapes(value)),
        BytecodeOperationKind::Index { base, index, .. } => operand_is_loan(base) || escapes(index),
        BytecodeOperationKind::Slice { base, bounds, .. } => {
            operand_is_loan(base)
                || bounds
                    .start
                    .iter()
                    .chain(&bounds.end)
                    .chain(&bounds.step)
                    .any(escapes)
        }
        BytecodeOperationKind::Call {
            callee, arguments, ..
        } => {
            operand_is_loan(callee)
                || arguments.iter().any(|argument| {
                    if argument.mode == BytecodeParameterMode::Value {
                        escapes(&argument.value)
                    } else {
                        !operand_is_loan(&argument.value)
                    }
                })
        }
        BytecodeOperationKind::Display { argument } => {
            argument.mode != BytecodeParameterMode::Ref || !operand_is_loan(&argument.value)
        }
        BytecodeOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => escapes(condition) || message_parts.iter().any(|part| escapes(&part.value)),
        BytecodeOperationKind::BootstrapHostCall { arguments, .. } => arguments.iter().any(escapes),
    }
}

fn operation_operands(operation: &BytecodeOperation) -> Vec<&BytecodeOperand> {
    let mut operands = Vec::new();
    match &operation.kind {
        BytecodeOperationKind::CheckedPrefix { operand, .. }
        | BytecodeOperationKind::ExplicitPanic { message: operand } => operands.push(operand),
        BytecodeOperationKind::CheckedBinary { left, right, .. }
        | BytecodeOperationKind::ArraySequence {
            array: left,
            argument: right,
            ..
        }
        | BytecodeOperationKind::Index {
            base: left,
            index: right,
            ..
        } => {
            operands.push(left);
            operands.push(right);
        }
        BytecodeOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                operands.push(key);
                operands.push(value);
            }
        }
        BytecodeOperationKind::Slice { base, bounds, .. } => {
            operands.push(base);
            operands.extend(bounds.start.iter().chain(&bounds.end).chain(&bounds.step));
        }
        BytecodeOperationKind::Call {
            callee, arguments, ..
        } => {
            operands.push(callee);
            operands.extend(arguments.iter().map(|argument| &argument.value));
        }
        BytecodeOperationKind::Display { argument } => operands.push(&argument.value),
        BytecodeOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            operands.push(condition);
            operands.extend(message_parts.iter().map(|part| &part.value));
        }
        BytecodeOperationKind::BootstrapHostCall { arguments, .. } => {
            operands.extend(arguments);
        }
    }
    operands
}

fn closure_capture_place(
    function: &BytecodeFunction,
    callable: BytecodeCallableId,
    place: &BytecodePlace,
) -> bool {
    function.parameters.first() == Some(&place.slot)
        && matches!(
            place.projections.first().map(|projection| &projection.kind),
            Some(BytecodeProjectionKind::ClosureCapture {
                callable: projected,
                ..
            }) if *projected == callable
        )
}

fn closure_capture_access(
    function: &BytecodeFunction,
    callable: BytecodeCallableId,
    access: &LocalAccess,
) -> bool {
    closure_capture_access_index(function, callable, access).is_some()
}

fn closure_capture_access_index(
    function: &BytecodeFunction,
    callable: BytecodeCallableId,
    access: &LocalAccess,
) -> Option<u32> {
    (function.parameters.first() == Some(&access.slot))
        .then(|| match access.path.first() {
            Some(MovePathComponent::ClosureCapture(projected, index)) if *projected == callable => {
                Some(*index)
            }
            _ => None,
        })
        .flatten()
}

fn closure_capture_transfer_index(
    function: &BytecodeFunction,
    callable: BytecodeCallableId,
    access: &LocalAccess,
) -> Option<u32> {
    let index = closure_capture_access_index(function, callable, access)?;
    access.path[1..]
        .iter()
        .all(|component| matches!(component, MovePathComponent::NewtypeValue))
        .then_some(index)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalState {
    live: bool,
    unavailable: BTreeSet<Vec<MovePathComponent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TagFact {
    place: BytecodePlace,
    tag: BytecodeTag,
}

#[derive(Debug, Clone)]
enum TagEvent {
    Require(TagFact),
    Write(BytecodePlace),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEvent {
    Read(LocalAccess),
    Resolve(LocalAccess),
    Move(LocalAccess),
    Write(LocalAccess),
    WriteAccess(LocalAccess),
    StorageLive(BytecodeSlotId),
    StorageDead(BytecodeSlotId),
}

#[derive(Debug, Clone, Copy)]
struct ClassifiedLocalAccess<'a> {
    access: &'a LocalAccess,
    kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoanEvent {
    Local(LocalEvent),
    Reserve(BytecodeLoanId),
    Release(BytecodeLoanId),
    Consume(Vec<BytecodeLoanId>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct LoanFlowState {
    active: BTreeSet<BytecodeLoanId>,
    validated: BTreeMap<BytecodeLoanId, Vec<BytecodeLoanId>>,
    accesses: BTreeMap<ValidatedAccess, Vec<BytecodeLoanId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ValidatedAccess {
    access: LocalAccess,
    for_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LocalAccess {
    slot: BytecodeSlotId,
    path: Vec<MovePathComponent>,
    source_loan: Option<BytecodeLoanId>,
}

impl LocalAccess {
    fn from_place(place: &BytecodePlace) -> Self {
        Self {
            slot: place.slot,
            path: place
                .projections
                .iter()
                .map(MovePathComponent::from_projection)
                .collect(),
            source_loan: place.source_loan,
        }
    }
}

fn is_complete_defer_owner_place(place: &BytecodePlace) -> bool {
    place.source_loan.is_none()
        && (place.projections.is_empty()
            || matches!(
                place.projections.as_slice(),
                [BytecodeProjection {
                    kind: BytecodeProjectionKind::ClosureCapture { .. },
                    ..
                }]
            ))
}

fn is_iterator_defer_target(place: &BytecodePlace) -> bool {
    place.source_loan.is_none()
        && matches!(
            place.projections.as_slice(),
            [BytecodeProjection {
                kind: BytecodeProjectionKind::IteratorSource,
                ..
            }]
        )
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct DeferFlowState {
    unguarded_scopes: BTreeSet<BytecodeScopeId>,
    guards: BTreeMap<LocalAccess, ActiveDeferGuard>,
    registrations: BTreeMap<(BytecodeBlockId, usize), ActiveCleanupRegistration>,
    scope_order: Vec<BytecodeScopeId>,
    pending_moves: BTreeMap<LocalAccess, PendingDeferTransition>,
    consumed: BTreeSet<LocalAccess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveDeferGuard {
    scope: BytecodeScopeId,
    registration: (BytecodeBlockId, usize),
    kind: CleanupEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveCleanupRegistration {
    scope: BytecodeScopeId,
    kind: CleanupEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CleanupEntryKind {
    Explicit,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PendingDeferTransition {
    Retarget,
    Disarm,
}

impl DeferFlowState {
    fn scope_is_active(&self, scope: BytecodeScopeId) -> bool {
        self.unguarded_scopes.contains(&scope)
            || self.guards.values().any(|candidate| {
                candidate.kind == CleanupEntryKind::Explicit && candidate.scope == scope
            })
    }

    fn activate_scope(
        &mut self,
        scope: BytecodeScopeId,
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        if self.scope_is_active(scope) {
            if self.scope_order.last() != Some(&scope) {
                return Err(BytecodeVerificationError::new(
                    context,
                    "defer registration re-enters an outer scope beneath active inner entries",
                ));
            }
        } else {
            self.scope_order.push(scope);
        }
        Ok(())
    }

    fn remove_inactive_scope(&mut self, scope: BytecodeScopeId) {
        if !self.scope_is_active(scope) {
            self.scope_order.retain(|candidate| *candidate != scope);
        }
    }

    fn drain(
        &mut self,
        scopes: &[BytecodeScopeId],
        context: &str,
    ) -> Result<(), BytecodeVerificationError> {
        let selected = scopes.iter().copied().collect::<BTreeSet<_>>();
        if let Some(first) = self
            .scope_order
            .iter()
            .position(|scope| selected.contains(scope))
            && self.scope_order[first..]
                .iter()
                .any(|scope| !selected.contains(scope))
        {
            return Err(BytecodeVerificationError::new(
                context,
                "defer drain skips a still-active inner scope",
            ));
        }
        self.consumed
            .extend(self.guards.iter().filter_map(|(place, guard)| {
                (guard.kind == CleanupEntryKind::Explicit && selected.contains(&guard.scope))
                    .then_some(place.clone())
            }));
        self.unguarded_scopes
            .retain(|scope| !selected.contains(scope));
        self.guards.retain(|_, guard| {
            guard.kind != CleanupEntryKind::Explicit || !selected.contains(&guard.scope)
        });
        self.registrations.retain(|_, registration| {
            registration.kind != CleanupEntryKind::Explicit
                || !selected.contains(&registration.scope)
        });
        self.scope_order.retain(|scope| !selected.contains(scope));
        Ok(())
    }

    fn drain_unwind(&mut self) {
        self.consumed.extend(self.guards.keys().cloned());
        self.unguarded_scopes.clear();
        self.guards.clear();
        self.registrations.clear();
        self.scope_order.clear();
        self.pending_moves.clear();
    }

    fn finish_normal(&mut self, context: &str) -> Result<(), BytecodeVerificationError> {
        let has_explicit = !self.unguarded_scopes.is_empty()
            || !self.scope_order.is_empty()
            || self
                .guards
                .values()
                .any(|guard| guard.kind == CleanupEntryKind::Explicit)
            || self
                .registrations
                .values()
                .any(|registration| registration.kind == CleanupEntryKind::Explicit)
            || !self.pending_moves.is_empty();
        if has_explicit {
            return Err(BytecodeVerificationError::new(
                context,
                "normal return abandons an explicit defer entry",
            ));
        }
        self.guards.clear();
        self.registrations.clear();
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.unguarded_scopes.is_empty()
            && self.guards.is_empty()
            && self.registrations.is_empty()
            && self.scope_order.is_empty()
            && self.pending_moves.is_empty()
    }
}

fn defer_assignment_transition(
    function: &BytecodeFunction,
    block: &BytecodeBlock,
    destination: &BytecodePlace,
    value: &BytecodeRvalue,
    guard: &LocalAccess,
    scope: BytecodeScopeId,
) -> Option<PendingDeferTransition> {
    if assignment_directly_moves(value, guard) {
        return Some(PendingDeferTransition::Retarget);
    }

    let exits_scope = block_exits_defer_scope(function, block, scope);
    let return_root =
        destination.slot == function.return_slot && destination.projections.is_empty();
    let confirmed_handoff = matches!(&value.kind, BytecodeRvalueKind::Coerce { .. })
        || (return_root
            && matches!(
                &value.kind,
                BytecodeRvalueKind::Construct {
                    shape: BytecodeAggregateKind::ResultOk | BytecodeAggregateKind::ResultErr,
                    ..
                }
            ));
    (exits_scope && confirmed_handoff).then_some(PendingDeferTransition::Disarm)
}

fn assignment_directly_moves(value: &BytecodeRvalue, guard: &LocalAccess) -> bool {
    match &value.kind {
        BytecodeRvalueKind::Use(BytecodeOperand {
            kind: BytecodeOperandKind::Move(place),
            ..
        })
        | BytecodeRvalueKind::IteratorState(BytecodeOperand {
            kind: BytecodeOperandKind::Move(place),
            ..
        }) => LocalAccess::from_place(place) == *guard,
        _ => false,
    }
}

fn store_cleanup_transfer(
    destination: &BytecodePlace,
    value: &BytecodeRvalue,
) -> Option<(BytecodePlace, BytecodePlace)> {
    let (from, to) = match &value.kind {
        BytecodeRvalueKind::Use(BytecodeOperand {
            kind: BytecodeOperandKind::Move(from),
            ..
        }) => (from.clone(), destination.clone()),
        BytecodeRvalueKind::IteratorState(BytecodeOperand {
            kind: BytecodeOperandKind::Move(from),
            ..
        }) => {
            let mut to = destination.clone();
            to.ty = from.ty;
            to.projections.push(BytecodeProjection {
                ty: from.ty,
                kind: BytecodeProjectionKind::IteratorSource,
            });
            (from.clone(), to)
        }
        _ => return None,
    };
    (is_complete_defer_owner_place(&from)
        && (is_complete_defer_owner_place(&to) || is_iterator_defer_target(&to)))
    .then_some((from, to))
}

fn defer_disarm_is_confirmed(
    function: &BytecodeFunction,
    block: &BytecodeBlock,
    instruction: usize,
    place: &LocalAccess,
    scope: BytecodeScopeId,
    pending: Option<PendingDeferTransition>,
) -> bool {
    if block.instructions[instruction + 1..]
        .iter()
        .any(|instruction| {
            !matches!(
                instruction.kind,
                BytecodeInstructionKind::ReleaseLoan(_) | BytecodeInstructionKind::DisarmCleanup(_)
            )
        })
    {
        return false;
    }
    match pending {
        Some(PendingDeferTransition::Retarget) => false,
        Some(PendingDeferTransition::Disarm) => block_exits_defer_scope(function, block, scope),
        None => {
            terminator_moves_defer_guard(&block.terminator.kind, place)
                || block_exits_defer_scope(function, block, scope)
        }
    }
}

fn terminator_moves_defer_guard(terminator: &BytecodeTerminatorKind, guard: &LocalAccess) -> bool {
    let BytecodeTerminatorKind::Invoke { operation, .. } = terminator else {
        return false;
    };
    operation_operands(operation).into_iter().any(|operand| {
        matches!(
            &operand.kind,
            BytecodeOperandKind::Move(place) if LocalAccess::from_place(place) == *guard
        )
    })
}

fn block_exits_defer_scope(
    function: &BytecodeFunction,
    block: &BytecodeBlock,
    scope: BytecodeScopeId,
) -> bool {
    let drains = |terminator: &BytecodeTerminatorKind| {
        matches!(
            terminator,
            BytecodeTerminatorKind::DrainDefers { scopes, .. } if scopes.contains(&scope)
        )
    };
    if drains(&block.terminator.kind) {
        return true;
    }
    let BytecodeTerminatorKind::Goto { target } = &block.terminator.kind else {
        return false;
    };
    function
        .blocks
        .get(target.index() as usize)
        .is_some_and(|target| drains(&target.terminator.kind))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MovePathComponent {
    ClosureCapture(BytecodeCallableId, u32),
    Field(u32),
    TupleField(u32),
    NewtypeValue,
    RefValue,
    VariantTuple(u32, u32),
    VariantField(u32, u32),
    OptionValue,
    ResultOkValue,
    ResultErrValue,
    UnionValue(BytecodeTypeId),
    ArrayPatternIndex(u32),
    ArrayPatternRest {
        start: u32,
        suffix: u32,
    },
    IteratorElement {
        index: BytecodeSlotId,
    },
    IteratorSource,
    Index {
        index: BytecodeSlotId,
        access: BytecodeIndexAccess,
    },
    Slice {
        start: Option<BytecodeSlotId>,
        end: Option<BytecodeSlotId>,
        step: Option<BytecodeSlotId>,
    },
}

impl MovePathComponent {
    fn from_projection(projection: &BytecodeProjection) -> Self {
        match &projection.kind {
            BytecodeProjectionKind::ClosureCapture { callable, index } => {
                Self::ClosureCapture(*callable, *index)
            }
            BytecodeProjectionKind::Field(field) => Self::Field(*field),
            BytecodeProjectionKind::TupleField(index) => Self::TupleField(*index),
            BytecodeProjectionKind::NewtypeValue => Self::NewtypeValue,
            BytecodeProjectionKind::RefValue => Self::RefValue,
            BytecodeProjectionKind::VariantTuple { variant, index } => {
                Self::VariantTuple(*variant, *index)
            }
            BytecodeProjectionKind::VariantField { variant, field } => {
                Self::VariantField(*variant, *field)
            }
            BytecodeProjectionKind::OptionValue => Self::OptionValue,
            BytecodeProjectionKind::ResultOkValue => Self::ResultOkValue,
            BytecodeProjectionKind::ResultErrValue => Self::ResultErrValue,
            BytecodeProjectionKind::UnionValue(member) => Self::UnionValue(*member),
            BytecodeProjectionKind::ArrayPatternIndex(index) => Self::ArrayPatternIndex(*index),
            BytecodeProjectionKind::ArrayPatternRest { start, suffix } => Self::ArrayPatternRest {
                start: *start,
                suffix: *suffix,
            },
            BytecodeProjectionKind::IteratorElement { index } => {
                Self::IteratorElement { index: *index }
            }
            BytecodeProjectionKind::IteratorSource => Self::IteratorSource,
            BytecodeProjectionKind::Index { index, access } => Self::Index {
                index: *index,
                access: *access,
            },
            BytecodeProjectionKind::Slice { start, end, step } => Self::Slice {
                start: *start,
                end: *end,
                step: *step,
            },
        }
    }
}

fn operation_access_place<'a>(
    operation: &'a BytecodeOperation,
    context: &str,
) -> Result<Option<(BytecodePlace, &'a [BytecodeLoanId])>, BytecodeVerificationError> {
    let (base, projection, against) = match &operation.kind {
        BytecodeOperationKind::Index {
            base,
            index,
            access,
            against,
        } => (
            base,
            BytecodeProjectionKind::Index {
                index: operand_materialized_slot(index, context)?,
                access: *access,
            },
            against.as_slice(),
        ),
        BytecodeOperationKind::Slice {
            base,
            bounds,
            against,
        } => (
            base,
            BytecodeProjectionKind::Slice {
                start: bounds
                    .start
                    .as_ref()
                    .map(|operand| operand_materialized_slot(operand, context))
                    .transpose()?,
                end: bounds
                    .end
                    .as_ref()
                    .map(|operand| operand_materialized_slot(operand, context))
                    .transpose()?,
                step: bounds
                    .step
                    .as_ref()
                    .map(|operand| operand_materialized_slot(operand, context))
                    .transpose()?,
            },
            against.as_slice(),
        ),
        _ => return Ok(None),
    };
    let BytecodeOperandKind::Borrow(base) = &base.kind else {
        return Err(BytecodeVerificationError::new(
            context,
            "indexed operation has no borrowed base place",
        ));
    };
    let mut place = base.clone();
    place.ty = operation.ty;
    place.projections.push(BytecodeProjection {
        ty: operation.ty,
        kind: projection,
    });
    Ok(Some((place, against)))
}

fn operand_materialized_slot(
    operand: &BytecodeOperand,
    context: &str,
) -> Result<BytecodeSlotId, BytecodeVerificationError> {
    match &operand.kind {
        BytecodeOperandKind::Copy(place)
        | BytecodeOperandKind::Move(place)
        | BytecodeOperandKind::Borrow(place)
            if place.projections.is_empty() && place.source_loan.is_none() =>
        {
            Ok(place.slot)
        }
        _ => Err(BytecodeVerificationError::new(
            context,
            "index or slice input is not a materialized slot",
        )),
    }
}

fn bytecode_loan_events(function: &BytecodeFunction, block: &BytecodeBlock) -> Vec<LoanEvent> {
    let mut events = Vec::new();
    for instruction in &block.instructions {
        match &instruction.kind {
            BytecodeInstructionKind::StorageLive(slot) => {
                events.push(LoanEvent::Local(LocalEvent::StorageLive(*slot)));
            }
            BytecodeInstructionKind::StorageDead(slot) => {
                events.push(LoanEvent::Local(LocalEvent::StorageDead(*slot)));
            }
            BytecodeInstructionKind::ReserveLoan(id) => {
                if let Some(loan) = function.loans.get(id.index() as usize) {
                    let mut local = Vec::new();
                    if place_requires_loan_validation(&loan.place) {
                        push_resolve_place_events(&loan.place, &mut local);
                    } else {
                        push_place_events(&loan.place, true, &mut local);
                    }
                    events.extend(local.into_iter().map(LoanEvent::Local));
                }
                events.push(LoanEvent::Reserve(*id));
            }
            BytecodeInstructionKind::ReleaseLoan(id) => {
                events.push(LoanEvent::Release(*id));
            }
            BytecodeInstructionKind::Store { destination, value } => {
                let mut local = Vec::new();
                push_rvalue_events(value, &mut local);
                push_destination_events(destination, &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            BytecodeInstructionKind::RegisterDefer { action, guard, .. } => {
                let mut local = Vec::new();
                push_defer_operation_events(action, guard.as_ref(), &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            BytecodeInstructionKind::RegisterFallback { owner, .. } => {
                let mut local = Vec::new();
                push_place_events(owner, true, &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            BytecodeInstructionKind::EnterTaskScope { .. }
            | BytecodeInstructionKind::RetargetCleanup { .. }
            | BytecodeInstructionKind::DisarmCleanup(_) => {}
        }
    }
    let mut local = Vec::new();
    match &block.terminator.kind {
        BytecodeTerminatorKind::Goto { .. }
        | BytecodeTerminatorKind::DrainDefers { .. }
        | BytecodeTerminatorKind::DrainScopes { .. }
        | BytecodeTerminatorKind::DrainUnwind { .. }
        | BytecodeTerminatorKind::ResumePanic
        | BytecodeTerminatorKind::Unreachable => {}
        BytecodeTerminatorKind::BranchBool { condition, .. } => {
            push_operand_events(condition, &mut local);
        }
        BytecodeTerminatorKind::BranchTag { value, .. } => {
            push_operand_events(value, &mut local);
        }
        BytecodeTerminatorKind::Invoke { operation, .. } => {
            if let Some((place, _)) = operation_access_place(operation, "loan events")
                .expect("verified indexed operations retain materialized places")
            {
                push_resolve_place_events(&place, &mut local);
            } else {
                push_operation_events(operation, &mut local);
            }
        }
        BytecodeTerminatorKind::Await { awaitable, .. } => match awaitable {
            BytecodeAwaitable::Call(operation) => push_operation_events(operation, &mut local),
            BytecodeAwaitable::Join(join) => push_operand_events(join, &mut local),
        },
        BytecodeTerminatorKind::Spawn { operation, .. } => {
            push_operation_events(operation, &mut local);
        }
        BytecodeTerminatorKind::IteratorNext {
            state,
            borrowed_source,
            ..
        } => {
            push_destination_reads(state, true, &mut local);
            if let Some(source) = borrowed_source {
                push_place_events(source, true, &mut local);
            }
        }
        BytecodeTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                push_resolve_place_events(place, &mut local);
            }
            for replacement in replacements.iter().flatten() {
                push_operand_events(replacement, &mut local);
            }
        }
        BytecodeTerminatorKind::ValidateLoan { loan, .. } => {
            if let Some(loan) = function.loans.get(loan.index() as usize) {
                push_resolve_place_events(&loan.place, &mut local);
            }
        }
        BytecodeTerminatorKind::Return => local.push(LocalEvent::Read(LocalAccess {
            slot: function.return_slot,
            path: Vec::new(),
            source_loan: None,
        })),
    }
    events.extend(local.into_iter().map(LoanEvent::Local));
    let operation = match &block.terminator.kind {
        BytecodeTerminatorKind::Invoke { operation, .. }
        | BytecodeTerminatorKind::Spawn { operation, .. } => Some(operation),
        BytecodeTerminatorKind::Await {
            awaitable: BytecodeAwaitable::Call(operation),
            ..
        } => Some(operation),
        _ => None,
    };
    if let Some(operation) = operation {
        let consumed = match &operation.kind {
            BytecodeOperationKind::Call { arguments, .. } => arguments
                .iter()
                .filter_map(|argument| match &argument.value.kind {
                    BytecodeOperandKind::Loan(loan) => Some(*loan),
                    _ => None,
                })
                .collect(),
            BytecodeOperationKind::Display { argument } => match argument.value.kind {
                BytecodeOperandKind::Loan(loan) => vec![loan],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        if !consumed.is_empty() {
            events.push(LoanEvent::Consume(consumed));
        }
    }
    events
}

#[derive(Debug, Clone)]
struct SuccessorEdge {
    target: BytecodeBlockId,
    refinement: Option<TagFact>,
    writes: Option<BytecodePlace>,
}

fn successor_edges(terminator: &BytecodeTerminatorKind) -> Vec<SuccessorEdge> {
    let edge = |target| SuccessorEdge {
        target,
        refinement: None,
        writes: None,
    };
    match terminator {
        BytecodeTerminatorKind::Goto { target } => vec![edge(*target)],
        BytecodeTerminatorKind::BranchBool {
            if_true, if_false, ..
        } => vec![edge(*if_true), edge(*if_false)],
        BytecodeTerminatorKind::BranchTag {
            value,
            cases,
            otherwise,
        } => {
            let place = match &value.kind {
                BytecodeOperandKind::Copy(place)
                | BytecodeOperandKind::Move(place)
                | BytecodeOperandKind::Borrow(place) => Some(place.clone()),
                BytecodeOperandKind::Constant(_)
                | BytecodeOperandKind::Function { .. }
                | BytecodeOperandKind::Loan(_) => None,
            };
            cases
                .iter()
                .map(|(tag, target)| SuccessorEdge {
                    target: *target,
                    refinement: place.clone().map(|place| TagFact { place, tag: *tag }),
                    writes: None,
                })
                .chain(std::iter::once(SuccessorEdge {
                    target: *otherwise,
                    refinement: (cases.len() == 1)
                        .then(|| complementary_tag(cases[0].0))
                        .flatten()
                        .and_then(|tag| place.clone().map(|place| TagFact { place, tag })),
                    writes: None,
                }))
                .collect()
        }
        BytecodeTerminatorKind::Invoke {
            destination,
            target,
            unwind,
            ..
        } => target
            .iter()
            .map(|target| SuccessorEdge {
                target: *target,
                refinement: None,
                writes: destination.clone(),
            })
            .chain(std::iter::once(edge(*unwind)))
            .collect(),
        BytecodeTerminatorKind::Await {
            destination,
            target,
            unwind,
            ..
        }
        | BytecodeTerminatorKind::Spawn {
            destination,
            target,
            unwind,
            ..
        } => vec![
            SuccessorEdge {
                target: *target,
                refinement: None,
                writes: Some(destination.clone()),
            },
            edge(*unwind),
        ],
        BytecodeTerminatorKind::IteratorNext {
            destination,
            has_value,
            exhausted,
            unwind,
            ..
        } => vec![
            SuccessorEdge {
                target: *has_value,
                refinement: None,
                writes: Some(destination.clone()),
            },
            edge(*exhausted),
            edge(*unwind),
        ],
        BytecodeTerminatorKind::ValidatePlaces { target, unwind, .. }
        | BytecodeTerminatorKind::ValidateLoan { target, unwind, .. }
        | BytecodeTerminatorKind::DrainDefers { target, unwind, .. }
        | BytecodeTerminatorKind::DrainScopes { target, unwind, .. } => {
            vec![edge(*target), edge(*unwind)]
        }
        BytecodeTerminatorKind::DrainUnwind { target } => vec![edge(*target)],
        BytecodeTerminatorKind::Return
        | BytecodeTerminatorKind::ResumePanic
        | BytecodeTerminatorKind::Unreachable => Vec::new(),
    }
}

fn intersect_optional_capture_set(target: &mut Option<BTreeSet<u32>>, source: BTreeSet<u32>) {
    let _ = intersect_incoming_capture_set(target, source);
}

fn intersect_incoming_capture_set(
    target: &mut Option<BTreeSet<u32>>,
    source: BTreeSet<u32>,
) -> bool {
    let Some(target) = target else {
        *target = Some(source);
        return true;
    };
    let previous = target.len();
    target.retain(|value| source.contains(value));
    target.len() != previous
}

fn transfer_local(state: LocalState, events: &[LocalEvent], slot: BytecodeSlotId) -> LocalState {
    let mut state = state;
    for event in events {
        match event {
            LocalEvent::Write(access) if access.slot == slot => {
                if state.live {
                    write_path_unchecked(&mut state.unavailable, &access.path);
                }
            }
            LocalEvent::Move(access) if access.slot == slot => {
                if state.live {
                    move_path_unchecked(&mut state.unavailable, access.path.clone());
                }
            }
            LocalEvent::StorageLive(event_slot) if *event_slot == slot => {
                state.live = true;
                state.unavailable.clear();
                state.unavailable.insert(Vec::new());
            }
            LocalEvent::StorageDead(event_slot) if *event_slot == slot => {
                state.live = false;
                state.unavailable.clear();
                state.unavailable.insert(Vec::new());
            }
            LocalEvent::Read(_)
            | LocalEvent::Resolve(_)
            | LocalEvent::Move(_)
            | LocalEvent::Write(_)
            | LocalEvent::WriteAccess(_)
            | LocalEvent::StorageLive(_)
            | LocalEvent::StorageDead(_) => {}
        }
    }
    state
}

fn path_is_available(
    unavailable: &BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) -> bool {
    unavailable
        .iter()
        .all(|moved| !move_paths_overlap(moved, path))
}

fn path_parent_is_available(
    unavailable: &BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) -> bool {
    unavailable
        .iter()
        .all(|moved| !(moved.len() < path.len() && move_path_is_prefix(moved, path)))
}

fn move_path_unchecked(
    unavailable: &mut BTreeSet<Vec<MovePathComponent>>,
    path: Vec<MovePathComponent>,
) {
    if path.is_empty() {
        unavailable.clear();
    } else if unavailable
        .iter()
        .any(|moved| move_path_is_prefix(moved, &path))
    {
        return;
    } else {
        unavailable.retain(|moved| !move_path_is_prefix(&path, moved));
    }
    unavailable.insert(path);
}

fn write_path_unchecked(
    unavailable: &mut BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) {
    unavailable.retain(|moved| !move_path_is_prefix(path, moved));
}

fn move_paths_overlap(left: &[MovePathComponent], right: &[MovePathComponent]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| !move_path_components_are_disjoint(left, right))
}

fn local_accesses_overlap(left: &LocalAccess, right: &LocalAccess) -> bool {
    left.slot == right.slot && move_paths_overlap(&left.path, &right.path)
}

fn local_access_contains(outer: &LocalAccess, inner: &LocalAccess) -> bool {
    outer.slot == inner.slot
        && outer.source_loan == inner.source_loan
        && outer.path.len() <= inner.path.len()
        && outer
            .path
            .iter()
            .zip(&inner.path)
            .all(|(left, right)| left == right)
}

fn apply_consumed_defer_events(
    state: &mut DeferFlowState,
    events: &[LocalEvent],
    context: &str,
) -> Result<(), BytecodeVerificationError> {
    for event in events {
        match event {
            LocalEvent::Read(access) | LocalEvent::Resolve(access) | LocalEvent::Move(access) => {
                if state
                    .consumed
                    .iter()
                    .any(|consumed| local_accesses_overlap(consumed, access))
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "accesses an owner already consumed by a deferred action",
                    ));
                }
            }
            LocalEvent::WriteAccess(access) => {
                if state.consumed.iter().any(|consumed| {
                    local_accesses_overlap(consumed, access)
                        && !local_access_contains(access, consumed)
                }) {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "resolves a partial write through an owner consumed by a deferred action",
                    ));
                }
            }
            LocalEvent::Write(access) => {
                let overlapping = state
                    .consumed
                    .iter()
                    .filter(|consumed| local_accesses_overlap(consumed, access))
                    .cloned()
                    .collect::<Vec<_>>();
                if overlapping
                    .iter()
                    .any(|consumed| !local_access_contains(access, consumed))
                {
                    return Err(BytecodeVerificationError::new(
                        context,
                        "partially reinitializes an owner consumed by a deferred action",
                    ));
                }
                state
                    .consumed
                    .retain(|consumed| !local_access_contains(access, consumed));
            }
            LocalEvent::StorageLive(slot) | LocalEvent::StorageDead(slot) => {
                state.consumed.retain(|consumed| consumed.slot != *slot);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn loan_paths_overlap(
    left: &[MovePathComponent],
    right: &[MovePathComponent],
    static_integers: &BTreeMap<BytecodeSlotId, u64>,
) -> bool {
    loan_paths_relation(left, right, static_integers) != StaticRegionRelation::Disjoint
}

fn loan_place_relation(
    left: &BytecodePlace,
    right: &BytecodePlace,
    static_integers: &BTreeMap<BytecodeSlotId, u64>,
) -> StaticRegionRelation {
    if left.slot != right.slot {
        return StaticRegionRelation::Disjoint;
    }
    let left = left
        .projections
        .iter()
        .map(MovePathComponent::from_projection)
        .collect::<Vec<_>>();
    let right = right
        .projections
        .iter()
        .map(MovePathComponent::from_projection)
        .collect::<Vec<_>>();
    loan_paths_relation(&left, &right, static_integers)
}

fn loan_paths_relation(
    left: &[MovePathComponent],
    right: &[MovePathComponent],
    static_integers: &BTreeMap<BytecodeSlotId, u64>,
) -> StaticRegionRelation {
    let mut relation = StaticRegionRelation::Overlap;
    for (left, right) in left.iter().zip(right) {
        if matches!(
            (left, right),
            (
                MovePathComponent::IteratorElement { index: left },
                MovePathComponent::IteratorElement { index: right }
            ) if left == right
        ) {
            continue;
        }
        match (
            collection_region(left, static_integers),
            collection_region(right, static_integers),
        ) {
            (CollectionComponent::Static(left), CollectionComponent::Static(right)) => {
                let current = static_collection_relation(left, right);
                if current == StaticRegionRelation::Disjoint {
                    return current;
                }
                if static_regions_are_identical(left, right) {
                    relation = current;
                    continue;
                }
                return current;
            }
            (CollectionComponent::None, CollectionComponent::None) => {
                if left == right {
                    continue;
                }
                if move_path_components_are_disjoint(left, right) {
                    return StaticRegionRelation::Disjoint;
                }
                return StaticRegionRelation::Overlap;
            }
            (CollectionComponent::Dynamic, _)
            | (_, CollectionComponent::Dynamic)
            | (CollectionComponent::Static(_), CollectionComponent::None)
            | (CollectionComponent::None, CollectionComponent::Static(_)) => {
                return StaticRegionRelation::Runtime;
            }
        }
    }
    relation
}

fn move_path_runtime_inputs(
    path: &[MovePathComponent],
) -> impl Iterator<Item = BytecodeSlotId> + '_ {
    path.iter().flat_map(|component| {
        let inputs = match component {
            MovePathComponent::Index { index, .. } => [Some(*index), None, None],
            MovePathComponent::IteratorElement { index } => [Some(*index), None, None],
            MovePathComponent::Slice { start, end, step } => [*start, *end, *step],
            MovePathComponent::ClosureCapture(_, _)
            | MovePathComponent::IteratorSource
            | MovePathComponent::Field(_)
            | MovePathComponent::TupleField(_)
            | MovePathComponent::NewtypeValue
            | MovePathComponent::RefValue
            | MovePathComponent::VariantTuple(_, _)
            | MovePathComponent::VariantField(_, _)
            | MovePathComponent::OptionValue
            | MovePathComponent::ResultOkValue
            | MovePathComponent::ResultErrValue
            | MovePathComponent::UnionValue(_)
            | MovePathComponent::ArrayPatternIndex(_)
            | MovePathComponent::ArrayPatternRest { .. } => [None, None, None],
        };
        inputs.into_iter().flatten()
    })
}

#[derive(Clone, Copy)]
enum CollectionComponent {
    None,
    Static(StaticCollectionRegion),
    Dynamic,
}

fn static_regions_are_identical(
    left: StaticCollectionRegion,
    right: StaticCollectionRegion,
) -> bool {
    if left == right {
        return true;
    }
    matches!(
        (left, right),
        (
            StaticCollectionRegion::Index(left),
            StaticCollectionRegion::PatternIndex(right)
        ) | (
            StaticCollectionRegion::PatternIndex(right),
            StaticCollectionRegion::Index(left)
        ) if left == u64::from(right)
    )
}

fn collection_region(
    component: &MovePathComponent,
    static_integers: &BTreeMap<BytecodeSlotId, u64>,
) -> CollectionComponent {
    match component {
        MovePathComponent::ArrayPatternIndex(index) => {
            CollectionComponent::Static(StaticCollectionRegion::PatternIndex(*index))
        }
        MovePathComponent::ArrayPatternRest { start, suffix } => {
            CollectionComponent::Static(StaticCollectionRegion::PatternRest {
                start: *start,
                suffix: *suffix,
            })
        }
        MovePathComponent::Index {
            index,
            access: BytecodeIndexAccess::Array,
        } => static_integers
            .get(index)
            .map_or(CollectionComponent::Dynamic, |index| {
                CollectionComponent::Static(StaticCollectionRegion::Index(*index))
            }),
        MovePathComponent::Index {
            access:
                BytecodeIndexAccess::String
                | BytecodeIndexAccess::MapLookup
                | BytecodeIndexAccess::MapEntry,
            ..
        }
        | MovePathComponent::IteratorElement { .. } => CollectionComponent::Dynamic,
        MovePathComponent::Slice { start, end, step } => {
            let Some(start) = static_optional_bound(*start, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(end) = static_optional_bound(*end, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(step) = static_optional_bound(*step, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            CollectionComponent::Static(StaticCollectionRegion::Slice(StaticSlice {
                start,
                end,
                step,
            }))
        }
        MovePathComponent::ClosureCapture(_, _)
        | MovePathComponent::IteratorSource
        | MovePathComponent::Field(_)
        | MovePathComponent::TupleField(_)
        | MovePathComponent::NewtypeValue
        | MovePathComponent::RefValue
        | MovePathComponent::VariantTuple(_, _)
        | MovePathComponent::VariantField(_, _)
        | MovePathComponent::OptionValue
        | MovePathComponent::ResultOkValue
        | MovePathComponent::ResultErrValue
        | MovePathComponent::UnionValue(_) => CollectionComponent::None,
    }
}

fn static_optional_bound(
    slot: Option<BytecodeSlotId>,
    static_integers: &BTreeMap<BytecodeSlotId, u64>,
) -> Option<Option<u64>> {
    match slot {
        Some(slot) => Some(Some(*static_integers.get(&slot)?)),
        None => Some(None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StaticSlice {
    start: Option<u64>,
    end: Option<u64>,
    step: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaticCollectionRegion {
    Index(u64),
    Slice(StaticSlice),
    PatternIndex(u32),
    PatternRest { start: u32, suffix: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaticRegionRelation {
    Disjoint,
    Overlap,
    Runtime,
}

fn static_collection_relation(
    left: StaticCollectionRegion,
    right: StaticCollectionRegion,
) -> StaticRegionRelation {
    use StaticCollectionRegion::{Index, PatternIndex, PatternRest, Slice};

    match (left, right) {
        (Index(left), Index(right)) => index_relation(left, right),
        (PatternIndex(left), PatternIndex(right)) => {
            index_relation(u64::from(left), u64::from(right))
        }
        (Index(left), PatternIndex(right)) | (PatternIndex(right), Index(left)) => {
            index_relation(left, u64::from(right))
        }
        (Index(index), Slice(slice)) | (Slice(slice), Index(index)) => {
            index_slice_relation(index, slice)
        }
        (PatternIndex(index), Slice(slice)) | (Slice(slice), PatternIndex(index)) => {
            index_slice_relation(u64::from(index), slice)
        }
        (Slice(left), Slice(right)) => slice_relation(left, right),
        (PatternIndex(index), PatternRest { start, suffix })
        | (PatternRest { start, suffix }, PatternIndex(index)) => {
            rest_index_relation(u64::from(index), start, suffix)
        }
        (Index(index), PatternRest { start, suffix })
        | (PatternRest { start, suffix }, Index(index)) => {
            rest_index_relation(index, start, suffix)
        }
        (PatternRest { .. }, PatternRest { .. })
        | (Slice(_), PatternRest { .. })
        | (PatternRest { .. }, Slice(_)) => StaticRegionRelation::Runtime,
    }
}

fn index_relation(left: u64, right: u64) -> StaticRegionRelation {
    if left == right {
        StaticRegionRelation::Overlap
    } else {
        StaticRegionRelation::Disjoint
    }
}

fn rest_index_relation(index: u64, start: u32, suffix: u32) -> StaticRegionRelation {
    if index < u64::from(start) {
        StaticRegionRelation::Disjoint
    } else if suffix == 0 {
        StaticRegionRelation::Overlap
    } else {
        StaticRegionRelation::Runtime
    }
}

fn index_slice_relation(index: u64, slice: StaticSlice) -> StaticRegionRelation {
    if slice_contains(slice, index) {
        StaticRegionRelation::Overlap
    } else {
        StaticRegionRelation::Disjoint
    }
}

fn slice_relation(left: StaticSlice, right: StaticSlice) -> StaticRegionRelation {
    let Some(left) = positive_progression(left) else {
        return StaticRegionRelation::Disjoint;
    };
    let Some(right) = positive_progression(right) else {
        return StaticRegionRelation::Disjoint;
    };
    if left.end.is_some_and(|end| end <= right.start)
        || right.end.is_some_and(|end| end <= left.start)
    {
        return StaticRegionRelation::Disjoint;
    }
    let divisor = greatest_common_divisor(left.step, right.step);
    if left.start % divisor != right.start % divisor {
        return StaticRegionRelation::Disjoint;
    }
    StaticRegionRelation::Runtime
}

fn slice_contains(slice: StaticSlice, index: u64) -> bool {
    let Some(slice) = positive_progression(slice) else {
        return false;
    };
    index >= slice.start
        && slice.end.is_none_or(|end| index < end)
        && (index - slice.start).is_multiple_of(slice.step)
}

#[derive(Clone, Copy)]
struct PositiveProgression {
    start: u64,
    end: Option<u64>,
    step: u64,
}

fn positive_progression(slice: StaticSlice) -> Option<PositiveProgression> {
    let start = slice.start.unwrap_or(0);
    let step = slice.step.unwrap_or(1);
    if step == 0 || slice.end.is_some_and(|end| end <= start) {
        return None;
    }
    Some(PositiveProgression {
        start,
        end: slice.end,
        step,
    })
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn static_integer_slots(
    program: &BytecodeProgram,
    function: &BytecodeFunction,
) -> BTreeMap<BytecodeSlotId, u64> {
    let mut candidates = BTreeMap::<BytecodeSlotId, Option<u64>>::new();
    let mut record = |place: &BytecodePlace, value: Option<u64>| {
        if !place.projections.is_empty()
            || function.slots[place.slot.index() as usize].kind != BytecodeSlotKind::Temporary
        {
            return;
        }
        candidates
            .entry(place.slot)
            .and_modify(|candidate| *candidate = None)
            .or_insert(value);
    };
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let BytecodeInstructionKind::Store { destination, value } = &instruction.kind {
                record(destination, static_integer_rvalue(program, value));
            }
        }
        match &block.terminator.kind {
            BytecodeTerminatorKind::Invoke {
                destination: Some(destination),
                ..
            }
            | BytecodeTerminatorKind::Await { destination, .. }
            | BytecodeTerminatorKind::Spawn { destination, .. }
            | BytecodeTerminatorKind::IteratorNext { destination, .. } => record(destination, None),
            BytecodeTerminatorKind::Goto { .. }
            | BytecodeTerminatorKind::BranchBool { .. }
            | BytecodeTerminatorKind::BranchTag { .. }
            | BytecodeTerminatorKind::Invoke {
                destination: None, ..
            }
            | BytecodeTerminatorKind::ValidatePlaces { .. }
            | BytecodeTerminatorKind::ValidateLoan { .. }
            | BytecodeTerminatorKind::DrainDefers { .. }
            | BytecodeTerminatorKind::DrainScopes { .. }
            | BytecodeTerminatorKind::DrainUnwind { .. }
            | BytecodeTerminatorKind::Return
            | BytecodeTerminatorKind::ResumePanic
            | BytecodeTerminatorKind::Unreachable => {}
        }
    }
    candidates
        .into_iter()
        .filter_map(|(slot, value)| value.map(|value| (slot, value)))
        .collect()
}

fn static_integer_rvalue(program: &BytecodeProgram, value: &BytecodeRvalue) -> Option<u64> {
    let BytecodeRvalueKind::Use(operand) = &value.kind else {
        return None;
    };
    match &operand.kind {
        BytecodeOperandKind::Constant(BytecodeConstant::Integer(spelling)) => {
            parse_nonnegative_integer(spelling)
        }
        BytecodeOperandKind::Constant(BytecodeConstant::Named(constant)) => {
            let BytecodeConstantValueKind::Integer(value) =
                &program.constants.get(constant.index() as usize)?.value.kind
            else {
                return None;
            };
            u64::try_from(*value).ok()
        }
        BytecodeOperandKind::Constant(
            BytecodeConstant::Unit
            | BytecodeConstant::Bool(_)
            | BytecodeConstant::Float(_)
            | BytecodeConstant::Char(_)
            | BytecodeConstant::String(_),
        )
        | BytecodeOperandKind::Copy(_)
        | BytecodeOperandKind::Move(_)
        | BytecodeOperandKind::Borrow(_)
        | BytecodeOperandKind::Loan(_)
        | BytecodeOperandKind::Function { .. } => None,
    }
}

fn parse_nonnegative_integer(spelling: &str) -> Option<u64> {
    let suffix_length = ["i16", "i32", "i64", "u16", "u32", "u64"]
        .into_iter()
        .find(|suffix| spelling.ends_with(suffix))
        .map_or_else(
            || {
                ["i8", "u8"]
                    .into_iter()
                    .find(|suffix| spelling.ends_with(suffix))
                    .map_or(0, |suffix| suffix.len())
            },
            |suffix| suffix.len(),
        );
    let body = &spelling[..spelling.len().checked_sub(suffix_length)?];
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, body)
    };
    u128::from_str_radix(&digits.replace('_', ""), radix)
        .ok()
        .and_then(|value| u64::try_from(value).ok())
}

fn move_path_is_prefix(prefix: &[MovePathComponent], path: &[MovePathComponent]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(left, right)| left == right)
}

fn move_path_components_are_disjoint(left: &MovePathComponent, right: &MovePathComponent) -> bool {
    match (left, right) {
        (
            MovePathComponent::ClosureCapture(_, left),
            MovePathComponent::ClosureCapture(_, right),
        )
        | (MovePathComponent::TupleField(left), MovePathComponent::TupleField(right))
        | (
            MovePathComponent::ArrayPatternIndex(left),
            MovePathComponent::ArrayPatternIndex(right),
        ) => left != right,
        (MovePathComponent::Field(left), MovePathComponent::Field(right)) => left != right,
        (
            MovePathComponent::VariantTuple(left_variant, left),
            MovePathComponent::VariantTuple(right_variant, right),
        ) => left_variant != right_variant || left != right,
        (
            MovePathComponent::VariantField(left_variant, left),
            MovePathComponent::VariantField(right_variant, right),
        ) => left_variant != right_variant || left != right,
        (
            MovePathComponent::VariantTuple(left, _) | MovePathComponent::VariantField(left, _),
            MovePathComponent::VariantTuple(right, _) | MovePathComponent::VariantField(right, _),
        ) => left != right,
        (MovePathComponent::OptionValue, MovePathComponent::ResultOkValue)
        | (MovePathComponent::OptionValue, MovePathComponent::ResultErrValue)
        | (MovePathComponent::ResultOkValue, MovePathComponent::OptionValue)
        | (MovePathComponent::ResultErrValue, MovePathComponent::OptionValue)
        | (MovePathComponent::ResultOkValue, MovePathComponent::ResultErrValue)
        | (MovePathComponent::ResultErrValue, MovePathComponent::ResultOkValue) => true,
        (MovePathComponent::UnionValue(left), MovePathComponent::UnionValue(right)) => {
            left != right
        }
        (
            MovePathComponent::ArrayPatternIndex(index),
            MovePathComponent::ArrayPatternRest { start, suffix: 0 },
        )
        | (
            MovePathComponent::ArrayPatternRest { start, suffix: 0 },
            MovePathComponent::ArrayPatternIndex(index),
        ) => index < start,
        _ => false,
    }
}

fn unavailable_read_message(slot: BytecodeSlotId, path: &[MovePathComponent]) -> String {
    if path.is_empty() {
        format!(
            "reads slot#{} before a dominating live definition",
            slot.index()
        )
    } else {
        format!("reads an unavailable move path of slot#{}", slot.index())
    }
}

fn unavailable_move_message(slot: BytecodeSlotId, path: &[MovePathComponent]) -> String {
    if path.is_empty() {
        format!(
            "moves slot#{} after its value became unavailable",
            slot.index()
        )
    } else {
        format!("moves an unavailable move path of slot#{}", slot.index())
    }
}

fn local_events(function: &BytecodeFunction, block: &BytecodeBlock) -> Vec<LocalEvent> {
    let mut events = Vec::new();
    for instruction in &block.instructions {
        match &instruction.kind {
            BytecodeInstructionKind::StorageLive(slot) => {
                events.push(LocalEvent::StorageLive(*slot));
            }
            BytecodeInstructionKind::StorageDead(slot) => {
                events.push(LocalEvent::StorageDead(*slot));
            }
            BytecodeInstructionKind::ReserveLoan(loan) => {
                if let Some(loan) = function.loans.get(loan.index() as usize) {
                    push_place_events(&loan.place, true, &mut events);
                }
            }
            BytecodeInstructionKind::ReleaseLoan(_) => {}
            BytecodeInstructionKind::Store { destination, value } => {
                push_rvalue_events(value, &mut events);
                push_destination_events(destination, &mut events);
            }
            BytecodeInstructionKind::RegisterDefer { action, guard, .. } => {
                push_defer_operation_events(action, guard.as_ref(), &mut events);
            }
            BytecodeInstructionKind::RegisterFallback { owner, .. } => {
                push_place_events(owner, true, &mut events);
            }
            BytecodeInstructionKind::EnterTaskScope { .. }
            | BytecodeInstructionKind::RetargetCleanup { .. }
            | BytecodeInstructionKind::DisarmCleanup(_) => {}
        }
    }
    match &block.terminator.kind {
        BytecodeTerminatorKind::Goto { .. }
        | BytecodeTerminatorKind::DrainDefers { .. }
        | BytecodeTerminatorKind::DrainScopes { .. }
        | BytecodeTerminatorKind::DrainUnwind { .. }
        | BytecodeTerminatorKind::ResumePanic
        | BytecodeTerminatorKind::Unreachable => {}
        BytecodeTerminatorKind::BranchBool { condition, .. } => {
            push_operand_events(condition, &mut events);
        }
        BytecodeTerminatorKind::BranchTag { value, .. } => {
            push_operand_events(value, &mut events);
        }
        BytecodeTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } => {
            push_operation_events(operation, &mut events);
            if let Some(destination) = destination {
                push_destination_reads(destination, true, &mut events);
            }
        }
        BytecodeTerminatorKind::Await {
            awaitable,
            destination,
            ..
        } => {
            match awaitable {
                BytecodeAwaitable::Call(operation) => {
                    push_operation_events(operation, &mut events);
                }
                BytecodeAwaitable::Join(join) => push_operand_events(join, &mut events),
            }
            push_destination_reads(destination, true, &mut events);
        }
        BytecodeTerminatorKind::Spawn {
            operation,
            destination,
            ..
        } => {
            push_operation_events(operation, &mut events);
            push_destination_reads(destination, true, &mut events);
        }
        BytecodeTerminatorKind::IteratorNext {
            state,
            destination,
            borrowed_source,
            ..
        } => {
            push_place_events(state, true, &mut events);
            push_destination_reads(destination, true, &mut events);
            if let Some(source) = borrowed_source {
                push_place_events(source, true, &mut events);
            }
        }
        BytecodeTerminatorKind::ValidatePlaces {
            places,
            replacements,
            for_write,
            ..
        } => {
            for place in places {
                push_destination_reads(place, *for_write, &mut events);
            }
            for replacement in replacements.iter().flatten() {
                push_operand_events(replacement, &mut events);
            }
        }
        BytecodeTerminatorKind::ValidateLoan { loan, .. } => {
            if let Some(loan) = function.loans.get(loan.index() as usize) {
                push_resolve_place_events(&loan.place, &mut events);
            }
        }
        BytecodeTerminatorKind::Return => events.push(LocalEvent::Read(LocalAccess {
            slot: function.return_slot,
            path: Vec::new(),
            source_loan: None,
        })),
    }
    events
}

fn tag_events(function: &BytecodeFunction, block: &BytecodeBlock) -> Vec<TagEvent> {
    let mut events = Vec::new();
    for instruction in &block.instructions {
        match &instruction.kind {
            BytecodeInstructionKind::StorageLive(_)
            | BytecodeInstructionKind::StorageDead(_)
            | BytecodeInstructionKind::EnterTaskScope { .. }
            | BytecodeInstructionKind::ReserveLoan(_)
            | BytecodeInstructionKind::ReleaseLoan(_)
            | BytecodeInstructionKind::RegisterFallback { .. }
            | BytecodeInstructionKind::RetargetCleanup { .. }
            | BytecodeInstructionKind::DisarmCleanup(_) => {}
            BytecodeInstructionKind::Store { destination, value } => {
                push_tag_rvalue(function, value, &mut events);
                push_tag_place(function, destination, true, &mut events);
            }
            BytecodeInstructionKind::RegisterDefer { action, .. } => {
                push_tag_operation(function, action, &mut events);
            }
        }
    }
    match &block.terminator.kind {
        BytecodeTerminatorKind::Goto { .. }
        | BytecodeTerminatorKind::DrainDefers { .. }
        | BytecodeTerminatorKind::DrainScopes { .. }
        | BytecodeTerminatorKind::DrainUnwind { .. }
        | BytecodeTerminatorKind::Return
        | BytecodeTerminatorKind::ResumePanic
        | BytecodeTerminatorKind::Unreachable => {}
        BytecodeTerminatorKind::BranchBool { condition, .. } => {
            push_tag_operand(function, condition, &mut events);
        }
        BytecodeTerminatorKind::BranchTag { value, .. } => {
            push_tag_operand(function, value, &mut events);
        }
        BytecodeTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } => {
            push_tag_operation(function, operation, &mut events);
            if let Some(destination) = destination {
                push_tag_place(function, destination, false, &mut events);
            }
        }
        BytecodeTerminatorKind::Await {
            awaitable,
            destination,
            ..
        } => {
            match awaitable {
                BytecodeAwaitable::Call(operation) => {
                    push_tag_operation(function, operation, &mut events);
                }
                BytecodeAwaitable::Join(join) => push_tag_operand(function, join, &mut events),
            }
            push_tag_place(function, destination, false, &mut events);
        }
        BytecodeTerminatorKind::Spawn {
            operation,
            destination,
            ..
        } => {
            push_tag_operation(function, operation, &mut events);
            push_tag_place(function, destination, false, &mut events);
        }
        BytecodeTerminatorKind::IteratorNext {
            state, destination, ..
        } => {
            push_tag_place(function, state, false, &mut events);
            push_tag_place(function, destination, false, &mut events);
        }
        BytecodeTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                push_tag_place(function, place, false, &mut events);
            }
            for replacement in replacements.iter().flatten() {
                push_tag_operand(function, replacement, &mut events);
            }
        }
        BytecodeTerminatorKind::ValidateLoan { loan, .. } => {
            if let Some(loan) = function.loans.get(loan.index() as usize) {
                push_tag_place(function, &loan.place, false, &mut events);
            }
        }
    }
    events
}

fn push_tag_rvalue(
    function: &BytecodeFunction,
    value: &BytecodeRvalue,
    events: &mut Vec<TagEvent>,
) {
    match &value.kind {
        BytecodeRvalueKind::Use(operand)
        | BytecodeRvalueKind::Prefix { operand, .. }
        | BytecodeRvalueKind::Coerce { value: operand, .. }
        | BytecodeRvalueKind::NumericConversion { value: operand, .. }
        | BytecodeRvalueKind::Length(operand)
        | BytecodeRvalueKind::IteratorState(operand) => {
            push_tag_operand(function, operand, events);
        }
        BytecodeRvalueKind::Binary { left, right, .. } => {
            push_tag_operand(function, left, events);
            push_tag_operand(function, right, events);
        }
        BytecodeRvalueKind::Construct { values, .. } => {
            for value in values {
                push_tag_operand(function, value, events);
            }
        }
        BytecodeRvalueKind::Interpolate { values, .. } => {
            for value in values {
                push_tag_operand(function, value, events);
            }
        }
        BytecodeRvalueKind::RecordUpdate { base, fields } => {
            push_tag_operand(function, base, events);
            for (_, value) in fields {
                push_tag_operand(function, value, events);
            }
        }
        BytecodeRvalueKind::Range { start, end, .. } => {
            push_tag_operand(function, start, events);
            push_tag_operand(function, end, events);
        }
        BytecodeRvalueKind::Contains {
            item, container, ..
        } => {
            push_tag_operand(function, item, events);
            push_tag_operand(function, container, events);
        }
        BytecodeRvalueKind::MapRemove { map, key } => {
            push_tag_place(function, map, true, events);
            push_tag_operand(function, key, events);
        }
    }
}

fn push_tag_operation(
    function: &BytecodeFunction,
    operation: &BytecodeOperation,
    events: &mut Vec<TagEvent>,
) {
    match &operation.kind {
        BytecodeOperationKind::CheckedPrefix { operand, .. } => {
            push_tag_operand(function, operand, events);
        }
        BytecodeOperationKind::CheckedBinary { left, right, .. } => {
            push_tag_operand(function, left, events);
            push_tag_operand(function, right, events);
        }
        BytecodeOperationKind::ArraySequence {
            array, argument, ..
        } => {
            push_tag_operand(function, array, events);
            push_tag_operand(function, argument, events);
        }
        BytecodeOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                push_tag_operand(function, key, events);
                push_tag_operand(function, value, events);
            }
        }
        BytecodeOperationKind::Index { base, index, .. } => {
            push_tag_operand(function, base, events);
            push_tag_operand(function, index, events);
        }
        BytecodeOperationKind::Slice { base, bounds, .. } => {
            push_tag_operand(function, base, events);
            for value in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                push_tag_operand(function, value, events);
            }
        }
        BytecodeOperationKind::Call {
            callee, arguments, ..
        } => {
            push_tag_operand(function, callee, events);
            for argument in arguments {
                push_tag_operand(function, &argument.value, events);
            }
        }
        BytecodeOperationKind::Display { argument } => {
            push_tag_operand(function, &argument.value, events);
        }
        BytecodeOperationKind::ExplicitPanic { message } => {
            push_tag_operand(function, message, events);
        }
        BytecodeOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            push_tag_operand(function, condition, events);
            for part in message_parts {
                push_tag_operand(function, &part.value, events);
            }
        }
        BytecodeOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                push_tag_operand(function, argument, events);
            }
        }
    }
}

fn push_tag_operand(
    function: &BytecodeFunction,
    operand: &BytecodeOperand,
    events: &mut Vec<TagEvent>,
) {
    if let BytecodeOperandKind::Copy(place)
    | BytecodeOperandKind::Move(place)
    | BytecodeOperandKind::Borrow(place) = &operand.kind
    {
        push_tag_place(function, place, false, events);
    }
}

fn push_tag_place(
    function: &BytecodeFunction,
    place: &BytecodePlace,
    write: bool,
    events: &mut Vec<TagEvent>,
) {
    let root_type = function.slots[place.slot.index() as usize].ty;
    for (index, projection) in place.projections.iter().enumerate() {
        let tag = match projection.kind {
            BytecodeProjectionKind::OptionValue => Some(BytecodeTag::OptionSome),
            BytecodeProjectionKind::ResultOkValue => Some(BytecodeTag::ResultOk),
            BytecodeProjectionKind::ResultErrValue => Some(BytecodeTag::ResultErr),
            BytecodeProjectionKind::VariantTuple { variant, .. }
            | BytecodeProjectionKind::VariantField { variant, .. } => {
                Some(BytecodeTag::Variant(variant))
            }
            BytecodeProjectionKind::UnionValue(member) => Some(BytecodeTag::Union(member)),
            BytecodeProjectionKind::ClosureCapture { .. }
            | BytecodeProjectionKind::IteratorSource
            | BytecodeProjectionKind::Field(_)
            | BytecodeProjectionKind::TupleField(_)
            | BytecodeProjectionKind::NewtypeValue
            | BytecodeProjectionKind::RefValue
            | BytecodeProjectionKind::ArrayPatternIndex(_)
            | BytecodeProjectionKind::ArrayPatternRest { .. }
            | BytecodeProjectionKind::IteratorElement { .. }
            | BytecodeProjectionKind::Index { .. }
            | BytecodeProjectionKind::Slice { .. } => None,
        };
        if let Some(tag) = tag {
            let base = BytecodePlace {
                slot: place.slot,
                ty: if index == 0 {
                    root_type
                } else {
                    place.projections[index - 1].ty
                },
                projections: place.projections[..index].to_vec(),
                source_loan: place.source_loan,
            };
            events.push(TagEvent::Require(TagFact { place: base, tag }));
        }
    }
    if write {
        events.push(TagEvent::Write(place.clone()));
    }
}

fn transfer_tag(state: bool, events: &[TagEvent], fact: &TagFact) -> bool {
    let mut state = state;
    for event in events {
        if let TagEvent::Write(write) = event
            && places_may_overlap(write, &fact.place)
        {
            state = false;
        }
    }
    state
}

fn places_may_overlap(left: &BytecodePlace, right: &BytecodePlace) -> bool {
    if left.slot != right.slot {
        return false;
    }
    for (left, right) in left.projections.iter().zip(&right.projections) {
        if left == right {
            continue;
        }
        return match (&left.kind, &right.kind) {
            (BytecodeProjectionKind::Field(left), BytecodeProjectionKind::Field(right)) => {
                left == right
            }
            (
                BytecodeProjectionKind::TupleField(left),
                BytecodeProjectionKind::TupleField(right),
            ) => left == right,
            (
                BytecodeProjectionKind::ArrayPatternIndex(left),
                BytecodeProjectionKind::ArrayPatternIndex(right),
            ) => left == right,
            (
                BytecodeProjectionKind::VariantTuple { variant: left, .. }
                | BytecodeProjectionKind::VariantField { variant: left, .. },
                BytecodeProjectionKind::VariantTuple { variant: right, .. }
                | BytecodeProjectionKind::VariantField { variant: right, .. },
            ) => left == right,
            _ => true,
        };
    }
    true
}

fn same_place_path(left: &BytecodePlace, right: &BytecodePlace) -> bool {
    left.slot == right.slot && left.ty == right.ty && left.projections == right.projections
}

fn complementary_tag(tag: BytecodeTag) -> Option<BytecodeTag> {
    match tag {
        BytecodeTag::OptionNone => Some(BytecodeTag::OptionSome),
        BytecodeTag::OptionSome => Some(BytecodeTag::OptionNone),
        BytecodeTag::ResultOk => Some(BytecodeTag::ResultErr),
        BytecodeTag::ResultErr => Some(BytecodeTag::ResultOk),
        BytecodeTag::Variant(_) | BytecodeTag::Union(_) => None,
    }
}

fn push_rvalue_events(value: &BytecodeRvalue, events: &mut Vec<LocalEvent>) {
    match &value.kind {
        BytecodeRvalueKind::Use(operand)
        | BytecodeRvalueKind::Prefix { operand, .. }
        | BytecodeRvalueKind::Coerce { value: operand, .. }
        | BytecodeRvalueKind::NumericConversion { value: operand, .. }
        | BytecodeRvalueKind::Length(operand)
        | BytecodeRvalueKind::IteratorState(operand) => push_operand_events(operand, events),
        BytecodeRvalueKind::Binary { left, right, .. } => {
            push_operand_events(left, events);
            push_operand_events(right, events);
        }
        BytecodeRvalueKind::Construct { values, .. } => {
            for value in values {
                push_operand_events(value, events);
            }
        }
        BytecodeRvalueKind::Interpolate { values, .. } => {
            for value in values {
                push_operand_events(value, events);
            }
        }
        BytecodeRvalueKind::RecordUpdate { base, fields } => {
            push_operand_events(base, events);
            for (_, value) in fields {
                push_operand_events(value, events);
            }
        }
        BytecodeRvalueKind::Range { start, end, .. } => {
            push_operand_events(start, events);
            push_operand_events(end, events);
        }
        BytecodeRvalueKind::Contains {
            item, container, ..
        } => {
            push_operand_events(item, events);
            push_operand_events(container, events);
        }
        BytecodeRvalueKind::MapRemove { map, key } => {
            push_destination_reads(map, true, events);
            push_operand_events(key, events);
        }
    }
}

fn push_operation_events(operation: &BytecodeOperation, events: &mut Vec<LocalEvent>) {
    match &operation.kind {
        BytecodeOperationKind::CheckedPrefix { operand, .. } => {
            push_operand_events(operand, events);
        }
        BytecodeOperationKind::CheckedBinary { left, right, .. } => {
            push_operand_events(left, events);
            push_operand_events(right, events);
        }
        BytecodeOperationKind::ArraySequence {
            array, argument, ..
        } => {
            push_operand_events(array, events);
            push_operand_events(argument, events);
        }
        BytecodeOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                push_operand_events(key, events);
                push_operand_events(value, events);
            }
        }
        BytecodeOperationKind::Index { base, index, .. } => {
            push_operand_events(base, events);
            push_operand_events(index, events);
        }
        BytecodeOperationKind::Slice { base, bounds, .. } => {
            push_operand_events(base, events);
            for value in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                push_operand_events(value, events);
            }
        }
        BytecodeOperationKind::Call {
            callee, arguments, ..
        } => {
            push_operand_events(callee, events);
            for argument in arguments {
                push_operand_events(&argument.value, events);
            }
        }
        BytecodeOperationKind::Display { argument } => {
            push_operand_events(&argument.value, events);
        }
        BytecodeOperationKind::ExplicitPanic { message } => {
            push_operand_events(message, events);
        }
        BytecodeOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            push_operand_events(condition, events);
            for part in message_parts {
                push_operand_events(&part.value, events);
            }
        }
        BytecodeOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                push_operand_events(argument, events);
            }
        }
    }
}

fn push_defer_operation_events(
    operation: &BytecodeOperation,
    guard: Option<&BytecodePlace>,
    events: &mut Vec<LocalEvent>,
) {
    for operand in operation_operands(operation) {
        if let (Some(guard), BytecodeOperandKind::Move(place)) = (guard, &operand.kind)
            && place == guard
        {
            push_resolve_place_events(place, events);
        } else {
            push_operand_events(operand, events);
        }
    }
}

fn push_operand_events(operand: &BytecodeOperand, events: &mut Vec<LocalEvent>) {
    match &operand.kind {
        BytecodeOperandKind::Move(place) => {
            push_projection_index_events(place, events);
            events.push(LocalEvent::Move(LocalAccess::from_place(place)));
        }
        BytecodeOperandKind::Copy(place) | BytecodeOperandKind::Borrow(place) => {
            push_projection_index_events(place, events);
            events.push(LocalEvent::Read(LocalAccess::from_place(place)));
        }
        BytecodeOperandKind::Constant(_)
        | BytecodeOperandKind::Function { .. }
        | BytecodeOperandKind::Loan(_) => {}
    }
}

fn push_destination_events(place: &BytecodePlace, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    events.push(LocalEvent::Write(LocalAccess::from_place(place)));
}

fn push_destination_reads(place: &BytecodePlace, for_write: bool, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    let access = LocalAccess::from_place(place);
    if for_write {
        events.push(LocalEvent::WriteAccess(access));
    } else {
        events.push(LocalEvent::Read(access));
    }
}

fn push_place_events(place: &BytecodePlace, read_root: bool, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    if read_root {
        events.push(LocalEvent::Read(LocalAccess::from_place(place)));
    }
}

fn push_resolve_place_events(place: &BytecodePlace, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    events.push(LocalEvent::Resolve(LocalAccess::from_place(place)));
}

fn push_projection_index_events(place: &BytecodePlace, events: &mut Vec<LocalEvent>) {
    for projection in &place.projections {
        match &projection.kind {
            BytecodeProjectionKind::Index { index, .. } => {
                events.push(LocalEvent::Read(LocalAccess {
                    slot: *index,
                    path: Vec::new(),
                    source_loan: None,
                }));
            }
            BytecodeProjectionKind::IteratorElement { index } => {
                events.push(LocalEvent::Read(LocalAccess {
                    slot: *index,
                    path: Vec::new(),
                    source_loan: None,
                }));
            }
            BytecodeProjectionKind::Slice { start, end, step } => {
                events.extend(start.iter().chain(end).chain(step).copied().map(|slot| {
                    LocalEvent::Read(LocalAccess {
                        slot,
                        path: Vec::new(),
                        source_loan: None,
                    })
                }));
            }
            BytecodeProjectionKind::ClosureCapture { .. }
            | BytecodeProjectionKind::IteratorSource
            | BytecodeProjectionKind::Field(_)
            | BytecodeProjectionKind::TupleField(_)
            | BytecodeProjectionKind::NewtypeValue
            | BytecodeProjectionKind::RefValue
            | BytecodeProjectionKind::VariantTuple { .. }
            | BytecodeProjectionKind::VariantField { .. }
            | BytecodeProjectionKind::OptionValue
            | BytecodeProjectionKind::ResultOkValue
            | BytecodeProjectionKind::ResultErrValue
            | BytecodeProjectionKind::UnionValue(_)
            | BytecodeProjectionKind::ArrayPatternIndex(_)
            | BytecodeProjectionKind::ArrayPatternRest { .. } => {}
        }
    }
}

fn place_requires_loan_validation(place: &BytecodePlace) -> bool {
    place.projections.iter().any(|projection| {
        matches!(
            projection.kind,
            BytecodeProjectionKind::Index { .. } | BytecodeProjectionKind::Slice { .. }
        )
    })
}

fn place_contains_ref_value(place: &BytecodePlace) -> bool {
    place
        .projections
        .iter()
        .any(|projection| matches!(projection.kind, BytecodeProjectionKind::RefValue))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal_program(opaque_witness_terminal: bool) -> BytecodeProgram {
        let int = BytecodeTypeId::new(0);
        let never = BytecodeTypeId::new(1);
        let parameter = BytecodeTypeId::new(2);
        let join = BytecodeTypeId::new(3);
        let wrapper_join = BytecodeTypeId::new(5);
        let option = BytecodeTypeId::new(6);
        let array = BytecodeTypeId::new(7);
        BytecodeProgram {
            types: vec![
                BytecodeType {
                    name: "Int".into(),
                    kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Int),
                },
                BytecodeType {
                    name: "Never".into(),
                    kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Never),
                },
                BytecodeType {
                    name: "$0".into(),
                    kind: BytecodeTypeKind::GenericParameter(0),
                },
                BytecodeType {
                    name: "Join[Int,Never]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Join,
                        arguments: vec![int, never],
                    },
                },
                BytecodeType {
                    name: "Wrapper[$0]".into(),
                    kind: BytecodeTypeKind::Nominal {
                        nominal: Some(BytecodeNominalId::new(0)),
                        identity: "test::Wrapper".into(),
                        arguments: vec![parameter],
                    },
                },
                BytecodeType {
                    name: "Wrapper[Join]".into(),
                    kind: BytecodeTypeKind::Nominal {
                        nominal: Some(BytecodeNominalId::new(0)),
                        identity: "test::Wrapper".into(),
                        arguments: vec![join],
                    },
                },
                BytecodeType {
                    name: "Wrapper[Join]?".into(),
                    kind: BytecodeTypeKind::Option(wrapper_join),
                },
                BytecodeType {
                    name: "Array[Wrapper[Join]?]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Array,
                        arguments: vec![option],
                    },
                },
                BytecodeType {
                    name: "opaque-result".into(),
                    kind: BytecodeTypeKind::OpaqueResult {
                        identity: "test::opaque".into(),
                        arguments: Vec::new(),
                        witness: if opaque_witness_terminal { array } else { int },
                        capabilities: BytecodeCapabilitySet {
                            discard: true,
                            ..BytecodeCapabilitySet::default()
                        },
                    },
                },
            ],
            nominals: vec![BytecodeNominal {
                name: "Wrapper".into(),
                identity: "test::Wrapper".into(),
                generic_arity: 1,
                shape: BytecodeNominalShape::Newtype {
                    underlying: parameter,
                },
            }],
            callables: Vec::new(),
            constants: Vec::new(),
            functions: Vec::new(),
        }
    }

    #[test]
    fn terminal_status_is_rederived_from_the_closed_bytecode_catalog() {
        let program = terminal_program(false);
        verify_bytecode(&program).unwrap();
        assert_eq!(
            derive_terminal_statuses(
                &program,
                &[
                    BytecodeTypeId::new(0),
                    BytecodeTypeId::new(3),
                    BytecodeTypeId::new(4),
                    BytecodeTypeId::new(5),
                    BytecodeTypeId::new(6),
                    BytecodeTypeId::new(7),
                    BytecodeTypeId::new(8),
                ],
            )
            .unwrap(),
            [
                BytecodeTerminalStatus::Absent,
                BytecodeTerminalStatus::Present,
                BytecodeTerminalStatus::Potential,
                BytecodeTerminalStatus::Present,
                BytecodeTerminalStatus::Present,
                BytecodeTerminalStatus::Present,
                BytecodeTerminalStatus::Absent,
            ]
        );
    }

    #[test]
    fn numeric_conversion_error_constants_use_only_the_closed_unit_variants() {
        let mut program = terminal_program(false);
        let error_ty = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "NumericConversionError".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::NumericConversionError,
                arguments: Vec::new(),
            },
        });
        program.constants = BytecodeNumericConversionError::ALL
            .into_iter()
            .map(|variant| BytecodeNamedConstant {
                name: format!("error{}", variant.index()),
                value: BytecodeConstantValue {
                    ty: error_ty,
                    kind: BytecodeConstantValueKind::Variant {
                        variant: variant.index(),
                        payload: BytecodeConstantVariantValue::Unit,
                    },
                },
            })
            .collect();
        verify_bytecode(&program).unwrap();

        let mut unknown = program.clone();
        let BytecodeConstantValueKind::Variant { variant, .. } =
            &mut unknown.constants[0].value.kind
        else {
            unreachable!()
        };
        *variant = BytecodeNumericConversionError::ALL.len() as u32;
        assert!(verify_bytecode(&unknown).is_err());

        let mut payload = program;
        let BytecodeConstantValueKind::Variant {
            payload: invalid, ..
        } = &mut payload.constants[0].value.kind
        else {
            unreachable!()
        };
        *invalid = BytecodeConstantVariantValue::Tuple(Vec::new());
        assert!(verify_bytecode(&payload).is_err());
    }

    #[test]
    fn named_integer_constants_must_fit_their_exact_scalar_type() {
        for (scalar, minimum, maximum) in [
            (BytecodeScalarType::Byte, 0, u8::MAX as i128),
            (BytecodeScalarType::UInt8, 0, u8::MAX as i128),
            (BytecodeScalarType::UInt16, 0, u16::MAX as i128),
            (BytecodeScalarType::UInt32, 0, u32::MAX as i128),
            (BytecodeScalarType::UInt64, 0, u64::MAX as i128),
            (BytecodeScalarType::Int8, i8::MIN as i128, i8::MAX as i128),
            (
                BytecodeScalarType::Int16,
                i16::MIN as i128,
                i16::MAX as i128,
            ),
            (
                BytecodeScalarType::Int32,
                i32::MIN as i128,
                i32::MAX as i128,
            ),
            (BytecodeScalarType::Int, i64::MIN as i128, i64::MAX as i128),
        ] {
            let mut program = terminal_program(false);
            let ty = if scalar == BytecodeScalarType::Int {
                BytecodeTypeId::new(0)
            } else {
                let ty = BytecodeTypeId::new(program.types.len() as u32);
                program.types.push(BytecodeType {
                    name: format!("{scalar:?}"),
                    kind: BytecodeTypeKind::Scalar(scalar),
                });
                ty
            };
            program.constants = [("minimum", minimum), ("maximum", maximum)]
                .into_iter()
                .map(|(name, value)| BytecodeNamedConstant {
                    name: name.into(),
                    value: BytecodeConstantValue {
                        ty,
                        kind: BytecodeConstantValueKind::Integer(value),
                    },
                })
                .collect();
            verify_bytecode(&program).unwrap();

            let mut below = program.clone();
            below.constants[0].value.kind = BytecodeConstantValueKind::Integer(minimum - 1);
            assert!(verify_bytecode(&below).is_err(), "{scalar:?} below minimum");

            let mut above = program;
            above.constants[1].value.kind = BytecodeConstantValueKind::Integer(maximum + 1);
            assert!(verify_bytecode(&above).is_err(), "{scalar:?} above maximum");
        }
    }

    #[test]
    fn named_float_constants_use_canonical_f64_storage_for_their_declared_precision() {
        let mut program = terminal_program(false);
        let float32 = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Float32".into(),
            kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Float32),
        });
        let float64 = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Float".into(),
            kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Float),
        });
        program.constants = vec![
            BytecodeNamedConstant {
                name: "single".into(),
                value: BytecodeConstantValue {
                    ty: float32,
                    kind: BytecodeConstantValueKind::Float(f64::from(1.5_f32).to_bits()),
                },
            },
            BytecodeNamedConstant {
                name: "double".into(),
                value: BytecodeConstantValue {
                    ty: float64,
                    kind: BytecodeConstantValueKind::Float(1.000_000_000_000_000_2_f64.to_bits()),
                },
            },
            BytecodeNamedConstant {
                name: "single_nan".into(),
                value: BytecodeConstantValue {
                    ty: float32,
                    kind: BytecodeConstantValueKind::Float(f64::NAN.to_bits()),
                },
            },
        ];
        verify_bytecode(&program).unwrap();

        let mut excessive_precision = program.clone();
        excessive_precision.constants[0].value.kind =
            BytecodeConstantValueKind::Float(1.000_000_000_000_000_2_f64.to_bits());
        assert!(verify_bytecode(&excessive_precision).is_err());

        let mut width_specific_bits = program;
        width_specific_bits.constants[0].value.kind =
            BytecodeConstantValueKind::Float(u64::from(1.5_f32.to_bits()));
        assert!(verify_bytecode(&width_specific_bits).is_err());
    }

    #[test]
    fn nominal_layouts_follow_generated_closure_captures() {
        let mut program = terminal_program(false);
        let environment = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "generated-closure".into(),
            kind: BytecodeTypeKind::Generated {
                identity: "test::closure".into(),
                arguments: Vec::new(),
            },
        });
        program.callables.push(BytecodeCallable {
            name: "closure".into(),
            generic_arity: 0,
            parameters: Vec::new(),
            outcome: BytecodeTypeId::new(0),
            function_type: BytecodeTypeId::new(0),
            implementation: None,
            closure: Some(BytecodeClosure {
                environment,
                captures: vec![BytecodeTypeId::new(3)],
                protocols: BytecodeClosureProtocols {
                    call: false,
                    call_mut: false,
                    call_once: true,
                },
            }),
        });
        let nominal = BytecodeNominalId::new(program.nominals.len() as u32);
        program.nominals.push(BytecodeNominal {
            name: "ClosureBox".into(),
            identity: "test::ClosureBox".into(),
            generic_arity: 0,
            shape: BytecodeNominalShape::Newtype {
                underlying: environment,
            },
        });
        let wrapper = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "ClosureBox".into(),
            kind: BytecodeTypeKind::Nominal {
                nominal: Some(nominal),
                identity: "test::ClosureBox".into(),
                arguments: Vec::new(),
            },
        });

        assert_eq!(
            derive_terminal_statuses(&program, &[environment, wrapper]).unwrap(),
            [
                BytecodeTerminalStatus::Present,
                BytecodeTerminalStatus::Present,
            ]
        );
    }

    fn trace_program() -> BytecodeProgram {
        let mut program = terminal_program(false);
        let int = BytecodeTypeId::new(0);
        let array = BytecodeTypeId::new(7);
        let string = BytecodeTypeId::new(9);
        let tuple = BytecodeTypeId::new(10);
        let map = BytecodeTypeId::new(11);
        let set = BytecodeTypeId::new(12);
        let result = BytecodeTypeId::new(13);
        let union = BytecodeTypeId::new(14);
        let range = BytecodeTypeId::new(15);
        let reference = BytecodeTypeId::new(16);
        let cursor = BytecodeTypeId::new(17);
        let environment = BytecodeTypeId::new(18);
        let record = BytecodeTypeId::new(19);
        let variant = BytecodeTypeId::new(20);
        let pointer = BytecodeTypeId::new(21);
        let opaque = BytecodeTypeId::new(22);
        program.types.extend([
            BytecodeType {
                name: "String".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::String),
            },
            BytecodeType {
                name: "(Int,String)".into(),
                kind: BytecodeTypeKind::Tuple(vec![int, string]),
            },
            BytecodeType {
                name: "Map[String,Array]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Map,
                    arguments: vec![string, array],
                },
            },
            BytecodeType {
                name: "Set[String]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Set,
                    arguments: vec![string],
                },
            },
            BytecodeType {
                name: "String ! Array".into(),
                kind: BytecodeTypeKind::Result {
                    success: string,
                    error: array,
                },
            },
            BytecodeType {
                name: "Int | String".into(),
                kind: BytecodeTypeKind::Union(vec![int, string]),
            },
            BytecodeType {
                name: "Range[Int]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Range,
                    arguments: vec![int],
                },
            },
            BytecodeType {
                name: "Ref[String]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Ref,
                    arguments: vec![string],
                },
            },
            BytecodeType {
                name: "cursor-ref".into(),
                kind: BytecodeTypeKind::Cursor {
                    mode: BytecodeCursorMode::Ref,
                    collection: array,
                },
            },
            BytecodeType {
                name: "closure-environment".into(),
                kind: BytecodeTypeKind::Generated {
                    identity: "test::closure".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Record".into(),
                kind: BytecodeTypeKind::Nominal {
                    nominal: Some(BytecodeNominalId::new(1)),
                    identity: "test::Record".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Variant".into(),
                kind: BytecodeTypeKind::Nominal {
                    nominal: Some(BytecodeNominalId::new(2)),
                    identity: "test::Variant".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Pointer[String]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Pointer,
                    arguments: vec![string],
                },
            },
            BytecodeType {
                name: "opaque-map".into(),
                kind: BytecodeTypeKind::OpaqueResult {
                    identity: "test::opaque-map".into(),
                    arguments: Vec::new(),
                    witness: map,
                    capabilities: BytecodeCapabilitySet::default(),
                },
            },
        ]);
        program.nominals.extend([
            BytecodeNominal {
                name: "Record".into(),
                identity: "test::Record".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Record {
                    fields: vec![
                        BytecodeField {
                            member: 0,
                            ty: string,
                        },
                        BytecodeField {
                            member: 1,
                            ty: array,
                        },
                    ],
                },
            },
            BytecodeNominal {
                name: "Variant".into(),
                identity: "test::Variant".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Enum {
                    variants: vec![
                        BytecodeVariant {
                            member: 0,
                            payload: BytecodeVariantPayload::Unit,
                        },
                        BytecodeVariant {
                            member: 1,
                            payload: BytecodeVariantPayload::Tuple(vec![string]),
                        },
                        BytecodeVariant {
                            member: 2,
                            payload: BytecodeVariantPayload::Record(vec![BytecodeField {
                                member: 0,
                                ty: array,
                            }]),
                        },
                    ],
                },
            },
        ]);
        program.callables.push(BytecodeCallable {
            name: "closure".into(),
            generic_arity: 0,
            parameters: Vec::new(),
            outcome: int,
            function_type: int,
            implementation: Some(BytecodeFunctionId::new(0)),
            closure: Some(BytecodeClosure {
                environment,
                captures: vec![string, array],
                protocols: BytecodeClosureProtocols {
                    call: true,
                    call_mut: false,
                    call_once: true,
                },
            }),
        });
        let span = BytecodeSpan {
            file: 0,
            start: 0,
            end: 0,
        };
        program.functions.push(BytecodeFunction {
            callable: BytecodeCallableId::new(0),
            source: span,
            types: vec![
                tuple, map, set, result, union, range, reference, cursor, record, variant, pointer,
                opaque,
            ],
            spans: vec![span],
            slots: vec![
                BytecodeSlot {
                    ty: map,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Return,
                },
                BytecodeSlot {
                    ty: environment,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Temporary,
                },
                BytecodeSlot {
                    ty: record,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Temporary,
                },
                BytecodeSlot {
                    ty: reference,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Temporary,
                },
            ],
            loans: Vec::new(),
            parameters: Vec::new(),
            return_slot: BytecodeSlotId::new(0),
            entry: BytecodeBlockId::new(0),
            unwind: BytecodeBlockId::new(0),
            blocks: Vec::new(),
        });
        program
    }

    #[test]
    fn trace_metadata_describes_every_managed_shape_and_frame_slot() {
        let metadata = derive_trace_metadata(&trace_program()).unwrap();
        assert_eq!(metadata.types[0], BytecodeTraceDescriptor::Inline);
        assert_eq!(metadata.types[9], BytecodeTraceDescriptor::String);
        assert_eq!(
            metadata.types[10],
            BytecodeTraceDescriptor::Tuple {
                fields: vec![BytecodeTypeId::new(0), BytecodeTypeId::new(9)]
            }
        );
        assert_eq!(
            metadata.types[11],
            BytecodeTraceDescriptor::Map {
                key: BytecodeTypeId::new(9),
                value: BytecodeTypeId::new(7),
            }
        );
        assert_eq!(
            metadata.types[12],
            BytecodeTraceDescriptor::Set {
                element: BytecodeTypeId::new(9)
            }
        );
        assert_eq!(
            metadata.types[13],
            BytecodeTraceDescriptor::Result {
                success: BytecodeTypeId::new(9),
                error: BytecodeTypeId::new(7),
            }
        );
        assert_eq!(
            metadata.types[14],
            BytecodeTraceDescriptor::Union {
                members: vec![BytecodeTypeId::new(0), BytecodeTypeId::new(9)]
            }
        );
        assert_eq!(
            metadata.types[15],
            BytecodeTraceDescriptor::Range {
                element: BytecodeTypeId::new(0)
            }
        );
        assert_eq!(
            metadata.types[16],
            BytecodeTraceDescriptor::Ref {
                value: BytecodeTypeId::new(9)
            }
        );
        assert_eq!(
            metadata.types[17],
            BytecodeTraceDescriptor::Cursor {
                mode: BytecodeCursorMode::Ref,
                collection: BytecodeTypeId::new(7),
            }
        );
        assert_eq!(
            metadata.types[18],
            BytecodeTraceDescriptor::Closure {
                callable: BytecodeCallableId::new(0),
                captures: vec![BytecodeTypeId::new(9), BytecodeTypeId::new(7)],
            }
        );
        assert_eq!(
            metadata.types[19],
            BytecodeTraceDescriptor::Record {
                nominal: BytecodeNominalId::new(1),
                arguments: Vec::new(),
                fields: vec![
                    BytecodeField {
                        member: 0,
                        ty: BytecodeTypeId::new(9),
                    },
                    BytecodeField {
                        member: 1,
                        ty: BytecodeTypeId::new(7),
                    },
                ],
            }
        );
        assert!(matches!(
            &metadata.types[20],
            BytecodeTraceDescriptor::Variant {
                nominal: Some(nominal),
                variants,
                ..
            } if *nominal == BytecodeNominalId::new(2) && variants.len() == 3
        ));
        assert_eq!(metadata.types[21], BytecodeTraceDescriptor::Inline);
        assert_eq!(metadata.types[22], metadata.types[11]);
        assert_eq!(
            metadata.frames,
            [BytecodeFrameTraceDescriptor {
                function: BytecodeFunctionId::new(0),
                slots: vec![
                    BytecodeTypeId::new(11),
                    BytecodeTypeId::new(18),
                    BytecodeTypeId::new(19),
                    BytecodeTypeId::new(16),
                ],
            }]
        );
    }

    #[test]
    fn trace_metadata_rejects_malformed_catalogs_without_panicking() {
        let mut wrong_arity = terminal_program(false);
        let BytecodeTypeKind::Intrinsic { arguments, .. } = &mut wrong_arity.types[7].kind else {
            panic!("fixture must contain an array");
        };
        arguments.clear();
        let error = derive_trace_metadata(&wrong_arity).unwrap_err();
        assert_eq!(error.context(), "type#7");
        assert!(error.message().contains("wrong generic arity"));

        let mut unknown_child = terminal_program(false);
        unknown_child.types[6].kind = BytecodeTypeKind::Option(BytecodeTypeId::new(999));
        let error = derive_trace_metadata(&unknown_child).unwrap_err();
        assert_eq!(error.context(), "type#6");
        assert!(error.message().contains("unknown type"));

        let mut duplicate_environment = trace_program();
        duplicate_environment
            .callables
            .push(duplicate_environment.callables[0].clone());
        let error = derive_trace_metadata(&duplicate_environment).unwrap_err();
        assert_eq!(error.context(), "type#18");
        assert!(error.message().contains("duplicate closure environments"));

        let mut non_generated_environment = trace_program();
        non_generated_environment.types[18].kind =
            BytecodeTypeKind::Tuple(vec![BytecodeTypeId::new(9)]);
        let error = derive_trace_metadata(&non_generated_environment).unwrap_err();
        assert_eq!(error.context(), "callable#0");
        assert!(error.message().contains("generated environment"));

        let mut opaque_cycle = terminal_program(false);
        let first = BytecodeTypeId::new(opaque_cycle.types.len() as u32);
        let second = BytecodeTypeId::new(first.index() + 1);
        opaque_cycle.types.extend([
            BytecodeType {
                name: "cycle-a".into(),
                kind: BytecodeTypeKind::OpaqueResult {
                    identity: "test::cycle-a".into(),
                    arguments: Vec::new(),
                    witness: second,
                    capabilities: BytecodeCapabilitySet::default(),
                },
            },
            BytecodeType {
                name: "cycle-b".into(),
                kind: BytecodeTypeKind::OpaqueResult {
                    identity: "test::cycle-b".into(),
                    arguments: Vec::new(),
                    witness: first,
                    capabilities: BytecodeCapabilitySet::default(),
                },
            },
        ]);
        let error = derive_trace_metadata(&opaque_cycle).unwrap_err();
        assert!(error.message().contains("trace descriptor cycle"));
    }

    #[test]
    fn trace_metadata_resolves_deep_opaque_chains_without_rust_recursion() {
        let mut program = terminal_program(false);
        let start = program.types.len() as u32;
        let depth = 4_096_u32;
        for offset in 0..depth {
            program.types.push(BytecodeType {
                name: format!("opaque-{offset}"),
                kind: BytecodeTypeKind::OpaqueResult {
                    identity: format!("test::opaque-{offset}"),
                    arguments: Vec::new(),
                    witness: if offset + 1 == depth {
                        BytecodeTypeId::new(0)
                    } else {
                        BytecodeTypeId::new(start + offset + 1)
                    },
                    capabilities: BytecodeCapabilitySet::default(),
                },
            });
        }

        let metadata = derive_trace_metadata(&program).unwrap();
        assert!(
            metadata.types[start as usize..]
                .iter()
                .all(|descriptor| *descriptor == BytecodeTraceDescriptor::Inline)
        );
    }

    #[test]
    fn opaque_bytecode_cannot_hide_a_terminal_witness() {
        let error = verify_bytecode(&terminal_program(true)).unwrap_err();
        assert_eq!(error.context(), "type#8");
        assert!(
            error
                .message()
                .contains("opaque result witness retains a terminal obligation")
        );
    }

    #[test]
    fn collection_loan_paths_rederive_static_disjunction() {
        let split = BytecodeSlotId::new(1);
        let dynamic = BytecodeSlotId::new(2);
        let static_integers = BTreeMap::from([(split, 2)]);
        let left = vec![MovePathComponent::Slice {
            start: None,
            end: Some(split),
            step: None,
        }];
        let right = vec![MovePathComponent::Slice {
            start: Some(split),
            end: None,
            step: None,
        }];
        assert!(!loan_paths_overlap(&left, &right, &static_integers));
        assert!(loan_paths_overlap(
            &left,
            &[MovePathComponent::Slice {
                start: None,
                end: None,
                step: None,
            }],
            &static_integers,
        ));
        assert!(loan_paths_overlap(
            &[MovePathComponent::Index {
                index: dynamic,
                access: BytecodeIndexAccess::Array,
            }],
            &[MovePathComponent::Index {
                index: split,
                access: BytecodeIndexAccess::Array,
            }],
            &static_integers,
        ));
        assert!(!loan_paths_overlap(
            &[MovePathComponent::ArrayPatternRest {
                start: 1,
                suffix: 0,
            }],
            &[MovePathComponent::ArrayPatternIndex(0)],
            &static_integers,
        ));
    }
}
