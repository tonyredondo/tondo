//! Scalar storage for local value aggregates, before backend serialization.
//! Only Int/Bool leaves are admitted. The verified source MIR is immutable;
//! copies and projected replacements snapshot every RHS leaf before any write.

use std::rc::Rc;

use super::*;
use crate::types::TypeKind;

const MAX_ADDITIONAL_LOCALS: u32 = 65_536;
const MAX_LAYOUT_DEPTH: u32 = 64;

pub(super) type RecordFields = BTreeMap<TypeId, Vec<(MemberId, TypeId)>>;

pub(super) fn record_fields(program: &MirProgram) -> RecordFields {
    let mut records = BTreeMap::new();
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
}

struct Layout {
    ty: TypeId,
    width: u32,
    height: u32,
    children: Vec<(Field, Rc<Layout>)>,
}

impl Layout {
    fn build(
        ty: TypeId,
        interner: &TypeInterner,
        records: &RecordFields,
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
            TypeKind::Scalar(ScalarType::Int | ScalarType::Bool) => Vec::new(),
            TypeKind::Tuple(fields) if !fields.is_empty() => fields
                .iter()
                .enumerate()
                .map(|(index, ty)| (Field::Tuple(index as u32), *ty))
                .collect(),
            TypeKind::Nominal { .. } => records
                .get(&ty)?
                .iter()
                .map(|(member, ty)| (Field::Record(*member), *ty))
                .collect(),
            _ => return None,
        };
        if fields.is_empty() && !matches!(interner.kind(ty), Ok(TypeKind::Scalar(_))) {
            return None;
        }
        let mut children = Vec::new();
        let mut width = u32::from(fields.is_empty());
        let mut height = 0;
        for (field, ty) in fields {
            let child = Self::build(ty, interner, records, cache, depth + 1)?;
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
) -> LowerResult<Vec<MirBasicBlock>> {
    let next = u32::try_from(function.locals.len()).map_err(|_| "aggregate:local-limit")?;
    let mut locals = NativeLocals {
        storage: BTreeMap::new(),
        next,
        limit: next.saturating_add(MAX_ADDITIONAL_LOCALS),
    };
    let mut cache = BTreeMap::new();
    for (index, local) in function.locals.iter().enumerate() {
        let Some(layout) = Layout::build(local.ty, interner, records, &mut cache, 0) else {
            continue;
        };
        if layout.children.is_empty() {
            continue;
        }
        let first = locals.allocate(layout.width)?;
        locals.storage.insert(index as u32, (first, layout));
    }
    let mut blocks = function.blocks.clone();
    if locals.storage.is_empty() {
        return Ok(blocks);
    }
    for block in &mut blocks {
        let mut statements = Vec::new();
        for mut statement in std::mem::take(&mut block.statements) {
            if let MirStatementKind::Assign { destination, value } = &mut statement.kind {
                if let Some((first, layout)) = locals.resolve(destination)?
                    && !layout.children.is_empty()
                {
                    let width = layout.width;
                    let values = locals.assignment_values(value, layout)?;
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
                locals.rvalue(value)?;
            }
            statements.push(statement);
        }
        block.statements = statements;
        locals.terminator(&mut block.terminator.kind)?;
    }
    Ok(blocks)
}

fn assign(span: Span, destination: u32, operand: MirOperand) -> MirStatement {
    MirStatement {
        span,
        kind: MirStatementKind::Assign {
            destination: scalar_place(destination, operand.ty),
            value: MirRvalue {
                ty: operand.ty,
                kind: MirRvalueKind::Use(operand),
            },
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
    ) -> LowerResult<Vec<MirOperand>> {
        match &value.kind {
            MirRvalueKind::Use(operand) => self.values(operand),
            MirRvalueKind::Aggregate { shape, values } => {
                let mut leaves = Vec::with_capacity(layout.width as usize);
                for (key, child) in &layout.children {
                    let index = match (shape, key) {
                        (MirAggregateKind::Tuple, Field::Tuple(index)) => *index as usize,
                        (MirAggregateKind::Record { fields, .. }, Field::Record(member)) => fields
                            .iter()
                            .position(|field| field == member)
                            .ok_or("aggregate:field-shape")?,
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

    fn operation(&self, operation: &mut MirOperation) -> LowerResult<()> {
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
                for argument in arguments {
                    self.operand(&mut argument.value)?;
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
                self.operation(operation)?;
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
                    MirAwaitable::Call(operation) => self.operation(operation)?,
                    MirAwaitable::Join(value) => self.operand(value)?,
                }
                self.place(destination)?;
            }
            MirTerminatorKind::Spawn {
                operation,
                destination,
                ..
            } => {
                self.operation(operation)?;
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
