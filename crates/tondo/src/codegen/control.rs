//! Loops, terminators and panics.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// The head of the loop that `terminator`, ending block `index`, jumps
    /// back to, when it is only a test: comparisons and a branch. A build
    /// that steps, counts or replaces code goes through the head itself.
    pub(super) fn loop_test(&self, index: usize, terminator: &Terminator) -> Option<&'a crate::mir::BlockData> {
        let Terminator::Goto(target) = terminator else { return None };
        let head = &self.body.blocks[target.0 as usize];
        let test = head.stmts.len() <= 2
            && head.stmts.iter().all(|stmt| {
                matches!(stmt, Statement::Assign(place, value, _)
                    if place.path.is_empty() && matches!(**value, Rvalue::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt
                        | BinOp::Ge | BinOp::Eq | BinOp::Ne, _, _)))
            })
            && matches!(head.terminator, Terminator::Branch(..));
        let plain = self.debug.is_none() && self.e.cg.coverage().is_none() && !self.e.cg.meta() && !self.e.cg.live();
        let Terminator::Branch(_, then, otherwise) = head.terminator else { return None };
        // One way of the test leaves the loop, so that its copy enters the
        // loop at one block, where Cranelift sees a loop it hoists from: a
        // test whose both ways go on in the loop, the first of `a and b`,
        // would enter it at two.
        let leaves = |way: crate::mir::BlockId| !self.reaches(way, index, target.0 as usize);
        (target.0 as usize <= index && test && plain && (leaves(then) || leaves(otherwise))).then_some(head)
    }

    /// Whether block `to` can be reached from `from` without passing
    /// through `head`.
    pub(super) fn reaches(&self, from: crate::mir::BlockId, to: usize, head: usize) -> bool {
        let mut seen = vec![false; self.body.blocks.len()];
        let mut pending = vec![from.0 as usize];
        while let Some(block) = pending.pop() {
            if block == to {
                return true;
            }
            if block == head || std::mem::replace(&mut seen[block], true) {
                continue;
            }
            match self.body.blocks[block].terminator {
                Terminator::Goto(next) => pending.push(next.0 as usize),
                Terminator::Branch(_, then, otherwise) => pending.extend([then.0 as usize, otherwise.0 as usize]),
                _ => {}
            }
        }
        false
    }

    pub(super) fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.e.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                // Every way out of a restart point is on the line its probe
                // was for: none probes it again.
                if self.carrying
                    && let Some(line) = self.debug.as_ref().and_then(|debug| debug.line.clone())
                {
                    self.carried.insert(then.0 as usize, line.clone());
                    self.carried.insert(otherwise.0 as usize, line);
                }
                let condition = self.scalar(condition);
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                // A condition known here, as a check for a panic that asks
                // nothing, is a jump.
                let dfg = &self.e.fb.func.dfg;
                let known = match dfg.value_def(condition) {
                    ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                        ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits() != 0),
                        _ => None,
                    },
                    _ => None,
                };
                match known {
                    Some(true) => self.e.fb.ins().jump(then, &[]),
                    Some(false) => self.e.fb.ins().jump(otherwise, &[]),
                    None => self.e.fb.ins().brif(condition, then, &[], otherwise, &[]),
                };
            }
            Terminator::Return(value) => match self.operand(value) {
                // A debugger stepping shows what the function returned.
                returned if self.debug.is_some() && self.operand_ty(value) != Ty::UNIT => {
                    let ty = self.operand_ty(value);
                    let address = self.in_memory(ty, returned);
                    let glue = self.e.glue_address(Glue::Show, ty);
                    let Some(debug) = self.debug.take() else { unreachable!("a debug build") };
                    let (site, ..) = self.debug_place(&debug);
                    self.debug = Some(debug);
                    self.e.call_rt(Rt::DebugReturned, &[site, glue, address]);
                    match returned {
                        Val::Scalar(value) => self.e.fb.ins().return_(&[value]),
                        Val::Memory(_) | Val::Zero => self.e.fb.ins().return_(&[]),
                    };
                }
                Val::Scalar(value) => {
                    self.e.fb.ins().return_(&[value]);
                }
                // Aggregates were already written through the result pointer.
                Val::Memory(_) | Val::Zero => {
                    self.e.fb.ins().return_(&[]);
                }
            },
            Terminator::Panic { code, message, span } => {
                self.at(*span);
                let message = match message {
                    Some(message) => self.scalar(message),
                    None => self.e.fb.ins().iconst(POINTER, 0),
                };
                self.keep_numbers();
                self.emit_panic(*code, message, *span);
            }
            Terminator::Unreachable => {
                self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
    }

    /// Before a panic the program wrote, for a dump: the numbers assigned
    /// by now, which the code after it would not need, each in its place
    /// after a word with the bits of those it holds.
    pub(super) fn keep_numbers(&mut self) {
        let Some((slot, kept)) = self.keeping.clone() else { return };
        let assigned = crate::mir::assigned_at_end(&self.body.blocks, &self.body.params, self.body.locals.len());
        let mut bits = 0u64;
        for (index, &(local, var)) in kept.iter().enumerate().take(63) {
            if assigned[self.block][local.0 as usize] {
                bits |= 1 << index;
                let value = self.e.fb.use_var(var);
                self.e.fb.ins().stack_store(POINTER, value, slot, (index as i32 + 1) * 8);
            }
        }
        let bits = self.e.fb.ins().iconst(types::I64, bits as i64);
        self.e.fb.ins().stack_store(POINTER, bits, slot, 0);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        self.e.call_rt(Rt::KeepNumbers, &[address]);
    }

    /// Before a panic, for a dump: the registers the runtime would change.
    pub(super) fn keep_registers(&mut self) {
        if self.e.cg.dumps() {
            self.e.call_rt(Rt::KeepRegisters, &[]);
        }
    }

    pub(super) fn emit_panic(&mut self, code: u32, message: ir::Value, span: Span) {
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let location = self.location(span);
        let outer = self.e.fb.srcloc();
        let site = self.site(span.start, None);
        self.e.fb.set_srcloc(site);
        self.keep_registers();
        self.e.call_rt(Rt::Panic, &[code, message, location]);
        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
        self.e.fb.set_srcloc(outer);
    }

    /// Branches to a cold block that panics with `code` when `condition`
    /// holds, with a message the runtime writes from the values `values`
    /// gives there: the detail, two values and the type they are about (see
    /// `tondo_rt_panic_values`).
    pub(super) fn panic_values_if(
        &mut self,
        condition: ir::Value,
        code: u32,
        span: Span,
        values: impl FnOnce(&mut Self) -> (u32, ir::Value, ir::Value, Ty),
    ) {
        let (failed, next) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.set_cold_block(failed);
        // On `not ok`, a branch on `ok` with the targets swapped, which
        // Cranelift makes one jump with the comparison that gave `ok`.
        let dfg = &self.e.fb.func.dfg;
        let constant = |value: ir::Value| match dfg.value_def(value) {
            ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits()),
                _ => None,
            },
            _ => None,
        };
        let negated = match dfg.value_def(condition) {
            ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                ir::InstructionData::Binary { opcode: ir::Opcode::Bxor, args: [ok, one] }
                    if constant(one) == Some(1) =>
                {
                    Some(ok)
                }
                _ => None,
            },
            _ => None,
        };
        match negated {
            Some(ok) => self.e.fb.ins().brif(ok, next, &[], failed, &[]),
            None => self.e.fb.ins().brif(condition, failed, &[], next, &[]),
        };
        self.e.fb.switch_to_block(failed);
        let (detail, a, b, ty) = values(self);
        let a = self.widest(a, detail & 0x100 != 0);
        let b = self.widest(b, detail & 0x200 != 0);
        let name = self.e.cg.types().display(ty);
        let name = self.e.string_ptr(&name);
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let detail = self.e.fb.ins().iconst(types::I32, detail as i64);
        let location = self.location(span);
        let outer = self.e.fb.srcloc();
        let site = self.site(span.start, None);
        self.e.fb.set_srcloc(site);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_store(POINTER, a, slot, 0);
        self.e.fb.ins().stack_store(POINTER, b, slot, 16);
        let values = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        match self.unwind_to {
            // A panic a `recover` may stop returns, and unwinds (15.9).
            Some(target) => {
                self.keep_registers();
                self.e.call_rt(Rt::PanicValuesUnwind, &[code, detail, values, name, location]);
                self.e.fb.ins().jump(target, &[]);
            }
            None => {
                self.keep_registers();
                self.e.call_rt(Rt::PanicValues, &[code, detail, values, name, location]);
                self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
        self.e.fb.set_srcloc(outer);
        self.e.fb.switch_to_block(next);
    }

    /// Whether a statement's panics are all checks of its own, whose cold
    /// paths jump to the unwinding: no call that may panic and return.
    pub(super) fn inline_only(&mut self, statement: &Statement) -> bool {
        let mut scalar = |operand: &Operand| match operand {
            Operand::Copy(place) => {
                let ty = self.body.place_ty(self.e.cg.types(), place);
                matches!(self.e.cg.class(ty), Class::Scalar(_) | Class::Zero)
            }
            Operand::Const(_) => true,
        };
        match statement {
            Statement::Assign(_, rvalue, _) => match &**rvalue {
                Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b) => scalar(a) && scalar(b),
                Rvalue::Unary(_, a) | Rvalue::Increment(a) => scalar(a),
                Rvalue::Use(_)
                | Rvalue::Aggregate(_)
                | Rvalue::Variant(..)
                | Rvalue::Discriminant(_)
                | Rvalue::Length(_)
                | Rvalue::Layout(..)
                | Rvalue::Convert(_) => true,
                _ => false,
            },
            _ => true,
        }
    }
}
