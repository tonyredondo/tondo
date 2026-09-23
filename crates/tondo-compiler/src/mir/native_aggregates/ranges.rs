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

    pub(super) fn range_cursor_values(
        &self,
        source: &MirOperand,
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Vec<MirOperand>> {
        let TypeKind::Cursor {
            mode: crate::types::CursorMode::Own,
            collection,
        } = interner.kind(layout.ty).map_err(|_| "range:cursor-type")?
        else {
            return Err("range:cursor-layout");
        };
        if source.ty != *collection || layout.width != 5 {
            return Err("range:cursor-source");
        }
        let mut values = self.values(source)?;
        if values.len() != 3 {
            return Err("range:cursor-source-shape");
        }
        values.push(values[0].clone());
        values.push(MirOperand {
            ty: interner.scalar(ScalarType::Bool),
            kind: MirOperandKind::Constant(MirConstant::Bool(true)),
        });
        Ok(values)
    }

    pub(super) fn lower_range_iterator_next(
        &mut self,
        block: &mut MirBasicBlock,
        interner: &TypeInterner,
        original_blocks: u32,
        added: &mut Vec<MirBasicBlock>,
    ) -> LowerResult<()> {
        let (state, destination, borrowed_source, exhaustion_guard, has_value, exhausted) =
            match &block.terminator.kind {
                MirTerminatorKind::IteratorNext {
                    state,
                    destination,
                    borrowed_source,
                    exhaustion_guard,
                    has_value,
                    exhausted,
                    ..
                } => (
                    state.clone(),
                    destination.clone(),
                    borrowed_source.clone(),
                    exhaustion_guard.clone(),
                    *has_value,
                    *exhausted,
                ),
                _ => return Ok(()),
            };
        let Some((first, layout)) = self.resolve(&state)? else {
            return Ok(());
        };
        let TypeKind::Cursor {
            mode: crate::types::CursorMode::Own,
            collection,
        } = interner.kind(layout.ty).map_err(|_| "range:cursor-type")?
        else {
            return Ok(());
        };
        let TypeKind::Intrinsic {
            constructor: crate::types::IntrinsicType::Range,
            arguments,
        } = interner
            .kind(*collection)
            .map_err(|_| "range:cursor-collection")?
        else {
            return Err("range:cursor-collection");
        };
        let [element] = arguments.as_slice() else {
            return Err("range:cursor-element");
        };
        if !is_native_range_element(*element, interner)
            || borrowed_source.is_some()
            || exhaustion_guard.is_some()
            || !destination.projections.is_empty()
            || destination.source_loan.is_some()
            || destination.ty != *element
        {
            return Err("range:cursor-protocol");
        }
        let (source_offset, source) = layout
            .child(Field::CursorSource)
            .ok_or("range:cursor-source")?;
        let (current_offset, _) = layout
            .child(Field::CursorCurrent)
            .ok_or("range:cursor-current")?;
        let (active_offset, _) = layout
            .child(Field::CursorActive)
            .ok_or("range:cursor-active")?;
        let (end_offset, _) = source.child(Field::RangeEnd).ok_or("range:cursor-end")?;
        let (inclusive_offset, _) = source
            .child(Field::RangeInclusive)
            .ok_or("range:cursor-inclusive")?;
        let current = first + current_offset;
        let end = first + source_offset + end_offset;
        let inclusive = first + source_offset + inclusive_offset;
        let active = first + active_offset;
        let span = block.terminator.span;
        let bool_ty = interner.scalar(ScalarType::Bool);
        let less = binary(
            self,
            span,
            &mut block.statements,
            HirBinaryOperator::Less,
            read(current, *element),
            read(end, *element),
            bool_ty,
        )?;
        let equal = binary(
            self,
            span,
            &mut block.statements,
            HirBinaryOperator::Equal,
            read(current, *element),
            read(end, *element),
            bool_ty,
        )?;
        let at_inclusive_end = binary(
            self,
            span,
            &mut block.statements,
            HirBinaryOperator::LogicalAnd,
            read(inclusive, bool_ty),
            equal,
            bool_ty,
        )?;
        let in_bounds = binary(
            self,
            span,
            &mut block.statements,
            HirBinaryOperator::LogicalOr,
            less.clone(),
            at_inclusive_end,
            bool_ty,
        )?;
        let has_next = binary(
            self,
            span,
            &mut block.statements,
            HirBinaryOperator::LogicalAnd,
            read(active, bool_ty),
            in_bounds,
            bool_ty,
        )?;

        let first_block = original_blocks
            .checked_add(u32::try_from(added.len()).map_err(|_| "range:iterator-block-limit")?)
            .ok_or("range:iterator-block-limit")?;
        let id = |offset: u32| {
            first_block
                .checked_add(offset)
                .map(MirBlockId)
                .ok_or("range:iterator-block-limit")
        };
        let emit = id(0)?;
        let advance = id(1)?;
        let final_item = id(2)?;
        let empty = id(3)?;
        block.terminator.kind = MirTerminatorKind::SwitchBool {
            condition: has_next,
            if_true: emit,
            if_false: empty,
        };
        let normal = |statements, kind| MirBasicBlock {
            kind: MirBlockKind::Normal,
            statements,
            terminator: MirTerminator { span, kind },
        };
        added.push(normal(
            vec![assign(
                span,
                destination.local.index(),
                read(current, *element),
            )],
            MirTerminatorKind::SwitchBool {
                condition: less,
                if_true: advance,
                if_false: final_item,
            },
        ));
        let one = MirOperand {
            ty: *element,
            kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
        };
        let int_ty = interner.scalar(ScalarType::Int);
        let next = if *element == interner.scalar(ScalarType::Char) {
            MirRvalue {
                ty: *element,
                kind: MirRvalueKind::Binary {
                    operator: HirBinaryOperator::Add,
                    // Char uses an i64 native carrier. Only this private copy
                    // views its bits as Int for the bounded successor step.
                    left: read(current, int_ty),
                    right: MirOperand {
                        ty: int_ty,
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    },
                },
            }
        } else {
            MirRvalue {
                ty: *element,
                kind: MirRvalueKind::Binary {
                    operator: HirBinaryOperator::Add,
                    left: read(current, *element),
                    right: one,
                },
            }
        };
        if *element == interner.scalar(ScalarType::Char) {
            let skip = id(4)?;
            let ordinary = id(5)?;
            let mut advance_statements = Vec::new();
            let boundary = binary(
                self,
                span,
                &mut advance_statements,
                HirBinaryOperator::Equal,
                read(current, *element),
                MirOperand {
                    ty: *element,
                    kind: MirOperandKind::Constant(MirConstant::Char("'\\u{d7ff}'".into())),
                },
                bool_ty,
            )?;
            added.push(normal(
                advance_statements,
                MirTerminatorKind::SwitchBool {
                    condition: boundary,
                    if_true: skip,
                    if_false: ordinary,
                },
            ));
        } else {
            added.push(normal(
                vec![assign_rvalue(span, current, next.clone())],
                MirTerminatorKind::Goto { target: has_value },
            ));
        }
        added.push(normal(
            vec![assign(
                span,
                active,
                MirOperand {
                    ty: bool_ty,
                    kind: MirOperandKind::Constant(MirConstant::Bool(false)),
                },
            )],
            MirTerminatorKind::Goto { target: has_value },
        ));
        added.push(normal(
            vec![assign(
                span,
                active,
                MirOperand {
                    ty: bool_ty,
                    kind: MirOperandKind::Constant(MirConstant::Bool(false)),
                },
            )],
            MirTerminatorKind::Goto { target: exhausted },
        ));
        if *element == interner.scalar(ScalarType::Char) {
            added.push(normal(
                vec![assign(
                    span,
                    current,
                    MirOperand {
                        ty: *element,
                        kind: MirOperandKind::Constant(MirConstant::Char("'\\u{e000}'".into())),
                    },
                )],
                MirTerminatorKind::Goto { target: has_value },
            ));
            added.push(normal(
                vec![assign_rvalue(span, current, next)],
                MirTerminatorKind::Goto { target: has_value },
            ));
        }
        Ok(())
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
