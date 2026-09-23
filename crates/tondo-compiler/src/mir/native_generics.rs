//! Bounded native instances of verified generic bodies. Source MIR and its
//! interner stay immutable; recursive calls reuse the same concrete instance.

use std::borrow::Cow;

use super::*;
use crate::types::{TypeKind, TypeSubstitution};

const MAX_INSTANCES: usize = 1_024;
const MAX_BODY_NODES: usize = 1_000_000;
const MAX_TYPE_DEPTH: usize = 64;
const MAX_TYPE_NODES: usize = 4_096;
const MAX_NEW_TYPES: usize = 65_536;

pub(super) type CallableOrdinals = BTreeMap<HirCallableId, BTreeMap<Vec<TypeId>, u32>>;
type Result<T> = std::result::Result<T, &'static str>;

pub(super) struct Instance<'a> {
    pub function: Cow<'a, MirFunction>,
    pub generics: Option<MirBackendGenerics>,
    pub rejection: Option<&'static str>,
}

pub(super) struct NativeProgram<'a> {
    pub instances: Vec<Instance<'a>>,
    pub interner: Cow<'a, TypeInterner>,
    pub records: native_aggregates::RecordFields,
    pub enums: native_aggregates::EnumVariants,
    pub ordinals: CallableOrdinals,
}

pub(super) fn specialize<'a>(
    program: &'a MirProgram,
    interner: &'a TypeInterner,
) -> NativeProgram<'a> {
    specialize_with_limits(program, interner, MAX_INSTANCES, MAX_BODY_NODES)
}

pub(super) fn specialize_with_limits<'a>(
    program: &'a MirProgram,
    interner: &'a TypeInterner,
    instance_limit: usize,
    node_limit: usize,
) -> NativeProgram<'a> {
    let mut native = NativeProgram {
        instances: Vec::new(),
        interner: Cow::Borrowed(interner),
        records: native_aggregates::record_fields(program),
        enums: program.enum_variants.clone(),
        ordinals: BTreeMap::new(),
    };
    let templates = program
        .functions
        .keys()
        .enumerate()
        .map(|(ordinal, id)| (*id, ordinal as u32))
        .collect::<BTreeMap<_, _>>();
    for function in program.functions() {
        let ordinal = native.instances.len() as u32;
        let generics = (function.generic_arity > 0).then_some(MirBackendGenerics::Template {
            arity: function.generic_arity,
        });
        if function.generic_arity == 0
            && let MirFunctionId::Callable(id) = function.id
        {
            native
                .ordinals
                .entry(id)
                .or_default()
                .insert(Vec::new(), ordinal);
        }
        native.instances.push(Instance {
            function: Cow::Borrowed(function),
            generics,
            rejection: None,
        });
    }
    let original_count = native.instances.len();
    let mut body_nodes = 0;
    let mut cursor = 0;
    while cursor < native.instances.len() {
        if matches!(
            native.instances[cursor].generics,
            Some(MirBackendGenerics::Template { .. })
        ) || native.instances[cursor].rejection.is_some()
        {
            cursor += 1;
            continue;
        }
        for (callable, arguments) in function_references(&native.instances[cursor].function) {
            if arguments.is_empty()
                || native
                    .ordinals
                    .get(&callable)
                    .is_some_and(|m| m.contains_key(&arguments))
            {
                continue;
            }
            let Some(function) = program.function(callable) else {
                continue;
            };
            let nodes = function.locals.len().saturating_add(
                function
                    .blocks
                    .iter()
                    .map(|block| block.statements.len().saturating_add(1))
                    .sum::<usize>(),
            );
            let rejection = if function.generic_arity as usize != arguments.len() {
                Some("generic:arity")
            } else if native.instances.len() - original_count >= instance_limit {
                Some("generic:instance-limit")
            } else if nodes > node_limit.saturating_sub(body_nodes) {
                Some("generic:body-limit")
            } else {
                arguments
                    .iter()
                    .find_map(|ty| concrete_type(&native.interner, *ty, 0).err())
            };
            if let Some(reason) = rejection {
                native.instances[cursor].rejection = Some(reason);
                continue;
            }
            body_nodes += nodes;
            let ordinal = native.instances.len() as u32;
            // Install the key before scanning its body: self and mutual recursion
            // must find this instance instead of expanding it again.
            native
                .ordinals
                .entry(callable)
                .or_default()
                .insert(arguments.clone(), ordinal);
            let names = arguments
                .iter()
                .map(|ty| backend_type_name(&native.interner, *ty))
                .collect();
            let mut body = function.clone();
            let mut substitution = Substitute {
                arguments: TypeSubstitution::new(arguments),
                interner: native.interner.to_mut(),
                cache: BTreeMap::new(),
                records: &mut native.records,
                declarations: &program.record_fields,
                enums: &mut native.enums,
                enum_declarations: &program.enum_variants,
                visiting: BTreeSet::new(),
                expanded: BTreeSet::new(),
                type_limit: interner.len().saturating_add(MAX_NEW_TYPES),
            };
            let rejection = substitution.function(&mut body).err();
            native.instances.push(Instance {
                function: Cow::Owned(body),
                generics: Some(MirBackendGenerics::Instance {
                    template: templates[&function.id],
                    arguments: names,
                }),
                rejection,
            });
        }
        cursor += 1;
    }
    if !program.enum_variants.is_empty() || !program.record_fields.is_empty() {
        // Ordinary functions may use generic records or enums inside unions
        // without constructing every member. Populate all payload declarations.
        let mut substitution = Substitute {
            arguments: TypeSubstitution::new(Vec::new()),
            interner: native.interner.to_mut(),
            cache: BTreeMap::new(),
            records: &mut native.records,
            declarations: &program.record_fields,
            enums: &mut native.enums,
            enum_declarations: &program.enum_variants,
            visiting: BTreeSet::new(),
            expanded: BTreeSet::new(),
            type_limit: interner.len().saturating_add(MAX_NEW_TYPES),
        };
        for instance in &mut native.instances {
            if instance.rejection.is_some()
                || matches!(instance.generics, Some(MirBackendGenerics::Template { .. }))
            {
                continue;
            }
            instance.rejection = instance
                .function
                .locals
                .iter()
                .find_map(|local| substitution.record(local.ty, 0).err());
        }
    }
    native
}

fn function_references(function: &MirFunction) -> Vec<(HirCallableId, Vec<TypeId>)> {
    fn operand(value: &MirOperand, references: &mut Vec<(HirCallableId, Vec<TypeId>)>) {
        if let MirOperandKind::Function {
            callable,
            arguments,
        } = &value.kind
        {
            references.push((*callable, arguments.clone()));
        }
    }
    let mut references = Vec::new();
    for block in &function.blocks {
        for statement in &block.statements {
            if let MirStatementKind::Assign { value, .. } = &statement.kind
                && let MirRvalueKind::Use(value) = &value.kind
            {
                operand(value, &mut references);
            }
        }
        if let MirTerminatorKind::Invoke { operation, .. } = &block.terminator.kind
            && let MirOperationKind::Call { callee, .. } = &operation.kind
        {
            operand(callee, &mut references);
        }
    }
    references
}

/// Bound the expanded shape as well as depth: repeated tuple children can have
/// tiny interned DAGs but exponentially large canonical names and layouts.
fn concrete_type(interner: &TypeInterner, ty: TypeId, depth: usize) -> Result<usize> {
    if depth > MAX_TYPE_DEPTH {
        return Err("generic:type-depth");
    }
    let children = match interner.kind(ty).map_err(|_| "generic:invalid-type")? {
        TypeKind::Scalar(_) => return Ok(1),
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Intrinsic { arguments, .. }
        | TypeKind::Union(arguments)
        | TypeKind::OpaqueResult { arguments, .. }
        | TypeKind::Generated { arguments, .. } => arguments.clone(),
        TypeKind::Option(item) => vec![*item],
        TypeKind::Result { success, error } => vec![*success, *error],
        TypeKind::Function(signature) => signature
            .parameters()
            .iter()
            .map(|p| p.ty())
            .chain(signature.variadic())
            .chain([signature.outcome()])
            .collect(),
        TypeKind::Cursor { collection, .. } => vec![*collection],
        TypeKind::GenericParameter(_) | TypeKind::Inference(_) | TypeKind::Error => {
            return Err("generic:unresolved-type");
        }
    };
    let mut nodes = 1;
    for child in children {
        nodes += concrete_type(interner, child, depth + 1)?;
        if nodes > MAX_TYPE_NODES {
            return Err("generic:type-size");
        }
    }
    Ok(nodes)
}

struct Substitute<'a> {
    arguments: TypeSubstitution,
    interner: &'a mut TypeInterner,
    cache: BTreeMap<TypeId, TypeId>,
    records: &'a mut native_aggregates::RecordFields,
    declarations: &'a native_aggregates::RecordFields,
    enums: &'a mut native_aggregates::EnumVariants,
    enum_declarations: &'a native_aggregates::EnumVariants,
    visiting: BTreeSet<TypeId>,
    expanded: BTreeSet<TypeId>,
    type_limit: usize,
}

impl Substitute<'_> {
    fn ty(&mut self, ty: &mut TypeId) -> Result<()> {
        if let Some(concrete) = self.cache.get(ty) {
            *ty = *concrete;
            return Ok(());
        }
        let concrete = self
            .arguments
            .apply(self.interner, *ty)
            .map_err(|_| "generic:substitution")?;
        concrete_type(self.interner, concrete, 0)?;
        self.record(concrete, 0)?;
        self.check_type_limit()?;
        self.cache.insert(*ty, concrete);
        *ty = concrete;
        Ok(())
    }

    fn check_type_limit(&self) -> Result<()> {
        if self.interner.len() > self.type_limit {
            Err("generic:type-limit")
        } else {
            Ok(())
        }
    }

    fn record(&mut self, ty: TypeId, depth: usize) -> Result<()> {
        if depth > MAX_TYPE_DEPTH {
            return Err("generic:record-depth");
        }
        if self.expanded.contains(&ty) || !self.visiting.insert(ty) {
            return Ok(());
        }
        let result = self.record_inner(ty, depth);
        self.visiting.remove(&ty);
        if result.is_ok() {
            self.expanded.insert(ty);
        }
        result
    }

    fn record_inner(&mut self, ty: TypeId, depth: usize) -> Result<()> {
        let kind = self
            .interner
            .kind(ty)
            .map_err(|_| "generic:invalid-type")?
            .clone();
        match kind {
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                if let Some(fields) = self.records.get(&ty).cloned() {
                    for (_, field) in fields {
                        self.record(field, depth + 1)?;
                    }
                    return Ok(());
                }
                let variants = self.enums.get(&ty).cloned().or_else(|| {
                    self.enum_declarations.iter().find_map(|(key, variants)| {
                        match self.interner.kind(*key).ok()? {
                            TypeKind::Nominal {
                                identity: candidate,
                                ..
                            } if *candidate == identity => Some(variants.clone()),
                            _ => None,
                        }
                    })
                });
                if let Some(mut variants) = variants {
                    let substitution = TypeSubstitution::new(arguments);
                    for variant in &mut variants {
                        for (_, field) in &mut variant.fields {
                            *field = substitution
                                .apply(self.interner, *field)
                                .map_err(|_| "generic:enum-substitution")?;
                            concrete_type(self.interner, *field, 0)?;
                            self.check_type_limit()?;
                            self.record(*field, depth + 1)?;
                        }
                    }
                    self.enums.insert(ty, variants);
                    return Ok(());
                }
                let declaration = self.declarations.iter().find_map(|(key, fields)| {
                    match self.interner.kind(*key).ok()? {
                        TypeKind::Nominal {
                            identity: candidate,
                            ..
                        } if *candidate == identity => Some(fields.clone()),
                        _ => None,
                    }
                });
                if let Some(mut fields) = declaration {
                    let substitution = TypeSubstitution::new(arguments);
                    for (_, field) in &mut fields {
                        *field = substitution
                            .apply(self.interner, *field)
                            .map_err(|_| "generic:record-substitution")?;
                        concrete_type(self.interner, *field, 0)?;
                        self.check_type_limit()?;
                        self.record(*field, depth + 1)?;
                    }
                    self.records.insert(ty, fields);
                }
            }
            TypeKind::Tuple(fields) | TypeKind::Union(fields) => {
                for field in fields {
                    self.record(field, depth + 1)?;
                }
            }
            TypeKind::Option(item) => self.record(item, depth + 1)?,
            TypeKind::Result { success, error } => {
                self.record(success, depth + 1)?;
                self.record(error, depth + 1)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn function(&mut self, function: &mut MirFunction) -> Result<()> {
        if !function.loans.is_empty()
            || function.parameters.iter().any(|id| {
                !matches!(
                    function.locals[id.index() as usize].kind,
                    MirLocalKind::Parameter {
                        mode: ParameterMode::Value,
                        ..
                    }
                )
            })
        {
            return Err("generic:parameter-mode");
        }
        self.ty(&mut function.outcome)?;
        for local in &mut function.locals {
            self.ty(&mut local.ty)?;
        }
        for ty in std::iter::once(function.outcome).chain(
            function
                .parameters
                .iter()
                .map(|id| function.locals[id.index() as usize].ty),
        ) {
            self.value_type(ty, 0)?;
        }
        for block in &mut function.blocks {
            for statement in &mut block.statements {
                match &mut statement.kind {
                    MirStatementKind::StorageLive(_) | MirStatementKind::StorageDead(_) => {}
                    MirStatementKind::Assign { destination, value } => {
                        self.place(destination)?;
                        self.rvalue(value)?;
                    }
                    MirStatementKind::RetargetCleanup { from, to } => {
                        self.place(from)?;
                        self.place(to)?;
                    }
                    MirStatementKind::DisarmCleanup(place) => self.place(place)?,
                    _ => return Err("generic:statement-protocol"),
                }
            }
            match &mut block.terminator.kind {
                MirTerminatorKind::Goto { .. }
                | MirTerminatorKind::Return
                | MirTerminatorKind::ResumePanic
                | MirTerminatorKind::Unreachable
                | MirTerminatorKind::DrainUnwind { .. }
                | MirTerminatorKind::DrainDefers { .. }
                | MirTerminatorKind::DrainScopes { .. } => {}
                MirTerminatorKind::SwitchBool { condition, .. } => self.operand(condition)?,
                MirTerminatorKind::SwitchTag { value, cases, .. } => {
                    self.operand(value)?;
                    for (tag, _) in cases {
                        if let MirTag::Union(member) = tag {
                            self.ty(member)?;
                        }
                    }
                }
                MirTerminatorKind::IteratorNext {
                    state,
                    destination,
                    borrowed_source,
                    exhaustion_guard,
                    ..
                } => {
                    if borrowed_source.is_some() || exhaustion_guard.is_some() {
                        return Err("generic:iterator-protocol");
                    }
                    self.place(state)?;
                    self.value_type(state.ty, 0)?;
                    self.place(destination)?;
                }
                MirTerminatorKind::ValidatePlaces {
                    places,
                    replacements,
                    against,
                    ..
                } => {
                    if against.iter().any(|loans| !loans.is_empty()) {
                        return Err("generic:loan");
                    }
                    for place in places {
                        self.place(place)?;
                    }
                    for replacement in replacements.iter_mut().flatten() {
                        self.operand(replacement)?;
                    }
                }
                MirTerminatorKind::Invoke {
                    operation,
                    destination,
                    ..
                } => {
                    self.operation(operation)?;
                    if let Some(place) = destination {
                        self.place(place)?;
                    }
                }
                _ => return Err("generic:terminator-protocol"),
            }
        }
        function.generic_arity = 0;
        Ok(())
    }

    fn value_type(&self, ty: TypeId, depth: usize) -> Result<()> {
        if depth > MAX_TYPE_DEPTH {
            return Err("generic:record-depth");
        }
        match self.interner.kind(ty).map_err(|_| "generic:invalid-type")? {
            TypeKind::Scalar(scalar) if native_integers::is_value_scalar(*scalar) => Ok(()),
            TypeKind::Option(item) => self.value_type(*item, depth + 1),
            TypeKind::Result { success, error } => {
                self.value_type(*success, depth + 1)?;
                self.value_type(*error, depth + 1)
            }
            TypeKind::Intrinsic {
                constructor: crate::types::IntrinsicType::NumericConversionError,
                arguments,
            } if arguments.is_empty() => Ok(()),
            TypeKind::Intrinsic {
                constructor: crate::types::IntrinsicType::Range,
                arguments,
            } if arguments.len() == 1
                && native_aggregates::is_native_range_element(arguments[0], self.interner) =>
            {
                Ok(())
            }
            TypeKind::Cursor {
                mode: crate::types::CursorMode::Own,
                collection,
            } => self.value_type(*collection, depth + 1),
            TypeKind::Tuple(fields) | TypeKind::Union(fields) if !fields.is_empty() => {
                for field in fields {
                    self.value_type(*field, depth + 1)?;
                }
                Ok(())
            }
            TypeKind::Nominal { .. } => {
                if let Some(variants) = self.enums.get(&ty) {
                    for variant in variants {
                        for (_, field) in &variant.fields {
                            self.value_type(*field, depth + 1)?;
                        }
                    }
                    return Ok(());
                }
                let fields = self.records.get(&ty).ok_or("generic:value-storage")?;
                for (_, field) in fields {
                    self.value_type(*field, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("generic:value-storage"),
        }
    }

    fn place(&mut self, place: &mut MirPlace) -> Result<()> {
        self.ty(&mut place.ty)?;
        for projection in &mut place.projections {
            self.ty(&mut projection.ty)?;
            if let MirProjectionKind::UnionValue(ty) = &mut projection.kind {
                self.ty(ty)?;
            }
        }
        Ok(())
    }

    fn operand(&mut self, operand: &mut MirOperand) -> Result<()> {
        self.ty(&mut operand.ty)?;
        match &mut operand.kind {
            MirOperandKind::Copy(place)
            | MirOperandKind::Move(place)
            | MirOperandKind::Borrow(place) => self.place(place),
            MirOperandKind::Function { arguments, .. }
            | MirOperandKind::PreludeTraitFunction { arguments, .. } => {
                for argument in arguments {
                    self.ty(argument)?;
                }
                Ok(())
            }
            MirOperandKind::Constant(_) => Ok(()),
            MirOperandKind::Loan(_) => Err("generic:loan"),
        }
    }

    fn rvalue(&mut self, value: &mut MirRvalue) -> Result<()> {
        self.ty(&mut value.ty)?;
        match &mut value.kind {
            MirRvalueKind::Use(operand)
            | MirRvalueKind::Prefix { operand, .. }
            | MirRvalueKind::Coerce { value: operand, .. }
            | MirRvalueKind::NumericConversion { value: operand, .. }
            | MirRvalueKind::IteratorState { source: operand } => self.operand(operand),
            MirRvalueKind::Binary { left, right, .. }
            | MirRvalueKind::Range {
                start: left,
                end: right,
                ..
            } => {
                self.operand(left)?;
                self.operand(right)
            }
            MirRvalueKind::Aggregate {
                shape:
                    MirAggregateKind::Tuple
                    | MirAggregateKind::Record { .. }
                    | MirAggregateKind::Variant { .. }
                    | MirAggregateKind::OptionNone
                    | MirAggregateKind::OptionSome
                    | MirAggregateKind::ResultOk
                    | MirAggregateKind::ResultErr
                    | MirAggregateKind::NumericConversionError(_),
                values,
            } => {
                for value in values {
                    self.operand(value)?;
                }
                Ok(())
            }
            MirRvalueKind::RecordUpdate { base, fields } => {
                self.operand(base)?;
                for (_, value) in fields {
                    self.operand(value)?;
                }
                Ok(())
            }
            _ => Err("generic:rvalue-protocol"),
        }
    }

    fn operation(&mut self, operation: &mut MirOperation) -> Result<()> {
        self.ty(&mut operation.ty)?;
        match &mut operation.kind {
            MirOperationKind::CheckedPrefix { operand, .. } => self.operand(operand),
            MirOperationKind::CheckedBinary { left, right, .. } => {
                self.operand(left)?;
                self.operand(right)
            }
            MirOperationKind::Call {
                callee,
                arguments,
                signature,
                protocol: HirCallProtocol::Call,
                unsafe_call: false,
            } => {
                self.ty(signature)?;
                self.operand(callee)?;
                for argument in arguments {
                    self.operand(&mut argument.value)?;
                }
                Ok(())
            }
            MirOperationKind::ExplicitPanic { message } => self.operand(message),
            _ => Err("generic:operation-protocol"),
        }
    }
}
