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
