//! Private scalar storage for discrete ranges and membership observations.

use super::*;

pub(in crate::mir) fn is_native_range_element(ty: TypeId, interner: &TypeInterner) -> bool {
    matches!(
        interner.kind(ty),
        Ok(TypeKind::Scalar(
            ScalarType::Int
                | ScalarType::Int8
                | ScalarType::Int16
                | ScalarType::Int32
                | ScalarType::UInt8
                | ScalarType::UInt16
                | ScalarType::UInt32
                | ScalarType::UInt64
                | ScalarType::Char
        ))
    )
}

fn read(index: u32, ty: TypeId) -> MirOperand {
    MirOperand {
        ty,
        kind: MirOperandKind::Copy(scalar_place(index, ty)),
    }
}

fn binary(
    locals: &mut NativeLocals,
    span: Span,
    statements: &mut Vec<MirStatement>,
    operator: HirBinaryOperator,
    left: MirOperand,
    right: MirOperand,
    boolean: TypeId,
) -> LowerResult<MirOperand> {
    let index = locals.allocate(1)?;
    statements.push(assign_rvalue(
        span,
        index,
        MirRvalue {
            ty: boolean,
            kind: MirRvalueKind::Binary {
                operator,
                left,
                right,
            },
        },
    ));
    Ok(read(index, boolean))
}

impl NativeLocals {
    pub(super) fn range_values(
        &self,
        kind: HirRangeKind,
        start: &MirOperand,
        end: &MirOperand,
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Vec<MirOperand>> {
        let TypeKind::Intrinsic {
            constructor: crate::types::IntrinsicType::Range,
            arguments,
        } = interner.kind(layout.ty).map_err(|_| "range:invalid-type")?
        else {
            return Err("range:layout");
        };
        let [item] = arguments.as_slice() else {
            return Err("range:arity");
        };
        if !is_native_range_element(*item, interner)
            || start.ty != *item
            || end.ty != *item
            || layout.width != 3
        {
            return Err("range:element-type");
        }
        let mut starts = self.values(start)?;
        let mut ends = self.values(end)?;
        if starts.len() != 1 || ends.len() != 1 {
            return Err("range:bound-shape");
        }
        starts.append(&mut ends);
        starts.push(MirOperand {
            ty: interner.scalar(ScalarType::Bool),
            kind: MirOperandKind::Constant(MirConstant::Bool(kind == HirRangeKind::Inclusive)),
        });
        Ok(starts)
    }

    pub(super) fn lower_contains(
        &mut self,
        span: Span,
        value: &mut MirRvalue,
        statements: &mut Vec<MirStatement>,
        interner: &TypeInterner,
    ) -> LowerResult<()> {
        let MirRvalueKind::Contains {
            kind: HirContainmentKind::Range,
            item,
            container,
        } = &value.kind
        else {
            return Ok(());
        };
        let MirOperandKind::Borrow(place) = &container.kind else {
            return Err("range:container-observation");
        };
        let (start, end, inclusive, element) = {
            let (first, layout) = self.resolve(place)?.ok_or("range:container-storage")?;
            let TypeKind::Intrinsic {
                constructor: crate::types::IntrinsicType::Range,
                arguments,
            } = interner.kind(layout.ty).map_err(|_| "range:invalid-type")?
            else {
                return Err("range:container-type");
            };
            let [element] = arguments.as_slice() else {
                return Err("range:arity");
            };
            let (start, _) = layout.child(Field::RangeStart).ok_or("range:start")?;
            let (end, _) = layout.child(Field::RangeEnd).ok_or("range:end")?;
            let (inclusive, _) = layout
                .child(Field::RangeInclusive)
                .ok_or("range:inclusive")?;
            (first + start, first + end, first + inclusive, *element)
        };
        if !is_native_range_element(element, interner) || item.ty != element {
            return Err("range:item-type");
        }
        let mut item = item.clone();
        self.operand(&mut item)?;
        let boolean = interner.scalar(ScalarType::Bool);
        let above_start = binary(
            self,
            span,
            statements,
            HirBinaryOperator::GreaterEqual,
            item.clone(),
            read(start, element),
            boolean,
        )?;
        let below_end = binary(
            self,
            span,
            statements,
            HirBinaryOperator::Less,
            item.clone(),
            read(end, element),
            boolean,
        )?;
        let at_end = binary(
            self,
            span,
            statements,
            HirBinaryOperator::Equal,
            item,
            read(end, element),
            boolean,
        )?;
        let inclusive_end = binary(
            self,
            span,
            statements,
            HirBinaryOperator::LogicalAnd,
            read(inclusive, boolean),
            at_end,
            boolean,
        )?;
        let under_limit = binary(
            self,
            span,
            statements,
            HirBinaryOperator::LogicalOr,
            below_end,
            inclusive_end,
            boolean,
        )?;
        let contained = binary(
            self,
            span,
            statements,
            HirBinaryOperator::LogicalAnd,
            above_start,
            under_limit,
            boolean,
        )?;
        value.kind = MirRvalueKind::Use(contained);
        Ok(())
    }
}
