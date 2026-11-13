//! The operations of `std.simd` (standard library 14.46): on a vector of
//! 128 bits, the generator's instruction for each lane; on a wider one, the
//! same on each of its parts, which it keeps in memory: parts of 256 bits
//! where the processor has AVX2, of 128 bits elsewhere.

use super::*;
use crate::simd::{Lanes, SimdOp};
use crate::types::Width;
use cranelift_codegen::ir::Endianness;

/// What a vector or a mask type is: its width, its lanes' type and kind,
/// whether it is a mask, and the bits of the parts it is computed by.
#[derive(Copy, Clone)]
struct Shape {
    width: Width,
    lane: Ty,
    lanes: Lanes,
    mask: bool,
    part: u32,
}

impl Shape {
    /// Its parts.
    fn parts(self) -> usize {
        (self.width.bits() / self.part) as usize
    }

    /// The bytes of a part.
    fn part_bytes(self) -> i32 {
        (self.part / 8) as i32
    }

    /// The lanes in each part.
    fn per_part(self) -> u32 {
        self.part / self.lanes.bits()
    }

    /// The same, by parts of 128 bits: for what moves lanes between parts,
    /// or takes them one at a time, in memory.
    fn narrow(self) -> Shape {
        Shape { part: 128, ..self }
    }

    fn count(self) -> u32 {
        self.per_part() * self.parts() as u32
    }
}

/// The same bits as lanes of another type, in the order of memory.
fn little() -> MemFlagsData {
    MemFlagsData::new().with_endianness(Endianness::Little)
}

impl<'a, 'b> FnCodegen<'a, 'b> {
    fn simd_shape(&mut self, ty: Ty) -> Shape {
        let types = self.e.cg.types();
        let (width, lane, mask) = match *types.kind(types.repr(ty)) {
            TyKind::Vector(width, lane) => (width, lane, false),
            TyKind::Mask(width, lane) => (width, lane, true),
            _ => unreachable!("not a vector: {}", types.display(ty)),
        };
        let lanes = Lanes::of(types, lane).expect("the checker allows only these");
        let part = if width.bits() >= 256 && self.accelerated(Width::W256) { 256 } else { 128 };
        Shape { width, lane, lanes, mask, part }
    }

    /// Whether the vectors of `width` are done with instructions of that
    /// width in this function's code: 128 bits on x64 and arm64, and 256
    /// bits on an x64 with AVX2 (rules 16 to 18 of the standard library's
    /// 14.46).
    fn accelerated(&self, width: Width) -> bool {
        match width.bits() {
            128 => true,
            256 => wide_vectors(self.e.cg.isa()),
            _ => false,
        }
    }

    /// The generator's type of a part of a vector of `shape`: a mask's
    /// lanes are integers of the lanes' width.
    fn part_type(&mut self, shape: Shape) -> ir::Type {
        let vector = vector_type(self.e.cg.types(), shape.lane).by(shape.part / 128).expect("a vector type");
        if shape.mask { vector.as_int() } else { vector }
    }

    /// The type of one lane of a part.
    fn lane_type(&mut self, shape: Shape) -> ir::Type {
        self.part_type(shape).lane_type()
    }

    /// The parts of a vector's value.
    fn parts_of(&mut self, operand: &Operand, shape: Shape) -> Vec<ir::Value> {
        let part = self.part_type(shape);
        match self.operand(operand) {
            // A register of 256 bits, by its halves.
            Val::Scalar(value) if self.e.fb.func.dfg.value_type(value).bits() > shape.part => {
                self.halves(value, shape).to_vec()
            }
            Val::Scalar(value) => vec![value],
            Val::Memory(address) => {
                let bytes = shape.part_bytes();
                (0..shape.parts()).map(|at| self.e.fb.ins().load(part, flags(), address, at as i32 * bytes)).collect()
            }
            Val::Zero => unreachable!("a vector has bits"),
        }
    }

    /// A value of the vector type `ty` from its parts: one register where
    /// the type is one, and else in memory.
    fn assembled(&mut self, parts: Vec<ir::Value>, ty: Ty) -> Val {
        let class = self.e.cg.class(ty);
        let part = self.e.fb.func.dfg.value_type(parts[0]);
        if let ([one], Class::Scalar(whole)) = (parts.as_slice(), class)
            && part == whole
        {
            return Val::Scalar(*one);
        }
        let bytes = part.bytes();
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, parts.len() as u32 * bytes, 4);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        for (at, part) in parts.into_iter().enumerate() {
            self.e.fb.ins().store(flags(), part, address, at as i32 * bytes as i32);
        }
        match class {
            Class::Scalar(whole) => Val::Scalar(self.e.fb.ins().load(whole, flags(), address, 0)),
            _ => Val::Memory(address),
        }
    }

    /// A vector's value in memory, where its lanes can be reached by index.
    fn simd_address(&mut self, operand: &Operand, shape: Shape) -> ir::Value {
        match self.operand(operand) {
            Val::Memory(address) => address,
            Val::Scalar(value) => {
                let bytes = self.e.fb.func.dfg.value_type(value).bytes();
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 4);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                self.e.fb.ins().store(flags(), value, address, 0);
                let _ = shape;
                address
            }
            Val::Zero => unreachable!("a vector has bits"),
        }
    }

    /// The operation `op` of `std.simd` on `args`, whose result has type
    /// `ty`.
    pub(super) fn simd(&mut self, op: SimdOp, args: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        use SimdOp::*;
        let _ = place;
        if let Accelerated(width) = op {
            let accelerated = self.accelerated(width) as i64;
            return Some(Val::Scalar(self.e.fb.ins().iconst(types::I8, accelerated)));
        }
        // The vector the operation is of: its first argument's, or what it
        // makes.
        let own = match op {
            Zero | Splat | Of | Load | Joined => ty,
            Store => self.operand_ty(&args[0]),
            _ => self.operand_ty(&args[0]),
        };
        let shape = self.simd_shape(own);
        // What moves lanes between parts or takes them one at a time does
        // it by parts of 128 bits.
        let shape = match op {
            Of | Lane | WithLane | MaskLane | Reversed | Swizzle | Joined | Lower | Upper | To => shape.narrow(),
            _ => shape,
        };
        let part = self.part_type(shape);
        let lane = self.lane_type(shape);
        let lanes = shape.lanes;
        let value = match op {
            Zero => {
                let zero = self.lane_zero(lane);
                let splat = self.e.fb.ins().splat(part, zero);
                self.assembled(vec![splat; shape.parts()], ty)
            }
            Splat => {
                let value = self.scalar(&args[0]);
                let splat = self.e.fb.ins().splat(part, value);
                self.assembled(vec![splat; shape.parts()], ty)
            }
            Of => self.simd_of(args, shape, ty, span),
            Load => {
                let array = self.scalar(&args[0]);
                let at = self.scalar(&args[1]);
                let array_ty = self.operand_ty(&args[0]);
                let address = self.lanes_address(array, array_ty, at, shape, span);
                let bytes = shape.part_bytes();
                let parts = (0..shape.parts())
                    .map(|p| self.e.fb.ins().load(part, flags(), address, p as i32 * bytes))
                    .collect();
                self.assembled(parts, ty)
            }
            Store => {
                let parts = self.parts_of(&args[0], shape);
                let Operand::Copy(target) = &args[1] else { unreachable!("an array is lent as its place") };
                let array_ty = self.place_ty(target);
                let element = self.e.cg.sequence_parts(array_ty).0;
                let (slot, spilled) = self.array_slot(target);
                self.make_unique(slot, element);
                let array = self.e.fb.ins().load(POINTER, flags(), slot, 0);
                if let Some(var) = spilled {
                    self.unspill(var, slot);
                }
                let at = self.scalar(&args[2]);
                let address = self.lanes_address(array, array_ty, at, shape, span);
                let bytes = shape.part_bytes();
                for (p, part) in parts.into_iter().enumerate() {
                    self.e.fb.ins().store(flags(), part, address, p as i32 * bytes);
                }
                Val::Zero
            }
            ToArray => {
                let parts = self.parts_of(&args[0], shape);
                let size = self.e.fb.ins().iconst(types::I64, (lanes.bits() / 8) as i64);
                let count = self.e.fb.ins().iconst(types::I64, shape.count() as i64);
                let array = self.e.call_rt(Rt::ArrayNew, &[size, count]).unwrap();
                let bytes = shape.part_bytes();
                for (p, part) in parts.into_iter().enumerate() {
                    self.e.fb.ins().store(flags(), part, array, ARRAY_HEADER as i32 + p as i32 * bytes);
                }
                Val::Scalar(array)
            }
            Lane | MaskLane => {
                let index = self.scalar(&args[1]);
                let address = self.simd_address(&args[0], shape);
                let at = self.lane_at(address, index, shape, span);
                let read = self.e.fb.ins().load(lane, flags(), at, 0);
                match op {
                    MaskLane => Val::Scalar(self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, read, 0)),
                    _ => Val::Scalar(read),
                }
            }
            WithLane => {
                let index = self.scalar(&args[1]);
                let value = self.scalar(&args[2]);
                // A copy, changed in one lane.
                let parts = self.parts_of(&args[0], shape);
                let copy = self.assembled(parts, ty);
                let address = match copy {
                    Val::Memory(address) => address,
                    Val::Scalar(vector) => {
                        let bytes = self.e.fb.func.dfg.value_type(vector).bytes();
                        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 4);
                        let slot = self.e.fb.create_sized_stack_slot(data);
                        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                        self.e.fb.ins().store(flags(), vector, address, 0);
                        address
                    }
                    Val::Zero => unreachable!(),
                };
                let at = self.lane_at(address, index, shape, span);
                self.e.fb.ins().store(flags(), value, at, 0);
                let parts =
                    (0..shape.parts()).map(|p| self.e.fb.ins().load(part, flags(), address, p as i32 * 16)).collect();
                self.assembled(parts, ty)
            }
            Add | Sub | Mul | Div | And | Or | Xor | Min | Max | SaturatingAdd | SaturatingSub => {
                let left = self.parts_of(&args[0], shape);
                let right = self.parts_of(&args[1], shape);
                let parts = left.into_iter().zip(right).map(|(a, b)| self.lanewise(op, shape, a, b)).collect();
                self.assembled(parts, ty)
            }
            Neg | Not | Abs | Sqrt | Floor | Ceil | Round | Truncate => {
                let parts = self.parts_of(&args[0], shape);
                let parts = parts.into_iter().map(|a| self.lane_unary(op, shape, a)).collect();
                self.assembled(parts, ty)
            }
            Shl | Shr => {
                let parts = self.parts_of(&args[0], shape);
                let count = self.scalar(&args[1]);
                let bits = lanes.bits() as i64;
                let signed = matches!(lanes, Lanes::Signed(_));
                let detail = (if op == Shl { 6 } else { 7 }) << 16 | 1 << 9 | (signed as u32) << 8 | lanes.bits();
                let outside = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, count, bits);
                let zero = self.lane_zero(lane);
                let lane_ty = shape.lane;
                self.panic_values_if(outside, 10, span, |_| (detail, zero, count, lane_ty));
                let parts = parts
                    .into_iter()
                    .map(|a| match (op, signed) {
                        (Shl, _) => self.e.fb.ins().ishl(a, count),
                        (_, true) => self.e.fb.ins().sshr(a, count),
                        _ => self.e.fb.ins().ushr(a, count),
                    })
                    .collect();
                self.assembled(parts, ty)
            }
            Fma => {
                let a = self.parts_of(&args[0], shape);
                let b = self.parts_of(&args[1], shape);
                let c = self.parts_of(&args[2], shape);
                let fused = calls::fused_multiply_add(self.e.cg.isa());
                let parts = (0..shape.parts())
                    .map(|p| match fused {
                        true => self.e.fb.ins().fma(a[p], b[p], c[p]),
                        // Lane by lane, which the generator rounds once
                        // through C's `fma` where the processor cannot.
                        false => {
                            let mut result = a[p];
                            for index in 0..shape.per_part() as u8 {
                                let x = self.e.fb.ins().extractlane(a[p], index);
                                let y = self.e.fb.ins().extractlane(b[p], index);
                                let z = self.e.fb.ins().extractlane(c[p], index);
                                let fused = self.e.fb.ins().fma(x, y, z);
                                result = self.e.fb.ins().insertlane(result, fused, index);
                            }
                            result
                        }
                    })
                    .collect();
                self.assembled(parts, ty)
            }
            Equal | NotEqual | Less | LessOrEqual | Greater | GreaterOrEqual => {
                let left = self.parts_of(&args[0], shape);
                let right = self.parts_of(&args[1], shape);
                let parts = left.into_iter().zip(right).map(|(a, b)| self.compare(op, shape, a, b)).collect();
                self.assembled(parts, ty)
            }
            Choose => {
                let mask = self.parts_of(&args[0], shape);
                let result_shape = Shape { part: shape.part, ..self.simd_shape(ty) };
                let yes = self.parts_of(&args[1], result_shape);
                let no = self.parts_of(&args[2], result_shape);
                let vector = self.part_type(result_shape);
                let parts = (0..shape.parts())
                    .map(|p| {
                        let mask = match vector == part {
                            true => mask[p],
                            false => self.e.fb.ins().bitcast(vector, little(), mask[p]),
                        };
                        self.e.fb.ins().bitselect(mask, yes[p], no[p])
                    })
                    .collect();
                self.assembled(parts, ty)
            }
            Any | All => {
                let parts = self.parts_of(&args[0], shape);
                let mut result = None;
                for part in parts {
                    let one = match op {
                        Any => self.e.fb.ins().vany_true(part),
                        _ => self.e.fb.ins().vall_true(part),
                    };
                    result = Some(match (result, op) {
                        (None, _) => one,
                        (Some(so_far), Any) => self.e.fb.ins().bor(so_far, one),
                        (Some(so_far), _) => self.e.fb.ins().band(so_far, one),
                    });
                }
                Val::Scalar(result.unwrap())
            }
            Count | Bits => {
                let parts = self.parts_of(&args[0], shape);
                let mut bits = self.e.fb.ins().iconst(types::I64, 0);
                for (p, part) in parts.into_iter().enumerate() {
                    let high = self.e.fb.ins().vhigh_bits(types::I64, part);
                    let shifted = self.e.fb.ins().ishl_imm_u(high, (p as u32 * shape.per_part()) as i64);
                    bits = self.e.fb.ins().bor(bits, shifted);
                }
                match op {
                    Count => Val::Scalar(self.e.fb.ins().popcnt(bits)),
                    _ => Val::Scalar(bits),
                }
            }
            Sum | MinLane | MaxLane => Val::Scalar(self.across(op, &args[0], shape)),
            Reversed => {
                let mut parts = self.parts_of(&args[0], shape);
                parts.reverse();
                let parts = parts.into_iter().map(|p| self.reverse_lanes(p, shape)).collect();
                self.assembled(parts, ty)
            }
            Swizzle => {
                let value = self.parts_of(&args[0], shape)[0];
                let indexes = self.parts_of(&args[1], shape)[0];
                Val::Scalar(self.e.fb.ins().swizzle(value, indexes))
            }
            Joined => {
                let half = Shape { width: if shape.width == Width::W512 { Width::W256 } else { Width::W128 }, ..shape };
                let mut parts = self.parts_of(&args[0], half);
                parts.extend(self.parts_of(&args[1], half));
                self.assembled(parts, ty)
            }
            Lower | Upper => {
                let parts = self.parts_of(&args[0], shape);
                let half = parts.len() / 2;
                let kept = if op == Lower { parts[..half].to_vec() } else { parts[half..].to_vec() };
                self.assembled(kept, ty)
            }
            Cast => {
                let target = Shape { part: shape.part, ..self.simd_shape(ty) };
                let target_part = self.part_type(target);
                let parts = self.parts_of(&args[0], shape);
                let parts = parts
                    .into_iter()
                    .map(|p| if target_part == part { p } else { self.e.fb.ins().bitcast(target_part, little(), p) })
                    .collect();
                self.assembled(parts, ty)
            }
            To => {
                let target = Shape { part: shape.part, ..self.simd_shape(ty) };
                let parts = self.parts_of(&args[0], shape);
                let parts = parts.into_iter().map(|p| self.convert(p, shape, target)).collect();
                self.assembled(parts, ty)
            }
            Same => {
                let left = self.parts_of(&args[0], shape);
                let right = self.parts_of(&args[1], shape);
                let mut all = None;
                for (a, b) in left.into_iter().zip(right) {
                    let equal = match lanes.is_float() && !shape.mask {
                        true => self.e.fb.ins().fcmp(FloatCC::Equal, a, b),
                        false => self.e.fb.ins().icmp(IntCC::Equal, a, b),
                    };
                    let every = self.e.fb.ins().vall_true(equal);
                    all = Some(match all {
                        None => every,
                        Some(so_far) => self.e.fb.ins().band(so_far, every),
                    });
                }
                Val::Scalar(all.unwrap())
            }
            Accelerated(_) => unreachable!("answered above"),
        };
        Some(value)
    }

    /// Zero of a lane's type.
    fn lane_zero(&mut self, lane: ir::Type) -> ir::Value {
        match lane {
            types::F32 => self.e.fb.ins().f32const(0.0),
            types::F64 => self.e.fb.ins().f64const(0.0),
            _ => self.e.fb.ins().iconst(lane, 0),
        }
    }

    /// `of(values...)`: the values in order, or one array of them whose
    /// length must be the lanes' count (P0006).
    fn simd_of(&mut self, args: &[Operand], shape: Shape, ty: Ty, span: Span) -> Val {
        let part = self.part_type(shape);
        let lane = self.lane_type(shape);
        let values: Vec<ir::Value> = match args {
            [array] if matches!(self.e.cg.types().kind(self.operand_ty(array)), TyKind::Array(_)) => {
                let array_ty = self.operand_ty(array);
                let array = self.scalar(array);
                let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
                let count = self.e.fb.ins().iconst(types::I64, shape.count() as i64);
                let wrong = self.e.fb.ins().icmp(IntCC::NotEqual, length, count);
                self.panic_values_if(wrong, 6, span, |_| (0, length, count, array_ty));
                let lane_bytes = (shape.lanes.bits() / 8) as i32;
                (0..shape.count())
                    .map(|i| self.e.fb.ins().load(lane, flags(), array, ARRAY_HEADER as i32 + i as i32 * lane_bytes))
                    .collect()
            }
            _ => args.iter().map(|a| self.scalar(a)).collect(),
        };
        let per = shape.per_part() as usize;
        let parts = values
            .chunks(per)
            .map(|chunk| {
                let mut vector = self.e.fb.ins().splat(part, chunk[0]);
                for (index, &value) in chunk.iter().enumerate().skip(1) {
                    vector = self.e.fb.ins().insertlane(vector, value, index as u8);
                }
                vector
            })
            .collect();
        self.assembled(parts, ty)
    }

    /// The address of the elements `at` to `at + lanes` of an array,
    /// panicking with `P0001` when they are not all inside it.
    fn lanes_address(&mut self, array: ir::Value, array_ty: Ty, at: ir::Value, shape: Shape, span: Span) -> ir::Value {
        let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
        let last = self.e.fb.ins().iadd_imm_s(at, shape.count() as i64);
        // `at` not negative and `at + lanes` not past the end.
        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, at, 0);
        let past = self.e.fb.ins().icmp(IntCC::SignedGreaterThan, last, length);
        let outside = self.e.fb.ins().bor(negative, past);
        self.panic_values_if(outside, 1, span, |_| (0x140, at, length, array_ty));
        self.element_at(array, array_ty, at)
    }

    /// The address of lane `index` of a vector in memory, panicking with
    /// `P0001` for an index that is not a lane.
    fn lane_at(&mut self, address: ir::Value, index: ir::Value, shape: Shape, span: Span) -> ir::Value {
        let count = self.e.fb.ins().iconst(types::I64, shape.count() as i64);
        let outside = self.e.fb.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, index, count);
        let lane_ty = shape.lane;
        self.panic_values_if(outside, 1, span, |_| (0x140, index, count, lane_ty));
        let offset = self.e.fb.ins().imul_imm_s(index, (shape.lanes.bits() / 8) as i64);
        self.e.fb.ins().iadd(address, offset)
    }

    /// The product of lanes of 8 bits, which x64 has no instruction for:
    /// the low bits of the products of the lanes widened to 16 bits.
    fn multiply_bytes(&mut self, a: ir::Value, b: ir::Value) -> ir::Value {
        // arm64 has it, and so has our generator of 256 bits on x64.
        if self.e.cg.isa().name() == "aarch64" || self.e.fb.func.dfg.value_type(a).bits() == 256 {
            return self.e.fb.ins().imul(a, b);
        }
        let low_byte = self.e.fb.ins().iconst(types::I16, 0xff);
        let low_byte = self.e.fb.ins().splat(types::I16X8, low_byte);
        let mut halves = [a; 2];
        for (half, low) in halves.iter_mut().zip([true, false]) {
            let (x, y) = match low {
                true => (self.e.fb.ins().uwiden_low(a), self.e.fb.ins().uwiden_low(b)),
                false => (self.e.fb.ins().uwiden_high(a), self.e.fb.ins().uwiden_high(b)),
            };
            let product = self.e.fb.ins().imul(x, y);
            *half = self.e.fb.ins().band(product, low_byte);
        }
        self.e.fb.ins().unarrow(halves[0], halves[1])
    }

    /// One operation of two operands on a part.
    fn lanewise(&mut self, op: SimdOp, shape: Shape, a: ir::Value, b: ir::Value) -> ir::Value {
        use SimdOp::*;
        let ins = self.e.fb.ins();
        match (op, shape.lanes) {
            (Add, Lanes::Float(_)) => ins.fadd(a, b),
            (Add, _) => ins.iadd(a, b),
            (Sub, Lanes::Float(_)) => ins.fsub(a, b),
            (Sub, _) => ins.isub(a, b),
            (Mul, Lanes::Float(_)) => ins.fmul(a, b),
            (Mul, _) if shape.lanes.bits() == 8 => self.multiply_bytes(a, b),
            (Mul, _) => ins.imul(a, b),
            (Div, _) => ins.fdiv(a, b),
            (And, _) => ins.band(a, b),
            (Or, _) => ins.bor(a, b),
            (Xor, _) => ins.bxor(a, b),
            (Min, Lanes::Float(_)) => ins.fmin(a, b),
            (Min, Lanes::Signed(_)) => ins.smin(a, b),
            (Min, _) => ins.umin(a, b),
            (Max, Lanes::Float(_)) => ins.fmax(a, b),
            (Max, Lanes::Signed(_)) => ins.smax(a, b),
            (Max, _) => ins.umax(a, b),
            (SaturatingAdd | SaturatingSub, lanes) => self.saturating(op, lanes, a, b),
            _ => unreachable!("not an operation of two vectors: {op:?}"),
        }
    }

    /// An operation of one operand on a part.
    fn lane_unary(&mut self, op: SimdOp, shape: Shape, a: ir::Value) -> ir::Value {
        use SimdOp::*;
        let ins = self.e.fb.ins();
        match (op, shape.lanes) {
            (Neg, Lanes::Float(_)) => ins.fneg(a),
            (Neg, _) => ins.ineg(a),
            (Not, _) => ins.bnot(a),
            (Abs, Lanes::Float(_)) => ins.fabs(a),
            (Abs, Lanes::Signed(_)) => ins.iabs(a),
            (Abs, _) => a,
            (Sqrt, _) => ins.sqrt(a),
            (Floor, _) => ins.floor(a),
            (Ceil, _) => ins.ceil(a),
            (Round, _) => ins.nearest(a),
            (Truncate, _) => ins.trunc(a),
            _ => unreachable!("not an operation of one vector: {op:?}"),
        }
    }

    /// A saturating sum or difference: the processor's for lanes of 8 and
    /// 16 bits; wider ones keep the wrapped result unless it overflowed,
    /// which the signs tell, and then the bound it went past.
    fn saturating(&mut self, op: SimdOp, lanes: Lanes, a: ir::Value, b: ir::Value) -> ir::Value {
        let add = op == SimdOp::SaturatingAdd;
        let ty = self.e.fb.func.dfg.value_type(a);
        if lanes.bits() <= 16 {
            let ins = self.e.fb.ins();
            return match (add, matches!(lanes, Lanes::Signed(_))) {
                (true, true) => ins.sadd_sat(a, b),
                (true, false) => ins.uadd_sat(a, b),
                (false, true) => ins.ssub_sat(a, b),
                (false, false) => ins.usub_sat(a, b),
            };
        }
        let bits = lanes.bits() as i64;
        let lane = ty.lane_type();
        let wrapped = if add { self.e.fb.ins().iadd(a, b) } else { self.e.fb.ins().isub(a, b) };
        match lanes {
            Lanes::Signed(_) => {
                // Overflow when the result's sign differs from both
                // operands' (a sum) or from the first's and not the
                // second's (a difference).
                let x = self.e.fb.ins().bxor(a, wrapped);
                let y = match add {
                    true => self.e.fb.ins().bxor(b, wrapped),
                    false => self.e.fb.ins().bxor(a, b),
                };
                let both = self.e.fb.ins().band(x, y);
                let zero = self.e.fb.ins().iconst(lane, 0);
                let zero = self.e.fb.ins().splat(ty, zero);
                let overflow = self.e.fb.ins().icmp(IntCC::SignedLessThan, both, zero);
                // The bound: the greatest for a positive `a`, the least
                // for a negative one.
                let sign = self.e.fb.ins().sshr_imm_u(a, bits - 1);
                let greatest = self.e.fb.ins().iconst(lane, (1i64 << (bits - 1)).wrapping_sub(1));
                let greatest = self.e.fb.ins().splat(ty, greatest);
                let bound = self.e.fb.ins().bxor(sign, greatest);
                self.e.fb.ins().bitselect(overflow, bound, wrapped)
            }
            _ => {
                let overflow = match add {
                    true => self.e.fb.ins().icmp(IntCC::UnsignedLessThan, wrapped, a),
                    false => self.e.fb.ins().icmp(IntCC::UnsignedLessThan, a, b),
                };
                let bound = match add {
                    true => self.e.fb.ins().iconst(lane, -1),
                    false => self.e.fb.ins().iconst(lane, 0),
                };
                let bound = self.e.fb.ins().splat(ty, bound);
                self.e.fb.ins().bitselect(overflow, bound, wrapped)
            }
        }
    }

    /// A comparison of a part's lanes: all ones where it holds.
    fn compare(&mut self, op: SimdOp, shape: Shape, a: ir::Value, b: ir::Value) -> ir::Value {
        use SimdOp::*;
        if shape.lanes.is_float() && !shape.mask {
            let condition = match op {
                Equal => FloatCC::Equal,
                NotEqual => FloatCC::NotEqual,
                Less => FloatCC::LessThan,
                LessOrEqual => FloatCC::LessThanOrEqual,
                Greater => FloatCC::GreaterThan,
                _ => FloatCC::GreaterThanOrEqual,
            };
            return self.e.fb.ins().fcmp(condition, a, b);
        }
        let signed = matches!(shape.lanes, Lanes::Signed(_));
        let condition = match (op, signed) {
            (Equal, _) => IntCC::Equal,
            (NotEqual, _) => IntCC::NotEqual,
            (Less, true) => IntCC::SignedLessThan,
            (Less, false) => IntCC::UnsignedLessThan,
            (LessOrEqual, true) => IntCC::SignedLessThanOrEqual,
            (LessOrEqual, false) => IntCC::UnsignedLessThanOrEqual,
            (Greater, true) => IntCC::SignedGreaterThan,
            (Greater, false) => IntCC::UnsignedGreaterThan,
            (_, true) => IntCC::SignedGreaterThanOrEqual,
            (_, false) => IntCC::UnsignedGreaterThanOrEqual,
        };
        self.e.fb.ins().icmp(condition, a, b)
    }

    /// `sum`, `minLane` or `maxLane`: lane `j` with lane `j + lanes / 2`,
    /// until one is left (rule 13).
    fn across(&mut self, op: SimdOp, operand: &Operand, shape: Shape) -> ir::Value {
        let mut parts = self.parts_of(operand, shape);
        let combine = |this: &mut Self, a: ir::Value, b: ir::Value| match op {
            SimdOp::Sum => this.lanewise(SimdOp::Add, shape, a, b),
            SimdOp::MinLane => this.lanewise(SimdOp::Min, shape, a, b),
            _ => this.lanewise(SimdOp::Max, shape, a, b),
        };
        while parts.len() > 1 {
            let half = parts.len() / 2;
            parts = (0..half).map(|p| combine(self, parts[p], parts[p + half])).collect();
        }
        // A part of 256 bits goes on by its halves, with the vector
        // operation of 128 bits as at every level: the same bits.
        let mut shape = shape;
        if shape.part == 256 {
            let [low, high] = self.halves(parts[0], shape);
            shape = shape.narrow();
            parts[0] = combine(self, low, high);
        }
        let part = parts[0];
        let mut lanes: Vec<ir::Value> =
            (0..shape.per_part() as u8).map(|index| self.e.fb.ins().extractlane(part, index)).collect();
        while lanes.len() > 1 {
            let half = lanes.len() / 2;
            lanes = (0..half).map(|j| combine(self, lanes[j], lanes[j + half])).collect();
        }
        lanes[0]
    }

    /// The halves of 128 bits of a part of 256, through memory.
    fn halves(&mut self, part: ir::Value, shape: Shape) -> [ir::Value; 2] {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 4);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        self.e.fb.ins().store(flags(), part, address, 0);
        let half = self.part_type(shape.narrow());
        [0, 16].map(|offset| self.e.fb.ins().load(half, flags(), address, offset))
    }

    /// A part with its lanes in the opposite order.
    fn reverse_lanes(&mut self, part: ir::Value, shape: Shape) -> ir::Value {
        let ty = self.e.fb.func.dfg.value_type(part);
        let bytes = (shape.lanes.bits() / 8) as usize;
        let mut order = Vec::with_capacity(16);
        for lane in (0..16 / bytes).rev() {
            order.extend((0..bytes).map(|byte| (lane * bytes + byte) as u8));
        }
        let mask = self.e.fb.func.dfg.immediates.push(order.into());
        let as_bytes = if ty == types::I8X16 { part } else { self.e.fb.ins().bitcast(types::I8X16, little(), part) };
        let reversed = self.e.fb.ins().shuffle(as_bytes, as_bytes, mask);
        if ty == types::I8X16 { reversed } else { self.e.fb.ins().bitcast(ty, little(), reversed) }
    }

    /// `to[U]`: each lane's value as a `U` of the same width, the nearest
    /// toward zero for a float, the greatest or the least where it does not
    /// fit, and zero for NaN.
    fn convert(&mut self, part: ir::Value, from: Shape, to: Shape) -> ir::Value {
        let target = self.part_type(to);
        let lane = target.lane_type();
        if from.lanes == to.lanes {
            return part;
        }
        let mut result = self.lane_zero(lane);
        result = self.e.fb.ins().splat(target, result);
        for index in 0..from.per_part() as u8 {
            let value = self.e.fb.ins().extractlane(part, index);
            let converted = match (from.lanes, to.lanes) {
                (Lanes::Float(_), Lanes::Signed(_)) => self.e.fb.ins().fcvt_to_sint_sat(lane, value),
                (Lanes::Float(_), _) => self.e.fb.ins().fcvt_to_uint_sat(lane, value),
                (Lanes::Signed(_), Lanes::Float(_)) => self.e.fb.ins().fcvt_from_sint(lane, value),
                (_, Lanes::Float(_)) => self.e.fb.ins().fcvt_from_uint(lane, value),
                // A negative value is zero as an unsigned one.
                (Lanes::Signed(_), _) => {
                    let zero = self.e.fb.ins().iconst(lane, 0);
                    self.e.fb.ins().smax(value, zero)
                }
                // An unsigned value past the greatest signed one is it.
                _ => {
                    let greatest = (1i64 << (from.lanes.bits() - 1)).wrapping_sub(1);
                    let greatest = self.e.fb.ins().iconst(lane, greatest);
                    self.e.fb.ins().umin(value, greatest)
                }
            };
            result = self.e.fb.ins().insertlane(result, converted, index);
        }
        result
    }
}
