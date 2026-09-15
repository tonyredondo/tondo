//! Sums use a tag and disjoint, initialized payload carriers. Inactive
//! carriers are canonical zero values, so existing copies and structural
//! equality can observe the full layout without reading uninitialized data.

use super::*;

fn integer(ty: TypeId, value: i64) -> MirOperand {
    MirOperand {
        ty,
        kind: MirOperandKind::Constant(MirConstant::Integer(value.to_string())),
    }
}

impl Layout {
    fn union_tag(&self, member: TypeId) -> LowerResult<u32> {
        self.child(Field::UnionValue(member))
            .ok_or("union:member-identity")?;
        // A member keeps its identity across distinct union layouts in this
        // native program. This private tag is not a cross-program type ABI.
        Ok(member.index())
    }

    fn variant_tag(&self, member: MemberId) -> LowerResult<u32> {
        self.variants
            .iter()
            .position(|candidate| *candidate == member)
            .and_then(|index| u32::try_from(index).ok())
            .ok_or("enum:variant-identity")
    }

    pub(super) fn defaults(&self, interner: &TypeInterner, values: &mut Vec<MirOperand>) {
        if self.children.is_empty() {
            let kind = match interner.kind(self.ty) {
                Ok(TypeKind::Scalar(ScalarType::Bool)) => MirConstant::Bool(false),
                Ok(TypeKind::Scalar(ScalarType::Unit)) => MirConstant::Unit,
                Ok(TypeKind::Scalar(ScalarType::Float | ScalarType::Float32)) => {
                    MirConstant::Float("0.0".to_owned())
                }
                _ => MirConstant::Integer("0".to_owned()),
            };
            values.push(MirOperand {
                ty: self.ty,
                kind: MirOperandKind::Constant(kind),
            });
        } else {
            for (_, child) in &self.children {
                child.defaults(interner, values);
            }
        }
    }
}

impl NativeLocals {
    pub(super) fn union_values(
        &self,
        kind: Assignability,
        value: &MirOperand,
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Vec<MirOperand>> {
        if !matches!(interner.kind(layout.ty), Ok(TypeKind::Union(_))) {
            return Err("union:destination-layout");
        }
        let mut values = Vec::with_capacity(layout.width as usize);
        layout.defaults(interner, &mut values);
        match kind {
            Assignability::UnionInjection => {
                let tag = layout.union_tag(value.ty)?;
                let (tag_offset, tag_layout) = layout.child(Field::Tag).ok_or("union:tag")?;
                values[tag_offset as usize] = integer(tag_layout.ty, i64::from(tag));
                let (offset, child) = layout
                    .child(Field::UnionValue(value.ty))
                    .ok_or("union:member-layout")?;
                let payload = self.values(value)?;
                if payload.len() != child.width as usize {
                    return Err("union:payload-shape");
                }
                values.splice(offset as usize..(offset + child.width) as usize, payload);
            }
            Assignability::UnionWidening => {
                let (MirOperandKind::Copy(place) | MirOperandKind::Move(place)) = &value.kind
                else {
                    return Err("union:widening-operand");
                };
                let (first, source) = self.resolve(place)?.ok_or("union:widening-storage")?;
                if !matches!(interner.kind(source.ty), Ok(TypeKind::Union(_))) {
                    return Err("union:widening-source");
                }
                // Copy the unchanged tag and every existing member by type,
                // not by offset. New members remain zero; inactive source
                // members are already canonical. The caller snapshots these
                // operands before writing a potentially overlapping destination.
                let mut source_offset = first;
                for (field, child) in &source.children {
                    let (offset, target) = layout.child(*field).ok_or("union:widening-member")?;
                    if target.ty != child.ty || target.width != child.width {
                        return Err("union:widening-shape");
                    }
                    let mut payload = Vec::with_capacity(child.width as usize);
                    child.operands(source_offset, &mut payload);
                    values.splice(offset as usize..(offset + child.width) as usize, payload);
                    source_offset += child.width;
                }
            }
            _ => return Err("union:coercion"),
        }
        Ok(values)
    }

    pub(super) fn sum_values(
        &self,
        shape: &MirAggregateKind,
        payloads: &[MirOperand],
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Option<Vec<MirOperand>>> {
        if let MirAggregateKind::Variant { variant, fields } = shape {
            return self
                .enum_values(*variant, fields, payloads, layout, interner)
                .map(Some);
        }
        let (tag_field, tag, payload_field) = match shape {
            MirAggregateKind::OptionNone => (Field::Tag, 0, None),
            MirAggregateKind::OptionSome => (Field::Tag, 1, Some(Field::OptionValue)),
            MirAggregateKind::ResultOk => (Field::Tag, 2, Some(Field::ResultOk)),
            MirAggregateKind::ResultErr => (Field::Tag, 3, Some(Field::ResultErr)),
            MirAggregateKind::NumericConversionError(variant) => {
                (Field::NumericError, i64::from(variant.index()), None)
            }
            _ => return Ok(None),
        };
        // Scalar types in defaults come from the already validated layout.
        // No default is ever exposed as an active payload by a constructor.
        let mut values = Vec::with_capacity(layout.width as usize);
        layout.defaults(interner, &mut values);
        let (offset, tag_layout) = layout.child(tag_field).ok_or("sum:constructor-tag")?;
        values[offset as usize] = integer(tag_layout.ty, tag);
        if let Some(field) = payload_field {
            let (offset, child) = layout.child(field).ok_or("sum:constructor-payload")?;
            let [payload] = payloads else {
                return Err("sum:constructor-arity");
            };
            let replacement = self.values(payload)?;
            if payload.ty != child.ty || replacement.len() != child.width as usize {
                return Err("sum:constructor-type");
            }
            values.splice(
                offset as usize..(offset + child.width) as usize,
                replacement,
            );
        } else if !payloads.is_empty() {
            return Err("sum:constructor-arity");
        }
        Ok(Some(values))
    }

    fn enum_values(
        &self,
        variant: MemberId,
        fields: &[Option<MemberId>],
        payloads: &[MirOperand],
        layout: &Layout,
        interner: &TypeInterner,
    ) -> LowerResult<Vec<MirOperand>> {
        let tag = layout.variant_tag(variant)?;
        let expected = layout.children.iter().filter(|(field, _)| {
            matches!(field, Field::VariantTuple(member, _) | Field::VariantRecord(member, _) if *member == variant)
        }).count();
        if fields.len() != expected || payloads.len() != expected {
            return Err("enum:constructor-arity");
        }
        let mut values = Vec::with_capacity(layout.width as usize);
        layout.defaults(interner, &mut values);
        let (offset, tag_layout) = layout.child(Field::Tag).ok_or("enum:constructor-tag")?;
        values[offset as usize] = integer(tag_layout.ty, i64::from(tag));
        for (index, (field, payload)) in fields.iter().zip(payloads).enumerate() {
            let key = match field {
                Some(field) => Field::VariantRecord(variant, *field),
                None => Field::VariantTuple(variant, index as u32),
            };
            let (offset, child) = layout.child(key).ok_or("enum:constructor-field")?;
            let replacement = self.values(payload)?;
            if payload.ty != child.ty || replacement.len() != child.width as usize {
                return Err("enum:constructor-type");
            }
            values.splice(
                offset as usize..(offset + child.width) as usize,
                replacement,
            );
        }
        Ok(values)
    }

    pub(super) fn lower_tags(
        &mut self,
        blocks: &mut Vec<MirBasicBlock>,
        interner: &TypeInterner,
    ) -> LowerResult<()> {
        let boolean = interner.scalar(ScalarType::Bool);
        let original_count = blocks.len();
        let mut added = Vec::new();
        for block in blocks.iter_mut() {
            let MirTerminatorKind::SwitchTag {
                value,
                cases,
                otherwise,
            } = &block.terminator.kind
            else {
                continue;
            };
            let (MirOperandKind::Copy(place)
            | MirOperandKind::Move(place)
            | MirOperandKind::Borrow(place)) = &value.kind
            else {
                continue;
            };
            let Some((first, layout)) = self.resolve(place)? else {
                continue;
            };
            let field = if layout.child(Field::NumericError).is_some() {
                Field::NumericError
            } else {
                Field::Tag
            };
            let (offset, tag_layout) = layout.child(field).ok_or("sum:switch-layout")?;
            let tag = MirOperand {
                ty: tag_layout.ty,
                kind: MirOperandKind::Copy(scalar_place(first + offset, tag_layout.ty)),
            };
            let cases = cases
                .iter()
                .map(|(case, target)| {
                    let discriminant = match case {
                        MirTag::NumericConversionError(variant) if field == Field::NumericError => {
                            variant.index()
                        }
                        MirTag::Variant(member) if field == Field::Tag => {
                            layout.variant_tag(*member)?
                        }
                        MirTag::Union(member) if field == Field::Tag => {
                            layout.union_tag(*member)?
                        }
                        _ if field == Field::Tag
                            && layout.variants.is_empty()
                            && !matches!(interner.kind(layout.ty), Ok(TypeKind::Union(_))) =>
                        {
                            backend_tag_discriminant(*case).ok_or("sum:switch-tag")?
                        }
                        _ => return Err("sum:switch-tag"),
                    };
                    Ok((discriminant, *target))
                })
                .collect::<LowerResult<Vec<_>>>()?;
            let mut otherwise = *otherwise;
            let span = block.terminator.span;
            for (index, (discriminant, target)) in cases.iter().enumerate().rev() {
                let condition = self.allocate(1)?;
                let test = assign_rvalue(
                    span,
                    condition,
                    MirRvalue {
                        ty: boolean,
                        kind: MirRvalueKind::Binary {
                            operator: HirBinaryOperator::Equal,
                            left: tag.clone(),
                            right: integer(tag.ty, i64::from(*discriminant)),
                        },
                    },
                );
                let terminator = MirTerminatorKind::SwitchBool {
                    condition: MirOperand {
                        ty: boolean,
                        kind: MirOperandKind::Copy(scalar_place(condition, boolean)),
                    },
                    if_true: *target,
                    if_false: otherwise,
                };
                if index == 0 {
                    block.statements.push(test);
                    block.terminator.kind = terminator;
                } else {
                    otherwise = block_id(original_count + added.len())?;
                    added.push(MirBasicBlock {
                        kind: block.kind,
                        statements: vec![test],
                        terminator: MirTerminator {
                            span,
                            kind: terminator,
                        },
                    });
                }
            }
            if cases.is_empty() {
                block.terminator.kind = MirTerminatorKind::Goto { target: otherwise };
            }
        }
        blocks.extend(added);
        Ok(())
    }

    pub(super) fn lower_checked_conversions(
        &mut self,
        blocks: &mut Vec<MirBasicBlock>,
        interner: &TypeInterner,
    ) -> LowerResult<()> {
        let int = interner.scalar(ScalarType::Int);
        let boolean = interner.scalar(ScalarType::Bool);
        let mut cursor = 0;
        while cursor < blocks.len() {
            let found =
                blocks[cursor]
                    .statements
                    .iter()
                    .enumerate()
                    .find_map(|(index, statement)| {
                        if let MirStatementKind::Assign {
                            destination,
                            value:
                                MirRvalue {
                                    kind:
                                        MirRvalueKind::NumericConversion {
                                            target,
                                            conversion: NumericConversion::Checked,
                                            value,
                                        },
                                    ..
                                },
                        } = &statement.kind
                        {
                            Some((
                                index,
                                statement.span,
                                destination.clone(),
                                *target,
                                value.clone(),
                            ))
                        } else {
                            None
                        }
                    });
            let Some((index, span, destination, target, mut source)) = found else {
                cursor += 1;
                continue;
            };
            if native_float_width(source.ty, interner).is_some() {
                self.lower_float_conversion(
                    blocks,
                    floats::CheckedConversion {
                        block: cursor,
                        statement: index,
                        destination,
                        target,
                        source,
                    },
                    interner,
                )?;
                cursor += 1;
                continue;
            }
            let Some((minimum, maximum)) = native_integers::value_bounds(target) else {
                return Err("sum:conversion-representation");
            };
            let Ok(TypeKind::Scalar(source_scalar)) = interner.kind(source.ty) else {
                return Err("sum:conversion-representation");
            };
            let (source_minimum, source_maximum) = native_integers::value_bounds(*source_scalar)
                .ok_or("sum:conversion-representation")?;
            // Compare in the source domain. UInt64 uses unsigned comparisons;
            // signed sources never need an unrepresentable unsigned maximum.
            let source_type = source.ty;
            let bound = |value: i128| MirOperand {
                ty: source_type,
                kind: MirOperandKind::Constant(MirConstant::Integer(value.to_string())),
            };
            let minimum = minimum.max(source_minimum);
            let maximum = maximum.min(source_maximum);
            let (first, layout) = self
                .resolve(&destination)?
                .ok_or("sum:conversion-storage")?;
            let (ok_offset, ok_layout) = layout
                .child(Field::ResultOk)
                .ok_or("sum:conversion-result")?;
            let (err_offset, err_layout) = layout
                .child(Field::ResultErr)
                .ok_or("sum:conversion-result")?;
            if ok_layout.ty != interner.scalar(target)
                || err_layout.child(Field::NumericError).is_none()
            {
                return Err("sum:conversion-type");
            }
            let mut defaults = Vec::with_capacity(layout.width as usize);
            layout.defaults(interner, &mut defaults);
            let ok_offset = ok_offset as usize;
            let err_offset = err_offset as usize;
            self.operand(&mut source)?;
            // Capture before writing: the result may replace a value whose
            // projected payload supplied the conversion operand.
            let temporary = self.allocate(4)?;
            let read = |index, ty| MirOperand {
                ty,
                kind: MirOperandKind::Copy(scalar_place(index, ty)),
            };
            let successor = block_id(blocks.len())?;
            let success = block_id(blocks.len() + 1)?;
            let failure = block_id(blocks.len() + 2)?;
            let block = &mut blocks[cursor];
            let tail = block.statements.split_off(index + 1);
            block.statements.pop();
            block.statements.push(assign(span, temporary, source));
            for (local, operator, left, right) in [
                (
                    temporary + 1,
                    HirBinaryOperator::Less,
                    read(temporary, source_type),
                    bound(minimum),
                ),
                (
                    temporary + 2,
                    HirBinaryOperator::Greater,
                    read(temporary, source_type),
                    bound(maximum),
                ),
                (
                    temporary + 3,
                    HirBinaryOperator::LogicalOr,
                    read(temporary + 1, boolean),
                    read(temporary + 2, boolean),
                ),
            ] {
                block.statements.push(assign_rvalue(
                    span,
                    local,
                    MirRvalue {
                        ty: boolean,
                        kind: MirRvalueKind::Binary {
                            operator,
                            left,
                            right,
                        },
                    },
                ));
            }
            let original_terminator = std::mem::replace(
                &mut block.terminator,
                MirTerminator {
                    span,
                    kind: MirTerminatorKind::SwitchBool {
                        condition: read(temporary + 3, boolean),
                        if_true: failure,
                        if_false: success,
                    },
                },
            );
            let kind = block.kind;
            blocks.push(MirBasicBlock {
                kind,
                statements: tail,
                terminator: original_terminator,
            });
            for ok in [true, false] {
                let mut values = defaults.clone();
                values[0] = integer(int, if ok { 2 } else { 3 });
                if ok {
                    values[ok_offset] = read(temporary, interner.scalar(target));
                } else {
                    values[err_offset] = integer(
                        int,
                        i64::from(NumericConversionErrorVariant::OutOfRange.index()),
                    );
                }
                blocks.push(MirBasicBlock {
                    kind,
                    statements: values
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| assign(span, first + index as u32, value))
                        .collect(),
                    terminator: MirTerminator {
                        span,
                        kind: MirTerminatorKind::Goto { target: successor },
                    },
                });
            }
            cursor += 1;
        }
        Ok(())
    }
}

fn block_id(index: usize) -> LowerResult<MirBlockId> {
    u32::try_from(index)
        .map(MirBlockId)
        .map_err(|_| "sum:block-limit")
}
