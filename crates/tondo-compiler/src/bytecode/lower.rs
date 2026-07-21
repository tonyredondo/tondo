use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tondo_vm::bytecode as bc;

use super::{BytecodeError, BytecodeLoweringLimits};
use crate::hir::{
    HirCallableId, HirConstantValue, HirConstantValueKind, HirConstantVariantValue,
    HirNominalShape, HirProgram, HirTypeDeclarationKind, HirVariantPayload,
};
use crate::mir::{
    MirAggregateKind, MirBasicBlock, MirBlockKind, MirCallArgument, MirConstant, MirFunction,
    MirLocalKind, MirOperand, MirOperandKind, MirOperation, MirOperationKind, MirPlace, MirProgram,
    MirProjection, MirProjectionKind, MirRvalue, MirRvalueKind, MirStatement, MirStatementKind,
    MirTag, MirTerminator, MirTerminatorKind, verify_mir,
};
use crate::resolve::{MemberOwner, ResolvedProgram, SymbolId};
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

struct Monomorphization {
    interner: TypeInterner,
    callables: Vec<CallableInstance>,
    functions: Vec<CallableInstance>,
    type_maps: BTreeMap<CallableInstance, BTreeMap<TypeId, TypeId>>,
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

    let monomorphization = monomorphize(hir, mir, limits.max_generic_instantiations)?;
    let nominal_ids = nominal_ids(hir, limits.max_nominals)?;
    let callable_ids = callable_ids(&monomorphization.callables, limits.max_callables)?;
    let function_ids = function_ids(&monomorphization.functions, limits.max_functions)?;
    let constant_ids = constant_ids(hir, limits.max_constants)?;
    let mut catalog = TypeCatalog::build(
        &monomorphization.interner,
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
        &constant_ids,
    )?;
    let functions = {
        let context = FunctionLoweringContext {
            catalog: &catalog,
            nominal_ids: &nominal_ids,
            callable_ids: &callable_ids,
            constant_ids: &constant_ids,
        };
        monomorphization
            .functions
            .iter()
            .map(|instance| {
                let function = mir.function(instance.callable).ok_or_else(|| {
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
    hir: &HirProgram,
    mir: &MirProgram,
    generic_limit: u32,
) -> Result<Monomorphization, BytecodeError> {
    let mut interner = hir.interner().clone();
    let mut callables = BTreeSet::new();
    let mut functions = BTreeSet::new();
    let mut pending = BTreeSet::new();
    let mut generic_count = 0usize;

    for callable in hir
        .callables()
        .filter(|callable| callable.generic_arity() == 0)
    {
        register_instance(
            hir,
            mir,
            &interner,
            CallableInstance {
                callable: callable.id(),
                arguments: Vec::new(),
            },
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
            register_instance(
                hir,
                mir,
                &interner,
                CallableInstance {
                    callable,
                    arguments,
                },
                generic_limit,
                &mut generic_count,
                &mut callables,
                &mut functions,
                &mut pending,
            )?;
        }
    }

    while let Some(instance) = pending.pop_first() {
        let function = mir.function(instance.callable).ok_or_else(|| {
            BytecodeError::construction(
                "monomorphization",
                format!("{instance:?} has no MIR template"),
            )
        })?;
        let substitution = TypeSubstitution::new(instance.arguments.clone());
        let mut references = Vec::new();
        collect_function_references(function, &mut references);
        for (callable, templates) in references {
            let arguments = templates
                .into_iter()
                .map(|template| {
                    substitution
                        .apply(&mut interner, template)
                        .map_err(|error| {
                            monomorphization_type_error(
                                error,
                                Some(function.span()),
                                format!("cannot specialize {template}"),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            register_instance(
                hir,
                mir,
                &interner,
                CallableInstance {
                    callable,
                    arguments,
                },
                generic_limit,
                &mut generic_count,
                &mut callables,
                &mut functions,
                &mut pending,
            )?;
        }
    }

    let callables = callables.into_iter().collect::<Vec<_>>();
    let functions = functions.into_iter().collect::<Vec<_>>();
    let function_set = functions.iter().cloned().collect::<BTreeSet<_>>();
    let mut type_maps = BTreeMap::new();
    for instance in &callables {
        let signature = hir.callable(instance.callable).ok_or_else(|| {
            BytecodeError::construction(
                "monomorphization",
                format!("{instance:?} has no HIR signature"),
            )
        })?;
        let mut templates = BTreeSet::from([signature.outcome(), signature.function_type()]);
        for parameter in signature.parameters() {
            templates.insert(parameter.ty());
            if let Some(element) = parameter.variadic_element() {
                templates.insert(element);
            }
        }
        if function_set.contains(instance) {
            collect_function_types(
                mir.function(instance.callable)
                    .expect("registered function instances have a MIR template"),
                &mut templates,
            );
        }
        let substitution = TypeSubstitution::new(instance.arguments.clone());
        let mut map = BTreeMap::new();
        for template in templates {
            let concrete = substitution
                .apply(&mut interner, template)
                .map_err(|error| {
                    monomorphization_type_error(
                        error,
                        Some(signature.span()),
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
fn register_instance(
    hir: &HirProgram,
    mir: &MirProgram,
    interner: &TypeInterner,
    instance: CallableInstance,
    generic_limit: u32,
    generic_count: &mut usize,
    callables: &mut BTreeSet<CallableInstance>,
    functions: &mut BTreeSet<CallableInstance>,
    pending: &mut BTreeSet<CallableInstance>,
) -> Result<(), BytecodeError> {
    let signature = hir.callable(instance.callable).ok_or_else(|| {
        BytecodeError::construction(
            "monomorphization",
            format!("{:?} has no HIR signature", instance.callable),
        )
    })?;
    if instance.arguments.len() != signature.generic_arity() as usize {
        return Err(BytecodeError::construction(
            "monomorphization",
            format!(
                "{:?} expects {} type arguments, found {}",
                instance.callable,
                signature.generic_arity(),
                instance.arguments.len()
            ),
        ));
    }
    for argument in &instance.arguments {
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
    if signature.generic_arity() != 0 {
        *generic_count = generic_count
            .checked_add(1)
            .ok_or(BytecodeError::NodeLimit {
                span: Some(signature.span()),
                resource: "generic instantiations",
            })?;
        ensure_count(
            *generic_count,
            generic_limit,
            Some(signature.span()),
            "generic instantiations",
        )?;
    }
    if mir.function(instance.callable).is_some() {
        functions.insert(instance.clone());
        pending.insert(instance);
    }
    Ok(())
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
    instances: &[CallableInstance],
    limit: u32,
) -> Result<BTreeMap<CallableInstance, bc::BytecodeCallableId>, BytecodeError> {
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
    instances: &[CallableInstance],
    limit: u32,
) -> Result<BTreeMap<CallableInstance, bc::BytecodeFunctionId>, BytecodeError> {
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
}

impl TypeCatalog {
    fn build(
        interner: &TypeInterner,
        hir: &HirProgram,
        type_maps: &BTreeMap<CallableInstance, BTreeMap<TypeId, TypeId>>,
        limit: u32,
    ) -> Result<Self, BytecodeError> {
        let mut seeds = BTreeSet::new();
        collect_metadata_types(hir, &mut seeds);
        for map in type_maps.values() {
            seeds.extend(map.values().copied());
        }
        let mut queue = seeds.iter().copied().collect::<VecDeque<_>>();
        while let Some(ty) = queue.pop_front() {
            let kind = interner
                .kind(ty)
                .map_err(|error| BytecodeError::construction("type catalog", error.to_string()))?;
            for child in type_children(kind) {
                if seeds.insert(child) {
                    ensure_count(seeds.len(), limit, None, "type table")?;
                    queue.push_back(child);
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
            TypeKind::OpaqueResult(identity) => {
                bc::BytecodeTypeKind::OpaqueResult(identity.canonical_name())
            }
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
        | TypeKind::Generated { arguments, .. } => arguments.clone(),
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
        | TypeKind::Inference(_)
        | TypeKind::OpaqueResult(_) => Vec::new(),
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
        MirOperandKind::Copy(place) | MirOperandKind::Move(place) => {
            collect_place_types(place, types);
        }
        MirOperandKind::Function { arguments, .. } => types.extend(arguments.iter().copied()),
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
        MirRvalueKind::Aggregate { values, .. } => {
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
        MirOperationKind::Call { callee, arguments } => {
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

fn collect_function_references(
    function: &MirFunction,
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
) {
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
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
) {
    if let MirOperandKind::Function {
        callable,
        arguments,
    } = operand.kind()
    {
        references.push((*callable, arguments.clone()));
    }
}

fn collect_rvalue_function_references(
    value: &MirRvalue,
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
) {
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
        MirRvalueKind::Aggregate { values, .. } => {
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
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
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
        MirOperationKind::Call { callee, arguments } => {
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
    references: &mut Vec<(HirCallableId, Vec<TypeId>)>,
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
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    function_ids: &BTreeMap<CallableInstance, bc::BytecodeFunctionId>,
) -> Result<Vec<bc::BytecodeCallable>, BytecodeError> {
    let mut output = vec![None; callable_ids.len()];
    for instance in &monomorphization.callables {
        let callable = hir.callable(instance.callable).ok_or_else(|| {
            BytecodeError::construction("callable metadata", "missing HIR signature")
        })?;
        let type_map = monomorphization
            .type_maps
            .get(instance)
            .expect("every callable instance has a type map");
        let id = callable_ids.get(instance).copied().ok_or_else(|| {
            BytecodeError::construction("callable metadata", "missing callable ID")
        })?;
        let mut name = callable_name(resolved, callable.id());
        if !instance.arguments.is_empty() {
            let arguments = instance
                .arguments
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
            parameters: callable
                .parameters()
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
            outcome: mapped_catalog_id(callable.outcome(), type_map, catalog)?,
            function_type: mapped_catalog_id(callable.function_type(), type_map, catalog)?,
            implementation: function_ids.get(instance).copied(),
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
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
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
            value: lower_constant_value(value, catalog, nominal_ids, callable_ids)?,
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
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
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
            callable: map_callable_instance(
                &CallableInstance {
                    callable: *callable,
                    arguments: arguments.clone(),
                },
                callable_ids,
            )?,
            arguments: Vec::new(),
        },
        HirConstantValueKind::Tuple(values) => bc::BytecodeConstantValueKind::Tuple(
            lower_constant_values(values, catalog, nominal_ids, callable_ids)?,
        ),
        HirConstantValueKind::Array(values) => bc::BytecodeConstantValueKind::Array(
            lower_constant_values(values, catalog, nominal_ids, callable_ids)?,
        ),
        HirConstantValueKind::Map(entries) => bc::BytecodeConstantValueKind::Map(
            entries
                .iter()
                .map(|(key, value)| {
                    Ok((
                        lower_constant_value(key, catalog, nominal_ids, callable_ids)?,
                        lower_constant_value(value, catalog, nominal_ids, callable_ids)?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        ),
        HirConstantValueKind::Set(values) => bc::BytecodeConstantValueKind::Set(
            lower_constant_values(values, catalog, nominal_ids, callable_ids)?,
        ),
        HirConstantValueKind::Newtype { constructor, value } => {
            bc::BytecodeConstantValueKind::Newtype {
                nominal: map_nominal(*constructor, nominal_ids)?,
                value: Box::new(lower_constant_value(
                    value,
                    catalog,
                    nominal_ids,
                    callable_ids,
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
                        lower_constant_value(field.value(), catalog, nominal_ids, callable_ids)?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        },
        HirConstantValueKind::Variant { variant, payload } => {
            bc::BytecodeConstantValueKind::Variant {
                variant: variant.index(),
                payload: lower_constant_variant(payload, catalog, nominal_ids, callable_ids)?,
            }
        }
        HirConstantValueKind::OptionNone => bc::BytecodeConstantValueKind::OptionNone,
        HirConstantValueKind::OptionSome(value) => {
            bc::BytecodeConstantValueKind::OptionSome(Box::new(lower_constant_value(
                value,
                catalog,
                nominal_ids,
                callable_ids,
            )?))
        }
        HirConstantValueKind::ResultOk(value) => bc::BytecodeConstantValueKind::ResultOk(Box::new(
            lower_constant_value(value, catalog, nominal_ids, callable_ids)?,
        )),
        HirConstantValueKind::ResultErr(value) => {
            bc::BytecodeConstantValueKind::ResultErr(Box::new(lower_constant_value(
                value,
                catalog,
                nominal_ids,
                callable_ids,
            )?))
        }
        HirConstantValueKind::Range { kind, start, end } => bc::BytecodeConstantValueKind::Range {
            kind: range_kind(*kind),
            start: Box::new(lower_constant_value(
                start,
                catalog,
                nominal_ids,
                callable_ids,
            )?),
            end: Box::new(lower_constant_value(
                end,
                catalog,
                nominal_ids,
                callable_ids,
            )?),
        },
        HirConstantValueKind::Converted(value) => {
            lower_constant_value(value, catalog, nominal_ids, callable_ids)?.kind
        }
    };
    Ok(bc::BytecodeConstantValue { ty, kind })
}

fn lower_constant_values(
    values: &[HirConstantValue],
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
) -> Result<Vec<bc::BytecodeConstantValue>, BytecodeError> {
    values
        .iter()
        .map(|value| lower_constant_value(value, catalog, nominal_ids, callable_ids))
        .collect()
}

fn lower_constant_variant(
    payload: &HirConstantVariantValue,
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
) -> Result<bc::BytecodeConstantVariantValue, BytecodeError> {
    Ok(match payload {
        HirConstantVariantValue::Unit => bc::BytecodeConstantVariantValue::Unit,
        HirConstantVariantValue::Tuple(values) => bc::BytecodeConstantVariantValue::Tuple(
            lower_constant_values(values, catalog, nominal_ids, callable_ids)?,
        ),
        HirConstantVariantValue::Record(fields) => bc::BytecodeConstantVariantValue::Record(
            fields
                .iter()
                .map(|field| {
                    Ok((
                        field.member().index(),
                        lower_constant_value(field.value(), catalog, nominal_ids, callable_ids)?,
                    ))
                })
                .collect::<Result<_, BytecodeError>>()?,
        ),
    })
}

struct FunctionLoweringContext<'a> {
    catalog: &'a TypeCatalog,
    nominal_ids: &'a BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &'a BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &'a BTreeMap<SymbolId, bc::BytecodeConstantId>,
}

fn lower_function(
    instance: &CallableInstance,
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
        .map(|block| {
            lower_block(
                block,
                &span_ids,
                context.catalog,
                context.nominal_ids,
                context.callable_ids,
                context.constant_ids,
                type_map,
            )
        })
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
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
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
            .map(|statement| {
                lower_statement(
                    statement,
                    span_ids,
                    catalog,
                    nominal_ids,
                    callable_ids,
                    constant_ids,
                    type_map,
                )
            })
            .collect::<Result<_, BytecodeError>>()?,
        terminator: lower_terminator(
            block.terminator(),
            span_ids,
            catalog,
            callable_ids,
            constant_ids,
            type_map,
        )?,
    })
}

fn lower_statement(
    statement: &MirStatement,
    span_ids: &BTreeMap<bc::BytecodeSpan, bc::BytecodeSpanId>,
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
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
            destination: lower_place(destination, catalog, type_map)?,
            value: lower_rvalue(
                value,
                catalog,
                nominal_ids,
                callable_ids,
                constant_ids,
                type_map,
            )?,
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
    catalog: &TypeCatalog,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
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
            condition: lower_operand(condition, catalog, callable_ids, constant_ids, type_map)?,
            if_true: block_id(*if_true),
            if_false: block_id(*if_false),
        },
        MirTerminatorKind::SwitchTag {
            value,
            cases,
            otherwise,
        } => bc::BytecodeTerminatorKind::BranchTag {
            value: lower_operand(value, catalog, callable_ids, constant_ids, type_map)?,
            cases: cases
                .iter()
                .map(|(tag, target)| Ok((lower_tag(*tag, catalog, type_map)?, block_id(*target))))
                .collect::<Result<_, BytecodeError>>()?,
            otherwise: block_id(*otherwise),
        },
        MirTerminatorKind::Invoke {
            operation,
            destination,
            target,
            unwind,
        } => bc::BytecodeTerminatorKind::Invoke {
            operation: lower_operation(operation, catalog, callable_ids, constant_ids, type_map)?,
            destination: destination
                .as_ref()
                .map(|place| lower_place(place, catalog, type_map))
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
            state: lower_place(state, catalog, type_map)?,
            destination: lower_place(destination, catalog, type_map)?,
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
                .map(|place| lower_place(place, catalog, type_map))
                .collect::<Result<_, BytecodeError>>()?,
            replacements: replacements
                .iter()
                .map(|replacement| {
                    replacement
                        .as_ref()
                        .map(|replacement| {
                            lower_operand(
                                replacement,
                                catalog,
                                callable_ids,
                                constant_ids,
                                type_map,
                            )
                        })
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
    catalog: &TypeCatalog,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodePlace, BytecodeError> {
    Ok(bc::BytecodePlace {
        slot: bc::BytecodeSlotId::new(place.local().index()),
        ty: mapped_catalog_id(place.ty(), type_map, catalog)?,
        projections: place
            .projections()
            .iter()
            .map(|projection| lower_projection(projection, catalog, type_map))
            .collect::<Result<_, BytecodeError>>()?,
    })
}

fn lower_projection(
    projection: &MirProjection,
    catalog: &TypeCatalog,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeProjection, BytecodeError> {
    let kind = match projection.kind() {
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
        MirProjectionKind::UnionValue(member) => {
            bc::BytecodeProjectionKind::UnionValue(mapped_catalog_id(*member, type_map, catalog)?)
        }
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
        ty: mapped_catalog_id(projection.ty(), type_map, catalog)?,
        kind,
    })
}

fn lower_operand(
    operand: &MirOperand,
    catalog: &TypeCatalog,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeOperand, BytecodeError> {
    let kind = match operand.kind() {
        MirOperandKind::Constant(value) => {
            bc::BytecodeOperandKind::Constant(lower_immediate(value, constant_ids)?)
        }
        MirOperandKind::Copy(place) => {
            bc::BytecodeOperandKind::Copy(lower_place(place, catalog, type_map)?)
        }
        MirOperandKind::Move(place) => {
            bc::BytecodeOperandKind::Move(lower_place(place, catalog, type_map)?)
        }
        MirOperandKind::Function {
            callable,
            arguments,
        } => bc::BytecodeOperandKind::Function {
            callable: map_callable_instance(
                &CallableInstance {
                    callable: *callable,
                    arguments: arguments
                        .iter()
                        .map(|argument| mapped_type(*argument, type_map))
                        .collect::<Result<_, _>>()?,
                },
                callable_ids,
            )?,
            arguments: Vec::new(),
        },
    };
    Ok(bc::BytecodeOperand {
        ty: mapped_catalog_id(operand.ty(), type_map, catalog)?,
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
    catalog: &TypeCatalog,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeRvalue, BytecodeError> {
    let operand =
        |value: &MirOperand| lower_operand(value, catalog, callable_ids, constant_ids, type_map);
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
        MirRvalueKind::Aggregate { shape, values } => bc::BytecodeRvalueKind::Construct {
            shape: lower_aggregate(shape, nominal_ids)?,
            values: values
                .iter()
                .map(operand)
                .collect::<Result<_, BytecodeError>>()?,
        },
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
        ty: mapped_catalog_id(value.ty(), type_map, catalog)?,
        kind,
    })
}

fn lower_aggregate(
    shape: &MirAggregateKind,
    nominal_ids: &BTreeMap<SymbolId, bc::BytecodeNominalId>,
) -> Result<bc::BytecodeAggregateKind, BytecodeError> {
    Ok(match shape {
        MirAggregateKind::Tuple => bc::BytecodeAggregateKind::Tuple,
        MirAggregateKind::Array => bc::BytecodeAggregateKind::Array,
        MirAggregateKind::Set => bc::BytecodeAggregateKind::Set,
        MirAggregateKind::Newtype { owner } => bc::BytecodeAggregateKind::Newtype {
            nominal: map_nominal(*owner, nominal_ids)?,
        },
        MirAggregateKind::Record { owner, fields } => bc::BytecodeAggregateKind::Record {
            nominal: map_nominal(*owner, nominal_ids)?,
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
    catalog: &TypeCatalog,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
    type_map: &BTreeMap<TypeId, TypeId>,
) -> Result<bc::BytecodeOperation, BytecodeError> {
    let operand =
        |value: &MirOperand| lower_operand(value, catalog, callable_ids, constant_ids, type_map);
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
        MirOperationKind::Call { callee, arguments } => bc::BytecodeOperationKind::Call {
            callee: operand(callee)?,
            arguments: arguments
                .iter()
                .map(|argument| {
                    lower_call_argument(argument, catalog, callable_ids, constant_ids, type_map)
                })
                .collect::<Result<_, BytecodeError>>()?,
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
        ty: mapped_catalog_id(operation.ty(), type_map, catalog)?,
        kind,
    })
}

fn lower_call_argument(
    argument: &MirCallArgument,
    catalog: &TypeCatalog,
    callable_ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
    constant_ids: &BTreeMap<SymbolId, bc::BytecodeConstantId>,
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
        value: lower_operand(
            argument.value(),
            catalog,
            callable_ids,
            constant_ids,
            type_map,
        )?,
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
    instance: &CallableInstance,
    ids: &BTreeMap<CallableInstance, bc::BytecodeCallableId>,
) -> Result<bc::BytecodeCallableId, BytecodeError> {
    ids.get(instance).copied().ok_or_else(|| {
        BytecodeError::construction(
            "callable reference",
            format!("{instance:?} has no callable metadata"),
        )
    })
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

fn coercion(value: Assignability) -> bc::BytecodeCoercion {
    match value {
        Assignability::Exact => bc::BytecodeCoercion::Exact,
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
}
