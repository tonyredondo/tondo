use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tondo_vm::bytecode as bc;

use super::{BytecodeError, BytecodeLoweringLimits};
use crate::hir::{
    CapabilityAnalysis, CapabilityAssumptions, HirCallProtocol, HirCallableId, HirCapability,
    HirClosureId, HirConstantValue, HirConstantValueKind, HirConstantVariantValue, HirNominalShape,
    HirPreludeTraitMethod, HirProgram, HirTraitConstructor, HirTraitMethodKey,
    HirTypeDeclarationKind, HirVariantPayload, TraitQuery, TraitSelectionError,
    analyze_closure_captures, select_implementation,
};
use crate::mir::{
    MirAggregateKind, MirAwaitable, MirBasicBlock, MirBlockKind, MirCallArgument, MirConstant,
    MirFunction, MirLoanKind, MirLocalKind, MirOperand, MirOperandKind, MirOperation,
    MirOperationKind, MirPlace, MirProgram, MirProjection, MirProjectionKind, MirRvalue,
    MirRvalueKind, MirStatement, MirStatementKind, MirTag, MirTerminator, MirTerminatorKind,
    verify_mir,
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
            callables: &callables,
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

    let mut program = bc::BytecodeProgram {
        types: catalog.types,
        nominals,
        callables,
        constants,
        functions,
    };
    specialize_closure_call_once(
        resolved,
        hir,
        &monomorphization,
        &callable_ids,
        &mut program,
    )?;
    specialize_terminal_fallbacks(&mut program)?;
    specialize_defer_guards(&mut program)?;
    specialize_iterator_exhaustion_guards(&mut program)?;
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

fn specialize_terminal_fallbacks(program: &mut bc::BytecodeProgram) -> Result<(), BytecodeError> {
    let sites = program
        .functions
        .iter()
        .enumerate()
        .flat_map(|(function, body)| {
            body.blocks
                .iter()
                .enumerate()
                .flat_map(move |(block, basic_block)| {
                    basic_block.instructions.iter().enumerate().filter_map(
                        move |(instruction, operation)| {
                            let bc::BytecodeInstructionKind::RegisterFallback { owner, .. } =
                                &operation.kind
                            else {
                                return None;
                            };
                            Some((function, block, instruction, owner.ty))
                        },
                    )
                })
        })
        .collect::<Vec<_>>();
    let roots = sites.iter().map(|(_, _, _, ty)| *ty).collect::<Vec<_>>();
    let statuses =
        bc::derive_terminal_statuses(program, &roots).map_err(BytecodeError::Invariant)?;
    let mut remove = BTreeMap::<(usize, usize), BTreeSet<usize>>::new();
    for ((function, block, instruction, _), status) in sites.into_iter().zip(statuses) {
        match status {
            bc::BytecodeTerminalStatus::Present => {}
            bc::BytecodeTerminalStatus::Absent => {
                remove
                    .entry((function, block))
                    .or_default()
                    .insert(instruction);
            }
            bc::BytecodeTerminalStatus::Potential => {
                return Err(BytecodeError::construction(
                    "terminal fallback specialization",
                    "monomorphization left terminal ownership unresolved",
                ));
            }
        }
    }
    for ((function, block), instructions) in remove {
        let mut index = 0usize;
        program.functions[function].blocks[block]
            .instructions
            .retain(|_| {
                let keep = !instructions.contains(&index);
                index += 1;
                keep
            });
    }
    Ok(())
}

fn specialize_defer_guards(program: &mut bc::BytecodeProgram) -> Result<(), BytecodeError> {
    let guard_types = program
        .functions
        .iter()
        .flat_map(|body| {
            body.blocks.iter().flat_map(|block| {
                block
                    .instructions
                    .iter()
                    .filter_map(|instruction| match &instruction.kind {
                        bc::BytecodeInstructionKind::RegisterDefer {
                            guard: Some(guard), ..
                        } => Some(guard.ty),
                        _ => None,
                    })
            })
        })
        .collect::<Vec<_>>();
    let copy =
        bc::derive_copy_capabilities(program, &guard_types).map_err(BytecodeError::Invariant)?;
    let closed_copy_types = guard_types
        .into_iter()
        .zip(copy)
        .filter_map(|(ty, copy)| copy.then_some(ty))
        .collect::<BTreeSet<_>>();
    if closed_copy_types.is_empty() {
        return Ok(());
    }

    for body in &mut program.functions {
        let copy_guard_types = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    guard: Some(guard), ..
                } if closed_copy_types.contains(&guard.ty) => Some(guard.ty),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if copy_guard_types.is_empty() {
            continue;
        }
        for block in &mut body.blocks {
            for instruction in &mut block.instructions {
                let bc::BytecodeInstructionKind::RegisterDefer {
                    action,
                    guard: Some(guard),
                    ..
                } = &mut instruction.kind
                else {
                    continue;
                };
                if !copy_guard_types.contains(&guard.ty) {
                    continue;
                }
                let guard = guard.clone();
                specialize_defer_snapshot(action, &guard)?;
                if let bc::BytecodeInstructionKind::RegisterDefer { guard, .. } =
                    &mut instruction.kind
                {
                    *guard = None;
                }
            }
            block
                .instructions
                .retain(|instruction| match &instruction.kind {
                    bc::BytecodeInstructionKind::RetargetCleanup { from, .. } => {
                        !copy_guard_types.contains(&from.ty)
                    }
                    bc::BytecodeInstructionKind::DisarmCleanup(place) => {
                        !copy_guard_types.contains(&place.ty)
                    }
                    _ => true,
                });
        }
    }
    Ok(())
}

fn specialize_defer_snapshot(
    action: &mut bc::BytecodeOperation,
    guard: &bc::BytecodePlace,
) -> Result<(), BytecodeError> {
    fn rewrite(operand: &mut bc::BytecodeOperand, guard: &bc::BytecodePlace) -> bool {
        let bc::BytecodeOperandKind::Move(place) = &operand.kind else {
            return false;
        };
        if place != guard {
            return false;
        }
        operand.kind = bc::BytecodeOperandKind::Copy(place.clone());
        true
    }

    let mut rewritten = 0usize;
    match &mut action.kind {
        bc::BytecodeOperationKind::Call {
            callee, arguments, ..
        } => {
            rewritten += usize::from(rewrite(callee, guard));
            for argument in arguments {
                rewritten += usize::from(rewrite(&mut argument.value, guard));
            }
        }
        bc::BytecodeOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            rewritten += usize::from(rewrite(condition, guard));
            for part in message_parts {
                rewritten += usize::from(rewrite(&mut part.value, guard));
            }
        }
        bc::BytecodeOperationKind::BootstrapHostCall { arguments, .. } => {
            for operand in arguments {
                rewritten += usize::from(rewrite(operand, guard));
            }
        }
        _ => {}
    }
    if rewritten != 1 {
        return Err(BytecodeError::construction(
            "defer specialization",
            "concrete Copy guard does not match exactly one moved invocation operand",
        ));
    }
    Ok(())
}

fn specialize_iterator_exhaustion_guards(
    program: &mut bc::BytecodeProgram,
) -> Result<(), BytecodeError> {
    let sites = program
        .functions
        .iter()
        .enumerate()
        .flat_map(|(function, body)| {
            body.blocks
                .iter()
                .enumerate()
                .filter_map(move |(block, basic_block)| {
                    let bc::BytecodeTerminatorKind::IteratorNext {
                        exhaustion_guard: Some(guard),
                        ..
                    } = &basic_block.terminator.kind
                    else {
                        return None;
                    };
                    Some((function, block, guard.ty))
                })
        })
        .collect::<Vec<_>>();
    let roots = sites.iter().map(|(_, _, ty)| *ty).collect::<Vec<_>>();
    let statuses =
        bc::derive_terminal_statuses(program, &roots).map_err(BytecodeError::Invariant)?;
    for ((function, block, _), status) in sites.into_iter().zip(statuses) {
        let bc::BytecodeTerminatorKind::IteratorNext {
            exhaustion_guard, ..
        } = &mut program.functions[function].blocks[block].terminator.kind
        else {
            unreachable!("recorded iterator site remains an iterator terminator")
        };
        match status {
            bc::BytecodeTerminalStatus::Present => {}
            bc::BytecodeTerminalStatus::Absent => *exhaustion_guard = None,
            bc::BytecodeTerminalStatus::Potential => {
                return Err(BytecodeError::construction(
                    "iterator exhaustion",
                    "monomorphization left terminal ownership unresolved",
                ));
            }
        }
    }
    Ok(())
}

fn specialize_closure_call_once(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    monomorphization: &Monomorphization,
    callable_ids: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    program: &mut bc::BytecodeProgram,
) -> Result<(), BytecodeError> {
    let capture_types = program
        .callables
        .iter()
        .flat_map(|callable| {
            callable
                .closure
                .iter()
                .flat_map(|closure| closure.captures.iter().copied())
        })
        .collect::<Vec<_>>();
    let discard = bc::derive_discard_capabilities(program, &capture_types)
        .map_err(BytecodeError::Invariant)?;
    let mut discard_by_callable = vec![None; program.callables.len()];
    let mut next = 0usize;
    for (index, callable) in program.callables.iter().enumerate() {
        let Some(closure) = &callable.closure else {
            continue;
        };
        let end = next.checked_add(closure.captures.len()).ok_or_else(|| {
            BytecodeError::construction("closure protocols", "capture count overflow")
        })?;
        discard_by_callable[index] = Some(
            discard
                .get(next..end)
                .ok_or_else(|| {
                    BytecodeError::construction(
                        "closure protocols",
                        "concrete capture capability table is incomplete",
                    )
                })?
                .to_vec(),
        );
        next = end;
    }

    let capabilities = CapabilityAnalysis::new(hir, resolved)
        .map_err(|error| monomorphization_type_error(error, None, "concrete closure protocols"))?;
    let mut transferred_on_all_exits = BTreeMap::new();
    for instance in &monomorphization.callables {
        let ExecutableInstance::Closure(instance) = instance else {
            continue;
        };
        if transferred_on_all_exits.contains_key(&instance.closure) {
            continue;
        }
        let closure = hir.closure(instance.closure).ok_or_else(|| {
            BytecodeError::construction("closure protocols", "missing HIR closure metadata")
        })?;
        let analysis = analyze_closure_captures(
            hir,
            &capabilities,
            CapabilityAssumptions::from_generics(hir, closure.generics()),
            closure.captures(),
            closure.body().root(),
        )
        .map_err(|error| {
            monomorphization_type_error(error, Some(closure.span()), "concrete closure protocols")
        })?;
        transferred_on_all_exits.insert(
            instance.closure,
            analysis.transferred_on_all_exits().clone(),
        );
    }

    let mut rows = Vec::new();
    for instance in &monomorphization.callables {
        let ExecutableInstance::Closure(closure_instance) = instance else {
            continue;
        };
        let closure = hir.closure(closure_instance.closure).ok_or_else(|| {
            BytecodeError::construction("closure protocols", "missing HIR closure metadata")
        })?;
        let callable = callable_ids.get(instance).copied().ok_or_else(|| {
            BytecodeError::construction("closure protocols", "missing bytecode callable ID")
        })?;
        let concrete_discard = discard_by_callable[callable.index() as usize]
            .as_ref()
            .ok_or_else(|| {
                BytecodeError::construction(
                    "closure protocols",
                    "closure callable has no concrete capture capabilities",
                )
            })?;
        if concrete_discard.len() != closure.captures().len() {
            return Err(BytecodeError::construction(
                "closure protocols",
                "concrete capture capability row has the wrong length",
            ));
        }
        let transferred = transferred_on_all_exits
            .get(&closure_instance.closure)
            .ok_or_else(|| {
                BytecodeError::construction(
                    "closure protocols",
                    "closure has no all-exit transfer analysis",
                )
            })?;
        let call_once = closure
            .captures()
            .iter()
            .zip(concrete_discard)
            .all(|(capture, discard)| *discard || transferred.contains(&capture.local()));
        rows.push((callable, call_once));
    }
    for (callable, call_once) in rows {
        program.callables[callable.index() as usize]
            .closure
            .as_mut()
            .expect("closure instances retain closure metadata")
            .protocols
            .call_once = call_once;
    }
    Ok(())
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
                FunctionReference::PreludeTrait { method, .. } => {
                    if !has_intrinsic_prelude_dispatch(hir, &mut interner, method, &arguments)? {
                        register_prelude_reference(
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
                        )?;
                    }
                }
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
            catalog.types.push(catalog.lower_type(interner, hir, ty)?);
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
        hir: &HirProgram,
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
                capabilities: bc::BytecodeCapabilitySet {
                    copy: hir.opaque_exposes_capability(identity, HirCapability::Copy),
                    discard: hir.opaque_exposes_capability(identity, HirCapability::Discard),
                    equatable: hir.opaque_exposes_capability(identity, HirCapability::Equatable),
                    key: hir.opaque_exposes_capability(identity, HirCapability::Key),
                    send: hir.opaque_exposes_capability(identity, HirCapability::Send),
                    share: hir.opaque_exposes_capability(identity, HirCapability::Share),
                },
            },
            TypeKind::Generated { arguments, .. } => bc::BytecodeTypeKind::Generated {
                identity: name.clone(),
                arguments: self.map_types(arguments)?,
            },
            TypeKind::Cursor { mode, collection } => bc::BytecodeTypeKind::Cursor {
                mode: match mode {
                    CursorMode::Own => bc::BytecodeCursorMode::Own,
                    CursorMode::Ref => bc::BytecodeCursorMode::Ref,
                    CursorMode::Mut => bc::BytecodeCursorMode::Mut,
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
        | HirConstantValueKind::NumericConversionError(_)
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
        | HirConstantValueKind::NumericConversionError(_)
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
    for loan in function.loans() {
        collect_place_types(loan.place(), types);
    }
    for block in function.blocks() {
        for statement in block.statements() {
            match statement.kind() {
                MirStatementKind::StorageLive(_)
                | MirStatementKind::StorageDead(_)
                | MirStatementKind::EnterTaskScope { .. }
                | MirStatementKind::ReserveLoan(_)
                | MirStatementKind::ReleaseLoan(_) => {}
                MirStatementKind::Assign { destination, value } => {
                    collect_place_types(destination, types);
                    collect_rvalue_types(value, types);
                }
                MirStatementKind::RegisterDefer { action, guard, .. } => {
                    collect_operation_types(action, types);
                    if let Some(guard) = guard {
                        collect_place_types(guard, types);
                    }
                }
                MirStatementKind::RegisterFallback { owner, .. } => {
                    collect_place_types(owner, types);
                }
                MirStatementKind::RetargetCleanup { from, to } => {
                    collect_place_types(from, types);
                    collect_place_types(to, types);
                }
                MirStatementKind::DisarmCleanup(place) => collect_place_types(place, types),
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
        MirOperandKind::Constant(_) | MirOperandKind::Loan(_) => {}
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
        MirRvalueKind::Interpolate { values, .. } => {
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
        MirRvalueKind::MapRemove { map, key } => {
            collect_place_types(map, types);
            collect_operand_types(key, types);
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
        MirOperationKind::ArraySequence {
            array, argument, ..
        } => {
            collect_operand_types(array, types);
            collect_operand_types(argument, types);
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
        MirOperationKind::Slice { base, bounds, .. } => {
            collect_operand_types(base, types);
            for value in bounds
                .start()
                .into_iter()
                .chain(bounds.end())
                .chain(bounds.step())
            {
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
        | MirTerminatorKind::ValidateLoan { .. }
        | MirTerminatorKind::DrainDefers { .. }
        | MirTerminatorKind::DrainScopes { .. }
        | MirTerminatorKind::DrainUnwind { .. }
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
        MirTerminatorKind::Await {
            awaitable,
            destination,
            ..
        } => {
            match awaitable {
                MirAwaitable::Call(operation) => collect_operation_types(operation, types),
                MirAwaitable::Join(join) => collect_operand_types(join, types),
            }
            collect_place_types(destination, types);
        }
        MirTerminatorKind::Spawn {
            operation,
            destination,
            ..
        } => {
            collect_operation_types(operation, types);
            collect_place_types(destination, types);
        }
        MirTerminatorKind::IteratorNext {
            state,
            destination,
            borrowed_source,
            exhaustion_guard,
            ..
        } => {
            collect_place_types(state, types);
            collect_place_types(destination, types);
            if let Some(source) = borrowed_source {
                collect_place_types(source, types);
            }
            if let Some(guard) = exhaustion_guard {
                collect_place_types(guard, types);
            }
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
            match statement.kind() {
                MirStatementKind::Assign { value, .. } => {
                    collect_rvalue_function_references(value, references);
                }
                MirStatementKind::RegisterDefer { action, .. } => {
                    collect_operation_function_references(action, references);
                }
                MirStatementKind::StorageLive(_)
                | MirStatementKind::StorageDead(_)
                | MirStatementKind::EnterTaskScope { .. }
                | MirStatementKind::ReserveLoan(_)
                | MirStatementKind::ReleaseLoan(_)
                | MirStatementKind::RegisterFallback { .. }
                | MirStatementKind::RetargetCleanup { .. }
                | MirStatementKind::DisarmCleanup(_) => {}
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
        | MirOperandKind::Borrow(_)
        | MirOperandKind::Loan(_) => {}
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
        MirRvalueKind::Interpolate { values, .. } => {
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
        MirRvalueKind::MapRemove { key, .. } => {
            collect_operand_function_references(key, references);
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
        MirOperationKind::ArraySequence {
            array, argument, ..
        } => {
            collect_operand_function_references(array, references);
            collect_operand_function_references(argument, references);
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
        MirOperationKind::Slice { base, bounds, .. } => {
            collect_operand_function_references(base, references);
            for value in bounds
                .start()
                .into_iter()
                .chain(bounds.end())
                .chain(bounds.step())
            {
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
        | MirTerminatorKind::IteratorNext { .. }
        | MirTerminatorKind::ValidateLoan { .. }
        | MirTerminatorKind::DrainDefers { .. }
        | MirTerminatorKind::DrainScopes { .. }
        | MirTerminatorKind::DrainUnwind { .. } => {}
        MirTerminatorKind::SwitchBool { condition, .. } => {
            collect_operand_function_references(condition, references);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            collect_operand_function_references(value, references);
        }
        MirTerminatorKind::Invoke { operation, .. }
        | MirTerminatorKind::Spawn { operation, .. } => {
            collect_operation_function_references(operation, references);
        }
        MirTerminatorKind::Await { awaitable, .. } => match awaitable {
            MirAwaitable::Call(operation) => {
                collect_operation_function_references(operation, references);
            }
            MirAwaitable::Join(join) => {
                collect_operand_function_references(join, references);
            }
        },
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
        HirCallableId::Host(function) => function.name().to_owned(),
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
        HirConstantValueKind::NumericConversionError(variant) => {
            bc::BytecodeConstantValueKind::Variant {
                variant: variant.index(),
                payload: bc::BytecodeConstantVariantValue::Unit,
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
    callables: &'a [bc::BytecodeCallable],
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
    for loan in function.loans() {
        collect_place_types(loan.place(), &mut function_types);
    }
    for block in function.blocks() {
        for statement in block.statements() {
            match statement.kind() {
                MirStatementKind::StorageLive(_)
                | MirStatementKind::StorageDead(_)
                | MirStatementKind::EnterTaskScope { .. }
                | MirStatementKind::ReserveLoan(_)
                | MirStatementKind::ReleaseLoan(_) => {}
                MirStatementKind::Assign { destination, value } => {
                    collect_place_types(destination, &mut function_types);
                    collect_rvalue_types(value, &mut function_types);
                }
                MirStatementKind::RegisterDefer { action, guard, .. } => {
                    collect_operation_types(action, &mut function_types);
                    if let Some(guard) = guard {
                        collect_place_types(guard, &mut function_types);
                    }
                }
                MirStatementKind::RegisterFallback { owner, .. } => {
                    collect_place_types(owner, &mut function_types);
                }
                MirStatementKind::RetargetCleanup { from, to } => {
                    collect_place_types(from, &mut function_types);
                    collect_place_types(to, &mut function_types);
                }
                MirStatementKind::DisarmCleanup(place) => {
                    collect_place_types(place, &mut function_types);
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
    let loans = function
        .loans()
        .map(|loan| {
            Ok(bc::BytecodeLoan {
                kind: match loan.kind() {
                    MirLoanKind::CallLocal => bc::BytecodeLoanKind::CallLocal,
                    MirLoanKind::Region => bc::BytecodeLoanKind::Region,
                },
                mode: parameter_mode(loan.mode()),
                place: lower_place(loan.place(), context, type_map)?,
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
        loans,
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
        MirStatementKind::ReserveLoan(loan) => {
            bc::BytecodeInstructionKind::ReserveLoan(bc::BytecodeLoanId::new(loan.index()))
        }
        MirStatementKind::ReleaseLoan(loan) => {
            bc::BytecodeInstructionKind::ReleaseLoan(bc::BytecodeLoanId::new(loan.index()))
        }
        MirStatementKind::Assign { destination, value } => bc::BytecodeInstructionKind::Store {
            destination: lower_place(destination, context, type_map)?,
            value: lower_rvalue(value, context, type_map)?,
        },
        MirStatementKind::EnterTaskScope { scope } => bc::BytecodeInstructionKind::EnterTaskScope {
            scope: bc::BytecodeScopeId::new(scope.index()),
        },
        MirStatementKind::RegisterDefer {
            scope,
            action,
            guard,
        } => bc::BytecodeInstructionKind::RegisterDefer {
            scope: bc::BytecodeScopeId::new(scope.index()),
            action: lower_operation(action, true, context, type_map)?,
            guard: guard
                .as_ref()
                .map(|place| lower_place(place, context, type_map))
                .transpose()?,
        },
        MirStatementKind::RegisterFallback { scope, owner } => {
            bc::BytecodeInstructionKind::RegisterFallback {
                scope: bc::BytecodeScopeId::new(scope.index()),
                owner: lower_place(owner, context, type_map)?,
            }
        }
        MirStatementKind::RetargetCleanup { from, to } => {
            bc::BytecodeInstructionKind::RetargetCleanup {
                from: lower_place(from, context, type_map)?,
                to: lower_place(to, context, type_map)?,
            }
        }
        MirStatementKind::DisarmCleanup(place) => {
            bc::BytecodeInstructionKind::DisarmCleanup(lower_place(place, context, type_map)?)
        }
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
            operation: lower_operation(operation, false, context, type_map)?,
            destination: destination
                .as_ref()
                .map(|place| lower_place(place, context, type_map))
                .transpose()?,
            target: target.map(block_id),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::Await {
            awaitable,
            destination,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::Await {
            awaitable: match awaitable {
                MirAwaitable::Call(operation) => bc::BytecodeAwaitable::Call(lower_operation(
                    operation, false, context, type_map,
                )?),
                MirAwaitable::Join(join) => {
                    bc::BytecodeAwaitable::Join(lower_operand(join, context, type_map)?)
                }
            },
            destination: lower_place(destination, context, type_map)?,
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::Spawn {
            operation,
            scope,
            destination,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::Spawn {
            operation: lower_operation(operation, true, context, type_map)?,
            scope: bc::BytecodeScopeId::new(scope.index()),
            destination: lower_place(destination, context, type_map)?,
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::IteratorNext {
            state,
            destination,
            borrowed_source,
            exhaustion_guard,
            has_value,
            exhausted,
            unwind,
        } => bc::BytecodeTerminatorKind::IteratorNext {
            state: lower_place(state, context, type_map)?,
            destination: lower_place(destination, context, type_map)?,
            borrowed_source: borrowed_source
                .as_ref()
                .map(|place| lower_place(place, context, type_map))
                .transpose()?,
            exhaustion_guard: exhaustion_guard
                .as_ref()
                .map(|place| lower_place(place, context, type_map))
                .transpose()?,
            has_value: block_id(*has_value),
            exhausted: block_id(*exhausted),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            against,
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
            against: against
                .iter()
                .map(|loans| {
                    loans
                        .iter()
                        .map(|loan| bc::BytecodeLoanId::new(loan.index()))
                        .collect()
                })
                .collect(),
            for_write: *for_write,
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::ValidateLoan {
            loan,
            against,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::ValidateLoan {
            loan: bc::BytecodeLoanId::new(loan.index()),
            against: against
                .iter()
                .map(|loan| bc::BytecodeLoanId::new(loan.index()))
                .collect(),
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::DrainDefers {
            scopes,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::DrainDefers {
            scopes: scopes
                .iter()
                .map(|scope| bc::BytecodeScopeId::new(scope.index()))
                .collect(),
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::DrainScopes {
            task_scopes,
            defer_scopes,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::DrainScopes {
            task_scopes: task_scopes
                .iter()
                .map(|scope| bc::BytecodeScopeId::new(scope.index()))
                .collect(),
            defer_scopes: defer_scopes
                .iter()
                .map(|scope| bc::BytecodeScopeId::new(scope.index()))
                .collect(),
            target: block_id(*target),
            unwind: block_id(*unwind),
        },
        MirTerminatorKind::DrainUnwind { target } => bc::BytecodeTerminatorKind::DrainUnwind {
            target: block_id(*target),
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
        source_loan: place
            .source_loan()
            .map(|loan| bc::BytecodeLoanId::new(loan.index())),
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
        MirProjectionKind::RefValue => bc::BytecodeProjectionKind::RefValue,
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
        MirProjectionKind::IteratorElement { index } => {
            bc::BytecodeProjectionKind::IteratorElement {
                index: bc::BytecodeSlotId::new(index.index()),
            }
        }
        MirProjectionKind::IteratorSource => bc::BytecodeProjectionKind::IteratorSource,
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
        MirOperandKind::Loan(loan) => {
            bc::BytecodeOperandKind::Loan(bc::BytecodeLoanId::new(loan.index()))
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
        MirRvalueKind::MapRemove { map, key } => bc::BytecodeRvalueKind::MapRemove {
            map: lower_place(map, context, type_map)?,
            key: operand(key)?,
        },
        MirRvalueKind::Interpolate { segments, values } => bc::BytecodeRvalueKind::Interpolate {
            segments: segments.clone(),
            values: values
                .iter()
                .map(operand)
                .collect::<Result<_, BytecodeError>>()?,
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
        MirAggregateKind::Ref => bc::BytecodeAggregateKind::Ref,
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
        MirAggregateKind::NumericConversionError(variant) => bc::BytecodeAggregateKind::Variant {
            variant: variant.index(),
            fields: Vec::new(),
        },
        MirAggregateKind::OptionNone => bc::BytecodeAggregateKind::OptionNone,
        MirAggregateKind::OptionSome => bc::BytecodeAggregateKind::OptionSome,
        MirAggregateKind::ResultOk => bc::BytecodeAggregateKind::ResultOk,
        MirAggregateKind::ResultErr => bc::BytecodeAggregateKind::ResultErr,
    })
}

fn lower_operation(
    operation: &MirOperation,
    deferred: bool,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeOperation, BytecodeError> {
    if let Some(kind) = lower_intrinsic_display_call(operation, deferred, context, type_map)? {
        return Ok(bc::BytecodeOperation {
            ty: mapped_catalog_id(operation.ty(), type_map, context.catalog)?,
            kind,
        });
    }
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
        MirOperationKind::ArraySequence {
            kind,
            array,
            argument,
        } => bc::BytecodeOperationKind::ArraySequence {
            kind: match kind {
                crate::hir::HirArraySequenceKind::Concat => bc::BytecodeArraySequenceKind::Concat,
                crate::hir::HirArraySequenceKind::Repeat => bc::BytecodeArraySequenceKind::Repeat,
            },
            array: operand(array)?,
            argument: operand(argument)?,
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
            against,
        } => bc::BytecodeOperationKind::Index {
            base: operand(base)?,
            index: operand(index)?,
            access: index_access(*access),
            against: against
                .iter()
                .map(|loan| bc::BytecodeLoanId::new(loan.index()))
                .collect(),
        },
        MirOperationKind::Slice {
            base,
            bounds,
            against,
        } => bc::BytecodeOperationKind::Slice {
            base: operand(base)?,
            bounds: Box::new(bc::BytecodeSliceBounds {
                start: bounds.start().map(operand).transpose()?,
                end: bounds.end().map(operand).transpose()?,
                step: bounds.step().map(operand).transpose()?,
            }),
            against: against
                .iter()
                .map(|loan| bc::BytecodeLoanId::new(loan.index()))
                .collect(),
        },
        MirOperationKind::Call {
            callee,
            arguments,
            signature,
            protocol,
            unsafe_call,
        } => {
            let callee = operand(callee)?;
            let protocol = normalized_call_protocol(*protocol, &callee, deferred, context)?;
            bc::BytecodeOperationKind::Call {
                callee,
                arguments: arguments
                    .iter()
                    .map(|argument| lower_call_argument(argument, context, type_map))
                    .collect::<Result<_, BytecodeError>>()?,
                signature: mapped_catalog_id(*signature, type_map, context.catalog)?,
                protocol,
                unsafe_call: *unsafe_call,
            }
        }
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
                crate::mir::MirBootstrapHostFunction::ConsolePrintln => {
                    bc::BytecodeBootstrapHostFunction::ConsolePrintln
                }
                crate::mir::MirBootstrapHostFunction::ProcessPipe => {
                    bc::BytecodeBootstrapHostFunction::ProcessPipe
                }
                crate::mir::MirBootstrapHostFunction::ProcessOutputStdout => {
                    bc::BytecodeBootstrapHostFunction::ProcessOutputStdout
                }
                crate::mir::MirBootstrapHostFunction::ProcessOutputStderr => {
                    bc::BytecodeBootstrapHostFunction::ProcessOutputStderr
                }
                crate::mir::MirBootstrapHostFunction::ProcessOutputStatuses => {
                    bc::BytecodeBootstrapHostFunction::ProcessOutputStatuses
                }
                crate::mir::MirBootstrapHostFunction::ExitStatusCode => {
                    bc::BytecodeBootstrapHostFunction::ExitStatusCode
                }
                crate::mir::MirBootstrapHostFunction::ExitStatusSuccess => {
                    bc::BytecodeBootstrapHostFunction::ExitStatusSuccess
                }
                crate::mir::MirBootstrapHostFunction::PointerRead => {
                    bc::BytecodeBootstrapHostFunction::PointerRead
                }
                crate::mir::MirBootstrapHostFunction::PointerWrite => {
                    bc::BytecodeBootstrapHostFunction::PointerWrite
                }
                crate::mir::MirBootstrapHostFunction::PointerOffset => {
                    bc::BytecodeBootstrapHostFunction::PointerOffset
                }
                crate::mir::MirBootstrapHostFunction::PointerCast => {
                    bc::BytecodeBootstrapHostFunction::PointerCast
                }
                crate::mir::MirBootstrapHostFunction::PointerAddress => {
                    bc::BytecodeBootstrapHostFunction::PointerAddress
                }
                crate::mir::MirBootstrapHostFunction::PointerFromAddress => {
                    bc::BytecodeBootstrapHostFunction::PointerFromAddress
                }
                crate::mir::MirBootstrapHostFunction::TestingLog => {
                    bc::BytecodeBootstrapHostFunction::TestingLog
                }
                crate::mir::MirBootstrapHostFunction::TestingTags => {
                    bc::BytecodeBootstrapHostFunction::TestingTags
                }
                crate::mir::MirBootstrapHostFunction::TestingFailNow => {
                    bc::BytecodeBootstrapHostFunction::TestingFailNow
                }
                crate::mir::MirBootstrapHostFunction::TestingSkip => {
                    bc::BytecodeBootstrapHostFunction::TestingSkip
                }
                crate::mir::MirBootstrapHostFunction::TestingAttach => {
                    bc::BytecodeBootstrapHostFunction::TestingAttach
                }
                crate::mir::MirBootstrapHostFunction::TestingSnapshot => {
                    bc::BytecodeBootstrapHostFunction::TestingSnapshot
                }
                crate::mir::MirBootstrapHostFunction::TestingRunLeaf => {
                    bc::BytecodeBootstrapHostFunction::TestingRunLeaf
                }
                crate::mir::MirBootstrapHostFunction::TestingRunSuite => {
                    bc::BytecodeBootstrapHostFunction::TestingRunSuite
                }
                crate::mir::MirBootstrapHostFunction::TestingBeginSuiteCleanup => {
                    bc::BytecodeBootstrapHostFunction::TestingBeginSuiteCleanup
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

fn lower_intrinsic_display_call(
    operation: &MirOperation,
    deferred: bool,
    context: &FunctionLoweringContext<'_>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<Option<bc::BytecodeOperationKind>, BytecodeError> {
    let MirOperationKind::Call {
        callee, arguments, ..
    } = operation.kind()
    else {
        return Ok(None);
    };
    let MirOperandKind::PreludeTraitFunction {
        method: crate::hir::HirPreludeTraitMethod::Display,
        arguments: display_arguments,
    } = callee.kind()
    else {
        return Ok(None);
    };
    let concrete = display_arguments
        .iter()
        .map(|argument| mapped_type(*argument, type_map))
        .collect::<Result<Vec<_>, _>>()?;
    let mut interner = context.hir.interner().clone();
    if !has_intrinsic_prelude_dispatch(
        context.hir,
        &mut interner,
        crate::hir::HirPreludeTraitMethod::Display,
        &concrete,
    )? {
        return Ok(None);
    }
    if deferred {
        return Err(BytecodeError::construction(
            "intrinsic Display lowering",
            "a String-returning Display call cannot be a deferred Unit action",
        ));
    }
    let [argument] = arguments.as_slice() else {
        return Err(BytecodeError::construction(
            "intrinsic Display lowering",
            "Display call does not contain exactly one receiver argument",
        ));
    };
    Ok(Some(bc::BytecodeOperationKind::Display {
        argument: lower_call_argument(argument, context, type_map)?,
    }))
}

fn has_intrinsic_prelude_dispatch(
    hir: &HirProgram,
    interner: &mut TypeInterner,
    method: crate::hir::HirPreludeTraitMethod,
    arguments: &[TypeId],
) -> Result<bool, BytecodeError> {
    let mut concrete = arguments.to_vec();
    if matches!(method, crate::hir::HirPreludeTraitMethod::Display)
        && let [target] = concrete.as_mut_slice()
    {
        *target = concrete_trait_target(hir, interner, *target)?;
    }
    method
        .has_intrinsic_implementation(interner, &concrete)
        .map_err(|error| {
            BytecodeError::construction("intrinsic prelude dispatch", error.to_string())
        })
}

fn normalized_call_protocol(
    source: HirCallProtocol,
    callee: &bc::BytecodeOperand,
    deferred: bool,
    context: &FunctionLoweringContext<'_>,
) -> Result<bc::BytecodeCallProtocol, BytecodeError> {
    let source = call_protocol(source);
    let concrete = match &context
        .catalog
        .types
        .get(callee.ty.index() as usize)
        .ok_or_else(|| {
            BytecodeError::construction(
                "call protocol",
                format!("callee references missing type#{}", callee.ty.index()),
            )
        })?
        .kind
    {
        bc::BytecodeTypeKind::Function(_) => bc::BytecodeCallProtocol::Call,
        bc::BytecodeTypeKind::Generated { .. } | bc::BytecodeTypeKind::OpaqueResult { .. } => {
            match concrete_callable_for_type(callee.ty, context)? {
                ConcreteCallable::Function => bc::BytecodeCallProtocol::Call,
                ConcreteCallable::Closure(closure) => {
                    let borrowed = matches!(callee.kind, bc::BytecodeOperandKind::Borrow(_));
                    if deferred
                        && source == bc::BytecodeCallProtocol::CallOnce
                        && closure.protocols.call_once
                        && !borrowed
                    {
                        bc::BytecodeCallProtocol::CallOnce
                    } else if closure.protocols.call {
                        bc::BytecodeCallProtocol::Call
                    } else if closure.protocols.call_mut && borrowed {
                        bc::BytecodeCallProtocol::CallMut
                    } else if closure.protocols.call_once && !borrowed {
                        bc::BytecodeCallProtocol::CallOnce
                    } else {
                        return Err(BytecodeError::construction(
                            "call protocol",
                            "specialized closure does not permit the lowered callee access",
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(BytecodeError::construction(
                "call protocol",
                "indirect callee has no executable callable representation",
            ));
        }
    };
    let valid_specialization = matches!(
        (source, concrete),
        (
            bc::BytecodeCallProtocol::Call,
            bc::BytecodeCallProtocol::Call
        ) | (
            bc::BytecodeCallProtocol::CallMut,
            bc::BytecodeCallProtocol::Call | bc::BytecodeCallProtocol::CallMut
        ) | (bc::BytecodeCallProtocol::CallOnce, _)
    );
    if !valid_specialization {
        return Err(BytecodeError::construction(
            "call protocol",
            "specialization weakens the source call protocol",
        ));
    }
    Ok(concrete)
}

enum ConcreteCallable<'a> {
    Function,
    Closure(&'a bc::BytecodeClosure),
}

fn concrete_callable_for_type<'a>(
    mut ty: bc::BytecodeTypeId,
    context: &'a FunctionLoweringContext<'_>,
) -> Result<ConcreteCallable<'a>, BytecodeError> {
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(ty) {
            return Err(BytecodeError::construction(
                "call protocol",
                "opaque callable representation forms a cycle",
            ));
        }
        let kind = &context
            .catalog
            .types
            .get(ty.index() as usize)
            .ok_or_else(|| {
                BytecodeError::construction(
                    "call protocol",
                    format!("callable references missing type#{}", ty.index()),
                )
            })?
            .kind;
        match kind {
            bc::BytecodeTypeKind::OpaqueResult { witness, .. } => ty = *witness,
            bc::BytecodeTypeKind::Generated { .. } => {
                let mut matches = context
                    .callables
                    .iter()
                    .filter_map(|callable| callable.closure.as_ref())
                    .filter(|closure| closure.environment == ty);
                let closure = matches.next();
                if matches.next().is_some() {
                    return Err(BytecodeError::construction(
                        "call protocol",
                        "generated environment maps to multiple closure callables",
                    ));
                }
                return closure.map(ConcreteCallable::Closure).ok_or_else(|| {
                    BytecodeError::construction(
                        "call protocol",
                        "generated callable has no closure metadata",
                    )
                });
            }
            bc::BytecodeTypeKind::Function(_) => return Ok(ConcreteCallable::Function),
            _ => {
                return Err(BytecodeError::construction(
                    "call protocol",
                    "callable representation is neither a function nor a closure",
                ));
            }
        }
    }
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
        MirTag::NumericConversionError(variant) => bc::BytecodeTag::Variant(variant.index()),
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
        IntrinsicType::Bytes => bc::BytecodeIntrinsicType::Bytes,
        IntrinsicType::BytesBuilder => bc::BytecodeIntrinsicType::BytesBuilder,
        IntrinsicType::BytesError => bc::BytecodeIntrinsicType::BytesError,
        IntrinsicType::TextError => bc::BytecodeIntrinsicType::TextError,
        IntrinsicType::Path => bc::BytecodeIntrinsicType::Path,
        IntrinsicType::PathError => bc::BytecodeIntrinsicType::PathError,
        IntrinsicType::FsError => bc::BytecodeIntrinsicType::FsError,
        IntrinsicType::MathError => bc::BytecodeIntrinsicType::MathError,
        IntrinsicType::FloatTolerance => bc::BytecodeIntrinsicType::FloatTolerance,
        IntrinsicType::FloatToleranceError => bc::BytecodeIntrinsicType::FloatToleranceError,
        IntrinsicType::TextDiff => bc::BytecodeIntrinsicType::TextDiff,
        IntrinsicType::TempDirectory => bc::BytecodeIntrinsicType::TempDirectory,
        IntrinsicType::TempError => bc::BytecodeIntrinsicType::TempError,
        IntrinsicType::Generator => bc::BytecodeIntrinsicType::Generator,
        IntrinsicType::GenerationId => bc::BytecodeIntrinsicType::GenerationId,
        IntrinsicType::GenerationError => bc::BytecodeIntrinsicType::GenerationError,
        IntrinsicType::Reader => bc::BytecodeIntrinsicType::Reader,
        IntrinsicType::Writer => bc::BytecodeIntrinsicType::Writer,
        IntrinsicType::IoError => bc::BytecodeIntrinsicType::IoError,
        IntrinsicType::ConsoleError => bc::BytecodeIntrinsicType::ConsoleError,
        IntrinsicType::ExitStatus => bc::BytecodeIntrinsicType::ExitStatus,
        IntrinsicType::ProcessOutput => bc::BytecodeIntrinsicType::ProcessOutput,
        IntrinsicType::ProcessHandle => bc::BytecodeIntrinsicType::ProcessHandle,
        IntrinsicType::ProcessError => bc::BytecodeIntrinsicType::ProcessError,
        IntrinsicType::ProcessExitError => bc::BytecodeIntrinsicType::ProcessExitError,
        IntrinsicType::Utf8Error => bc::BytecodeIntrinsicType::Utf8Error,
        IntrinsicType::NumericConversionError => bc::BytecodeIntrinsicType::NumericConversionError,
        IntrinsicType::Duration => bc::BytecodeIntrinsicType::Duration,
        IntrinsicType::Instant => bc::BytecodeIntrinsicType::Instant,
        IntrinsicType::Timer => bc::BytecodeIntrinsicType::Timer,
        IntrinsicType::DurationError => bc::BytecodeIntrinsicType::DurationError,
        IntrinsicType::ClockError => bc::BytecodeIntrinsicType::ClockError,
        IntrinsicType::EnvSnapshot => bc::BytecodeIntrinsicType::EnvSnapshot,
        IntrinsicType::EnvName => bc::BytecodeIntrinsicType::EnvName,
        IntrinsicType::EnvValue => bc::BytecodeIntrinsicType::EnvValue,
        IntrinsicType::EnvError => bc::BytecodeIntrinsicType::EnvError,
        IntrinsicType::VirtualTime => bc::BytecodeIntrinsicType::VirtualTime,
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
        Assignability::CallableOnceErasure => bc::BytecodeCoercion::CallableOnceErasure,
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
        crate::hir::HirIndexAccess::String => bc::BytecodeIndexAccess::String,
        crate::hir::HirIndexAccess::MapLookup => bc::BytecodeIndexAccess::MapLookup,
        crate::hir::HirIndexAccess::MapEntry => bc::BytecodeIndexAccess::MapEntry,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tondo_vm::runtime::{
        PanicCode, RejectingHost, RuntimeValue, ValueCopyStrategy, VmError, VmHost, VmLimits,
        VmOutcome, VmStatistics, execute, execute_with_limits,
        execute_with_limits_and_copy_strategy,
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

    #[test]
    fn trait_dispatch_error_adapters_preserve_the_closed_diagnostic() {
        let prelude = prelude_trait_dispatch_selection_error(TraitSelectionError::Ambiguous);
        let (_, hir) = checked("fn value(): Int { 1 }\n");
        let span = hir
            .expressions()
            .next()
            .expect("the checked expression supplies a diagnostic span")
            .span();
        let source = trait_dispatch_selection_error(TraitSelectionError::Ambiguous, span);
        assert!(prelude.to_string().contains("trait dispatch"));
        assert!(source.to_string().contains("trait dispatch"));
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

    #[test]
    fn defer_snapshot_rewrites_each_supported_operation_shape_exactly_once() {
        let ty = bc::BytecodeTypeId::new(0);
        let guard = bc::BytecodePlace {
            slot: bc::BytecodeSlotId::new(1),
            ty,
            projections: Vec::new(),
            source_loan: None,
        };
        let operand = |kind| bc::BytecodeOperand { ty, kind };

        let mut call = bc::BytecodeOperation {
            ty,
            kind: bc::BytecodeOperationKind::Call {
                callee: operand(bc::BytecodeOperandKind::Constant(
                    bc::BytecodeConstant::Unit,
                )),
                arguments: vec![bc::BytecodeCallArgument {
                    mode: bc::BytecodeParameterMode::Value,
                    target: bc::BytecodeCallArgumentTarget::Fixed(0),
                    value: operand(bc::BytecodeOperandKind::Move(guard.clone())),
                }],
                signature: ty,
                protocol: bc::BytecodeCallProtocol::Call,
                unsafe_call: false,
            },
        };
        specialize_defer_snapshot(&mut call, &guard).unwrap();
        let bc::BytecodeOperationKind::Call { arguments, .. } = call.kind else {
            unreachable!()
        };
        assert!(matches!(
            arguments[0].value.kind,
            bc::BytecodeOperandKind::Copy(_)
        ));

        let mut assertion = bc::BytecodeOperation {
            ty,
            kind: bc::BytecodeOperationKind::Assert {
                condition: operand(bc::BytecodeOperandKind::Constant(
                    bc::BytecodeConstant::Bool(true),
                )),
                condition_repr: "true".into(),
                message_parts: vec![bc::BytecodeAssertMessagePart {
                    value: operand(bc::BytecodeOperandKind::Move(guard.clone())),
                    spread: false,
                }],
            },
        };
        specialize_defer_snapshot(&mut assertion, &guard).unwrap();
        let bc::BytecodeOperationKind::Assert { message_parts, .. } = assertion.kind else {
            unreachable!()
        };
        assert!(matches!(
            message_parts[0].value.kind,
            bc::BytecodeOperandKind::Copy(_)
        ));

        let mut host_call = bc::BytecodeOperation {
            ty,
            kind: bc::BytecodeOperationKind::BootstrapHostCall {
                function: bc::BytecodeBootstrapHostFunction::ConsolePrint,
                arguments: vec![
                    operand(bc::BytecodeOperandKind::Move(guard.clone())),
                    operand(bc::BytecodeOperandKind::Constant(
                        bc::BytecodeConstant::Unit,
                    )),
                ],
            },
        };
        specialize_defer_snapshot(&mut host_call, &guard).unwrap();
        let bc::BytecodeOperationKind::BootstrapHostCall { arguments, .. } = host_call.kind else {
            unreachable!()
        };
        assert!(matches!(
            arguments[0].kind,
            bc::BytecodeOperandKind::Copy(_)
        ));

        let mut unsupported = bc::BytecodeOperation {
            ty,
            kind: bc::BytecodeOperationKind::CheckedPrefix {
                operator: bc::BytecodePrefixOperator::Negate,
                operand: operand(bc::BytecodeOperandKind::Constant(
                    bc::BytecodeConstant::Integer("1".into()),
                )),
            },
        };
        let error = specialize_defer_snapshot(&mut unsupported, &guard).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exactly one moved invocation operand")
        );
    }

    fn execute_function(source: &str, name: &str) -> RuntimeValue {
        match execute_outcome(source, name) {
            VmOutcome::Returned(value) => value,
            VmOutcome::Panicked(panic) => panic!("unexpected VM panic: {panic:#?}"),
        }
    }

    #[test]
    fn arrays_store_length_at_runtime_and_preserve_it_across_boundaries() {
        let source = r#"fn choose(expanded: Bool): Array[Int] {
    if expanded {
        [1, 2, 3, 4, 5]
    } else {
        []
    }
}

fn fourValues(): Array[Int] {
    [1, 2, 3, 4]
}

fn relay(values: Array[Int]): Array[Int] {
    values
}

fn runtimeLength(values: Array[Int]): Int {
    match values {
        [] => 0
        [_] => 1
        [_, _] => 2
        [_, _, _] => 3
        [_, _, _, _] => 4
        [_, _, _, _, _, ..] => 5
    }
}

fn observe(): (Int, Int, Int, Int, Int, Int, Int, Int, Int) {
    let returned = relay(choose(true))
    let copied = returned
    (
        runtimeLength([]),
        runtimeLength([1]),
        runtimeLength([1, 2]),
        runtimeLength([1, 2, 3]),
        runtimeLength(fourValues()),
        runtimeLength(choose(true)),
        runtimeLength(choose(false)),
        runtimeLength(returned),
        runtimeLength(copied),
    )
}
"#;
        let program = lowered(source);
        let array_types = program
            .types
            .iter()
            .enumerate()
            .filter_map(|(index, ty)| match &ty.kind {
                bc::BytecodeTypeKind::Intrinsic {
                    constructor: bc::BytecodeIntrinsicType::Array,
                    arguments,
                } if arguments.len() == 1
                    && matches!(
                        program.types[arguments[0].index() as usize].kind,
                        bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Int)
                    ) =>
                {
                    Some((bc::BytecodeTypeId::new(index as u32), ty, arguments))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            array_types.len(),
            1,
            "every runtime length must share one Array[Int] type"
        );
        let (array_type, array, arguments) = array_types[0];
        assert_eq!(array.name, "Array[Int]");
        assert_eq!(
            arguments.len(),
            1,
            "an array type stores only its element type"
        );
        assert!(matches!(
            program.types[arguments[0].index() as usize].kind,
            bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Int)
        ));

        let length_function =
            &program.functions[function_id(&program, "runtimeLength").index() as usize];
        let lengths = length_function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            ty,
                            kind: bc::BytecodeRvalueKind::Length(operand),
                        },
                    ..
                } => Some((*ty, operand)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !lengths.is_empty(),
            "array patterns must read runtime length"
        );
        for (result, operand) in lengths {
            assert!(matches!(
                program.types[result.index() as usize].kind,
                bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::Int)
            ));
            assert_eq!(operand.ty, array_type);
        }

        assert_eq!(
            execute_function(source, "observe"),
            RuntimeValue::Tuple(vec![
                RuntimeValue::Integer(0),
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(2),
                RuntimeValue::Integer(3),
                RuntimeValue::Integer(4),
                RuntimeValue::Integer(5),
                RuntimeValue::Integer(0),
                RuntimeValue::Integer(5),
                RuntimeValue::Integer(5),
            ])
        );

        let mut forged = program;
        let int_type = forged
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
        let length_function_id = function_id(&forged, "runtimeLength");
        let length_function = &mut forged.functions[length_function_id.index() as usize];
        let operand = length_function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::Length(operand),
                            ..
                        },
                    ..
                } => Some(operand),
                _ => None,
            })
            .expect("runtimeLength must contain a Length rvalue");
        *operand = bc::BytecodeOperand {
            ty: int_type,
            kind: bc::BytecodeOperandKind::Constant(bc::BytecodeConstant::Integer("0".into())),
        };
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("rvalue"), "{error}");
    }

    #[test]
    fn string_length_rvalue_counts_unicode_scalars() {
        let source = r#"fn textLength(): Int {
    let text = "añ🙂"
    let values = [0, 0, 0]
    match values {
        [] => 0
        [_, _, _] => 3
        [_, ..] => 1
    }
}
"#;
        let mut program = lowered(source);
        let string_type = program
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Scalar(bc::BytecodeScalarType::String)
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .expect("the function must retain its String type");
        let function_id = function_id(&program, "textLength");
        let function = &mut program.functions[function_id.index() as usize];
        let string_slot = function
            .slots
            .iter()
            .enumerate()
            .find(|(_, slot)| {
                slot.ty == string_type && matches!(slot.kind, bc::BytecodeSlotKind::User { .. })
            })
            .map(|(index, _)| bc::BytecodeSlotId::new(index as u32))
            .expect("the text binding must have a user slot");
        let operand = function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::Length(operand),
                            ..
                        },
                    ..
                } => Some(operand),
                _ => None,
            })
            .expect("the array match must contain a Length rvalue");
        *operand = bc::BytecodeOperand {
            ty: string_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: string_slot,
                ty: string_type,
                projections: Vec::new(),
                source_loan: None,
            }),
        };

        bc::verify_bytecode(&program).unwrap();
        let mut host = RejectingHost;
        let result = execute(&program, function_id, &mut host).unwrap();
        assert_eq!(
            result.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(3))
        );
    }

    #[test]
    fn string_index_access_tag_is_closed() {
        let mut program = lowered(
            "fn characterAt(text: String, index: Int): Char {\n\
                 text[index]\n\
             }\n",
        );
        let function_id = function_id(&program, "characterAt");
        let access = program.functions[function_id.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Index { access, .. },
                            ..
                        },
                    ..
                } => Some(access),
                _ => None,
            })
            .expect("String indexing must lower to a checked Index operation");
        assert_eq!(*access, bc::BytecodeIndexAccess::String);
        *access = bc::BytecodeIndexAccess::Array;
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error.message().contains("operation")
                || error.message().contains("projection")
                || error.message().contains("intrinsic"),
            "{error}"
        );
    }

    #[test]
    fn array_indices_normalize_reads_writes_borrows_and_bounds_uniformly() {
        let source = r#"fn inspect(value: ref Int): Int {
    value
}

fn observe(): (Int, Int, Int, Int, Int, Int, Int) {
    var values = [10, 20, 30, 40]
    let first = values[0]
    let lastPositive = values[3]
    let lastNegative = values[-1]
    let firstNegative = values[-4]
    values[1] = 21
    values[-2] = 31
    let borrowed = inspect(ref values[-1])
    (
        first,
        lastPositive,
        lastNegative,
        firstNegative,
        values[1],
        values[2],
        borrowed,
    )
}

fn highRead(): Int {
    let values = [10, 20, 30, 40]
    let index = 4
    values[index]
}

fn lowRead(): Int {
    let values = [10, 20, 30, 40]
    let index = -5
    values[index]
}

fn emptyRead(): Int {
    let values: Array[Int] = []
    let index = 0
    values[index]
}

fn minimumRead(): Int {
    let values = [10, 20, 30, 40]
    let index = -9223372036854775808
    values[index]
}

fn maximumRead(): Int {
    let values = [10, 20, 30, 40]
    let index = 9223372036854775807
    values[index]
}

fn highWrite(): Array[Int] {
    var values = [10, 20, 30, 40]
    let index = 4
    values[index] = 99
    values
}

fn lowWrite(): Array[Int] {
    var values = [10, 20, 30, 40]
    let index = -5
    values[index] = 99
    values
}

fn panicRhs(): Int {
    panic("rhs")
}

fn invalidWriteRunsRhs() {
    var values = [1]
    let index = 1
    values[index] = panicRhs()
}

fn verifierTarget(index: Int, flag: Bool): Int {
    _ = flag
    [1, 2][index]
}
"#;
        assert_eq!(
            execute_function(source, "observe"),
            RuntimeValue::Tuple(vec![
                RuntimeValue::Integer(10),
                RuntimeValue::Integer(40),
                RuntimeValue::Integer(40),
                RuntimeValue::Integer(10),
                RuntimeValue::Integer(21),
                RuntimeValue::Integer(31),
                RuntimeValue::Integer(40),
            ])
        );

        for name in [
            "highRead",
            "lowRead",
            "emptyRead",
            "minimumRead",
            "maximumRead",
            "highWrite",
            "lowWrite",
        ] {
            let VmOutcome::Panicked(panic) = execute_outcome(source, name) else {
                panic!("{name} must panic on an invalid array index")
            };
            assert_eq!(panic.code, PanicCode::Bounds, "{name}");
            assert_eq!(panic.code.code(), "P0001", "{name}");
        }

        let VmOutcome::Panicked(panic) = execute_outcome(source, "invalidWriteRunsRhs") else {
            panic!("the RHS panic must occur before write bounds validation")
        };
        assert_eq!(panic.code, PanicCode::ExplicitPanic);
        assert_eq!(panic.message, "rhs");

        let mut forged = lowered(source);
        let target = function_id(&forged, "verifierTarget");
        let function = &mut forged.functions[target.index() as usize];
        let bool_slot = function.parameters[1];
        let bool_type = function.slots[bool_slot.index() as usize].ty;
        let index = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Index { index, .. },
                            ..
                        },
                    ..
                } => Some(index),
                _ => None,
            })
            .expect("verifierTarget must contain an index operation");
        *index = bc::BytecodeOperand {
            ty: bool_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: bool_slot,
                ty: bool_type,
                projections: Vec::new(),
                source_loan: None,
            }),
        };
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("projection"), "{error}");

        let mut forged = lowered(source);
        let target = function_id(&forged, "highWrite");
        let function = &mut forged.functions[target.index() as usize];
        let access = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidatePlaces { places, .. } => places
                    .iter_mut()
                    .flat_map(|place| &mut place.projections)
                    .find_map(|projection| match &mut projection.kind {
                        bc::BytecodeProjectionKind::Index { access, .. } => Some(access),
                        _ => None,
                    }),
                _ => None,
            })
            .expect("highWrite must validate an indexed place");
        *access = bc::BytecodeIndexAccess::String;
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("projection"), "{error}");
    }

    #[test]
    fn array_slices_share_defaults_clamping_extreme_steps_and_verification() {
        let source = r#"fn values(): Array[Int] {
    [0, 1, 2, 3, 4]
}

fn full(): Array[Int] {
    values()[:]
}

fn middle(): Array[Int] {
    values()[1:4]
}

fn clipped(): Array[Int] {
    values()[-100:100]
}

fn alternate(): Array[Int] {
    values()[::2]
}

fn reversed(): Array[Int] {
    values()[::-1]
}

fn explicitNegativeEnd(): Array[Int] {
    values()[:-1:-1]
}

fn reverseStride(): Array[Int] {
    values()[4:0:-2]
}

fn negativeBounds(): Array[Int] {
    values()[-1:-6:-2]
}

fn extremeClipped(): Array[Int] {
    let minimum = -9223372036854775808
    let maximum = 9223372036854775807
    values()[minimum:maximum]
}

fn extremeReversed(): Array[Int] {
    let minimum = -9223372036854775808
    let maximum = 9223372036854775807
    values()[maximum:minimum:-1]
}

fn minimumStep(): Array[Int] {
    let step = -9223372036854775808
    values()[::step]
}

fn emptyReversed(): Array[Int] {
    let empty: Array[Int] = []
    empty[::-1]
}

fn zeroStep(): Array[Int] {
    let step = 0
    values()[::step]
}

fn verifierTarget(start: Int, flag: Bool): Array[Int] {
    _ = flag
    values()[start:]
}
"#;
        let program = lowered(source);
        let run = |name| {
            let mut host = RejectingHost;
            execute(&program, function_id(&program, name), &mut host)
                .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)))
                .outcome
        };
        let array = |items: &[i128]| {
            RuntimeValue::Array(items.iter().copied().map(RuntimeValue::Integer).collect())
        };
        for (name, expected) in [
            ("full", array(&[0, 1, 2, 3, 4])),
            ("middle", array(&[1, 2, 3])),
            ("clipped", array(&[0, 1, 2, 3, 4])),
            ("alternate", array(&[0, 2, 4])),
            ("reversed", array(&[4, 3, 2, 1, 0])),
            ("explicitNegativeEnd", array(&[])),
            ("reverseStride", array(&[4, 2])),
            ("negativeBounds", array(&[4, 2, 0])),
            ("extremeClipped", array(&[0, 1, 2, 3, 4])),
            ("extremeReversed", array(&[4, 3, 2, 1, 0])),
            ("minimumStep", array(&[4])),
            ("emptyReversed", array(&[])),
        ] {
            assert_eq!(run(name), VmOutcome::Returned(expected), "{name}");
        }

        let VmOutcome::Panicked(panic) = run("zeroStep") else {
            panic!("zeroStep must panic");
        };
        assert_eq!(panic.code, PanicCode::ZeroSliceStep);
        assert_eq!(panic.code.code(), "P0002");

        let mut forged = program;
        let target = function_id(&forged, "verifierTarget");
        let function = &mut forged.functions[target.index() as usize];
        let bool_slot = function.parameters[1];
        let bool_type = function.slots[bool_slot.index() as usize].ty;
        let bounds = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Slice { bounds, .. },
                            ..
                        },
                    ..
                } => Some(bounds),
                _ => None,
            })
            .expect("verifierTarget must contain a slice operation");
        bounds.start = Some(bc::BytecodeOperand {
            ty: bool_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: bool_slot,
                ty: bool_type,
                projections: Vec::new(),
                source_loan: None,
            }),
        });
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(
            error.message().contains("operation") || error.message().contains("projection"),
            "{error}"
        );
    }

    #[test]
    fn slice_snapshot_verification_requires_copy_elements() {
        let borrowed = lowered(
            "fn inspect(values: ref Array[Join[Int, Never]]) {}\n\
             fn borrow(values: ref Array[Join[Int, Never]]) {\n\
                 inspect(ref values[:])\n\
             }\n",
        );
        bc::verify_bytecode(&borrowed).unwrap();
        assert!(borrowed.functions.iter().any(|function| {
            function.loans.iter().any(|loan| {
                loan.place.projections.iter().any(|projection| {
                    matches!(projection.kind, bc::BytecodeProjectionKind::Slice { .. })
                })
            })
        }));

        let mut forged = lowered(
            "fn view(values: Array[Int]): Array[Int] {\n\
                 values[:]\n\
             }\n\
             fn consume(value: Join[Int, Never]): Never {\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let join = forged
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Intrinsic {
                        constructor: bc::BytecodeIntrinsicType::Join,
                        ..
                    }
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .expect("the terminal parameter retains its Join type");
        let array = forged
            .types
            .iter_mut()
            .find(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Intrinsic {
                        constructor: bc::BytecodeIntrinsicType::Array,
                        ..
                    }
                )
            })
            .expect("view retains its Array type");
        let bc::BytecodeTypeKind::Intrinsic { arguments, .. } = &mut array.kind else {
            unreachable!()
        };
        arguments[0] = join;

        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(
            error.message().contains("materializes a non-Copy Array"),
            "{error}"
        );
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
                .filter(|callable| !callable.name.starts_with("std."))
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
    fn aggregate_constants_use_the_shared_lowering_helpers() {
        let program = lowered(
            "type UserId = Int\n\
             type Person = { id: UserId, name: String }\n\
             enum Choice { Empty, Item(Int), Named { value: Int } }\n\
             const UnitValue: Unit = ()\n\
             const Flag: Bool = true\n\
             const Answer: Int = 42\n\
             const Ratio: Float = 2.5\n\
             const Letter: Char = 'x'\n\
             const Text: String = \"value\"\n\
             const Pair: (Int, String) = (Answer, Text)\n\
             const Numbers: Array[Int] = [1, 2]\n\
             const Entries: Map[String, Int] = [\"one\": 1]\n\
             const Permissions: Set[String] = Set[\"read\"]\n\
             const Missing: Int? = none\n\
             const Present: Int? = some(Answer)\n\
             const Success: Int ! String = ok(Answer)\n\
             const Failure: Int ! String = err(Text)\n\
             const Span: Range[Int] = 1..=3\n\
             const Converted: Int8 ! NumericConversionError = Int8(127)\n\
             const ConversionFailure: Int8 ! NumericConversionError = Int8(128)\n\
             const Id: UserId = UserId(9)\n\
             const User: Person = Person { id: Id, name: \"Ada\" }\n\
             const UnitChoice: Choice = Choice.Empty\n\
             const TupleChoice: Choice = Choice.Item(1)\n\
             const RecordChoice: Choice = Choice.Named { value: 2 }\n\
             fn read(): Int { match RecordChoice { Choice.Empty => 0\n Choice.Item(value) => value\n Choice.Named { value } => value\n } }\n",
        );
        assert!(program.constants.len() >= 20);
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
                      fn render[T: Discard + Summary](value: T): String { value.summarize() }\n\
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
                      fn render[T: Discard + Display](value: T): String { value.display() }\n\
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
    fn interpolation_uses_static_display_and_survives_gc_pressure() {
        let source = r#"
type Label = { text: String }

impl Display for Label {
    fn display(self): String { self.text }
}

fn render[T: Discard + Display](value: T): String {
    "<{value}>"
}

fn hidden(): impl Display + Discard {
    9
}

fn execute(): String {
    let label = Label { text: "Tondo" }
    "{render(42)}:{render(label)}:{render(hidden())}"
}
"#;
        let program = lowered(source);
        let entry = function_id(&program, "execute");
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    &block.terminator.kind,
                    bc::BytecodeTerminatorKind::Invoke {
                        operation: bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Display { .. },
                            ..
                        },
                        ..
                    }
                )
            })
        }));
        assert!(program.functions.iter().any(|function| {
            function
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| {
                    matches!(
                        &instruction.kind,
                        bc::BytecodeInstructionKind::Store {
                            value: bc::BytecodeRvalue {
                                kind: bc::BytecodeRvalueKind::Interpolate { .. },
                                ..
                            },
                            ..
                        }
                    )
                })
        }));
        for limits in [
            VmLimits::default(),
            VmLimits {
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        ] {
            let mut host = RejectingHost;
            let result = execute_with_limits(&program, entry, &mut host, limits)
                .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
            assert_eq!(
                result.outcome,
                VmOutcome::Returned(RuntimeValue::String("<42>:<Tondo>:<9>".into()))
            );
        }
    }

    #[test]
    fn bytecode_verifier_rejects_forged_interpolation_and_display_shapes() {
        let source = "fn execute(value: Int): String { \"value={value}\" }\n";
        let program = lowered(source);

        let mut interpolation = program.clone();
        let value = interpolation
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::Interpolate { segments, .. },
                            ..
                        },
                    ..
                } => Some(segments),
                _ => None,
            })
            .expect("interpolation must lower to one verified rvalue");
        value.clear();
        assert!(bc::verify_bytecode(&interpolation).is_err());

        let mut display = program;
        let argument = display
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Display { argument },
                            ..
                        },
                    ..
                } => Some(argument),
                _ => None,
            })
            .expect("scalar Display must lower to one verified intrinsic operation");
        argument.target = bc::BytecodeCallArgumentTarget::Fixed(0);
        assert!(bc::verify_bytecode(&display).is_err());
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
                      fn unwrap[T](boxed: Box[T]): T {\n\
                          let Box { value } = boxed\n\
                          value\n\
                      }\n\
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
    fn ref_identity_is_shared_and_its_content_survives_gc_pressure() {
        let aliases = "fn observe(reference: ref Ref[Int]): Int { reference.value }\n\
                       fn aliases(): (Ref[Int], Ref[Int]) {\n\
                           let reference = Ref(42)\n\
                           let same = reference\n\
                           assert(observe(ref same) == 42)\n\
                           (reference, same)\n\
                       }\n";
        let program = lowered(aliases);
        let function = function_id(&program, "aliases");
        let mut host = RejectingHost;
        let execution = execute(&program, function, &mut host).unwrap();
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Tuple(vec![
                RuntimeValue::Ref(Some(Box::new(RuntimeValue::Integer(42)))),
                RuntimeValue::Ref(Some(Box::new(RuntimeValue::Integer(42)))),
            ]))
        );
        assert_eq!(
            execution.statistics.allocations, 2,
            "one Ref cell and one result tuple must be allocated; copying Ref must not allocate"
        );

        let retained = "fn retained(): Ref[Array[String]] {\n\
                            let reference = Ref([\"alive\"])\n\
                            let same = reference\n\
                            var index = 0\n\
                            for index < 64 {\n\
                                _ = [\"garbage\", \"pressure\"]\n\
                                index += 1\n\
                            }\n\
                            assert(reference.value[0] == \"alive\")\n\
                            same\n\
                        }\n";
        let program = lowered(retained);
        let function = function_id(&program, "retained");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            function,
            &mut host,
            VmLimits {
                max_heap_objects: 32,
                max_heap_bytes: 64 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Ref(Some(Box::new(RuntimeValue::Array(
                vec![RuntimeValue::String("alive".into())],
            )))))
        );
        assert!(execution.statistics.collections > 0);
        assert!(execution.statistics.reclaimed_objects > 0);
    }

    #[test]
    fn ref_equality_and_collection_keys_use_identity_independently_of_content() {
        let value = execute_function(
            "fn identity(value: Int): Int { value }\n\
             fn compare(): Bool {\n\
                 let first = Ref(identity)\n\
                 let same = first\n\
                 let other = Ref(identity)\n\
                 let missing = Ref(identity)\n\
                 assert(first == same)\n\
                 assert(first != other)\n\
                 let values = [first: 1, same: 3, other: 2]\n\
                 assert(values[first] == some(3))\n\
                 assert(values[same] == some(3))\n\
                 assert(values[other] == some(2))\n\
                 let identities = Set[first, same, other]\n\
                 assert(same in identities)\n\
                 assert(other in identities)\n\
                 assert(not (missing in identities))\n\
                 true\n\
             }\n",
            "compare",
        );
        assert_eq!(value, RuntimeValue::Bool(true));
    }

    #[test]
    fn eager_logical_copies_cover_every_managed_copy_shape() {
        const DECLARATIONS: &str = "type Wrapped = Int\n\
            type Record = { value: Int }\n\
            enum Choice {\n\
                Empty\n\
                Item(Int)\n\
                Named { value: Int }\n\
            }\n";

        fn allocations(body: &str, case: &str) -> u64 {
            let source = format!("{DECLARATIONS}fn execute(): Bool {{\n{body}\n}}\n");
            let program = lowered(&source);
            let mut host = RejectingHost;
            let execution = execute_with_limits_and_copy_strategy(
                &program,
                function_id(&program, "execute"),
                &mut host,
                VmLimits::default(),
                ValueCopyStrategy::Eager,
            )
            .unwrap_or_else(|error| panic!("{case}: {error}\n{}", bc::disassemble(&program)));
            assert_eq!(
                execution.outcome,
                VmOutcome::Returned(RuntimeValue::Bool(true)),
                "{case}"
            );
            execution.statistics.allocations
        }

        for (name, setup, binding, original_result, copied_result, shares_storage) in [
            (
                "tuple",
                "let original = (1, 2)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "array",
                "let original = [1, 2]",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "map",
                "let original = [1: 2]",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "set",
                "let original = Set[1, 2]",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "closure",
                "var count = 0\n\
                 var original = (): Int {\n\
                     count += 1\n\
                     count\n\
                 }",
                "var copied = original",
                "original() == 1",
                "original() == 1 and copied() == 1",
                false,
            ),
            (
                "newtype",
                "let original = Wrapped(1)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "record",
                "let original = Record { value: 1 }",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "unit variant",
                "let original = Choice.Empty",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "tuple variant",
                "let original = Choice.Item(1)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "record variant",
                "let original = Choice.Named { value: 1 }",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "none",
                "let original: Int? = none",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "some",
                "let original = some(1)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "ok",
                "let original: Int ! Bool = ok(1)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "err",
                "let original: Int ! Bool = err(false)",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "union",
                "let original: Int | Bool = 1",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "range",
                "let original = 1..=3",
                "let copied = original",
                "2 in original",
                "2 in copied",
                false,
            ),
            (
                "nested",
                "let original = ([1, 2], [3, 4])",
                "let copied = original",
                "original == original",
                "copied == original",
                false,
            ),
            (
                "string",
                "let original = \"value\"",
                "let copied = original",
                "original == original",
                "copied == original",
                true,
            ),
            (
                "Ref",
                "let original = Ref(1)",
                "let copied = original",
                "original == original",
                "copied == original",
                true,
            ),
        ] {
            let baseline = allocations(&format!("{setup}\n{original_result}"), name);
            let copied = allocations(&format!("{setup}\n{binding}\n{copied_result}"), name);
            if shares_storage {
                assert_eq!(
                    copied, baseline,
                    "{name} must preserve its deliberate sharing rule"
                );
            } else {
                let minimum_copy = if name == "nested" { 3 } else { 1 };
                assert!(
                    copied >= baseline + minimum_copy,
                    "{name} did not eagerly allocate its complete logical copy: \
                     baseline={baseline}, copied={copied}"
                );
            }
        }

        assert_eq!(
            execute_function(
                "type Holder = { values: Array[Int] }\n\
                 type Values = Array[Int]\n\
                 fn separated(): Bool {\n\
                     var originalTuple = ([1], [2])\n\
                     var copiedTuple = originalTuple\n\
                     copiedTuple.0[0] = 9\n\
                     var originalRecord = Holder { values: [3] }\n\
                     var copiedRecord = originalRecord\n\
                     copiedRecord.values[0] = 8\n\
                     var originalNewtype = Values([4])\n\
                     var copiedNewtype = originalNewtype\n\
                     copiedNewtype.value[0] = 7\n\
                     var originalMap = [1: 5]\n\
                     var copiedMap = originalMap\n\
                     copiedMap[1] = 6\n\
                     originalTuple.0[0] == 1 and copiedTuple.0[0] == 9 and\n\
                         originalRecord.values[0] == 3 and copiedRecord.values[0] == 8 and\n\
                         originalNewtype.value[0] == 4 and copiedNewtype.value[0] == 7 and\n\
                         originalMap[1] == some(5) and copiedMap[1] == some(6)\n\
                 }\n",
                "separated",
            ),
            RuntimeValue::Bool(true)
        );
    }

    #[test]
    fn collection_copy_profile_justifies_cow_with_reproducible_workloads() {
        fn run(source: &str, strategy: ValueCopyStrategy) -> VmStatistics {
            let program = lowered(source);
            let function = function_id(&program, "execute");
            let mut host = RejectingHost;
            let execution = execute_with_limits_and_copy_strategy(
                &program,
                function,
                &mut host,
                VmLimits::default(),
                strategy,
            )
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
            assert_eq!(
                execution.outcome,
                VmOutcome::Returned(RuntimeValue::Integer(32))
            );
            execution.statistics
        }

        let array_values = (0..256)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let array_source = format!(
            "fn inspect(values: Array[Int]): Bool {{\n\
                 values[0] == 0 and values[-1] == 255\n\
             }}\n\
             fn execute(): Int {{\n\
                 let values = [{array_values}]\n\
                 var matches = 0\n\
                 for _ in 0..32 {{\n\
                     if inspect(values) {{\n\
                         matches += 1\n\
                     }}\n\
                 }}\n\
                 matches\n\
             }}\n"
        );

        let map_values = (0..128)
            .map(|value| format!("{value}: {value}"))
            .collect::<Vec<_>>()
            .join(", ");
        let map_source = format!(
            "fn inspect(values: Map[Int, Int]): Bool {{\n\
                 values[0] == some(0) and values[127] == some(127)\n\
             }}\n\
             fn execute(): Int {{\n\
                 let values = [{map_values}]\n\
                 var matches = 0\n\
                 for _ in 0..32 {{\n\
                     if inspect(values) {{\n\
                         matches += 1\n\
                     }}\n\
                 }}\n\
                 matches\n\
             }}\n"
        );

        let set_values = (0..128)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let set_source = format!(
            "fn inspect(values: Set[Int]): Bool {{\n\
                 0 in values and 127 in values\n\
             }}\n\
             fn execute(): Int {{\n\
                 let values = Set[{set_values}]\n\
                 var matches = 0\n\
                 for _ in 0..32 {{\n\
                     if inspect(values) {{\n\
                         matches += 1\n\
                     }}\n\
                 }}\n\
                 matches\n\
             }}\n"
        );

        for (name, source, elements) in [
            ("Array[Int]", array_source, 256_u64),
            ("Map[Int, Int]", map_source, 128),
            ("Set[Int]", set_source, 128),
        ] {
            let eager = run(&source, ValueCopyStrategy::Eager);
            let cow = run(&source, ValueCopyStrategy::CopyOnWrite);
            eprintln!(
                "{name}: logical_copies={}, eager_elements={}, cow_elements={}, cow_shares={}",
                eager.logical_collection_copies,
                eager.collection_elements_copied,
                cow.collection_elements_copied,
                cow.collection_buffer_shares,
            );
            assert_eq!(eager.logical_collection_copies, 65, "{name}");
            assert_eq!(
                cow.logical_collection_copies, eager.logical_collection_copies,
                "{name}"
            );
            assert_eq!(
                eager.collection_elements_copied,
                eager.logical_collection_copies * elements,
                "{name}"
            );
            assert_eq!(eager.collection_buffer_shares, 0, "{name}");
            assert_eq!(eager.collection_buffer_detaches, 0, "{name}");
            assert_eq!(cow.collection_elements_copied, 0, "{name}");
            assert_eq!(
                cow.collection_buffer_shares, cow.logical_collection_copies,
                "{name}"
            );
            assert_eq!(cow.collection_buffer_detaches, 0, "{name}");
        }
    }

    #[test]
    fn cow_detaches_shared_collection_buffers_before_writes() {
        let source = "fn uniqueWrite(): Bool {\n\
             var unique = [1, 2]\n\
             unique[0] = 3\n\
             unique == [3, 2]\n\
         }\n\
         fn arrayWrite(): Bool {\n\
             var original = [10, 20]\n\
             var copied = original\n\
             copied[0] = 30\n\
             original == [10, 20] and copied == [30, 20]\n\
         }\n\
         fn mapWrite(): Bool {\n\
             var originalMap = [1: 10]\n\
             var copiedMap = originalMap\n\
             copiedMap[1] = 30\n\
             originalMap == [1: 10] and copiedMap == [1: 30]\n\
         }\n";
        let program = lowered(source);
        let run = |name, strategy| {
            let mut host = RejectingHost;
            execute_with_limits_and_copy_strategy(
                &program,
                function_id(&program, name),
                &mut host,
                VmLimits::default(),
                strategy,
            )
            .unwrap()
        };

        // Checked writes retain a transactional pre-write snapshot, so even
        // `uniqueWrite` has two physical buffer owners at detachment time.
        for name in ["uniqueWrite", "arrayWrite", "mapWrite"] {
            let eager = run(name, ValueCopyStrategy::Eager);
            let cow = run(name, ValueCopyStrategy::CopyOnWrite);
            assert_eq!(
                eager.outcome,
                VmOutcome::Returned(RuntimeValue::Bool(true)),
                "{name}"
            );
            assert_eq!(cow.outcome, eager.outcome, "{name}");
            assert_eq!(eager.statistics.collection_buffer_shares, 0, "{name}");
            assert_eq!(eager.statistics.collection_buffer_detaches, 0, "{name}");
            assert_eq!(cow.statistics.collection_buffer_detaches, 1, "{name}");
        }
    }

    #[test]
    fn eager_and_cow_match_the_same_value_copy_observable_corpus() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one String")
                };
                self.output.push_str(text);
                Ok(RuntimeValue::Unit)
            }
        }

        fn run(
            program: &bc::BytecodeProgram,
            function: bc::BytecodeFunctionId,
            strategy: ValueCopyStrategy,
            limits: VmLimits,
        ) -> (VmOutcome, String, VmStatistics) {
            let mut host = RecordingHost::default();
            let execution = execute_with_limits_and_copy_strategy(
                program, function, &mut host, limits, strategy,
            )
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(program)));
            (execution.outcome, host.output, execution.statistics)
        }

        let mut cow_shares = 0_u64;
        let mut cow_detaches = 0_u64;
        for (name, source) in [
            (
                "gc-pressure",
                include_str!("../../../../tests/runtime/value-copy/gc-pressure.to"),
            ),
            (
                "identity",
                include_str!("../../../../tests/runtime/value-copy/identity.to"),
            ),
            (
                "iteration",
                include_str!("../../../../tests/runtime/value-copy/iteration.to"),
            ),
            (
                "map-remove",
                include_str!("../../../../tests/runtime/value-copy/map-remove.to"),
            ),
            (
                "panic",
                include_str!("../../../../tests/runtime/value-copy/panic.to"),
            ),
            (
                "slice-snapshot",
                include_str!("../../../../tests/runtime/value-copy/slice-snapshot.to"),
            ),
            (
                "value",
                include_str!("../../../../tests/runtime/value-copy/value.to"),
            ),
            (
                "write-independence",
                include_str!("../../../../tests/runtime/value-copy/write-independence.to"),
            ),
        ] {
            let program = lowered(source);
            let function = function_id(&program, "main");
            let eager = run(
                &program,
                function,
                ValueCopyStrategy::Eager,
                VmLimits::default(),
            );
            let cow = run(
                &program,
                function,
                ValueCopyStrategy::CopyOnWrite,
                VmLimits::default(),
            );
            assert_eq!((&cow.0, &cow.1), (&eager.0, &eager.1), "{name}");
            assert_eq!(eager.2.collection_buffer_shares, 0, "{name}");
            assert_eq!(eager.2.collection_buffer_detaches, 0, "{name}");
            cow_shares = cow_shares.saturating_add(cow.2.collection_buffer_shares);
            cow_detaches = cow_detaches.saturating_add(cow.2.collection_buffer_detaches);

            let pressure = VmLimits {
                initial_gc_threshold: 1,
                ..VmLimits::default()
            };
            let eager_pressure = run(&program, function, ValueCopyStrategy::Eager, pressure);
            let cow_pressure = run(&program, function, ValueCopyStrategy::CopyOnWrite, pressure);
            assert_eq!(
                (&eager_pressure.0, &eager_pressure.1),
                (&eager.0, &eager.1),
                "{name}: eager changed under GC pressure"
            );
            assert_eq!(
                (&cow_pressure.0, &cow_pressure.1),
                (&eager.0, &eager.1),
                "{name}: COW changed an observable"
            );
            assert!(eager_pressure.2.collections > 0, "{name}");
            assert!(cow_pressure.2.collections > 0, "{name}");
            cow_shares = cow_shares.saturating_add(cow_pressure.2.collection_buffer_shares);
            cow_detaches = cow_detaches.saturating_add(cow_pressure.2.collection_buffer_detaches);
        }
        assert!(cow_shares > 0, "the corpus did not exercise COW sharing");
        assert!(
            cow_detaches > 0,
            "the corpus did not exercise COW detachment"
        );
    }

    #[test]
    fn bytecode_verifier_seals_ref_shape_and_shared_value_access() {
        let source = "fn make(value: Int): Ref[Int] { Ref(value) }\n\
                      fn inspect(value: ref Int) {}\n\
                      fn inspectArray(value: ref Array[Int]) {}\n\
                      fn arrayIdentity(value: Array[Int]): Array[Int] { value }\n\
                      fn read(reference: Ref[Int]): Int {\n\
                          inspect(ref reference.value)\n\
                          reference.value\n\
                      }\n\
                      fn writeSinks(reference: Ref[Array[Int]]) {\n\
                          inspectArray(ref reference.value)\n\
                          _ = arrayIdentity([1])\n\
                          for item in [[1]] {\n\
                              _ = item\n\
                          }\n\
                          var values = [1, 2]\n\
                          values[:] = [3, 4]\n\
                      }\n";
        let program = lowered(source);
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        &instruction.kind,
                        bc::BytecodeInstructionKind::Store {
                            value:
                                bc::BytecodeRvalue {
                                    kind:
                                        bc::BytecodeRvalueKind::Construct {
                                            shape: bc::BytecodeAggregateKind::Ref,
                                            values,
                                        },
                                    ..
                                },
                            ..
                        } if values.len() == 1
                    )
                })
            })
        }));
        bc::verify_bytecode(&program).unwrap();

        let mut malformed = program.clone();
        let values = malformed
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind:
                                bc::BytecodeRvalueKind::Construct {
                                    shape: bc::BytecodeAggregateKind::Ref,
                                    values,
                                },
                            ..
                        },
                    ..
                } => Some(values),
                _ => None,
            })
            .unwrap();
        values.clear();
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("rvalue operands"));

        let mut moved = program.clone();
        let read = function_id(&moved, "read");
        let function = &mut moved.functions[read.index() as usize];
        let operand = function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::Use(operand),
                            ..
                        },
                    ..
                } if matches!(
                    &operand.kind,
                    bc::BytecodeOperandKind::Copy(place)
                        if place.projections.iter().any(|projection| {
                            matches!(
                                &projection.kind,
                                bc::BytecodeProjectionKind::RefValue
                            )
                        })
                ) =>
                {
                    Some(operand)
                }
                _ => None,
            })
            .unwrap();
        let bc::BytecodeOperandKind::Copy(place) = &operand.kind else {
            unreachable!("the selected Ref projection is copied")
        };
        operand.kind = bc::BytecodeOperandKind::Move(place.clone());
        let error = bc::verify_bytecode(&moved).unwrap_err();
        assert!(error.message().contains("cannot be moved"));

        let mut written = program.clone();
        let read = function_id(&written, "read");
        let function = &mut written.functions[read.index() as usize];
        let mut forged = false;
        'blocks: for block in &mut function.blocks {
            for instruction in &mut block.instructions {
                let bc::BytecodeInstructionKind::Store { destination, value } =
                    &mut instruction.kind
                else {
                    continue;
                };
                let bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                    kind: bc::BytecodeOperandKind::Copy(place),
                    ..
                }) = &value.kind
                else {
                    continue;
                };
                if place.projections.iter().any(|projection| {
                    matches!(&projection.kind, bc::BytecodeProjectionKind::RefValue)
                }) {
                    *destination = place.clone();
                    forged = true;
                    break 'blocks;
                }
            }
        }
        assert!(forged);
        let error = bc::verify_bytecode(&written).unwrap_err();
        assert!(error.message().contains("read-only"));

        let mut exclusive = program.clone();
        let read = function_id(&exclusive, "read");
        let function = &mut exclusive.functions[read.index() as usize];
        let loan = function
            .loans
            .iter_mut()
            .find(|loan| {
                loan.place.projections.iter().any(|projection| {
                    matches!(&projection.kind, bc::BytecodeProjectionKind::RefValue)
                })
            })
            .unwrap();
        loan.mode = bc::BytecodeParameterMode::Mut;
        let error = bc::verify_bytecode(&exclusive).unwrap_err();
        assert!(error.message().contains("only shared `ref` loans"));

        let write_sinks = function_id(&program, "writeSinks");
        let ref_value_place = program.functions[write_sinks.index() as usize]
            .loans
            .iter()
            .find(|loan| {
                loan.place.projections.iter().any(|projection| {
                    matches!(projection.kind, bc::BytecodeProjectionKind::RefValue)
                })
            })
            .unwrap()
            .place
            .clone();

        let mut invoked = program.clone();
        let destination = invoked.functions[write_sinks.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation,
                    destination: Some(destination),
                    ..
                } if operation.ty == ref_value_place.ty => Some(destination),
                _ => None,
            })
            .unwrap();
        *destination = ref_value_place.clone();
        let error = bc::verify_bytecode(&invoked).unwrap_err();
        assert!(error.message().contains("read-only"));

        let mut advanced = program.clone();
        let destination = advanced.functions[write_sinks.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext { destination, .. }
                    if destination.ty == ref_value_place.ty =>
                {
                    Some(destination)
                }
                _ => None,
            })
            .unwrap();
        *destination = ref_value_place.clone();
        let error = bc::verify_bytecode(&advanced).unwrap_err();
        assert!(error.message().contains("read-only"));

        let mut validated = program;
        let place = validated.functions[write_sinks.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidatePlaces {
                    places,
                    for_write: true,
                    ..
                } => places.first_mut(),
                _ => None,
            })
            .unwrap();
        *place = ref_value_place;
        let error = bc::verify_bytecode(&validated).unwrap_err();
        assert!(error.message().contains("read-only"));
    }

    #[test]
    fn generic_checked_operations_and_tag_projections_are_specialized() {
        let source = "fn first[T: Copy](values: Array[T]): T { values[0] }\n\
                      fn value_or[T: Discard](value: T?, fallback: T): T {\n\
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
    fn generic_closure_bodies_share_the_monomorphization_budget() {
        let source = "fn invoke[T: Copy + Discard](value: T): T {\n\
                          let get = (): T { value }\n\
                          get()\n\
                      }\n\
                      fn execute(): Int { invoke(42) }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let error = lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_generic_instantiations: 1,
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

        lower_to_bytecode(
            &resolved,
            &hir,
            &mir,
            BytecodeLoweringLimits {
                max_generic_instantiations: 2,
                ..BytecodeLoweringLimits::default()
            },
        )
        .unwrap();
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
    fn bytecode_function_metadata_corruption_matrix_is_closed() {
        let program = lowered(
            "fn inspect(value: ref Int) {}\n\
             fn combine(left: Int, right: Int, label: String): Int {\n\
                 inspect(ref left)\n\
                 let total = left + right\n\
                 _ = label\n\
                 total\n\
             }\n\
             fn main() {}\n",
        );
        bc::verify_bytecode(&program).unwrap();
        let combine = function_id(&program, "combine");

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].callable =
            bc::BytecodeCallableId::new(u32::MAX);
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("unknown callable"), "{error}");

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        function.source.start = function.source.end.saturating_add(1);
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("source span is reversed"),
            "{error}"
        );

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].types.clear();
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("type table is empty"), "{error}");

        let mut malformed = program.clone();
        let types = &mut malformed.functions[combine.index() as usize].types;
        assert!(types.len() >= 2);
        types[1] = types[0];
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error
                .message()
                .contains("type table is empty, duplicated, or unordered"),
            "{error}"
        );

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].spans.clear();
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("span table is empty"), "{error}");

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].spans[0].file = malformed.functions
            [combine.index() as usize]
            .source
            .file
            .saturating_add(1);
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("cross-file"), "{error}");

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].slots.clear();
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("no slots or blocks"), "{error}");

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].blocks.clear();
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("no slots or blocks"), "{error}");

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize].return_slot =
            bc::BytecodeSlotId::new(u32::MAX);
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(error.message().contains("unknown slot"), "{error}");

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        function.slots[function.return_slot.index() as usize].kind =
            bc::BytecodeSlotKind::Temporary;
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("return slot kind or type"),
            "{error}"
        );

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        let temporary = function
            .slots
            .iter()
            .position(|slot| slot.kind == bc::BytecodeSlotKind::Temporary)
            .expect("combine has one temporary slot");
        function.slots[temporary].kind = bc::BytecodeSlotKind::Return;
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("return or parameter slot count"),
            "{error}"
        );

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        let parameter = function.parameters[0];
        function.slots[parameter.index() as usize].kind =
            bc::BytecodeSlotKind::Parameter { index: 4 };
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error
                .message()
                .contains("parameter slot indices are not contiguous"),
            "{error}"
        );

        let mut malformed = program.clone();
        malformed.functions[combine.index() as usize]
            .parameters
            .swap(0, 1);
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("parameter slot table differs"),
            "{error}"
        );

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        let parameter = *function
            .parameters
            .last()
            .expect("combine has three parameter slots");
        function.slots[parameter.index() as usize].kind = bc::BytecodeSlotKind::Temporary;
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("return or parameter slot count"),
            "{error}"
        );

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        let user = function
            .slots
            .iter()
            .find_map(|slot| match slot.kind {
                bc::BytecodeSlotKind::User { local } => Some(local),
                _ => None,
            })
            .expect("combine has one user slot");
        let temporary = function
            .slots
            .iter()
            .position(|slot| slot.kind == bc::BytecodeSlotKind::Temporary)
            .expect("combine has one temporary slot");
        function.slots[temporary].kind = bc::BytecodeSlotKind::User { local: user };
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error
                .message()
                .contains("user local identity is duplicated"),
            "{error}"
        );

        let mut malformed = program.clone();
        let function = &mut malformed.functions[combine.index() as usize];
        function.entry = function.unwind;
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error.message().contains("required distinct shapes"),
            "{error}"
        );

        let mut malformed = program;
        let function = &mut malformed.functions[combine.index() as usize];
        let loan = function
            .loans
            .first_mut()
            .expect("the ref argument creates a call-local loan");
        loan.mode = bc::BytecodeParameterMode::Value;
        let error = bc::verify_bytecode(&malformed).unwrap_err();
        assert!(
            error
                .message()
                .contains("loan metadata uses the owning value mode"),
            "{error}"
        );
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
        let tooling = bc::disassemble(&program);
        assert!(tooling.contains("closure=Some(BytecodeClosure"));
        assert!(tooling.contains("protocols: BytecodeClosureProtocols"));

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
    fn affine_closure_captures_retain_moves_and_execute_nested_call_once() {
        let source = "fn make(value: Int): impl CallOnce[fn(): Int] + Discard {\n\
                          (): Int { value }\n\
                      }\n\
                      fn execute(): Int {\n\
                          let resource = make(42)\n\
                          let outer = (): Int { resource() }\n\
                          outer()\n\
                      }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();
        let (outer_id, outer_callable, outer) = program
            .callables
            .iter()
            .enumerate()
            .find_map(|(index, callable)| {
                callable.closure.as_ref().and_then(|closure| {
                    (!closure.protocols.call && !closure.protocols.call_mut).then_some((
                        bc::BytecodeCallableId::new(index as u32),
                        callable,
                        closure,
                    ))
                })
            })
            .expect("the outer closure is a concrete CallOnce environment");
        assert!(outer.protocols.call_once);
        let implementation = outer_callable.implementation.unwrap();
        let body = &program.functions[implementation.index() as usize];
        assert!(
            body.blocks.iter().any(
                |block| block.instructions.iter().any(|instruction| matches!(
                    &instruction.kind,
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                        kind: bc::BytecodeOperandKind::Move(place),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } if matches!(
                        place.projections.first().map(|projection| &projection.kind),
                        Some(bc::BytecodeProjectionKind::ClosureCapture {
                            callable,
                            ..
                        }) if *callable == outer_id
                    )
                ))
            )
        );
        assert!(program.functions.iter().any(|function| {
            function.blocks.iter().flat_map(|block| &block.instructions).any(
                |instruction| matches!(
                    &instruction.kind,
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Construct {
                                        shape: bc::BytecodeAggregateKind::Closure { callable, .. },
                                        values,
                                    },
                                ..
                            },
                        ..
                    } if *callable == outer_id
                        && matches!(
                            values.as_slice(),
                            [bc::BytecodeOperand {
                                kind: bc::BytecodeOperandKind::Move(_),
                                ..
                            }]
                        )
                ),
            )
        }));

        let mut forged = program.clone();
        let protocols = &mut forged.callables[outer_id.index() as usize]
            .closure
            .as_mut()
            .unwrap()
            .protocols;
        protocols.call = true;
        protocols.call_mut = true;
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("implementation body"), "{error}");

        let mut host = RejectingHost;
        let execution = execute(&program, function_id(&program, "execute"), &mut host)
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(42))
        );
    }

    #[test]
    fn affine_closure_capture_temporaries_survive_gc_pressure() {
        let program = lowered(
            "fn make(value: Int): impl CallOnce[fn(): Int] + Discard {\n\
                 (): Int { value }\n\
             }\n\
             fn execute(): Int {\n\
                 let first = make(20)\n\
                 let second = make(22)\n\
                 let combined = (): Int { first() + second() }\n\
                 combined()\n\
             }\n",
        );
        let entry = function_id(&program, "execute");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 16,
                max_heap_bytes: 64 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(42))
        );
        assert!(execution.statistics.collections > 0);
    }

    #[test]
    fn bytecode_rederives_call_once_for_a_terminal_closure_environment() {
        let source = "fn observe(value: ref Join[Int, String]) {}\n\
                      fn sink[T](value: T): Never { panic(\"stop\") }\n\
                      fn build(input: Join[Int, String]): Never {\n\
                          let operation = () { observe(ref input) }\n\
                          sink(operation)\n\
                      }\n";
        let program = lowered(source);
        let (callable_id, closure) = program
            .callables
            .iter()
            .enumerate()
            .find_map(|(index, callable)| {
                callable.closure.as_ref().and_then(|closure| {
                    (closure.protocols.call
                        && closure.protocols.call_mut
                        && !closure.protocols.call_once)
                        .then_some((bc::BytecodeCallableId::new(index as u32), closure))
                })
            })
            .expect("the Join environment is repeatable but cannot be consumed");
        assert_eq!(closure.captures.len(), 1);
        let join = program
            .types
            .iter()
            .position(|ty| {
                matches!(
                    ty.kind,
                    bc::BytecodeTypeKind::Intrinsic {
                        constructor: bc::BytecodeIntrinsicType::Join,
                        ..
                    }
                )
            })
            .map(|index| bc::BytecodeTypeId::new(index as u32))
            .expect("the terminal capture retains its concrete Join type");
        assert_eq!(
            bc::derive_terminal_statuses(&program, &[join, closure.environment]).unwrap(),
            [
                bc::BytecodeTerminalStatus::Present,
                bc::BytecodeTerminalStatus::Present,
            ]
        );
        bc::verify_bytecode(&program).unwrap();

        let mut forged = program;
        forged.callables[callable_id.index() as usize]
            .closure
            .as_mut()
            .unwrap()
            .protocols
            .call_once = true;
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("implementation body"), "{error}");

        let complete = lowered(
            "fn sink[T](value: T): Never { panic(\"stop\") }\n\
             fn build(input: Join[Int, String], choose: Bool): Never {\n\
                 let operation = (): Join[Int, String] {\n\
                     if choose {\n\
                         return input\n\
                     }\n\
                     input\n\
                 }\n\
                 sink(operation)\n\
             }\n",
        );
        assert_eq!(
            complete
                .callables
                .iter()
                .find_map(|callable| callable.closure.as_ref())
                .unwrap()
                .protocols,
            bc::BytecodeClosureProtocols {
                call: false,
                call_mut: false,
                call_once: true,
            }
        );
        bc::verify_bytecode(&complete).unwrap();

        let complete_newtype = lowered(
            "type Wrapped = Join[Int, String]\n\
             fn sink[T](value: T): Never { panic(\"stop\") }\n\
             fn build(input: Wrapped): Never {\n\
                 let operation = (): Join[Int, String] { input.value }\n\
                 sink(operation)\n\
             }\n",
        );
        assert_eq!(
            complete_newtype
                .callables
                .iter()
                .find_map(|callable| callable.closure.as_ref())
                .unwrap()
                .protocols,
            bc::BytecodeClosureProtocols {
                call: false,
                call_mut: false,
                call_once: true,
            }
        );
        bc::verify_bytecode(&complete_newtype).unwrap();

        let partial = lowered(
            "fn sink[T](value: T): Never { panic(\"stop\") }\n\
             fn build(input: Join[Int, String], choose: Bool): Never {\n\
                 let operation = (): Join[Int, String]? {\n\
                     if choose {\n\
                         return some(input)\n\
                     }\n\
                     none\n\
                 }\n\
                 sink(operation)\n\
             }\n",
        );
        assert_eq!(
            partial
                .callables
                .iter()
                .find_map(|callable| callable.closure.as_ref())
                .unwrap()
                .protocols,
            bc::BytecodeClosureProtocols {
                call: false,
                call_mut: false,
                call_once: false,
            }
        );
        bc::verify_bytecode(&partial).unwrap();

        let specialized = lowered(
            "fn observe[T](value: ref T) {}\n\
             fn sink[T](value: T): Never { panic(\"stop\") }\n\
             fn inspect[T](input: T): Never {\n\
                 let operation = () { observe(ref input) }\n\
                 operation()\n\
                 sink(operation)\n\
             }\n\
             fn execute(value: Join[Int, String], choose: Bool): Never {\n\
                 if choose {\n\
                     inspect(1)\n\
                 } else {\n\
                     inspect(value)\n\
                 }\n\
             }\n",
        );
        let mut specialized_rows = specialized
            .callables
            .iter()
            .filter_map(|callable| callable.closure.as_ref())
            .map(|closure| closure.protocols)
            .collect::<Vec<_>>();
        specialized_rows.sort_by_key(|protocols| protocols.call_once);
        assert_eq!(
            specialized_rows,
            vec![
                bc::BytecodeClosureProtocols {
                    call: true,
                    call_mut: true,
                    call_once: false,
                },
                bc::BytecodeClosureProtocols {
                    call: true,
                    call_mut: true,
                    call_once: true,
                },
            ]
        );
        bc::verify_bytecode(&specialized).unwrap();
    }

    #[test]
    fn bytecode_preserves_all_closure_effects_and_rederives_async_protocols() {
        let source = "fn build() {\n\
                          let sync: fn(): Int = () { 1 }\n\
                          let raw: unsafe fn(): Int = unsafe () { 2 }\n\
                          let later: async fn(): Int = async () { 3 }\n\
                          let both: async unsafe fn(): Int = async unsafe () { 4 }\n\
                          _ = sync()\n\
                          _ = sync\n\
                          _ = raw\n\
                          _ = later\n\
                          _ = both\n\
                      }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();
        let effects = program
            .callables
            .iter()
            .filter(|callable| callable.closure.is_some())
            .map(
                |callable| match &program.types[callable.function_type.index() as usize].kind {
                    bc::BytecodeTypeKind::Function(function) => {
                        (function.is_async, function.is_unsafe)
                    }
                    _ => panic!("closure callable must retain a function type"),
                },
            )
            .collect::<Vec<_>>();
        assert_eq!(
            effects,
            vec![(false, false), (false, true), (true, false), (true, true)]
        );

        let async_signature = program
            .callables
            .iter()
            .find(|callable| {
                callable.closure.is_some()
                    && matches!(
                        &program.types[callable.function_type.index() as usize].kind,
                        bc::BytecodeTypeKind::Function(function)
                            if function.is_async && !function.is_unsafe
                    )
            })
            .unwrap()
            .function_type;
        let mut forged_call = lowered(source);
        let signature = forged_call
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { signature, .. },
                            ..
                        },
                    ..
                } => Some(signature),
                _ => None,
            })
            .unwrap();
        *signature = async_signature;
        let error = bc::verify_bytecode(&forged_call).unwrap_err();
        assert!(error.message().contains("initiation context"), "{error}");

        let stateful = "fn build() {\n\
                            var count = 0\n\
                            let operation = async (): Int {\n\
                                count += 1\n\
                                count\n\
                            }\n\
                            _ = operation\n\
                        }\n";
        let program = lowered(stateful);
        let closure = program
            .callables
            .iter()
            .find_map(|callable| callable.closure.as_ref())
            .unwrap();
        assert_eq!(
            closure.protocols,
            bc::BytecodeClosureProtocols {
                call: false,
                call_mut: false,
                call_once: true,
            }
        );

        let mut forged_protocol = lowered(stateful);
        forged_protocol
            .callables
            .iter_mut()
            .find_map(|callable| callable.closure.as_mut())
            .unwrap()
            .protocols
            .call_mut = true;
        let error = bc::verify_bytecode(&forged_protocol).unwrap_err();
        assert!(error.message().contains("protocols"));

        let mut forged_parameter = lowered(
            "fn build() {\n\
                 let operation = async (value: ref Int) { () }\n\
                 _ = operation\n\
             }\n",
        );
        let callable = forged_parameter
            .callables
            .iter_mut()
            .find(|callable| callable.closure.is_some())
            .unwrap();
        callable.parameters[0].mode = bc::BytecodeParameterMode::Mut;
        let function_type = callable.function_type;
        let bc::BytecodeTypeKind::Function(function) =
            &mut forged_parameter.types[function_type.index() as usize].kind
        else {
            unreachable!()
        };
        function.parameters[0].mode = bc::BytecodeParameterMode::Mut;
        let error = bc::verify_bytecode(&forged_parameter).unwrap_err();
        assert!(error.message().contains("exclusive parameter"));
    }

    #[test]
    fn vm_entry_drives_async_bodies_but_rejects_unsafe_roots() {
        let program = lowered(
            "async fn later(): Int { 1 }\n\
             unsafe fn raw(): Int { 2 }\n",
        );
        let mut host = RejectingHost;
        let execution = execute(&program, function_id(&program, "later"), &mut host).unwrap();
        assert_eq!(
            execution.outcome,
            tondo_vm::runtime::VmOutcome::Returned(tondo_vm::runtime::RuntimeValue::Integer(1))
        );

        let mut host = RejectingHost;
        let error = execute(&program, function_id(&program, "raw"), &mut host).unwrap_err();
        assert!(matches!(error, VmError::InvalidEntry(_)), "{error}");
        assert!(error.to_string().contains("unsafe"), "{error}");
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

        let mut generic_once = lowered(
            "fn increment(value: Int): Int { value + 1 }\n\
             fn invoke[F: Copy + CallOnce[fn(Int): Int]](operation: F): Int {\n\
                 operation(41)\n\
             }\n\
             fn execute(): Int { invoke(increment) }\n",
        );
        assert_eq!(
            *indirect_call(&mut generic_once).2,
            bc::BytecodeCallProtocol::Call,
            "a closed specialization records the strongest concrete protocol"
        );
        *indirect_call(&mut generic_once).2 = bc::BytecodeCallProtocol::CallOnce;
        let error = bc::verify_bytecode(&generic_once).unwrap_err();
        assert!(error.message().contains("operation"));

        let mut opaque_mut = lowered(
            "fn make(offset: Int): impl CallMut[fn(Int): Int] + Discard {\n\
                 (value: Int): Int { value + offset }\n\
             }\n\
             fn execute(): Int {\n\
                 var operation = make(40)\n\
                 operation(2)\n\
             }\n",
        );
        assert_eq!(
            *indirect_call(&mut opaque_mut).2,
            bc::BytecodeCallProtocol::Call,
            "the sealed pure witness safely strengthens its published protocol"
        );
        *indirect_call(&mut opaque_mut).2 = bc::BytecodeCallProtocol::CallMut;
        let error = bc::verify_bytecode(&opaque_mut).unwrap_err();
        assert!(error.message().contains("operation"));
    }

    #[test]
    fn bytecode_verifier_rejects_borrows_in_value_arguments() {
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

        let mut program = lowered(
            "fn inspect(value: ref Int): Int { value }\n\
             fn execute(): Int {\n\
                 let value = 42\n\
                 inspect(ref value)\n\
             }\n",
        );
        let (function, loan) = program
            .functions
            .iter()
            .enumerate()
            .find_map(|(function, body)| {
                body.blocks
                    .iter()
                    .find_map(|block| match &block.terminator.kind {
                        bc::BytecodeTerminatorKind::Invoke {
                            operation:
                                bc::BytecodeOperation {
                                    kind: bc::BytecodeOperationKind::Call { arguments, .. },
                                    ..
                                },
                            ..
                        } => arguments.iter().find_map(|argument| {
                            if argument.mode == bc::BytecodeParameterMode::Ref {
                                match argument.value.kind {
                                    bc::BytecodeOperandKind::Loan(loan) => Some((function, loan)),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        }),
                        _ => None,
                    })
            })
            .expect("ref argument consumes an explicit loan");
        let place = program.functions[function].loans[loan.index() as usize]
            .place
            .clone();
        let argument = program.functions[function]
            .blocks
            .iter_mut()
            .find_map(|block| {
                match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => arguments.iter_mut().find(|argument| {
                    matches!(argument.value.kind, bc::BytecodeOperandKind::Loan(id) if id == loan)
                }),
                _ => None,
            }
            })
            .unwrap();
        argument.value.kind = bc::BytecodeOperandKind::Borrow(place);
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(error.message().contains("borrow escapes"));
    }

    #[test]
    fn call_local_loans_execute_shared_exclusive_reborrow_and_field_write_through() {
        let source = "type Pair = { left: Int, right: Int }\n\
                      type Inner = { value: Int }\n\
                      type Outer = { inner: Inner }\n\
                      fn inspect(value: ref Int): Int { value }\n\
                      fn nestedProjection(value: ref Outer): Int {\n\
                          inspect(ref value.inner.value)\n\
                      }\n\
                      fn nestedProjectionRun(): Int {\n\
                          nestedProjection(ref Outer { inner: Inner { value: 5 } })\n\
                      }\n\
                      fn increment(value: mut Int) {\n\
                          value += 1\n\
                      }\n\
                      fn replace(value: var Int) {\n\
                          value = 42\n\
                      }\n\
                      fn replaceField(value: mut Pair) {\n\
                          replace(var value.left)\n\
                      }\n\
                      fn nested(value: mut Int) { increment(mut value) }\n\
                      fn captured(): Int {\n\
                          var value = 1\n\
                          var operation = (): Int {\n\
                              increment(mut value)\n\
                              value\n\
                          }\n\
                          operation()\n\
                      }\n\
                      fn projected(): Int {\n\
                          var value = Pair { left: 1, right: 2 }\n\
                          replaceField(mut value)\n\
                          value.left\n\
                      }\n\
                      fn replacementValue(): Int { 7 }\n\
                      fn assign(value: mut Int, next: Int) {\n\
                          value = next\n\
                      }\n\
                      fn missing(): Int? { none }\n\
                      fn early(): Int? {\n\
                          var value = 1\n\
                          assign(mut value, missing()?)\n\
                          some(value)\n\
                      }\n\
                      fn breakArgument(): Int {\n\
                          var value = 1\n\
                          for {\n\
                              assign(mut value, {\n\
                                  break\n\
                              })\n\
                          }\n\
                          value\n\
                      }\n\
                      fn continueArgument(): Int {\n\
                          var value = 1\n\
                          var again = true\n\
                          for again {\n\
                              again = false\n\
                              assign(mut value, {\n\
                                  continue\n\
                              })\n\
                          }\n\
                          value\n\
                      }\n\
                      fn innerBreak(): Int {\n\
                          var value = 1\n\
                          assign(mut value, {\n\
                              for {\n\
                                  break\n\
                              }\n\
                              9\n\
                          })\n\
                          value\n\
                      }\n\
                      fn updateBoth(left: mut Int, right: mut Int) {\n\
                          left += 1\n\
                          right += 2\n\
                      }\n\
                      fn execute(): (Int, Int, Int, Int) {\n\
                          var value = 1\n\
                          let before = inspect(ref value)\n\
                          let temporary = inspect(ref (1 + 1))\n\
                          assign(mut value, replacementValue())\n\
                          nested(mut value)\n\
                          replace(var value)\n\
                          var pair = Pair { left: 10, right: 20 }\n\
                          updateBoth(mut pair.left, mut pair.right)\n\
                          (before, temporary, value, pair.left + pair.right)\n\
                      }\n";
        let program = lowered(source);
        assert!(program.functions.iter().any(|function| {
            !function.loans.is_empty()
                && function.blocks.iter().any(|block| {
                    block.instructions.iter().any(|instruction| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(_)
                        )
                    })
                })
        }));
        let disassembly = bc::disassemble(&program);
        assert!(disassembly.contains("loan l0:"));
        assert!(disassembly.contains("reserve_loan l0"));
        assert!(disassembly.contains("release_loan l0"));
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Tuple(vec![
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(2),
                RuntimeValue::Integer(42),
                RuntimeValue::Integer(33),
            ])
        );

        let indices = "fn update(left: mut Int, right: mut Int) {\n\
                           left += 10\n\
                           right *= 2\n\
                       }\n\
                       fn execute(): Array[Int] {\n\
                           var values = [1, 2]\n\
                           update(mut values[0], mut values[1])\n\
                           values\n\
                       }\n";
        assert_eq!(
            execute_function(indices, "execute"),
            RuntimeValue::Array(vec![RuntimeValue::Integer(11), RuntimeValue::Integer(4),])
        );

        let strided = "fn update(left: mut Array[Int], right: mut Array[Int]) {\n\
                           left += 10\n\
                           right += 20\n\
                       }\n\
                       fn execute(): Array[Int] {\n\
                           var values = [1, 2, 3, 4]\n\
                           update(mut values[::2], mut values[1::2])\n\
                           values\n\
                       }\n";
        assert_eq!(
            execute_function(strided, "execute"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(11),
                RuntimeValue::Integer(22),
                RuntimeValue::Integer(13),
                RuntimeValue::Integer(24),
            ])
        );
        assert_eq!(execute_function(source, "early"), RuntimeValue::OptionNone);
        assert_eq!(
            execute_function(source, "captured"),
            RuntimeValue::Integer(2)
        );
        assert_eq!(
            execute_function(source, "projected"),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(source, "nestedProjectionRun"),
            RuntimeValue::Integer(5)
        );
        assert_eq!(
            execute_function(source, "breakArgument"),
            RuntimeValue::Integer(1)
        );
        assert_eq!(
            execute_function(source, "continueArgument"),
            RuntimeValue::Integer(1)
        );
        assert_eq!(
            execute_function(source, "innerBreak"),
            RuntimeValue::Integer(9)
        );

        let VmOutcome::Panicked(panic) = execute_outcome(
            "fn assign(value: mut Int, next: Int) {\n\
                 value = next\n\
             }\n\
             fn explode(): Int {\n\
                 panic(\"boom\")\n\
             }\n\
             fn execute() {\n\
                 var value = 1\n\
                 assign(mut value, explode())\n\
             }\n",
            "execute",
        ) else {
            panic!("loaned call should propagate its language panic")
        };
        assert_eq!(panic.code, PanicCode::ExplicitPanic);
    }

    #[test]
    fn mut_array_root_writes_defend_fixed_extent_at_runtime() {
        let source = "fn preserve(values: mut Array[Int], replacement: Array[Int]) {\n\
                          values += 0\n\
                      }\n\
                      fn execute(): Array[Int] {\n\
                          var values = [1, 2, 3, 4]\n\
                          preserve(mut values[1:3], [9, 8, 7])\n\
                          values\n\
                      }\n";
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(2),
                RuntimeValue::Integer(3),
                RuntimeValue::Integer(4),
            ])
        );

        let mut forged = lowered(source);
        let preserve = function_id(&forged, "preserve");
        let function = &mut forged.functions[preserve.index() as usize];
        let destination = function.parameters[0];
        let replacement = function.parameters[1];
        let replacement_ty = function.slots[replacement.index() as usize].ty;
        let store = function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    destination: place,
                    value,
                } if place.slot == destination && place.projections.is_empty() => Some(value),
                _ => None,
            })
            .expect("preserve writes its root mut parameter");
        *store = bc::BytecodeRvalue {
            ty: replacement_ty,
            kind: bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                ty: replacement_ty,
                kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                    slot: replacement,
                    ty: replacement_ty,
                    projections: Vec::new(),
                    source_loan: None,
                }),
            }),
        };
        bc::verify_bytecode(&forged).unwrap();

        let entry = function_id(&forged, "execute");
        for limits in [
            VmLimits::default(),
            VmLimits {
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        ] {
            let mut host = RejectingHost;
            let error = execute_with_limits(&forged, entry, &mut host, limits).unwrap_err();
            assert!(
                matches!(
                    error,
                    VmError::Invariant(ref message)
                        if message.contains("mut Array write changed structural extent")
                ),
                "{error}"
            );
        }
    }

    #[test]
    fn collection_region_loans_validate_and_execute_disjoint_views() {
        let source = "const Split: Int = 2\n\
                      fn update(left: mut Array[Int], right: mut Array[Int]) {\n\
                          left += 10\n\
                          right *= 2\n\
                      }\n\
                      fn execute(): Array[Int] {\n\
                          var values = [1, 2, 3, 4]\n\
                          update(mut values[:Split], mut values[Split:])\n\
                          values\n\
                      }\n";
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(11),
                RuntimeValue::Integer(12),
                RuntimeValue::Integer(6),
                RuntimeValue::Integer(8),
            ])
        );

        let dynamic = "fn scale(values: mut Array[Int]) {\n\
                           values *= 3\n\
                       }\n\
                       fn execute(): Array[Int] {\n\
                           var values = [1, 2, 3, 4]\n\
                           let start = 1\n\
                           let end = 3\n\
                           scale(mut values[start:end])\n\
                           values\n\
                       }\n";
        assert_eq!(
            execute_function(dynamic, "execute"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(1),
                RuntimeValue::Integer(6),
                RuntimeValue::Integer(9),
                RuntimeValue::Integer(4),
            ])
        );

        for (source, expected) in [
            (
                "fn update(value: mut Int) {\n\
                 }\n\
                 fn explode() {\n\
                     var values = [1]\n\
                     update(mut values[2])\n\
                 }\n",
                PanicCode::Bounds,
            ),
            (
                "fn update(values: mut Array[Int]) {\n\
                 }\n\
                 fn explode() {\n\
                     var values = [1]\n\
                     update(mut values[::0])\n\
                 }\n",
                PanicCode::ZeroSliceStep,
            ),
        ] {
            let VmOutcome::Panicked(panic) = execute_outcome(source, "explode") else {
                panic!("collection loan should produce {expected:?}")
            };
            assert_eq!(panic.code, expected);
        }

        let mut forged = lowered(source);
        let function = function_id(&forged, "execute");
        let second = forged.functions[function.index() as usize]
            .loans
            .iter_mut()
            .filter(|loan| loan.kind == bc::BytecodeLoanKind::CallLocal)
            .nth(1)
            .expect("the disjoint call has two collection loans");
        let Some(bc::BytecodeProjection {
            kind: bc::BytecodeProjectionKind::Slice { start, .. },
            ..
        }) = second.place.projections.last_mut()
        else {
            panic!("the second collection loan keeps its slice projection")
        };
        *start = None;
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("runtime proof lists"), "{error}");
    }

    #[test]
    fn runtime_collection_overlap_proofs_cover_loans_reads_writes_slices_and_maps() {
        let source = "fn updateInts(left: mut Int, right: mut Int) {\n\
                          left += 10\n\
                          right *= 2\n\
                      }\n\
                      fn updateSlices(left: mut Array[Int], right: mut Array[Int]) {\n\
                          left += 10\n\
                          right *= 2\n\
                      }\n\
                      fn updateMixed(left: mut Int, right: mut Array[Int]) {}\n\
                      fn addObserved(value: mut Int, observed: Int) {\n\
                          value += observed\n\
                      }\n\
                      fn hold(value: mut Int, token: Int) {}\n\
                      fn neverRun(left: mut Int, right: mut Int) {\n\
                          panic(\"callee must not run\")\n\
                      }\n\
                      fn indexDisjoint(): Array[Int] {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 1\n\
                          updateInts(mut values[left], mut values[right])\n\
                          values\n\
                      }\n\
                      fn indexOverlap() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 0\n\
                          updateInts(mut values[left], mut values[right])\n\
                      }\n\
                      fn negativeIndexDisjoint(): Array[Int] {\n\
                          var values = [1, 2]\n\
                          let left = -1\n\
                          let right = 0\n\
                          updateInts(mut values[left], mut values[right])\n\
                          values\n\
                      }\n\
                      fn negativeIndexOverlap() {\n\
                          var values = [1, 2]\n\
                          let left = -1\n\
                          let right = 1\n\
                          updateInts(mut values[left], mut values[right])\n\
                      }\n\
                      fn overlapBeforeCallee() {\n\
                          var values = [1, 2]\n\
                          let index = 0\n\
                          neverRun(mut values[index], mut values[index])\n\
                      }\n\
                      fn laterBoundsFailure() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 9\n\
                          updateInts(mut values[left], mut values[right])\n\
                      }\n\
                      fn laterZeroStepFailure() {\n\
                          var values = [1, 2]\n\
                          let index = 0\n\
                          let step = 0\n\
                          updateMixed(mut values[index], mut values[::step])\n\
                      }\n\
                      fn sliceDisjoint(): Array[Int] {\n\
                          var values = [1, 2, 3, 4]\n\
                          let leftEnd = 2\n\
                          let rightStart = 2\n\
                          updateSlices(mut values[:leftEnd], mut values[rightStart:])\n\
                          values\n\
                      }\n\
                      fn sliceOverlap() {\n\
                          var values = [1, 2, 3, 4]\n\
                          let leftEnd = 3\n\
                          let rightStart = 2\n\
                          updateSlices(mut values[:leftEnd], mut values[rightStart:])\n\
                      }\n\
                      fn negativeStrideDisjoint(): Array[Int] {\n\
                          var values = [1, 2, 3, 4]\n\
                          let reverseStep = -2\n\
                          updateSlices(mut values[::reverseStep], mut values[0:3:2])\n\
                          values\n\
                      }\n\
                      fn negativeStrideOverlap() {\n\
                          var values = [1, 2, 3, 4]\n\
                          let reverseStep = -2\n\
                          updateSlices(mut values[::reverseStep], mut values[1::2])\n\
                      }\n\
                      fn readDisjoint(): Array[Int] {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 1\n\
                          addObserved(mut values[left], values[right])\n\
                          values\n\
                      }\n\
                      fn readOverlap() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 0\n\
                          addObserved(mut values[left], values[right])\n\
                      }\n\
                      fn writeDisjoint(): Array[Int] {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 1\n\
                          hold(mut values[left], {\n\
                              values[right] = 9\n\
                              0\n\
                          })\n\
                          values\n\
                      }\n\
                      fn writeOverlap() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 0\n\
                          hold(mut values[left], {\n\
                              values[right] = 9\n\
                              0\n\
                          })\n\
                      }\n\
                      fn mapDisjoint(): Int? {\n\
                          var values = [\"left\": 1, \"right\": 2]\n\
                          let left = \"left\"\n\
                          let right = \"right\"\n\
                          (values[left], values[right]) = (11, 4)\n\
                          values[\"left\"]\n\
                      }\n\
                      fn mapOverlap() {\n\
                          var values = [\"left\": 1, \"right\": 2]\n\
                          let left = \"left\"\n\
                          let right = \"left\"\n\
                          (values[left], values[right]) = (11, 4)\n\
                      }\n\
                      fn mapInsert(): Int? {\n\
                          var values = [\"left\": 1]\n\
                          let left = \"left\"\n\
                          let right = \"missing\"\n\
                          (values[left], values[right]) = (11, 4)\n\
                          values[\"missing\"]\n\
                      }\n";

        assert_eq!(
            execute_function(source, "indexDisjoint"),
            RuntimeValue::Array(vec![RuntimeValue::Integer(11), RuntimeValue::Integer(4)])
        );
        assert_eq!(
            execute_function(source, "sliceDisjoint"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(11),
                RuntimeValue::Integer(12),
                RuntimeValue::Integer(6),
                RuntimeValue::Integer(8),
            ])
        );
        assert_eq!(
            execute_function(source, "negativeIndexDisjoint"),
            RuntimeValue::Array(vec![RuntimeValue::Integer(2), RuntimeValue::Integer(12)])
        );
        assert_eq!(
            execute_function(source, "negativeStrideDisjoint"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(2),
                RuntimeValue::Integer(12),
                RuntimeValue::Integer(6),
                RuntimeValue::Integer(14),
            ])
        );
        assert_eq!(
            execute_function(source, "readDisjoint"),
            RuntimeValue::Array(vec![RuntimeValue::Integer(3), RuntimeValue::Integer(2)])
        );
        assert_eq!(
            execute_function(source, "writeDisjoint"),
            RuntimeValue::Array(vec![RuntimeValue::Integer(1), RuntimeValue::Integer(9)])
        );
        assert_eq!(
            execute_function(source, "mapDisjoint"),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(11)))
        );
        assert_eq!(
            execute_function(source, "mapInsert"),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(4)))
        );

        for name in [
            "indexOverlap",
            "negativeIndexOverlap",
            "sliceOverlap",
            "negativeStrideOverlap",
            "readOverlap",
            "writeOverlap",
            "mapOverlap",
            "overlapBeforeCallee",
        ] {
            let VmOutcome::Panicked(panic) = execute_outcome(source, name) else {
                panic!("{name} should reject its runtime overlap")
            };
            assert_eq!(panic.code, PanicCode::OverlappingBorrow, "{name}");
        }
        let VmOutcome::Panicked(panic) = execute_outcome(source, "laterBoundsFailure") else {
            panic!("laterBoundsFailure should preserve its bounds failure")
        };
        assert_eq!(panic.code, PanicCode::Bounds);
        let VmOutcome::Panicked(panic) = execute_outcome(source, "laterZeroStepFailure") else {
            panic!("laterZeroStepFailure should preserve its zero-step failure")
        };
        assert_eq!(panic.code, PanicCode::ZeroSliceStep);
    }

    #[test]
    fn runtime_pattern_regions_use_dynamic_access_proofs() {
        let source = "fn keep(values: ref Array[Int]) {}\n\
                      fn disjoint(): Array[Int] {\n\
                          var values = [1, 2, 3]\n\
                          let index = 0\n\
                          match values {\n\
                              [] => ()\n\
                              [_, ..ref tail] => {\n\
                                  values[index] = 9\n\
                                  keep(ref tail)\n\
                              }\n\
                          }\n\
                          values\n\
                      }\n\
                      fn overlap() {\n\
                          var values = [1, 2, 3]\n\
                          let index = 1\n\
                          match values {\n\
                              [] => ()\n\
                              [_, ..ref tail] => {\n\
                                  values[index] = 9\n\
                                  keep(ref tail)\n\
                              }\n\
                          }\n\
                      }\n";
        assert_eq!(
            execute_function(source, "disjoint"),
            RuntimeValue::Array(vec![
                RuntimeValue::Integer(9),
                RuntimeValue::Integer(2),
                RuntimeValue::Integer(3),
            ])
        );
        let VmOutcome::Panicked(panic) = execute_outcome(source, "overlap") else {
            panic!("the dynamic index should overlap the live pattern-rest region")
        };
        assert_eq!(panic.code, PanicCode::OverlappingBorrow);
    }

    #[test]
    fn bytecode_verifier_rederives_every_dynamic_collection_proof() {
        let source = "fn update(left: mut Int, right: mut Int) {}\n\
                      fn hold(value: mut Int, token: Int) {}\n\
                      fn execute() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 1\n\
                          update(mut values[left], mut values[right])\n\
                          hold(mut values[left], values[right])\n\
                          hold(mut values[left], {\n\
                              values[right] = 9\n\
                              0\n\
                          })\n\
                      }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();
        let disassembly = bc::disassemble(&program);
        assert!(
            disassembly
                .lines()
                .any(|line| line.contains("validate_loan") && !line.contains("against []"))
        );
        assert!(
            disassembly
                .lines()
                .any(|line| line.contains("validate_places") && !line.contains("against [[]]"))
        );

        let mut missing_loan_proof = program.clone();
        let against = missing_loan_proof
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidateLoan { against, .. } if !against.is_empty() => {
                    Some(against)
                }
                _ => None,
            })
            .expect("the second dynamic loan has one runtime conflict proof");
        against.clear();
        let error = bc::verify_bytecode(&missing_loan_proof).unwrap_err();
        assert!(error.message().contains("runtime proof lists"), "{error}");

        let mut missing_read_proof = program.clone();
        let against = missing_read_proof
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Index { against, .. },
                            ..
                        },
                    ..
                } if !against.is_empty() => Some(against),
                _ => None,
            })
            .expect("the later indexed read has one runtime conflict proof");
        against.clear();
        let error = bc::verify_bytecode(&missing_read_proof).unwrap_err();
        assert!(
            error.message().contains("indexed operation runtime proof"),
            "{error}"
        );

        let mut missing_write_proof = program.clone();
        let against = missing_write_proof
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidatePlaces { against, .. }
                    if against.iter().any(|loans| !loans.is_empty()) =>
                {
                    Some(against)
                }
                _ => None,
            })
            .expect("the later indexed write has one runtime conflict proof");
        against.iter_mut().for_each(Vec::clear);
        let error = bc::verify_bytecode(&missing_write_proof).unwrap_err();
        assert!(
            error.message().contains("place validation runtime proof"),
            "{error}"
        );

        let mut detached_validation = program;
        let function = function_id(&detached_validation, "execute");
        let target = detached_validation.functions[function.index() as usize]
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::ValidateLoan { target, .. } => Some(*target),
                _ => None,
            })
            .expect("dynamic loan has an explicit validation target");
        let first = detached_validation.functions[function.index() as usize].blocks
            [target.index() as usize]
            .instructions
            .remove(0);
        assert!(matches!(
            first.kind,
            bc::BytecodeInstructionKind::ReserveLoan(_)
        ));
        let error = bc::verify_bytecode(&detached_validation).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminator edge or block kind is invalid"),
            "{error}"
        );
    }

    #[test]
    fn bytecode_verifier_requires_one_reservation_per_call_local_loan() {
        let source = "fn inspect(value: ref Int): Int { value }\n\
                      fn execute(): Int {\n\
                          let value = 42\n\
                          inspect(ref value)\n\
                      }\n";
        let mut program = lowered(source);
        let function = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &mut program.functions[function.index() as usize];
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .enumerate()
                    .find_map(|(index, instruction)| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(_)
                        )
                        .then_some((block, index))
                    })
            })
            .unwrap();
        let duplicate = function.blocks[block].instructions[index].clone();
        function.blocks[block]
            .instructions
            .insert(index + 1, duplicate);
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(error.message().contains("reservations"), "{error}");

        let mut program = lowered(source);
        let function = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &mut program.functions[function.index() as usize];
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .position(|instruction| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(_)
                        )
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let reservation = function.blocks[block].instructions.remove(index);
        function.blocks[function.unwind.index() as usize]
            .instructions
            .push(reservation);
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error
                .message()
                .contains("cleanup block manipulates a loan reservation"),
            "{error}"
        );

        let mut program = lowered(source);
        let function = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &mut program.functions[function.index() as usize];
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .position(|instruction| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(_)
                        )
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let mut release = function.blocks[block].instructions[index].clone();
        release.kind = bc::BytecodeInstructionKind::ReleaseLoan(bc::BytecodeLoanId::new(0));
        function.blocks[block].instructions.insert(index, release);
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error.message().contains("releases inactive loan"),
            "{error}"
        );

        let mut program = lowered(source);
        let function = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &mut program.functions[function.index() as usize];
        let loan_slot = function.loans[0].place.slot;
        let write = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find(|instruction| {
                matches!(
                    &instruction.kind,
                    bc::BytecodeInstructionKind::Store { destination, .. }
                        if destination.slot == loan_slot
                )
            })
            .cloned()
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .enumerate()
                    .find_map(|(index, instruction)| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(_)
                        )
                        .then_some((block, index))
                    })
            })
            .unwrap();
        function.blocks[block].instructions.insert(index + 1, write);
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error.message().contains("write overlaps active loan"),
            "{error}"
        );

        let mut program = lowered(
            "fn inspect(value: ref Bool): Bool { value }\n\
             fn execute(): Bool {\n\
                 let value = true\n\
                 if inspect(ref value) { true } else { false }\n\
             }\n",
        );
        let function = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let condition = program.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::BranchBool { condition, .. } => Some(condition),
                _ => None,
            })
            .unwrap();
        condition.kind = bc::BytecodeOperandKind::Loan(bc::BytecodeLoanId::new(0));
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(error.message().contains("terminator"), "{error}");
    }

    #[test]
    fn borrow_pattern_regions_survive_bytecode_lowering_and_runtime_checks() {
        let source = "type Pair = { left: Int, right: Int }\n\
                      fn inspect(value: ref Int) {}\n\
                      fn abandonBorrow(value: ref Int, after: Unit) {}\n\
                      fn abandon(pair: var Pair) {\n\
                          match pair {\n\
                              Pair { ref left, right: _ } => {\n\
                                  abandonBorrow(ref left, {\n\
                                      return\n\
                                  })\n\
                              }\n\
                          }\n\
                      }\n\
                      fn execute(): Int {\n\
                          var pair = Pair { left: 1, right: 2 }\n\
                          match pair {\n\
                              Pair { ref left, right: _ } => {\n\
                                  if pair.right == 2 {\n\
                                      match left {\n\
                                          ref nested => inspect(ref nested)\n\
                                      }\n\
                                  }\n\
                                  pair.left = 7\n\
                              }\n\
                          }\n\
                          pair.left\n\
                      }\n\
                      fn update(value: mut Int) {\n\
                          value += 1\n\
                      }\n\
                      fn inspectArray(values: ref Array[Int]) {}\n\
                      fn executeArrayPrefix(): Int {\n\
                          var values = [1, 2, 3]\n\
                          match values {\n\
                              [] => 0\n\
                              [ref first, ..] => {\n\
                                  update(mut values[1])\n\
                                  inspect(ref first)\n\
                                  values[0] + values[1]\n\
                              }\n\
                          }\n\
                      }\n\
                      fn executeArrayRest(): Int {\n\
                          var values = [1, 2, 3]\n\
                          match values {\n\
                              [] => 0\n\
                              [first, ..ref tail] => {\n\
                                  update(mut values[0])\n\
                                  inspectArray(ref tail)\n\
                                  values[0] + values[1]\n\
                              }\n\
                          }\n\
                      }\n";
        let program = lowered(source);
        let function_id = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &program.functions[function_id.index() as usize];
        let regions = function
            .loans
            .iter()
            .enumerate()
            .filter(|(_, loan)| loan.kind == bc::BytecodeLoanKind::Region)
            .map(|(index, loan)| (bc::BytecodeLoanId::new(index as u32), loan))
            .collect::<Vec<_>>();
        assert_eq!(regions.len(), 2);
        let (region, region_loan) = regions[0];
        let (nested_region, nested_loan) = regions[1];
        assert_eq!(region_loan.mode, bc::BytecodeParameterMode::Ref);
        assert_eq!(nested_loan.place.source_loan, Some(region));
        assert!(function.loans.iter().any(|loan| {
            loan.kind == bc::BytecodeLoanKind::CallLocal
                && loan.place.source_loan == Some(nested_region)
        }));
        assert!(function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::ReleaseLoan(id) if id == region
                )
            })
        }));
        let abandon = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::abandon"))
            .and_then(|callable| callable.implementation)
            .map(|function| &program.functions[function.index() as usize])
            .unwrap();
        let abandon_region = abandon
            .loans
            .iter()
            .position(|loan| loan.kind == bc::BytecodeLoanKind::Region)
            .map(|index| bc::BytecodeLoanId::new(index as u32))
            .unwrap();
        let abandoned_call = abandon
            .loans
            .iter()
            .position(|loan| {
                loan.kind == bc::BytecodeLoanKind::CallLocal
                    && loan.place.source_loan == Some(abandon_region)
            })
            .map(|index| bc::BytecodeLoanId::new(index as u32))
            .unwrap();
        assert!(abandon.blocks.iter().any(|block| {
            block.instructions.windows(2).any(|instructions| {
                matches!(
                    instructions[0].kind,
                    bc::BytecodeInstructionKind::ReleaseLoan(id) if id == abandoned_call
                ) && matches!(
                    instructions[1].kind,
                    bc::BytecodeInstructionKind::ReleaseLoan(id) if id == abandon_region
                )
            })
        }));
        let disassembly = bc::disassemble(&program);
        assert!(disassembly.contains("Region Ref"));
        assert!(disassembly.contains(&format!("@l{}", region.index())));
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Integer(7)
        );
        assert_eq!(
            execute_function(source, "executeArrayPrefix"),
            RuntimeValue::Integer(4)
        );
        assert_eq!(
            execute_function(source, "executeArrayRest"),
            RuntimeValue::Integer(4)
        );

        let mut forged = lowered(source);
        let function_id = forged
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::execute"))
            .and_then(|callable| callable.implementation)
            .unwrap();
        let function = &mut forged.functions[function_id.index() as usize];
        let region = function
            .loans
            .iter()
            .position(|loan| loan.kind == bc::BytecodeLoanKind::Region)
            .map(|index| bc::BytecodeLoanId::new(index as u32))
            .unwrap();
        let nested_region = function
            .loans
            .iter()
            .enumerate()
            .find(|(_, loan)| {
                loan.kind == bc::BytecodeLoanKind::Region && loan.place.source_loan == Some(region)
            })
            .map(|(index, _)| bc::BytecodeLoanId::new(index as u32))
            .unwrap();
        for block in &mut function.blocks {
            block.instructions.retain(|instruction| {
                !matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::ReleaseLoan(id) if id == region
                )
            });
        }
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .position(|instruction| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::ReserveLoan(id) if id == nested_region
                        )
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let span = function.blocks[block].instructions[index].span;
        function.blocks[block].instructions.insert(
            index + 1,
            bc::BytecodeInstruction {
                span,
                kind: bc::BytecodeInstructionKind::ReleaseLoan(region),
            },
        );
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("dependent loan"), "{error}");
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
    fn verifier_rejects_malformed_or_out_of_range_immediate_integers() {
        let mut program = lowered("fn value(): Int8 { 1i8 }\n");
        fn integer_spelling(program: &mut bc::BytecodeProgram) -> &mut String {
            program
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.instructions)
                .find_map(|instruction| match &mut instruction.kind {
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                        kind:
                                            bc::BytecodeOperandKind::Constant(
                                                bc::BytecodeConstant::Integer(spelling),
                                            ),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } => Some(spelling),
                    _ => None,
                })
                .unwrap()
        }
        *integer_spelling(&mut program) = "128i8".into();
        assert!(bc::verify_bytecode(&program).is_err());

        *integer_spelling(&mut program) = "not-an-integer".into();
        assert!(bc::verify_bytecode(&program).is_err());
    }

    #[test]
    fn verifier_rejects_malformed_overflowing_or_mistyped_immediate_floats() {
        let mut program = lowered("fn value(): Float32 { 1.0f32 }\n");
        fn float_spelling(program: &mut bc::BytecodeProgram) -> &mut String {
            program
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.instructions)
                .find_map(|instruction| match &mut instruction.kind {
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                        kind:
                                            bc::BytecodeOperandKind::Constant(
                                                bc::BytecodeConstant::Float(spelling),
                                            ),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } => Some(spelling),
                    _ => None,
                })
                .unwrap()
        }
        *float_spelling(&mut program) = "3.4028236e38f32".into();
        assert!(bc::verify_bytecode(&program).is_err());

        *float_spelling(&mut program) = "1.0f64".into();
        assert!(bc::verify_bytecode(&program).is_err());

        *float_spelling(&mut program) = "not-a-float".into();
        assert!(bc::verify_bytecode(&program).is_err());
    }

    #[test]
    fn verifier_rejects_malformed_immediate_text_literals() {
        let mut string = lowered("fn value(): String { \"valid\" }\n");
        let spelling =
            string
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.instructions)
                .find_map(|instruction| match &mut instruction.kind {
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                        kind:
                                            bc::BytecodeOperandKind::Constant(
                                                bc::BytecodeConstant::String(spelling),
                                            ),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } => Some(spelling),
                    _ => None,
                })
                .unwrap();
        *spelling = "\"unterminated".into();
        assert!(bc::verify_bytecode(&string).is_err());

        let mut character = lowered("fn value(): Char { 'x' }\n");
        let spelling =
            character
                .functions
                .iter_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.instructions)
                .find_map(|instruction| match &mut instruction.kind {
                    bc::BytecodeInstructionKind::Store {
                        value:
                            bc::BytecodeRvalue {
                                kind:
                                    bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                        kind:
                                            bc::BytecodeOperandKind::Constant(
                                                bc::BytecodeConstant::Char(spelling),
                                            ),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } => Some(spelling),
                    _ => None,
                })
                .unwrap();
        *spelling = "'\\u{d800}'".into();
        assert!(bc::verify_bytecode(&character).is_err());
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
    fn bytecode_verifier_rejects_forged_variadic_associations_and_affine_copy() {
        let source = "fn collect(values: ...Int): Array[Int] { values }\n\
                      fn execute(): Array[Int] { collect(20, 22) }\n";
        let program = lowered(source);
        bc::verify_bytecode(&program).unwrap();

        let mut forged = program;
        let argument = forged
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
                } => arguments.iter_mut().find(|argument| {
                    argument.target == bc::BytecodeCallArgumentTarget::VariadicElement
                }),
                _ => None,
            })
            .expect("the call must retain one variadic-element association");
        argument.target = bc::BytecodeCallArgumentTarget::Fixed(0);

        assert!(bc::verify_bytecode(&forged).is_err());

        let spread = lowered(
            "fn collect(values: ...Int): Array[Int] { values }\n\
             fn execute(): Array[Int] {\n\
                 let values = [20, 22]\n\
                 collect(...values)\n\
             }\n",
        );
        let mut forged_spread = spread;
        let argument = forged_spread
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
                } => arguments.iter_mut().find(|argument| {
                    argument.target == bc::BytecodeCallArgumentTarget::VariadicSpread
                }),
                _ => None,
            })
            .expect("the call must retain its spread association");
        assert!(matches!(
            argument.value.kind,
            bc::BytecodeOperandKind::Copy(_)
        ));
        argument.target = bc::BytecodeCallArgumentTarget::VariadicElement;
        assert!(bc::verify_bytecode(&forged_spread).is_err());

        let affine = lowered(
            "fn make(value: Int): impl CallOnce[fn(): Int] + Discard {\n\
                 () { value }\n\
             }\n\
             fn runAll[F: CallOnce[fn(): Int] + Discard](operations: ...F): Int {\n\
                 var total = 0\n\
                 for operation in operations {\n\
                     total += operation()\n\
                 }\n\
                 total\n\
             }\n\
             fn execute(): Int {\n\
                 let operations = [make(42)]\n\
                 runAll(...operations)\n\
             }\n",
        );
        let mut forged_affine = affine;
        let argument = forged_affine
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
                } => arguments.iter_mut().find(|argument| {
                    argument.target == bc::BytecodeCallArgumentTarget::VariadicSpread
                }),
                _ => None,
            })
            .expect("the affine call must retain its spread association");
        let bc::BytecodeOperandKind::Move(place) = &argument.value.kind else {
            panic!("an affine Array spread must move its complete owner")
        };
        argument.value.kind = bc::BytecodeOperandKind::Copy(place.clone());
        assert!(bc::verify_bytecode(&forged_affine).is_err());
    }

    #[test]
    fn intrinsic_cursor_state_is_explicit_verified_and_logically_copyable() {
        const SOURCE: &str = "fn sum(): Int {\n\
             var total = 0\n\
             for value in [1, 2, 3, 4] {\n\
                 total += value\n\
             }\n\
             total\n\
         }\n";

        let mut forged = lowered(SOURCE);
        let entry = function_id(&forged, "sum");
        let (block_index, instruction_index, state, collection) = {
            let function = forged.function(entry).unwrap();
            function
                .blocks
                .iter()
                .enumerate()
                .find_map(|(block_index, block)| {
                    block.instructions.iter().enumerate().find_map(
                        |(instruction_index, instruction)| {
                            let bc::BytecodeInstructionKind::Store {
                                destination,
                                value:
                                    bc::BytecodeRvalue {
                                        kind: bc::BytecodeRvalueKind::IteratorState(source),
                                        ..
                                    },
                            } = &instruction.kind
                            else {
                                return None;
                            };
                            Some((
                                block_index,
                                instruction_index,
                                destination.clone(),
                                source.ty,
                            ))
                        },
                    )
                })
                .expect("the intrinsic loop constructs one cursor state")
        };
        assert!(matches!(
            forged.types[state.ty.index() as usize].kind,
            bc::BytecodeTypeKind::Cursor {
                mode: bc::BytecodeCursorMode::Own,
                collection: actual,
            } if actual == collection
        ));

        {
            let function = &mut forged.functions[entry.index() as usize];
            function.slots[state.slot.index() as usize].ty = collection;
            let bc::BytecodeInstructionKind::Store { destination, value } =
                &mut function.blocks[block_index].instructions[instruction_index].kind
            else {
                unreachable!()
            };
            destination.ty = collection;
            value.ty = collection;
        }
        let error = bc::verify_bytecode(&forged).unwrap_err();
        assert!(error.message().contains("rvalue"), "{error}");

        let mut copyable = lowered(SOURCE);
        let entry = function_id(&copyable, "sum");
        let baseline_allocations = {
            let mut host = RejectingHost;
            execute(&copyable, entry, &mut host)
                .unwrap()
                .statistics
                .allocations
        };
        let (block_index, instruction_index, span, state) = {
            let function = copyable.function(entry).unwrap();
            function
                .blocks
                .iter()
                .enumerate()
                .find_map(|(block_index, block)| {
                    block.instructions.iter().enumerate().find_map(
                        |(instruction_index, instruction)| {
                            let bc::BytecodeInstructionKind::Store {
                                destination,
                                value:
                                    bc::BytecodeRvalue {
                                        kind: bc::BytecodeRvalueKind::IteratorState(_),
                                        ..
                                    },
                            } = &instruction.kind
                            else {
                                return None;
                            };
                            Some((
                                block_index,
                                instruction_index,
                                instruction.span,
                                destination.clone(),
                            ))
                        },
                    )
                })
                .expect("the intrinsic loop constructs one cursor state")
        };
        let function = &mut copyable.functions[entry.index() as usize];
        let duplicate = bc::BytecodeSlotId::new(function.slots.len() as u32);
        function.slots.push(bc::BytecodeSlot {
            ty: state.ty,
            span: function.slots[state.slot.index() as usize].span,
            kind: bc::BytecodeSlotKind::Temporary,
        });
        let instructions = &mut function.blocks[block_index].instructions;
        instructions.splice(
            instruction_index + 1..instruction_index + 1,
            [
                bc::BytecodeInstruction {
                    span,
                    kind: bc::BytecodeInstructionKind::StorageLive(duplicate),
                },
                bc::BytecodeInstruction {
                    span,
                    kind: bc::BytecodeInstructionKind::Store {
                        destination: bc::BytecodePlace {
                            slot: duplicate,
                            ty: state.ty,
                            projections: Vec::new(),
                            source_loan: None,
                        },
                        value: bc::BytecodeRvalue {
                            ty: state.ty,
                            kind: bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                ty: state.ty,
                                kind: bc::BytecodeOperandKind::Copy(state.clone()),
                            }),
                        },
                    },
                },
                bc::BytecodeInstruction {
                    span,
                    kind: bc::BytecodeInstructionKind::StorageDead(duplicate),
                },
            ],
        );
        bc::verify_bytecode(&copyable).unwrap();
        let mut host = RejectingHost;
        let execution = execute(&copyable, entry, &mut host).unwrap();
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(10))
        );
        assert_eq!(
            execution.statistics.allocations,
            baseline_allocations + 2,
            "copying an owning cursor must allocate a logical source wrapper and a new cursor"
        );

        let mut wrong_borrow_access = copyable.clone();
        let bc::BytecodeTypeKind::Cursor { mode, .. } =
            &mut wrong_borrow_access.types[state.ty.index() as usize].kind
        else {
            unreachable!()
        };
        *mode = bc::BytecodeCursorMode::Ref;
        let error = bc::verify_bytecode(&wrong_borrow_access).unwrap_err();
        assert!(error.message().contains("rvalue"), "{error}");

        let mut borrowed_forgery = wrong_borrow_access;
        let source = borrowed_forgery.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| {
                let bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::IteratorState(source),
                            ..
                        },
                    ..
                } = &mut instruction.kind
                else {
                    return None;
                };
                Some(source)
            })
            .expect("the intrinsic loop retains its cursor source");
        let bc::BytecodeOperandKind::Copy(source_place) = source.kind.clone() else {
            panic!("the Copy bootstrap should read the iterable temporary")
        };
        source.kind = bc::BytecodeOperandKind::Borrow(source_place);
        let error = bc::verify_bytecode(&borrowed_forgery).unwrap_err();
        assert!(error.message().contains("terminator"), "{error}");
    }

    #[test]
    fn borrowed_intrinsic_iteration_executes_without_consuming_collection_elements() {
        let array = execute_function(
            "fn read(value: ref Int): Int { value }\n\
             fn arraySum(): Int {\n\
                 let values = [1, 2, 3, 4]\n\
                 var total = 0\n\
                 for ref value in values {\n\
                     if value == 2 {\n\
                         continue\n\
                     }\n\
                     total += read(ref value)\n\
                     if value == 3 {\n\
                         break\n\
                     }\n\
                 }\n\
                 total + values[0]\n\
             }\n",
            "arraySum",
        );
        assert_eq!(array, RuntimeValue::Integer(5));

        let mutation_after = execute_function(
            "fn mutationAfter(): Int {\n\
                 var values = [1, 2]\n\
                 for ref value in values {\n\
                     _ = value\n\
                 }\n\
                 values[0] = 9\n\
                 values[0]\n\
             }\n",
            "mutationAfter",
        );
        assert_eq!(mutation_after, RuntimeValue::Integer(9));

        let reborrow = execute_function(
            "fn sum(values: ref Array[Int]): Int {\n\
                 var total = 0\n\
                 for ref value in values {\n\
                     total += value\n\
                 }\n\
                 total\n\
             }\n\
             fn reborrow(): Int {\n\
                 let values = [4, 5, 6]\n\
                 sum(ref values) + values[0]\n\
             }\n",
            "reborrow",
        );
        assert_eq!(reborrow, RuntimeValue::Integer(19));

        let return_from_loop = execute_function(
            "fn first(values: ref Array[Int]): Int {\n\
                 for ref value in values {\n\
                     return value\n\
                 }\n\
                 0\n\
             }\n\
             fn returnFromLoop(): Int {\n\
                 let values = [7, 8]\n\
                 first(ref values) + values[1]\n\
             }\n",
            "returnFromLoop",
        );
        assert_eq!(return_from_loop, RuntimeValue::Integer(15));

        let map = execute_function(
            "fn mapSum(): Int {\n\
                 let entries = [\"one\": 1, \"two\": 2]\n\
                 var total = 0\n\
                 for (ref key, ref value) in entries {\n\
                     _ = key\n\
                     total += value\n\
                 }\n\
                 if \"one\" in entries { total } else { 0 }\n\
             }\n",
            "mapSum",
        );
        assert_eq!(map, RuntimeValue::Integer(3));

        let mixed = execute_function(
            "fn mixedMapSum(): Int {\n\
                 let entries = [\"one\": 1, \"two\": 2]\n\
                 var total = 0\n\
                 for (ref key, value) in entries {\n\
                     _ = key\n\
                     total += value\n\
                 }\n\
                 total\n\
             }\n",
            "mixedMapSum",
        );
        assert_eq!(mixed, RuntimeValue::Integer(3));

        let nested = execute_function(
            "fn nestedSum(): Int {\n\
                 let groups = [[1, 2], [3, 4]]\n\
                 var total = 0\n\
                 for ref value in groups[0] {\n\
                     total += value\n\
                 }\n\
                 total + groups[0][0]\n\
             }\n",
            "nestedSum",
        );
        assert_eq!(nested, RuntimeValue::Integer(4));

        let frozen_source = execute_function(
            "fn frozenSource(): Int {\n\
                 let groups = [[1, 2], [10, 20]]\n\
                 var selected = 0\n\
                 var total = 0\n\
                 for ref value in groups[selected] {\n\
                     total += value\n\
                     selected = 1\n\
                 }\n\
                 total\n\
             }\n",
            "frozenSource",
        );
        assert_eq!(frozen_source, RuntimeValue::Integer(3));

        let set = execute_function(
            "fn setSum(): Int {\n\
                 let values = Set[1, 2, 3]\n\
                 var total = 0\n\
                 for ref value in values {\n\
                     total += value\n\
                 }\n\
                 if 1 in values { total } else { 0 }\n\
             }\n",
            "setSum",
        );
        assert_eq!(set, RuntimeValue::Integer(6));
    }

    #[test]
    fn exclusive_intrinsic_iteration_is_verified_and_executes_write_through() {
        const SOURCE: &str = "fn edit(\n\
             values: var Array[Int],\n\
             groups: var Array[Array[Int]],\n\
         ) {\n\
             for mut value in values {\n\
                 value += 1\n\
             }\n\
             for var group in groups {\n\
                 group = [9]\n\
             }\n\
         }\n";
        let program = lowered(SOURCE);
        bc::verify_bytecode(&program).unwrap();
        let entry = function_id(&program, "edit");
        let function = &program.functions[entry.index() as usize];
        let root_loans = function
            .blocks
            .iter()
            .filter_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    state,
                    borrowed_source: Some(source),
                    ..
                } => {
                    assert!(matches!(
                        program.types[state.ty.index() as usize].kind,
                        bc::BytecodeTypeKind::Cursor {
                            mode: bc::BytecodeCursorMode::Mut,
                            ..
                        }
                    ));
                    source.source_loan
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(root_loans.len(), 2);
        for loan in root_loans {
            assert_eq!(
                function.loans[loan.index() as usize].mode,
                bc::BytecodeParameterMode::Mut
            );
        }
        let element_modes = function
            .loans
            .iter()
            .filter(|loan| {
                loan.place.projections.iter().any(|projection| {
                    matches!(
                        projection.kind,
                        bc::BytecodeProjectionKind::IteratorElement { .. }
                    )
                })
            })
            .map(|loan| loan.mode)
            .collect::<BTreeSet<_>>();
        assert!(element_modes.contains(&bc::BytecodeParameterMode::Mut));
        assert!(element_modes.contains(&bc::BytecodeParameterMode::Var));

        let value = execute_function(
            "fn edit(): Int {\n\
                 var values = [1, 2]\n\
                 for mut value in values {\n\
                     value += 1\n\
                 }\n\
                 var groups = [1: [1], 2: [2]]\n\
                 for (_, var group) in groups {\n\
                     group = [7, 8]\n\
                 }\n\
                 assert(values == [2, 3])\n\
                 assert(groups == [2: [7, 8], 1: [7, 8]])\n\
                 values[0]\n\
             }\n",
            "edit",
        );
        assert_eq!(value, RuntimeValue::Integer(2));

        let mut mutable_key = lowered(
            "fn edit(entries: var Map[Int, Int]) {\n\
                 for (ref key, mut value) in entries {\n\
                     value += key\n\
                 }\n\
             }\n",
        );
        let map_entry = function_id(&mutable_key, "edit");
        let field = mutable_key.functions[map_entry.index() as usize]
            .loans
            .iter_mut()
            .find(|loan| {
                loan.mode == bc::BytecodeParameterMode::Mut
                    && loan.place.projections.iter().any(|projection| {
                        matches!(
                            projection.kind,
                            bc::BytecodeProjectionKind::IteratorElement { .. }
                        )
                    })
            })
            .and_then(|loan| {
                loan.place.projections.iter_mut().find(|projection| {
                    matches!(projection.kind, bc::BytecodeProjectionKind::TupleField(1))
                })
            })
            .expect("the map value loan projects tuple field 1");
        field.kind = bc::BytecodeProjectionKind::TupleField(0);
        let error = bc::verify_bytecode(&mutable_key).unwrap_err();
        assert!(
            error
                .message()
                .contains("does not project through its value"),
            "{error}"
        );

        let mut wrong_root = program;
        let function = &mut wrong_root.functions[entry.index() as usize];
        let root = function
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    borrowed_source: Some(source),
                    ..
                } => source.source_loan,
                _ => None,
            })
            .unwrap();
        function.loans[root.index() as usize].mode = bc::BytecodeParameterMode::Ref;
        let error = bc::verify_bytecode(&wrong_root).unwrap_err();
        assert!(
            error.message().contains("exact region loan")
                || error.message().contains("terminator edge"),
            "{error}"
        );
    }

    #[test]
    fn bytecode_rederives_borrowed_iterator_origins_and_boundary_loans() {
        const SOURCE: &str = "fn observe(values: ref Array[Int]) {\n\
             let marker = 0\n\
             for ref value in values {\n\
                 _ = value + marker\n\
             }\n\
         }\n";
        let program = lowered(SOURCE);
        bc::verify_bytecode(&program).unwrap();
        let entry = function_id(&program, "observe");

        let mut missing_source = program.clone();
        let source = missing_source.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    borrowed_source, ..
                } => Some(borrowed_source),
                _ => None,
            })
            .unwrap();
        *source = None;
        let error = bc::verify_bytecode(&missing_source).unwrap_err();
        assert!(
            error.message().contains("projection") || error.message().contains("terminator"),
            "{error}"
        );

        let mut forged_position = program.clone();
        let function = &mut forged_position.functions[entry.index() as usize];
        let marker = function
            .slots
            .iter()
            .enumerate()
            .find_map(|(index, slot)| {
                matches!(slot.kind, bc::BytecodeSlotKind::User { .. })
                    .then_some(bc::BytecodeSlotId::new(index as u32))
            })
            .unwrap();
        let projection = function
            .loans
            .iter_mut()
            .flat_map(|loan| &mut loan.place.projections)
            .find(|projection| {
                matches!(
                    projection.kind,
                    bc::BytecodeProjectionKind::IteratorElement { .. }
                )
            })
            .unwrap();
        projection.kind = bc::BytecodeProjectionKind::IteratorElement { index: marker };
        let error = bc::verify_bytecode(&forged_position).unwrap_err();
        assert!(error.message().contains("projection"), "{error}");

        let mut overwritten_position = program.clone();
        let function = &mut overwritten_position.functions[entry.index() as usize];
        let position = function
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext { destination, .. } => {
                    Some(destination.clone())
                }
                _ => None,
            })
            .unwrap();
        let span = function.blocks[function.entry.index() as usize]
            .terminator
            .span;
        function.blocks[function.entry.index() as usize]
            .instructions
            .push(bc::BytecodeInstruction {
                span,
                kind: bc::BytecodeInstructionKind::Store {
                    destination: position.clone(),
                    value: bc::BytecodeRvalue {
                        ty: position.ty,
                        kind: bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                            ty: position.ty,
                            kind: bc::BytecodeOperandKind::Constant(bc::BytecodeConstant::Integer(
                                "0".into(),
                            )),
                        }),
                    },
                },
            });
        let error = bc::verify_bytecode(&overwritten_position).unwrap_err();
        assert!(error.message().contains("terminator"), "{error}");

        let mut crossing_call_loan = program.clone();
        let function = &mut crossing_call_loan.functions[entry.index() as usize];
        let place = function
            .loans
            .iter()
            .find(|loan| loan.kind == bc::BytecodeLoanKind::Region)
            .unwrap()
            .place
            .clone();
        let loan = bc::BytecodeLoanId::new(function.loans.len() as u32);
        function.loans.push(bc::BytecodeLoan {
            kind: bc::BytecodeLoanKind::CallLocal,
            mode: bc::BytecodeParameterMode::Ref,
            place,
        });
        let span = function.blocks[function.entry.index() as usize]
            .terminator
            .span;
        function.blocks[function.entry.index() as usize]
            .instructions
            .push(bc::BytecodeInstruction {
                span,
                kind: bc::BytecodeInstructionKind::ReserveLoan(loan),
            });
        let error = bc::verify_bytecode(&crossing_call_loan).unwrap_err();
        assert!(error.message().contains("iterator source chain"), "{error}");

        let mut crossing_unrelated_exclusive = program;
        let function = &mut crossing_unrelated_exclusive.functions[entry.index() as usize];
        let (slot, ty) = function
            .slots
            .iter()
            .enumerate()
            .find_map(|(index, slot)| {
                matches!(slot.kind, bc::BytecodeSlotKind::User { .. })
                    .then_some((bc::BytecodeSlotId::new(index as u32), slot.ty))
            })
            .expect("the marker has one user slot");
        let loan = bc::BytecodeLoanId::new(function.loans.len() as u32);
        function.loans.push(bc::BytecodeLoan {
            kind: bc::BytecodeLoanKind::Region,
            mode: bc::BytecodeParameterMode::Mut,
            place: bc::BytecodePlace {
                slot,
                ty,
                projections: Vec::new(),
                source_loan: None,
            },
        });
        let block = function
            .blocks
            .iter_mut()
            .find(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::IteratorNext { .. }
                )
            })
            .expect("the loop has one iterator boundary");
        block.instructions.push(bc::BytecodeInstruction {
            span: block.terminator.span,
            kind: bc::BytecodeInstructionKind::ReserveLoan(loan),
        });
        let error = bc::verify_bytecode(&crossing_unrelated_exclusive).unwrap_err();
        assert!(error.message().contains("iterator source chain"), "{error}");
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
                source_loan: None,
            }),
        };
        *right = bc::BytecodeOperand {
            ty: function_type,
            kind: bc::BytecodeOperandKind::Copy(bc::BytecodePlace {
                slot: right_slot,
                ty: function_type,
                projections: Vec::new(),
                source_loan: None,
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
    fn lifted_array_arithmetic_preserves_types_preflights_shape_and_survives_gc() {
        let source = "fn calculate(): (Array[Float], Array[Float32], Array[Array[Int]]) {\n\
                          let wide = 10.0 - [1.5, 2.0]\n\
                          let narrow = 4.0f32 / [1.0f32, 2.0f32]\n\
                          let nested = [[1, 2], [3, 4]] + [10, 20]\n\
                          (wide, narrow, nested)\n\
                      }\n";
        assert_eq!(
            execute_function(source, "calculate"),
            RuntimeValue::Tuple(vec![
                RuntimeValue::Array(vec![RuntimeValue::Float(8.5), RuntimeValue::Float(8.0),]),
                RuntimeValue::Array(vec![RuntimeValue::Float(4.0), RuntimeValue::Float(2.0),]),
                RuntimeValue::Array(vec![
                    RuntimeValue::Array(
                        vec![RuntimeValue::Integer(11), RuntimeValue::Integer(12),]
                    ),
                    RuntimeValue::Array(
                        vec![RuntimeValue::Integer(23), RuntimeValue::Integer(24),]
                    ),
                ]),
            ])
        );

        let VmOutcome::Panicked(panic) = execute_outcome(
            "fn explode(): Array[Array[Int]] {\n\
                 let left = [[9223372036854775807, 0], [1]]\n\
                 let right = [[1, 0], [1, 2]]\n\
                 left + right\n\
             }\n",
            "explode",
        ) else {
            panic!("a deep shape mismatch must be detected before leaf arithmetic")
        };
        assert_eq!(panic.code, PanicCode::ArrayShapeMismatch);

        let program = lowered(source);
        let entry = function_id(&program, "calculate");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 256,
                max_heap_bytes: 128 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert!(matches!(execution.outcome, VmOutcome::Returned(_)));
        assert!(execution.statistics.collections > 0);
    }

    #[test]
    fn array_sequences_copy_logically_preserve_identity_and_survive_gc() {
        let source = "fn repeatBorrowed(\n\
                 values: ref Array[Array[Int]],\n\
                 count: Int,\n\
             ): Array[Array[Int]] {\n\
                 values.repeat(count)\n\
             }\n\
\n\
             fn sequences(): (\n\
                 Array[Array[Int]],\n\
                 Array[Array[Int]],\n\
                 Array[Array[Int]],\n\
                 Array[Array[Int]],\n\
                 Array[Int],\n\
             ) {\n\
                 var left = [[1], [2]]\n\
                 var right = [[3]]\n\
                 var joined = left.concat(right)\n\
                 joined[0][0] = 9\n\
                 var repeated = right.repeat(2)\n\
                 repeated[0][0] = 7\n\
                 assert(left == [[1], [2]])\n\
                 assert(right == [[3]])\n\
                 assert(joined == [[9], [2], [3]])\n\
                 assert(repeated == [[7], [3]])\n\
\n\
                 let qualified = Array.concat(left, right)\n\
                 assert(qualified == [[1], [2], [3]])\n\
                 assert(Array.repeat(right, 0) == [])\n\
                 assert(repeatBorrowed(ref right, 2) == [[3], [3]])\n\
\n\
                 let marker = Ref(42)\n\
                 let joinedRefs = [marker].concat([marker])\n\
                 let repeatedRefs = [marker].repeat(2)\n\
                 assert(joinedRefs[0] == marker)\n\
                 assert(joinedRefs[1] == marker)\n\
                 assert(repeatedRefs[0] == marker)\n\
                 assert(repeatedRefs[1] == marker)\n\
\n\
                 let empty: Array[Int] = []\n\
                 let hugeEmpty = empty.repeat(9223372036854775807)\n\
                 (left, right, joined, repeated, hugeEmpty)\n\
             }\n";
        assert_eq!(
            execute_function(source, "sequences"),
            RuntimeValue::Tuple(vec![
                RuntimeValue::Array(vec![
                    RuntimeValue::Array(vec![RuntimeValue::Integer(1)]),
                    RuntimeValue::Array(vec![RuntimeValue::Integer(2)]),
                ]),
                RuntimeValue::Array(vec![RuntimeValue::Array(vec![RuntimeValue::Integer(3)])]),
                RuntimeValue::Array(vec![
                    RuntimeValue::Array(vec![RuntimeValue::Integer(9)]),
                    RuntimeValue::Array(vec![RuntimeValue::Integer(2)]),
                    RuntimeValue::Array(vec![RuntimeValue::Integer(3)]),
                ]),
                RuntimeValue::Array(vec![
                    RuntimeValue::Array(vec![RuntimeValue::Integer(7)]),
                    RuntimeValue::Array(vec![RuntimeValue::Integer(3)]),
                ]),
                RuntimeValue::Array(Vec::new()),
            ])
        );

        let program = lowered(source);
        let entry = function_id(&program, "sequences");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 512,
                max_heap_bytes: 256 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert!(matches!(execution.outcome, VmOutcome::Returned(_)));
        assert!(execution.statistics.collections > 0);
    }

    #[test]
    fn array_sequence_preflight_has_stable_panics_and_left_to_right_evaluation() {
        let VmOutcome::Panicked(negative) =
            execute_outcome("fn explode(): Array[Int] { [1].repeat(-1) }\n", "explode")
        else {
            panic!("a negative repeat count must panic")
        };
        assert_eq!(negative.code, PanicCode::InvalidRepeatCount);
        assert_eq!(negative.code.code(), "P0011");

        let VmOutcome::Panicked(overflow) = execute_outcome(
            "fn explode(): Array[Int] {\n\
                 [1, 2].repeat(9223372036854775807)\n\
             }\n",
            "explode",
        ) else {
            panic!("a result length outside Int must panic")
        };
        assert_eq!(overflow.code, PanicCode::CheckedOverflow);
        assert_eq!(overflow.code.code(), "P0005");

        let VmOutcome::Panicked(receiver) = execute_outcome(
            "fn receiver(): Array[Int] { panic(\"receiver-first\") }\n\
             fn count(): Int { panic(\"argument-second\") }\n\
             fn explode(): Array[Int] { receiver().repeat(count()) }\n",
            "explode",
        ) else {
            panic!("the receiver must be evaluated first")
        };
        assert_eq!(receiver.code, PanicCode::ExplicitPanic);
        assert_eq!(receiver.message, "receiver-first");

        let VmOutcome::Panicked(argument) = execute_outcome(
            "fn count(): Int { panic(\"argument-second\") }\n\
             fn explode(): Array[Int] { [1].repeat(count()) }\n",
            "explode",
        ) else {
            panic!("the argument must be evaluated before repeat preflight")
        };
        assert_eq!(argument.code, PanicCode::ExplicitPanic);
        assert_eq!(argument.message, "argument-second");
    }

    #[test]
    fn bytecode_verifier_rederives_array_sequence_kind_and_receiver_mode() {
        let program = lowered(
            "fn combine(left: Array[Int], right: Array[Int]): Array[Int] {\n\
                 left.concat(right)\n\
             }\n",
        );
        let entry = function_id(&program, "combine");

        let mut wrong_kind = program.clone();
        let operation = wrong_kind.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::ArraySequence { kind, .. },
                            ..
                        },
                    ..
                } => Some(kind),
                _ => None,
            })
            .expect("concat lowers to one checked Array sequence operation");
        *operation = bc::BytecodeArraySequenceKind::Repeat;
        let error = bc::verify_bytecode(&wrong_kind).unwrap_err();
        assert!(error.message().contains("operation"), "{error}");

        let mut wrong_receiver = program;
        let array = wrong_receiver.functions[entry.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::ArraySequence { array, .. },
                            ..
                        },
                    ..
                } => Some(array),
                _ => None,
            })
            .expect("concat lowers with one Array receiver");
        let bc::BytecodeOperandKind::Borrow(place) = &array.kind else {
            unreachable!("a verified Array sequence receiver is borrowed")
        };
        array.kind = bc::BytecodeOperandKind::Copy(place.clone());
        let error = bc::verify_bytecode(&wrong_receiver).unwrap_err();
        assert!(error.message().contains("operation"), "{error}");
    }

    #[test]
    fn bytecode_verifier_rederives_map_remove_region_mode() {
        let program = lowered(
            "fn remove(values: var Map[Int, String], key: Int): String? {\n\
                 values.remove(key)\n\
             }\n",
        );
        let entry = function_id(&program, "remove");
        let source = program.functions[entry.index() as usize]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::Store {
                    value:
                        bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::MapRemove { map, .. },
                            ..
                        },
                    ..
                } => map.source_loan,
                _ => None,
            })
            .expect("Map.remove bytecode retains its source region");
        bc::verify_bytecode(&program).unwrap();

        let mut wrong_mode = program;
        wrong_mode.functions[entry.index() as usize].loans[source.index() as usize].mode =
            bc::BytecodeParameterMode::Ref;
        let error = bc::verify_bytecode(&wrong_mode).unwrap_err();
        assert!(error.message().contains("rvalue"), "{error}");
    }

    #[test]
    fn bytecode_verifier_rejects_array_arithmetic_forged_as_pure() {
        let mut program = lowered(
            "fn combine(scalar: Float, values: Array[Float]): Array[Float] {\n\
                 scalar + values\n\
             }\n",
        );
        let entry = function_id(&program, "combine");
        let function = &mut program.functions[entry.index() as usize];
        let block = function
            .blocks
            .iter_mut()
            .find(|block| {
                matches!(
                    block.terminator.kind,
                    bc::BytecodeTerminatorKind::Invoke {
                        operation: bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::CheckedBinary { .. },
                            ..
                        },
                        ..
                    }
                )
            })
            .unwrap();
        let span = block.terminator.span;
        let bc::BytecodeTerminatorKind::Invoke {
            operation,
            destination: Some(destination),
            target: Some(target),
            ..
        } = std::mem::replace(
            &mut block.terminator.kind,
            bc::BytecodeTerminatorKind::Unreachable,
        )
        else {
            unreachable!("the selected terminator is a returning checked Invoke")
        };
        let bc::BytecodeOperationKind::CheckedBinary {
            operator,
            left,
            right,
        } = operation.kind
        else {
            unreachable!("the selected operation is checked binary")
        };
        block.instructions.push(bc::BytecodeInstruction {
            span,
            kind: bc::BytecodeInstructionKind::Store {
                destination,
                value: bc::BytecodeRvalue {
                    ty: operation.ty,
                    kind: bc::BytecodeRvalueKind::Binary {
                        operator,
                        left,
                        right,
                    },
                },
            },
        });
        block.terminator.kind = bc::BytecodeTerminatorKind::Goto { target };

        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error
                .message()
                .contains("potentially panicking binary operation"),
            "{error}"
        );
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
    fn runtime_executes_defer_lifo_on_normal_and_abrupt_scope_exits() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one String")
                };
                self.output.push_str(text);
                Ok(RuntimeValue::Unit)
            }
        }

        fn run(program: &bc::BytecodeProgram, name: &str, host: &mut RecordingHost) -> VmOutcome {
            let entry = function_id(program, name);
            execute(program, entry, host)
                .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(program)))
                .outcome
        }

        let program = lowered(
            "import std.console\n\
             fn emit(value: String) {\n\
                 console.print(value)\n\
             }\n\
             fn hidden(value: Int): impl Discard { value }\n\
             fn consume[T: Discard](value: T) {\n\
                 emit(\"guard\")\n\
             }\n\
             fn normal() {\n\
                 var value = \"captured\"\n\
                 defer emit(\"outer-first\")\n\
                 defer emit(value)\n\
                 value = \"changed\"\n\
                 var blockValue = \"block-captured\"\n\
                 defer {\n\
                     emit(blockValue)\n\
                 }\n\
                 blockValue = \"block-changed\"\n\
                 {\n\
                     defer emit(\"inner\")\n\
                     emit(\"body\")\n\
                 }\n\
             }\n\
             fn guardMove() {\n\
                 let owner = hidden(1)\n\
                 defer consume(owner)\n\
                 let moved = owner\n\
             }\n\
             fn guardTemporary() {\n\
                 defer consume(hidden(2))\n\
             }\n\
             fn guardConsume() {\n\
                 let owner = hidden(3)\n\
                 defer consume(owner)\n\
                 consume(owner)\n\
             }\n\
             fn handoff(): impl Discard {\n\
                 {\n\
                     let owner = hidden(4)\n\
                     defer consume(owner)\n\
                     owner\n\
                 }\n\
             }\n\
             fn guardHandoff() {\n\
                 _ = handoff()\n\
             }\n\
             fn rootHandoff(): impl Discard {\n\
                 let owner = hidden(5)\n\
                 defer consume(owner)\n\
                 owner\n\
             }\n\
             fn guardRootHandoff() {\n\
                 _ = rootHandoff()\n\
             }\n\
             fn failOwner[T: Discard](owner: T): Unit ! T {\n\
                 defer consume(owner)\n\
                 fail owner\n\
             }\n\
             fn guardFailHandoff() {\n\
                 _ = failOwner(hidden(6))\n\
             }\n\
             fn earlyReturn() {\n\
                 defer emit(\"return\")\n\
                 return\n\
             }\n\
             fn failure(): Unit ! String {\n\
                 defer emit(\"fail\")\n\
                 fail \"bad\"\n\
             }\n\
             fn propagate(): Unit ! String {\n\
                 defer emit(\"question\")\n\
                 failure()?\n\
             }\n\
             fn loops() {\n\
                 var index = 0\n\
                 for index < 3 {\n\
                     defer emit(\"loop\")\n\
                     index += 1\n\
                     if index == 1 {\n\
                         continue\n\
                     }\n\
                     if index == 2 {\n\
                         break\n\
                     }\n\
                 }\n\
             }\n\
             fn explode() {\n\
                 defer emit(\"panic\")\n\
                 panic(\"primary\")\n\
             }\n\
             fn consumeInt(value: Int) {\n\
                 _ = value\n\
             }\n\
             fn registrationPanic() {\n\
                 defer emit(\"outer\")\n\
                 defer consumeInt(panic(\"registration\"))\n\
             }\n",
        );
        let mut host = RecordingHost::default();

        assert_eq!(
            run(&program, "normal", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "bodyinnerblock-capturedcapturedouter-first");

        host.output.clear();
        assert_eq!(
            run(&program, "guardMove", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "guard");

        host.output.clear();
        assert_eq!(
            run(&program, "guardTemporary", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "guard");

        host.output.clear();
        assert_eq!(
            run(&program, "guardConsume", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "guard");

        host.output.clear();
        assert_eq!(
            run(&program, "guardHandoff", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "");

        host.output.clear();
        assert_eq!(
            run(&program, "guardRootHandoff", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "");

        host.output.clear();
        assert_eq!(
            run(&program, "guardFailHandoff", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "");

        host.output.clear();
        assert_eq!(
            run(&program, "earlyReturn", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "return");

        host.output.clear();
        assert_eq!(
            run(&program, "failure", &mut host),
            VmOutcome::Returned(RuntimeValue::ResultErr(Box::new(RuntimeValue::String(
                "bad".into()
            ))))
        );
        assert_eq!(host.output, "fail");

        host.output.clear();
        assert_eq!(
            run(&program, "propagate", &mut host),
            VmOutcome::Returned(RuntimeValue::ResultErr(Box::new(RuntimeValue::String(
                "bad".into()
            ))))
        );
        assert_eq!(host.output, "failquestion");

        host.output.clear();
        assert_eq!(
            run(&program, "loops", &mut host),
            VmOutcome::Returned(RuntimeValue::Unit)
        );
        assert_eq!(host.output, "looploop");

        host.output.clear();
        let VmOutcome::Panicked(panic) = run(&program, "explode", &mut host) else {
            panic!("explicit panic must unwind through defer")
        };
        assert_eq!(panic.message, "primary");
        assert!(panic.suppressed.is_empty());
        assert_eq!(host.output, "panic");

        host.output.clear();
        let VmOutcome::Panicked(panic) = run(&program, "registrationPanic", &mut host) else {
            panic!("a registration-time panic must unwind through earlier defers")
        };
        assert_eq!(panic.message, "registration");
        assert!(panic.suppressed.is_empty());
        assert_eq!(host.output, "outer");
    }

    #[test]
    fn runtime_roots_copy_snapshots_while_registering_a_defer() {
        let program = lowered(
            "fn verify(first: Array[Array[Int]], second: Array[Array[Int]]) {\n\
                 assert(first[0][0] == 11)\n\
                 assert(first[5][0] == 16)\n\
                 assert(second[0][0] == 21)\n\
                 assert(second[5][0] == 26)\n\
             }\n\
             fn execute() {\n\
                 let first = [[11], [12], [13], [14], [15], [16]]\n\
                 let second = [[21], [22], [23], [24], [25], [26]]\n\
                 defer verify(first, second)\n\
             }\n",
        );
        let entry = function_id(&program, "execute");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 256,
                max_heap_bytes: 128 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit));
        assert!(execution.statistics.collections > 0);
        assert!(execution.statistics.reclaimed_objects > 0);
    }

    #[test]
    fn runtime_keeps_the_primary_panic_and_runs_later_defer_actions() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one String")
                };
                self.output.push_str(text);
                Ok(RuntimeValue::Unit)
            }
        }

        let program = lowered(
            "import std.console\n\
             fn emit(value: String) {\n\
                 console.print(value)\n\
             }\n\
             fn first() {\n\
                 emit(\"first\")\n\
                 panic(\"first\")\n\
             }\n\
             fn second() {\n\
                 emit(\"second\")\n\
                 panic(\"second\")\n\
             }\n\
             fn cleanupOnly() {\n\
                 defer first()\n\
                 defer second()\n\
             }\n\
             fn primary() {\n\
                 defer first()\n\
                 defer second()\n\
                 panic(\"primary\")\n\
             }\n",
        );

        let mut host = RecordingHost::default();
        let cleanup = execute(&program, function_id(&program, "cleanupOnly"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(cleanup) = cleanup else {
            panic!("cleanup panic must become the primary panic")
        };
        assert_eq!(cleanup.message, "second");
        assert_eq!(
            cleanup
                .suppressed
                .iter()
                .map(|panic| panic.message.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
        assert_eq!(host.output, "secondfirst");

        host.output.clear();
        let primary = execute(&program, function_id(&program, "primary"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(primary) = primary else {
            panic!("body panic must remain primary")
        };
        assert_eq!(primary.message, "primary");
        assert_eq!(
            primary
                .suppressed
                .iter()
                .map(|panic| panic.message.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(host.output, "secondfirst");
    }

    #[test]
    fn bytecode_defer_ledger_rejects_undrained_and_out_of_order_scopes() {
        let mut wrong_guard = lowered(
            "fn note(value: Int) {}\n\
             fn execute() {\n\
                 defer note(1)\n\
             }\n",
        );
        let mut moved_copy = wrong_guard.clone();
        let function = function_id(&moved_copy, "execute");
        let instruction = moved_copy.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find(|instruction| {
                matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::RegisterDefer { .. }
                )
            })
            .expect("copy defer has a registration");
        let bc::BytecodeInstructionKind::RegisterDefer { action, .. } = &mut instruction.kind
        else {
            unreachable!()
        };
        let bc::BytecodeOperationKind::Call { arguments, .. } = &mut action.kind else {
            unreachable!()
        };
        let place = match &arguments[0].value.kind {
            bc::BytecodeOperandKind::Copy(place) => place.clone(),
            _ => unreachable!(),
        };
        arguments[0].value.kind = bc::BytecodeOperandKind::Move(place);
        let error = bc::verify_bytecode(&moved_copy).unwrap_err();
        assert!(
            error.message().contains("snapshot a Copy operand"),
            "{error}"
        );

        let mut forged_disarm = lowered(
            "fn hidden(): impl Discard { 1 }\n\
             fn consume[T: Discard](value: T) {}\n\
             fn execute() {\n\
                 let owner = hidden()\n\
                 defer consume(owner)\n\
                 let moved = owner\n\
             }\n",
        );
        let function = function_id(&forged_disarm, "execute");
        let body = &mut forged_disarm.functions[function.index() as usize];
        let guard = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    guard: Some(guard), ..
                } => Some(guard.clone()),
                _ => None,
            })
            .expect("affine defer has a guard");
        let transition = body
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RetargetCleanup { from, .. } => from == &guard,
                _ => false,
            })
            .expect("whole affine move has a retarget transition");
        let from = match &transition.kind {
            bc::BytecodeInstructionKind::RetargetCleanup { from, .. } => from.clone(),
            _ => unreachable!(),
        };
        transition.kind = bc::BytecodeInstructionKind::DisarmCleanup(from);
        let error = bc::verify_bytecode(&forged_disarm).unwrap_err();
        assert!(
            error.message().contains("disarmed without an immediate"),
            "{error}"
        );

        let mut wrong_callee_protocol = lowered(
            "fn hidden(): impl Discard { 1 }\n\
             fn consume[T: Discard](value: T) {}\n\
             fn execute() {\n\
                 let owner = hidden()\n\
                 let action = () {\n\
                     consume(owner)\n\
                 }\n\
                 defer action()\n\
             }\n",
        );
        let function = function_id(&wrong_callee_protocol, "execute");
        let protocol = wrong_callee_protocol.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find_map(|instruction| match &mut instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    action:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { protocol, .. },
                            ..
                        },
                    guard: Some(_),
                    ..
                } => Some(protocol),
                _ => None,
            })
            .expect("the affine deferred callee has a guarded call");
        assert_eq!(*protocol, bc::BytecodeCallProtocol::CallOnce);
        *protocol = bc::BytecodeCallProtocol::CallMut;
        let error = bc::verify_bytecode(&wrong_callee_protocol).unwrap_err();
        assert!(
            error
                .message()
                .contains("non-Copy deferred callee does not use CallOnce"),
            "{error}"
        );

        let repeatable_callee = lowered(
            "fn hidden(value: Int): impl Discard { value }\n\
             fn observe[T](value: ref T) {}\n\
             fn execute() {\n\
                 let owner = hidden(1)\n\
                 let action = () {\n\
                     observe(ref owner)\n\
                 }\n\
                 defer action()\n\
             }\n",
        );
        bc::verify_bytecode(&repeatable_callee).unwrap();
        let function = function_id(&repeatable_callee, "execute");
        let protocol = repeatable_callee.functions[function.index() as usize]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    action:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { protocol, .. },
                            ..
                        },
                    guard: Some(_),
                    ..
                } => Some(*protocol),
                _ => None,
            })
            .expect("the repeatable affine deferred callee has a guarded call");
        assert_eq!(protocol, bc::BytecodeCallProtocol::CallOnce);

        let mut overwritten_guard = lowered(
            "fn hidden(value: Int): impl Discard { value }\n\
             fn identity[T: Discard](value: T): T { value }\n\
             fn consume[T: Discard](value: T) {}\n\
             fn execute() {\n\
                 let owner = hidden(1)\n\
                 let replacement = hidden(2)\n\
                 defer consume(owner)\n\
                 _ = identity(replacement)\n\
             }\n",
        );
        let identity = overwritten_guard
            .callables
            .iter()
            .enumerate()
            .find_map(|(index, callable)| {
                callable
                    .name
                    .contains("::value::identity[")
                    .then_some(bc::BytecodeCallableId::new(index as u32))
            })
            .expect("identity has one concrete callable");
        let function = function_id(&overwritten_guard, "execute");
        let overwrite_guard = overwritten_guard.functions[function.index() as usize]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    guard: Some(guard), ..
                } => Some(guard.clone()),
                _ => None,
            })
            .expect("the affine defer has a guard");
        let destination =
            overwritten_guard.functions[function.index() as usize]
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.terminator.kind {
                    bc::BytecodeTerminatorKind::Invoke {
                        operation:
                            bc::BytecodeOperation {
                                kind:
                                    bc::BytecodeOperationKind::Call {
                                        callee:
                                            bc::BytecodeOperand {
                                                kind:
                                                    bc::BytecodeOperandKind::Function {
                                                        callable, ..
                                                    },
                                                ..
                                            },
                                        ..
                                    },
                                ..
                            },
                        destination,
                        ..
                    } if *callable == identity => Some(destination),
                    _ => None,
                })
                .expect("identity is lowered as an invocation");
        *destination = Some(overwrite_guard);
        let error = bc::verify_bytecode(&overwritten_guard).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminator overwrites an active defer guard"),
            "{error}"
        );

        let function = function_id(&wrong_guard, "execute");
        let instruction = wrong_guard.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
            .find(|instruction| {
                matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::RegisterDefer { .. }
                )
            })
            .expect("copy defer has a registration");
        let bc::BytecodeInstructionKind::RegisterDefer { action, guard, .. } =
            &mut instruction.kind
        else {
            unreachable!()
        };
        let bc::BytecodeOperationKind::Call { arguments, .. } = &action.kind else {
            unreachable!()
        };
        let bc::BytecodeOperandKind::Copy(place) = &arguments[0].value.kind else {
            unreachable!()
        };
        *guard = Some(place.clone());
        let error = bc::verify_bytecode(&wrong_guard).unwrap_err();
        assert!(
            error
                .message()
                .contains("exactly one guard for its affine operand"),
            "{error}"
        );

        let mut undrained = lowered(
            "fn note(value: Int) {}\n\
             fn marker() {}\n\
             fn execute() {\n\
                 marker()\n\
                 defer note(1)\n\
             }\n",
        );
        let mut unknown_scope = undrained.clone();
        let function = function_id(&unknown_scope, "execute");
        let scopes = unknown_scope.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| {
                let bc::BytecodeTerminatorKind::DrainDefers { scopes, .. } =
                    &mut block.terminator.kind
                else {
                    return None;
                };
                Some(scopes)
            })
            .expect("the defer exit has an explicit drain");
        scopes.push(bc::BytecodeScopeId::new(u32::MAX));
        let error = bc::verify_bytecode(&unknown_scope).unwrap_err();
        assert!(
            error.message().contains("scope with no registration"),
            "{error}"
        );

        let function = function_id(&undrained, "execute");
        let terminator = undrained.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| {
                if block.kind != bc::BytecodeBlockKind::Normal {
                    return None;
                }
                let bc::BytecodeTerminatorKind::DrainDefers { .. } = &block.terminator.kind else {
                    return None;
                };
                Some(&mut block.terminator)
            })
            .expect("normal defer exit has an explicit drain");
        let target = match &terminator.kind {
            bc::BytecodeTerminatorKind::DrainDefers { target, .. } => *target,
            _ => unreachable!(),
        };
        terminator.kind = bc::BytecodeTerminatorKind::Goto { target };
        let error = bc::verify_bytecode(&undrained).unwrap_err();
        assert!(
            error.message().contains("abandons an explicit defer entry"),
            "{error}"
        );

        let mut repeated = lowered(
            "fn note(value: Int) {}\n\
             fn marker() {}\n\
             fn execute(flag: Bool) {\n\
                 marker()\n\
                 defer note(1)\n\
                 if flag {}\n\
             }\n",
        );
        let function = function_id(&repeated, "execute");
        let body = &mut repeated.functions[function.index() as usize];
        let registration = body
            .blocks
            .iter()
            .position(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction.kind,
                        bc::BytecodeInstructionKind::RegisterDefer { .. }
                    )
                })
            })
            .map(|index| bc::BytecodeBlockId::new(index as u32))
            .expect("defer registration follows the marker call");
        assert_ne!(registration, body.entry);
        let if_true = match &body.blocks[registration.index() as usize].terminator.kind {
            bc::BytecodeTerminatorKind::BranchBool { if_true, .. } => *if_true,
            _ => panic!("registration block must branch on the source condition"),
        };
        let bc::BytecodeTerminatorKind::Goto { target } =
            &mut body.blocks[if_true.index() as usize].terminator.kind
        else {
            panic!("true Unit branch must join through one goto")
        };
        *target = registration;
        let error = bc::verify_bytecode(&repeated).unwrap_err();
        assert!(
            error.message().contains("registration is re-executed"),
            "{error}"
        );

        let mut out_of_order = lowered(
            "fn note(value: Int) {}\n\
             fn execute() {\n\
                 defer note(1)\n\
                 {\n\
                     defer note(2)\n\
                 }\n\
             }\n",
        );
        let function = function_id(&out_of_order, "execute");
        let scopes = out_of_order.functions[function.index() as usize]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer { scope, .. } => Some(scope),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(scopes.len(), 2);
        let outer = scopes[0];
        let inner = scopes[1];
        let inner_drain = out_of_order.functions[function.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| {
                let bc::BytecodeTerminatorKind::DrainDefers { scopes, .. } =
                    &mut block.terminator.kind
                else {
                    return None;
                };
                (block.kind == bc::BytecodeBlockKind::Normal && scopes.as_slice() == [inner])
                    .then_some(scopes)
            })
            .expect("inner lexical scope has its own drain");
        *inner_drain = vec![outer];
        let error = bc::verify_bytecode(&out_of_order).unwrap_err();
        assert!(
            error.message().contains("skips a still-active inner scope"),
            "{error}"
        );

        let mut use_after_drain = lowered(
            "fn hidden(): impl Discard { 1 }\n\
             fn consume[T: Discard](value: T) {}\n\
             fn execute() {\n\
                 let owner = hidden()\n\
                 {\n\
                     defer consume(owner)\n\
                 }\n\
             }\n",
        );
        let function = function_id(&use_after_drain, "execute");
        let body = &mut use_after_drain.functions[function.index() as usize];
        let (guard, scope) = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    scope,
                    guard: Some(guard),
                    ..
                } => Some((guard.clone(), *scope)),
                _ => None,
            })
            .expect("the inner defer has one guard");
        let (target, span) = body
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::DrainDefers { scopes, target, .. }
                    if block.kind == bc::BytecodeBlockKind::Normal && scopes.contains(&scope) =>
                {
                    Some((*target, block.terminator.span))
                }
                _ => None,
            })
            .expect("the inner scope has a normal drain");
        let guard_ty = guard.ty;
        body.blocks[target.index() as usize].instructions.insert(
            0,
            bc::BytecodeInstruction {
                span,
                kind: bc::BytecodeInstructionKind::Store {
                    destination: guard.clone(),
                    value: bc::BytecodeRvalue {
                        ty: guard_ty,
                        kind: bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                            ty: guard_ty,
                            kind: bc::BytecodeOperandKind::Move(guard),
                        }),
                    },
                },
            },
        );
        let error = bc::verify_bytecode(&use_after_drain).unwrap_err();
        assert!(
            error
                .message()
                .contains("already consumed by a deferred action"),
            "{error}"
        );

        let reinitialized = lowered(
            "fn hidden(value: Int): impl Discard { value }\n\
             fn consume[T: Discard](value: T) {}\n\
             fn execute() {\n\
                 var owner = hidden(1)\n\
                 {\n\
                     defer consume(owner)\n\
                 }\n\
                 owner = hidden(2)\n\
                 consume(owner)\n\
             }\n",
        );
        bc::verify_bytecode(&reinitialized).unwrap();
    }

    #[test]
    fn iterator_exhaustion_guards_are_specialized_and_reverified() {
        let mut nonterminal = lowered(
            "fn hidden(value: Int): impl Discard { value }\n\
             fn cleanup[T: Discard](values: Array[T]) {}\n\
             fn consumeOne[T: Discard](value: T) {}\n\
             fn drain[T](\n\
                 values: Array[T],\n\
                 cleanupAll: fn(Array[T]),\n\
                 consume: fn(T),\n\
             ) {\n\
                 defer cleanupAll(values)\n\
                 for value in values {\n\
                     consume(value)\n\
                 }\n\
             }\n\
             fn main() {\n\
                 drain([hidden(1)], cleanup, consumeOne)\n\
             }\n",
        );
        let drain = nonterminal
            .callables
            .iter()
            .find(|callable| callable.name.contains("::value::drain["))
            .and_then(|callable| callable.implementation)
            .expect("generic drain has one concrete bytecode body");
        let state = nonterminal.functions[drain.index() as usize]
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    state,
                    exhaustion_guard,
                    ..
                } => {
                    assert!(
                        exhaustion_guard.is_none(),
                        "concrete Discard element removes the potential terminal transition"
                    );
                    Some(state.clone())
                }
                _ => None,
            })
            .expect("concrete drain has an intrinsic cursor");
        let bc::BytecodeTypeKind::Cursor { collection, .. } =
            nonterminal.types[state.ty.index() as usize].kind
        else {
            panic!("iterator state has a concrete cursor type")
        };
        let mut forged = state;
        forged.ty = collection;
        forged.projections.push(bc::BytecodeProjection {
            ty: collection,
            kind: bc::BytecodeProjectionKind::IteratorSource,
        });
        let guard = nonterminal.functions[drain.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    exhaustion_guard, ..
                } => Some(exhaustion_guard),
                _ => None,
            })
            .unwrap();
        *guard = Some(forged);
        let error = bc::verify_bytecode(&nonterminal).unwrap_err();
        assert!(
            error.message().contains("terminator"),
            "unexpected bytecode rejection: {error}"
        );

        let mut terminal = lowered(
            "fn drainTerminal(\n\
                 values: Array[Join[Int, Never]],\n\
                 cleanupAll: fn(Array[Join[Int, Never]]),\n\
                 consume: fn(Join[Int, Never]),\n\
             ) {\n\
                 defer cleanupAll(values)\n\
                 for value in values {\n\
                     consume(value)\n\
                 }\n\
             }\n",
        );
        let drain = function_id(&terminal, "drainTerminal");
        let guard = terminal.functions[drain.index() as usize]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext {
                    exhaustion_guard, ..
                } => exhaustion_guard.take(),
                _ => None,
            })
            .expect("terminal collection keeps its exact exhaustion guard");
        let error = bc::verify_bytecode(&terminal).unwrap_err();
        assert!(
            error.message().contains("terminator"),
            "unexpected bytecode rejection after removing {guard:?}: {error}"
        );
    }

    #[test]
    fn terminal_explicit_cleanup_replaces_exactly_one_bytecode_fallback() {
        let mut program = lowered(
            "fn cleanup(value: Join[Int, String]?) {\n\
                 panic(\"cleanup\")\n\
             }\n\
             fn guarded(owner: Join[Int, String]?) {\n\
                 defer cleanup(owner)\n\
                 let moved = owner\n\
                 panic(\"primary\")\n\
             }\n",
        );
        let guarded = function_id(&program, "guarded");
        let body = &program.functions[guarded.index() as usize];
        let guard = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer {
                    guard: Some(guard), ..
                } => Some(guard.clone()),
                _ => None,
            })
            .expect("terminal defer has one explicit guard");
        assert_eq!(
            body.blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter(|instruction| matches!(
                    &instruction.kind,
                    bc::BytecodeInstructionKind::RegisterFallback { owner, .. }
                        if owner == &guard
                ))
                .count(),
            1,
            "the explicit terminal guard has exactly one fallback predecessor"
        );

        let body = &mut program.functions[guarded.index() as usize];
        let (block, index, scope, owner, span) = body
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .enumerate()
                    .find_map(|(index, instruction)| match &instruction.kind {
                        bc::BytecodeInstructionKind::RegisterDefer {
                            scope,
                            guard: Some(guard),
                            ..
                        } => Some((block, index, *scope, guard.clone(), instruction.span)),
                        _ => None,
                    })
            })
            .expect("terminal defer registration is present before mutation");
        body.blocks[block].instructions.insert(
            index + 1,
            bc::BytecodeInstruction {
                span,
                kind: bc::BytecodeInstructionKind::RegisterFallback { scope, owner },
            },
        );
        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminal fallback overlaps an active explicit cleanup guard"),
            "{error}"
        );
        let mut host = RejectingHost;
        let error = execute(&program, guarded, &mut host).unwrap_err();
        assert!(
            matches!(error, VmError::InvalidBytecode(_)),
            "the VM must reject a double-armed terminal owner: {error}"
        );
    }

    #[test]
    fn runtime_excludes_terminal_fallbacks_across_every_explicit_cleanup_transition() {
        let program = lowered(
            "fn cleanup(value: Join[Int, String]?) {\n\
                 panic(\"cleanup\")\n\
             }\n\
             fn cleanupMarked(value: Join[Int, String]?, marker: Int) {\n\
                 panic(\"cleanup\")\n\
             }\n\
             fn cleanupAll(values: Array[Join[Int, String]?]) {\n\
                 panic(\"cleanup\")\n\
             }\n\
             fn cleanupPair(value: (Join[Int, String]?, Int)) {\n\
                 panic(\"cleanup\")\n\
             }\n\
             fn primary(owner: Join[Int, String]?) {\n\
                 defer cleanup(owner)\n\
                 let moved = owner\n\
                 panic(\"primary\")\n\
             }\n\
             fn normal(owner: Join[Int, String]?) {\n\
                 defer cleanup(owner)\n\
             }\n\
             fn aggregate(owner: Join[Int, String]?) {\n\
                 let wrapped = (owner, 1)\n\
                 defer cleanupPair(wrapped)\n\
                 let moved = wrapped\n\
                 panic(\"aggregate\")\n\
             }\n\
             fn sink(owner: Join[Int, String]?) {\n\
                 panic(\"sink\")\n\
             }\n\
             fn call(owner: Join[Int, String]?) {\n\
                 defer cleanup(owner)\n\
                 sink(owner)\n\
             }\n\
             fn iteration(values: Array[Join[Int, String]?]) {\n\
                 defer cleanupAll(values)\n\
                 for value in values {\n\
                     panic(\"item\")\n\
                 }\n\
             }\n\
             fn registration(owner: Join[Int, String]?) {\n\
                 defer cleanupMarked(owner, panic(\"registration\"))\n\
             }\n\
             fn runPrimary() {\n\
                 let owner: Join[Int, String]? = none\n\
                 primary(owner)\n\
             }\n\
             fn runNormal() {\n\
                 let owner: Join[Int, String]? = none\n\
                 normal(owner)\n\
             }\n\
             fn runAggregate() {\n\
                 let owner: Join[Int, String]? = none\n\
                 aggregate(owner)\n\
             }\n\
             fn runCall() {\n\
                 let owner: Join[Int, String]? = none\n\
                 call(owner)\n\
             }\n\
             fn runIteration() {\n\
                 let values: Array[Join[Int, String]?] = []\n\
                 iteration(values)\n\
             }\n\
             fn runRegistration() {\n\
                 let owner: Join[Int, String]? = none\n\
                 registration(owner)\n\
             }\n",
        );

        let mut host = RejectingHost;
        let outcome = execute(&program, function_id(&program, "runPrimary"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("the body panic must remain primary")
        };
        assert_eq!(panic.message, "primary");
        assert_eq!(
            panic
                .suppressed
                .iter()
                .map(|panic| panic.message.as_str())
                .collect::<Vec<_>>(),
            ["cleanup"]
        );

        let outcome = execute(&program, function_id(&program, "runNormal"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("normal scope exit must execute the explicit cleanup")
        };
        assert_eq!(panic.message, "cleanup");
        assert!(panic.suppressed.is_empty());

        let outcome = execute(&program, function_id(&program, "runAggregate"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("the aggregate path must retain the primary panic")
        };
        assert_eq!(panic.message, "aggregate");
        assert_eq!(
            panic
                .suppressed
                .iter()
                .map(|panic| panic.message.as_str())
                .collect::<Vec<_>>(),
            ["cleanup"]
        );

        let outcome = execute(&program, function_id(&program, "runCall"), &mut host)
            .unwrap()
            .outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("the consuming call must keep its own panic")
        };
        assert_eq!(panic.message, "sink");
        assert!(panic.suppressed.is_empty());

        assert_eq!(
            execute(&program, function_id(&program, "runIteration"), &mut host)
                .unwrap()
                .outcome,
            VmOutcome::Returned(RuntimeValue::Unit),
            "natural exhaustion must disarm the explicit cleanup and its former fallback"
        );

        let outcome = execute(
            &program,
            function_id(&program, "runRegistration"),
            &mut host,
        )
        .unwrap()
        .outcome;
        let VmOutcome::Panicked(panic) = outcome else {
            panic!("registration-time panic must preserve the original fallback")
        };
        assert_eq!(panic.message, "registration");
        assert!(panic.suppressed.is_empty());
    }

    #[test]
    fn terminal_fallbacks_are_closed_per_bytecode_instance() {
        let mut terminal = lowered(
            "fn stop(task: Join[Int, String]): Never {\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let stop = function_id(&terminal, "stop");
        let body = &terminal.functions[stop.index() as usize];
        let entry = body.entry;
        assert!(matches!(
            body.block(body.entry)
                .and_then(|block| block.instructions.first())
                .map(|instruction| &instruction.kind),
            Some(bc::BytecodeInstructionKind::RegisterFallback { owner, .. })
                if owner.slot == body.parameters[0] && owner.projections.is_empty()
        ));
        assert!(body.blocks.iter().any(|block| matches!(
            block.terminator.kind,
            bc::BytecodeTerminatorKind::DrainUnwind { target } if target == body.unwind
        )));

        terminal.functions[stop.index() as usize].blocks[entry.index() as usize]
            .instructions
            .retain(|instruction| {
                !matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::RegisterFallback { .. }
                )
            });
        let error = bc::verify_bytecode(&terminal).unwrap_err();
        assert!(
            error
                .message()
                .contains("has no entry fallback registration"),
            "{error}"
        );

        let nonterminal = lowered(
            "fn stop[T](value: T): Never {\n\
                 panic(\"stop\")\n\
             }\n\
             fn main() {\n\
                 stop(1)\n\
             }\n",
        );
        let stop = nonterminal
            .callables
            .iter()
            .find(|callable| callable.name.contains("::value::stop["))
            .and_then(|callable| callable.implementation)
            .expect("generic stop has one closed Int body");
        assert!(
            !nonterminal.functions[stop.index() as usize]
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| matches!(
                    instruction.kind,
                    bc::BytecodeInstructionKind::RegisterFallback { .. }
                )),
            "the Int specialization must remove its conservative MIR fallback"
        );

        let empty = lowered(
            "fn main() {\n\
                 let pending: Join[Int, String]? = none\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let main = function_id(&empty, "main");
        let mut host = RejectingHost;
        let execution = execute(&empty, main, &mut host).unwrap();
        let VmOutcome::Panicked(panic) = execution.outcome else {
            panic!("the explicit panic must survive terminal fallback cleanup")
        };
        assert_eq!(panic.code, PanicCode::ExplicitPanic);
    }

    #[test]
    fn runtime_unifies_explicit_and_structural_fallback_entries_in_lifo_order() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one String")
                };
                self.output.push_str(text);
                Ok(RuntimeValue::Unit)
            }
        }

        let program = lowered(
            "import std.console\n\
             fn main() {\n\
                 let first: Join[Int, String]? = none\n\
                 defer console.print(\"a\")\n\
                 let second: Join[Int, String]? = none\n\
                 defer console.print(\"b\")\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let main = function_id(&program, "main");
        let mut host = RecordingHost::default();
        let execution = execute(&program, main, &mut host).unwrap();
        assert_eq!(host.output, "ba");
        let VmOutcome::Panicked(panic) = execution.outcome else {
            panic!("the original panic must resume after the unified drain")
        };
        assert_eq!(panic.code, PanicCode::ExplicitPanic);
    }

    #[test]
    fn bytecode_requires_fallback_coverage_at_every_terminal_materialization_edge() {
        let mut missing_store = lowered(
            "fn main() {\n\
                 let pending: Join[Int, String]? = none\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let main = function_id(&missing_store, "main");
        let body = &mut missing_store.functions[main.index() as usize];
        let registration = body
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.instructions
                    .iter()
                    .position(|instruction| {
                        matches!(
                            instruction.kind,
                            bc::BytecodeInstructionKind::RegisterFallback { .. }
                        )
                    })
                    .map(|instruction| (block, instruction))
            })
            .expect("terminal Option construction registers a fallback");
        body.blocks[registration.0]
            .instructions
            .remove(registration.1);
        let error = bc::verify_bytecode(&missing_store).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminal store result has no immediate fallback"),
            "{error}"
        );

        let mut missing_invoke = lowered(
            "fn empty(): Join[Int, String]? { none }\n\
             fn main() {\n\
                 let pending = empty()\n\
                 panic(\"stop\")\n\
             }\n",
        );
        let main = function_id(&missing_invoke, "main");
        let body = &mut missing_invoke.functions[main.index() as usize];
        let target = body
            .blocks
            .iter()
            .find_map(|block| match block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    destination: Some(_),
                    target: Some(target),
                    ..
                } => Some(target),
                _ => None,
            })
            .expect("terminal call has a normal result edge");
        body.blocks[target.index() as usize].instructions.remove(0);
        let error = bc::verify_bytecode(&missing_invoke).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminal invocation result edge has no fallback"),
            "{error}"
        );

        let mut missing_item = lowered(
            "fn inspect(values: Array[Join[Int, String]?]) {\n\
                 for value in values {\n\
                     panic(\"stop\")\n\
                 }\n\
             }\n",
        );
        let inspect = function_id(&missing_item, "inspect");
        let body = &mut missing_item.functions[inspect.index() as usize];
        let has_value = body
            .blocks
            .iter()
            .find_map(|block| match block.terminator.kind {
                bc::BytecodeTerminatorKind::IteratorNext { has_value, .. } => Some(has_value),
                _ => None,
            })
            .expect("own iterator has a value edge");
        body.blocks[has_value.index() as usize]
            .instructions
            .remove(0);
        let error = bc::verify_bytecode(&missing_item).unwrap_err();
        assert!(
            error
                .message()
                .contains("terminal iterator value edge has no fallback"),
            "{error}"
        );
    }

    #[test]
    fn copy_defer_guards_are_specialized_to_snapshots() {
        #[derive(Default)]
        struct RecordingHost {
            output: String,
        }

        impl VmHost for RecordingHost {
            fn invoke(
                &mut self,
                name: &str,
                arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                assert_eq!(name, "std.console.print");
                let [RuntimeValue::String(text)] = arguments else {
                    panic!("console print must receive one String")
                };
                self.output.push_str(text);
                Ok(RuntimeValue::Unit)
            }
        }

        let program = lowered(
            "import std.console\n\
             fn cleanup(value: Int) {\n\
                 assert(value == 1)\n\
                 console.print(\"cleanup\")\n\
             }\n\
             fn guarded[T: Discard](owner: T, action: fn(T)) {\n\
                 defer action(owner)\n\
                 let moved = owner\n\
                 _ = moved\n\
             }\n\
             fn execute() {\n\
                 guarded(1, cleanup)\n\
             }\n",
        );
        let guarded = program
            .callables
            .iter()
            .find(|callable| callable.name.contains("::value::guarded["))
            .and_then(|callable| callable.implementation)
            .expect("generic guarded has one concrete bytecode body");
        let body = &program.functions[guarded.index() as usize];
        let (action, guard) = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match &instruction.kind {
                bc::BytecodeInstructionKind::RegisterDefer { action, guard, .. } => {
                    Some((action, guard))
                }
                _ => None,
            })
            .expect("generic guarded registers one defer");
        assert!(guard.is_none(), "the concrete Copy owner needs no guard");
        let bc::BytecodeOperationKind::Call { arguments, .. } = &action.kind else {
            panic!("the deferred action remains a call")
        };
        assert!(matches!(
            arguments[0].value.kind,
            bc::BytecodeOperandKind::Copy(_)
        ));
        let owner_ty = arguments[0].value.ty;
        assert!(
            !body.blocks.iter().any(|block| {
                block
                    .instructions
                    .iter()
                    .any(|instruction| match &instruction.kind {
                        bc::BytecodeInstructionKind::RetargetCleanup { from, .. } => {
                            from.ty == owner_ty
                        }
                        bc::BytecodeInstructionKind::DisarmCleanup(place) => place.ty == owner_ty,
                        _ => false,
                    })
            }),
            "{}",
            bc::disassemble(&program)
        );

        let mut host = RecordingHost::default();
        let execution = execute(&program, function_id(&program, "execute"), &mut host).unwrap();
        assert_eq!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit));
        assert_eq!(host.output, "cleanup");
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
    fn operation_temporaries_survive_gc_between_left_to_right_evaluations() {
        let program = lowered(
            "type Pair = { left: String, right: String }\n\
             fn key(value: String): String { value }\n\
             fn execute(): Int {\n\
                 assert(\"same\" == \"same\")\n\
                 assert(\"needle\" in [\"other\", \"needle\"])\n\
                 let entries = [\n\
                     key(\"first\"): [\"one\", \"two\"],\n\
                     key(\"second\"): [\"three\", \"four\"],\n\
                 ]\n\
                 assert(\"first\" in entries)\n\
                 let pair = Pair { left: \"old-left\", right: \"old-right\" }\n\
                 let updated = pair with {\n\
                     left: \"new-left\",\n\
                     right: \"new-right\",\n\
                 }\n\
                 assert(updated.left == \"new-left\")\n\
                 assert(updated.right == \"new-right\")\n\
                 let groups = [[\"a\"], [\"b\"], [\"c\"], [\"d\"]]\n\
                 let copied = groups[:]\n\
                 assert(copied[3][0] == \"d\")\n\
                 let totals = [[1, 2], [3, 4]] + [[10, 20], [30, 40]]\n\
                 assert(totals[1][1] == 44)\n\
                 assert(true, \"message-left\", \"message-right\")\n\
                 42\n\
             }\n",
        );
        let entry = function_id(&program, "execute");
        let mut host = RejectingHost;
        let execution = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_objects: 256,
                max_heap_bytes: 256 * 1024,
                initial_gc_threshold: 1,
                ..VmLimits::default()
            },
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(42))
        );
        assert!(execution.statistics.collections > 0);
        assert!(execution.statistics.reclaimed_objects > 0);
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
    fn nested_projected_stateful_and_fallible_closures_execute() {
        assert_eq!(
            execute_function(
                "fn execute(): Int {\n\
                     let base = 39\n\
                     let make = (offset: Int) {\n\
                         (value: Int): Int { base + offset + value }\n\
                     }\n\
                     let operation = make(1)\n\
                     operation(2)\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(
                "fn make_counter(start: Int): impl CallMut[fn(): Int] + Discard {\n\
                     var count = start\n\
                     (): Int {\n\
                         count += 1\n\
                         count\n\
                     }\n\
                 }\n\
                 fn execute(): Int {\n\
                     var counter = make_counter(40)\n\
                     counter() + counter()\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(83)
        );
        assert_eq!(
            execute_function(
                "fn execute(): Int {\n\
                     let offset = 40\n\
                     let add: fn(Int): Int = (value) { offset + value }\n\
                     let operations = [add]\n\
                     operations[0](2)\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(42)
        );

        let source = "fn evaluate(value: Int): Int ! String {\n\
                          let adjust = (candidate: Int): Int ! String {\n\
                              if candidate < 0 {\n\
                                  fail \"negative\"\n\
                              }\n\
                              candidate + 1\n\
                          }\n\
                          let adjusted = adjust(value)?\n\
                          adjusted + 1\n\
                      }\n\
                      fn success(): Int ! String { evaluate(0) }\n\
                      fn failure(): Int ! String { evaluate(-1) }\n";
        assert_eq!(
            execute_function(source, "success"),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(2)))
        );
        assert_eq!(
            execute_function(source, "failure"),
            RuntimeValue::ResultErr(Box::new(RuntimeValue::String("negative".into())))
        );
    }

    #[test]
    fn generic_opaque_and_variadic_closure_calls_use_the_same_indirect_path() {
        assert_eq!(
            execute_function(
                "fn increment(value: Int): Int { value + 1 }\n\
                 fn apply[F: Discard + Call[fn(Int): Int]](operation: F, value: Int): Int {\n\
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
                "fn increment(value: Int): Int { value + 1 }\n\
                 fn invoke_once[F: Copy + CallOnce[fn(Int): Int]](\n\
                     operation: F,\n\
                     value: Int,\n\
                 ): Int {\n\
                     operation(value)\n\
                 }\n\
                 fn execute(): (Int, Int) {\n\
                     let offset = 40\n\
                     let closure = (value: Int): Int { value + offset }\n\
                     (invoke_once(increment, 41), invoke_once(closure, 2))\n\
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
                "fn make(offset: Int): impl CallMut[fn(Int): Int] + Discard {\n\
                     (value: Int): Int { value + offset }\n\
                 }\n\
                 fn execute(): Int {\n\
                     var operation = make(40)\n\
                     operation(2)\n\
                 }\n",
                "execute",
            ),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            execute_function(
                "fn make(offset: Int): impl Copy + CallOnce[fn(Int): Int] + Discard {\n\
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
                "fn increment(value: Int): Int { value + 1 }\n\
                 fn make(): impl Copy + CallOnce[fn(Int): Int] + Discard {\n\
                     increment\n\
                 }\n\
                 fn execute(): Int {\n\
                     let operation = make()\n\
                     operation(41)\n\
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
    fn affine_opaque_values_move_through_generic_call_once_and_execute() {
        let source = "fn identity[T: Discard](value: T): T {\n\
                          let local = value\n\
                          local\n\
                      }\n\
                      fn make(offset: Int): impl CallOnce[fn(Int): Int] + Discard {\n\
                          (value: Int): Int { value + offset }\n\
                      }\n\
                      fn execute(): Int {\n\
                          let operation = make(40)\n\
                          let moved = identity(operation)\n\
                          moved(2)\n\
                      }\n";
        let program = lowered(source);
        let entry = function_id(&program, "execute");
        let execute_body = program.function(entry).unwrap();
        assert!(execute_body.blocks.iter().any(|block| matches!(
            &block.terminator.kind,
            bc::BytecodeTerminatorKind::Invoke {
                operation: bc::BytecodeOperation {
                    kind: bc::BytecodeOperationKind::Call {
                        callee: bc::BytecodeOperand {
                            kind: bc::BytecodeOperandKind::Move(_),
                            ..
                        },
                        ..
                    },
                    ..
                },
                ..
            }
        )));
        let mut host = RejectingHost;
        let execution = execute(&program, entry, &mut host)
            .unwrap_or_else(|error| panic!("{error}\n{}", bc::disassemble(&program)));
        assert_eq!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(42))
        );
    }

    #[test]
    fn affine_vars_reinitialize_after_a_complete_write_and_execute() {
        let source = "fn replace[T: Discard](first: T, second: T): T {\n\
                          var value = first\n\
                          let old = value\n\
                          value = second\n\
                          _ = old\n\
                          value\n\
                      }\n\
                      fn execute(): Int { replace(1, 42) }\n";
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Integer(42)
        );
    }

    #[test]
    fn affine_equality_and_membership_borrow_without_consuming_their_inputs() {
        let source = "fn hidden(value: Int): impl Equatable + Discard { value }\n\
                      fn execute(): (Bool, Bool) {\n\
                          let left = hidden(42)\n\
                          let right = hidden(42)\n\
                          let equal = left == right\n\
                          _ = left\n\
                          _ = right\n\
                          let needle = hidden(2)\n\
                          let values = [hidden(1), hidden(2)]\n\
                          let found = needle in values\n\
                          _ = needle\n\
                          _ = values\n\
                          (equal, found)\n\
                      }\n";
        assert_eq!(
            execute_function(source, "execute"),
            RuntimeValue::Tuple(vec![RuntimeValue::Bool(true), RuntimeValue::Bool(true)])
        );
        let program = lowered(source);
        let execute = program.function(function_id(&program, "execute")).unwrap();
        assert!(
            execute
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| matches!(
                    &instruction.kind,
                    bc::BytecodeInstructionKind::Store {
                        value: bc::BytecodeRvalue {
                            kind: bc::BytecodeRvalueKind::Binary {
                                left: bc::BytecodeOperand {
                                    kind: bc::BytecodeOperandKind::Borrow(_),
                                    ..
                                },
                                right: bc::BytecodeOperand {
                                    kind: bc::BytecodeOperandKind::Borrow(_),
                                    ..
                                },
                                ..
                            },
                            ..
                        },
                        ..
                    }
                ))
        );
    }

    #[test]
    fn bytecode_verifier_rejects_repeated_and_joined_affine_moves() {
        let mut program = lowered(
            "fn consume[T: Discard](input: T) {\n\
                 _ = input\n\
             }\n\
             fn execute() {\n\
                 consume(42)\n\
             }\n",
        );
        let (function, block, instruction) = program
            .functions
            .iter()
            .enumerate()
            .find_map(|(function, body)| {
                body.blocks.iter().enumerate().find_map(|(block, body)| {
                    body.instructions
                        .iter()
                        .position(|instruction| {
                            matches!(
                                &instruction.kind,
                                bc::BytecodeInstructionKind::Store {
                                    value: bc::BytecodeRvalue {
                                        kind: bc::BytecodeRvalueKind::Use(
                                            bc::BytecodeOperand {
                                                kind: bc::BytecodeOperandKind::Move(place),
                                                ..
                                            }
                                        ),
                                        ..
                                    },
                                    ..
                                } if place.projections.is_empty()
                            )
                        })
                        .map(|instruction| (function, block, instruction))
                })
            })
            .expect("generic source transfer remains one direct bytecode move");
        let duplicate = program.functions[function].blocks[block].instructions[instruction].clone();
        program.functions[function].blocks[block]
            .instructions
            .insert(instruction + 1, duplicate);

        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable")
        );

        let mut program = lowered(
            "fn consume[T: Discard](input: T, flag: Bool) {\n\
                 if flag {\n\
                     _ = input\n\
                     return\n\
                 }\n\
                 _ = input\n\
             }\n\
             fn execute() { consume(42, true) }\n",
        );
        let (function_index, returning, joined) = program
            .functions
            .iter()
            .enumerate()
            .find_map(|(function_index, function)| {
                function.slots.iter().enumerate().find_map(|(slot, info)| {
                    if !matches!(info.kind, bc::BytecodeSlotKind::Parameter { index: 0 }) {
                        return None;
                    }
                    let slot = bc::BytecodeSlotId::new(slot as u32);
                    let move_blocks = function
                        .blocks
                        .iter()
                        .enumerate()
                        .filter_map(|(block, body)| {
                            body.instructions
                                .iter()
                                .any(|instruction| {
                                    matches!(
                                        &instruction.kind,
                                        bc::BytecodeInstructionKind::Store {
                                            value: bc::BytecodeRvalue {
                                                kind: bc::BytecodeRvalueKind::Use(
                                                    bc::BytecodeOperand {
                                                        kind: bc::BytecodeOperandKind::Move(place),
                                                        ..
                                                    }
                                                ),
                                                ..
                                            },
                                            ..
                                        } if place.slot == slot && place.projections.is_empty()
                                    )
                                })
                                .then_some(block)
                        })
                        .collect::<Vec<_>>();
                    if move_blocks.len() != 2 {
                        return None;
                    }
                    let returning = move_blocks.iter().copied().find(|block| {
                        matches!(
                            function.blocks[*block].terminator.kind,
                            bc::BytecodeTerminatorKind::Return
                        )
                    })?;
                    let joined = move_blocks.into_iter().find(|block| *block != returning)?;
                    Some((function_index, returning, joined))
                })
            })
            .expect("the specialized generic body has two path-exclusive moves");
        program.functions[function_index].blocks[returning]
            .terminator
            .kind = bc::BytecodeTerminatorKind::Goto {
            target: bc::BytecodeBlockId::new(joined as u32),
        };

        let error = bc::verify_bytecode(&program).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable")
        );
    }

    #[test]
    fn bytecode_move_paths_allow_siblings_restore_children_and_reject_root_reuse() {
        let source = "fn identity[T](value: T): T { value }\n\
                      fn rebuild[T: Discard](input: (T, T)): (T, T) {\n\
                          let (left, right) = input\n\
                          identity((left, right))\n\
                      }\n\
                      fn execute(): (String, String) {\n\
                          rebuild((\"left\", \"right\"))\n\
                      }\n";

        let mut duplicate = lowered(source);
        let (function, block, instruction) = duplicate
            .functions
            .iter()
            .enumerate()
            .find_map(|(function, body)| {
                body.blocks.iter().enumerate().find_map(|(block, body)| {
                    body.instructions
                        .iter()
                        .position(|instruction| {
                            matches!(
                                &instruction.kind,
                                bc::BytecodeInstructionKind::Store {
                                    value: bc::BytecodeRvalue {
                                        kind: bc::BytecodeRvalueKind::Use(
                                            bc::BytecodeOperand {
                                                kind: bc::BytecodeOperandKind::Move(place),
                                                ..
                                            }
                                        ),
                                        ..
                                    },
                                    ..
                                } if matches!(
                                    place.projections.as_slice(),
                                    [bc::BytecodeProjection {
                                        kind: bc::BytecodeProjectionKind::TupleField(_),
                                        ..
                                    }]
                                )
                            )
                        })
                        .map(|instruction| (function, block, instruction))
                })
            })
            .expect("tuple destructuring moves one projected bytecode child");
        let repeated =
            duplicate.functions[function].blocks[block].instructions[instruction].clone();
        duplicate.functions[function].blocks[block]
            .instructions
            .insert(instruction + 1, repeated);
        let error = bc::verify_bytecode(&duplicate).unwrap_err();
        assert!(error.message().contains("unavailable move path"), "{error}");

        let mut restored = lowered(source);
        let (function, block, instruction) = restored
            .functions
            .iter()
            .enumerate()
            .find_map(|(function, body)| {
                body.blocks.iter().enumerate().find_map(|(block, body)| {
                    body.instructions
                        .iter()
                        .position(|instruction| {
                            matches!(
                                &instruction.kind,
                                bc::BytecodeInstructionKind::Store {
                                    value: bc::BytecodeRvalue {
                                        kind: bc::BytecodeRvalueKind::Use(
                                            bc::BytecodeOperand {
                                                kind: bc::BytecodeOperandKind::Move(place),
                                                ..
                                            }
                                        ),
                                        ..
                                    },
                                    ..
                                } if matches!(
                                    place.projections.as_slice(),
                                    [bc::BytecodeProjection {
                                        kind: bc::BytecodeProjectionKind::TupleField(_),
                                        ..
                                    }]
                                )
                            )
                        })
                        .map(|instruction| (function, block, instruction))
                })
            })
            .unwrap();
        let projected_move =
            restored.functions[function].blocks[block].instructions[instruction].clone();
        let bc::BytecodeInstructionKind::Store {
            destination: child_owner,
            value:
                bc::BytecodeRvalue {
                    kind:
                        bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                            kind: bc::BytecodeOperandKind::Move(child),
                            ..
                        }),
                    ..
                },
        } = &projected_move.kind
        else {
            unreachable!()
        };
        let restore = bc::BytecodeInstruction {
            span: projected_move.span,
            kind: bc::BytecodeInstructionKind::Store {
                destination: child.clone(),
                value: bc::BytecodeRvalue {
                    ty: child_owner.ty,
                    kind: bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                        ty: child_owner.ty,
                        kind: bc::BytecodeOperandKind::Move(child_owner.clone()),
                    }),
                },
            },
        };
        restored.functions[function].blocks[block]
            .instructions
            .insert(instruction + 1, restore);
        restored.functions[function].blocks[block]
            .instructions
            .insert(instruction + 2, projected_move);
        bc::verify_bytecode(&restored).unwrap();

        let mut root_reuse = lowered(source);
        let (function_index, root, root_type) = root_reuse
            .functions
            .iter()
            .enumerate()
            .find_map(|(function, body)| {
                body.blocks
                    .iter()
                    .flat_map(|block| &block.instructions)
                    .find_map(|instruction| match &instruction.kind {
                        bc::BytecodeInstructionKind::Store {
                            value:
                                bc::BytecodeRvalue {
                                    kind:
                                        bc::BytecodeRvalueKind::Use(bc::BytecodeOperand {
                                            kind: bc::BytecodeOperandKind::Move(place),
                                            ..
                                        }),
                                    ..
                                },
                            ..
                        } if matches!(
                            place.projections.as_slice(),
                            [bc::BytecodeProjection {
                                kind: bc::BytecodeProjectionKind::TupleField(_),
                                ..
                            }]
                        ) =>
                        {
                            Some((
                                function,
                                place.slot,
                                body.slots[place.slot.index() as usize].ty,
                            ))
                        }
                        _ => None,
                    })
            })
            .unwrap();
        let call_place = root_reuse.functions[function_index]
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                bc::BytecodeTerminatorKind::Invoke {
                    operation:
                        bc::BytecodeOperation {
                            kind: bc::BytecodeOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => arguments
                    .iter_mut()
                    .find_map(|argument| match &mut argument.value.kind {
                        bc::BytecodeOperandKind::Move(place)
                            if place.projections.is_empty() && place.ty == root_type =>
                        {
                            Some(place)
                        }
                        _ => None,
                    }),
                _ => None,
            })
            .expect("the rebuilt tuple is passed by value");
        call_place.slot = root;

        let error = bc::verify_bytecode(&root_reuse).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable"),
            "{error}"
        );
    }

    #[test]
    fn affine_destructuring_observation_and_guarded_consumption_execute() {
        let source = "type Box[T] = { left: T, right: T }\n\
                      type Token[T] = T\n\
                      enum Choice[T] {\n\
                          First(T)\n\
                          Second(T)\n\
                      }\n\
                      fn swapPair[T: Discard](pair: (T, T)): (T, T) {\n\
                          let (left, right) = pair\n\
                          (right, left)\n\
                      }\n\
                      fn swapBox[T: Discard](box: Box[T]): (T, T) {\n\
                          let Box { left, right } = box\n\
                          (right, left)\n\
                      }\n\
                      fn unwrapChoice[T: Discard](choice: Choice[T]): T {\n\
                          match choice {\n\
                              Choice.First(item) => item\n\
                              Choice.Second(item) => item\n\
                          }\n\
                      }\n\
                      fn first[T: Discard](values: Array[T]): T {\n\
                          match values {\n\
                              [item, ..rest] => {\n\
                                  _ = rest\n\
                                  item\n\
                              }\n\
                              [] => panic(\"empty array\")\n\
                          }\n\
                      }\n\
                      fn unwrap[T](token: Token[T]): T { token.value }\n\
                      fn preserve[T: Discard](value: T): T {\n\
                          match value {\n\
                              _ => ()\n\
                          }\n\
                          value\n\
                      }\n\
                      fn guarded[T: Discard](value: T?): T {\n\
                          match value {\n\
                              some(ref item) if false => panic(\"unreachable guard\")\n\
                              some(item) => item\n\
                              none => panic(\"missing value\")\n\
                          }\n\
                      }\n\
                      fn execute(): Int {\n\
                          let pair = swapPair((\"left\", \"right\"))\n\
                          assert(pair.0 == \"right\")\n\
                          assert(pair.1 == \"left\")\n\
                          let box = swapBox(Box[String] { left: \"up\", right: \"down\" })\n\
                          assert(box.0 == \"down\")\n\
                          assert(box.1 == \"up\")\n\
                          assert(unwrapChoice(Choice[String].First(\"choice\")) == \"choice\")\n\
                          assert(first([\"head\", \"tail\"]) == \"head\")\n\
                          assert(unwrap(Token[String](\"token\")) == \"token\")\n\
                          assert(Token[String](\"direct\").value == \"direct\")\n\
                          assert(preserve(\"preserved\") == \"preserved\")\n\
                          assert(guarded(some(\"guarded\")) == \"guarded\")\n\
                          42\n\
                      }\n";

        assert_eq!(
            execute_function(source, "execute"),
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
        let bc::BytecodeOperandKind::Borrow(place) = &replacement.as_ref().unwrap().kind else {
            panic!("slice assignment validation observes its replacement")
        };
        replacement.as_mut().unwrap().kind = bc::BytecodeOperandKind::Copy(place.clone());
        let error = execute(&program, entry, &mut host).unwrap_err();
        assert!(matches!(error, VmError::InvalidBytecode(_)));

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
        let capabilities = program
            .types
            .iter()
            .find_map(|ty| match &ty.kind {
                bc::BytecodeTypeKind::OpaqueResult { capabilities, .. } => Some(*capabilities),
                _ => None,
            })
            .expect("opaque result retains its published capability row");
        assert!(capabilities.discard);
        assert!(!capabilities.copy);
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

        let mut missing_discard = program.clone();
        let bc::BytecodeTypeKind::OpaqueResult { capabilities, .. } =
            &mut missing_discard.types[opaque].kind
        else {
            unreachable!()
        };
        capabilities.discard = false;
        let error = bc::verify_bytecode(&missing_discard).unwrap_err();
        assert!(error.message().contains("published capability set"));

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
                    ..
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
                      fn render[T: Discard + Display](value: T): String { value.display() }\n\
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
