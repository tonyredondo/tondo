use std::collections::{BTreeMap, BTreeSet};

use crate::package::SymbolIdentity;
use crate::resolve::{ResolvedProgram, SymbolId};
use crate::types::{
    CursorMode, IntrinsicType, TypeError, TypeId, TypeInterner, TypeKind, TypeSubstitution,
};

use super::capabilities::bounds_imply;
use super::{
    CapabilityAssumptions, HirCapability, HirCapabilityStatus, HirNominalShape, HirProgram,
    HirTypeDeclarationKind, HirVariantPayload,
};

/// Whether a type owns a terminal token.
///
/// `Potential` is retained for an unconstrained generic, unresolved inference,
/// or a source-less nominal whose privileged library contract is not present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirTerminalStatus {
    Absent,
    Potential,
    Present,
}

/// The source-visible operation that consumes a direct intrinsic terminal root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirTerminalOperation {
    JoinAwait,
    ProcessFinish,
    TimerFinish,
}

/// The closed fallback used only while unwinding a direct intrinsic root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirTerminalUnwindAction {
    JoinTeardown,
    ProcessCleanup,
    TimerCleanup,
}

/// The complete language-owned contract for one direct terminal root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HirTerminalContract {
    operation: HirTerminalOperation,
    unwind: HirTerminalUnwindAction,
    unwind_may_suspend: bool,
}

impl HirTerminalContract {
    pub fn operation(self) -> HirTerminalOperation {
        self.operation
    }

    pub fn unwind(self) -> HirTerminalUnwindAction {
        self.unwind
    }

    pub fn unwind_may_suspend(self) -> bool {
        self.unwind_may_suspend
    }
}

const JOIN_CONTRACT: HirTerminalContract = HirTerminalContract {
    operation: HirTerminalOperation::JoinAwait,
    unwind: HirTerminalUnwindAction::JoinTeardown,
    unwind_may_suspend: true,
};

const PROCESS_HANDLE_CONTRACT: HirTerminalContract = HirTerminalContract {
    operation: HirTerminalOperation::ProcessFinish,
    unwind: HirTerminalUnwindAction::ProcessCleanup,
    unwind_may_suspend: true,
};

const TIMER_CONTRACT: HirTerminalContract = HirTerminalContract {
    operation: HirTerminalOperation::TimerFinish,
    unwind: HirTerminalUnwindAction::TimerCleanup,
    unwind_may_suspend: false,
};

/// This match is the language-owned terminal registry. Source declarations
/// cannot extend it. Privileged opaque library entries will be supplied by the
/// future standard-library interface catalog rather than by a user trait.
pub(crate) const fn intrinsic_terminal_contract(
    constructor: IntrinsicType,
) -> Option<HirTerminalContract> {
    match constructor {
        IntrinsicType::Join => Some(JOIN_CONTRACT),
        IntrinsicType::ProcessHandle => Some(PROCESS_HANDLE_CONTRACT),
        IntrinsicType::Timer => Some(TIMER_CONTRACT),
        IntrinsicType::Array
        | IntrinsicType::Map
        | IntrinsicType::Set
        | IntrinsicType::Range
        | IntrinsicType::Ref
        | IntrinsicType::Pointer
        | IntrinsicType::Command
        | IntrinsicType::Pipeline
        | IntrinsicType::Bytes
        | IntrinsicType::BytesBuilder
        | IntrinsicType::BytesError
        | IntrinsicType::Path
        | IntrinsicType::PathError
        | IntrinsicType::FsError
        | IntrinsicType::ExitStatus
        | IntrinsicType::ProcessOutput
        | IntrinsicType::ProcessError
        | IntrinsicType::ProcessExitError
        | IntrinsicType::Utf8Error
        | IntrinsicType::NumericConversionError
        | IntrinsicType::Duration
        | IntrinsicType::Instant
        | IntrinsicType::DurationError
        | IntrinsicType::ClockError
        | IntrinsicType::EnvSnapshot
        | IntrinsicType::EnvName
        | IntrinsicType::EnvValue
        | IntrinsicType::EnvError
        | IntrinsicType::VirtualTime => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalRequirement {
    floor: HirTerminalStatus,
    parameters: BTreeSet<u32>,
}

impl Default for TerminalRequirement {
    fn default() -> Self {
        Self {
            floor: HirTerminalStatus::Absent,
            parameters: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct TerminalNode {
    floor: HirTerminalStatus,
    dependencies: Vec<TypeId>,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalAnalysis {
    by_identity: BTreeMap<SymbolIdentity, SymbolId>,
    summaries: BTreeMap<SymbolId, TerminalRequirement>,
}

impl TerminalAnalysis {
    pub(crate) fn new(program: &HirProgram, resolved: &ResolvedProgram) -> Result<Self, TypeError> {
        let by_identity = program
            .declarations
            .iter()
            .filter_map(|(symbol, declaration)| {
                matches!(declaration.kind(), HirTypeDeclarationKind::Nominal(_))
                    .then(|| {
                        resolved
                            .symbol(*symbol)
                            .map(|resolved| (resolved.identity().clone(), *symbol))
                    })
                    .flatten()
            })
            .collect::<BTreeMap<_, _>>();
        let summaries = compute_summaries(program, &by_identity)?;
        Ok(Self {
            by_identity,
            summaries,
        })
    }

    pub(crate) fn status(
        &self,
        program: &HirProgram,
        root: TypeId,
        assumptions: &CapabilityAssumptions,
    ) -> Result<HirTerminalStatus, TypeError> {
        let mut interner = program.interner.clone();
        let mut nodes = BTreeMap::<TypeId, TerminalNode>::new();
        let mut pending = vec![root];
        while let Some(ty) = pending.pop() {
            if nodes.contains_key(&ty) {
                continue;
            }
            let mut node = self.node(program, &mut interner, ty, assumptions)?;
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
            .collect::<BTreeMap<_, Vec<TypeId>>>();
        for (user, node) in &nodes {
            for dependency in &node.dependencies {
                users
                    .get_mut(dependency)
                    .expect("all terminal dependencies are indexed")
                    .push(*user);
            }
        }
        let mut changed = statuses
            .iter()
            .filter_map(|(ty, status)| (*status != HirTerminalStatus::Absent).then_some(*ty))
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
                    .expect("all terminal graph users have a status");
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
        program: &HirProgram,
        interner: &mut TypeInterner,
        ty: TypeId,
        assumptions: &CapabilityAssumptions,
    ) -> Result<TerminalNode, TypeError> {
        let node = match interner.kind(ty)?.clone() {
            TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Function(_) => {
                fixed(HirTerminalStatus::Absent)
            }
            TypeKind::Tuple(items) | TypeKind::Union(items) => dependent(items),
            TypeKind::Option(item) => dependent(vec![item]),
            TypeKind::Result { success, error } => dependent(vec![success, error]),
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => intrinsic_node(constructor, arguments),
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = self.by_identity.get(&identity) else {
                    return Ok(fixed(HirTerminalStatus::Potential));
                };
                let summary = &self.summaries[symbol];
                let mut dependencies = Vec::with_capacity(summary.parameters.len());
                for position in &summary.parameters {
                    let Some(argument) = arguments.get(*position as usize) else {
                        return Ok(fixed(HirTerminalStatus::Potential));
                    };
                    dependencies.push(*argument);
                }
                TerminalNode {
                    floor: summary.floor,
                    dependencies,
                }
            }
            TypeKind::GenericParameter(position) => fixed(
                if assumptions.status(position, HirCapability::Discard)
                    == HirCapabilityStatus::Satisfied
                {
                    HirTerminalStatus::Absent
                } else {
                    HirTerminalStatus::Potential
                },
            ),
            TypeKind::Inference(_) => fixed(HirTerminalStatus::Potential),
            TypeKind::OpaqueResult { identity, .. } => fixed(
                if program
                    .opaque_result(&identity)
                    .is_some_and(|opaque| bounds_imply(&opaque.bounds, HirCapability::Discard))
                {
                    HirTerminalStatus::Absent
                } else {
                    HirTerminalStatus::Potential
                },
            ),
            TypeKind::Generated {
                identity,
                arguments,
            } => {
                let Some(closure) = program.closure_by_identity(&identity) else {
                    return Ok(fixed(HirTerminalStatus::Potential));
                };
                let substitution = TypeSubstitution::new(arguments);
                dependent(
                    closure
                        .captures()
                        .iter()
                        .map(|capture| substitution.apply(interner, capture.ty()))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            TypeKind::Cursor { mode, collection } => match mode {
                CursorMode::Own => dependent(vec![collection]),
                CursorMode::Ref | CursorMode::Mut => fixed(HirTerminalStatus::Absent),
            },
        };
        Ok(node)
    }
}

fn compute_summaries(
    program: &HirProgram,
    by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
) -> Result<BTreeMap<SymbolId, TerminalRequirement>, TypeError> {
    let roots = program
        .declarations
        .iter()
        .filter_map(|(symbol, declaration)| {
            let HirTypeDeclarationKind::Nominal(definition) = declaration.kind() else {
                return None;
            };
            Some((*symbol, nominal_roots(definition.shape())))
        })
        .collect::<BTreeMap<_, _>>();
    let mut summaries = roots
        .keys()
        .copied()
        .map(|symbol| (symbol, TerminalRequirement::default()))
        .collect::<BTreeMap<_, _>>();
    loop {
        let mut changes = Vec::new();
        for (symbol, roots) in &roots {
            let next = terminal_requirement(program, roots, by_identity, &summaries)?;
            if summaries[symbol] != next {
                changes.push((*symbol, next));
            }
        }
        if changes.is_empty() {
            break;
        }
        for (symbol, requirement) in changes {
            summaries.insert(symbol, requirement);
        }
    }
    Ok(summaries)
}

fn terminal_requirement(
    program: &HirProgram,
    roots: &[TypeId],
    by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    summaries: &BTreeMap<SymbolId, TerminalRequirement>,
) -> Result<TerminalRequirement, TypeError> {
    let mut requirement = TerminalRequirement::default();
    let mut pending = roots.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        match program.interner.kind(ty)?.clone() {
            TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Function(_) => {}
            TypeKind::Tuple(items) | TypeKind::Union(items) => pending.extend(items),
            TypeKind::Option(item) => pending.push(item),
            TypeKind::Result { success, error } => {
                pending.push(success);
                pending.push(error);
            }
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                let node = intrinsic_node(constructor, arguments);
                requirement.floor = requirement.floor.max(node.floor);
                pending.extend(node.dependencies);
            }
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = by_identity.get(&identity) else {
                    requirement.floor = requirement.floor.max(HirTerminalStatus::Potential);
                    continue;
                };
                let summary = &summaries[symbol];
                requirement.floor = requirement.floor.max(summary.floor);
                for position in &summary.parameters {
                    if let Some(argument) = arguments.get(*position as usize) {
                        pending.push(*argument);
                    } else {
                        requirement.floor = requirement.floor.max(HirTerminalStatus::Potential);
                    }
                }
            }
            TypeKind::GenericParameter(position) => {
                requirement.parameters.insert(position);
            }
            TypeKind::Inference(_) => {
                requirement.floor = requirement.floor.max(HirTerminalStatus::Potential);
            }
            TypeKind::OpaqueResult { identity, .. } => {
                if !program
                    .opaque_result(&identity)
                    .is_some_and(|opaque| bounds_imply(&opaque.bounds, HirCapability::Discard))
                {
                    requirement.floor = requirement.floor.max(HirTerminalStatus::Potential);
                }
            }
            TypeKind::Generated { .. } => {
                requirement.floor = requirement.floor.max(HirTerminalStatus::Potential);
            }
            TypeKind::Cursor { mode, collection } => {
                if mode == CursorMode::Own {
                    pending.push(collection);
                }
            }
        }
    }
    Ok(requirement)
}

fn intrinsic_node(constructor: IntrinsicType, arguments: Vec<TypeId>) -> TerminalNode {
    if intrinsic_terminal_contract(constructor).is_some() {
        return fixed(HirTerminalStatus::Present);
    }
    match constructor {
        IntrinsicType::Array | IntrinsicType::Map | IntrinsicType::Set | IntrinsicType::Range => {
            dependent(arguments)
        }
        IntrinsicType::Ref
        | IntrinsicType::Pointer
        | IntrinsicType::Command
        | IntrinsicType::Pipeline
        | IntrinsicType::Bytes
        | IntrinsicType::BytesBuilder
        | IntrinsicType::BytesError
        | IntrinsicType::Path
        | IntrinsicType::PathError
        | IntrinsicType::FsError
        | IntrinsicType::ExitStatus
        | IntrinsicType::ProcessOutput
        | IntrinsicType::ProcessError
        | IntrinsicType::ProcessExitError
        | IntrinsicType::Utf8Error
        | IntrinsicType::NumericConversionError => fixed(HirTerminalStatus::Absent),
        IntrinsicType::Duration
        | IntrinsicType::Instant
        | IntrinsicType::DurationError
        | IntrinsicType::ClockError
        | IntrinsicType::EnvSnapshot
        | IntrinsicType::EnvName
        | IntrinsicType::EnvValue
        | IntrinsicType::EnvError
        | IntrinsicType::VirtualTime => fixed(HirTerminalStatus::Absent),
        IntrinsicType::Join | IntrinsicType::ProcessHandle | IntrinsicType::Timer => {
            unreachable!("registered terminal roots return above")
        }
    }
}

fn fixed(floor: HirTerminalStatus) -> TerminalNode {
    TerminalNode {
        floor,
        dependencies: Vec::new(),
    }
}

fn dependent(dependencies: Vec<TypeId>) -> TerminalNode {
    TerminalNode {
        floor: HirTerminalStatus::Absent,
        dependencies,
    }
}

fn nominal_roots(shape: &HirNominalShape) -> Vec<TypeId> {
    match shape {
        HirNominalShape::Newtype { underlying } => vec![*underlying],
        HirNominalShape::Record { fields } => fields.iter().map(|field| field.ty()).collect(),
        HirNominalShape::Enum { variants } => variants
            .iter()
            .flat_map(|variant| match variant.payload() {
                HirVariantPayload::Unit => Vec::new(),
                HirVariantPayload::Tuple(items) => items.clone(),
                HirVariantPayload::Record(fields) => {
                    fields.iter().map(|field| field.ty()).collect()
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrinsic_terminal_registry_is_sealed_to_language_resources() {
        let constructors = [
            IntrinsicType::Array,
            IntrinsicType::Map,
            IntrinsicType::Set,
            IntrinsicType::Range,
            IntrinsicType::Ref,
            IntrinsicType::Pointer,
            IntrinsicType::Join,
            IntrinsicType::Command,
            IntrinsicType::Pipeline,
            IntrinsicType::Bytes,
            IntrinsicType::BytesBuilder,
            IntrinsicType::BytesError,
            IntrinsicType::Path,
            IntrinsicType::PathError,
            IntrinsicType::FsError,
            IntrinsicType::ExitStatus,
            IntrinsicType::ProcessOutput,
            IntrinsicType::ProcessHandle,
            IntrinsicType::ProcessError,
            IntrinsicType::ProcessExitError,
            IntrinsicType::Utf8Error,
            IntrinsicType::NumericConversionError,
            IntrinsicType::Duration,
            IntrinsicType::Instant,
            IntrinsicType::Timer,
            IntrinsicType::DurationError,
            IntrinsicType::ClockError,
        ];
        let registered = constructors
            .into_iter()
            .filter_map(intrinsic_terminal_contract)
            .collect::<Vec<_>>();
        assert_eq!(
            registered,
            [JOIN_CONTRACT, PROCESS_HANDLE_CONTRACT, TIMER_CONTRACT]
        );
    }
}
