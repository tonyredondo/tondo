use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tondo_vm::bytecode as bc;

use super::{BytecodeError, BytecodeLoweringLimits};
use crate::hir::{
    HirCallProtocol, HirCallableId, HirClosureId, HirConstantValue, HirConstantValueKind,
    HirConstantVariantValue, HirNominalShape, HirPreludeTraitMethod, HirProgram,
    HirTraitConstructor, HirTraitMethodKey, HirTypeDeclarationKind, HirVariantPayload, TraitQuery,
    TraitSelectionError, select_implementation,
};
use crate::mir::{
    MirAggregateKind, MirBasicBlock, MirBlockKind, MirCallArgument, MirConstant, MirFunction,
    MirLocalKind, MirOperand, MirOperandKind, MirOperation, MirOperationKind, MirPlace, MirProgram,
    MirProjection, MirProjectionKind, MirRvalue, MirRvalueKind, MirStatement, MirStatementKind,
    MirTag, MirTerminator, MirTerminatorKind, verify_mir,
};
use crate::resolve::{MemberOwner, ResolvedProgram, SymbolId, SymbolKind};
use crate::source::Span;
use crate::types::{
    Assignability, CursorMode, FunctionType, IntrinsicType, NumericConversion, ParameterMode,
    ScalarType, TypeError, TypeId, TypeInterner, TypeKind, TypeSubstitution,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CallableInstance {
    callable: HirCallableId,
    arguments: Vec<TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClosureInstance {
    closure: HirClosureId,
    arguments: Vec<TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ExecutableInstance {
    Named(CallableInstance),
    Closure(ClosureInstance),
}

impl ExecutableInstance {
    fn arguments(&self) -> &[TypeId] {
        match self {
            Self::Named(instance) => &instance.arguments,
            Self::Closure(instance) => &instance.arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PreludeTraitInstance {
    method: HirPreludeTraitMethod,
    arguments: Vec<TypeId>,
}

#[derive(Debug, Clone)]
enum FunctionReference {
    Callable {
        callable: HirCallableId,
        arguments: Vec<TypeId>,
    },
    PreludeTrait {
        method: HirPreludeTraitMethod,
        arguments: Vec<TypeId>,
    },
    Closure {
        closure: HirClosureId,
        arguments: Vec<TypeId>,
    },
}

struct Monomorphization {
    interner: TypeInterner,
    callables: Vec<ExecutableInstance>,
    functions: Vec<ExecutableInstance>,
    type_maps: BTreeMap<ExecutableInstance, BTreeMap<TypeId, TypeId>>,
    dispatches: BTreeMap<CallableInstance, CallableInstance>,
    prelude_dispatches: BTreeMap<PreludeTraitInstance, CallableInstance>,
}

pub fn lower_to_bytecode(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    mir: &MirProgram,
    limits: BytecodeLoweringLimits,
) -> Result<bc::BytecodeProgram, BytecodeError> {
    verify_mir(resolved, hir, mir).map_err(|error| {
        BytecodeError::construction("MIR admission", format!("input MIR is invalid: {error}"))
    })?;

    let mut monomorphization = monomorphize(resolved, hir, mir, limits.max_generic_instantiations)?;
    let nominal_ids = nominal_ids(hir, limits.max_nominals)?;
    let callable_ids = callable_ids(&monomorphization.callables, limits.max_callables)?;
    let function_ids = function_ids(&monomorphization.functions, limits.max_functions)?;
    let constant_ids = constant_ids(hir, limits.max_constants)?;
    let mut catalog = TypeCatalog::build(
        &mut monomorphization.interner,
        hir,
        &monomorphization.type_maps,
        limits.max_types,
    )?;
    catalog.attach_nominal_ids(resolved, &nominal_ids);

    let nominals = lower_nominals(hir, &catalog, &nominal_ids)?;
    let callables = lower_callables(
        resolved,
        hir,
        &monomorphization,
        &catalog,
        &callable_ids,
        &function_ids,
    )?;
    let constants = lower_constants(
        resolved,
        hir,
        &catalog,
        &nominal_ids,
        &callable_ids,
        &monomorphization.dispatches,
        &constant_ids,
    )?;
    let functions = {
        let context = FunctionLoweringContext {
            hir,
            catalog: &catalog,
            nominal_ids: &nominal_ids,
            callable_ids: &callable_ids,
            dispatches: &monomorphization.dispatches,
            prelude_dispatches: &monomorphization.prelude_dispatches,
            constant_ids: &constant_ids,
        };
        monomorphization
            .functions
            .iter()
            .map(|instance| {
                let function = mir_function(mir, instance).ok_or_else(|| {
                    BytecodeError::construction(
                        "monomorphization",
                        format!("{instance:?} has no MIR template"),
                    )
                })?;
                lower_function(
                    instance,
                    function,
                    &context,
                    monomorphization
                        .type_maps
                        .get(instance)
                        .expect("every function instance has a type map"),
                    limits,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let program = bc::BytecodeProgram {
        types: catalog.types,
        nominals,
        callables,
        constants,
        functions,
    };
    match bc::verify_bytecode_with_limits(
        &program,
        bc::BytecodeVerificationLimits {
            max_dataflow_steps: limits.max_verification_steps,
        },
    ) {
        Ok(()) => Ok(program),
        Err(error) if error.is_resource_limit() => Err(BytecodeError::VerificationLimit {
            resource: "verification dataflow",
        }),
        Err(error) => Err(BytecodeError::Invariant(error)),
    }
}

fn monomorphize(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    mir: &MirProgram,
    generic_limit: u32,
) -> Result<Monomorphization, BytecodeError> {
    let mut interner = hir.interner().clone();
    let mut callables = BTreeSet::new();
    let mut functions = BTreeSet::new();
    let mut pending = BTreeSet::new();
    let mut dispatches = BTreeMap::new();
    let mut prelude_dispatches = BTreeMap::new();
    let mut generic_count = 0usize;

    for callable in hir
        .callables()
        .filter(|callable| callable.generic_arity() == 0)
    {
        register_instance(
            hir,
            mir,
            &interner,
            ExecutableInstance::Named(CallableInstance {
                callable: callable.id(),
                arguments: Vec::new(),
            }),
            generic_limit,
            &mut generic_count,
            &mut callables,
            &mut functions,
            &mut pending,
        )?;
    }
    for closure in hir
        .closures()
        .filter(|closure| closure.generic_arity() == 0)
    {
        register_instance(
            hir,
            mir,
            &interner,
            ExecutableInstance::Closure(ClosureInstance {
                closure: closure.id(),
                arguments: Vec::new(),
            }),
            generic_limit,
            &mut generic_count,
            &mut callables,
            &mut functions,
            &mut pending,
        )?;
    }
    for (_, constant) in hir.constants() {
        let Some(value) = constant.evaluated() else {
            continue;
        };
        let mut references = Vec::new();
        collect_constant_function_references(value, &mut references);
        for (callable, arguments) in references {
            register_reference(
                resolved,
                hir,
                mir,
                &mut interner,
                CallableInstance {
                    callable,
                    arguments,
                },
                generic_limit,
                &mut generic_count,
                &mut callables,
                &mut functions,
                &mut pending,
                &mut dispatches,
            )?;
        }
    }

    while let Some(instance) = pending.pop_first() {
        let function = mir_function(mir, &instance).ok_or_else(|| {
            BytecodeError::construction(
                "monomorphization",
                format!("{instance:?} has no MIR template"),
            )
        })?;
        let substitution = TypeSubstitution::new(instance.arguments().to_vec());
        let mut references = Vec::new();
        collect_function_references(function, &mut references);
        for reference in references {
            let templates = match &reference {
                FunctionReference::Callable { arguments, .. }
                | FunctionReference::PreludeTrait { arguments, .. }
                | FunctionReference::Closure { arguments, .. } => arguments,
            };
            let arguments = templates
                .iter()
                .map(|template| {
                    substitution
                        .apply(&mut interner, *template)
                        .map_err(|error| {
                            monomorphization_type_error(
                                error,
                                Some(function.span()),
                                format!("cannot specialize {template}"),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            match reference {
                FunctionReference::Callable { callable, .. } => register_reference(
                    resolved,
                    hir,
                    mir,
                    &mut interner,
                    CallableInstance {
                        callable,
                        arguments,
                    },
                    generic_limit,
                    &mut generic_count,
                    &mut callables,
                    &mut functions,
                    &mut pending,
                    &mut dispatches,
                )?,
                FunctionReference::PreludeTrait { method, .. } => register_prelude_reference(
                    hir,
                    mir,
                    &mut interner,
                    PreludeTraitInstance { method, arguments },
                    generic_limit,
                    &mut generic_count,
                    &mut callables,
                    &mut functions,
                    &mut pending,
                    &mut prelude_dispatches,
                )?,
                FunctionReference::Closure { closure, .. } => register_instance(
                    hir,
                    mir,
                    &interner,
                    ExecutableInstance::Closure(ClosureInstance { closure, arguments }),
                    generic_limit,
                    &mut generic_count,
                    &mut callables,
                    &mut functions,
                    &mut pending,
                )?,
            }
        }
    }

    let callables = callables.into_iter().collect::<Vec<_>>();
    let functions = functions.into_iter().collect::<Vec<_>>();
    let function_set = functions.iter().cloned().collect::<BTreeSet<_>>();
    let mut type_maps = BTreeMap::new();
    for instance in &callables {
        let (span, mut templates) = match instance {
            ExecutableInstance::Named(instance) => {
                let signature = hir.callable(instance.callable).ok_or_else(|| {
                    BytecodeError::construction(
                        "monomorphization",
                        format!("{instance:?} has no HIR signature"),
                    )
                })?;
                let mut templates =
                    BTreeSet::from([signature.outcome(), signature.function_type()]);
                for parameter in signature.parameters() {
                    templates.insert(parameter.ty());
                    if let Some(element) = parameter.variadic_element() {
                        templates.insert(element);
                    }
                }
                (signature.span(), templates)
            }
            ExecutableInstance::Closure(instance) => {
                let closure = hir.closure(instance.closure).ok_or_else(|| {
                    BytecodeError::construction(
                        "monomorphization",
                        format!("{instance:?} has no HIR closure metadata"),
                    )
                })?;
                let mut templates = BTreeSet::from([closure.ty(), closure.function_type()]);
                templates.extend(closure.captures().iter().map(|capture| capture.ty()));
                for parameter in closure.parameters() {
                    templates.insert(parameter.ty());
                    if let Some(element) = parameter.variadic_element() {
                        templates.insert(element);
                    }
                }
                (closure.span(), templates)
            }
        };
        if function_set.contains(instance) {
            collect_function_types(
                mir_function(mir, instance)
                    .expect("registered function instances have a MIR template"),
                &mut templates,
            );
        }
        let substitution = TypeSubstitution::new(instance.arguments().to_vec());
        let mut map = BTreeMap::new();
        for template in templates {
            let concrete = substitution
                .apply(&mut interner, template)
                .map_err(|error| {
                    monomorphization_type_error(
                        error,
                        Some(span),
                        format!("cannot specialize {template}"),
                    )
                })?;
            if type_contains_generic_parameter(&interner, concrete)? {
                return Err(BytecodeError::construction(
                    "monomorphization",
                    format!("{instance:?} leaves {template} generic"),
                ));
            }
            map.insert(template, concrete);
        }
        type_maps.insert(instance.clone(), map);
    }

    Ok(Monomorphization {
        interner,
        callables,
        functions,
        type_maps,
        dispatches,
        prelude_dispatches,
    })
}

fn monomorphization_type_error(
    error: TypeError,
    span: Option<Span>,
    context: impl Into<String>,
) -> BytecodeError {
    match error {
        TypeError::ResourceLimit { .. } => BytecodeError::NodeLimit {
            span,
            resource: "specialized type nodes",
        },
        error => BytecodeError::construction(context, error.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn register_reference(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    mir: &MirProgram,
    interner: &mut TypeInterner,
    reference: CallableInstance,
    generic_limit: u32,
    generic_count: &mut usize,
    callables: &mut BTreeSet<ExecutableInstance>,
    functions: &mut BTreeSet<ExecutableInstance>,
    pending: &mut BTreeSet<ExecutableInstance>,
    dispatches: &mut BTreeMap<CallableInstance, CallableInstance>,
) -> Result<(), BytecodeError> {
    let target = resolve_source_trait_dispatch(resolved, hir, interner, &reference)?;
    let target = if let Some(target) = target {
        if let Some(existing) = dispatches.get(&reference) {
            if existing != &target {
                return Err(BytecodeError::construction(
                    "trait dispatch",
                    format!("{reference:?} resolved inconsistently to {existing:?} and {target:?}"),
                ));
            }
        } else {
            dispatches.insert(reference, target.clone());
        }
        target
    } else {
        reference
    };
    register_instance(
        hir,
        mir,
        interner,
        ExecutableInstance::Named(target),
        generic_limit,
        generic_count,
        callables,
        functions,
        pending,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_prelude_reference(
    hir: &HirProgram,
    mir: &MirProgram,
    interner: &mut TypeInterner,
    reference: PreludeTraitInstance,
    generic_limit: u32,
    generic_count: &mut usize,
    callables: &mut BTreeSet<ExecutableInstance>,
    functions: &mut BTreeSet<ExecutableInstance>,
    pending: &mut BTreeSet<ExecutableInstance>,
    dispatches: &mut BTreeMap<PreludeTraitInstance, CallableInstance>,
) -> Result<(), BytecodeError> {
    let target = resolve_prelude_trait_dispatch(hir, interner, &reference)?;
    if let Some(existing) = dispatches.get(&reference) {
        if existing != &target {
            return Err(BytecodeError::construction(
                "trait dispatch",
                format!("{reference:?} resolved inconsistently to {existing:?} and {target:?}"),
            ));
        }
    } else {
        dispatches.insert(reference, target.clone());
    }
    register_instance(
        hir,
        mir,
        interner,
        ExecutableInstance::Named(target),
        generic_limit,
        generic_count,
        callables,
        functions,
        pending,
    )
}

fn resolve_prelude_trait_dispatch(
    hir: &HirProgram,
    interner: &mut TypeInterner,
    reference: &PreludeTraitInstance,
) -> Result<CallableInstance, BytecodeError> {
    let query = reference
        .method
        .query(&reference.arguments)
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "prelude method {:?} has {} type arguments instead of {}",
                    reference.method,
                    reference.arguments.len(),
                    reference.method.generic_arity()
                ),
            )
        })?;
    let query = TraitQuery::from_parts(
        query.constructor().clone(),
        query.arguments().to_vec(),
        concrete_trait_target(hir, interner, query.target())?,
    );
    let selection = select_implementation(interner, hir.implementations(), &query)
        .map_err(prelude_trait_dispatch_selection_error)?
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "prelude method {}.{} has no implementation for its concrete query",
                    reference.method.trait_name(),
                    reference.method.method_name()
                ),
            )
        })?;
    let implementation = hir
        .implementation(selection.implementation())
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "selected implementation#{} is not indexed",
                    selection.implementation().index()
                ),
            )
        })?;
    let key = HirTraitMethodKey::Prelude(reference.method);
    let method = implementation
        .methods()
        .iter()
        .find(|method| {
            method
                .contract()
                .is_some_and(|contract| contract.method() == key)
        })
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "implementation#{} does not provide prelude method {}.{}",
                    implementation.id().index(),
                    reference.method.trait_name(),
                    reference.method.method_name()
                ),
            )
        })?;
    let target = CallableInstance {
        callable: HirCallableId::Implementation(method.id()),
        arguments: selection.arguments().to_vec(),
    };
    verify_prelude_dispatch_signature(hir, interner, reference, &target, method.span())?;
    Ok(target)
}

fn prelude_trait_dispatch_selection_error(error: TraitSelectionError) -> BytecodeError {
    match error {
        TraitSelectionError::Type(error) => {
            monomorphization_type_error(error, None, "prelude trait dispatch")
        }
        TraitSelectionError::Ambiguous => BytecodeError::construction(
            "trait dispatch",
            "a coherent prelude trait query selected more than one implementation",
        ),
    }
}

fn concrete_trait_target(
    hir: &HirProgram,
    interner: &mut TypeInterner,
    target: TypeId,
) -> Result<TypeId, BytecodeError> {
    hir.opaque_representation_for(interner, target)
        .map_err(|error| {
            BytecodeError::construction(
                "trait dispatch",
                format!("cannot reveal opaque Self representation: {error}"),
            )
        })
}

fn verify_prelude_dispatch_signature(
    hir: &HirProgram,
    interner: &mut TypeInterner,
    source: &PreludeTraitInstance,
    target: &CallableInstance,
    span: Span,
) -> Result<(), BytecodeError> {
    let source_type = source
        .method
        .function_type(interner, &source.arguments)
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "prelude trait source signature")
        })?
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                "prelude trait source has an invalid specialization arity",
            )
        })?;
    let target_signature = hir.callable(target.callable).ok_or_else(|| {
        BytecodeError::construction("trait dispatch", format!("{target:?} has no HIR signature"))
    })?;
    if target.arguments.len() != target_signature.generic_arity() as usize {
        return Err(BytecodeError::construction(
            "trait dispatch",
            "prelude trait target specialization has the wrong generic arity",
        ));
    }
    let target_type = TypeSubstitution::new(target.arguments.clone())
        .apply(interner, target_signature.function_type())
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "prelude trait target signature")
        })?;
    let source_representation = hir
        .opaque_representation_for(interner, source_type)
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "prelude trait source representation")
        })?;
    let target_representation = hir
        .opaque_representation_for(interner, target_type)
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "prelude trait target representation")
        })?;
    if source_representation != target_representation {
        return Err(BytecodeError::construction(
            "trait dispatch",
            format!(
                "selected prelude target has type `{}` instead of `{}`",
                interner
                    .canonical(target_type)
                    .unwrap_or_else(|_| target_type.to_string()),
                interner
                    .canonical(source_type)
                    .unwrap_or_else(|_| source_type.to_string())
            ),
        ));
    }
    Ok(())
}

fn resolve_source_trait_dispatch(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    interner: &mut TypeInterner,
    reference: &CallableInstance,
) -> Result<Option<CallableInstance>, BytecodeError> {
    let HirCallableId::Member(member) = reference.callable else {
        return Ok(None);
    };
    let Some(member_declaration) = resolved.member(member) else {
        return Err(BytecodeError::construction(
            "trait dispatch",
            format!("member#{} is not indexed", member.index()),
        ));
    };
    let MemberOwner::Type(owner) = member_declaration.owner() else {
        return Ok(None);
    };
    if resolved
        .symbol(owner)
        .is_none_or(|symbol| symbol.kind() != SymbolKind::Trait)
    {
        return Ok(None);
    }

    let declaration = hir.declaration(owner).ok_or_else(|| {
        BytecodeError::construction(
            "trait dispatch",
            format!("trait symbol#{} has no HIR declaration", owner.index()),
        )
    })?;
    let HirTypeDeclarationKind::Trait(definition) = declaration.kind() else {
        return Err(BytecodeError::construction(
            "trait dispatch",
            format!("trait symbol#{} has non-trait HIR metadata", owner.index()),
        ));
    };
    let trait_arity = declaration.parameters().len();
    let fixed_arity = trait_arity.checked_add(1).ok_or_else(|| {
        BytecodeError::construction("trait dispatch", "trait generic prefix overflow")
    })?;
    if reference.arguments.len() < fixed_arity {
        return Err(BytecodeError::construction(
            "trait dispatch",
            format!(
                "member#{} requires {fixed_arity} trait and Self arguments, found {}",
                member.index(),
                reference.arguments.len()
            ),
        ));
    }
    let query = TraitQuery::from_parts(
        HirTraitConstructor::Symbol(owner),
        reference.arguments[..trait_arity].to_vec(),
        concrete_trait_target(hir, interner, reference.arguments[trait_arity])?,
    );
    let selection = select_implementation(interner, hir.implementations(), &query)
        .map_err(|error| trait_dispatch_selection_error(error, member_declaration.span()))?
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "member#{} has no implementation for its concrete query",
                    member.index()
                ),
            )
        })?;
    let implementation = hir
        .implementation(selection.implementation())
        .ok_or_else(|| {
            BytecodeError::construction(
                "trait dispatch",
                format!(
                    "selected implementation#{} is not indexed",
                    selection.implementation().index()
                ),
            )
        })?;
    let key = HirTraitMethodKey::Source(member);
    let method_arguments = &reference.arguments[fixed_arity..];
    let target = if let Some(method) = implementation.methods().iter().find(|method| {
        method
            .contract()
            .is_some_and(|contract| contract.method() == key)
    }) {
        CallableInstance {
            callable: HirCallableId::Implementation(method.id()),
            arguments: selection
                .arguments()
                .iter()
                .copied()
                .chain(method_arguments.iter().copied())
                .collect(),
        }
    } else {
        let has_default = definition
            .methods()
            .iter()
            .find(|method| method.member() == member)
            .is_some_and(|method| method.has_default());
        if !has_default || hir.body(HirCallableId::Member(member)).is_none() {
            return Err(BytecodeError::construction(
                "trait dispatch",
                format!(
                    "implementation#{} provides neither member#{} nor its default",
                    implementation.id().index(),
                    member.index()
                ),
            ));
        }
        reference.clone()
    };
    verify_dispatch_signature(hir, interner, reference, &target, member_declaration.span())?;
    Ok(Some(target))
}

fn trait_dispatch_selection_error(error: TraitSelectionError, span: Span) -> BytecodeError {
    match error {
        TraitSelectionError::Type(error) => {
            monomorphization_type_error(error, Some(span), "trait dispatch")
        }
        TraitSelectionError::Ambiguous => BytecodeError::construction(
            "trait dispatch",
            "a coherent trait query selected more than one implementation",
        ),
    }
}

fn verify_dispatch_signature(
    hir: &HirProgram,
    interner: &mut TypeInterner,
    source: &CallableInstance,
    target: &CallableInstance,
    span: Span,
) -> Result<(), BytecodeError> {
    let source_signature = hir.callable(source.callable).ok_or_else(|| {
        BytecodeError::construction("trait dispatch", format!("{source:?} has no HIR signature"))
    })?;
    let target_signature = hir.callable(target.callable).ok_or_else(|| {
        BytecodeError::construction("trait dispatch", format!("{target:?} has no HIR signature"))
    })?;
    if source.arguments.len() != source_signature.generic_arity() as usize
        || target.arguments.len() != target_signature.generic_arity() as usize
    {
        return Err(BytecodeError::construction(
            "trait dispatch",
            "source or target specialization has the wrong generic arity",
        ));
    }
    let source_type = TypeSubstitution::new(source.arguments.clone())
        .apply(interner, source_signature.function_type())
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "trait dispatch source signature")
        })?;
    let target_type = TypeSubstitution::new(target.arguments.clone())
        .apply(interner, target_signature.function_type())
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "trait dispatch target signature")
        })?;
    let source_representation = hir
        .opaque_representation_for(interner, source_type)
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "trait source representation")
        })?;
    let target_representation = hir
        .opaque_representation_for(interner, target_type)
        .map_err(|error| {
            monomorphization_type_error(error, Some(span), "trait target representation")
        })?;
    if source_representation != target_representation {
        return Err(BytecodeError::construction(
            "trait dispatch",
            format!(
                "selected target has type `{}` instead of `{}`",
                interner
                    .canonical(target_type)
                    .unwrap_or_else(|_| target_type.to_string()),
                interner
                    .canonical(source_type)
                    .unwrap_or_else(|_| source_type.to_string())
            ),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn register_instance(
    hir: &HirProgram,
    mir: &MirProgram,
    interner: &TypeInterner,
    instance: ExecutableInstance,
    generic_limit: u32,
    generic_count: &mut usize,
    callables: &mut BTreeSet<ExecutableInstance>,
    functions: &mut BTreeSet<ExecutableInstance>,
    pending: &mut BTreeSet<ExecutableInstance>,
) -> Result<(), BytecodeError> {
    let (generic_arity, span) = match &instance {
        ExecutableInstance::Named(instance) => {
            let signature = hir.callable(instance.callable).ok_or_else(|| {
                BytecodeError::construction(
                    "monomorphization",
                    format!("{:?} has no HIR signature", instance.callable),
                )
            })?;
            (signature.generic_arity(), signature.span())
        }
        ExecutableInstance::Closure(instance) => {
            let closure = hir.closure(instance.closure).ok_or_else(|| {
                BytecodeError::construction(
                    "monomorphization",
                    format!("closure#{} has no HIR metadata", instance.closure.index()),
                )
            })?;
            (closure.generic_arity(), closure.span())
        }
    };
    if instance.arguments().len() != generic_arity as usize {
        return Err(BytecodeError::construction(
            "monomorphization",
            format!(
                "{instance:?} expects {generic_arity} type arguments, found {}",
                instance.arguments().len()
            ),
        ));
    }
    for argument in instance.arguments() {
        if type_contains_generic_parameter(interner, *argument)? {
            return Err(BytecodeError::construction(
                "monomorphization",
                format!("{instance:?} is not concrete"),
            ));
        }
    }
    if !callables.insert(instance.clone()) {
        return Ok(());
    }
    if generic_arity != 0 {
        *generic_count = generic_count
            .checked_add(1)
            .ok_or(BytecodeError::NodeLimit {
                span: Some(span),
                resource: "generic instantiations",
            })?;
        ensure_count(
            *generic_count,
            generic_limit,
            Some(span),
            "generic instantiations",
        )?;
    }
    if mir_function(mir, &instance).is_some() {
        functions.insert(instance.clone());
        pending.insert(instance);
    }
    Ok(())
}

fn mir_function<'a>(mir: &'a MirProgram, instance: &ExecutableInstance) -> Option<&'a MirFunction> {
    match instance {
        ExecutableInstance::Named(instance) => mir.function(instance.callable),
        ExecutableInstance::Closure(instance) => mir.closure_function(instance.closure),
    }
}

fn type_contains_generic_parameter(
    interner: &TypeInterner,
    root: TypeId,
) -> Result<bool, BytecodeError> {
    let mut visited = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        let kind = interner
            .kind(ty)
            .map_err(|error| BytecodeError::construction("monomorphization", error.to_string()))?;
        if matches!(kind, TypeKind::GenericParameter(_) | TypeKind::Inference(_)) {
            return Ok(true);
        }
        pending.extend(type_children(kind));
    }
    Ok(false)
}

fn nominal_ids(
    hir: &HirProgram,
    limit: u32,
) -> Result<BTreeMap<SymbolId, bc::BytecodeNominalId>, BytecodeError> {
    let symbols = hir
        .declarations()
        .filter_map(|(symbol, declaration)| {
            matches!(declaration.kind(), HirTypeDeclarationKind::Nominal(_)).then_some(*symbol)
        })
        .collect::<Vec<_>>();
    ensure_count(symbols.len(), limit, None, "nominal metadata")?;
    symbols
        .into_iter()
        .enumerate()
        .map(|(index, symbol)| {
            Ok((
                symbol,
                bc::BytecodeNominalId::new(checked_index(index, "nominal")?),
            ))
        })
        .collect()
}

fn callable_ids(
    instances: &[ExecutableInstance],
    limit: u32,
) -> Result<BTreeMap<ExecutableInstance, bc::BytecodeCallableId>, BytecodeError> {
    ensure_count(instances.len(), limit, None, "callable metadata")?;
    instances
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, instance)| {
            Ok((
                instance,
                bc::BytecodeCallableId::new(checked_index(index, "callable")?),
            ))
        })
        .collect()
}

fn function_ids(
    instances: &[ExecutableInstance],
    limit: u32,
) -> Result<BTreeMap<ExecutableInstance, bc::BytecodeFunctionId>, BytecodeError> {
    ensure_count(instances.len(), limit, None, "function count")?;
    instances
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, instance)| {
            Ok((
                instance,
                bc::BytecodeFunctionId::new(checked_index(index, "function")?),
            ))
        })
        .collect()
}

fn constant_ids(
    hir: &HirProgram,
    limit: u32,
) -> Result<BTreeMap<SymbolId, bc::BytecodeConstantId>, BytecodeError> {
    let constants = hir
        .constants()
        .filter_map(|(symbol, constant)| constant.evaluated().map(|_| *symbol))
        .collect::<Vec<_>>();
    ensure_count(constants.len(), limit, None, "constant pool")?;
    constants
        .into_iter()
        .enumerate()
        .map(|(index, symbol)| {
            Ok((
                symbol,
                bc::BytecodeConstantId::new(checked_index(index, "constant")?),
            ))
        })
        .collect()
}

struct TypeCatalog {
    ids: BTreeMap<TypeId, bc::BytecodeTypeId>,
    types: Vec<bc::BytecodeType>,
    opaque_witnesses: BTreeMap<TypeId, TypeId>,
}

impl TypeCatalog {
    fn build(
        interner: &mut TypeInterner,
        hir: &HirProgram,
        type_maps: &BTreeMap<ExecutableInstance, BTreeMap<TypeId, TypeId>>,
        limit: u32,
    ) -> Result<Self, BytecodeError> {
        let mut seeds = BTreeSet::new();
        collect_metadata_types(hir, &mut seeds);
        for map in type_maps.values() {
            seeds.extend(map.values().copied());
        }
        let mut opaque_witnesses = BTreeMap::new();
        let mut queue = seeds.iter().copied().collect::<VecDeque<_>>();
        while let Some(ty) = queue.pop_front() {
            let kind = interner
                .kind(ty)
                .map_err(|error| BytecodeError::construction("type catalog", error.to_string()))?
                .clone();
            for child in type_children(&kind) {
                if seeds.insert(child) {
                    ensure_count(seeds.len(), limit, None, "type table")?;
                    queue.push_back(child);
                }
            }
            if matches!(kind, TypeKind::OpaqueResult { .. }) {
                let witness = hir
                    .opaque_witness_for(interner, ty)
                    .map_err(|error| {
                        BytecodeError::construction("type catalog", error.to_string())
                    })?
                    .ok_or_else(|| {
                        BytecodeError::construction(
                            "type catalog",
                            "opaque type has no concrete witness",
                        )
                    })?;
                opaque_witnesses.insert(ty, witness);
                if seeds.insert(witness) {
                    ensure_count(seeds.len(), limit, None, "type table")?;
                    queue.push_back(witness);
                }
            }
        }
        ensure_count(seeds.len(), limit, None, "type table")?;
        let ids = seeds
            .iter()
            .enumerate()
            .map(|(index, ty)| Ok((*ty, bc::BytecodeTypeId::new(checked_index(index, "type")?))))
            .collect::<Result<BTreeMap<_, _>, BytecodeError>>()?;
        let mut catalog = Self {
            ids,
            types: Vec::with_capacity(seeds.len()),
            opaque_witnesses,
        };
        for ty in seeds {
            catalog.types.push(catalog.lower_type(interner, ty)?);
        }
        Ok(catalog)
    }

    fn id(&self, ty: TypeId) -> Result<bc::BytecodeTypeId, BytecodeError> {
        self.ids.get(&ty).copied().ok_or_else(|| {
            BytecodeError::construction("type catalog", format!("missing reachable {ty}"))
        })
    }

    fn lower_type(
        &self,
        interner: &TypeInterner,
        ty: TypeId,
    ) -> Result<bc::BytecodeType, BytecodeError> {
        let name = interner.canonical(ty).map_err(|error| {
            BytecodeError::construction("type catalog", format!("{ty} is not canonical: {error}"))
        })?;
        let kind = match interner
            .kind(ty)
            .map_err(|error| BytecodeError::construction("type catalog", error.to_string()))?
        {
            TypeKind::Error | TypeKind::Inference(_) => {
                return Err(BytecodeError::construction(
                    "type catalog",
                    format!("recovery or inference type {ty} reached bytecode"),
                ));
            }
            TypeKind::Scalar(scalar) => bc::BytecodeTypeKind::Scalar(scalar_type(*scalar)),
            TypeKind::Nominal {
                identity,
                arguments,
            } => bc::BytecodeTypeKind::Nominal {
                nominal: None,
                identity: identity.canonical_name(),
                arguments: self.map_types(arguments)?,
            },
            TypeKind::Tuple(items) => bc::BytecodeTypeKind::Tuple(self.map_types(items)?),
            TypeKind::Function(function) => {
                bc::BytecodeTypeKind::Function(self.lower_function_type(function)?)
            }
            TypeKind::Option(item) => bc::BytecodeTypeKind::Option(self.id(*item)?),
            TypeKind::Result { success, error } => bc::BytecodeTypeKind::Result {
                success: self.id(*success)?,
                error: self.id(*error)?,
            },
            TypeKind::Union(members) => bc::BytecodeTypeKind::Union(self.map_types(members)?),
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => bc::BytecodeTypeKind::Intrinsic {
                constructor: intrinsic_type(*constructor),
                arguments: self.map_types(arguments)?,
            },
            TypeKind::GenericParameter(position) => {
                bc::BytecodeTypeKind::GenericParameter(*position)
            }
            TypeKind::OpaqueResult {
                identity,
                arguments,
            } => bc::BytecodeTypeKind::OpaqueResult {
                identity: identity.canonical_name(),
                arguments: self.map_types(arguments)?,
                witness: self.id(*self.opaque_witnesses.get(&ty).ok_or_else(|| {
                    BytecodeError::construction(
                        "type catalog",
                        "opaque type is missing its witness mapping",
                    )
                })?)?,
            },
            TypeKind::Generated { arguments, .. } => bc::BytecodeTypeKind::Generated {
                identity: name.clone(),
                arguments: self.map_types(arguments)?,
            },
            TypeKind::Cursor { mode, collection } => bc::BytecodeTypeKind::Cursor {
                mode: match mode {
                    CursorMode::Own => bc::BytecodeCursorMode::Own,
                    CursorMode::Ref => bc::BytecodeCursorMode::Ref,
                },
                collection: self.id(*collection)?,
            },
        };
        Ok(bc::BytecodeType { name, kind })
    }

    fn attach_nominal_ids(
        &mut self,
        resolved: &ResolvedProgram,
        nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    ) {
        let by_identity = resolved
            .symbols()
            .filter_map(|symbol| {
                nominal_ids
                    .get(&symbol.id())
                    .map(|id| (symbol.identity().canonical_name(), *id))
            })
            .collect::<BTreeMap<_, _>>();
        for ty in &mut self.types {
            if let bc::BytecodeTypeKind::Nominal {
                nominal, identity, ..
            } = &mut ty.kind
            {
                *nominal = by_identity.get(identity).copied();
            }
        }
    }

    fn lower_function_type(
        &self,
        function: &FunctionType,
    ) -> Result<bc::BytecodeFunctionType, BytecodeError> {
        Ok(bc::BytecodeFunctionType {
            is_async: function.is_async(),
            is_unsafe: function.is_unsafe(),
            parameters: function
                .parameters()
                .iter()
                .map(|parameter| {
                    Ok(bc::BytecodeFunctionParameter {
                        mode: parameter_mode(parameter.mode()),
                        ty: self.id(parameter.ty())?,
                    })
                })
                .collect::<Result<_, BytecodeError>>()?,
            variadic: function.variadic().map(|ty| self.id(ty)).transpose()?,
            outcome: self.id(function.outcome())?,
        })
    }

    fn map_types(&self, types: &[TypeId]) -> Result<Vec<bc::BytecodeTypeId>, BytecodeError> {
        types.iter().map(|ty| self.id(*ty)).collect()
    }
}

fn collect_metadata_types(hir: &HirProgram, types: &mut BTreeSet<TypeId>) {
    for (_, declaration) in hir.declarations() {
        let HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
            continue;
        };
        types.insert(nominal.self_type());
        match nominal.shape() {
            HirNominalShape::Newtype { underlying } => {
                types.insert(*underlying);
            }
            HirNominalShape::Record { fields } => {
                types.extend(fields.iter().map(|field| field.ty()));
            }
            HirNominalShape::Enum { variants } => {
                for variant in variants {
                    match variant.payload() {
                        HirVariantPayload::Unit => {}
                        HirVariantPayload::Tuple(items) => types.extend(items.iter().copied()),
                        HirVariantPayload::Record(fields) => {
                            types.extend(fields.iter().map(|field| field.ty()));
                        }
                    }
                }
            }
        }
    }
    for (_, constant) in hir.constants() {
        if let Some(value) = constant.evaluated() {
            collect_constant_types(value, types);
        }
    }
}

fn collect_constant_types(value: &HirConstantValue, types: &mut BTreeSet<TypeId>) {
    types.insert(value.ty());
    match value.kind() {
        HirConstantValueKind::Function { arguments, .. } => types.extend(arguments.iter().copied()),
        HirConstantValueKind::Tuple(values)
        | HirConstantValueKind::Array(values)
        | HirConstantValueKind::Set(values) => {
            for value in values {
                collect_constant_types(value, types);
            }
        }
        HirConstantValueKind::Map(entries) => {
            for (key, value) in entries {
                collect_constant_types(key, types);
                collect_constant_types(value, types);
            }
        }
        HirConstantValueKind::Newtype { value, .. }
        | HirConstantValueKind::OptionSome(value)
        | HirConstantValueKind::ResultOk(value)
        | HirConstantValueKind::ResultErr(value)
        | HirConstantValueKind::Converted(value) => collect_constant_types(value, types),
        HirConstantValueKind::Record { fields, .. } => {
            for field in fields {
                collect_constant_types(field.value(), types);
            }
        }
        HirConstantValueKind::Variant { payload, .. } => match payload {
            HirConstantVariantValue::Unit => {}
            HirConstantVariantValue::Tuple(values) => {
                for value in values {
                    collect_constant_types(value, types);
                }
            }
            HirConstantVariantValue::Record(fields) => {
                for field in fields {
                    collect_constant_types(field.value(), types);
                }
            }
        },
        HirConstantValueKind::Range { start, end, .. } => {
            collect_constant_types(start, types);
            collect_constant_types(end, types);
        }
        HirConstantValueKind::Unit
        | HirConstantValueKind::Bool(_)
        | HirConstantValueKind::Integer(_)
        | HirConstantValueKind::Float(_)
        | HirConstantValueKind::Char(_)
        | HirConstantValueKind::String(_)
        | HirConstantValueKind::OptionNone => {}
    }
}

fn collect_constant_function_references(
    value: &HirConstantValue,
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
) {
    match value.kind() {
        HirConstantValueKind::Function {
            callable,
            arguments,
        } => references.push((*callable, arguments.clone())),
        HirConstantValueKind::Tuple(values)
        | HirConstantValueKind::Array(values)
        | HirConstantValueKind::Set(values) => {
            for value in values {
                collect_constant_function_references(value, references);
            }
        }
        HirConstantValueKind::Map(entries) => {
            for (key, value) in entries {
                collect_constant_function_references(key, references);
                collect_constant_function_references(value, references);
            }
        }
        HirConstantValueKind::Newtype { value, .. }
        | HirConstantValueKind::OptionSome(value)
        | HirConstantValueKind::ResultOk(value)
        | HirConstantValueKind::ResultErr(value)
        | HirConstantValueKind::Converted(value) => {
            collect_constant_function_references(value, references);
        }
        HirConstantValueKind::Record { fields, .. } => {
            for field in fields {
                collect_constant_function_references(field.value(), references);
            }
        }
        HirConstantValueKind::Variant { payload, .. } => match payload {
            HirConstantVariantValue::Unit => {}
            HirConstantVariantValue::Tuple(values) => {
                for value in values {
                    collect_constant_function_references(value, references);
                }
            }
            HirConstantVariantValue::Record(fields) => {
                for field in fields {
                    collect_constant_function_references(field.value(), references);
                }
            }
        },
        HirConstantValueKind::Range { start, end, .. } => {
            collect_constant_function_references(start, references);
            collect_constant_function_references(end, references);
        }
        HirConstantValueKind::Unit
        | HirConstantValueKind::Bool(_)
        | HirConstantValueKind::Integer(_)
        | HirConstantValueKind::Float(_)
        | HirConstantValueKind::Char(_)
        | HirConstantValueKind::String(_)
        | HirConstantValueKind::OptionNone => {}
    }
}

fn type_children(kind: &TypeKind) -> Vec<TypeId> {
    match kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Union(arguments)
        | TypeKind::Intrinsic { arguments, .. }
        | TypeKind::Generated { arguments, .. }
        | TypeKind::OpaqueResult { arguments, .. } => arguments.clone(),
        TypeKind::Function(function) => function
            .parameters()
            .iter()
            .map(|parameter| parameter.ty())
            .chain(function.variadic())
            .chain([function.outcome()])
            .collect(),
        TypeKind::Option(item) => vec![*item],
        TypeKind::Result { success, error } => vec![*success, *error],
        TypeKind::Cursor { collection, .. } => vec![*collection],
        TypeKind::Error
        | TypeKind::Scalar(_)
        | TypeKind::GenericParameter(_)
        | TypeKind::Inference(_) => Vec::new(),
    }
}

fn collect_function_types(function: &MirFunction, types: &mut BTreeSet<TypeId>) {
    types.insert(function.outcome());
    types.extend(function.locals().map(|local| local.ty()));
    for block in function.blocks() {
        for statement in block.statements() {
            match statement.kind() {
                MirStatementKind::StorageLive(_) | MirStatementKind::StorageDead(_) => {}
                MirStatementKind::Assign { destination, value } => {
                    collect_place_types(destination, types);
                    collect_rvalue_types(value, types);
                }
            }
        }
        collect_terminator_types(block.terminator(), types);
    }
}

fn collect_place_types(place: &MirPlace, types: &mut BTreeSet<TypeId>) {
    types.insert(place.ty());
    for projection in place.projections() {
        types.insert(projection.ty());
        if let MirProjectionKind::UnionValue(member) = projection.kind() {
            types.insert(*member);
        }
    }
}

fn collect_operand_types(operand: &MirOperand, types: &mut BTreeSet<TypeId>) {
    types.insert(operand.ty());
    match operand.kind() {
        MirOperandKind::Copy(place)
        | MirOperandKind::Move(place)
        | MirOperandKind::Borrow(place) => {
            collect_place_types(place, types);
        }
        MirOperandKind::Function { arguments, .. }
        | MirOperandKind::PreludeTraitFunction { arguments, .. } => {
            types.extend(arguments.iter().copied());
        }
        MirOperandKind::Constant(_) => {}
    }
}

fn collect_rvalue_types(value: &MirRvalue, types: &mut BTreeSet<TypeId>) {
    types.insert(value.ty());
    match value.kind() {
        MirRvalueKind::Use(value)
        | MirRvalueKind::Length(value)
        | MirRvalueKind::IteratorState { source: value }
        | MirRvalueKind::Prefix { operand: value, .. }
        | MirRvalueKind::NumericConversion { value, .. }
        | MirRvalueKind::Coerce { value, .. } => collect_operand_types(value, types),
        MirRvalueKind::Binary { left, right, .. }
        | MirRvalueKind::Range {
            start: left,
            end: right,
            ..
        }
        | MirRvalueKind::Contains {
            item: left,
            container: right,
            ..
        } => {
            collect_operand_types(left, types);
            collect_operand_types(right, types);
        }
        MirRvalueKind::Aggregate { shape, values } => {
            if let MirAggregateKind::Closure { arguments, .. } = shape {
                types.extend(arguments.iter().copied());
            }
            for value in values {
                collect_operand_types(value, types);
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => {
            collect_operand_types(base, types);
            for (_, value) in fields {
                collect_operand_types(value, types);
            }
        }
    }
}

fn collect_operation_types(operation: &MirOperation, types: &mut BTreeSet<TypeId>) {
    types.insert(operation.ty());
    match operation.kind() {
        MirOperationKind::CheckedPrefix { operand, .. }
        | MirOperationKind::ExplicitPanic { message: operand } => {
            collect_operand_types(operand, types);
        }
        MirOperationKind::CheckedBinary { left, right, .. } => {
            collect_operand_types(left, types);
            collect_operand_types(right, types);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                collect_operand_types(key, types);
                collect_operand_types(value, types);
            }
        }
        MirOperationKind::Index { base, index, .. } => {
            collect_operand_types(base, types);
            collect_operand_types(index, types);
        }
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            collect_operand_types(base, types);
            for value in start.iter().chain(end).chain(step) {
                collect_operand_types(value, types);
            }
        }
        MirOperationKind::Call {
            callee,
            arguments,
            signature,
            ..
        } => {
            types.insert(*signature);
            collect_operand_types(callee, types);
            for argument in arguments {
                collect_operand_types(argument.value(), types);
            }
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            collect_operand_types(condition, types);
            for part in message_parts {
                collect_operand_types(part.value(), types);
            }
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                collect_operand_types(argument, types);
            }
        }
    }
}

fn collect_terminator_types(terminator: &MirTerminator, types: &mut BTreeSet<TypeId>) {
    match terminator.kind() {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => {}
        MirTerminatorKind::SwitchBool { condition, .. } => collect_operand_types(condition, types),
        MirTerminatorKind::SwitchTag { value, cases, .. } => {
            collect_operand_types(value, types);
            for (tag, _) in cases {
                if let MirTag::Union(member) = tag {
                    types.insert(*member);
                }
            }
        }
        MirTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } => {
            collect_operation_types(operation, types);
            if let Some(destination) = destination {
                collect_place_types(destination, types);
            }
        }
        MirTerminatorKind::IteratorNext {
            state, destination, ..
        } => {
            collect_place_types(state, types);
            collect_place_types(destination, types);
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                collect_place_types(place, types);
            }
            for replacement in replacements.iter().flatten() {
                collect_operand_types(replacement, types);
            }
        }
    }
}

fn collect_function_references(function: &MirFunction, references: &mut Vec<FunctionReference>) {
    for block in function.blocks() {
        for statement in block.statements() {
            if let MirStatementKind::Assign { value, .. } = statement.kind() {
                collect_rvalue_function_references(value, references);
            }
        }
        collect_terminator_function_references(block.terminator(), references);
    }
}

fn collect_operand_function_references(
    operand: &MirOperand,
    references: &mut Vec<FunctionReference>,
) {
    match operand.kind() {
        MirOperandKind::Function {
            callable,
            arguments,
        } => references.push(FunctionReference::Callable {
            callable: *callable,
            arguments: arguments.clone(),
        }),
        MirOperandKind::PreludeTraitFunction { method, arguments } => {
            references.push(FunctionReference::PreludeTrait {
                method: *method,
                arguments: arguments.clone(),
            });
        }
        MirOperandKind::Constant(_)
        | MirOperandKind::Copy(_)
        | MirOperandKind::Move(_)
        | MirOperandKind::Borrow(_) => {}
    }
}

fn collect_rvalue_function_references(value: &MirRvalue, references: &mut Vec<FunctionReference>) {
    match value.kind() {
        MirRvalueKind::Use(value)
        | MirRvalueKind::Length(value)
        | MirRvalueKind::IteratorState { source: value }
        | MirRvalueKind::Prefix { operand: value, .. }
        | MirRvalueKind::NumericConversion { value, .. }
        | MirRvalueKind::Coerce { value, .. } => {
            collect_operand_function_references(value, references);
        }
        MirRvalueKind::Binary { left, right, .. }
        | MirRvalueKind::Range {
            start: left,
            end: right,
            ..
        }
        | MirRvalueKind::Contains {
            item: left,
            container: right,
            ..
        } => {
            collect_operand_function_references(left, references);
            collect_operand_function_references(right, references);
        }
        MirRvalueKind::Aggregate { shape, values } => {
            if let MirAggregateKind::Closure { closure, arguments } = shape {
                references.push(FunctionReference::Closure {
                    closure: *closure,
                    arguments: arguments.clone(),
                });
            }
            for value in values {
                collect_operand_function_references(value, references);
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => {
            collect_operand_function_references(base, references);
            for (_, value) in fields {
                collect_operand_function_references(value, references);
            }
        }
    }
}

fn collect_operation_function_references(
    operation: &MirOperation,
    references: &mut Vec<FunctionReference>,
) {
    match operation.kind() {
        MirOperationKind::CheckedPrefix { operand, .. }
        | MirOperationKind::ExplicitPanic { message: operand } => {
            collect_operand_function_references(operand, references);
        }
        MirOperationKind::CheckedBinary { left, right, .. } => {
            collect_operand_function_references(left, references);
            collect_operand_function_references(right, references);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                collect_operand_function_references(key, references);
                collect_operand_function_references(value, references);
            }
        }
        MirOperationKind::Index { base, index, .. } => {
            collect_operand_function_references(base, references);
            collect_operand_function_references(index, references);
        }
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            collect_operand_function_references(base, references);
            for value in start.iter().chain(end).chain(step) {
                collect_operand_function_references(value, references);
            }
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            collect_operand_function_references(callee, references);
            for argument in arguments {
                collect_operand_function_references(argument.value(), references);
            }
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            collect_operand_function_references(condition, references);
            for part in message_parts {
                collect_operand_function_references(part.value(), references);
            }
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                collect_operand_function_references(argument, references);
            }
        }
    }
}

fn collect_terminator_function_references(
    terminator: &MirTerminator,
    references: &mut Vec<FunctionReference>,
) {
    match terminator.kind() {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable
        | MirTerminatorKind::IteratorNext { .. } => {}
        MirTerminatorKind::SwitchBool { condition, .. } => {
            collect_operand_function_references(condition, references);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            collect_operand_function_references(value, references);
        }
        MirTerminatorKind::Invoke { operation, .. } => {
            collect_operation_function_references(operation, references);
        }
        MirTerminatorKind::ValidatePlaces { replacements, .. } => {
            for replacement in replacements.iter().flatten() {
                collect_operand_function_references(replacement, references);
            }
        }
    }
}

fn lower_nominals(
    hir: &HirProgram,
    catalog: &TypeCatalog,
    ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
) -> Result<Vec<bc::BytecodeNominal>, BytecodeError> {
    let mut output = vec![None; ids.len()];
    for (symbol, declaration) in hir.declarations() {
        let HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
            continue;
        };
        let id = ids
            .get(symbol)
            .copied()
            .ok_or_else(|| BytecodeError::construction("nominal metadata", "missing nominal ID"))?;
        let name = catalog
            .types
            .get(catalog.id(nominal.self_type())?.index() as usize)
            .map(|ty| ty.name.clone())
            .ok_or_else(|| BytecodeError::construction("nominal metadata", "missing self type"))?;
        let identity =
            match hir.interner().kind(nominal.self_type()).map_err(|error| {
                BytecodeError::construction("nominal metadata", error.to_string())
            })? {
                TypeKind::Nominal { identity, .. } => identity.canonical_name(),
                _ => {
                    return Err(BytecodeError::construction(
                        "nominal metadata",
                        "nominal self type is not nominal",
                    ));
                }
            };
        let shape = match nominal.shape() {
            HirNominalShape::Newtype { underlying } => bc::BytecodeNominalShape::Newtype {
                underlying: catalog.id(*underlying)?,
            },
            HirNominalShape::Record { fields } => bc::BytecodeNominalShape::Record {
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(bc::BytecodeField {
                            member: field.member().index(),
                            ty: catalog.id(field.ty())?,
                        })
                    })
                    .collect::<Result<_, BytecodeError>>()?,
            },
            HirNominalShape::Enum { variants } => bc::BytecodeNominalShape::Enum {
                variants: variants
                    .iter()
                    .map(|variant| {
                        let payload = match variant.payload() {
                            HirVariantPayload::Unit => bc::BytecodeVariantPayload::Unit,
                            HirVariantPayload::Tuple(items) => {
                                bc::BytecodeVariantPayload::Tuple(catalog.map_types(items)?)
                            }
                            HirVariantPayload::Record(fields) => {
                                bc::BytecodeVariantPayload::Record(
                                    fields
                                        .iter()
                                        .map(|field| {
                                            Ok(bc::BytecodeField {
                                                member: field.member().index(),
                                                ty: catalog.id(field.ty())?,
                                            })
                                        })
                                        .collect::<Result<_, BytecodeError>>()?,
                                )
                            }
                        };
                        Ok(bc::BytecodeVariant {
                            member: variant.member().index(),
                            payload,
                        })
                    })
                    .collect::<Result<_, BytecodeError>>()?,
            },
        };
        output[id.index() as usize] = Some(bc::BytecodeNominal {
            name,
            identity,
            generic_arity: u32::try_from(declaration.parameters().len()).map_err(|_| {
                BytecodeError::construction("nominal metadata", "generic arity exceeds u32")
            })?,
            shape,
        });
    }
    output
        .into_iter()
        .map(|item| {
            item.ok_or_else(|| {
                BytecodeError::construction("nominal metadata", "nominal table has a hole")
            })
        })
        .collect()
}

fn lower_callables(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    monomorphization: &Monomorphization,
    catalog: &TypeCatalog,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    function_ids: &BTreeMap<ExecutableInstance, bc::BytecodeFunctionId>,
) -> Result<Vec<bc::BytecodeCallable>, BytecodeError> {
    let mut output = vec![None; callable_ids.len()];
    for instance in &monomorphization.callables {
        let type_map = monomorphization
            .type_maps
            .get(instance)
            .expect("every callable instance has a type map");
        let id = callable_ids.get(instance).copied().ok_or_else(|| {
            BytecodeError::construction("callable metadata", "missing callable ID")
        })?;
        let (mut name, parameters, outcome, function_type, closure) = match instance {
            ExecutableInstance::Named(instance) => {
                let callable = hir.callable(instance.callable).ok_or_else(|| {
                    BytecodeError::construction("callable metadata", "missing HIR signature")
                })?;
                (
                    callable_name(resolved, callable.id()),
                    callable.parameters(),
                    callable.outcome(),
                    callable.function_type(),
                    None,
                )
            }
            ExecutableInstance::Closure(instance) => {
                let closure = hir.closure(instance.closure).ok_or_else(|| {
                    BytecodeError::construction("callable metadata", "missing HIR closure")
                })?;
                let TypeKind::Function(function) = hir
                    .interner()
                    .kind(closure.function_type())
                    .map_err(|error| {
                        BytecodeError::construction("callable metadata", error.to_string())
                    })?
                else {
                    return Err(BytecodeError::construction(
                        "callable metadata",
                        "closure signature is not a function type",
                    ));
                };
                let protocols = closure.protocols();
                (
                    format!("closure#{}", closure.id().index()),
                    closure.parameters(),
                    function.outcome(),
                    closure.function_type(),
                    Some(bc::BytecodeClosure {
                        environment: mapped_catalog_id(closure.ty(), type_map, catalog)?,
                        captures: closure
                            .captures()
                            .iter()
                            .map(|capture| mapped_catalog_id(capture.ty(), type_map, catalog))
                            .collect::<Result<_, BytecodeError>>()?,
                        protocols: bc::BytecodeClosureProtocols {
                            call: protocols.call(),
                            call_mut: protocols.call_mut(),
                            call_once: protocols.call_once(),
                        },
                    }),
                )
            }
        };
        if !instance.arguments().is_empty() {
            let arguments = instance
                .arguments()
                .iter()
                .map(|argument| monomorphization.interner.canonical(*argument))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    BytecodeError::construction("callable metadata", error.to_string())
                })?;
            name.push('[');
            name.push_str(&arguments.join(", "));
            name.push(']');
        }
        output[id.index() as usize] = Some(bc::BytecodeCallable {
            name,
            generic_arity: 0,
            parameters: parameters
                .iter()
                .map(|parameter| {
                    Ok(bc::BytecodeParameter {
                        mode: parameter_mode(parameter.mode()),
                        ty: mapped_catalog_id(parameter.ty(), type_map, catalog)?,
                        variadic_element: parameter
                            .variadic_element()
                            .map(|ty| mapped_catalog_id(ty, type_map, catalog))
                            .transpose()?,
                        receiver: parameter.is_receiver(),
                    })
                })
                .collect::<Result<_, BytecodeError>>()?,
            outcome: mapped_catalog_id(outcome, type_map, catalog)?,
            function_type: mapped_catalog_id(function_type, type_map, catalog)?,
            implementation: function_ids.get(instance).copied(),
            closure,
        });
    }
    output
        .into_iter()
        .map(|item| {
            item.ok_or_else(|| {
                BytecodeError::construction("callable metadata", "callable table has a hole")
            })
        })
        .collect()
}

fn callable_name(resolved: &ResolvedProgram, id: HirCallableId) -> String {
    match id {
        HirCallableId::Symbol(symbol) => resolved
            .symbol(symbol)
            .map(|symbol| symbol.identity().canonical_name())
            .unwrap_or_else(|| format!("symbol#{}", symbol.index())),
        HirCallableId::Member(member) => resolved
            .member(member)
            .map(|declaration| {
                let owner = match declaration.owner() {
                    MemberOwner::Type(symbol) => resolved
                        .symbol(symbol)
                        .map(|symbol| symbol.identity().canonical_name())
                        .unwrap_or_else(|| format!("type#{}", symbol.index())),
                    MemberOwner::Variant(variant) => format!("variant#{}", variant.index()),
                };
                format!("{owner}.{}", declaration.name())
            })
            .unwrap_or_else(|| format!("member#{}", member.index())),
        HirCallableId::Implementation(method) => format!(
            "implementation#{}.method#{}",
            method.implementation().index(),
            method.index()
        ),
    }
}

fn lower_constants(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    dispatches: &BTreeMap<CallableInstance, CallableInstance>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
) -> Result<Vec<bc::BytecodeNamedConstant>, BytecodeError> {
    let mut output = vec![None; constant_ids.len()];
    for (symbol, constant) in hir.constants() {
        let Some(value) = constant.evaluated() else {
            continue;
        };
        let id = constant_ids
            .get(symbol)
            .copied()
            .ok_or_else(|| BytecodeError::construction("constant pool", "missing constant ID"))?;
        let name = resolved
            .symbol(*symbol)
            .map(|symbol| symbol.identity().canonical_name())
            .unwrap_or_else(|| format!("constant#{}", symbol.index()));
        output[id.index() as usize] = Some(bc::BytecodeNamedConstant {
            name,
            value: lower_constant_value(value, catalog, nominal_ids, callable_ids, dispatches)?,
        });
    }
    output
        .into_iter()
        .map(|item| {
            item.ok_or_else(|| {
                BytecodeError::construction("constant pool", "constant table has a hole")
            })
        })
        .collect()
}

fn lower_constant_value(
    value: &HirConstantValue,
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    dispatches: &BTreeMap<CallableInstance, CallableInstance>,
) -> Result<bc::BytecodeConstantValue, BytecodeError> {
    let ty = catalog.id(value.ty())?;
    let kind = match value.kind() {
        HirConstantValueKind::Unit => bc::BytecodeConstantValueKind::Unit,
        HirConstantValueKind::Bool(value) => bc::BytecodeConstantValueKind::Bool(*value),
        HirConstantValueKind::Integer(value) => bc::BytecodeConstantValueKind::Integer(*value),
        HirConstantValueKind::Float(value) => bc::BytecodeConstantValueKind::Float(*value),
        HirConstantValueKind::Char(value) => bc::BytecodeConstantValueKind::Char(*value),
        HirConstantValueKind::String(value) => bc::BytecodeConstantValueKind::String(value.clone()),
        HirConstantValueKind::Function {
            callable,
            arguments,
        } => bc::BytecodeConstantValueKind::Function {
            callable: {
                let source = CallableInstance {
                    callable: *callable,
                    arguments: arguments.clone(),
                };
                let target = dispatches.get(&source).unwrap_or(&source);
                map_named_callable_instance(target, callable_ids)?
            },
            arguments: Vec::new(),
        },
        HirConstantValueKind::Tuple(values) => bc::BytecodeConstantValueKind::Tuple(
            lower_constant_values(values, catalog, nominal_ids, callable_ids, dispatches)?,
        ),
        HirConstantValueKind::Array(values) => bc::BytecodeConstantValueKind::Array(
            lower_constant_values(values, catalog, nominal_ids, callable_ids, dispatches)?,
        ),
        HirConstantValueKind::Map(entries) => bc::BytecodeConstantValueKind::Map(
            entries
                .iter()
                .map(|(key, value)| {
                    Ok((
                        lower_constant_value(key, catalog, nominal_ids, callable_ids, dispatches)?,
                        lower_constant_value(
                            value,
                            catalog,
                            nominal_ids,
                            callable_ids,
                            dispatches,
                        )?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        ),
        HirConstantValueKind::Set(values) => bc::BytecodeConstantValueKind::Set(
            lower_constant_values(values, catalog, nominal_ids, callable_ids, dispatches)?,
        ),
        HirConstantValueKind::Newtype { constructor, value } => {
            bc::BytecodeConstantValueKind::Newtype {
                nominal: map_nominal(*constructor, nominal_ids)?,
                value: Box::new(lower_constant_value(
                    value,
                    catalog,
                    nominal_ids,
                    callable_ids,
                    dispatches,
                )?),
            }
        }
        HirConstantValueKind::Record { owner, fields } => bc::BytecodeConstantValueKind::Record {
            nominal: map_nominal(*owner, nominal_ids)?,
            fields: fields
                .iter()
                .map(|field| {
                    Ok((
                        field.member().index(),
                        lower_constant_value(
                            field.value(),
                            catalog,
                            nominal_ids,
                            callable_ids,
                            dispatches,
                        )?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        },
        HirConstantValueKind::Variant { variant, payload } => {
            bc::BytecodeConstantValueKind::Variant {
                variant: variant.index(),
                payload: lower_constant_variant(
                    payload,
                    catalog,
                    nominal_ids,
                    callable_ids,
                    dispatches,
                )?,
            }
        }
        HirConstantValueKind::OptionNone => bc::BytecodeConstantValueKind::OptionNone,
        HirConstantValueKind::OptionSome(value) => {
            bc::BytecodeConstantValueKind::OptionSome(Box::new(lower_constant_value(
                value,
                catalog,
                nominal_ids,
                callable_ids,
                dispatches,
            )?))
        }
        HirConstantValueKind::ResultOk(value) => bc::BytecodeConstantValueKind::ResultOk(Box::new(
            lower_constant_value(value, catalog, nominal_ids, callable_ids, dispatches)?,
        )),
        HirConstantValueKind::ResultErr(value) => {
            bc::BytecodeConstantValueKind::ResultErr(Box::new(lower_constant_value(
                value,
                catalog,
                nominal_ids,
                callable_ids,
                dispatches,
            )?))
        }
        HirConstantValueKind::Range { kind, start, end } => bc::BytecodeConstantValueKind::Range {
            kind: range_kind(*kind),
            start: Box::new(lower_constant_value(
                start,
                catalog,
                nominal_ids,
                callable_ids,
                dispatches,
            )?),
            end: Box::new(lower_constant_value(
                end,
                catalog,
                nominal_ids,
                callable_ids,
                dispatches,
            )?),
        },
        HirConstantValueKind::Converted(value) => {
            lower_constant_value(value, catalog, nominal_ids, callable_ids, dispatches)?.kind
        }
    };
    Ok(bc::BytecodeConstantValue { ty, kind })
}

fn lower_constant_values(
    values: &[HirConstantValue],
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    dispatches: &BTreeMap<CallableInstance, CallableInstance>,
) -> Result<Vec<bc::BytecodeConstantValue>, BytecodeError> {
    values
        .iter()
        .map(|value| lower_constant_value(value, catalog, nominal_ids, callable_ids, dispatches))
        .collect()
}

fn lower_constant_variant(
    payload: &HirConstantVariantValue,
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    dispatches: &BTreeMap<CallableInstance, CallableInstance>,
) -> Result<bc::BytecodeConstantVariantValue, BytecodeError> {
    Ok(match payload {
        HirConstantVariantValue::Unit => bc::BytecodeConstantVariantValue::Unit,
        HirConstantVariantValue::Tuple(values) => bc::BytecodeConstantVariantValue::Tuple(
            lower_constant_values(values, catalog, nominal_ids, callable_ids, dispatches)?,
        ),
        HirConstantVariantValue::Record(fields) => bc::BytecodeConstantVariantValue::Record(
            fields
                .iter()
                .map(|field| {
                    Ok((
                        field.member().index(),
                        lower_constant_value(
                            field.value(),
                            catalog,
                            nominal_ids,
                            callable_ids,
                            dispatches,
                        )?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        ),
    })
}

struct FunctionLoweringContext<'a> {
    hir: &'a HirProgram,
    catalog: &'a TypeCatalog,
    nominal_ids: &'a BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &'a BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    dispatches: &'a BTreeMap<CallableInstance, CallableInstance>,
    prelude_dispatches: &'a BTreeMap<PreludeTraitInstance, CallableInstance>,
    constant_ids: &'a BTreeMap<SymbolId, bc::BytecodeConstantId>,
}

fn lower_function(
    instance: &ExecutableInstance,
    function: &MirFunction,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
    limits: BytecodeLoweringLimits,
) -> Result<bc::BytecodeFunction, BytecodeError> {
    ensure_count(
        function.locals().len(),
        limits.max_slots_per_function,
        Some(function.span()),
        "slots per function",
    )?;
    ensure_count(
        function.blocks().len(),
        limits.max_blocks_per_function,
        Some(function.span()),
        "blocks per function",
    )?;
    let instruction_count = function
        .blocks()
        .try_fold(0usize, |count, block| {
            count.checked_add(block.statements().len())
        })
        .ok_or(BytecodeError::NodeLimit {
            span: Some(function.span()),
            resource: "instructions per function",
        })?;
    ensure_count(
        instruction_count,
        limits.max_instructions_per_function,
        Some(function.span()),
        "instructions per function",
    )?;

    let span_ids = function_span_ids(function, limits.max_spans_per_function)?;
    let mut function_types = BTreeSet::new();
    function_types.insert(function.outcome());
    function_types.extend(function.locals().map(|local| local.ty()));
    for block in function.blocks() {
        for statement in block.statements() {
            match statement.kind() {
                MirStatementKind::StorageLive(_) | MirStatementKind::StorageDead(_) => {}
                MirStatementKind::Assign { destination, value } => {
                    collect_place_types(destination, &mut function_types);
                    collect_rvalue_types(value, &mut function_types);
                }
            }
        }
        collect_terminator_types(block.terminator(), &mut function_types);
    }

    let slots = function
        .locals()
        .map(|local| {
            Ok(bc::BytecodeSlot {
                ty: mapped_catalog_id(local.ty(), type_map, context.catalog)?,
                span: span_id(&span_ids, local.span())?,
                kind: match local.kind() {
                    MirLocalKind::Return => bc::BytecodeSlotKind::Return,
                    MirLocalKind::Parameter { index, .. } => {
                        bc::BytecodeSlotKind::Parameter { index }
                    }
                    MirLocalKind::User(local) => bc::BytecodeSlotKind::User {
                        local: local.index(),
                    },
                    MirLocalKind::Temporary => bc::BytecodeSlotKind::Temporary,
                },
            })
        })
        .collect::<Result<_, BytecodeError>>()?;
    let blocks = function
        .blocks()
        .map(|block| lower_block(block, &span_ids, context, type_map))
        .collect::<Result<_, BytecodeError>>()?;
    let spans = span_ids.keys().copied().collect::<Vec<_>>();

    Ok(bc::BytecodeFunction {
        callable: map_callable_instance(instance, context.callable_ids)?,
        source: bytecode_span(function.span()),
        types: function_types
            .into_iter()
            .map(|ty| mapped_catalog_id(ty, type_map, context.catalog))
            .collect::<Result<BTreeSet<_>, BytecodeError>>()?
            .into_iter()
            .collect(),
        spans,
        slots,
        parameters: function
            .parameters()
            .iter()
            .map(|local| bc::BytecodeSlotId::new(local.index()))
            .collect(),
        return_slot: bc::BytecodeSlotId::new(function.return_local().index()),
        entry: bc::BytecodeBlockId::new(function.entry().index()),
        unwind: bc::BytecodeBlockId::new(function.unwind().index()),
        blocks,
    })
}

fn function_span_ids(
    function: &MirFunction,
    limit: u32,
) -> Result<BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>, BytecodeError> {
    let mut spans = BTreeSet::from([bytecode_span(function.span())]);
    spans.extend(function.locals().map(|local| bytecode_span(local.span())));
    for block in function.blocks() {
        spans.extend(
            block
                .statements()
                .iter()
                .map(|statement| bytecode_span(statement.span())),
        );
        spans.insert(bytecode_span(block.terminator().span()));
    }
    ensure_count(
        spans.len(),
        limit,
        Some(function.span()),
        "source spans per function",
    )?;
    spans
        .into_iter()
        .enumerate()
        .map(|(index, span)| {
            Ok((
                span,
                bc::BytecodeSpanId::new(checked_index(index, "source span")?),
            ))
        })
        .collect()
}

fn lower_block(
    block: &MirBasicBlock,
    span_ids: &BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeBlock, BytecodeError> {
    Ok(bc::BytecodeBlock {
        kind: match block.kind() {
            MirBlockKind::Normal => bc::BytecodeBlockKind::Normal,
            MirBlockKind::Cleanup => bc::BytecodeBlockKind::Cleanup,
        },
        instructions: block
            .statements()
            .iter()
            .map(|statement| lower_statement(statement, span_ids, context, type_map))
            .collect::<Result<_, BytecodeError>>()?,
        terminator: lower_terminator(block.terminator(), span_ids, context, type_map)?,
    })
}

fn lower_statement(
    statement: &MirStatement,
    span_ids: &BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeInstruction, BytecodeError> {
    let kind = match statement.kind() {
        MirStatementKind::StorageLive(local) => {
            bc::BytecodeInstructionKind::StorageLive(bc::BytecodeSlotId::new(local.index()))
        }
        MirStatementKind::StorageDead(local) => {
            bc::BytecodeInstructionKind::StorageDead(bc::BytecodeSlotId::new(local.index()))
        }
        MirStatementKind::Assign { destination, value } => bc::BytecodeInstructionKind::Store {
            destination: lower_place(destination, context, type_map)?,
            value: lower_rvalue(value, context, type_map)?,
        },
    };
    Ok(bc::BytecodeInstruction {
        span: span_id(span_ids, statement.span())?,
        kind,
    })
}

fn lower_terminator(
    terminator: &MirTerminator,
    span_ids: &BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeTerminator, BytecodeError> {
    let kind = match terminator.kind() {
        MirTerminatorKind::Goto { target } => bc::BytecodeTerminatorKind::Goto {
            target: block_id(*target),
        },
        MirTerminatorKind::SwitchBool {
            condition,
            if_true,
            if_false,
        } => bc::BytecodeTerminatorKind::BranchBool {
            condition: lower_operand(condition, context, type_map)?,
            if_true: block_id(*if_true),
            if_false: block_id(*if_false),
        },
        MirTerminatorKind::SwitchTag {
            value,
            cases,
            otherwise,
        } => bc::BytecodeTerminatorKind::BranchTag {
            value: lower_operand(value, context, type_map)?,
            cases: cases
                .iter()
                .map(|(tag, target)| {
                    Ok((
                        lower_tag(*tag, context.catalog, type_map)?,
                        block_id(*target),
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
            otherwise: block_id(*otherwise),
        },
        MirTerminatorKind::Invoke {
            operation,
            destination,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::Invoke {
            operation: lower_operation(operation, context, type_map)?,
            destination: destination
                .as_ref()
                .map(|place| lower_place(place, context, type_map))
                .transpose()?,
            target: target.map(block_id),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::IteratorNext {
            state,
            destination,
            has_value,
            exhausted,
            unwind,
        } => bc::BytecodeTerminatorKind::IteratorNext {
            state: lower_place(state, context, type_map)?,
            destination: lower_place(destination, context, type_map)?,
            has_value: block_id(*has_value),
            exhausted: block_id(*exhausted),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            for_write,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::ValidatePlaces {
            places: places
                .iter()
                .map(|place| lower_place(place, context, type_map))
                .collect::<Result<_, BytecodeError>>()?,
            replacements: replacements
                .iter()
                .map(|replacement| {
                    replacement
                        .as_ref()
                        .map(|replacement| lower_operand(replacement, context, type_map))
                        .transpose()
                })
                .collect::<Result<_, BytecodeError>>()?,
            for_write: *for_write,
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::Return => bc::BytecodeTerminatorKind::Return,
        MirTerminatorKind::ResumePanic => bc::BytecodeTerminatorKind::ResumePanic,
        MirTerminatorKind::Unreachable => bc::BytecodeTerminatorKind::Unreachable,
    };
    Ok(bc::BytecodeTerminator {
        span: span_id(span_ids, terminator.span())?,
        kind,
    })
}

fn lower_place(
    place: &MirPlace,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodePlace, BytecodeError> {
    Ok(bc::BytecodePlace {
        slot: bc::BytecodeSlotId::new(place.local().index()),
        ty: mapped_catalog_id(place.ty(), type_map, context.catalog)?,
        projections: place
            .projections()
            .iter()
            .map(|projection| lower_projection(projection, context, type_map))
            .collect::<Result<_, BytecodeError>>()?,
    })
}

fn lower_projection(
    projection: &MirProjection,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeProjection, BytecodeError> {
    let kind = match projection.kind() {
        MirProjectionKind::ClosureCapture { closure, index } => {
            bc::BytecodeProjectionKind::ClosureCapture {
                callable: closure_callable_id(*closure, context, type_map)?,
                index: *index,
            }
        }
        MirProjectionKind::Field(member) => bc::BytecodeProjectionKind::Field(member.index()),
        MirProjectionKind::TupleField(index) => bc::BytecodeProjectionKind::TupleField(*index),
        MirProjectionKind::NewtypeValue => bc::BytecodeProjectionKind::NewtypeValue,
        MirProjectionKind::VariantTuple { variant, index } => {
            bc::BytecodeProjectionKind::VariantTuple {
                variant: variant.index(),
                index: *index,
            }
        }
        MirProjectionKind::VariantField { variant, field } => {
            bc::BytecodeProjectionKind::VariantField {
                variant: variant.index(),
                field: field.index(),
            }
        }
        MirProjectionKind::OptionValue => bc::BytecodeProjectionKind::OptionValue,
        MirProjectionKind::ResultOkValue => bc::BytecodeProjectionKind::ResultOkValue,
        MirProjectionKind::ResultErrValue => bc::BytecodeProjectionKind::ResultErrValue,
        MirProjectionKind::UnionValue(member) => bc::BytecodeProjectionKind::UnionValue(
            mapped_catalog_id(*member, type_map, context.catalog)?,
        ),
        MirProjectionKind::ArrayPatternIndex(index) => {
            bc::BytecodeProjectionKind::ArrayPatternIndex(*index)
        }
        MirProjectionKind::ArrayPatternRest { start, suffix } => {
            bc::BytecodeProjectionKind::ArrayPatternRest {
                start: *start,
                suffix: *suffix,
            }
        }
        MirProjectionKind::Index { index, access } => bc::BytecodeProjectionKind::Index {
            index: bc::BytecodeSlotId::new(index.index()),
            access: index_access(*access),
        },
        MirProjectionKind::Slice { start, end, step } => bc::BytecodeProjectionKind::Slice {
            start: start.map(|slot| bc::BytecodeSlotId::new(slot.index())),
            end: end.map(|slot| bc::BytecodeSlotId::new(slot.index())),
            step: step.map(|slot| bc::BytecodeSlotId::new(slot.index())),
        },
    };
    Ok(bc::BytecodeProjection {
        ty: mapped_catalog_id(projection.ty(), type_map, context.catalog)?,
        kind,
    })
}

fn closure_callable_id(
    closure: HirClosureId,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeCallableId, BytecodeError> {
    let closure_metadata = context.hir.closure(closure).ok_or_else(|| {
        BytecodeError::construction(
            "closure projection",
            format!("closure#{} has no HIR metadata", closure.index()),
        )
    })?;
    let TypeKind::Generated { arguments, .. } = context
        .hir
        .interner()
        .kind(closure_metadata.ty())
        .map_err(|error| BytecodeError::construction("closure projection", error.to_string()))?
    else {
        return Err(BytecodeError::construction(
            "closure projection",
            "closure environment is not a generated type",
        ));
    };
    let instance = ExecutableInstance::Closure(ClosureInstance {
        closure,
        arguments: arguments
            .iter()
            .map(|argument| mapped_type(*argument, type_map))
            .collect::<Result<_, BytecodeError>>()?,
    });
    map_callable_instance(&instance, context.callable_ids)
}

fn lower_operand(
    operand: &MirOperand,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeOperand, BytecodeError> {
    let kind = match operand.kind() {
        MirOperandKind::Constant(value) => {
            bc::BytecodeOperandKind::Constant(lower_immediate(value, context.constant_ids)?)
        }
        MirOperandKind::Copy(place) => {
            bc::BytecodeOperandKind::Copy(lower_place(place, context, type_map)?)
        }
        MirOperandKind::Move(place) => {
            bc::BytecodeOperandKind::Move(lower_place(place, context, type_map)?)
        }
        MirOperandKind::Borrow(place) => {
            bc::BytecodeOperandKind::Borrow(lower_place(place, context, type_map)?)
        }
        MirOperandKind::Function {
            callable,
            arguments,
        } => bc::BytecodeOperandKind::Function {
            callable: {
                let source = CallableInstance {
                    callable: *callable,
                    arguments: arguments
                        .iter()
                        .map(|argument| mapped_type(*argument, type_map))
                        .collect::<Result<_, _>>()?,
                };
                let target = context.dispatches.get(&source).unwrap_or(&source);
                map_named_callable_instance(target, context.callable_ids)?
            },
            arguments: Vec::new(),
        },
        MirOperandKind::PreludeTraitFunction { method, arguments } => {
            let source = PreludeTraitInstance {
                method: *method,
                arguments: arguments
                    .iter()
                    .map(|argument| mapped_type(*argument, type_map))
                    .collect::<Result<_, _>>()?,
            };
            let target = context.prelude_dispatches.get(&source).ok_or_else(|| {
                BytecodeError::construction(
                    "trait dispatch",
                    format!("prelude trait reference {source:?} has no selected target"),
                )
            })?;
            bc::BytecodeOperandKind::Function {
                callable: map_named_callable_instance(target, context.callable_ids)?,
                arguments: Vec::new(),
            }
        }
    };
    Ok(bc::BytecodeOperand {
        ty: mapped_catalog_id(operand.ty(), type_map, context.catalog)?,
        kind,
    })
}

fn lower_immediate(
    value: &MirConstant,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
) -> Result<bc::BytecodeConstant, BytecodeError> {
    Ok(match value {
        MirConstant::Unit => bc::BytecodeConstant::Unit,
        MirConstant::Bool(value) => bc::BytecodeConstant::Bool(*value),
        MirConstant::Integer(value) => bc::BytecodeConstant::Integer(value.clone()),
        MirConstant::Float(value) => bc::BytecodeConstant::Float(value.clone()),
        MirConstant::Char(value) => bc::BytecodeConstant::Char(value.clone()),
        MirConstant::String(value) => bc::BytecodeConstant::String(value.clone()),
        MirConstant::Named(symbol) => {
            bc::BytecodeConstant::Named(constant_ids.get(symbol).copied().ok_or_else(|| {
                BytecodeError::construction(
                    "constant operand",
                    format!("constant symbol#{} has no pool entry", symbol.index()),
                )
            })?)
        }
    })
}

fn lower_rvalue(
    value: &MirRvalue,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeRvalue, BytecodeError> {
    let operand = |value: &MirOperand| lower_operand(value, context, type_map);
    let kind = match value.kind() {
        MirRvalueKind::Use(value) => bc::BytecodeRvalueKind::Use(operand(value)?),
        MirRvalueKind::Prefix {
            operator,
            operand: v,
        } => bc::BytecodeRvalueKind::Prefix {
            operator: prefix_operator(*operator),
            operand: operand(v)?,
        },
        MirRvalueKind::Binary {
            operator,
            left,
            right,
        } => bc::BytecodeRvalueKind::Binary {
            operator: binary_operator(*operator),
            left: operand(left)?,
            right: operand(right)?,
        },
        MirRvalueKind::Aggregate { shape, values } => {
            let values = values
                .iter()
                .map(operand)
                .collect::<Result<Vec<_>, BytecodeError>>()?;
            bc::BytecodeRvalueKind::Construct {
                shape: lower_aggregate(shape, context, type_map, &values)?,
                values,
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => bc::BytecodeRvalueKind::RecordUpdate {
            base: operand(base)?,
            fields: fields
                .iter()
                .map(|(field, value)| Ok((field.index(), operand(value)?)))
                .collect::<Result<_, BytecodeError>>()?,
        },
        MirRvalueKind::Coerce { kind, value } => bc::BytecodeRvalueKind::Coerce {
            kind: coercion(*kind),
            value: operand(value)?,
        },
        MirRvalueKind::NumericConversion {
            target,
            conversion,
            value,
        } => bc::BytecodeRvalueKind::NumericConversion {
            target: scalar_type(*target),
            conversion: numeric_conversion(*conversion),
            value: operand(value)?,
        },
        MirRvalueKind::Range { kind, start, end } => bc::BytecodeRvalueKind::Range {
            kind: range_kind(*kind),
            start: operand(start)?,
            end: operand(end)?,
        },
        MirRvalueKind::Contains {
            kind,
            item,
            container,
        } => bc::BytecodeRvalueKind::Contains {
            kind: containment_kind(*kind),
            item: operand(item)?,
            container: operand(container)?,
        },
        MirRvalueKind::Length(value) => bc::BytecodeRvalueKind::Length(operand(value)?),
        MirRvalueKind::IteratorState { source } => {
            bc::BytecodeRvalueKind::IteratorState(operand(source)?)
        }
    };
    Ok(bc::BytecodeRvalue {
        ty: mapped_catalog_id(value.ty(), type_map, context.catalog)?,
        kind,
    })
}

fn lower_aggregate(
    shape: &MirAggregateKind,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
    values: &[bc::BytecodeOperand],
) -> Result<bc::BytecodeAggregateKind, BytecodeError> {
    Ok(match shape {
        MirAggregateKind::Tuple => bc::BytecodeAggregateKind::Tuple,
        MirAggregateKind::Array => bc::BytecodeAggregateKind::Array,
        MirAggregateKind::Set => bc::BytecodeAggregateKind::Set,
        MirAggregateKind::Closure { closure, arguments } => bc::BytecodeAggregateKind::Closure {
            callable: map_callable_instance(
                &ExecutableInstance::Closure(ClosureInstance {
                    closure: *closure,
                    arguments: arguments
                        .iter()
                        .map(|argument| mapped_type(*argument, type_map))
                        .collect::<Result<_, BytecodeError>>()?,
                }),
                context.callable_ids,
            )?,
            captures: values.iter().map(|value| value.ty).collect(),
        },
        MirAggregateKind::Newtype { owner } => bc::BytecodeAggregateKind::Newtype {
            nominal: map_nominal(*owner, context.nominal_ids)?,
        },
        MirAggregateKind::Record { owner, fields } => bc::BytecodeAggregateKind::Record {
            nominal: map_nominal(*owner, context.nominal_ids)?,
            fields: fields.iter().map(|field| field.index()).collect(),
        },
        MirAggregateKind::Variant { variant, fields } => bc::BytecodeAggregateKind::Variant {
            variant: variant.index(),
            fields: fields
                .iter()
                .map(|field| field.map(|field| field.index()))
                .collect(),
        },
        MirAggregateKind::OptionNone => bc::BytecodeAggregateKind::OptionNone,
        MirAggregateKind::OptionSome => bc::BytecodeAggregateKind::OptionSome,
        MirAggregateKind::ResultOk => bc::BytecodeAggregateKind::ResultOk,
        MirAggregateKind::ResultErr => bc::BytecodeAggregateKind::ResultErr,
    })
}

fn lower_operation(
    operation: &MirOperation,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeOperation, BytecodeError> {
    let operand = |value: &MirOperand| lower_operand(value, context, type_map);
    let kind = match operation.kind() {
        MirOperationKind::CheckedPrefix {
            operator,
            operand: v,
        } => bc::BytecodeOperationKind::CheckedPrefix {
            operator: prefix_operator(*operator),
            operand: operand(v)?,
        },
        MirOperationKind::CheckedBinary {
            operator,
            left,
            right,
        } => bc::BytecodeOperationKind::CheckedBinary {
            operator: binary_operator(*operator),
            left: operand(left)?,
            right: operand(right)?,
        },
        MirOperationKind::BuildMap {
            entries,
            reject_dynamic_duplicates,
        } => bc::BytecodeOperationKind::BuildMap {
            entries: entries
                .iter()
                .map(|(key, value)| Ok((operand(key)?, operand(value)?)))
                .collect::<Result<_, BytecodeError>>()?,
            reject_dynamic_duplicates: *reject_dynamic_duplicates,
        },
        MirOperationKind::Index {
            base,
            index,
            access,
        } => bc::BytecodeOperationKind::Index {
            base: operand(base)?,
            index: operand(index)?,
            access: index_access(*access),
        },
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => bc::BytecodeOperationKind::Slice {
            base: operand(base)?,
            start: start.as_ref().map(operand).transpose()?,
            end: end.as_ref().map(operand).transpose()?,
            step: step.as_ref().map(operand).transpose()?,
        },
        MirOperationKind::Call {
            callee,
            arguments,
            signature,
            protocol,
        } => bc::BytecodeOperationKind::Call {
            callee: operand(callee)?,
            arguments: arguments
                .iter()
                .map(|argument| lower_call_argument(argument, context, type_map))
                .collect::<Result<_, BytecodeError>>()?,
            signature: mapped_catalog_id(*signature, type_map, context.catalog)?,
            protocol: call_protocol(*protocol),
        },
        MirOperationKind::ExplicitPanic { message } => bc::BytecodeOperationKind::ExplicitPanic {
            message: operand(message)?,
        },
        MirOperationKind::Assert {
            condition,
            condition_repr,
            message_parts,
        } => bc::BytecodeOperationKind::Assert {
            condition: operand(condition)?,
            condition_repr: condition_repr.clone(),
            message_parts: message_parts
                .iter()
                .map(|part| {
                    Ok(bc::BytecodeAssertMessagePart {
                        value: operand(part.value())?,
                        spread: part.is_spread(),
                    })
                })
                .collect::<Result<_, BytecodeError>>()?,
        },
        MirOperationKind::BootstrapHostCall {
            function,
            arguments,
        } => bc::BytecodeOperationKind::BootstrapHostCall {
            function: match function {
                crate::mir::MirBootstrapHostFunction::ConsolePrint => {
                    bc::BytecodeBootstrapHostFunction::ConsolePrint
                }
            },
            arguments: arguments
                .iter()
                .map(operand)
                .collect::<Result<_, BytecodeError>>()?,
        },
    };
    Ok(bc::BytecodeOperation {
        ty: mapped_catalog_id(operation.ty(), type_map, context.catalog)?,
        kind,
    })
}

fn lower_call_argument(
    argument: &MirCallArgument,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeCallArgument, BytecodeError> {
    use crate::hir::HirCallArgumentTarget;

    let target = match argument.target() {
        HirCallArgumentTarget::Receiver => bc::BytecodeCallArgumentTarget::Receiver,
        HirCallArgumentTarget::Fixed(index) => bc::BytecodeCallArgumentTarget::Fixed(index),
        HirCallArgumentTarget::VariadicElement => bc::BytecodeCallArgumentTarget::VariadicElement,
        HirCallArgumentTarget::VariadicSpread => bc::BytecodeCallArgumentTarget::VariadicSpread,
        HirCallArgumentTarget::Invalid => {
            return Err(BytecodeError::construction(
                "call argument",
                "unresolved argument association reached bytecode",
            ));
        }
    };
    Ok(bc::BytecodeCallArgument {
        mode: parameter_mode(argument.mode()),
        target,
        value: lower_operand(argument.value(), context, type_map)?,
    })
}

fn lower_tag(
    tag: MirTag,
    catalog: &TypeCatalog,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeTag, BytecodeError> {
    Ok(match tag {
        MirTag::OptionNone => bc::BytecodeTag::OptionNone,
        MirTag::OptionSome => bc::BytecodeTag::OptionSome,
        MirTag::ResultOk => bc::BytecodeTag::ResultOk,
        MirTag::ResultErr => bc::BytecodeTag::ResultErr,
        MirTag::Variant(variant) => bc::BytecodeTag::Variant(variant.index()),
        MirTag::Union(member) => {
            bc::BytecodeTag::Union(mapped_catalog_id(member, type_map, catalog)?)
        }
    })
}

fn map_nominal(
    symbol: SymbolId,
    ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
) -> Result<bc::BytecodeNominalId, BytecodeError> {
    ids.get(&symbol).copied().ok_or_else(|| {
        BytecodeError::construction(
            "nominal reference",
            format!("symbol#{} has no nominal metadata", symbol.index()),
        )
    })
}

fn mapped_type(
    template: TypeId,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<TypeId, BytecodeError> {
    type_map.get(&template).copied().ok_or_else(|| {
        BytecodeError::construction(
            "monomorphized type",
            format!("missing concrete form of {template}"),
        )
    })
}

fn mapped_catalog_id(
    template: TypeId,
    type_map: &BTreeMap<TypeId, TypeId>,
    catalog: &TypeCatalog,
) -> Result<bc::BytecodeTypeId, BytecodeError> {
    catalog.id(mapped_type(template, type_map)?)
}

fn map_callable_instance(
    instance: &ExecutableInstance,
    ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
) -> Result<bc::BytecodeCallableId, BytecodeError> {
    ids.get(instance).copied().ok_or_else(|| {
        BytecodeError::construction(
            "callable reference",
            format!("{instance:?} has no callable metadata"),
        )
    })
}

fn map_named_callable_instance(
    instance: &CallableInstance,
    ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
) -> Result<bc::BytecodeCallableId, BytecodeError> {
    map_callable_instance(&ExecutableInstance::Named(instance.clone()), ids)
}

fn block_id(id: crate::mir::MirBlockId) -> bc::BytecodeBlockId {
    bc::BytecodeBlockId::new(id.index())
}

fn bytecode_span(span: Span) -> bc::BytecodeSpan {
    bc::BytecodeSpan {
        file: span.file().index(),
        start: span.range().start(),
        end: span.range().end(),
    }
}

fn span_id(
    ids: &BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>,
    span: Span,
) -> Result<bc::BytecodeSpanId, BytecodeError> {
    ids.get(&bytecode_span(span)).copied().ok_or_else(|| {
        BytecodeError::construction("source map", "executable span is absent from its table")
    })
}

fn ensure_count(
    actual: usize,
    limit: u32,
    span: Option<Span>,
    resource: &'static str,
) -> Result<(), BytecodeError> {
    if actual > limit as usize {
        return Err(BytecodeError::NodeLimit { span, resource });
    }
    Ok(())
}

fn checked_index(index: usize, context: &str) -> Result<u32, BytecodeError> {
    u32::try_from(index).map_err(|_| {
        BytecodeError::construction(context, "table index is not representable as u32")
    })
}

fn scalar_type(value: ScalarType) -> bc::BytecodeScalarType {
    match value {
        ScalarType::Bool => bc::BytecodeScalarType::Bool,
        ScalarType::Int => bc::BytecodeScalarType::Int,
        ScalarType::Float => bc::BytecodeScalarType::Float,
        ScalarType::Byte => bc::BytecodeScalarType::Byte,
        ScalarType::Char => bc::BytecodeScalarType::Char,
        ScalarType::String => bc::BytecodeScalarType::String,
        ScalarType::Unit => bc::BytecodeScalarType::Unit,
        ScalarType::Never => bc::BytecodeScalarType::Never,
        ScalarType::Int8 => bc::BytecodeScalarType::Int8,
        ScalarType::Int16 => bc::BytecodeScalarType::Int16,
        ScalarType::Int32 => bc::BytecodeScalarType::Int32,
        ScalarType::UInt8 => bc::BytecodeScalarType::UInt8,
        ScalarType::UInt16 => bc::BytecodeScalarType::UInt16,
        ScalarType::UInt32 => bc::BytecodeScalarType::UInt32,
        ScalarType::UInt64 => bc::BytecodeScalarType::UInt64,
        ScalarType::Float32 => bc::BytecodeScalarType::Float32,
    }
}

fn intrinsic_type(value: IntrinsicType) -> bc::BytecodeIntrinsicType {
    match value {
        IntrinsicType::Array => bc::BytecodeIntrinsicType::Array,
        IntrinsicType::Map => bc::BytecodeIntrinsicType::Map,
        IntrinsicType::Set => bc::BytecodeIntrinsicType::Set,
        IntrinsicType::Range => bc::BytecodeIntrinsicType::Range,
        IntrinsicType::Ref => bc::BytecodeIntrinsicType::Ref,
        IntrinsicType::Pointer => bc::BytecodeIntrinsicType::Pointer,
        IntrinsicType::Join => bc::BytecodeIntrinsicType::Join,
        IntrinsicType::Command => bc::BytecodeIntrinsicType::Command,
        IntrinsicType::Pipeline => bc::BytecodeIntrinsicType::Pipeline,
        IntrinsicType::NumericConversionError => bc::BytecodeIntrinsicType::NumericConversionError,
    }
}

fn parameter_mode(value: ParameterMode) -> bc::BytecodeParameterMode {
    match value {
        ParameterMode::Value => bc::BytecodeParameterMode::Value,
        ParameterMode::Ref => bc::BytecodeParameterMode::Ref,
        ParameterMode::Mut => bc::BytecodeParameterMode::Mut,
        ParameterMode::Var => bc::BytecodeParameterMode::Var,
    }
}

fn call_protocol(value: HirCallProtocol) -> bc::BytecodeCallProtocol {
    match value {
        HirCallProtocol::Call => bc::BytecodeCallProtocol::Call,
        HirCallProtocol::CallMut => bc::BytecodeCallProtocol::CallMut,
        HirCallProtocol::CallOnce => bc::BytecodeCallProtocol::CallOnce,
    }
}

fn coercion(value: Assignability) -> bc::BytecodeCoercion {
    match value {
        Assignability::Exact => bc::BytecodeCoercion::Exact,
        Assignability::Opaque => bc::BytecodeCoercion::Opaque,
        Assignability::CallableErasure => bc::BytecodeCoercion::CallableErasure,
        Assignability::UnionInjection => bc::BytecodeCoercion::UnionInjection,
        Assignability::UnionWidening => bc::BytecodeCoercion::UnionWidening,
        Assignability::OptionLift => bc::BytecodeCoercion::OptionLift,
        Assignability::Diverging => bc::BytecodeCoercion::Diverging,
    }
}

fn numeric_conversion(value: NumericConversion) -> bc::BytecodeNumericConversion {
    match value {
        NumericConversion::Identity => bc::BytecodeNumericConversion::Identity,
        NumericConversion::Total => bc::BytecodeNumericConversion::Total,
        NumericConversion::Checked => bc::BytecodeNumericConversion::Checked,
    }
}

fn prefix_operator(value: crate::hir::HirPrefixOperator) -> bc::BytecodePrefixOperator {
    match value {
        crate::hir::HirPrefixOperator::Negate => bc::BytecodePrefixOperator::Negate,
        crate::hir::HirPrefixOperator::LogicalNot => bc::BytecodePrefixOperator::LogicalNot,
        crate::hir::HirPrefixOperator::BitwiseNot => bc::BytecodePrefixOperator::BitwiseNot,
    }
}

fn binary_operator(value: crate::hir::HirBinaryOperator) -> bc::BytecodeBinaryOperator {
    use crate::hir::HirBinaryOperator as Source;
    match value {
        Source::Multiply => bc::BytecodeBinaryOperator::Multiply,
        Source::Divide => bc::BytecodeBinaryOperator::Divide,
        Source::Remainder => bc::BytecodeBinaryOperator::Remainder,
        Source::Add => bc::BytecodeBinaryOperator::Add,
        Source::Subtract => bc::BytecodeBinaryOperator::Subtract,
        Source::ShiftLeft => bc::BytecodeBinaryOperator::ShiftLeft,
        Source::ShiftRight => bc::BytecodeBinaryOperator::ShiftRight,
        Source::BitwiseAnd => bc::BytecodeBinaryOperator::BitwiseAnd,
        Source::BitwiseXor => bc::BytecodeBinaryOperator::BitwiseXor,
        Source::BitwiseOr => bc::BytecodeBinaryOperator::BitwiseOr,
        Source::Less => bc::BytecodeBinaryOperator::Less,
        Source::LessEqual => bc::BytecodeBinaryOperator::LessEqual,
        Source::Greater => bc::BytecodeBinaryOperator::Greater,
        Source::GreaterEqual => bc::BytecodeBinaryOperator::GreaterEqual,
        Source::Equal => bc::BytecodeBinaryOperator::Equal,
        Source::NotEqual => bc::BytecodeBinaryOperator::NotEqual,
        Source::LogicalAnd => bc::BytecodeBinaryOperator::LogicalAnd,
        Source::LogicalOr => bc::BytecodeBinaryOperator::LogicalOr,
    }
}

fn range_kind(value: crate::hir::HirRangeKind) -> bc::BytecodeRangeKind {
    match value {
        crate::hir::HirRangeKind::Exclusive => bc::BytecodeRangeKind::Exclusive,
        crate::hir::HirRangeKind::Inclusive => bc::BytecodeRangeKind::Inclusive,
    }
}

fn containment_kind(value: crate::hir::HirContainmentKind) -> bc::BytecodeContainmentKind {
    match value {
        crate::hir::HirContainmentKind::Array => bc::BytecodeContainmentKind::Array,
        crate::hir::HirContainmentKind::MapKey => bc::BytecodeContainmentKind::MapKey,
        crate::hir::HirContainmentKind::Set => bc::BytecodeContainmentKind::Set,
        crate::hir::HirContainmentKind::Range => bc::BytecodeContainmentKind::Range,
        crate::hir::HirContainmentKind::StringChar => bc::BytecodeContainmentKind::StringChar,
    }
}

fn index_access(value: crate::hir::HirIndexAccess) -> bc::BytecodeIndexAccess {
    match value {
        crate::hir::HirIndexAccess::Array => bc::BytecodeIndexAccess::Array,
        crate::hir::HirIndexAccess::MapLookup => bc::BytecodeIndexAccess::MapLookup,
        crate::hir::HirIndexAccess::MapEntry => bc::BytecodeIndexAccess::MapEntry,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tondo_vm::runtime::{
        PanicCode, RejectingHost, RuntimeValue, VmError, VmHost, VmLimits, VmOutcome, execute,
        execute_with_limits,
    };

    use crate::hir::{ExpressionCheckLimits, TypeLoweringLimits, check_expressions, lower_types};
    use crate::mir::{MirLoweringLimits, lower_to_mir};
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn checked(source: &str) -> (ResolvedProgram, HirProgram) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:bytecode-lowering").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(source.as_bytes().to_vec()),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        assert!(lexed.diagnostics().is_empty());
        let parsed = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            parsed.diagnostics().is_empty(),
            "source:\n{source}\n{:#?}",
            parsed.diagnostics()
        );
        let packages = PackageGraph::loose(&sources, file).unwrap();
        let (resolved, diagnostics) = resolve(&packages, &sources, [(file, &parsed)], 100)
            .unwrap()
            .into_parts();
        assert!(
            diagnostics.is_empty(),
            "source:\n{source}\n{diagnostics:#?}"
        );
        let (hir, diagnostics) = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty());
        let (hir, diagnostics, complete) = check_expressions(
            &sources,
            [(file, &parsed)],
            &resolved,
            hir,
            ExpressionCheckLimits {
                max_nodes: 100_000,
                max_pattern_steps: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(complete);
        (resolved, hir)
    }

    fn lowered(source: &str) -> bc::BytecodeProgram {
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        lower_to_bytecode(&resolved, &hir, &mir, BytecodeLoweringLimits::default()).unwrap()
    }

    fn execute_outcome(source: &str, name: &str) -> VmOutcome {
        let program = lowered(source);
        let function = function_id(&program, name);
        let mut host = RejectingHost;
        execute(&program, function, &mut host)
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)))
            .outcome
    }

    fn function_id(program: &bc::BytecodeProgram, name: &str) -> bc::BytecodeFunctionId {
        program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with(&format!("::value::{name}")))
            .and_then(|callable| callable.implementation)
            .unwrap_or_else(|| panic!("missing bytecode body for `{name}`"))
    }

    fn execute_function(source: &str, name: &str) -> RuntimeValue {
        match execute_outcome(source, name) {
            VmOutcome::Returned(value) => value,
            VmOutcome::Panicked(panic) => panic!("unexpected VM panic: {panic:#?}"),
        }
    }

    #[test]
    fn lowering_is_deterministic_and_preserves_slots_spans_and_edges() {
        let source = "fn choose(flag: Bool): Int {\n    if flag { 20 + 22 } else { 0 }\n}\n";
        let first = lowered(source);
        let second = lowered(source);
        assert_eq!(first, second);
        assert_eq!(first.functions.len(), 1);
        let function = &first.functions[0];
        assert!(!function.types.is_empty());
        assert!(
            function
                .types
                .windows(2)
                .all(|pair| pair[0].index() < pair[1].index())
        );
        assert!(function.spans.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(function.blocks.iter().any(|block| matches!(
            block.terminator.kind,
            bc::BytecodeTerminatorKind::BranchBool { .. }
        )));
        assert!(function.blocks.iter().any(|block| matches!(
            block.terminator.kind,
            bc::BytecodeTerminatorKind::Invoke { .. }
        )));
        assert!(matches!(
            function.blocks[function.unwind.index() as usize]
                .terminator
                .kind,
            bc::BytecodeTerminatorKind::ResumePanic
        ));
    }

    #[test]
    fn nominal_callable_constant_and_projection_metadata_are_self_contained() {
        let source = "const Answer: Int = 42\n\
                      type User = { name: String, age: Int }\n\
                      enum Choice { Empty, Item(Int) }\n\
                      fn make(name: String): User {\n\
                          User { name, age: Answer }\n\
                      }\n\
                      fn age(user: User): Int { user.age }\n\
                      fn choose(value: Choice): Int {\n\
                          match value {\n\
                              Choice.Empty => 0\n\
                              Choice.Item(number) => number\n\
                          }\n\
                      }\n";
        let program = lowered(source);
        assert_eq!(program.nominals.len(), 2);
        assert_eq!(program.constants.len(), 1);
        assert!(
            program
                .callables
                .iter()
                .all(|callable| callable.implementation.is_some())
        );
        assert!(program.types.iter().any(|ty| matches!(
            ty.kind,
            bc::BytecodeTypeKind::Nominal {
                nominal: Some(_),
                ..
            }
        )));
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        &instruction.kind,
                        bc::BytecodeInstructionKind::Store {
                            value:
                                bc::BytecodeRvalue {
                                    kind:
                                        bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                            kind: bc::BytecodeOperandKind::Copy(place),
                                            ..
                                        }),
                                    ..
                                },
                            ..
                        } if place.projections.iter().any(|projection| matches!(
                            projection.kind,
                            bc::BytecodeProjectionKind::Field(_)
                        ))
                    )
                })
            })
        }));
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::BranchTag { .. }
                )
            })
        }));
    }

    #[test]
    fn bytecode_construction_limits_fail_before_table_growth() {
        let (resolved, hir) = checked("fn main() {}\n");
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let error = lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_types: 1,
                ..BytecodeLoweringLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            BytecodeError::NodeLimit {
                resource: "type table",
                ..
            }
        ));

        let error = monomorphization_type_error(
            TypeError::ResourceLimit { limit: 1 },
            None,
            "specialization",
        );
        assert!(matches!(
            error,
            BytecodeError::NodeLimit {
                resource: "specialized type nodes",
                ..
            }
        ));
    }

    #[test]
    fn generic_functions_are_monomorphized_deduplicated_and_transitive() {
        let source = "fn identity[T](value: T): T { value }\n\
                      fn relay[T](value: T): T { identity[T](value) }\n\
                      fn recursive[T](value: T, again: Bool): T {\n\
                          if again { recursive(value, false) } else { value }\n\
                      }\n\
                      fn use(): Int {\n\
                          let one = identity(1)\n\
                          let two = identity(2)\n\
                          let text = identity(\"ok\")\n\
                          _ = text\n\
                          relay(one) + recursive(two, true)\n\
                      }\n";
        let first = lowered(source);
        let second = lowered(source);
        assert_eq!(first, second);
        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(3));

        let identity = first
            .callables
            .iter()
            .filter(|callable| callable.name.contains("::value::identity["))
            .collect::<Vec<_>>();
        assert_eq!(identity.len(), 2, "one Int and one String instance");
        assert!(
            identity.iter().all(|callable| {
                callable.generic_arity == 0 && callable.implementation.is_some()
            })
        );
        assert_eq!(
            first
                .callables
                .iter()
                .filter(|callable| callable.name.contains("::value::relay["))
                .count(),
            1
        );
        assert_eq!(
            first
                .callables
                .iter()
                .filter(|callable| callable.name.contains("::value::recursive["))
                .count(),
            1,
            "same-substitution recursion is deduplicated"
        );
        assert!(
            first
                .callables
                .iter()
                .all(|callable| callable.generic_arity == 0)
        );
        assert!(
            !first
                .types
                .iter()
                .any(|ty| { matches!(ty.kind, bc::BytecodeTypeKind::GenericParameter(_)) })
        );
        for function in &first.functions {
            for block in &function.blocks {
                let bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { callee, .. },
                            ..
                        },
                    ..
                } = &block.terminator.kind
                else {
                    continue;
                };
                if let bc::BytecodeOperandKind::Function { arguments, .. } = &callee.kind {
                    assert!(arguments.is_empty(), "monomorphic calls carry no type pack");
                }
            }
        }
    }

    #[test]
    fn source_trait_calls_dispatch_statically_to_the_selected_implementation() {
        let source = "trait Summary {\n\
                          fn summarize(self): String\n\
                      }\n\
                      type User = { name: String }\n\
                      impl Summary for User {\n\
                          fn summarize(self): String { self.name }\n\
                      }\n\
                      fn render[T: Summary](value: T): String { value.summarize() }\n\
                      fn use(): String {\n\
                          let generic = render(User { name: \"generic\" })\n\
                          assert(generic == \"generic\")\n\
                          Summary.summarize(User { name: \"qualified\" })\n\
                      }\n";
        let program = lowered(source);
        let implementation_calls = program
            .callables
            .iter()
            .filter(|callable| callable.name == "implementation#0.method#0")
            .count();
        assert_eq!(implementation_calls, 1);
        assert!(program.callables.iter().all(|callable| {
            !callable.name.contains("::type::Summary::summarize")
                || callable.implementation.is_some()
        }));
    }

    #[test]
    fn associated_trait_operations_execute_through_a_direct_static_target() {
        let source = "trait Answer {\n\
                          fn answer(): Int\n\
                      }\n\
                      type Marker = Unit\n\
                      impl Answer for Marker {\n\
                          fn answer(): Int { 42 }\n\
                      }\n\
                      fn use(): Int { Answer.answer[Marker]() }\n";
        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(42));

        let program = lowered(source);
        assert_eq!(
            program
                .callables
                .iter()
                .filter(|callable| callable.name == "implementation#0.method#0")
                .count(),
            1
        );
        assert!(
            !program
                .callables
                .iter()
                .any(|callable| callable.name.contains("::type::Answer::answer"))
        );
    }

    #[test]
    fn trait_defaults_dispatch_nested_calls_and_yield_to_overrides() {
        let source = "trait Values {\n\
                          fn base(): Int\n\
                          fn answer(): Int { Values.base[Self]() + 1 }\n\
                      }\n\
                      type Defaulted = { marker: Unit }\n\
                      type Overridden = { marker: Unit }\n\
                      impl Values for Defaulted {\n\
                          fn base(): Int { 41 }\n\
                      }\n\
                      impl Values for Overridden {\n\
                          fn base(): Int { 0 }\n\
                          fn answer(): Int { 99 }\n\
                      }\n\
                      fn use(): Int {\n\
                          Values.answer[Defaulted]() + Values.answer[Overridden]()\n\
                      }\n";
        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(141));
    }

    #[test]
    fn recursive_generic_implementation_bounds_dispatch_transitively() {
        let source = "trait Value {\n\
                          fn value(): Int\n\
                      }\n\
                      type Leaf = { marker: Unit }\n\
                      type Box[T] = { item: T }\n\
                      impl Value for Leaf {\n\
                          fn value(): Int { 42 }\n\
                      }\n\
                      impl[T: Value] Value for Box[T] {\n\
                          fn value(): Int { Value.value[T]() }\n\
                      }\n\
                      fn use(): Int { Value.value[Box[Leaf]]() }\n";
        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(42));
    }

    #[test]
    fn prelude_trait_calls_lower_to_selected_static_implementations() {
        let source = "type Label = { text: String }\n\
                      type Cursor = { value: Int }\n\
                      impl Display for Label {\n\
                          fn display(self): String { self.text }\n\
                      }\n\
                      impl Iterator[Int] for Cursor {\n\
                          fn next(mut self): Int? { none }\n\
                      }\n\
                      fn render[T: Display](value: T): String { value.display() }\n\
                      fn use_display(value: Label): String {\n\
                          let generic = render(value)\n\
                          _ = generic\n\
                          Display.display(value)\n\
                      }\n\
                      fn use_iterator(cursor: var Cursor): Int? {\n\
                          Iterator[Int].next(mut cursor)\n\
                      }\n";
        let program = lowered(source);
        let implementation_ids = program
            .callables
            .iter()
            .enumerate()
            .filter(|(_, callable)| callable.name.starts_with("implementation#"))
            .map(|(index, _)| bc::BytecodeCallableId::new(index as u32))
            .collect::<BTreeSet<_>>();
        assert_eq!(implementation_ids.len(), 2);

        let called = program
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .filter_map(|block| {
                let bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { callee, .. },
                            ..
                        },
                    ..
                } = &block.terminator.kind
                else {
                    return None;
                };
                let bc::BytecodeOperandKind::Function { callable, .. } = &callee.kind else {
                    return None;
                };
                Some(*callable)
            })
            .collect::<BTreeSet<_>>();
        assert!(implementation_ids.is_subset(&called));
        assert!(program.callables.iter().all(|callable| {
            !callable.name.contains("::type::Display::display")
                && !callable.name.contains("::type::Iterator::next")
        }));
    }

    #[test]
    fn generic_prelude_implementation_bounds_dispatch_transitively() {
        let source = "type Label = { text: String }\n\
                      type Wrapper[T] = { value: T }\n\
                      impl Display for Label {\n\
                          fn display(self): String { self.text }\n\
                      }\n\
                      impl[T: Display] Display for Wrapper[T] {\n\
                          fn display(self): String { self.value.display() }\n\
                      }\n\
                      fn use(value: Wrapper[Label]): String { Display.display(value) }\n";
        let program = lowered(source);
        let implementations = program
            .callables
            .iter()
            .filter(|callable| callable.name.starts_with("implementation#"))
            .map(|callable| callable.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(implementations.len(), 2, "{implementations:#?}");
        assert!(
            implementations
                .iter()
                .any(|name| name.contains("method#0[") && name.ends_with("::Label]")),
            "{implementations:#?}"
        );
    }

    #[test]
    fn user_iterator_for_loops_lower_through_static_next_dispatch() {
        let source = "type Cursor = { value: Int }\n\
                      impl Iterator[Int] for Cursor {\n\
                          fn next(mut self): Int? { none }\n\
                      }\n\
                      fn consume[I: Discard + Iterator[Int]](cursor: I) {\n\
                          for value in cursor {\n\
                              _ = value\n\
                          }\n\
                      }\n\
                      fn use(cursor: Cursor) { consume(cursor) }\n";
        let program = lowered(source);
        let implementation = program
            .callables
            .iter()
            .enumerate()
            .find_map(|(index, callable)| {
                (callable.name == "implementation#0.method#0")
                    .then(|| bc::BytecodeCallableId::new(index as u32))
            })
            .expect("Iterator.next implementation is monomorphized");
        let mut called_next = false;
        let mut branches_on_option = false;
        for function in &program.functions {
            for block in &function.blocks {
                assert!(
                    !matches!(
                        block.terminator.kind,
                        bc::BytecodeTerminatorKind::IteratorNext { .. }
                    ),
                    "a user Iterator must not use the intrinsic iterator terminator"
                );
                if matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::BranchTag { .. }
                ) {
                    branches_on_option = true;
                }
                let bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { callee, .. },
                            ..
                        },
                    ..
                } = &block.terminator.kind
                else {
                    continue;
                };
                if matches!(
                    &callee.kind,
                    bc::BytecodeOperandKind::Function { callable, .. }
                        if *callable == implementation
                ) {
                    called_next = true;
                }
            }
        }
        assert!(called_next);
        assert!(branches_on_option);
    }

    #[test]
    fn generic_nominals_and_projection_types_are_concrete_per_instance() {
        let source = "type Box[T] = { value: T }\n\
                      fn unwrap[T](boxed: Box[T]): T { boxed.value }\n\
                      fn use(): String {\n\
                          let number = unwrap(Box[Int] { value: 42 })\n\
                          assert(number == 42)\n\
                          unwrap(Box[String] { value: \"ready\" })\n\
                      }\n";
        let program = lowered(source);
        assert_eq!(
            execute_function(source, "use"),
            RuntimeValue::String("ready".into())
        );
        assert_eq!(
            program
                .callables
                .iter()
                .filter(|callable| callable.name.contains("::value::unwrap["))
                .count(),
            2
        );
        for function in &program.functions {
            for ty in &function.types {
                assert!(
                    !matches!(
                        program.types[ty.index() as usize].kind,
                        bc::BytecodeTypeKind::GenericParameter(_)
                    ),
                    "an executable function retained a generic type: {}",
                    program.types[ty.index() as usize].name
                );
            }
        }
    }

    #[test]
    fn generic_checked_operations_and_tag_projections_are_specialized() {
        let source = "fn first[T](values: Array[T]): T { values[0] }\n\
                      fn value_or[T](value: T?, fallback: T): T {\n\
                          match value {\n\
                              some(item) => item\n\
                              none => fallback\n\
                          }\n\
                      }\n\
                      fn use(): String {\n\
                          value_or(some(first([\"ready\"])), \"missing\")\n\
                      }\n";
        assert_eq!(
            execute_function(source, "use"),
            RuntimeValue::String("ready".into())
        );
        let program = lowered(source);
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::Invoke {
                        operation: bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Index { .. },
                            ..
                        },
                        ..
                    }
                )
            })
        }));
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::BranchTag { .. }
                )
            })
        }));
    }

    #[test]
    fn generic_function_constants_root_their_concrete_instance() {
        let source = "fn identity[T](value: T): T { value }\n\
                      const Handler: fn(Int): Int = identity[Int]\n\
                      fn use(): Int { Handler(42) }\n";
        let program = lowered(source);
        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(42));
        assert_eq!(
            program
                .callables
                .iter()
                .filter(|callable| callable.name.contains("::value::identity[Int]"))
                .count(),
            1
        );
        let function_constant = program.constants.iter().find(|constant| {
            matches!(
                constant.value.kind,
                bc::BytecodeConstantValueKind::Function { .. }
            )
        });
        assert!(function_constant.is_some());
    }

    #[test]
    fn uniform_named_function_values_execute_through_every_supported_origin() {
        let source = "trait Factory {\n\
                          fn create(): Self\n\
                          fn offset(): Int { 2 }\n\
                      }\n\
                      type Item = { value: Int }\n\
                      impl Factory for Item {\n\
                          fn create(): Item { Item { value: 20 } }\n\
                      }\n\
                      type Box[T] = { value: T }\n\
                      fn Box[T].wrap(value: T): Box[T] { Box { value } }\n\
                      fn identity[T: Copy](value: T): T { value }\n\
                      const Identity: fn(Int): Int = identity\n\
                      const Wrap: fn(Int): Box[Int] = Box.wrap\n\
                      const Make: fn(): Item = Factory.create[Item]\n\
                      const Offset: fn(): Int = Factory.offset[Item]\n\
                      fn apply(operation: fn(Int): Int, value: Int): Int { operation(value) }\n\
                      fn use(): Int {\n\
                          let wrap: fn(Int): Box[Int] = Box.wrap\n\
                          let make: fn(): Item = Factory.create[Item]\n\
                          apply(\n\
                              identity,\n\
                              Identity(\n\
                                  Wrap(wrap(make().value + Make().value + Offset()).value).value,\n\
                              ),\n\
                          )\n\
                      }\n";

        assert_eq!(execute_function(source, "use"), RuntimeValue::Integer(42));
        let program = lowered(source);
        assert!(program.constants.iter().any(|constant| matches!(
            constant.value.kind,
            bc::BytecodeConstantValueKind::Function { .. }
        )));
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::Invoke {
                        operation: bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call {
                                callee: bc::BytecodeOperand {
                                    kind: bc::BytecodeOperandKind::Copy(_)
                                        | bc::BytecodeOperandKind::Move(_),
                                    ..
                                },
                                ..
                            },
                            ..
                        },
                        ..
                    }
                )
            })
        }));
    }

    #[test]
    fn zero_generic_budget_accepts_plain_code_and_rejects_the_first_instance() {
        let (resolved, hir) = checked("fn main() {}\n");
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_generic_instantiations: 0,
                ..BytecodeLoweringLimits::default()
            },
        )
        .unwrap();

        let (resolved, hir) =
            checked("fn identity[T](value: T): T { value }\nfn main(): Int { identity(1) }\n");
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let error = lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_generic_instantiations: 0,
                ..BytecodeLoweringLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            BytecodeError::NodeLimit {
                resource: "generic instantiations",
                ..
            }
        ));
    }

    #[test]
    fn expanding_generic_recursion_stops_at_the_instantiation_budget() {
        let source = "fn expand[T: Discard](value: T) {\n\
                          let wrapped = some(value)\n\
                          expand(wrapped)\n\
                      }\n\
                      fn main() {\n\
                          expand(1)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let error = lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_generic_instantiations: 3,
                ..BytecodeLoweringLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            BytecodeError::NodeLimit {
                resource: "generic instantiations",
                ..
            }
        ));
    }

    #[test]
    fn verifier_rejects_invalid_targets_types_and_missing_definitions() {
        let mut invalid_target =
            lowered("fn choose(flag: Bool): Int {\n    if flag { 1 } else { 2 }\n}\n");
        let function = &mut invalid_target.functions[0];
        let branch = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::BranchBool { if_true, .. } => Some(if_true),
                _ => None,
            })
            .unwrap();
        *branch = bc::BytecodeBlockId::new(u32::MAX);
        assert!(bc::verify_bytecode(&invalid_target).is_err());

        let mut invalid_type = lowered("fn main() {}\n");
        invalid_type.functions[0].slots[0].ty = bc::BytecodeTypeId::new(u32::MAX);
        assert!(bc::verify_bytecode(&invalid_type).is_err());

        let mut undefined_return = lowered("fn answer(): Int { 42 }\n");
        for block in &mut undefined_return.functions[0].blocks {
            block.instructions.clear();
        }
        let error = bc::verify_bytecode(&undefined_return).unwrap_err();
        assert!(error.message().contains("dominating live definition"));
    }

    #[test]
    fn bytecode_verifier_rejects_forged_closure_capture_schemas() {
        let source = "fn build() {\n\
                          let seed = 41\n\
                          let closure = (): Int { seed + 1 }\n\
                          _ = closure\n\
                      }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();

        fn closure_schema(
            program: &mut bc::BytecodeProgram,
        ) -> (
            bc::BytecodeTypeId,
            &mut bc::BytecodeCallableId,
            &mut Vec<bc::BytecodeTypeId>,
        ) {
            program
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.instructions)
                .find_map(|instruction| match &mut instruction.kind {
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                ty,
                                    kind:
                                        bc::BytecodeRvalueKind::Construct {
                                            shape:
                                                bc::BytecodeAggregateKind::Closure {
                                                    callable,
                                                    captures,
                                                },
                                            ..
                                        },
                                },
                            ..
                        } => Some((*ty, callable, captures)),
                    _ => None,
                })
                .expect("closure construction lowers to bytecode")
        }

        let mut wrong_count = program.clone();
        closure_schema(&mut wrong_count).2.clear();
        let error = bc::verify_bytecode(&wrong_count).unwrap_err();
        assert!(error.message().contains("rvalue"));

        let mut wrong_type = program.clone();
        let (closure_type, _, captures) = closure_schema(&mut wrong_type);
        captures[0] = closure_type;
        let error = bc::verify_bytecode(&wrong_type).unwrap_err();
        assert!(error.message().contains("rvalue"));

        let mut wrong_callable = program.clone();
        let named = bc::BytecodeCallableId::new(
            wrong_callable
                .callables
                .iter()
                .position(|callable| callable.closure.is_none())
                .unwrap() as u32,
        );
        *closure_schema(&mut wrong_callable).1 = named;
        let error = bc::verify_bytecode(&wrong_callable).unwrap_err();
        assert!(error.message().contains("rvalue"));

        let mut wrong_protocols = program;
        wrong_protocols
            .callables
            .iter_mut()
            .find_map(|callable| callable.closure.as_mut())
            .unwrap()
            .protocols
            .call = false;
        let error = bc::verify_bytecode(&wrong_protocols).unwrap_err();
        assert!(error.message().contains("implementation body"));
    }

    #[test]
    fn bytecode_verifier_rederives_indirect_call_signature_and_protocol() {
        fn indirect_call(
            program: &mut bc::BytecodeProgram,
        ) -> (
            &mut bc::BytecodeOperand,
            &mut bc::BytecodeTypeId,
            &mut bc::BytecodeCallProtocol,
        ) {
            program
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .find_map(|block| match &mut block.terminator.kind {
                    bc::BytecodeTerminatorKind::Invoke {
                        operation:
                            bc::BytecodeOperation {
                                kind:
                                    bc::BytecodeOperationKind::Call {
                                        callee,
                                        signature,
                                        protocol,
                                        ..
                                    },
                                ..
                            },
                        ..
                    } if !matches!(callee.kind, bc::BytecodeOperandKind::Function { .. }) => {
                        Some((callee, signature, protocol))
                    }
                    _ => None,
                })
                .expect("program contains one indirect closure call")
        }

        let pure = lowered(
            "fn execute(): Int {\n\
                 let operation = (value: Int): Int { value + 1 }\n\
                 operation(41)\n\
             }\n",
        );
        let mut wrong_selection = pure.clone();
        let (callee, _, protocol) = indirect_call(&mut wrong_selection);
        let bc::BytecodeOperandKind::Borrow(place) = &callee.kind else {
            panic!("a closure place call borrows its environment")
        };
        callee.kind = bc::BytecodeOperandKind::Copy(place.clone());
        *protocol = bc::BytecodeCallProtocol::CallOnce;
        let error = bc::verify_bytecode(&wrong_selection).unwrap_err();
        assert!(error.message().contains("operation"));

        let mut wrong_signature = pure;
        let int = wrong_signature
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Int)
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .unwrap();
        *indirect_call(&mut wrong_signature).1 = int;
        let error = bc::verify_bytecode(&wrong_signature).unwrap_err();
        assert!(error.message().contains("operation"));

        let mut wrong_stateful = lowered(
            "fn execute(): Int {\n\
                 var count = 0\n\
                 var next = (): Int {\n\
                     count += 1\n\
                     count\n\
                 }\n\
                 next()\n\
             }\n",
        );
        *indirect_call(&mut wrong_stateful).2 = bc::BytecodeCallProtocol::Call;
        let error = bc::verify_bytecode(&wrong_stateful).unwrap_err();
        assert!(error.message().contains("operation"));
    }

    #[test]
    fn bytecode_verifier_confines_environment_borrows_to_indirect_callees() {
        let mut program = lowered(
            "fn execute(): Int {\n\
                 let operation = (value: Int): Int { value + 1 }\n\
                 operation(41)\n\
             }\n",
        );
        let (callee, arguments) = program
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind:
                                bc::BytecodeOperationKind::Call {
                                    callee, arguments, ..
                                },
                            ..
                        },
                    ..
                } if matches!(callee.kind, bc::BytecodeOperandKind::Borrow(_)) => {
                    Some((callee, arguments))
                }
                _ => None,
            })
            .expect("closure place call borrows its environment");
        let bc::BytecodeOperandKind::Borrow(environment) = &callee.kind else {
            unreachable!()
        };
        arguments[0].value.kind = bc::BytecodeOperandKind::Borrow(environment.clone());

        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(error.message().contains("borrow escapes"));
    }

    #[test]
    fn verifier_rejects_call_arity_and_unguarded_payload_projection() {
        let mut invalid_call = lowered(
            "fn add(left: Int, right: Int): Int { left + right }\n\
             fn use(): Int { add(20, 22) }\n",
        );
        let arguments = invalid_call
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => Some(arguments),
                _ => None,
            })
            .unwrap();
        arguments.pop();
        assert!(bc::verify_bytecode(&invalid_call).is_err());

        let mut invalid_payload = lowered(
            "enum Choice { Empty, Item(Int) }\n\
             fn choose(value: Choice): Int {\n\
                 match value {\n\
                     Choice.Empty => 0\n\
                     Choice.Item(number) => number\n\
                 }\n\
             }\n",
        );
        let function = &mut invalid_payload.functions[0];
        let payload_read = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind:
                                bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                    kind:
                                        bc::BytecodeOperandKind::Copy(place)
                                        | bc::BytecodeOperandKind::Move(place),
                                    ..
                                }),
                            ..
                        },
                    ..
                } => place.projections.iter().any(|projection| {
                    matches!(
                        projection.kind,
                        bc::BytecodeProjectionKind::VariantTuple { .. }
                    )
                }),
                _ => false,
            })
            .cloned()
            .unwrap();
        function.blocks[function.entry.index() as usize]
            .instructions
            .push(payload_read);
        let error = bc::verify_bytecode(&invalid_payload).unwrap_err();
        assert!(
            error.message().contains("dominating matching BranchTag"),
            "{error:?}"
        );
    }

    #[test]
    fn verifier_budget_and_disassembler_are_explicit_tooling_boundaries() {
        let program = lowered("fn answer(): Int { 20 + 22 }\n");
        let error = bc::verify_bytecode_with_limits(
            &program,
            bc::BytecodeVerificationLimits {
                max_dataflow_steps: 0,
            },
        )
        .unwrap_err();
        assert!(error.is_resource_limit());

        let first = bc::disassemble(&program);
        let second = bc::disassemble(&program);
        assert_eq!(first, second);
        assert!(first.starts_with("; Tondo bootstrap bytecode (tooling only)\n"));
        assert!(first.contains("function f0"));
        assert!(first.contains("invoke CheckedBinary"));
        assert!(first.contains("resume_panic"));
    }

    #[test]
    fn every_material_bootstrap_control_and_value_family_reaches_verified_bytecode() {
        for source in [
            "fn index(): Int { 0 }\n\
             fn replacement(): Int { 3 }\n\
             fn update(values: var Array[Int]) {\n\
                 var left = 1\n\
                 var right = 2\n\
                 values[index()] = replacement()\n\
                 left += right\n\
                 (left, right) = (right, left)\n\
             }\n\
             fn read(values: Array[Int], position: Int): Int { values[position] }\n\
             fn view(values: Array[Int]): Array[Int] { values[1:] }\n",
            "const Answer: Int = 42\n\
             fn collections(): (Array[Int], Map[String, Int?], Set[String]) {\n\
                 ([1, Answer], [\"one\": 1, \"none\": none], Set[\"read\", \"write\"])\n\
             }\n\
             fn inspect(): Bool {\n\
                 let numbers = 0..10\n\
                 let ages = [\"Ada\": 37]\n\
                 let permissions = Set[\"read\", \"write\"]\n\
                 5 in numbers and \"Ada\" in ages and\n\
                     \"read\" in permissions and 'x' in \"text\"\n\
             }\n",
            "fn source(): Int ! String { 1 }\n\
             fn optional(): Int? { some(1) }\n\
             fn widen(): Int ! (Bool | String) { source()? }\n\
             fn unwrap_optional(): Int? { optional()? }\n\
             fn nested(): Int? ! String { optional()? }\n\
             fn widen_number(value: Int32): Int { Int(value) }\n\
             fn narrow(value: Int): Int8 ! NumericConversionError { Int8(value) }\n",
            "type Counter = { value: Int }\n\
             fn Counter.add(self, amount: Int): Int { self.value + amount }\n\
             fn connect(host: String, port: Int): String { host }\n\
             fn log(prefix: String, parts: ...String): Array[String] { parts }\n\
             fn use(counter: Counter): Int {\n\
                 _ = connect(port: 8080, host: \"localhost\")\n\
                 let parts = [\"server\", \" started\"]\n\
                 _ = log(\"Info: \", ...parts)\n\
                 counter.add(amount: 3)\n\
             }\n",
            "fn loops(values: Array[Int], entries: Map[String, Int], unique: Set[Int], numbers: Range[Int], text: String) {\n\
                 for {\n\
                     break\n\
                 }\n\
                 for false {\n\
                     continue\n\
                 }\n\
                 for value in values {\n\
                     _ = value\n\
                 }\n\
                 for entry in entries {\n\
                     _ = entry\n\
                 }\n\
                 for value in unique {\n\
                     _ = value\n\
                 }\n\
                 for value in numbers {\n\
                     _ = value\n\
                 }\n\
                 for character in text {\n\
                     _ = character\n\
                 }\n\
             }\n",
        ] {
            let program = lowered(source);
            bc::verify_bytecode(&program).unwrap();
        }
    }

    #[test]
    fn verified_bytecode_executes_real_frames_calls_and_checked_arithmetic() {
        let value = execute_function(
            "fn add(left: Int, right: Int): Int { left + right }\n\
             fn answer(): Int { add(20, 22) }\n",
            "answer",
        );
        assert_eq!(value, RuntimeValue::Integer(42));

        let sum = execute_function(
            "fn sum(): Int {\n\
                 var total = 0\n\
                 for value in [1, 2, 3, 4] {\n\
                     total += value\n\
                 }\n\
                 total\n\
             }\n",
            "sum",
        );
        assert_eq!(sum, RuntimeValue::Integer(10));
    }

    #[test]
    fn verified_bytecode_executes_the_bootstrap_scalar_and_tuple_values() {
        let value = execute_function(
            "fn values(): (Unit, Bool, Int, Float, String) {\n\
                 ((), true, 42, 1.5, \"Tondo\")\n\
             }\n",
            "values",
        );
        assert_eq!(
            value,
            RuntimeValue::Tuple(vec![
                RuntimeValue::Unit,
                RuntimeValue::Bool(true),
                RuntimeValue::Integer(42),
                RuntimeValue::Float(1.5),
                RuntimeValue::String("Tondo".into()),
            ])
        );
    }

    #[test]
    fn verified_bytecode_executes_records_enums_options_results_and_collections() {
        let value = execute_function(
            "type Pair = { left: Int, right: Int }\n\
             enum Choice { Empty, Item(Pair) }\n\
             fn make(): Choice { Choice.Item(Pair { left: 20, right: 22 }) }\n\
             fn inspect(): Int {\n\
                 let selected = match make() {\n\
                     Choice.Empty => 0\n\
                     Choice.Item(pair) => pair.left + pair.right\n\
                 }\n\
                 let values = [selected, 7, 9]\n\
                 if selected in values { values[0] } else { 0 }\n\
             }\n",
            "inspect",
        );
        assert_eq!(value, RuntimeValue::Integer(42));

        let result = execute_function(
            "fn source(): Int ! String { ok(42) }\n\
             fn forward(): Int ! String { source()? }\n",
            "forward",
        );
        assert_eq!(
            result,
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(42)))
        );
    }

    #[test]
    fn structural_equality_executes_for_every_closed_aggregate_shape() {
        let value = execute_function(
            "type User = { id: Int, name: String }\n\
             type Node = { value: Int, next: Node? }\n\
             fn compare(): Bool {\n\
                 let left = User { id: 1, name: \"Tony\" }\n\
                 let right = User { id: 1, name: \"Tony\" }\n\
                 let firstNode = Node { value: 1, next: none }\n\
                 let secondNode = Node { value: 1, next: none }\n\
                 let firstMap = [\"one\": 1, \"two\": 2]\n\
                 let secondMap = [\"two\": 2, \"one\": 1]\n\
                 let firstSet = Set[\"one\", \"two\"]\n\
                 let secondSet = Set[\"two\", \"one\"]\n\
                 left == right\n\
                     and some(left) == some(right)\n\
                     and firstNode == secondNode\n\
                     and [1, 2] == [1, 2]\n\
                     and [1, 2] != [2, 1]\n\
                     and firstMap == secondMap\n\
                     and firstSet == secondSet\n\
             }\n",
            "compare",
        );
        assert_eq!(value, RuntimeValue::Bool(true));
    }

    #[test]
    fn bytecode_verifier_rederives_closed_capability_contracts() {
        let mut malformed = lowered("fn types(float: Float, integer: Int) {}\n");
        let float = malformed
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Float)
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .unwrap();
        let integer = malformed
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Int)
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .unwrap();
        malformed.types.push(bc::BytecodeType {
            name: "malicious::Map[Float, Int]".into(),
            kind: bc::BytecodeTypeKind::Intrinsic {
                constructor: bc::BytecodeIntrinsicType::Map,
                arguments: vec![float, integer],
            },
        });
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("Map key"));

        let mut operation = lowered(
            "fn compare(\n\
                 left: fn(Int): Int,\n\
                 right: fn(Int): Int,\n\
             ): Bool {\n\
                 _ = left\n\
                 _ = right\n\
                 1 == 1\n\
             }\n",
        );
        let function = function_id(&operation, "compare");
        let function = &mut operation.functions[function.index() as usize];
        let left_slot = function.parameters[0];
        let right_slot = function.parameters[1];
        let function_type = function.slots[left_slot.index() as usize].ty;
        let binary = function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| {
                let bc::BytecodeInstructionKind::Store { value, .. } = &mut instruction.kind else {
                    return None;
                };
                matches!(
                    value.kind,
                    bc::BytecodeRvalueKind::Binary {
                        operator: bc::BytecodeBinaryOperator::Equal,
                        ..
                    }
                )
                .then_some(value)
            })
            .expect("comparison lowers to a binary rvalue");
        let bc::BytecodeRvalueKind::Binary { left, right, .. } = &mut binary.kind else {
            unreachable!()
        };
        *left = bc::BytecodeOperand {
            ty: function_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: left_slot,
                ty: function_type,
                projections: Vec::new(),
            }),
        };
        *right = bc::BytecodeOperand {
            ty: function_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: right_slot,
                ty: function_type,
                projections: Vec::new(),
            }),
        };
        let error = bc::verify_bytecode(&operation).unwrap_err();
        assert!(error.message().contains("rvalue"));
    }

    #[test]
    fn runtime_arithmetic_and_bounds_fail_with_normative_panic_classes() {
        for (source, expected) in [
            (
                "fn explode(): Int8 { let maximum = 127i8\n maximum + 1i8 }\n",
                PanicCode::CheckedOverflow,
            ),
            (
                "fn explode(): Int { let zero = 0\n 42 / zero }\n",
                PanicCode::IntegerDivisionByZero,
            ),
            (
                "fn explode(): Int { let values = [1]\n values[2] }\n",
                PanicCode::Bounds,
            ),
            (
                "fn explode(): Array[Int] { let values = [1, 2]\n values[::0] }\n",
                PanicCode::ZeroSliceStep,
            ),
            (
                "fn explode(): Int { let count = 64\n 1 << count }\n",
                PanicCode::InvalidShiftCount,
            ),
        ] {
            let VmOutcome::Panicked(panic) = execute_outcome(source, "explode") else {
                panic!("expected {expected:?} for {source}");
            };
            assert_eq!(panic.code, expected, "{source}");
            assert_eq!(panic.code.code(), expected.code());
            assert!(!panic.stack.is_empty());
        }

        let VmOutcome::Panicked(overlap) = execute_outcome(
            "fn left(): Int { 0 }\n\
             fn right(): Int { 0 }\n\
             fn explode() {\n\
                 var values = [0, 0]\n\
                 (values[left()], values[right()]) = (1, 2)\n\
             }\n",
            "explode",
        ) else {
            panic!("a dynamically overlapping assignment must panic")
        };
        assert_eq!(overlap.code, PanicCode::OverlappingBorrow);
        assert_eq!(overlap.code.code(), "P0004");
    }

    #[test]
    fn runtime_validates_slice_assignment_after_the_rhs_and_before_any_write() {
        let value = execute_function(
            "fn replace(): Array[Int] {\n\
                 var values = [1, 2, 3]\n\
                 values[1:3] = values[0:2]\n\
                 values\n\
             }\n",
            "replace",
        );
        assert_eq!(
            value,
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(2),
            ])
        );

        let VmOutcome::Panicked(mismatch) = execute_outcome(
            "fn replace() {\n\
                 var values = [1, 2, 3]\n\
                 values[0:2] = [9]\n\
             }\n",
            "replace",
        ) else {
            panic!("a mismatched slice assignment must panic")
        };
        assert_eq!(mismatch.code, PanicCode::ArrayShapeMismatch);
        assert!(mismatch.message.contains("destination length 2"));
        assert!(mismatch.message.contains("replacement length 1"));

        let VmOutcome::Panicked(rhs) = execute_outcome(
            "fn replace() {\n\
                 var values = [1]\n\
                 values[99] = panic(\"rhs-first\")\n\
             }\n",
            "replace",
        ) else {
            panic!("the diverging RHS must panic")
        };
        assert_eq!(rhs.code, PanicCode::ExplicitPanic);
        assert_eq!(rhs.message, "rhs-first");
    }

    #[test]
    fn runtime_map_duplicate_policy_is_explicit_and_supports_p0009() {
        let source = "fn key(): String { \"same\" }\n\
                      fn build(): Map[String, Int] { [key(): 1, key(): 2] }\n";
        assert_eq!(
            execute_function(source, "build"),
            RuntimeValue::Map(vec![(
                RuntimeValue::String("same".into()),
                RuntimeValue::Integer(2),
            )])
        );

        let mut program = lowered(source);
        let entry = function_id(&program, "build");
        let reject = program.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind:
                                bc::BytecodeOperationKind::BuildMap {
                                    reject_dynamic_duplicates,
                                    ..
                                },
                            ..
                        },
                    ..
                } => Some(reject_dynamic_duplicates),
                _ => None,
            })
            .expect("the map literal lowers to a checked map construction");
        *reject = true;
        let mut host = RejectingHost;
        let outcome = execute(&program, entry, &mut host).unwrap().outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("a rejecting map construction must panic on a duplicate key")
        };
        assert_eq!(panic.code, PanicCode::DuplicateDynamicMapKey);
        assert_eq!(panic.code.code(), "P0009");
    }

    #[test]
    fn runtime_executes_assert_and_concatenates_variadic_message_parts() {
        let value = execute_function(
            "fn answer(): Int {\n\
                 assert(20 + 22 == 42, \"unreachable\")\n\
                 42\n\
             }\n",
            "answer",
        );
        assert_eq!(value, RuntimeValue::Integer(42));

        let VmOutcome::Panicked(panic) = execute_outcome(
            "fn explode() {\n\
                 let parts = [\"middle\", \"end\"]\n\
                 assert(false, \"start\", ...parts)\n\
             }\n",
            "explode",
        ) else {
            panic!("failed assertion must panic");
        };
        assert_eq!(panic.code, PanicCode::AssertionFailed);
        assert_eq!(panic.message, "startmiddleend");
        assert_eq!(panic.stack.len(), 1);
        assert!(panic.stack[0].function.ends_with("::value::explode"));

        let VmOutcome::Panicked(default) = execute_outcome(
            "fn default_message() { assert(20 + 20 == 42) }\n",
            "default_message",
        ) else {
            panic!("failed assertion without message parts must panic");
        };
        assert_eq!(default.code, PanicCode::AssertionFailed);
        assert_eq!(default.message, "assertion failed: 20 + 20 == 42");
        assert_eq!(default.span, default.stack[0].span);
    }

    #[test]
    fn runtime_explicit_panic_preserves_message_and_canonical_stack() {
        let VmOutcome::Panicked(panic) = execute_outcome(
            "fn inner(): Never { panic(\"boom\") }\n\
             fn outer() { inner() }\n",
            "outer",
        ) else {
            panic!("explicit panic must unwind to the root");
        };
        assert_eq!(panic.code, PanicCode::ExplicitPanic);
        assert_eq!(panic.message, "boom");
        assert_eq!(panic.stack.len(), 2);
        assert!(panic.stack[0].function.ends_with("::value::inner"));
        assert!(panic.stack[1].function.ends_with("::value::outer"));
        assert_eq!(panic.span, panic.stack[0].span);
    }

    #[test]
    fn runtime_invokes_the_typed_console_print_host_boundary() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
            calls: usize,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one snapshotted String argument")
                };
                self.output.push_str(text);
                self.calls += 1;
                Ok(RuntimeValue::Unit)
            }
        }

        let program = lowered(
            "import std.console\n\
             fn main() {\n\
                 console.print(\"Hello\")\n\
                 console.print(\", Tondo!\")\n\
             }\n",
        );
        let entry = function_id(&program, "main");
        let mut host = RecordingHost::default();
        let execution = execute(&program, entry, &mut host)
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit));
        assert_eq!(host.calls, 2);
        assert_eq!(host.output, "Hello, Tondo!");
    }

    #[test]
    fn runtime_collects_unreachable_program_objects_under_allocation_pressure() {
        let program = lowered(
            "fn collect(): Int {\n\
                 var count = 0\n\
                 for count < 200 {\n\
                     _ = [count, count + 1]\n\
                     count += 1\n\
                 }\n\
                 count\n\
             }\n",
        );
        let entry = function_id(&program, "collect");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 16,
                max_heap_bytes: 64 * 1024,
                initial_gc_threshold: 2,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(200))
        );
        assert!(execution.statistics.collections > 0);
        assert!(execution.statistics.reclaimed_objects > 0);
        assert!(execution.statistics.peak_live_objects <= 16);
    }

    #[test]
    fn closure_capture_temporaries_survive_gc_pressure() {
        let program = lowered(
            "fn main() {\n\
                 let a = [1]\n\
                 let b = [2]\n\
                 let c = [3]\n\
                 let d = [4]\n\
                 let e = [5]\n\
                 let f = [6]\n\
                 let g = [7]\n\
                 let h = [8]\n\
                 let closure = () {\n\
                     _ = a\n\
                     _ = b\n\
                     _ = c\n\
                     _ = d\n\
                     _ = e\n\
                     _ = f\n\
                     _ = g\n\
                     _ = h\n\
                 }\n\
                 let copied = closure\n\
                 _ = closure\n\
                 _ = copied\n\
             }\n",
        );
        let entry = function_id(&program, "main");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 256,
                max_heap_bytes: 64 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit));
        assert!(execution.statistics.collections > 0);
    }

    #[test]
    fn closures_execute_with_call_call_mut_call_once_and_fn_erasure_semantics() {
        assert_eq!(
            execute_function(
                "fn pure(): Int {\n\
                     let offset = 40\n\
                     let add = (value: Int): Int { offset + value }\n\
                     add(2)\n\
                 }\n",
                "pure",
            ),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(
                "fn stateful(): Int {\n\
                     var count = 0\n\
                     var next = (): Int {\n\
                         count += 1\n\
                         count\n\
                     }\n\
                     next() + next()\n\
                 }\n",
                "stateful",
            ),
            RuntimeValue::Integer(3)
        );
        assert_eq!(
            execute_function(
                "fn copied_once(): Int {\n\
                     var count = 0\n\
                     let next = (): Int {\n\
                         count += 1\n\
                         count\n\
                     }\n\
                     next() + next()\n\
                 }\n",
                "copied_once",
            ),
            RuntimeValue::Integer(2)
        );
        assert_eq!(
            execute_function(
                "fn erased(): Int {\n\
                     let offset = 40\n\
                     let add: fn(Int): Int = (value) { offset + value }\n\
                     add(2)\n\
                 }\n",
                "erased",
            ),
            RuntimeValue::Integer(42)
        );
    }

    #[test]
    fn generic_opaque_and_variadic_closure_calls_use_the_same_indirect_path() {
        assert_eq!(
            execute_function(
                "fn increment(value: Int): Int { value + 1 }\n\
                 fn apply[F: Call[fn(Int): Int]](operation: F, value: Int): Int {\n\
                     operation(value)\n\
                 }\n\
                 fn execute(): (Int, Int) {\n\
                     let offset = 2\n\
                     let closure = (value: Int): Int { value + offset }\n\
                     (apply(closure, 40), apply(increment, 41))\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Tuple(vec![RuntimeValue::Integer(42), RuntimeValue::Integer(42)])
        );
        assert_eq!(
            execute_function(
                "fn make(offset: Int): impl Call[fn(Int): Int] + Discard {\n\
                     (value: Int): Int { value + offset }\n\
                 }\n\
                 fn execute(): Int {\n\
                     let operation = make(40)\n\
                     operation(2)\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(
                "fn invoke[T: Copy + Discard](value: T): T {\n\
                     let get = (): T { value }\n\
                     get()\n\
                 }\n\
                 fn execute(): (Int, Bool) { (invoke(42), invoke(true)) }\n",
                "execute",
            ),
            RuntimeValue::Tuple(vec![RuntimeValue::Integer(42), RuntimeValue::Bool(true)])
        );
        assert_eq!(
            execute_function(
                "fn execute(): Int {\n\
                     let sum = (head: Int, tail: ...Int): Int {\n\
                         head + tail[0] + tail[1]\n\
                     }\n\
                     sum(10, 20, 12)\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(42)
        );
    }

    #[test]
    fn borrowed_closure_environments_remain_rooted_during_argument_gc_pressure() {
        let program = lowered(
            "fn execute(): Int {\n\
                 let anchor = [40, 2]\n\
                 var count = 0\n\
                 var next = (items: ...Array[Int]): Int {\n\
                     _ = anchor\n\
                     _ = items\n\
                     count += 1\n\
                     count\n\
                 }\n\
                 for _ in 0..200 {\n\
                     _ = next([1], [2], [3])\n\
                 }\n\
                 next([4], [5], [6])\n\
             }\n",
        );
        let entry = function_id(&program, "execute");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 128,
                max_heap_bytes: 128 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(201))
        );
        assert!(execution.statistics.collections > 0);
        assert!(execution.statistics.reclaimed_objects > 0);
    }

    #[test]
    fn runtime_rejects_invalid_bytecode_before_execution() {
        #[derive(Default)]
        struct CountingHost {
            calls: usize,
        }

        impl VmHost for CountingHost {
            fn invoke(
                &mut self,
                _name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                self.calls += 1;
                Ok(RuntimeValue::Unit)
            }
        }

        let mut program =
            lowered("import std.console\nfn main() { console.print(\"must not execute\") }\n");
        let entry = function_id(&program, "main");
        program.functions[entry.index() as usize].entry = bc::BytecodeBlockId::new(u32::MAX);
        let mut host = CountingHost::default();
        let error = execute(&program, entry, &mut host).unwrap_err();
        assert!(matches!(error, VmError::InvalidBytecode(_)));
        assert_eq!(host.calls, 0);

        let mut program = lowered(
            "fn replace() {\n\
                 var values = [1, 2]\n\
                 values[:] = [3, 4]\n\
             }\n",
        );
        let entry = function_id(&program, "replace");
        let replacement = program.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidatePlaces {
                    replacements,
                    for_write: true,
                    ..
                } => replacements
                    .iter_mut()
                    .find(|replacement| replacement.is_some()),
                _ => None,
            })
            .expect("slice assignment has a checked replacement");
        *replacement = None;
        let error = execute(&program, entry, &mut host).unwrap_err();
        assert!(matches!(error, VmError::InvalidBytecode(_)));

        let mut program = lowered("fn check() { assert(true) }\n");
        let entry = function_id(&program, "check");
        let condition_repr = program.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Assert { condition_repr, .. },
                            ..
                        },
                    ..
                } => Some(condition_repr),
                _ => None,
            })
            .expect("assert lowers to a checked operation");
        condition_repr.clear();
        let error = execute(&program, entry, &mut host).unwrap_err();
        assert!(matches!(error, VmError::InvalidBytecode(_)));
    }

    #[test]
    fn opaque_results_execute_as_zero_cost_sealed_witnesses() {
        let source = "fn hidden(): impl Discard { 42 }\n";
        assert_eq!(
            execute_function(source, "hidden"),
            RuntimeValue::Integer(42)
        );
        let program = lowered(source);
        assert!(
            program
                .types
                .iter()
                .any(|ty| matches!(ty.kind, bc::BytecodeTypeKind::OpaqueResult { .. }))
        );
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction.kind,
                        bc::BytecodeInstructionKind::Store {
                            value: bc::BytecodeRvalue {
                                kind: bc::BytecodeRvalueKind::Coerce {
                                    kind: bc::BytecodeCoercion::Opaque,
                                    ..
                                },
                                ..
                            },
                            ..
                        }
                    )
                })
            })
        }));
        let tooling = bc::disassemble(&program);
        assert!(tooling.contains("OpaqueResult"));
        assert!(!tooling.contains("witness:"));
    }

    #[test]
    fn bytecode_verifier_rejects_mutated_opaque_metadata_and_seals() {
        let source = "fn hidden(): impl Discard { 42 }\n\
                      fn text(): String { \"available type\" }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();
        let opaque = program
            .types
            .iter()
            .position(|ty| matches!(ty.kind, bc::BytecodeTypeKind::OpaqueResult { .. }))
            .unwrap();
        let string = program
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::String)
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .unwrap();

        let mut wrong_witness = program.clone();
        let bc::BytecodeTypeKind::OpaqueResult { witness, .. } =
            &mut wrong_witness.types[opaque].kind
        else {
            unreachable!()
        };
        *witness = string;
        let error = bc::verify_bytecode(&wrong_witness).unwrap_err();
        assert!(
            error.message().contains("coercion")
                || error.message().contains("opaque")
                || error.message().contains("rvalue"),
            "{error:?}"
        );

        let mut cyclic = program.clone();
        let bc::BytecodeTypeKind::OpaqueResult { witness, .. } = &mut cyclic.types[opaque].kind
        else {
            unreachable!()
        };
        *witness = bc::BytecodeTypeId::new(opaque as u32);
        let error = bc::verify_bytecode(&cyclic).unwrap_err();
        assert!(error.message().contains("form a cycle"));

        let mut generic = program.clone();
        let generic_id = bc::BytecodeTypeId::new(generic.types.len() as u32);
        generic.types.push(bc::BytecodeType {
            name: "$malicious".into(),
            kind: bc::BytecodeTypeKind::GenericParameter(0),
        });
        let bc::BytecodeTypeKind::OpaqueResult { arguments, .. } = &mut generic.types[opaque].kind
        else {
            unreachable!()
        };
        arguments.push(generic_id);
        let error = bc::verify_bytecode(&generic).unwrap_err();
        assert!(error.message().contains("retains a generic parameter"));

        let mut duplicate = program.clone();
        let mut duplicated = duplicate.types[opaque].clone();
        duplicated.name.push_str("#duplicate");
        duplicate.types.push(duplicated);
        let error = bc::verify_bytecode(&duplicate).unwrap_err();
        assert!(
            error
                .message()
                .contains("family and arguments are duplicated")
        );

        let mut wrong_seal = program;
        let seal = wrong_seal
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| {
                let bc::BytecodeInstructionKind::Store { value, .. } = &mut instruction.kind else {
                    return None;
                };
                matches!(
                    &value.kind,
                    bc::BytecodeRvalueKind::Coerce {
                        kind: bc::BytecodeCoercion::Opaque,
                        ..
                    }
                )
                .then_some(value)
            })
            .expect("opaque return lowers to a bytecode seal");
        let bc::BytecodeRvalueKind::Coerce { kind, .. } = &mut seal.kind else {
            unreachable!()
        };
        *kind = bc::BytecodeCoercion::OptionLift;
        let error = bc::verify_bytecode(&wrong_seal).unwrap_err();
        assert!(error.message().contains("rvalue") || error.message().contains("coercion"));
    }

    #[test]
    fn fallible_opaque_results_preserve_both_channels_through_an_outer_opaque() {
        let source = "fn choose(flag: Bool): impl Discard ! String {\n\
                          if flag { ok(42) } else { err(\"bad\") }\n\
                      }\n\
                      fn success(): impl Discard ! String { choose(true) }\n\
                      fn failure(): impl Discard ! String { choose(false) }\n";
        assert_eq!(
            execute_function(source, "success"),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(42)))
        );
        assert_eq!(
            execute_function(source, "failure"),
            RuntimeValue::ResultErr(Box::new(RuntimeValue::String("bad".into())))
        );
    }

    #[test]
    fn generic_opaque_families_monomorphize_distinct_concrete_representations() {
        let source = "fn hide[T: Discard](value: T): impl Discard { value }\n\
                      fn number(): impl Discard { hide(42) }\n\
                      fn text(): impl Discard { hide(\"ready\") }\n";
        assert_eq!(
            execute_function(source, "number"),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(source, "text"),
            RuntimeValue::String("ready".into())
        );

        let program = lowered(source);
        let hidden = program
            .types
            .iter()
            .filter_map(|ty| {
                let bc::BytecodeTypeKind::OpaqueResult {
                    identity,
                    arguments,
                    witness,
                } = &ty.kind
                else {
                    return None;
                };
                identity
                    .ends_with("::value::hide")
                    .then_some((arguments, *witness))
            })
            .collect::<Vec<_>>();
        assert_eq!(hidden.len(), 2);
        assert!(hidden.iter().all(|(arguments, witness)| {
            arguments.as_slice() == [*witness]
                && !matches!(
                    program.types[witness.index() as usize].kind,
                    bc::BytecodeTypeKind::GenericParameter(_)
                )
        }));
    }

    #[test]
    fn opaque_published_traits_dispatch_statically_through_generic_consumers() {
        let source = "trait Value {\n\
                          fn value(value: Self): Int\n\
                      }\n\
                      type Boxed = { number: Int }\n\
                      impl Value for Boxed {\n\
                          fn value(value: Self): Int { value.number }\n\
                      }\n\
                      fn hidden(): impl Value + Discard { Boxed { number: 42 } }\n\
                      fn generic[T: Value](value: T): Int { Value.value[T](value) }\n\
                      fn forwarded(): Int { generic(hidden()) }\n";
        assert_eq!(
            execute_function(source, "forwarded"),
            RuntimeValue::Integer(42)
        );
        let program = lowered(source);
        assert!(
            program
                .callables
                .iter()
                .all(|callable| callable.generic_arity == 0)
        );
        assert!(!bc::disassemble(&program).contains("vtable"));
    }

    #[test]
    fn opaque_prelude_bounds_dispatch_to_concrete_display_and_iterator_impls() {
        let source = "type Label = { text: String }\n\
                      type Cursor = { done: Bool }\n\
                      impl Display for Label {\n\
                          fn display(self): String { self.text }\n\
                      }\n\
                      impl Iterator[Int] for Cursor {\n\
                          fn next(mut self): Int? { none }\n\
                      }\n\
                      fn hiddenLabel(): impl Display + Discard {\n\
                          Label { text: \"ready\" }\n\
                      }\n\
                      fn hiddenCursor(): impl Iterator[Int] + Discard {\n\
                          Cursor { done: false }\n\
                      }\n\
                      fn render[T: Display](value: T): String { value.display() }\n\
                      fn consume[I: Discard + Iterator[Int]](cursor: I) {\n\
                          for value in cursor {\n\
                              _ = value\n\
                          }\n\
                      }\n\
                      fn use() {\n\
                          _ = render(hiddenLabel())\n\
                          consume(hiddenCursor())\n\
                      }\n";
        let program = lowered(source);
        let implementations = program
            .callables
            .iter()
            .filter(|callable| callable.name.starts_with("implementation#"))
            .count();
        assert_eq!(implementations, 2);
        assert!(program.callables.iter().all(|callable| {
            !callable.name.contains("::type::Display::display")
                && !callable.name.contains("::type::Iterator::next")
        }));
        assert!(!bc::disassemble(&program).contains("vtable"));
    }
}
