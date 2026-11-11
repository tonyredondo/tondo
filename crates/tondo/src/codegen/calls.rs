//! Calls: direct, indirect, of the runtime and that may fail.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    pub(super) fn call(&mut self, callee: Callee, args: &[Operand], result: Option<ir::Value>) -> Option<ir::Value> {
        if let Callee::Extern(id) = callee {
            let declared = &self.e.cg.externs()[id as usize];
            let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
            return self.c_call(CTarget::Symbol(symbol), &params, result_ty, args, result);
        }
        // A frame that moves passes its parameters, and where the result
        // goes, from where the runtime says, a word each.
        let moved = self.osr_params.take();
        let result = match (moved, result) {
            (Some(base), Some(_)) => Some(self.e.fb.ins().load(POINTER, flags(), base, 0)),
            _ => result,
        };
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        let moved_word = |fb: &mut FunctionBuilder, index: usize, ty: ir::Type| {
            moved.map(|base| fb.ins().load(ty, flags(), base, (index as i32 + 1) * 8))
        };
        let loans = match callee {
            Callee::Fn(id) => self.e.cg.loans(id),
            Callee::Std(_) => Vec::new(),
            Callee::Trait(..) | Callee::Extern(_) => unreachable!("handled before"),
        };
        let mut spilled = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if loans.get(index).copied().unwrap_or(false) {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                let address = self.loan_address(place, &mut spilled);
                values.push(moved_word(&mut self.e.fb, index, POINTER).unwrap_or(address));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) => {
                    let ty = self.e.fb.func.dfg.value_type(value);
                    values.push(moved_word(&mut self.e.fb, index, ty).unwrap_or(value));
                }
                Val::Memory(value) => values.push(moved_word(&mut self.e.fb, index, POINTER).unwrap_or(value)),
                Val::Zero => {}
            }
        }
        let result = match callee {
            Callee::Fn(id) => {
                let state = self.osr_arg.take();
                match self.unwind_to.filter(|_| state.is_none() && self.e.cg.slot_of(id).is_none()) {
                    Some(unwind) => {
                        let func = self.e.cg.function(id);
                        let callee = self.e.cg.declare_func_in_func(func, self.e.fb.func);
                        self.try_call(Called::Direct(callee), &values, unwind, &spilled)
                    }
                    None => {
                        let call = call_body_with(self.e.cg, &mut self.e.fb, id, &values, state);
                        self.e.fb.inst_results(call).first().copied()
                    }
                }
            }
            Callee::Std(StdFn::ConsolePrint) => self.e.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.e.call_rt(Rt::ConsolePrintln, &values),
            Callee::Std(StdFn::ConsoleSet(index)) => {
                let index = self.e.fb.ins().iconst(types::I64, index as i64);
                self.e.call_rt(Rt::ConsoleSet, &[index, values[0]])
            }
            Callee::Std(StdFn::Number(_)) => unreachable!("assigned by `number`"),
            Callee::Std(StdFn::Native(index)) => {
                // Arithmetic the processor does in one instruction, as
                // `hir::IN_ONE_INSTRUCTION` lists it.
                let ins = self.e.fb.ins();
                match crate::hir::NATIVES[index as usize].symbol {
                    "tondo_rt_math_sqrt" => Some(ins.sqrt(values[0])),
                    "tondo_rt_math_floor" => Some(ins.floor(values[0])),
                    "tondo_rt_math_ceil" => Some(ins.ceil(values[0])),
                    "tondo_rt_math_trunc" => Some(ins.trunc(values[0])),
                    "tondo_rt_math_round" => Some(ins.nearest(values[0])),
                    // What a model's marks need, only in a build of tests.
                    "tondo_rt_model_recording" => Some(ins.iconst(types::I8, self.e.cg.testing() as i64)),
                    _ => self.e.call_rt(Rt::Native(index), &values),
                }
            }
            Callee::Std(StdFn::Process(ProcessFn::Exit)) => self.e.call_rt(Rt::Process(ProcessFn::Exit), &values),
            Callee::Std(StdFn::Process(ProcessFn::Detach)) => self.e.call_rt(Rt::Process(ProcessFn::Detach), &values),
            Callee::Std(StdFn::Process(ProcessFn::Signal)) => {
                // The variant of `process.Signal`.
                let signal = self.operand_ty(&args[1]);
                let value = self.operand(&args[1]);
                let tag = self.e.tag(signal, value);
                let tag = self.e.fb.ins().uextend(types::I64, tag);
                self.e.call_rt(Rt::Process(ProcessFn::Signal), &[values[0], tag])
            }
            Callee::Std(StdFn::ConsoleEprint) => self.e.call_rt(Rt::ConsoleEprint, &values),
            Callee::Std(StdFn::ConsoleEprintln) => self.e.call_rt(Rt::ConsoleEprintln, &values),
            Callee::Std(StdFn::Io(IoFn::BufferClear)) => self.e.call_rt(Rt::Io(IoFn::BufferClear), &values),
            Callee::Std(StdFn::TimeSleep) => self.e.call_rt(Rt::Sleep, &values),
            Callee::Std(StdFn::TestLog) => self.e.call_rt(Rt::TestLog, &values),
            Callee::Std(StdFn::TestTags) => self.e.call_rt(Rt::TestTags, &values),
            Callee::Std(StdFn::TestWhere) => {
                let location = self.location(self.span);
                self.e.call_rt(Rt::TestWhere, &[location])
            }
            Callee::Std(StdFn::PanicUnwind(code)) => {
                let code = self.e.fb.ins().iconst(types::I32, i64::from(code));
                let location = self.location(self.span);
                values.insert(0, code);
                values.push(location);
                self.keep_registers();
                self.e.call_rt(Rt::PanicUnwind, &values)
            }
            Callee::Std(StdFn::ContextGet(index)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                self.e.call_rt(Rt::ContextGet, &[index])
            }
            Callee::Std(StdFn::ContextField(index, field)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                let field = self.e.fb.ins().iconst(types::I64, i64::from(field));
                let location = self.location(self.span);
                self.e.call_rt(Rt::ContextField, &[index, field, location])
            }
            Callee::Std(StdFn::ContextSet(index, fields)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                let fields = self.e.fb.ins().iconst(types::I64, fields as i64);
                // Where it is, for the tools that show which `with` set a
                // field (toolchain 7.6).
                let at = format!("{} in {}", self.e.cg.sources().location(self.span), self.body.name);
                let at = self.e.string_ptr(&at);
                self.e.call_rt(Rt::ContextSet, &[index, values[0], fields, at])
            }
            Callee::Std(StdFn::ContextRestore) => self.e.call_rt(Rt::ContextRestore, &values),
            Callee::Std(StdFn::RecoverEnter) => self.e.call_rt(Rt::RecoverEnter, &[]),
            Callee::Std(StdFn::RecoverEnd) => self.e.call_rt(Rt::RecoverEnd, &[]),
            Callee::Std(StdFn::RecoverLeave) => self.e.call_rt(Rt::RecoverLeave, &[]),
            Callee::Std(StdFn::RecoverCatch) => self.e.call_rt(Rt::RecoverCatch, &[]),
            Callee::Std(StdFn::WithinEnter) => self.e.call_rt(Rt::WithinEnter, &values),
            Callee::Std(StdFn::SpawnDetached) => self.e.call_rt(Rt::SpawnDetached, &[]),
            Callee::Std(StdFn::WithinLeave) => self.e.call_rt(Rt::WithinLeave, &[]),
            Callee::Std(StdFn::WithinCatch) => self.e.call_rt(Rt::WithinCatch, &[]),
            Callee::Std(StdFn::UnwindHold) => self.e.call_rt(Rt::UnwindHold, &[]),
            Callee::Std(StdFn::PanicHeld) => self.e.call_rt(Rt::PanicHeld, &[]),
            // A function that returns from here goes to its caller's way,
            // through its frame's return address (15.9).
            Callee::Std(StdFn::UnwindResume) if self.returns => self.e.call_rt(Rt::UnwindReturn, &[]),
            Callee::Std(StdFn::UnwindResume) => self.e.call_rt(Rt::UnwindResume, &[]),
            Callee::Std(StdFn::DeferEnd) => self.e.call_rt(Rt::DeferEnd, &[]),
            Callee::Std(StdFn::CancelReturn) => self.e.call_rt(Rt::CancelReturn, &[]),
            Callee::Std(StdFn::ClockBegin) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::ClockBegin, &values)
            }
            Callee::Std(StdFn::ClockEnd) => self.e.call_rt(Rt::ClockEnd, &values),
            Callee::Std(StdFn::BenchStart) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::BenchStart, &values)
            }
            Callee::Std(StdFn::BenchNext) => self.e.call_rt(Rt::BenchNext, &values),
            Callee::Std(StdFn::BenchEnd) => self.e.call_rt(Rt::BenchEnd, &values),
            Callee::Std(StdFn::Keep) => {
                // The value as a word the runtime is given, so that its
                // computation stays.
                let fb = &mut self.e.fb;
                let word = match values.first().copied() {
                    None => fb.ins().iconst(types::I64, 0),
                    Some(value) => match fb.func.dfg.value_type(value) {
                        types::I64 => value,
                        types::F64 => fb.ins().bitcast(types::I64, MemFlagsData::new(), value),
                        types::F32 => {
                            let bits = fb.ins().bitcast(types::I32, MemFlagsData::new(), value);
                            fb.ins().uextend(types::I64, bits)
                        }
                        types::I128 => fb.ins().ireduce(types::I64, value),
                        _ => fb.ins().uextend(types::I64, value),
                    },
                };
                self.e.call_rt(Rt::Keep, &[word])
            }
            Callee::Std(StdFn::ClockSettle) => self.e.call_rt(Rt::ClockSettle, &values),
            Callee::Std(StdFn::ClockAdvance) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::ClockAdvance, &values)
            }
            Callee::Std(function @ (StdFn::TestAttach | StdFn::TestSnapshot)) => {
                // Their panics name where they were called.
                values.push(self.location(self.span));
                let rt = if function == StdFn::TestAttach { Rt::TestAttach } else { Rt::TestSnapshot };
                self.e.call_rt(rt, &values)
            }
            Callee::Std(
                StdFn::Fs(_)
                | StdFn::Path(_)
                | StdFn::Io(_)
                | StdFn::Env(_)
                | StdFn::Process(_)
                | StdFn::TimeNow
                | StdFn::TimeUnix
                | StdFn::TimeUtc
                | StdFn::TimeUtcNow
                | StdFn::ConsoleFlush
                | StdFn::ConsoleReadLine
                | StdFn::ConsoleStream(_),
            ) => unreachable!("built where they are assigned"),
            Callee::Trait(..) | Callee::Extern(_) => unreachable!("handled before"),
        };
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
    }

    /// Gives the runner the members of a suite: `[node, glue, fn value]`
    /// for each, read while it runs them; then releases the values.
    pub(super) fn test_members(&mut self, members: &[(u32, Operand, bool)]) {
        // `[node, glue, fn value]` for each member, read by the
        // runner while it runs them.
        let size = (members.len() as u32 * 24).max(8);
        let slot = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let mut objects = Vec::new();
        for (index, (node, member, retain)) in members.iter().enumerate() {
            let ty = self.operand_ty(member);
            let object = self.scalar(member);
            if *retain {
                self.e.refcount(ty, Val::Scalar(object), false);
            }
            let glue = self.e.cg.glue(Glue::Test, ty);
            let glue = self.e.cg.declare_func_in_func(glue, self.e.fb.func);
            let glue = self.e.fb.ins().func_addr(POINTER, glue);
            let node = self.e.fb.ins().iconst(types::I64, *node as i64);
            let offset = index as i32 * 24;
            self.e.fb.ins().store(flags(), node, address, offset);
            self.e.fb.ins().store(flags(), glue, address, offset + 8);
            self.e.fb.ins().store(flags(), object, address, offset + 16);
            objects.push((ty, object));
        }
        let count = self.e.fb.ins().iconst(types::I64, members.len() as i64);
        self.e.call_rt(Rt::TestMembers, &[address, count]);
        for (ty, object) in objects {
            self.e.refcount(ty, Val::Scalar(object), true);
        }
    }

    /// A call through a `fn(...)` value: its code receives the captures'
    /// address first (see `Codegen::uniform_signature`).
    pub(super) fn call_indirect(
        &mut self,
        callee: &Operand,
        args: &[Operand],
        result: Option<ir::Value>,
    ) -> Option<ir::Value> {
        if let TyKind::CFn(params, result_ty, _) = self.e.cg.types().kind(self.operand_ty(callee)).clone() {
            let code = self.scalar(callee);
            return self.c_call(CTarget::Address(code), &params, result_ty, args, result);
        }
        let TyKind::Fn(params, result_ty, _, modes, _) = self.e.cg.types().kind(self.operand_ty(callee)).clone() else {
            unreachable!("an indirect call goes through a `fn(...)` value")
        };
        let loans: Vec<bool> = modes.iter().map(|m| matches!(m, Mode::Mut | Mode::Var)).collect();
        let object = self.scalar(callee);
        let code = self.e.fb.ins().load(POINTER, flags(), object, 16);
        let captures = self.e.fb.ins().iadd_imm_s(object, FN_HEADER);
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        values.push(captures);
        let mut spilled = Vec::new();
        for (arg, &loan) in args.iter().zip(&loans) {
            if loan {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                values.push(self.loan_address(place, &mut spilled));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) | Val::Memory(value) => values.push(value),
                Val::Zero => {}
            }
        }
        let signature = self.e.cg.uniform_signature(&params, &loans, result_ty);
        let signature = self.e.fb.import_signature(signature);
        let result = match self.unwind_to.filter(|_| !self.e.cg.live()) {
            Some(unwind) => self.try_call(Called::Indirect(signature, code), &values, unwind, &spilled),
            None => {
                let call = self.e.fb.ins().call_indirect(signature, code, &values);
                self.e.fb.inst_results(call).first().copied()
            }
        };
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
    }

    /// A call of compiled code that a panic may unwind (15.9), as a
    /// `try_call`: when the callee unwinds, the runtime makes it return to
    /// the handler, which goes the way the check after the call would have.
    /// A call that returns as usual costs nothing more, and needs no check.
    pub(super) fn try_call(
        &mut self,
        callee: Called,
        args: &[ir::Value],
        unwind: ir::Block,
        spilled: &[(Variable, ir::Value)],
    ) -> Option<ir::Value> {
        use cranelift_codegen::ir::{BlockArg, BlockCall, ExceptionTableData, ExceptionTableItem};
        let signature = match callee {
            Called::Direct(callee) => self.e.fb.func.dfg.ext_funcs[callee].signature,
            Called::Indirect(signature, _) => signature,
        };
        let returns: Vec<ir::Type> =
            self.e.fb.func.dfg.signatures[signature].returns.iter().map(|r| r.value_type).collect();
        let (next, handler) = (self.e.fb.create_block(), self.e.fb.create_block());
        for &ty in &returns {
            self.e.fb.append_block_param(next, ty);
        }
        let pool = &mut self.e.fb.func.dfg.value_lists;
        let normal = BlockCall::new(next, (0..returns.len() as u32).map(BlockArg::TryCallRet), pool);
        let caught = BlockCall::new(handler, [], pool);
        let table = ExceptionTableData::new(signature, normal, [ExceptionTableItem::Default(caught)]);
        let table = self.e.fb.func.dfg.exception_tables.push(table);
        match callee {
            Called::Direct(callee) => self.e.fb.ins().try_call(callee, args, table),
            Called::Indirect(_, code) => self.e.fb.ins().try_call_indirect(code, args, table),
        };
        self.e.fb.switch_to_block(handler);
        self.e.fb.set_cold_block(handler);
        for &(var, slot) in spilled {
            self.unspill(var, slot);
        }
        self.e.fb.ins().jump(unwind, &[]);
        self.e.fb.switch_to_block(next);
        self.tried = true;
        self.e.fb.block_params(next).first().copied()
    }

    /// The address a loan of `place` passes; a local held in a variable is
    /// spilled to the stack and recorded in `spilled` to reload after the call.
    pub(super) fn loan_address(&mut self, place: &Place, spilled: &mut Vec<(Variable, ir::Value)>) -> ir::Value {
        if self.e.cg.class(self.place_ty(place)) == Class::Zero {
            return self.e.fb.ins().iconst(POINTER, 0);
        }
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            let slot = self.spill(var);
            spilled.push((var, slot));
            return slot;
        }
        self.pointer_to(place, true)
    }
}
