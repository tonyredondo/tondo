//! Scalar storage and private call carriers for value aggregates.
//! Numeric, Bool, Char and Unit leaves are admitted. Source MIR is immutable;
//! copies and projected replacements snapshot every RHS leaf before any write.

use std::rc::Rc;

use super::*;
use crate::types::TypeKind;

mod floats;
mod ranges;
mod sums;

pub(super) use ranges::is_native_range_element;

const MAX_ADDITIONAL_LOCALS: u32 = 65_536;
const MAX_LAYOUT_DEPTH: u32 = 64;

pub(super) type RecordFields = BTreeMap<TypeId, Vec<(MemberId, TypeId)>>;
pub(super) type EnumVariants = BTreeMap<TypeId, Vec<EnumVariant>>;

#[derive(Debug, Clone)]
pub(super) struct EnumVariant {
    pub member: MemberId,
    pub fields: Vec<(Option<MemberId>, TypeId)>,
}

pub(super) fn record_fields(program: &MirProgram) -> RecordFields {
    let mut records = program.record_fields.clone();
    for function in program.functions() {
        for block in &function.blocks {
            for statement in &block.statements {
                if let MirStatementKind::Assign { value, .. } = &statement.kind
                    && let MirRvalueKind::Aggregate {
                        shape: MirAggregateKind::Record { fields, .. },
                        values,
                    } = &value.kind
                {
                    // Verified MIR fixes declaration order and instantiated
                    // field types, including generic record instances.
                    records.entry(value.ty).or_insert_with(|| {
                        fields
                            .iter()
                            .copied()
                            .zip(values.iter().map(|v| v.ty))
                            .collect()
                    });
                }
            }
        }
    }
    records
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Tuple(u32),
    Record(MemberId),
    // Private carrier only; never a source record member or projection.
    EmptyRecord,
    Tag,
    OptionValue,
    ResultOk,
    ResultErr,
    NumericError,
    VariantTuple(MemberId, u32),
    VariantRecord(MemberId, MemberId),
    UnionValue(TypeId),
    RangeStart,
    RangeEnd,
    RangeInclusive,
    CursorSource,
    CursorCurrent,
    CursorActive,
}

struct Layout {
    ty: TypeId,
    width: u32,
    height: u32,
    children: Vec<(Field, Rc<Layout>)>,
    variants: Vec<MemberId>,
}

impl Layout {
    fn build(
        ty: TypeId,
        interner: &TypeInterner,
        records: &RecordFields,
        enums: &EnumVariants,
        cache: &mut BTreeMap<TypeId, Rc<Self>>,
        depth: u32,
    ) -> Option<Rc<Self>> {
        if depth > MAX_LAYOUT_DEPTH {
            return None;
        }
        if let Some(layout) = cache.get(&ty) {
            return (depth + layout.height <= MAX_LAYOUT_DEPTH).then(|| Rc::clone(layout));
        }
        let fields = match interner.kind(ty).ok()? {
            TypeKind::Scalar(scalar) if native_integers::is_value_scalar(*scalar) => Vec::new(),
            TypeKind::Tuple(fields) if !fields.is_empty() => fields
                .iter()
                .enumerate()
                .map(|(index, ty)| (Field::Tuple(index as u32), *ty))
                .collect(),
            TypeKind::Option(item) => vec![
                (Field::Tag, interner.scalar(ScalarType::Int)),
                (Field::OptionValue, *item),
            ],
            TypeKind::Result { success, error } => vec![
                (Field::Tag, interner.scalar(ScalarType::Int)),
                (Field::ResultOk, *success),
                (Field::ResultErr, *error),
            ],
            TypeKind::Intrinsic {
                constructor: crate::types::IntrinsicType::Range,
                arguments,
            } if arguments.len() == 1
                && ranges::is_native_range_element(arguments[0], interner) =>
            {
                vec![
                    (Field::RangeStart, arguments[0]),
                    (Field::RangeEnd, arguments[0]),
                    (Field::RangeInclusive, interner.scalar(ScalarType::Bool)),
                ]
            }
            TypeKind::Cursor {
                mode: crate::types::CursorMode::Own,
                collection,
            } => {
                let TypeKind::Intrinsic {
                    constructor: crate::types::IntrinsicType::Range,
                    arguments,
                } = interner.kind(*collection).ok()?
                else {
                    return None;
                };
                let [element] = arguments.as_slice() else {
                    return None;
                };
                if !ranges::is_native_range_element(*element, interner) {
                    return None;
                }
                vec![
                    (Field::CursorSource, *collection),
                    (Field::CursorCurrent, *element),
                    (Field::CursorActive, interner.scalar(ScalarType::Bool)),
                ]
            }
            TypeKind::Union(members) => {
                std::iter::once((Field::Tag, interner.scalar(ScalarType::Int)))
                    .chain(
                        members
                            .iter()
                            .map(|member| (Field::UnionValue(*member), *member)),
                    )
                    .collect()
            }
            TypeKind::Intrinsic {
                constructor: crate::types::IntrinsicType::NumericConversionError,
                arguments,
            } if arguments.is_empty() => {
                vec![(Field::NumericError, interner.scalar(ScalarType::Int))]
            }
            TypeKind::Nominal { .. } if enums.contains_key(&ty) => {
                let mut fields = vec![(Field::Tag, interner.scalar(ScalarType::Int))];
                for variant in &enums[&ty] {
                    for (index, (member, ty)) in variant.fields.iter().enumerate() {
                        let field = match member {
                            Some(member) => Field::VariantRecord(variant.member, *member),
                            None => Field::VariantTuple(variant.member, index as u32),
                        };
                        fields.push((field, *ty));
                    }
                }
                fields
            }
            TypeKind::Nominal { .. } => {
                let fields = records.get(&ty)?;
                if fields.is_empty() {
                    // Keep the existing nonempty private aggregate call protocol.
                    // Nominal identity remains on this layout and the source MIR.
                    vec![(Field::EmptyRecord, interner.scalar(ScalarType::Unit))]
                } else {
                    fields
                        .iter()
                        .map(|(member, ty)| (Field::Record(*member), *ty))
                        .collect()
                }
            }
            _ => return None,
        };
        if fields.is_empty() && !matches!(interner.kind(ty), Ok(TypeKind::Scalar(_))) {
            return None;
        }
        let mut children = Vec::new();
        let mut width = u32::from(fields.is_empty());
        let mut height = 0;
        for (field, ty) in fields {
            let child = Self::build(ty, interner, records, enums, cache, depth + 1)?;
            width = width
                .checked_add(child.width)
                .filter(|width| *width <= MAX_ADDITIONAL_LOCALS)?;
            height = height.max(1 + child.height);
            children.push((field, child));
        }
        let layout = Rc::new(Self {
            ty,
            width,
            height,
            children,
            variants: enums
                .get(&ty)
                .map(|variants| variants.iter().map(|variant| variant.member).collect())
                .unwrap_or_default(),
        });
        cache.insert(ty, Rc::clone(&layout));
        Some(layout)
    }

    fn child(&self, field: Field) -> Option<(u32, &Self)> {
        let mut offset = 0;
        for (key, layout) in &self.children {
            if *key == field {
                return Some((offset, layout));
            }
            offset += layout.width;
        }
        None
    }

    fn operands(&self, first: u32, values: &mut Vec<MirOperand>) {
        if self.children.is_empty() {
            values.push(MirOperand {
                ty: self.ty,
                kind: MirOperandKind::Copy(scalar_place(first, self.ty)),
            });
        } else {
            let mut offset = first;
            for (_, child) in &self.children {
                child.operands(offset, values);
                offset += child.width;
            }
        }
    }
}

struct NativeLocals {
    storage: BTreeMap<u32, (u32, Rc<Layout>)>,
    next: u32,
    limit: u32,
}

type LowerResult<T> = Result<T, &'static str>;

pub(super) struct LoweredFunction {
    pub blocks: Vec<MirBasicBlock>,
    pub parameters: Vec<u32>,
    pub aggregate_parameters: BTreeSet<u32>,
    pub return_fields: Vec<u32>,
    pub call_destinations: BTreeMap<u32, Vec<u32>>,
}

impl LoweredFunction {
    pub fn unchanged(function: &MirFunction) -> Self {
        Self {
            blocks: function.blocks.clone(),
            parameters: function.parameters.iter().map(|id| id.index()).collect(),
            aggregate_parameters: BTreeSet::new(),
            return_fields: Vec::new(),
            call_destinations: BTreeMap::new(),
        }
    }
}

fn scalar_place(index: u32, ty: TypeId) -> MirPlace {
    MirPlace {
        local: MirLocalId(index),
        ty,
        projections: Vec::new(),
        source_loan: None,
    }
}

pub(super) fn lower(
    function: &MirFunction,
    interner: &TypeInterner,
    records: &RecordFields,
    enums: &EnumVariants,
) -> LowerResult<LoweredFunction> {
    lower_with_limit(function, interner, records, enums, MAX_ADDITIONAL_LOCALS)
}

pub(super) fn lower_with_limit(
    function: &MirFunction,
    interner: &TypeInterner,
    records: &RecordFields,
    enums: &EnumVariants,
    additional_locals: u32,
) -> LowerResult<LoweredFunction> {
    let next = u32::try_from(function.locals.len()).map_err(|_| "aggregate:local-limit")?;
    let mut locals = NativeLocals {
        storage: BTreeMap::new(),
        next,
        limit: next.saturating_add(additional_locals.min(MAX_ADDITIONAL_LOCALS)),
    };
    let mut cache = BTreeMap::new();
    for (index, local) in function.locals.iter().enumerate() {
        let Some(layout) = Layout::build(local.ty, interner, records, enums, &mut cache, 0) else {
            continue;
        };
        if layout.children.is_empty() {
            continue;
        }
        let first = locals.allocate(layout.width)?;
        locals.storage.insert(index as u32, (first, layout));
    }
    let mut lowered = LoweredFunction::unchanged(function);
    lowered.parameters.clear();
    for parameter in &function.parameters {
        let local = &function.locals[parameter.index() as usize];
        if (native_float_width(local.ty, interner).is_some()
            || local.ty == interner.scalar(ScalarType::Char))
            && !matches!(
                local.kind,
                MirLocalKind::Parameter {
                    mode: ParameterMode::Value,
                    ..
                }
            )
        {
            return Err(if local.ty == interner.scalar(ScalarType::Char) {
                "char:parameter-mode"
            } else {
                "float:parameter-mode"
            });
        }
        if let Some((first, layout)) = locals.storage.get(&parameter.index()) {
            if !matches!(
                function.locals[parameter.index() as usize].kind,
                MirLocalKind::Parameter {
                    mode: ParameterMode::Value,
                    ..
                }
            ) {
                return Err("aggregate:parameter-mode");
            }
            lowered.parameters.extend(*first..*first + layout.width);
            lowered.aggregate_parameters.insert(parameter.index());
        } else {
            lowered.parameters.push(parameter.index());
        }
    }
    if let Some((first, layout)) = locals.storage.get(&function.return_local.index()) {
        lowered.return_fields.extend(*first..*first + layout.width);
    }
    locals.lower_checked_conversions(&mut lowered.blocks, interner)?;
    locals.lower_tags(&mut lowered.blocks, interner)?;
    let original_blocks =
        u32::try_from(lowered.blocks.len()).map_err(|_| "range:iterator-block-limit")?;
    let mut added_blocks = Vec::new();
    for (block_index, block) in lowered.blocks.iter_mut().enumerate() {
        let mut statements = Vec::new();
        for mut statement in std::mem::take(&mut block.statements) {
            if let MirStatementKind::Assign { destination, value } = &mut statement.kind {
                if let Some((first, layout)) = locals.resolve(destination)?
                    && !layout.children.is_empty()
                {
                    let width = layout.width;
                    let values = locals.assignment_values(value, layout, interner)?;
                    if values.len() != width as usize {
                        return Err("aggregate:assignment-shape");
                    }
                    let temporary = locals.allocate(width)?;
                    // Snapshot all sources first, including overlapping nested
                    // replacements and a value reassigned from its own fields.
                    for (index, value) in values.iter().enumerate() {
                        statements.push(assign(
                            statement.span,
                            temporary + index as u32,
                            value.clone(),
                        ));
                    }
                    for (index, value) in values.iter().enumerate() {
                        let source = MirOperand {
                            ty: value.ty,
                            kind: MirOperandKind::Copy(scalar_place(
                                temporary + index as u32,
                                value.ty,
                            )),
                        };
                        statements.push(assign(statement.span, first + index as u32, source));
                    }
                    continue;
                }
                locals.place(destination)?;
                locals.lower_equality(statement.span, value, &mut statements)?;
                locals.lower_contains(statement.span, value, &mut statements, interner)?;
                locals.rvalue(value)?;
            }
            statements.push(statement);
        }
        block.statements = statements;
        if let MirTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } = &mut block.terminator.kind
            && matches!(operation.kind, MirOperationKind::Call { .. })
            && let Some(layout) =
                Layout::build(operation.ty, interner, records, enums, &mut cache, 0)
            && !layout.children.is_empty()
        {
            let first = if let Some(place) = destination.as_ref() {
                let (first, actual) = locals.resolve(place)?.ok_or("aggregate:call-destination")?;
                if actual.ty != layout.ty {
                    return Err("aggregate:call-result-type");
                }
                first
            } else {
                // Even a discarded result needs private storage until the
                // callee completes. It never aliases any argument's fields.
                locals.allocate(layout.width)?
            };
            lowered
                .call_destinations
                .insert(block_index as u32, (first..first + layout.width).collect());
            *destination = None;
        }
        locals.lower_range_iterator_next(block, interner, original_blocks, &mut added_blocks)?;
        locals.terminator(&mut block.terminator.kind)?;
    }
    lowered.blocks.extend(added_blocks);
    native_integers::lower(&mut lowered.blocks, interner, &mut |width| {
        locals.allocate(width)
    })?;
    Ok(lowered)
}

fn assign(span: Span, destination: u32, operand: MirOperand) -> MirStatement {
    assign_rvalue(
        span,
        destination,
        MirRvalue {
            ty: operand.ty,
            kind: MirRvalueKind::Use(operand),
        },
    )
}

pub(super) fn assign_rvalue(span: Span, destination: u32, value: MirRvalue) -> MirStatement {
    MirStatement {
        span,
        kind: MirStatementKind::Assign {
            destination: scalar_place(destination, value.ty),
            value,
        },
    }
}

impl NativeLocals {
    fn allocate(&mut self, width: u32) -> LowerResult<u32> {
        let first = self.next;
        self.next = first
            .checked_add(width)
            .filter(|end| *end <= self.limit)
            .ok_or("aggregate:local-limit")?;
        Ok(first)
    }

    fn resolve(&self, place: &MirPlace) -> LowerResult<Option<(u32, &Layout)>> {
        let Some((first, layout)) = self.storage.get(&place.local.index()) else {
            return Ok(None);
        };
        if place.source_loan.is_some() {
            return Err("aggregate:loan-storage");
        }
        let mut first = *first;
        let mut layout = layout.as_ref();
        for projection in &place.projections {
            let key = match projection.kind {
                MirProjectionKind::TupleField(index) => Field::Tuple(index),
                MirProjectionKind::Field(member) => Field::Record(member),
                MirProjectionKind::OptionValue => Field::OptionValue,
                MirProjectionKind::ResultOkValue => Field::ResultOk,
                MirProjectionKind::ResultErrValue => Field::ResultErr,
                MirProjectionKind::UnionValue(member) => Field::UnionValue(member),
                MirProjectionKind::VariantTuple { variant, index } => {
                    Field::VariantTuple(variant, index)
                }
                MirProjectionKind::VariantField { variant, field } => {
                    Field::VariantRecord(variant, field)
                }
                _ => return Err("aggregate:projection-storage"),
            };
            let (offset, child) = layout.child(key).ok_or("aggregate:projection-shape")?;
            first += offset;
            layout = child;
        }
        if place.ty != layout.ty {
            return Err("aggregate:projection-type");
        }
        Ok(Some((first, layout)))
    }

    fn place(&self, place: &mut MirPlace) -> LowerResult<()> {
        if let Some((first, layout)) = self.resolve(place)? {
            if !layout.children.is_empty() {
                return Err("aggregate:whole-value-consumer");
            }
            *place = scalar_place(first, layout.ty);
        }
        Ok(())
    }

    fn operand(&self, operand: &mut MirOperand) -> LowerResult<()> {
        match &mut operand.kind {
            MirOperandKind::Copy(place)
            | MirOperandKind::Move(place)
            | MirOperandKind::Borrow(place) => self.place(place),
            _ => Ok(()),
        }
    }

    fn values(&self, operand: &MirOperand) -> LowerResult<Vec<MirOperand>> {
        if let MirOperandKind::Copy(place) | MirOperandKind::Move(place) = &operand.kind
            && let Some((first, layout)) = self.resolve(place)?
        {
            let mut values = Vec::with_capacity(layout.width as usize);
            layout.operands(first, &mut values);
            return Ok(values);
        }
        let mut operand = operand.clone();
        self.operand(&mut operand)?;
        Ok(vec![operand])
    }

    fn assignment_values(
        &self,
        value: &MirRvalue,
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Vec<MirOperand>> {
        match &value.kind {
            MirRvalueKind::Use(operand) => self.values(operand),
            MirRvalueKind::Range { kind, start, end } => {
                self.range_values(*kind, start, end, layout, interner)
            }
            MirRvalueKind::IteratorState { source } => {
                self.range_cursor_values(source, layout, interner)
            }
            MirRvalueKind::Coerce {
                kind: kind @ (Assignability::UnionInjection | Assignability::UnionWidening),
                value,
            } => self.union_values(*kind, value, layout, interner),
            MirRvalueKind::Coerce {
                kind: Assignability::OptionLift,
                value,
            } => self
                .sum_values(
                    &MirAggregateKind::OptionSome,
                    std::slice::from_ref(value),
                    layout,
                    interner,
                )?
                .ok_or("sum:option-lift-layout"),
            MirRvalueKind::Aggregate { shape, values } => {
                if let Some(leaves) = self.sum_values(shape, values, layout, interner)? {
                    return Ok(leaves);
                }
                let mut leaves = Vec::with_capacity(layout.width as usize);
                for (key, child) in &layout.children {
                    let index = match (shape, key) {
                        (MirAggregateKind::Tuple, Field::Tuple(index)) => *index as usize,
                        (MirAggregateKind::Record { fields, .. }, Field::Record(member)) => fields
                            .iter()
                            .position(|field| field == member)
                            .ok_or("aggregate:field-shape")?,
                        (MirAggregateKind::Record { fields, .. }, Field::EmptyRecord)
                            if fields.is_empty() && values.is_empty() =>
                        {
                            leaves.push(MirOperand {
                                ty: child.ty,
                                kind: MirOperandKind::Constant(MirConstant::Unit),
                            });
                            continue;
                        }
                        _ => return Err("aggregate:constructor-shape"),
                    };
                    let operand = values
                        .get(index)
                        .filter(|operand| operand.ty == child.ty)
                        .ok_or("aggregate:field-type")?;
                    leaves.extend(self.values(operand)?);
                }
                Ok(leaves)
            }
            MirRvalueKind::RecordUpdate { base, fields } => {
                let mut leaves = self.values(base)?;
                if leaves.len() != layout.width as usize {
                    return Err("aggregate:update-shape");
                }
                for (member, value) in fields {
                    let (offset, child) = layout
                        .child(Field::Record(*member))
                        .ok_or("aggregate:update-field")?;
                    let replacement = self.values(value)?;
                    if replacement.len() != child.width as usize || value.ty != child.ty {
                        return Err("aggregate:update-type");
                    }
                    leaves.splice(
                        offset as usize..(offset + child.width) as usize,
                        replacement,
                    );
                }
                Ok(leaves)
            }
            _ => Err("aggregate:assignment-storage"),
        }
    }

    fn lower_equality(
        &mut self,
        span: Span,
        value: &mut MirRvalue,
        statements: &mut Vec<MirStatement>,
    ) -> LowerResult<()> {
        let MirRvalueKind::Binary {
            operator,
            left,
            right,
        } = &value.kind
        else {
            return Ok(());
        };
        let combine = match operator {
            HirBinaryOperator::Equal => HirBinaryOperator::LogicalAnd,
            HirBinaryOperator::NotEqual => HirBinaryOperator::LogicalOr,
            _ => return Ok(()),
        };
        let MirOperandKind::Borrow(left_place) = &left.kind else {
            return Ok(());
        };
        let Some((left_first, layout)) = self.resolve(left_place)? else {
            return Ok(());
        };
        if layout.children.is_empty() {
            return Ok(());
        }
        let MirOperandKind::Borrow(right_place) = &right.kind else {
            return Err("aggregate:comparison-observation");
        };
        let (right_first, right_layout) = self
            .resolve(right_place)?
            .ok_or("aggregate:comparison-storage")?;
        if left.ty != right.ty || layout.ty != right_layout.ty {
            return Err("aggregate:comparison-type");
        }
        let width = layout.width;
        let mut left_values = Vec::with_capacity(width as usize);
        let mut right_values = Vec::with_capacity(width as usize);
        layout.operands(left_first, &mut left_values);
        right_layout.operands(right_first, &mut right_values);

        // One scratch local per leaf charges code expansion to the existing
        // budget. Commit the result only after all reads: its destination can
        // be a Bool field of either operand. Scalar leaf comparisons are pure.
        let first = self.allocate(width)?;
        let observed = |index| MirOperand {
            ty: value.ty,
            kind: MirOperandKind::Copy(scalar_place(index, value.ty)),
        };
        for (index, (left, right)) in left_values.into_iter().zip(right_values).enumerate() {
            let comparison = first + index as u32;
            statements.push(assign_rvalue(
                span,
                comparison,
                MirRvalue {
                    ty: value.ty,
                    kind: MirRvalueKind::Binary {
                        operator: *operator,
                        left,
                        right,
                    },
                },
            ));
            if index != 0 {
                statements.push(assign_rvalue(
                    span,
                    first,
                    MirRvalue {
                        ty: value.ty,
                        kind: MirRvalueKind::Binary {
                            operator: combine,
                            left: observed(first),
                            right: observed(comparison),
                        },
                    },
                ));
            }
        }
        value.kind = MirRvalueKind::Use(observed(first));
        Ok(())
    }

    fn rvalue(&self, value: &mut MirRvalue) -> LowerResult<()> {
        match &mut value.kind {
            MirRvalueKind::Use(value)
            | MirRvalueKind::Prefix { operand: value, .. }
            | MirRvalueKind::NumericConversion { value, .. }
            | MirRvalueKind::Coerce { value, .. }
            | MirRvalueKind::Length(value)
            | MirRvalueKind::IteratorState { source: value } => self.operand(value)?,
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
                self.operand(left)?;
                self.operand(right)?;
            }
            MirRvalueKind::Aggregate { values, .. } | MirRvalueKind::Interpolate { values, .. } => {
                for value in values {
                    self.operand(value)?;
                }
            }
            MirRvalueKind::RecordUpdate { base, fields } => {
                self.operand(base)?;
                for (_, value) in fields {
                    self.operand(value)?;
                }
            }
            MirRvalueKind::MapRemove { map, key } => {
                self.place(map)?;
                self.operand(key)?;
            }
        }
        Ok(())
    }

    fn operation(&self, operation: &mut MirOperation, direct: bool) -> LowerResult<()> {
        match &mut operation.kind {
            MirOperationKind::CheckedPrefix { operand, .. }
            | MirOperationKind::ExplicitPanic { message: operand } => self.operand(operand)?,
            MirOperationKind::CheckedBinary { left, right, .. }
            | MirOperationKind::ArraySequence {
                array: left,
                argument: right,
                ..
            }
            | MirOperationKind::Index {
                base: left,
                index: right,
                ..
            } => {
                self.operand(left)?;
                self.operand(right)?;
            }
            MirOperationKind::BuildMap { entries, .. } => {
                for (key, value) in entries {
                    self.operand(key)?;
                    self.operand(value)?;
                }
            }
            MirOperationKind::Slice { base, bounds, .. } => {
                self.operand(base)?;
                for value in [&mut bounds.start, &mut bounds.end, &mut bounds.step]
                    .into_iter()
                    .flatten()
                {
                    self.operand(value)?;
                }
            }
            MirOperationKind::Call {
                callee, arguments, ..
            } => {
                self.operand(callee)?;
                let mut has_aggregate = false;
                for argument in arguments.iter() {
                    if let MirOperandKind::Copy(place)
                    | MirOperandKind::Move(place)
                    | MirOperandKind::Borrow(place) = &argument.value.kind
                        && let Some((_, layout)) = self.resolve(place)?
                        && !layout.children.is_empty()
                    {
                        has_aggregate = true;
                        break;
                    }
                }
                if has_aggregate {
                    if !direct {
                        return Err("aggregate:async-call-storage");
                    }
                    // MIR has already evaluated expressions in source order.
                    // Assign the resulting carriers in declared parameter order.
                    let mut ordered = BTreeMap::new();
                    for argument in arguments.iter() {
                        let HirCallArgumentTarget::Fixed(index) = argument.target else {
                            return Err("aggregate:call-argument-target");
                        };
                        if argument.mode != ParameterMode::Value
                            || matches!(argument.value.kind, MirOperandKind::Borrow(_))
                        {
                            return Err("aggregate:call-argument-mode");
                        }
                        if ordered.insert(index, argument).is_some() {
                            return Err("aggregate:call-argument-duplicate");
                        }
                    }
                    let mut flattened = Vec::new();
                    for (expected, (index, argument)) in ordered.into_iter().enumerate() {
                        if index as usize != expected {
                            return Err("aggregate:call-argument-noncontiguous");
                        }
                        for value in self.values(&argument.value)? {
                            let index = u32::try_from(flattened.len())
                                .map_err(|_| "aggregate:call-argument-limit")?;
                            flattened.push(MirCallArgument {
                                mode: argument.mode,
                                target: HirCallArgumentTarget::Fixed(index),
                                value,
                            });
                        }
                    }
                    *arguments = flattened;
                } else {
                    for argument in arguments {
                        self.operand(&mut argument.value)?;
                    }
                }
            }
            MirOperationKind::Assert {
                condition,
                message_parts,
                ..
            } => {
                self.operand(condition)?;
                for part in message_parts {
                    self.operand(&mut part.value)?;
                }
            }
            MirOperationKind::Format { value, display } => {
                self.operand(value)?;
                if let Some(display) = display {
                    self.operand(display)?;
                }
            }
            MirOperationKind::JoinFormat {
                values,
                separator,
                display,
            } => {
                self.operand(values)?;
                self.operand(separator)?;
                if let Some(display) = display {
                    self.operand(display)?;
                }
            }
            MirOperationKind::BootstrapHostCall { arguments, .. } => {
                for argument in arguments {
                    self.operand(argument)?;
                }
            }
        }
        Ok(())
    }

    fn terminator(&self, terminator: &mut MirTerminatorKind) -> LowerResult<()> {
        match terminator {
            MirTerminatorKind::SwitchBool {
                condition: value, ..
            }
            | MirTerminatorKind::SwitchTag { value, .. } => self.operand(value)?,
            MirTerminatorKind::Invoke {
                operation,
                destination,
                ..
            } => {
                self.operation(operation, true)?;
                if let Some(destination) = destination {
                    self.place(destination)?;
                }
            }
            MirTerminatorKind::Await {
                awaitable,
                destination,
                ..
            } => {
                match awaitable {
                    MirAwaitable::Call(operation) => self.operation(operation, false)?,
                    MirAwaitable::Join(value) => self.operand(value)?,
                }
                self.place(destination)?;
            }
            MirTerminatorKind::Spawn {
                operation,
                destination,
                ..
            } => {
                self.operation(operation, false)?;
                self.place(destination)?;
            }
            MirTerminatorKind::ValidatePlaces {
                places, against, ..
            } => {
                for place in places {
                    if let Some((first, layout)) = self.resolve(place)? {
                        if against.iter().any(|loans| !loans.is_empty()) {
                            return Err("aggregate:loan-storage");
                        }
                        // A value-only local cannot alias another local. This
                        // validation edge has no dynamic range or loan checks.
                        *place = scalar_place(first, layout.ty);
                    }
                }
            }
            // These edges already have explicit rejection in backend_block.
            MirTerminatorKind::IteratorNext { .. }
            | MirTerminatorKind::ValidateLoan { .. }
            | MirTerminatorKind::DrainDefers { .. }
            | MirTerminatorKind::DrainScopes { .. }
            | MirTerminatorKind::DrainUnwind { .. }
            | MirTerminatorKind::CommitSelect { .. }
            | MirTerminatorKind::Goto { .. }
            | MirTerminatorKind::Return
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
        }
        Ok(())
    }
}
