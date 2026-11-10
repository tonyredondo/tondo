//! Unary and binary operators, and text.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// An integer as 128 bits.
    pub(super) fn widest(&mut self, value: ir::Value, signed: bool) -> ir::Value {
        match self.e.fb.func.dfg.value_type(value) {
            types::I128 => value,
            _ if signed => self.e.fb.ins().sextend(types::I128, value),
            _ => self.e.fb.ins().uextend(types::I128, value),
        }
    }

    /// An integer as 64 bits.
    pub(super) fn wide(&mut self, value: ir::Value, signed: bool) -> ir::Value {
        match self.e.fb.func.dfg.value_type(value) {
            types::I64 => value,
            _ if signed => self.e.fb.ins().sextend(types::I64, value),
            _ => self.e.fb.ins().uextend(types::I64, value),
        }
    }

    // ---- operators -----------------------------------------------------------

    pub(super) fn unary(&mut self, op: UnaryOp, operand: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.scalar(operand);
        match op {
            UnaryOp::Not => self.e.fb.ins().bxor_imm_u(value, 1),
            UnaryOp::BitNot => self.e.fb.ins().bnot(value),
            UnaryOp::Neg if matches!(self.e.cg.types().kind(ty), TyKind::Float(_)) => self.e.fb.ins().fneg(value),
            UnaryOp::Neg => {
                let clif = self.e.cg.scalar_type(ty);
                let zero = self.e.int_const(clif, 0);
                let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, value);
                let ty = self.e.cg.types().repr(ty);
                self.panic_values_if(overflow, 5, span, |_| (5 << 16 | 0x100 | clif.bits(), value, value, ty));
                result
            }
        }
    }

    /// `x = "{x}text"`: the text's bytes written after the string's own,
    /// as `push` adds an element, when nothing else holds the string and
    /// its room takes them; or else the runtime's append.
    pub(super) fn append_text(&mut self, place: &Place, text: &str) -> ir::Value {
        let string = self.scalar(&Operand::Copy(place.clone()));
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), string);
        let length = self.e.fb.ins().load(types::I64, flags(), string, STRING_LENGTH);
        let room = self.e.fb.ins().load(types::I64, flags(), string, STRING_ROOM);
        let longer = self.e.fb.ins().iadd_imm_s(length, text.len() as i64);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let fits = self.e.fb.ins().icmp(IntCC::UnsignedLessThanOrEqual, longer, room);
        let in_place = self.e.fb.ins().band(unique, fits);
        let (fast, slow, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        let result = self.e.fb.append_block_param(done, POINTER);
        self.e.fb.ins().brif(in_place, fast, &[], slow, &[]);
        self.e.fb.switch_to_block(fast);
        let end = self.e.fb.ins().iadd(string, length);
        // The bytes in words of 8, 4, 2 and 1 bytes, stored as constants.
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            let (width, ty) = match bytes.len() - at {
                8.. => (8, types::I64),
                4..=7 => (4, types::I32),
                2..=3 => (2, types::I16),
                _ => (1, types::I8),
            };
            let mut word = [0u8; 8];
            word[..width].copy_from_slice(&bytes[at..at + width]);
            let value = self.e.fb.ins().iconst(ty, u64::from_le_bytes(word) as i64);
            let unaligned = MemFlagsData::new().with_notrap();
            self.e.fb.ins().store(unaligned, value, end, (STRING_BYTES + at as i64) as i32);
            at += width;
        }
        self.e.fb.ins().store(flags(), longer, string, STRING_LENGTH);
        self.e.fb.ins().jump(done, &[string.into()]);
        self.e.fb.set_cold_block(slow);
        self.e.fb.switch_to_block(slow);
        let text = self.e.string_ptr(text);
        let appended = self.e.call_rt(Rt::StrAppend, &[string, text]).unwrap();
        self.e.fb.ins().jump(done, &[appended.into()]);
        self.e.fb.switch_to_block(done);
        result
    }

    /// The room the frame keeps for the text made at `place`, when it is
    /// one only the frame reads (`Body.framed`).
    pub(super) fn framed_room(&mut self, place: &Place) -> Option<ir::Value> {
        if !place.path.is_empty() || !self.body.framed.contains(&place.local) {
            return None;
        }
        let slot = *self.framed.entry(place.local).or_insert_with(|| {
            let data = StackSlotData::new(StackSlotKind::ExplicitSlot, STRING_BYTES as u32 + FRAMED_ROOM, 3);
            self.e.fb.create_sized_stack_slot(data)
        });
        Some(self.e.fb.ins().stack_addr(POINTER, slot, 0))
    }

    /// The text the pieces make, in `framed`, the room of the frame, when
    /// it is one only the frame reads and fits there.
    pub(super) fn concat(&mut self, pieces: &[Piece], framed: Option<ir::Value>) -> ir::Value {
        // A type shown as what it is made of: one that shows itself, as a
        // `Duration` made of an `Int`, goes through its `Display`.
        let plain = |this: &Self, ty: Ty| {
            let types = this.e.cg.types();
            let shows = this.e.cg.display().contains_key(&ty) || types.shown.contains_key(&ty);
            (!shows).then(|| types.kind(types.repr(ty)).clone())
        };
        // `x = "{x}text"` with a short text known now, in place.
        let known = match pieces {
            [Piece::Taken(place), Piece::Text(text) | Piece::Value(Operand::Const(Const::Str(text)), _)] => {
                Some((place, text.clone()))
            }
            [Piece::Taken(place), Piece::Value(Operand::Copy(of), _)] if of.path.is_empty() => {
                self.literal.get(&of.local).cloned().flatten().map(|text| (place, text))
            }
            _ => None,
        };
        if let Some((place, text)) = known
            && (1..=16).contains(&text.len())
        {
            return self.append_text(place, &text);
        }
        // `x = "{x}{s}"` and `x = "{x}{i}"` in one call.
        if let [Piece::Taken(place), Piece::Value(operand, ty)] = pieces
            && let Some(kind) = plain(self, *ty)
        {
            let appended = match kind {
                TyKind::String => Some((Rt::StrAppend, false)),
                TyKind::Int(int) if int.bits() < 128 && (int.signed() || int.bits() < 64) => {
                    Some((Rt::StrAppendInt, int.signed()))
                }
                _ => None,
            };
            if let Some((rt, signed)) = appended {
                let text = self.scalar(&Operand::Copy(place.clone()));
                let value = self.scalar(operand);
                let value = if rt == Rt::StrAppendInt { self.wide(value, signed) } else { value };
                return self.e.call_rt(rt, &[text, value]).unwrap();
            }
        }
        // An integer alone, `"{i}"`, in one call.
        if let [Piece::Value(operand, ty)] = pieces
            && let Some(TyKind::Int(int)) = plain(self, *ty)
            && int.bits() < 128
            && (int.signed() || int.bits() < 64)
        {
            let value = self.scalar(operand);
            let value = self.wide(value, int.signed());
            return match framed {
                Some(room) => self.e.call_rt(Rt::IntTextInto, &[value, room]).unwrap(),
                None => self.e.call_rt(Rt::IntText, &[value]).unwrap(),
            };
        }
        // A string alone is that string.
        if let [Piece::Value(operand, ty)] = pieces
            && plain(self, *ty).is_some()
            && self.e.cg.types().repr(*ty) == Ty::STRING
        {
            let text = self.scalar(operand);
            self.e.call_rt(Rt::StrRetain, &[text]);
            return text;
        }
        // Room for the text and the strings, and a guess for the rest, so
        // that the text is allocated once.
        let mut known = 0;
        let mut room = None;
        let mut values = Vec::new();
        let mut taken = None;
        let mut released = None;
        for piece in pieces {
            match piece {
                Piece::Text(text) => known += text.len() as i64,
                Piece::Taken(place) => taken = Some((place, self.scalar(&Operand::Copy(place.clone())))),
                Piece::Value(operand, ty) => {
                    let value = self.operand(operand);
                    match (self.e.cg.types().kind(self.e.cg.types().repr(*ty)), value) {
                        (TyKind::String, Val::Scalar(text)) => {
                            let length = self.e.fb.ins().load(types::I64, flags(), text, STRING_LENGTH);
                            room = Some(room.map_or(length, |room| self.e.fb.ins().iadd(room, length)));
                        }
                        (TyKind::Int(int), _) if int.bits() == 128 => known += 40,
                        // At most one digit more than it has, and its sign.
                        (TyKind::Int(_), Val::Scalar(value)) => {
                            let value = self.wide(value, true);
                            let sign = self.e.fb.ins().sshr_imm_u(value, 63);
                            let magnitude = self.e.fb.ins().bxor(value, sign);
                            let magnitude = self.e.fb.ins().bor_imm_u(magnitude, 1);
                            let zeros = self.e.fb.ins().clz(magnitude);
                            let sixty_four = self.e.fb.ins().iconst(types::I64, 64);
                            let bits = self.e.fb.ins().isub(sixty_four, zeros);
                            let digits = self.e.fb.ins().imul_imm_u(bits, 1233);
                            let digits = self.e.fb.ins().ushr_imm_u(digits, 12);
                            room = Some(room.map_or(digits, |room| self.e.fb.ins().iadd(room, digits)));
                            known += 2;
                        }
                        (TyKind::Int(_) | TyKind::Float(_), _) => known += 24,
                        _ => known += 16,
                    }
                    values.push(value);
                }
            }
        }
        let room = match room {
            Some(room) => self.e.fb.ins().iadd_imm_s(room, known),
            None => self.e.fb.ins().iconst(types::I64, known),
        };
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let builder = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        match taken {
            // The string goes on in its own block when nothing else holds
            // it; else it is copied, and released once the text is built.
            Some((_, text)) => {
                let adopted = self.e.call_rt(Rt::SbAdopt, &[builder, text, room]).unwrap();
                released = Some((adopted, text));
            }
            None => match framed {
                Some(framed) => {
                    let framed_room = self.e.fb.ins().iconst(types::I64, FRAMED_ROOM as i64);
                    self.e.call_rt(Rt::SbStartIn, &[builder, room, framed, framed_room]);
                }
                None => {
                    self.e.call_rt(Rt::SbStart, &[builder, room]);
                }
            },
        }
        let mut values = values.into_iter();
        for piece in pieces {
            match piece {
                Piece::Taken(_) => {}
                Piece::Text(text) => {
                    let text = self.e.string_ptr(text);
                    self.e.call_rt(Rt::SbStr, &[builder, text]);
                }
                Piece::Value(_, ty) => {
                    let value = values.next().unwrap();
                    self.e.append_display(builder, *ty, value);
                }
            }
        }
        let built = self.e.call_rt(Rt::SbFinish, &[builder]).unwrap();
        if let Some((adopted, text)) = released {
            let (release, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            self.e.fb.ins().brif(adopted, next, &[], release, &[]);
            self.e.fb.switch_to_block(release);
            self.e.call_rt(Rt::StrRelease, &[text]);
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        built
    }

    /// `a * b` and whether it overflows, when one factor is a constant
    /// that Cranelift multiplies by without a multiplication: 3, 5, 7, 9
    /// or a power of two (`Tondo:` in its `opts/arithmetic.isle`). The
    /// product overflows when the other factor is out of the range whose
    /// products fit, which waits for nothing, as the flag of a
    /// multiplication waits for it: `3 * n + 1` is then one `lea` on
    /// x86-64.
    fn multiplied(&mut self, a: ir::Value, b: ir::Value, int: Option<IntTy>) -> Option<(ir::Value, ir::Value)> {
        let int = int.filter(|int| int.bits() <= 64)?;
        let constant = |fb: &FunctionBuilder, value: ir::Value| {
            let ir::ValueDef::Result(inst, _) = fb.func.dfg.value_def(value) else { return None };
            match fb.func.dfg.insts[inst] {
                ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits()),
                _ => None,
            }
        };
        let (factor, c) = match (constant(&self.e.fb, a), constant(&self.e.fb, b)) {
            (_, Some(c)) => (a, c),
            (Some(c), _) => (b, c),
            _ => return None,
        };
        if !by_range(c as i128, int.bits()) {
            return None;
        }
        let clif = self.e.fb.func.dfg.value_type(factor);
        let out = if int.signed() {
            let (low, high) = (int.min() / c as i128, int.max() as i128 / c as i128);
            let low = self.e.int_const(clif, low);
            let offset = self.e.fb.ins().isub(factor, low);
            let span = self.e.int_const(clif, high - int.min() / c as i128);
            self.e.fb.ins().icmp(IntCC::UnsignedGreaterThan, offset, span)
        } else {
            let high = self.e.int_const(clif, (int.max() / c as u128) as i128);
            self.e.fb.ins().icmp(IntCC::UnsignedGreaterThan, factor, high)
        };
        Some((self.e.fb.ins().imul(a, b), out))
    }

    pub(super) fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        self.compared(op, left, right, ty);
        let is_array = |ty: Ty| matches!(self.e.cg.types().kind(self.e.cg.types().repr(ty)), TyKind::Array(_));
        if !matches!(op, BinOp::Eq | BinOp::Ne) && (is_array(ty) || is_array(right_ty)) {
            return self.array_arithmetic(op, left, right, span);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let (a, b) = (self.operand(left), self.operand(right));
            // Integers, characters and booleans differ by one comparison,
            // which a branch on it takes as is.
            let types = self.e.cg.types();
            if op == BinOp::Ne
                && let (Val::Scalar(a), Val::Scalar(b)) = (a, b)
                && matches!(types.kind(types.repr(ty)), TyKind::Int(_) | TyKind::Char | TyKind::Bool)
            {
                return self.e.fb.ins().icmp(IntCC::NotEqual, a, b);
            }
            let equal = self.e.equal(ty, a, b);
            return if op == BinOp::Eq { equal } else { self.e.fb.ins().bxor_imm_u(equal, 1) };
        }
        let (a, b) = (self.scalar(left), self.scalar(right));
        if self.e.cg.types().repr(ty) == Ty::STRING {
            let order = self.e.call_rt(Rt::StrCmp, &[a, b]).unwrap();
            return self.e.fb.ins().icmp_imm_s(int_condition(op, true), order, 0);
        }
        let types = self.e.cg.types();
        if let TyKind::Float(_) = types.kind(ty) {
            // IEEE 754 without traps (18.5); relational comparisons with NaN are false.
            let ins = self.e.fb.ins();
            return match op {
                BinOp::Add => ins.fadd(a, b),
                BinOp::Sub => ins.fsub(a, b),
                BinOp::Mul => ins.fmul(a, b),
                BinOp::Div => ins.fdiv(a, b),
                BinOp::Lt => ins.fcmp(FloatCC::LessThan, a, b),
                BinOp::Le => ins.fcmp(FloatCC::LessThanOrEqual, a, b),
                BinOp::Gt => ins.fcmp(FloatCC::GreaterThan, a, b),
                BinOp::Ge => ins.fcmp(FloatCC::GreaterThanOrEqual, a, b),
                _ => unreachable!("rejected by the checker"),
            };
        }
        // A newtype computes as the integer it holds.
        let ty = types.repr(ty);
        let signed = types.as_int(ty).is_some_and(IntTy::signed);
        let right_signed = types.as_int(types.repr(right_ty)).is_some_and(IntTy::signed);
        // What the runtime needs to tell the operation in a panic.
        let operation = |op: BinOp| {
            let code = match op {
                BinOp::Add => 0,
                BinOp::Sub => 1,
                BinOp::Mul => 2,
                BinOp::Div => 3,
                BinOp::Rem => 4,
                BinOp::Shl => 6,
                _ => 7,
            };
            let bits = self.e.cg.types().as_int(ty).map_or(8, IntTy::bits);
            code << 16 | (right_signed as u32) << 9 | (signed as u32) << 8 | bits
        };
        let detail = operation(op);
        // A product by a constant 1 is the other factor, and cannot
        // overflow: no multiply, which qemu's arm64 emulation gets wrong
        // for `smulh` by a known 1.
        let one = |fb: &FunctionBuilder, value: ir::Value| {
            let ir::ValueDef::Result(inst, _) = fb.func.dfg.value_def(value) else { return false };
            matches!(fb.func.dfg.insts[inst], ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm }
                if imm.bits() == 1)
        };
        if op == BinOp::Mul && (one(&self.e.fb, a) || one(&self.e.fb, b)) {
            return if one(&self.e.fb, b) { a } else { b };
        }
        let wide = self.e.fb.func.dfg.value_type(a) == types::I128;
        match op {
            // Cranelift multiplies 128 bits, but does not tell an overflow.
            BinOp::Mul if wide => {
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let (result, overflow) = self.int128_call(Rt::Int128Multiply, &[signed_flag], a, b);
                self.panic_values_if(overflow.unwrap(), 5, span, |_| (detail, a, b, ty));
                result
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let constant = match op {
                    BinOp::Mul => self.multiplied(a, b, types.as_int(ty)),
                    _ => None,
                };
                let ins = self.e.fb.ins();
                let (result, overflow) = match (op, signed) {
                    _ if let Some(found) = constant => found,
                    (BinOp::Add, true) => ins.sadd_overflow(a, b),
                    (BinOp::Add, false) => ins.uadd_overflow(a, b),
                    (BinOp::Sub, true) => ins.ssub_overflow(a, b),
                    (BinOp::Sub, false) => ins.usub_overflow(a, b),
                    (_, true) => ins.smul_overflow(a, b),
                    (_, false) => ins.umul_overflow(a, b),
                };
                // The left operand of a sum or a difference, for the panic's
                // message, from the result, which wraps: the code that goes on
                // needs to keep only the result.
                self.panic_values_if(overflow, 5, span, |this| {
                    let left = match op {
                        BinOp::Add => this.e.fb.ins().isub(result, b),
                        BinOp::Sub => this.e.fb.ins().iadd(result, b),
                        _ => a,
                    };
                    (detail, left, b, ty)
                });
                result
            }
            BinOp::Div | BinOp::Rem => {
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.panic_values_if(zero, 3, span, |_| (detail, a, b, ty));
                let int = types.as_int(ty).unwrap();
                let clif = int_type(int);
                // Cranelift does not divide 128 bits: the runtime does.
                let divide = |this: &mut Self, divisor: ir::Value| {
                    let remainder = this.e.fb.ins().iconst(types::I8, (op == BinOp::Rem) as i64);
                    let signed = this.e.fb.ins().iconst(types::I8, signed as i64);
                    this.int128_call(Rt::Int128Divide, &[remainder, signed], a, divisor).0
                };
                if !signed {
                    return match (wide, op) {
                        (true, _) => divide(self, b),
                        (_, BinOp::Div) => self.e.fb.ins().udiv(a, b),
                        _ => self.e.fb.ins().urem(a, b),
                    };
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let min = self.e.int_const(clif, int.min());
                let minus_one = self.e.int_const(clif, -1);
                let is_min = self.e.fb.ins().icmp(IntCC::Equal, a, min);
                let is_minus_one = self.e.fb.ins().icmp(IntCC::Equal, b, minus_one);
                let both = self.e.fb.ins().band(is_min, is_minus_one);
                if wide {
                    if op == BinOp::Div {
                        self.panic_values_if(both, 5, span, |_| (detail, a, b, ty));
                        return divide(self, b);
                    }
                    let one = self.e.int_const(clif, 1);
                    let divisor = self.e.fb.ins().select(both, one, b);
                    return divide(self, divisor);
                }
                if op == BinOp::Div {
                    self.panic_values_if(both, 5, span, |_| (detail, a, b, ty));
                    self.e.fb.ins().sdiv(a, b)
                } else {
                    let one = self.e.int_const(int_type(int), 1);
                    let divisor = self.e.fb.ins().select(both, one, b);
                    self.e.fb.ins().srem(a, divisor)
                }
            }
            BinOp::BitAnd => self.e.fb.ins().band(a, b),
            BinOp::BitOr => self.e.fb.ins().bor(a, b),
            BinOp::BitXor => self.e.fb.ins().bxor(a, b),
            BinOp::Shl | BinOp::Shr => {
                let width = self.e.cg.scalar_type(ty).bits() as i64;
                if types.as_int(right_ty).is_some_and(IntTy::signed) {
                    let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                    self.panic_values_if(negative, 10, span, |_| (detail, a, b, ty));
                }
                let too_wide = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b, width);
                self.panic_values_if(too_wide, 10, span, |_| (detail, a, b, ty));
                // The count is below the width: a count of 128 bits fits in
                // 64, and one of 128 bits shifts by a count of 64.
                let b = match self.e.fb.func.dfg.value_type(b) {
                    types::I128 => self.e.fb.ins().ireduce(types::I64, b),
                    count if wide && count != types::I64 => self.e.fb.ins().uextend(types::I64, b),
                    _ => b,
                };
                match (op, signed) {
                    (BinOp::Shl, _) => self.e.fb.ins().ishl(a, b),
                    (_, true) => self.e.fb.ins().sshr(a, b),
                    (_, false) => self.e.fb.ins().ushr(a, b),
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => self.e.fb.ins().icmp(int_condition(op, signed), a, b),
            BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
                unreachable!("handled above or lowered before codegen")
            }
        }
    }
}

/// Whether a checked product by the constant `c`, of an integer of `bits`,
/// is checked by the range of the other factor (`multiplied`).
pub(crate) fn by_range(c: i128, bits: u32) -> bool {
    bits <= 64 && c >= 2 && c < 1 << (bits - 2) && ((c as u64).is_power_of_two() || [3, 5, 7, 9].contains(&c))
}
