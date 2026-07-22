use std::collections::{BTreeMap, BTreeSet};

use crate::package::SymbolIdentity;
use crate::resolve::{ResolvedProgram, SymbolId};
use crate::types::{
    CursorMode, IntrinsicType, ScalarType, TypeError, TypeId, TypeInterner, TypeKind,
    TypeSubstitution,
};

use super::{
    HirCapability, HirCapabilityStatus, HirGenericParameter, HirNominalShape, HirProgram,
    HirTraitConstructor, HirTraitReference, HirTypeDeclarationKind, HirVariantPayload,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct CapabilityAssumptions {
    parameters: BTreeMap<u32, BTreeSet<HirCapability>>,
}

impl CapabilityAssumptions {
    pub(crate) fn from_generics(program: &HirProgram, parameters: &[HirGenericParameter]) -> Self {
        let mut assumptions = Self::default();
        for parameter in parameters {
            let capabilities = assumptions
                .parameters
                .entry(parameter.position)
                .or_default();
            for bound in &parameter.bounds {
                match &bound.constructor {
                    HirTraitConstructor::Prelude(name) => {
                        for capability in HirCapability::ALL {
                            if named_bound_implies(name.as_str(), capability) {
                                capabilities.insert(capability);
                            }
                        }
                    }
                    HirTraitConstructor::Symbol(symbol)
                        if trait_requires_self_send(program, *symbol) =>
                    {
                        capabilities.insert(HirCapability::Send);
                    }
                    HirTraitConstructor::Symbol(_) | HirTraitConstructor::External(_) => {}
                }
            }
        }
        assumptions
    }

    pub(crate) fn status(&self, position: u32, capability: HirCapability) -> HirCapabilityStatus {
        let Some(capabilities) = self.parameters.get(&position) else {
            return HirCapabilityStatus::Deferred;
        };
        if capabilities.contains(&capability) {
            HirCapabilityStatus::Satisfied
        } else {
            HirCapabilityStatus::Unsatisfied
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapabilityRequirement {
    floor: HirCapabilityStatus,
    parameters: BTreeSet<(u32, HirCapability)>,
}

impl Default for CapabilityRequirement {
    fn default() -> Self {
        Self {
            floor: HirCapabilityStatus::Satisfied,
            parameters: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct CapabilityNode {
    floor: HirCapabilityStatus,
    dependencies: Vec<(TypeId, HirCapability)>,
}

#[derive(Clone, Debug)]
pub(crate) struct CapabilityAnalysis {
    by_identity: BTreeMap<SymbolIdentity, SymbolId>,
    summaries: BTreeMap<(SymbolId, HirCapability), CapabilityRequirement>,
}

impl CapabilityAnalysis {
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
        let summaries = Self::compute_summaries(program, &by_identity)?;
        Ok(Self {
            by_identity,
            summaries,
        })
    }

    pub(crate) fn status(
        &self,
        program: &HirProgram,
        root: TypeId,
        capability: HirCapability,
        assumptions: &CapabilityAssumptions,
    ) -> Result<HirCapabilityStatus, TypeError> {
        let mut interner = program.interner.clone();
        let mut nodes = BTreeMap::<(TypeId, HirCapability), CapabilityNode>::new();
        let mut pending = vec![(root, capability)];
        while let Some(key @ (ty, capability)) = pending.pop() {
            if nodes.contains_key(&key) {
                continue;
            }
            let mut node = self.node(program, &mut interner, ty, capability, assumptions)?;
            node.dependencies.sort_unstable();
            node.dependencies.dedup();
            pending.extend(node.dependencies.iter().copied());
            nodes.insert(key, node);
        }

        let mut statuses = nodes
            .iter()
            .map(|(key, node)| (*key, node.floor))
            .collect::<BTreeMap<_, _>>();
        let mut users = nodes
            .keys()
            .copied()
            .map(|key| (key, Vec::new()))
            .collect::<BTreeMap<_, Vec<(TypeId, HirCapability)>>>();
        for (user, node) in &nodes {
            for dependency in &node.dependencies {
                users
                    .get_mut(dependency)
                    .expect("all capability dependencies are indexed")
                    .push(*user);
            }
        }
        let mut changed = statuses
            .iter()
            .filter_map(|(key, status)| (*status != HirCapabilityStatus::Satisfied).then_some(*key))
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
                    .expect("all capability graph users have a status");
                if next > *current {
                    *current = next;
                    changed.insert(*user);
                }
            }
        }
        Ok(statuses[&(root, capability)])
    }

    fn compute_summaries(
        program: &HirProgram,
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    ) -> Result<BTreeMap<(SymbolId, HirCapability), CapabilityRequirement>, TypeError> {
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
            .flat_map(|symbol| {
                HirCapability::ALL.into_iter().map(move |capability| {
                    ((*symbol, capability), CapabilityRequirement::default())
                })
            })
            .collect::<BTreeMap<_, _>>();
        let mut users = roots
            .keys()
            .copied()
            .map(|symbol| (symbol, BTreeSet::new()))
            .collect::<BTreeMap<_, BTreeSet<SymbolId>>>();
        for (user, types) in &roots {
            for dependency in nominal_references(program, types, by_identity)? {
                users.entry(dependency).or_default().insert(*user);
            }
        }

        let mut pending = roots
            .keys()
            .flat_map(|symbol| {
                HirCapability::ALL
                    .into_iter()
                    .map(move |capability| (*symbol, capability))
            })
            .collect::<BTreeSet<_>>();
        while let Some(key @ (symbol, capability)) = pending.pop_first() {
            let next = capability_requirement(
                program,
                &roots[&symbol],
                capability,
                by_identity,
                &summaries,
            )?;
            if summaries[&key] == next {
                continue;
            }
            summaries.insert(key, next);
            for user in users.get(&symbol).into_iter().flatten() {
                pending.extend(
                    HirCapability::ALL
                        .into_iter()
                        .map(|capability| (*user, capability)),
                );
            }
        }
        Ok(summaries)
    }

    fn node(
        &self,
        program: &HirProgram,
        interner: &mut TypeInterner,
        ty: TypeId,
        capability: HirCapability,
        assumptions: &CapabilityAssumptions,
    ) -> Result<CapabilityNode, TypeError> {
        let satisfied = |dependencies| CapabilityNode {
            floor: HirCapabilityStatus::Satisfied,
            dependencies,
        };
        let fixed = |floor| CapabilityNode {
            floor,
            dependencies: Vec::new(),
        };
        let node = match interner.kind(ty)?.clone() {
            TypeKind::Error => satisfied(Vec::new()),
            TypeKind::Scalar(scalar) => fixed(scalar_status(scalar, capability)),
            TypeKind::Function(_) => fixed(function_status(capability)),
            TypeKind::Tuple(items) | TypeKind::Union(items) => {
                satisfied(items.into_iter().map(|item| (item, capability)).collect())
            }
            TypeKind::Option(item) => satisfied(vec![(item, capability)]),
            TypeKind::Result { success, error } => {
                satisfied(vec![(success, capability), (error, capability)])
            }
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => intrinsic_node(constructor, &arguments, capability),
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = self.by_identity.get(&identity) else {
                    return Ok(fixed(HirCapabilityStatus::Deferred));
                };
                let summary = &self.summaries[&(*symbol, capability)];
                let mut dependencies = Vec::with_capacity(summary.parameters.len());
                for (position, required) in &summary.parameters {
                    let Some(argument) = arguments.get(*position as usize) else {
                        return Ok(fixed(HirCapabilityStatus::Deferred));
                    };
                    dependencies.push((*argument, *required));
                }
                CapabilityNode {
                    floor: summary.floor,
                    dependencies,
                }
            }
            TypeKind::GenericParameter(position) => fixed(assumptions.status(position, capability)),
            TypeKind::OpaqueResult { identity, .. } => fixed(
                if program.opaque_result(&identity).is_some_and(|opaque| {
                    bounds_imply_capability(program, &opaque.bounds, capability)
                }) {
                    HirCapabilityStatus::Satisfied
                } else {
                    HirCapabilityStatus::Unsatisfied
                },
            ),
            TypeKind::Generated {
                identity,
                arguments,
            } => {
                let Some(closure) = program.closure_by_identity(&identity) else {
                    return Ok(fixed(HirCapabilityStatus::Deferred));
                };
                if matches!(capability, HirCapability::Equatable | HirCapability::Key) {
                    fixed(HirCapabilityStatus::Unsatisfied)
                } else {
                    let substitution = TypeSubstitution::new(arguments);
                    let dependencies = closure
                        .captures()
                        .iter()
                        .map(|capture| {
                            substitution
                                .apply(interner, capture.ty())
                                .map(|ty| (ty, capability))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    satisfied(dependencies)
                }
            }
            TypeKind::Cursor { mode, collection } => cursor_node(mode, collection, capability),
            TypeKind::Inference(_) => fixed(HirCapabilityStatus::Deferred),
        };
        Ok(node)
    }
}

pub(crate) fn bounds_imply(bounds: &[HirTraitReference], capability: HirCapability) -> bool {
    bounds.iter().any(|bound| {
        matches!(
            bound.constructor(),
            HirTraitConstructor::Prelude(name)
                if named_bound_implies(name.as_str(), capability)
                    && bound.arguments().is_empty()
        )
    })
}

fn bounds_imply_capability(
    program: &HirProgram,
    bounds: &[HirTraitReference],
    capability: HirCapability,
) -> bool {
    bounds_imply(bounds, capability)
        || (capability == HirCapability::Send
            && bounds.iter().any(|bound| {
                matches!(
                    bound.constructor(),
                    HirTraitConstructor::Symbol(symbol)
                        if trait_requires_self_send(program, *symbol)
                )
            }))
}

fn trait_requires_self_send(program: &HirProgram, symbol: SymbolId) -> bool {
    let Some(declaration) = program.declaration(symbol) else {
        return false;
    };
    let HirTypeDeclarationKind::Trait(definition) = declaration.kind() else {
        return false;
    };
    definition
        .methods()
        .iter()
        .any(super::HirTraitMethod::requires_self_send)
}

pub(crate) fn named_bound_implies(name: &str, capability: HirCapability) -> bool {
    match capability {
        HirCapability::Discard => matches!(name, "Discard" | "Copy" | "Key"),
        HirCapability::Copy => matches!(name, "Copy" | "Key"),
        HirCapability::Equatable => matches!(name, "Equatable" | "Key"),
        HirCapability::Key => name == "Key",
        HirCapability::Send => name == "Send",
        HirCapability::Share => name == "Share",
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

fn nominal_references(
    program: &HirProgram,
    roots: &[TypeId],
    by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
) -> Result<BTreeSet<SymbolId>, TypeError> {
    let mut references = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending = roots.to_vec();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        match program.interner.kind(ty)? {
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                if let Some(symbol) = by_identity.get(identity) {
                    references.insert(*symbol);
                }
                pending.extend(arguments.iter().copied());
            }
            TypeKind::Tuple(items) | TypeKind::Union(items) => {
                pending.extend(items.iter().copied());
            }
            TypeKind::Option(item) => pending.push(*item),
            TypeKind::Result { success, error } => {
                pending.push(*success);
                pending.push(*error);
            }
            TypeKind::Intrinsic { arguments, .. }
            | TypeKind::Generated { arguments, .. }
            | TypeKind::OpaqueResult { arguments, .. } => {
                pending.extend(arguments.iter().copied());
            }
            TypeKind::Cursor { collection, .. } => pending.push(*collection),
            TypeKind::Error
            | TypeKind::Scalar(_)
            | TypeKind::Function(_)
            | TypeKind::GenericParameter(_)
            | TypeKind::Inference(_) => {}
        }
    }
    Ok(references)
}

fn capability_requirement(
    program: &HirProgram,
    roots: &[TypeId],
    capability: HirCapability,
    by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    summaries: &BTreeMap<(SymbolId, HirCapability), CapabilityRequirement>,
) -> Result<CapabilityRequirement, TypeError> {
    let mut requirement = CapabilityRequirement::default();
    let mut visited = BTreeSet::new();
    let mut pending = roots
        .iter()
        .copied()
        .map(|ty| (ty, capability))
        .collect::<Vec<_>>();
    while let Some(key @ (ty, capability)) = pending.pop() {
        if !visited.insert(key) {
            continue;
        }
        match program.interner.kind(ty)?.clone() {
            TypeKind::Error => {}
            TypeKind::Scalar(scalar) => {
                requirement.floor = requirement.floor.max(scalar_status(scalar, capability));
            }
            TypeKind::Function(_) => {
                requirement.floor = requirement.floor.max(function_status(capability));
            }
            TypeKind::Tuple(items) | TypeKind::Union(items) => {
                pending.extend(items.into_iter().map(|item| (item, capability)));
            }
            TypeKind::Option(item) => pending.push((item, capability)),
            TypeKind::Result { success, error } => {
                pending.push((success, capability));
                pending.push((error, capability));
            }
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                let node = intrinsic_node(constructor, &arguments, capability);
                requirement.floor = requirement.floor.max(node.floor);
                pending.extend(node.dependencies);
            }
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = by_identity.get(&identity) else {
                    requirement.floor = requirement.floor.max(HirCapabilityStatus::Deferred);
                    continue;
                };
                let summary = &summaries[&(*symbol, capability)];
                requirement.floor = requirement.floor.max(summary.floor);
                for (position, required) in &summary.parameters {
                    if let Some(argument) = arguments.get(*position as usize) {
                        pending.push((*argument, *required));
                    } else {
                        requirement.floor = requirement.floor.max(HirCapabilityStatus::Deferred);
                    }
                }
            }
            TypeKind::GenericParameter(position) => {
                requirement.parameters.insert((position, capability));
            }
            TypeKind::OpaqueResult { identity, .. } => {
                if !program.opaque_result(&identity).is_some_and(|opaque| {
                    bounds_imply_capability(program, &opaque.bounds, capability)
                }) {
                    requirement.floor = requirement.floor.max(HirCapabilityStatus::Unsatisfied);
                }
            }
            TypeKind::Cursor { mode, collection } => {
                let node = cursor_node(mode, collection, capability);
                requirement.floor = requirement.floor.max(node.floor);
                pending.extend(node.dependencies);
            }
            TypeKind::Inference(_) | TypeKind::Generated { .. } => {
                requirement.floor = requirement.floor.max(HirCapabilityStatus::Deferred);
            }
        }
    }
    Ok(requirement)
}

fn scalar_status(scalar: ScalarType, capability: HirCapability) -> HirCapabilityStatus {
    if capability == HirCapability::Key && matches!(scalar, ScalarType::Float | ScalarType::Float32)
    {
        HirCapabilityStatus::Unsatisfied
    } else {
        HirCapabilityStatus::Satisfied
    }
}

fn function_status(capability: HirCapability) -> HirCapabilityStatus {
    if matches!(
        capability,
        HirCapability::Copy | HirCapability::Discard | HirCapability::Send | HirCapability::Share
    ) {
        HirCapabilityStatus::Satisfied
    } else {
        HirCapabilityStatus::Unsatisfied
    }
}

fn cursor_node(mode: CursorMode, collection: TypeId, capability: HirCapability) -> CapabilityNode {
    let satisfied = |dependencies| CapabilityNode {
        floor: HirCapabilityStatus::Satisfied,
        dependencies,
    };
    let unsatisfied = || CapabilityNode {
        floor: HirCapabilityStatus::Unsatisfied,
        dependencies: Vec::new(),
    };
    match (mode, capability) {
        (_, HirCapability::Equatable | HirCapability::Key) => unsatisfied(),
        (CursorMode::Ref, HirCapability::Copy | HirCapability::Discard) => satisfied(Vec::new()),
        (CursorMode::Ref, HirCapability::Send | HirCapability::Share) => satisfied(vec![
            (collection, HirCapability::Send),
            (collection, HirCapability::Share),
        ]),
        (CursorMode::Own, capability) => satisfied(vec![(collection, capability)]),
    }
}

fn intrinsic_node(
    constructor: IntrinsicType,
    arguments: &[TypeId],
    capability: HirCapability,
) -> CapabilityNode {
    let satisfied = |dependencies| CapabilityNode {
        floor: HirCapabilityStatus::Satisfied,
        dependencies,
    };
    let fixed = |floor| CapabilityNode {
        floor,
        dependencies: Vec::new(),
    };
    let same = || {
        satisfied(
            arguments
                .iter()
                .copied()
                .map(|argument| (argument, capability))
                .collect(),
        )
    };
    match constructor {
        IntrinsicType::Array => {
            if capability == HirCapability::Key {
                fixed(HirCapabilityStatus::Unsatisfied)
            } else {
                same()
            }
        }
        IntrinsicType::Map => match capability {
            HirCapability::Key => fixed(HirCapabilityStatus::Unsatisfied),
            HirCapability::Copy => satisfied(vec![
                (arguments[0], HirCapability::Key),
                (arguments[1], HirCapability::Copy),
            ]),
            HirCapability::Discard
            | HirCapability::Equatable
            | HirCapability::Send
            | HirCapability::Share => same(),
        },
        IntrinsicType::Set => match capability {
            HirCapability::Key => fixed(HirCapabilityStatus::Unsatisfied),
            HirCapability::Copy => satisfied(vec![(arguments[0], HirCapability::Key)]),
            HirCapability::Discard
            | HirCapability::Equatable
            | HirCapability::Send
            | HirCapability::Share => same(),
        },
        IntrinsicType::Range => {
            if matches!(capability, HirCapability::Equatable | HirCapability::Key) {
                fixed(HirCapabilityStatus::Unsatisfied)
            } else {
                same()
            }
        }
        IntrinsicType::Ref => match capability {
            HirCapability::Copy
            | HirCapability::Discard
            | HirCapability::Equatable
            | HirCapability::Key => satisfied(Vec::new()),
            HirCapability::Send | HirCapability::Share => satisfied(vec![
                (arguments[0], HirCapability::Send),
                (arguments[0], HirCapability::Share),
            ]),
        },
        IntrinsicType::Pointer => {
            if matches!(capability, HirCapability::Copy | HirCapability::Discard) {
                satisfied(Vec::new())
            } else {
                fixed(HirCapabilityStatus::Unsatisfied)
            }
        }
        IntrinsicType::Join => fixed(HirCapabilityStatus::Unsatisfied),
        IntrinsicType::Command | IntrinsicType::Pipeline => {
            if matches!(
                capability,
                HirCapability::Copy
                    | HirCapability::Discard
                    | HirCapability::Send
                    | HirCapability::Share
            ) {
                satisfied(Vec::new())
            } else {
                fixed(HirCapabilityStatus::Unsatisfied)
            }
        }
        IntrinsicType::NumericConversionError => satisfied(Vec::new()),
    }
}
