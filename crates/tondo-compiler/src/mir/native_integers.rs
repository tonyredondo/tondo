//! Preserve narrow integer semantics before erasing types into i64 carriers.
//!
//! Only the private native copy is rewritten. Existing checked operations,
//! Boolean guards and bitwise operators suffice; adapters need no new ABI.

use super::native_aggregates::assign_rvalue;
use super::*;
use crate::types::TypeKind;

type Result<T> = std::result::Result<T, &'static str>;

pub(super) fn is_value_scalar(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Bool | ScalarType::Unit)
        || is_native_integer_scalar(scalar.as_str())
}

#[derive(Clone, Copy)]
struct NarrowInteger {
    bits: u32,
    signed: bool,
}

impl NarrowInteger {
    fn of(ty: TypeId, interner: &TypeInterner) -> Option<Self> {
        let (bits, signed) = match interner.kind(ty).ok()? {
            TypeKind::Scalar(ScalarType::Byte | ScalarType::UInt8) => (8, false),
            TypeKind::Scalar(ScalarType::UInt16) => (16, false),
            TypeKind::Scalar(ScalarType::UInt32) => (32, false),
            TypeKind::Scalar(ScalarType::Int8) => (8, true),
            TypeKind::Scalar(ScalarType::Int16) => (16, true),
            TypeKind::Scalar(ScalarType::Int32) => (32, true),
            _ => return None,
        };
        Some(Self { bits, signed })
    }

    fn mask(self) -> i64 {
        (1_i64 << self.bits) - 1
    }

    fn bounds(self) -> (i64, i64) {
        if self.signed {
            let sign = 1_i64 << (self.bits - 1);
            (-sign, sign - 1)
        } else {
            (0, self.mask())
        }
    }
}

struct Lowering<'a, F> {
    int: TypeId,
    boolean: TypeId,
    allocate: &'a mut F,
}

impl<F: FnMut(u32) -> Result<u32>> Lowering<'_, F> {
    fn constant(&self, value: i64) -> MirOperand {
        MirOperand {
            ty: self.int,
            kind: MirOperandKind::Constant(MirConstant::Integer(value.to_string())),
        }
    }

    fn binary(
        &mut self,
        block: &mut MirBasicBlock,
        operator: HirBinaryOperator,
        left: MirOperand,
        right: MirOperand,
        ty: TypeId,
    ) -> Result<MirOperand> {
        let local = (self.allocate)(1)?;
        block.statements.push(assign_rvalue(
            block.terminator.span,
            local,
            MirRvalue {
                ty,
                kind: MirRvalueKind::Binary {
                    operator,
                    left,
                    right,
                },
            },
        ));
        Ok(read(local, ty))
    }

    fn guard(
        &mut self,
        block: &mut MirBasicBlock,
        value: MirOperand,
        minimum: i64,
        maximum: i64,
        target: MirBlockId,
        unwind: MirBlockId,
    ) -> Result<()> {
        let lower = self.binary(
            block,
            HirBinaryOperator::GreaterEqual,
            value.clone(),
            self.constant(minimum),
            self.boolean,
        )?;
        let upper = self.binary(
            block,
            HirBinaryOperator::LessEqual,
            value,
            self.constant(maximum),
            self.boolean,
        )?;
        let condition = self.binary(
            block,
            HirBinaryOperator::LogicalAnd,
            lower,
            upper,
            self.boolean,
        )?;
        block.terminator.kind = MirTerminatorKind::Invoke {
            operation: MirOperation {
                ty: self.boolean,
                kind: MirOperationKind::Assert {
                    condition,
                    condition_repr: "native integer range".to_owned(),
                    message_parts: Vec::new(),
                },
            },
            destination: None,
            target: Some(target),
            unwind,
        };
        Ok(())
    }

    fn truncate(
        &mut self,
        block: &mut MirBasicBlock,
        value: MirOperand,
        integer: NarrowInteger,
    ) -> Result<MirOperand> {
        let bits = self.binary(
            block,
            HirBinaryOperator::BitwiseAnd,
            value,
            self.constant(integer.mask()),
            self.int,
        )?;
        if !integer.signed {
            return Ok(bits);
        }
        // These operations stay within i64 even for a 32-bit signed source.
        // Reinterpret the low bits without depending on a host-language cast.
        let sign = self.constant(1_i64 << (integer.bits - 1));
        let biased = self.binary(
            block,
            HirBinaryOperator::BitwiseXor,
            bits,
            sign.clone(),
            self.int,
        )?;
        self.binary(block, HirBinaryOperator::Subtract, biased, sign, self.int)
    }
}

fn read(local: u32, ty: TypeId) -> MirOperand {
    MirOperand {
        ty,
        kind: MirOperandKind::Copy(place(local, ty)),
    }
}

fn place(local: u32, ty: TypeId) -> MirPlace {
    MirPlace {
        local: MirLocalId(local),
        ty,
        projections: Vec::new(),
        source_loan: None,
    }
}

fn continuation(span: Span, target: MirBlockId) -> MirBasicBlock {
    MirBasicBlock {
        kind: MirBlockKind::Normal,
        statements: Vec::new(),
        terminator: MirTerminator {
            span,
            kind: MirTerminatorKind::Goto { target },
        },
    }
}

pub(super) fn lower(
    blocks: &mut Vec<MirBasicBlock>,
    interner: &TypeInterner,
    allocate: &mut impl FnMut(u32) -> Result<u32>,
) -> Result<()> {
    let mut lowering = Lowering {
        int: interner.scalar(ScalarType::Int),
        boolean: interner.scalar(ScalarType::Bool),
        allocate,
    };
    let original_count = blocks.len();
    let mut added = Vec::new();
    for block in blocks.iter_mut().filter(|b| b.kind == MirBlockKind::Normal) {
        for statement in &mut block.statements {
            if let MirStatementKind::Assign { value, .. } = &mut statement.kind
                && let MirRvalueKind::Prefix {
                    operator: HirPrefixOperator::BitwiseNot,
                    operand,
                } = &value.kind
                && let Some(integer) = NarrowInteger::of(operand.ty, interner)
                && !integer.signed
            {
                value.kind = MirRvalueKind::Binary {
                    operator: HirBinaryOperator::BitwiseXor,
                    left: operand.clone(),
                    right: lowering.constant(integer.mask()),
                };
            }
        }
        let MirTerminatorKind::Invoke {
            operation,
            destination,
            target: Some(target),
            unwind,
        } = &block.terminator.kind
        else {
            continue;
        };
        let Some(integer) = NarrowInteger::of(operation.ty, interner) else {
            continue;
        };
        let shift = match &operation.kind {
            MirOperationKind::CheckedBinary {
                operator,
                left,
                right,
            } if matches!(
                operator,
                HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight
            ) =>
            {
                Some((*operator, left.clone(), right.clone()))
            }
            MirOperationKind::CheckedBinary { .. } | MirOperationKind::CheckedPrefix { .. } => None,
            _ => continue,
        };
        let next = u32::try_from(original_count + added.len())
            .map(MirBlockId)
            .map_err(|_| "integer:block-limit")?;
        let target = *target;
        let unwind = *unwind;
        let destination = destination.clone();
        let span = block.terminator.span;
        if let Some((operator, left, right)) = shift {
            let mut success = continuation(span, target);
            let value =
                lowering.binary(&mut success, operator, left, right.clone(), lowering.int)?;
            let value = if operator == HirBinaryOperator::ShiftLeft {
                lowering.truncate(&mut success, value, integer)?
            } else {
                value
            };
            if let Some(destination) = destination {
                success.statements.push(MirStatement {
                    span,
                    kind: MirStatementKind::Assign {
                        destination,
                        value: MirRvalue {
                            ty: value.ty,
                            kind: MirRvalueKind::Use(value),
                        },
                    },
                });
            }
            lowering.guard(block, right, 0, i64::from(integer.bits - 1), next, unwind)?;
            added.push(success);
        } else {
            let temporary = (lowering.allocate)(1)?;
            let value = read(temporary, lowering.int);
            let mut range = continuation(span, target);
            let success_id = MirBlockId(next.index().checked_add(1).ok_or("integer:block-limit")?);
            let (minimum, maximum) = integer.bounds();
            lowering.guard(
                &mut range,
                value.clone(),
                minimum,
                maximum,
                success_id,
                unwind,
            )?;
            let mut success = continuation(span, target);
            if let Some(destination) = destination {
                success.statements.push(MirStatement {
                    span,
                    kind: MirStatementKind::Assign {
                        destination,
                        value: MirRvalue {
                            ty: value.ty,
                            kind: MirRvalueKind::Use(value),
                        },
                    },
                });
            }
            // Preserve the original checked i64 operation before the narrower
            // range guard. UInt32 multiplication may itself overflow i64;
            // such a result is necessarily also outside UInt32.
            let mut operation = operation.clone();
            operation.ty = lowering.int;
            block.terminator.kind = MirTerminatorKind::Invoke {
                operation,
                destination: Some(place(temporary, lowering.int)),
                target: Some(next),
                unwind,
            };
            added.extend([range, success]);
        }
    }
    blocks.extend(added);
    Ok(())
}
