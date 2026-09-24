//! Checked float conversions become scalar predicates and initialized Results.
//! Saturating conversion is only an internal total instruction: its candidate
//! payload is exposed after the source-language checks have succeeded.

use super::*;

pub(super) struct CheckedConversion {
    pub block: usize,
    pub statement: usize,
    pub destination: MirPlace,
    pub target: ScalarType,
    pub source: MirOperand,
}

struct Builder<'a> {
    locals: &'a mut NativeLocals,
    interner: &'a TypeInterner,
    span: Span,
    statements: Vec<MirStatement>,
}

impl Builder<'_> {
    fn emit(&mut self, ty: TypeId, kind: MirRvalueKind) -> LowerResult<MirOperand> {
        let local = self.locals.allocate(1)?;
        self.statements
            .push(assign_rvalue(self.span, local, MirRvalue { ty, kind }));
        Ok(MirOperand {
            ty,
            kind: MirOperandKind::Copy(scalar_place(local, ty)),
        })
    }

    fn compare(
        &mut self,
        op: HirBinaryOperator,
        left: MirOperand,
        right: MirOperand,
    ) -> LowerResult<MirOperand> {
        self.emit(
            self.interner.scalar(ScalarType::Bool),
            MirRvalueKind::Binary {
                operator: op,
                left,
                right,
            },
        )
    }

    fn convert(&mut self, target: ScalarType, value: MirOperand) -> LowerResult<MirOperand> {
        self.emit(
            self.interner.scalar(target),
            MirRvalueKind::NumericConversion {
                target,
                conversion: NumericConversion::Total,
                value,
            },
        )
    }

    fn finite(&mut self, value: MirOperand) -> LowerResult<MirOperand> {
        let maximum = if value.ty == self.interner.scalar(ScalarType::Float32) {
            f64::from(f32::MAX)
        } else {
            f64::MAX
        };
        let lower = self.compare(
            HirBinaryOperator::GreaterEqual,
            value.clone(),
            float(value.ty, -maximum),
        )?;
        let upper = self.compare(
            HirBinaryOperator::LessEqual,
            value.clone(),
            float(value.ty, maximum),
        )?;
        self.compare(HirBinaryOperator::LogicalAnd, lower, upper)
    }

    fn integral(&mut self, value: MirOperand) -> LowerResult<MirOperand> {
        let scalar = if value.ty == self.interner.scalar(ScalarType::Float32) {
            ScalarType::Float32
        } else {
            ScalarType::Float
        };
        // Beyond the significand's fractional range every finite value is
        // integral, including values outside i64. Below it, a saturating i64
        // conversion and exact round trip distinguish every fractional value.
        let limit = if scalar == ScalarType::Float32 {
            16_777_216.0
        } else {
            9_007_199_254_740_992.0
        };
        let above = self.compare(
            HirBinaryOperator::GreaterEqual,
            value.clone(),
            float(value.ty, limit),
        )?;
        let below = self.compare(
            HirBinaryOperator::LessEqual,
            value.clone(),
            float(value.ty, -limit),
        )?;
        let large = self.compare(HirBinaryOperator::LogicalOr, above, below)?;
        let integer = self.convert(ScalarType::Int, value.clone())?;
        let restored = self.convert(scalar, integer)?;
        let exact = self.compare(HirBinaryOperator::Equal, value, restored)?;
        self.compare(HirBinaryOperator::LogicalOr, large, exact)
    }
}

fn float(ty: TypeId, value: f64) -> MirOperand {
    MirOperand {
        ty,
        kind: MirOperandKind::Constant(MirConstant::Float(value.to_string())),
    }
}

fn integer(ty: TypeId, value: i64) -> MirOperand {
    MirOperand {
        ty,
        kind: MirOperandKind::Constant(MirConstant::Integer(value.to_string())),
    }
}

fn block_id(index: usize) -> LowerResult<MirBlockId> {
    u32::try_from(index)
        .map(MirBlockId)
        .map_err(|_| "float:block-limit")
}

impl NativeLocals {
    pub(super) fn lower_float_conversion(
        &mut self,
        blocks: &mut Vec<MirBasicBlock>,
        conversion: CheckedConversion,
        interner: &TypeInterner,
    ) -> LowerResult<()> {
        let CheckedConversion {
            block: cursor,
            statement: index,
            destination,
            target,
            mut source,
        } = conversion;
        let span = blocks[cursor].statements[index].span;
        let (first, layout) = self
            .resolve(&destination)?
            .ok_or("float:conversion-storage")?;
        let (ok_offset, ok_layout) = layout
            .child(Field::ResultOk)
            .ok_or("float:conversion-result")?;
        let (err_offset, err_layout) = layout
            .child(Field::ResultErr)
            .ok_or("float:conversion-result")?;
        if ok_layout.ty != interner.scalar(target)
            || err_layout.child(Field::NumericError).is_none()
        {
            return Err("float:conversion-type");
        }
        let int = interner.scalar(ScalarType::Int);
        let mut defaults = Vec::with_capacity(layout.width as usize);
        layout.defaults(interner, &mut defaults);
        self.operand(&mut source)?;
        let mut builder = Builder {
            locals: self,
            interner,
            span,
            statements: Vec::new(),
        };
        let source = builder.emit(source.ty, MirRvalueKind::Use(source))?;
        let finite = builder.finite(source.clone())?;
        let candidate = builder.convert(target, source.clone())?;
        let checks = if target == ScalarType::Float32
            && source.ty == interner.scalar(ScalarType::Float)
        {
            let target_finite = builder.finite(candidate.clone())?;
            let nonfinite = builder.emit(
                interner.scalar(ScalarType::Bool),
                MirRvalueKind::Prefix {
                    operator: HirPrefixOperator::LogicalNot,
                    operand: finite,
                },
            )?;
            let valid = builder.compare(HirBinaryOperator::LogicalOr, nonfinite, target_finite)?;
            vec![(valid, NumericConversionErrorVariant::OutOfRange)]
        } else {
            let (minimum, maximum) =
                native_integers::value_bounds(target).ok_or("float:conversion-target")?;
            let integral = builder.integral(source.clone())?;
            // Exclusive powers-of-two upper bounds avoid rounding Int.max or
            // UInt64.max up into an erroneously admitted float value.
            let lower = builder.compare(
                HirBinaryOperator::GreaterEqual,
                source.clone(),
                float(source.ty, minimum as f64),
            )?;
            let upper = builder.compare(
                HirBinaryOperator::Less,
                source.clone(),
                float(source.ty, (maximum + 1) as f64),
            )?;
            let range = builder.compare(HirBinaryOperator::LogicalAnd, lower, upper)?;
            vec![
                (finite, NumericConversionErrorVariant::NotFinite),
                (integral, NumericConversionErrorVariant::NotIntegral),
                (range, NumericConversionErrorVariant::OutOfRange),
            ]
        };
        let successor = block_id(blocks.len())?;
        let success = block_id(blocks.len() + 1)?;
        let errors = blocks.len() + 2;
        let decisions = errors + checks.len();
        let block = &mut blocks[cursor];
        let tail = block.statements.split_off(index + 1);
        block.statements.pop();
        block.statements.extend(builder.statements);
        let decision = |i: usize| -> LowerResult<MirTerminator> {
            Ok(MirTerminator {
                span,
                kind: MirTerminatorKind::SwitchBool {
                    condition: checks[i].0.clone(),
                    if_true: if i + 1 == checks.len() {
                        success
                    } else {
                        block_id(decisions + i)?
                    },
                    if_false: block_id(errors + i)?,
                },
            })
        };
        let original = std::mem::replace(&mut block.terminator, decision(0)?);
        let kind = block.kind;
        blocks.push(MirBasicBlock {
            kind,
            statements: tail,
            terminator: original,
        });
        let mut outputs = vec![None];
        outputs.extend(checks.iter().map(|(_, error)| Some(*error)));
        for error in outputs {
            let mut values = defaults.clone();
            values[0] = integer(int, if error.is_none() { 2 } else { 3 });
            if let Some(error) = error {
                values[err_offset as usize] = integer(int, i64::from(error.index()));
            } else {
                values[ok_offset as usize] = candidate.clone();
            }
            blocks.push(MirBasicBlock {
                kind,
                statements: values
                    .into_iter()
                    .enumerate()
                    .map(|(i, value)| assign(span, first + i as u32, value))
                    .collect(),
                terminator: MirTerminator {
                    span,
                    kind: MirTerminatorKind::Goto { target: successor },
                },
            });
        }
        for i in 1..checks.len() {
            blocks.push(MirBasicBlock {
                kind,
                statements: Vec::new(),
                terminator: decision(i)?,
            });
        }
        Ok(())
    }
}

impl NativeLocals {
    pub(super) fn lower_math_sqrt_calls(
        &mut self,
        blocks: &mut Vec<MirBasicBlock>,
        interner: &TypeInterner,
        records: &RecordFields,
        enums: &EnumVariants,
        cache: &mut BTreeMap<TypeId, Rc<Layout>>,
    ) -> LowerResult<()> {
        let float_type = interner.scalar(ScalarType::Float);
        let int_type = interner.scalar(ScalarType::Int);
        let mut cursor = 0;
        while cursor < blocks.len() {
            let MirTerminatorKind::Invoke {
                operation,
                destination,
                target: Some(target),
                unwind,
            } = &blocks[cursor].terminator.kind
            else {
                cursor += 1;
                continue;
            };
            let MirOperationKind::Call {
                callee,
                arguments,
                protocol: HirCallProtocol::Call | HirCallProtocol::CallOnce,
                unsafe_call: false,
                ..
            } = &operation.kind
            else {
                cursor += 1;
                continue;
            };
            if !matches!(
                callee.kind,
                MirOperandKind::Function {
                    callable: HirCallableId::Host(crate::hir::HirBootstrapHostFunction::MathSqrt),
                    ..
                }
            ) {
                cursor += 1;
                continue;
            }
            let [argument] = arguments.as_slice() else {
                return Err("math:sqrt-arity");
            };
            if argument.mode != ParameterMode::Value
                || argument.target != HirCallArgumentTarget::Fixed(0)
                || argument.value.ty != float_type
            {
                return Err("math:sqrt-argument");
            }
            let Ok(TypeKind::Result { success, error }) = interner.kind(operation.ty) else {
                return Err("math:sqrt-result");
            };
            if *success != float_type
                || !matches!(
                    interner.kind(*error),
                    Ok(TypeKind::Intrinsic {
                        constructor: crate::types::IntrinsicType::MathError,
                        arguments,
                    }) if arguments.is_empty()
                )
            {
                return Err("math:sqrt-result");
            }
            let layout = Layout::build(operation.ty, interner, records, enums, cache, 0)
                .ok_or("math:sqrt-layout")?;
            let (tag_offset, tag) = layout.child(Field::Tag).ok_or("math:sqrt-layout")?;
            let (ok_offset, ok) = layout.child(Field::ResultOk).ok_or("math:sqrt-layout")?;
            let (err_offset, err) = layout.child(Field::ResultErr).ok_or("math:sqrt-layout")?;
            if tag_offset != 0
                || tag.width != 1
                || ok.width != 1
                || ok.ty != float_type
                || err.width != 1
                || err.child(Field::MathError).is_none()
            {
                return Err("math:sqrt-layout");
            }
            let first = if let Some(destination) = destination {
                let (first, actual) = self.resolve(destination)?.ok_or("math:sqrt-storage")?;
                if actual.ty != layout.ty {
                    return Err("math:sqrt-storage");
                }
                first
            } else {
                self.allocate(layout.width)?
            };
            let mut source = argument.value.clone();
            self.operand(&mut source)?;
            let target = *target;
            let unwind = *unwind;
            let span = blocks[cursor].terminator.span;
            let kind = blocks[cursor].kind;
            let input = self.allocate(1)?;
            let read = |index: u32, ty: TypeId| MirOperand {
                ty,
                kind: MirOperandKind::Copy(scalar_place(index, ty)),
            };
            let mut builder = Builder {
                locals: self,
                interner,
                span,
                statements: Vec::new(),
            };
            // Only negative infinity is non-finite and rejected. NaN and
            // positive infinity follow IEEE sqrt; finite negatives are Domain.
            let negative_infinity = builder.compare(
                HirBinaryOperator::Less,
                read(input, float_type),
                float(float_type, -f64::MAX),
            )?;
            let negative = builder.compare(
                HirBinaryOperator::Less,
                read(input, float_type),
                float(float_type, 0.0),
            )?;
            let comparison_statements = builder.statements;
            let mut defaults = Vec::with_capacity(layout.width as usize);
            layout.defaults(interner, &mut defaults);
            let raw = self.allocate(1)?;
            let check_domain = block_id(blocks.len())?;
            let nonfinite = block_id(blocks.len() + 1)?;
            let domain = block_id(blocks.len() + 2)?;
            let compute = block_id(blocks.len() + 3)?;
            let success = block_id(blocks.len() + 4)?;
            let block = &mut blocks[cursor];
            block.statements.push(assign(span, input, source));
            block.statements.extend(comparison_statements);
            block.terminator.kind = MirTerminatorKind::SwitchBool {
                condition: negative_infinity,
                if_true: nonfinite,
                if_false: check_domain,
            };
            blocks.push(MirBasicBlock {
                kind,
                statements: Vec::new(),
                terminator: MirTerminator {
                    span,
                    kind: MirTerminatorKind::SwitchBool {
                        condition: negative,
                        if_true: domain,
                        if_false: compute,
                    },
                },
            });
            for (error_block, code) in [(nonfinite, 1), (domain, 0)] {
                let mut values = defaults.clone();
                values[tag_offset as usize] = integer(int_type, 3);
                values[err_offset as usize] = integer(int_type, code);
                debug_assert_eq!(error_block.index() as usize, blocks.len());
                blocks.push(MirBasicBlock {
                    kind,
                    statements: values
                        .into_iter()
                        .enumerate()
                        .map(|(offset, value)| assign(span, first + offset as u32, value))
                        .collect(),
                    terminator: MirTerminator {
                        span,
                        kind: MirTerminatorKind::Goto { target },
                    },
                });
            }
            blocks.push(MirBasicBlock {
                kind,
                statements: Vec::new(),
                terminator: MirTerminator {
                    span,
                    kind: MirTerminatorKind::Invoke {
                        operation: MirOperation {
                            ty: float_type,
                            kind: MirOperationKind::BootstrapHostCall {
                                function: MirBootstrapHostFunction::NativeMathSqrtUnchecked,
                                arguments: vec![read(input, float_type)],
                            },
                        },
                        destination: Some(scalar_place(raw, float_type)),
                        target: Some(success),
                        unwind,
                    },
                },
            });
            let mut values = defaults;
            values[tag_offset as usize] = integer(int_type, 2);
            values[ok_offset as usize] = read(raw, float_type);
            blocks.push(MirBasicBlock {
                kind,
                statements: values
                    .into_iter()
                    .enumerate()
                    .map(|(offset, value)| assign(span, first + offset as u32, value))
                    .collect(),
                terminator: MirTerminator {
                    span,
                    kind: MirTerminatorKind::Goto { target },
                },
            });
            cursor += 1;
        }
        Ok(())
    }
}
