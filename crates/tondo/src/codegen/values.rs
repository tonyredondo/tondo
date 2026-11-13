//! Values of the runtime: shared and atomic cells, processes, files, conversions, numbers.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// A value of zeros, which releases nothing.
    pub(super) fn zeroed(&mut self, ty: Ty) -> Val {
        match self.e.cg.class(ty) {
            Class::Zero => Val::Zero,
            Class::Scalar(t) if t.is_float() => {
                let zero = self.e.fb.ins().f64const(0.0);
                Val::Scalar(if t == types::F32 { self.e.fb.ins().fdemote(types::F32, zero) } else { zero })
            }
            Class::Scalar(t) => Val::Scalar(self.e.int_const(t, 0)),
            Class::Memory => {
                let address = self.scratch(ty);
                let layout = self.e.cg.layout(ty);
                let config = self.e.cg.isa().frontend_config();
                let align = layout.align.clamp(1, 8) as u8;
                self.e.fb.emit_small_memset(config, address, 0, layout.size as u64, align, flags());
                Val::Memory(address)
            }
        }
    }

    /// The address of a value an operation reads, retained first when the
    /// operation takes it over and it was copied rather than moved.
    pub(super) fn address_of(&mut self, operand: &Operand, retain: bool) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.operand(operand);
        if retain {
            self.e.refcount(ty, value, false);
        }
        self.copy_to_stack(ty, value)
    }

    /// A shared collection made from the value collection it takes over.
    pub(super) fn shared_new(&mut self, handle: Handle, state: Ty, source: &Operand, retain: bool) -> ir::Value {
        let value = self.operand(source);
        if retain {
            self.e.refcount(state, value, false);
        }
        let pointer = match value {
            Val::Scalar(pointer) => pointer,
            _ => unreachable!("collections are pointers"),
        };
        if let Handle::SyncMap | Handle::SyncSet = handle {
            let (size, offset) = self.e.cg.entry_layout(state);
            let size = self.e.fb.ins().iconst(POINTER, size as i64);
            let offset = self.e.fb.ins().iconst(POINTER, offset as i64);
            let glue = self.e.map_glue_address(state);
            return self.e.call_rt(Rt::SharedMapFrom, &[pointer, size, offset, glue]).unwrap();
        }
        let (element, _) = self.e.cg.sequence_parts(state);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let visit = self.e.glue_or_null(Glue::Visit, element);
        let rt = match handle {
            Handle::SyncQueue => Rt::SharedQueueFrom,
            Handle::SyncStack => Rt::SharedStackFrom,
            _ => Rt::SharedArrayFrom,
        };
        self.e.call_rt(rt, &[pointer, size, retain, release, visit]).unwrap()
    }

    /// `present` and the address of the payload of an option value, or of
    /// scratch space when it has no payload.
    pub(super) fn option_parts(&mut self, ty: Ty, value: Val) -> (ir::Value, ir::Value) {
        let TyKind::Option(payload) = *self.e.cg.types().kind(ty) else { unreachable!() };
        let tag = self.e.tag(ty, value);
        let present = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, tag, 0);
        let address = match value {
            Val::Memory(base) => {
                let slot = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                self.e.offset(base, slot.offset)
            }
            _ => self.scratch(payload),
        };
        let null = self.e.fb.ins().iconst(POINTER, 0);
        (present, self.e.fb.ins().select(present, address, null))
    }

    /// An operation of a shared collection (see the runtime's `shared`).
    pub(super) fn shared_op(
        &mut self,
        op: SharedOp,
        operands: &[Operand],
        retain: &[bool],
        place: &Place,
    ) -> Option<Val> {
        let ty = self.place_ty(place);
        let TyKind::Handle(handle, state) = *self.e.cg.types().kind(self.operand_ty(&operands[0])) else {
            unreachable!("a shared collection")
        };
        let collection = self.scalar(&operands[0]);
        let map = matches!(handle, Handle::SyncMap | Handle::SyncSet);
        let (key, value) = if map { self.e.cg.map_parts(state) } else { (Ty::INT, self.e.cg.sequence_parts(state).0) };
        let rt = |op: SharedOp| match (handle, op) {
            (Handle::SyncMap | Handle::SyncSet, SharedOp::Length) => Rt::SharedMapLength,
            (Handle::SyncMap | Handle::SyncSet, _) => Rt::SharedMapSnapshot,
            (Handle::SyncQueue, SharedOp::Length) => Rt::SharedQueueLength,
            (Handle::SyncQueue, _) => Rt::SharedQueueSnapshot,
            (Handle::SyncStack, SharedOp::Length) => Rt::SharedStackLength,
            (Handle::SyncStack, _) => Rt::SharedStackSnapshot,
            (_, SharedOp::Length) => Rt::SharedArrayLength,
            _ => Rt::SharedArraySnapshot,
        };
        match op {
            SharedOp::Length | SharedOp::Snapshot => Some(Val::Scalar(self.e.call_rt(rt(op), &[collection]).unwrap())),
            SharedOp::Contains => {
                let key = self.address_of(&operands[1], false);
                let null = self.e.fb.ins().iconst(POINTER, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SharedMapGet, &[collection, key, null]).unwrap()))
            }
            SharedOp::Get | SharedOp::Pop | SharedOp::Remove if !(handle == Handle::SyncSet) => {
                let out = self.scratch(value);
                let found = match (handle, op) {
                    (Handle::SyncMap, SharedOp::Get) | (Handle::SyncMap, _) => {
                        let key = self.address_of(&operands[1], false);
                        let rt = if op == SharedOp::Get { Rt::SharedMapGet } else { Rt::SharedMapRemove };
                        self.e.call_rt(rt, &[collection, key, out])
                    }
                    (_, SharedOp::Pop) => {
                        let rt = if handle == Handle::SyncQueue { Rt::SharedQueuePop } else { Rt::SharedStackPop };
                        self.e.call_rt(rt, &[collection, out])
                    }
                    _ => {
                        let index = self.scalar(&operands[1]);
                        let rt = match handle {
                            Handle::SyncQueue => Rt::SharedQueueGet,
                            Handle::SyncStack => Rt::SharedStackGet,
                            _ => Rt::SharedArrayGet,
                        };
                        self.e.call_rt(rt, &[collection, index, out])
                    }
                };
                self.removal_result(place, ty, value, out, found.unwrap())
            }
            SharedOp::Remove => {
                let key = self.address_of(&operands[1], false);
                let null = self.e.fb.ins().iconst(POINTER, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SharedMapRemove, &[collection, key, null]).unwrap()))
            }
            SharedOp::Insert => {
                let key_address = self.address_of(&operands[1], retain[1]);
                let _ = key;
                if handle == Handle::SyncSet {
                    let unit = self.e.fb.ins().iconst(POINTER, 8);
                    let keep = self.e.fb.ins().iconst(types::I8, 1);
                    let present =
                        self.e.call_rt(Rt::SharedMapInsert, &[collection, key_address, unit, unit, keep]).unwrap();
                    return Some(Val::Scalar(self.e.fb.ins().icmp_imm_s(IntCC::Equal, present, 0)));
                }
                let value_address = self.address_of(&operands[2], retain[2]);
                let old = self.scratch(value);
                let keep = self.e.fb.ins().iconst(types::I8, 0);
                let present =
                    self.e.call_rt(Rt::SharedMapInsert, &[collection, key_address, value_address, old, keep]).unwrap();
                self.removal_result(place, ty, value, old, present)
            }
            SharedOp::Get | SharedOp::Pop => unreachable!("sets have no `get` or `pop`"),
            SharedOp::Push => {
                let address = self.address_of(&operands[1], retain[1]);
                let rt = if handle == Handle::SyncQueue { Rt::SharedQueuePush } else { Rt::SharedStackPush };
                self.e.call_rt(rt, &[collection, address]);
                None
            }
            SharedOp::Set => {
                let index = self.scalar(&operands[1]);
                let address = self.address_of(&operands[2], retain[2]);
                let old = self.scratch(value);
                let found = self.e.call_rt(Rt::SharedArraySet, &[collection, index, address, old]).unwrap();
                self.removal_result(place, ty, value, old, found)
            }
            SharedOp::Exchange => {
                let Operand::Copy(seen) = &operands[4] else { unreachable!("the place for the value seen") };
                // Zeros stay when nothing was seen; they release nothing.
                let out = self.scratch(value);
                let config = self.e.cg.isa().frontend_config();
                let size = self.e.cg.layout(value).size;
                self.e.fb.emit_small_memset(config, out, 0, size as u64, 8, flags());
                let equal = self.e.glue_address(Glue::Equal, value);
                let code = if map {
                    let key = self.address_of(&operands[1], false);
                    let expected_ty = self.operand_ty(&operands[2]);
                    let expected = self.operand(&operands[2]);
                    let (_, expected) = self.option_parts(expected_ty, expected);
                    let desired_ty = self.operand_ty(&operands[3]);
                    let desired = self.operand(&operands[3]);
                    if retain[3] {
                        self.e.refcount(desired_ty, desired, false);
                    }
                    let desired = match desired {
                        Val::Scalar(_) => self.in_memory(desired_ty, desired),
                        _ => self.copy_to_stack(desired_ty, desired),
                    };
                    let (_, desired) = self.option_parts(desired_ty, Val::Memory(desired));
                    self.e.call_rt(Rt::SharedMapExchange, &[collection, key, expected, desired, out, equal])
                } else {
                    let index = self.scalar(&operands[1]);
                    let expected = self.address_of(&operands[2], false);
                    let desired = self.address_of(&operands[3], retain[3]);
                    self.e.call_rt(Rt::SharedArrayExchange, &[collection, index, expected, desired, out, equal])
                };
                let value = self.e.value_at(out, value);
                self.write(seen, value);
                Some(Val::Scalar(self.e.fb.ins().uextend(types::I64, code.unwrap())))
            }
        }
    }

    /// An operation of an atomic integer, `Bool`, `Char` or `Byte`.
    pub(super) fn atomic_op(&mut self, op: AtomicOp, operands: &[Operand], ty: Ty, span: Span) -> Val {
        let TyKind::Handle(_, state) = *self.e.cg.types().kind(self.operand_ty(&operands[0])) else { unreachable!() };
        let Class::Scalar(clif) = self.e.cg.class(state) else { unreachable!("a scalar") };
        let cell = self.scalar(&operands[0]);
        let width = self.e.fb.ins().iconst(types::I8, clif.bytes() as i64);
        let word = |this: &mut Self, operand: &Operand| {
            let value = this.scalar(operand);
            if clif == types::I64 { value } else { this.e.fb.ins().uextend(types::I64, value) }
        };
        let result = match op {
            AtomicOp::Load => self.e.call_rt(Rt::AtomicLoad, &[cell, width]),
            AtomicOp::Store => {
                let value = word(self, &operands[1]);
                self.e.call_rt(Rt::AtomicStore, &[cell, width, value]);
                return Val::Zero;
            }
            AtomicOp::Swap => {
                let value = word(self, &operands[1]);
                self.e.call_rt(Rt::AtomicSwap, &[cell, width, value])
            }
            AtomicOp::Exchange => {
                let expected = word(self, &operands[1]);
                let desired = word(self, &operands[2]);
                self.e.call_rt(Rt::AtomicExchange, &[cell, width, expected, desired])
            }
            AtomicOp::Add => {
                let delta = word(self, &operands[1]);
                let signed = self.e.cg.types().as_int(state).is_some_and(|i| i.signed());
                let signed = self.e.fb.ins().iconst(types::I8, signed as i64);
                let location = self.location(span);
                self.e.call_rt(Rt::AtomicAdd, &[cell, width, signed, delta, location])
            }
        };
        let _ = ty;
        let result = result.unwrap();
        Val::Scalar(if clif == types::I64 { result } else { self.e.fb.ins().ireduce(clif, result) })
    }

    pub(super) fn removal_result(
        &mut self,
        place: &Place,
        ty: Ty,
        element: Ty,
        out: ir::Value,
        found: ir::Value,
    ) -> Option<Val> {
        if self.e.cg.class(ty) != Class::Memory {
            // The element has no size: the option is just its tag.
            return Some(Val::Scalar(found));
        }
        let base = self.pointer_to(place, true);
        self.e.fb.ins().store(flags(), found, base, 0);
        let (some, done) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(found, some, &[], done, &[]);
        self.e.fb.switch_to_block(some);
        let value = self.e.value_at(out, element);
        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
        self.e.init_component(base, &payload, value);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
        None
    }

    pub(super) fn scalar_of(&mut self, value: Val) -> ir::Value {
        match value {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar"),
        }
    }

    /// The option of a value a runtime function gives as a pointer, null
    /// for `none`; stored at `place` unless returned.
    pub(super) fn pointer_option(&mut self, place: &Place, ty: Ty, element: Ty, pointer: ir::Value) -> Option<Val> {
        let out = self.scratch(element);
        self.e.fb.ins().store(flags(), pointer, out, 0);
        let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, pointer, 0);
        self.removal_result(place, ty, element, out, found)
    }

    /// A call of `std.fs`, `std.io` or `console.flush` (see the runtime's
    /// `fs` and `io`): the runtime returns 0 with the value in `out`, or the
    /// index of the error variant plus one; this builds the `T ! E` at
    /// `place`.
    pub(super) fn status_call(&mut self, rt: Rt, args: &[Operand], place: &Place) {
        let ty = self.place_ty(place);
        let mut values: Vec<ir::Value> = Vec::new();
        for arg in args {
            let value = self.scalar(arg);
            let value = if self.e.fb.func.dfg.value_type(value) == types::I64 {
                value
            } else {
                self.e.fb.ins().uextend(types::I64, value)
            };
            values.push(value);
        }
        let TyKind::Result(success, _) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
        // The value comes as one word.
        let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
        let returns_value = success != Ty::UNIT;
        if returns_value {
            values.push(out);
        }
        let status = self.e.call_rt(rt, &values).unwrap();
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.e.fb.ins().store(flags(), failed, base, 0);
        let (good, bad, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, bad, &[], good, &[]);
        self.e.fb.switch_to_block(bad);
        let error = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        let variant = self.e.fb.ins().iadd_imm_s(status, -1);
        let variant = self.e.fb.ins().ireduce(types::I8, variant);
        self.e.fb.ins().store(flags(), variant, base, error.offset as i32);
        // `FsError.Host` holds the system's number of the error, which the
        // runtime kept for this thread.
        let host = self
            .e
            .cg
            .types()
            .variants(error.ty)
            .and_then(|variants| variants.iter().position(|payload| payload.as_slice() == [Ty::INT]));
        if let Some(host) = host {
            let (is_host, other) = (self.e.fb.create_block(), self.e.fb.create_block());
            let matched = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, host as i64 + 1);
            self.e.fb.ins().brif(matched, is_host, &[], other, &[]);
            self.e.fb.switch_to_block(is_host);
            let number = self.e.call_rt(Rt::FsHostError, &[]).unwrap();
            let payload = self.e.cg.slot(error.ty, Proj::Variant(host as u32, 0));
            self.e.fb.ins().store(flags(), number, base, (error.offset + payload.offset) as i32);
            self.e.fb.ins().jump(other, &[]);
            self.e.fb.switch_to_block(other);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(good);
        if returns_value {
            let value = self.fs_value(success, out);
            let slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
            self.e.init_component(base, &slot, value);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// A call of `std.process`, its value when it is not built at `place`.
    pub(super) fn process_call(&mut self, function: ProcessFn, args: &[Operand], place: &Place) -> Option<Val> {
        let ty = self.place_ty(place);
        let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar_or_address(a)).collect();
        let rt = Rt::Process(function);
        let flag = |this: &mut Self, on: bool| this.e.fb.ins().iconst(types::I8, on as i64);
        match function {
            ProcessFn::Configure(setting) => {
                let setting_value = self.e.fb.ins().iconst(types::I64, setting as i64);
                let zero = self.e.fb.ins().iconst(types::I64, 0);
                let (a, b) = match setting {
                    0 | 2 | 9 | 10 => (values[1], zero),
                    1 => (values[1], values[2]),
                    3 | 7 | 8 => (zero, zero),
                    // An `Input` or `Output`: its tag and the pointer its
                    // payload holds, the same place in every variant.
                    _ => {
                        let enum_ty = self.operand_ty(&args[1]);
                        let value = self.operand(&args[1]);
                        let tag = self.e.tag(enum_ty, value);
                        let tag = self.e.fb.ins().uextend(types::I64, tag);
                        let payload = match value {
                            Val::Memory(address) => {
                                let variants = self.e.cg.types().variants(enum_ty).unwrap();
                                let carrying = variants.iter().position(|v| !v.is_empty()).unwrap();
                                let slot = self.e.cg.slot(enum_ty, Proj::Variant(carrying as u32, 0));
                                self.e.fb.ins().load(types::I64, flags(), address, slot.offset as i32)
                            }
                            _ => zero,
                        };
                        (tag, payload)
                    }
                };
                let plan = values[0];
                return Some(Val::Scalar(self.e.call_rt(rt, &[plan, setting_value, a, b]).unwrap()));
            }
            ProcessFn::Command | ProcessFn::Shell | ProcessFn::Pipe | ProcessFn::Id => {
                return Some(Val::Scalar(self.e.call_rt(rt, &values).unwrap()));
            }
            ProcessFn::Signals => {
                // The array of signals, the stride of its elements, and the
                // size and alignment of the channel's values.
                let TyKind::Handle(_, signal) = *self.e.cg.types().kind(ty) else { unreachable!("a receiver") };
                let stride = self.e.element_size(signal);
                let layout = self.e.cg.layout(signal);
                let [size, align] =
                    [layout.size as i64, layout.align as i64].map(|value| self.e.fb.ins().iconst(types::I64, value));
                return Some(Val::Scalar(self.e.call_rt(rt, &[values[0], stride, size, align]).unwrap()));
            }
            ProcessFn::Stream(which) => {
                let which = self.e.fb.ins().iconst(types::I64, which as i64);
                return Some(Val::Scalar(self.e.call_rt(rt, &[values[0], which]).unwrap()));
            }
            ProcessFn::Text => {
                self.status_call(rt, args, place);
                return None;
            }
            ProcessFn::Status | ProcessFn::Wait | ProcessFn::Cancel => {
                let on = matches!(function, ProcessFn::Status | ProcessFn::Cancel);
                values.push(flag(self, on));
            }
            ProcessFn::Run
            | ProcessFn::Output
            | ProcessFn::Check
            | ProcessFn::ReadText
            | ProcessFn::ReadLines
            | ProcessFn::Collect(_) => {
                // `run` fails as `check` does, without capturing; `text` and
                // `lines` are what `check` captured.
                let check = match function {
                    ProcessFn::Run => 2,
                    ProcessFn::ReadText => 3,
                    ProcessFn::ReadLines => 4,
                    ProcessFn::Check | ProcessFn::Collect(true) => 1,
                    _ => 0,
                };
                values.push(self.e.fb.ins().iconst(types::I8, check));
                // Where the runtime writes each `ExitStatus` of the array.
                let TyKind::Result(_, error) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
                let result = self.e.cg.types().variants(error).unwrap()[3][1];
                let statuses = self.e.cg.types().components(result).unwrap()[3];
                let TyKind::Array(status) = *self.e.cg.types().kind(statuses) else { unreachable!("an array") };
                let size = self.e.fb.ins().iconst(types::I64, self.e.cg.layout(status).size as i64);
                let success = self.e.cg.slot(status, Proj::Field(1)).offset;
                let success = self.e.fb.ins().iconst(types::I64, success as i64);
                values.extend([size, success]);
            }
            ProcessFn::Start | ProcessFn::Exec => {}
            ProcessFn::Exit | ProcessFn::Detach | ProcessFn::Signal => unreachable!("they return nothing"),
        }
        self.process_result(rt, values, place);
        None
    }

    /// Calls a runtime function of `std.process` that writes up to six words
    /// at `out` and returns 0 or a `ProcessError` status, and builds the
    /// result at `place`: an `ExitStatus`, a `ProcessOutput` or a `Process`,
    /// or the error, with the status and output of `Failed`.
    pub(super) fn process_result(&mut self, rt: Rt, mut values: Vec<ir::Value>, place: &Place) {
        const FAILED: i64 = 4;
        let ty = self.place_ty(place);
        let TyKind::Result(success, error) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
        let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3));
        let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
        values.push(out);
        let status = self.e.call_rt(rt, &values).unwrap();
        let words: Vec<ir::Value> = (0..6).map(|i| self.e.fb.ins().load(types::I64, flags(), out, i * 8)).collect();
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.e.fb.ins().store(flags(), failed, base, 0);
        let (good, bad, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, bad, &[], good, &[]);
        self.e.fb.switch_to_block(bad);
        let value = self.scratch(error);
        let variant = self.e.fb.ins().iadd_imm_s(status, -1);
        let variant = self.e.fb.ins().ireduce(types::I8, variant);
        self.e.fb.ins().store(flags(), variant, value, 0);
        let output_ty = self.e.cg.types().variants(error).unwrap()[FAILED as usize - 1][1];
        // `check` fails with the output, and so do `run`, `text` and `lines`.
        let checked = matches!(rt, Rt::Process(ProcessFn::Run | ProcessFn::ReadText | ProcessFn::ReadLines));
        if success == output_ty || checked {
            let (failure, other) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_failure = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, FAILED);
            self.e.fb.ins().brif(is_failure, failure, &[], other, &[]);
            self.e.fb.switch_to_block(failure);
            let status_ty = self.e.cg.types().variants(error).unwrap()[FAILED as usize - 1][0];
            let exit = self.record_from_words(status_ty, &words[4..6]);
            let output = self.record_from_words(output_ty, &words[0..4]);
            for (index, component) in [exit, output].into_iter().enumerate() {
                let slot = self.e.cg.slot(error, Proj::Variant(FAILED as u32 - 1, index as u32));
                self.e.init_component(value, &slot, component);
            }
            self.e.fb.ins().jump(other, &[]);
            self.e.fb.switch_to_block(other);
        }
        let slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        self.e.init_component(base, &slot, Val::Memory(value));
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(good);
        // `exec` comes back only when it fails.
        if success == Ty::NEVER {
            self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            self.e.fb.switch_to_block(done);
            return;
        }
        if success != Ty::UNIT {
            let ok = match self.e.cg.types().components(success).map(|c| c.len()) {
                Some(count) => self.record_from_words(success, &words[0..count]),
                None => Val::Scalar(words[0]),
            };
            let slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
            self.e.init_component(base, &slot, ok);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// A record whose fields are scalars, from one word each.
    pub(super) fn record_from_words(&mut self, ty: Ty, words: &[ir::Value]) -> Val {
        let address = self.scratch(ty);
        for (index, &word) in words.iter().enumerate() {
            let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
            let field = match self.e.cg.class(slot.ty) {
                Class::Scalar(t) if t != types::I64 => self.e.fb.ins().ireduce(t, word),
                _ => word,
            };
            self.e.init_component(address, &slot, Val::Scalar(field));
        }
        Val::Memory(address)
    }

    /// An operand as one word: a scalar, or the address of a value in memory.
    pub(super) fn scalar_or_address(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) | Val::Memory(value) => value,
            Val::Zero => self.e.fb.ins().iconst(types::I64, 0),
        }
    }

    /// The value a successful status call wrote at `out`.
    pub(super) fn fs_value(&mut self, ty: Ty, out: ir::Value) -> Val {
        let word = |this: &mut Self, index: i32| this.e.fb.ins().load(types::I64, flags(), out, index * 8);
        match *self.e.cg.types().kind(ty) {
            // `String?` or `Array[Byte]?`: null for `none`.
            TyKind::Option(element) => {
                let pointer = word(self, 0);
                let address = self.scratch(ty);
                let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, pointer, 0);
                self.e.fb.ins().store(flags(), found, address, 0);
                let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                let _ = element;
                self.e.fb.ins().store(flags(), pointer, address, payload.offset as i32);
                Val::Memory(address)
            }
            TyKind::Bool => {
                let word = word(self, 0);
                Val::Scalar(self.e.fb.ins().ireduce(types::I8, word))
            }
            _ => Val::Scalar(word(self, 0)),
        }
    }

    /// A conversion that cannot fail (18.6).
    pub(super) fn convert_total(&mut self, value: ir::Value, from: Ty, to: Ty) -> ir::Value {
        let types = self.e.cg.types();
        let to_clif = self.e.cg.scalar_type(to);
        let from_clif = self.e.cg.scalar_type(from);
        match (types.kind(from), types.kind(to)) {
            (TyKind::Float(_), TyKind::Float(_)) if from_clif == to_clif => value,
            (TyKind::Float(_), TyKind::Float(_)) => self.e.fb.ins().fpromote(to_clif, value),
            (TyKind::Int(int), TyKind::Float(_)) if int.bits() == 128 => self.int128_to_float(value, *int, to_clif),
            (_, TyKind::Float(_)) if types.as_int(from).is_some_and(IntTy::signed) => {
                self.e.fb.ins().fcvt_from_sint(to_clif, value)
            }
            (_, TyKind::Float(_)) => self.e.fb.ins().fcvt_from_uint(to_clif, value),
            _ => self.resize(value, from, to_clif),
        }
    }

    /// An integer of 128 bits as a float of type `to`, by the runtime.
    pub(super) fn int128_to_float(&mut self, value: ir::Value, int: IntTy, to: ir::Type) -> ir::Value {
        let (low, high) = self.e.fb.ins().isplit(value);
        let signed = self.e.fb.ins().iconst(types::I8, int.signed() as i64);
        let single = self.e.fb.ins().iconst(types::I8, (to == types::F32) as i64);
        let float = self.e.call_rt(Rt::Int128ToFloat, &[low, high, signed, single]).unwrap();
        if to == types::F32 { self.e.fb.ins().fdemote(types::F32, float) } else { float }
    }

    /// Calls the runtime with two integers of 128 bits in memory after
    /// `first`, and gives the integer of 128 bits it writes and what it
    /// returns.
    pub(super) fn int128_call(
        &mut self,
        rt: Rt,
        first: &[ir::Value],
        a: ir::Value,
        b: ir::Value,
    ) -> (ir::Value, Option<ir::Value>) {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_store(POINTER, a, slot, 0);
        self.e.fb.ins().stack_store(POINTER, b, slot, 16);
        let values = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let out = self.e.fb.ins().stack_addr(POINTER, slot, 32);
        let mut args = first.to_vec();
        args.extend([values, out]);
        let returned = self.e.call_rt(rt, &args);
        (self.e.fb.ins().stack_load(POINTER, types::I128, slot, 32), returned)
    }

    /// Resizes an integer bit pattern, extending by the source's signedness.
    pub(super) fn resize(&mut self, value: ir::Value, from: Ty, to: ir::Type) -> ir::Value {
        let from_clif = self.e.cg.scalar_type(from);
        let signed = self.e.cg.types().as_int(from).is_some_and(IntTy::signed);
        if from_clif.bits() == to.bits() {
            value
        } else if from_clif.bits() > to.bits() {
            self.e.fb.ins().ireduce(to, value)
        } else if signed {
            self.e.fb.ins().sextend(to, value)
        } else {
            self.e.fb.ins().uextend(to, value)
        }
    }

    /// A conversion that can fail: the converted value (meaningful only on
    /// success) and the error variant, or -1 for success.
    pub(super) fn convert_checked(&mut self, value: ir::Value, from: Ty, to: Ty) -> (ir::Value, ir::Value) {
        const OUT_OF_RANGE: i64 = 0;
        const NOT_FINITE: i64 = 1;
        const NOT_INTEGRAL: i64 = 2;
        let types = self.e.cg.types();
        let to_clif = self.e.cg.scalar_type(to);
        let int = |ty: Ty| match types.kind(ty) {
            TyKind::Int(int) => Some(*int),
            _ => None,
        };
        // A `Byte` has the range of a `UInt8`.
        let range = |ty: Ty| int(ty).map_or((0, 255), |int| (int.min(), int.max()));
        let ok = self.e.fb.ins().iconst(types::I8, -1);
        let pick = |e: &mut Emit<_>, failed: ir::Value, code: i64, current: ir::Value| {
            let code = e.fb.ins().iconst(types::I8, code);
            e.fb.ins().select(failed, code, current)
        };
        match types.kind(from) {
            TyKind::Float(_) if matches!(types.kind(to), TyKind::Float(_)) => {
                // Float64 to Float32: a finite value that rounds to infinity fails.
                let narrowed = self.e.fb.ins().fdemote(to_clif, value);
                let infinite_before = self.is_infinite(value, types::F64);
                let infinite_after = self.is_infinite(narrowed, types::F32);
                let finite_before = self.e.fb.ins().bxor_imm_u(infinite_before, 1);
                let overflowed = self.e.fb.ins().band(finite_before, infinite_after);
                let error = pick(&mut self.e, overflowed, OUT_OF_RANGE, ok);
                (narrowed, error)
            }
            TyKind::Int(source) if matches!(types.kind(to), TyKind::Float(_)) => {
                // The highest `UInt128`s round to infinity in a `Float32`.
                let source = *source;
                let converted = self.int128_to_float(value, source, to_clif);
                let infinite = self.is_infinite(converted, to_clif);
                (converted, pick(&mut self.e, infinite, OUT_OF_RANGE, ok))
            }
            TyKind::Float(_) => {
                // Classified in order: NotFinite, NotIntegral, OutOfRange (18.6).
                let from_clif = self.e.cg.scalar_type(from);
                let (min, _) = range(to);
                let signed = min < 0;
                let not_nan = self.e.fb.ins().fcmp(FloatCC::Equal, value, value);
                let infinite = self.is_infinite(value, from_clif);
                let finite = self.e.fb.ins().bxor_imm_u(infinite, 1);
                let finite = self.e.fb.ins().band(finite, not_nan);
                let truncated = self.e.fb.ins().trunc(value);
                let integral = self.e.fb.ins().fcmp(FloatCC::Equal, truncated, value);
                // `min` and `max + 1` are powers of two, exact in both
                // formats, or beyond a `Float32` as an infinity.
                let bits = to_clif.bits() as i32 - signed as i32;
                let low = self.float_const(from_clif, min as f64);
                let high = self.float_const(from_clif, 2f64.powi(bits));
                let below = self.e.fb.ins().fcmp(FloatCC::LessThan, value, low);
                let above = self.e.fb.ins().fcmp(FloatCC::GreaterThanOrEqual, value, high);
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                let not_integral = self.e.fb.ins().bxor_imm_u(integral, 1);
                let error = pick(&mut self.e, not_integral, NOT_INTEGRAL, error);
                let not_finite = self.e.fb.ins().bxor_imm_u(finite, 1);
                let error = pick(&mut self.e, not_finite, NOT_FINITE, error);
                // Convert a safe placeholder when the value is invalid.
                let invalid = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
                let zero = self.float_const(from_clif, 0.0);
                let safe = self.e.fb.ins().select(invalid, zero, value);
                if to_clif == types::I128 {
                    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                    let slot = self.e.fb.create_sized_stack_slot(data);
                    let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                    let safe = if from_clif == types::F32 { self.e.fb.ins().fpromote(types::F64, safe) } else { safe };
                    let signed = self.e.fb.ins().iconst(types::I8, signed as i64);
                    self.e.call_rt(Rt::FloatToInt128, &[safe, signed, out]);
                    return (self.e.fb.ins().stack_load(POINTER, types::I128, slot, 0), error);
                }
                // x86 converts to 32 or 64 bits only; the range is already checked.
                let wide = if to_clif.bits() < 32 { types::I32 } else { to_clif };
                let converted = if signed {
                    self.e.fb.ins().fcvt_to_sint(wide, safe)
                } else {
                    self.e.fb.ins().fcvt_to_uint(wide, safe)
                };
                let converted = if wide == to_clif { converted } else { self.e.fb.ins().ireduce(to_clif, converted) };
                (converted, error)
            }
            _ => {
                let (min, max) = range(to);
                let (from_min, from_max) = range(from);
                let signed = from_min < 0;
                // Compare in the source's domain, widened to 64 bits, or to
                // 128 when either side has them.
                let domain = if self.e.cg.scalar_type(from).bits() == 128 || to_clif.bits() == 128 {
                    types::I128
                } else {
                    types::I64
                };
                let wide = self.resize(value, from, domain);
                let below = if min > from_min {
                    let min = self.e.int_const(domain, min);
                    self.e.fb.ins().icmp(IntCC::SignedLessThan, wide, min)
                } else {
                    self.e.fb.ins().iconst(types::I8, 0)
                };
                // `max` is below the source's, so it fits in its domain.
                let above = if max < from_max {
                    let bound = self.e.int_const(domain, max as i128);
                    let cc = if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                    self.e.fb.ins().icmp(cc, wide, bound)
                } else {
                    self.e.fb.ins().iconst(types::I8, 0)
                };
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                (self.resize(value, from, to_clif), error)
            }
        }
    }

    pub(super) fn float_const(&mut self, ty: ir::Type, value: f64) -> ir::Value {
        if ty == types::F32 { self.e.fb.ins().f32const(value as f32) } else { self.e.fb.ins().f64const(value) }
    }

    pub(super) fn is_infinite(&mut self, value: ir::Value, ty: ir::Type) -> ir::Value {
        let magnitude = self.e.fb.ins().fabs(value);
        let infinity = self.float_const(ty, f64::INFINITY);
        self.e.fb.ins().fcmp(FloatCC::Equal, magnitude, infinity)
    }

    /// Writes `some(value)` into `place` when `present`, or `none`.
    pub(super) fn write_option(&mut self, place: &Place, ty: Ty, present: ir::Value, value: ir::Value) {
        let base = self.pointer_to(place, true);
        self.e.fb.ins().store(flags(), present, base, 0);
        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
        self.e.fb.ins().store(flags(), value, base, payload.offset as i32);
    }

    /// An integer as one of `ty`, from 128 bits or to them.
    pub(super) fn fit_width(&mut self, value: ir::Value, ty: ir::Type) -> ir::Value {
        match (self.e.fb.func.dfg.value_type(value).bits(), ty.bits()) {
            (from, to) if from == to => value,
            (from, to) if from > to => self.e.fb.ins().ireduce(ty, value),
            _ => self.e.fb.ins().uextend(ty, value),
        }
    }

    /// The zeros above the highest 1 of an integer of `bits` bits, as an
    /// `Int`; Cranelift counts 32 bits and more.
    pub(super) fn leading_zeros(&mut self, value: ir::Value, bits: u32) -> ir::Value {
        if bits >= 32 {
            let zeros = self.e.fb.ins().clz(value);
            return self.fit_width(zeros, types::I64);
        }
        let wide = self.e.fb.ins().uextend(types::I32, value);
        let zeros = self.e.fb.ins().clz(wide);
        let zeros = self.e.fb.ins().iadd_imm_s(zeros, -(32 - i64::from(bits)));
        self.e.fb.ins().uextend(types::I64, zeros)
    }

    /// A named operation of a number (18.9) for `place`, of type `ty`, or
    /// `None` when it wrote the place itself.
    pub(super) fn number(&mut self, op: NumOp, args: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        use NumOp::*;
        let types = self.e.cg.types();
        // The number the operation is of: its receiver's, or what it makes.
        let own = match op {
            Parse | FromBytes | Wrapping | Saturating | FromBits | FromInt | Least | Greatest => {
                match *types.kind(types.repr(ty)) {
                    TyKind::Result(ok, _) => ok,
                    _ => ty,
                }
            }
            _ => self.operand_ty(&args[0]),
        };
        let own = types.repr(own);
        let clif = self.e.cg.scalar_type(own);
        // A `Byte` has the bits of a `UInt8`.
        let int = match *types.kind(own) {
            TyKind::Int(int) => Some(int),
            TyKind::Byte => Some(IntTy::U8),
            _ => None,
        };
        let bits = clif.bits();
        let signed = int.is_some_and(IntTy::signed);
        let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
        let less = |this: &mut Self, a: ir::Value, b: ir::Value| match clif.is_float() {
            true => this.e.fb.ins().fcmp(FloatCC::LessThan, a, b),
            false => this.e.fb.ins().icmp(if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan }, a, b),
        };
        let wide128 = clif == types::I128;
        // The detail of a panic of the operation: its code, signedness and width.
        let detail = |code: u32| code << 16 | 1 << 9 | (signed as u32) << 8 | bits;
        let value = match op {
            Compare => {
                let (a, b) = (values[0], values[1]);
                let (lt, gt) = (less(self, a, b), less(self, b, a));
                let (lt, gt) = (self.e.fb.ins().uextend(types::I64, lt), self.e.fb.ins().uextend(types::I64, gt));
                let order = self.e.fb.ins().isub(gt, lt);
                if !clif.is_float() {
                    return Some(Val::Scalar(order));
                }
                // NaN after every other value, and equal to itself.
                let a_nan = self.e.fb.ins().fcmp(FloatCC::Unordered, a, a);
                let b_nan = self.e.fb.ins().fcmp(FloatCC::Unordered, b, b);
                let (a_nan, b_nan) =
                    (self.e.fb.ins().uextend(types::I64, a_nan), self.e.fb.ins().uextend(types::I64, b_nan));
                let order = self.e.fb.ins().iadd(order, a_nan);
                self.e.fb.ins().isub(order, b_nan)
            }
            Min => {
                let smaller = less(self, values[1], values[0]);
                self.e.fb.ins().select(smaller, values[1], values[0])
            }
            Max => {
                let larger = less(self, values[0], values[1]);
                self.e.fb.ins().select(larger, values[1], values[0])
            }
            Clamp => {
                let (a, low, high) = (values[0], values[1], values[2]);
                let below = less(self, a, low);
                let raised = self.e.fb.ins().select(below, low, a);
                let above = less(self, high, raised);
                self.e.fb.ins().select(above, high, raised)
            }
            Pow | WrappingPow | CheckedPow | SaturatingPow if int.is_some() => {
                let base = self.widest(values[0], signed);
                let exponent = self.e.fb.ins().sextend(types::I128, values[1]);
                let saturate = self.e.fb.ins().iconst(types::I8, (op == SaturatingPow) as i64);
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let width = self.e.fb.ins().iconst(types::I32, i64::from(bits));
                let (result, status) = self.int128_call(Rt::IntPow, &[saturate, signed_flag, width], base, exponent);
                let status = status.unwrap();
                let negative = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 2);
                self.panic_values_if(negative, 18, span, |_| (detail(8), values[0], values[1], own));
                let result = self.fit_width(result, clif);
                match op {
                    Pow => {
                        let overflow = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 1);
                        self.panic_values_if(overflow, 5, span, |_| (detail(8), values[0], values[1], own));
                        result
                    }
                    CheckedPow => {
                        let fits = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        self.write_option(place, ty, fits, result);
                        return None;
                    }
                    _ => result,
                }
            }
            Pow => {
                let float = |this: &mut Self, v: ir::Value| match clif == types::F32 {
                    true => this.e.fb.ins().fpromote(types::F64, v),
                    false => v,
                };
                let (a, b) = (float(self, values[0]), float(self, values[1]));
                let power = self.e.call_rt(Rt::MathPow, &[a, b]).unwrap();
                if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, power) } else { power }
            }
            WrappingPow | CheckedPow | SaturatingPow => unreachable!("of integers only"),
            Abs if clif.is_float() => self.e.fb.ins().fabs(values[0]),
            Abs => {
                let min = self.e.int_const(clif, int.unwrap().min());
                let least = self.e.fb.ins().icmp(IntCC::Equal, values[0], min);
                self.panic_values_if(least, 5, span, |_| (5 << 16 | 0x100 | bits, values[0], values[0], own));
                self.e.fb.ins().iabs(values[0])
            }
            WrappingAdd => self.e.fb.ins().iadd(values[0], values[1]),
            WrappingSub => self.e.fb.ins().isub(values[0], values[1]),
            WrappingMul => self.e.fb.ins().imul(values[0], values[1]),
            WrappingNeg => self.e.fb.ins().ineg(values[0]),
            CheckedAdd | CheckedSub | CheckedMul | SaturatingAdd | SaturatingSub | SaturatingMul | OverflowingAdd
            | OverflowingSub | OverflowingMul => {
                let (a, b) = (values[0], values[1]);
                let (result, overflow) = match (op, signed) {
                    (CheckedMul | SaturatingMul | OverflowingMul, _) if wide128 => {
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        let (product, overflow) = self.int128_call(Rt::Int128Multiply, &[signed_flag], a, b);
                        (product, overflow.unwrap())
                    }
                    (CheckedAdd | SaturatingAdd | OverflowingAdd, true) => self.e.fb.ins().sadd_overflow(a, b),
                    (CheckedAdd | SaturatingAdd | OverflowingAdd, false) => self.e.fb.ins().uadd_overflow(a, b),
                    (CheckedSub | SaturatingSub | OverflowingSub, true) => self.e.fb.ins().ssub_overflow(a, b),
                    (CheckedSub | SaturatingSub | OverflowingSub, false) => self.e.fb.ins().usub_overflow(a, b),
                    (_, true) => self.e.fb.ins().smul_overflow(a, b),
                    (_, false) => self.e.fb.ins().umul_overflow(a, b),
                };
                match op {
                    CheckedAdd | CheckedSub | CheckedMul => {
                        let fits = self.e.fb.ins().bxor_imm_u(overflow, 1);
                        self.write_option(place, ty, fits, result);
                        return None;
                    }
                    OverflowingAdd | OverflowingSub | OverflowingMul => {
                        let base = self.pointer_to(place, true);
                        let (first, second) = (self.e.cg.slot(ty, Proj::Field(0)), self.e.cg.slot(ty, Proj::Field(1)));
                        self.e.fb.ins().store(flags(), result, base, first.offset as i32);
                        self.e.fb.ins().store(flags(), overflow, base, second.offset as i32);
                        return None;
                    }
                    _ => {}
                }
                // The bound the exact result went past.
                let int = int.unwrap();
                let (min, max) = (self.e.int_const(clif, int.min()), self.e.int_const(clif, int.max() as i128));
                let bound = match (op, signed) {
                    (SaturatingSub, false) => min,
                    (_, false) => max,
                    (SaturatingMul, true) => {
                        let sign = self.e.fb.ins().bxor(a, b);
                        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, sign, 0);
                        self.e.fb.ins().select(negative, min, max)
                    }
                    _ => {
                        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                        let (low, high) = if op == SaturatingAdd { (min, max) } else { (max, min) };
                        self.e.fb.ins().select(negative, low, high)
                    }
                };
                self.e.fb.ins().select(overflow, bound, result)
            }
            CheckedDiv | CheckedRem => {
                let (a, b) = (values[0], values[1]);
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                let failed = match signed {
                    true => {
                        let int = int.unwrap();
                        let (min, minus_one) = (self.e.int_const(clif, int.min()), self.e.int_const(clif, -1));
                        let is_min = self.e.fb.ins().icmp(IntCC::Equal, a, min);
                        let is_minus_one = self.e.fb.ins().icmp(IntCC::Equal, b, minus_one);
                        let both = self.e.fb.ins().band(is_min, is_minus_one);
                        self.e.fb.ins().bor(zero, both)
                    }
                    false => zero,
                };
                // A divisor that cannot fail; `MIN % -1` is 0, as `MIN % 1`.
                let one = self.e.int_const(clif, 1);
                let divisor = self.e.fb.ins().select(failed, one, b);
                let result = match (wide128, op, signed) {
                    (true, _, _) => {
                        let remainder = self.e.fb.ins().iconst(types::I8, (op == CheckedRem) as i64);
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        self.int128_call(Rt::Int128Divide, &[remainder, signed_flag], a, divisor).0
                    }
                    (_, CheckedDiv, true) => self.e.fb.ins().sdiv(a, divisor),
                    (_, CheckedDiv, false) => self.e.fb.ins().udiv(a, divisor),
                    (_, _, true) => self.e.fb.ins().srem(a, divisor),
                    (_, _, false) => self.e.fb.ins().urem(a, divisor),
                };
                let present = if op == CheckedRem {
                    self.e.fb.ins().bxor_imm_u(zero, 1)
                } else {
                    self.e.fb.ins().bxor_imm_u(failed, 1)
                };
                self.write_option(place, ty, present, result);
                return None;
            }
            CheckedNeg => {
                let zero = self.e.int_const(clif, 0);
                let (result, present) = match signed {
                    true => {
                        let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, values[0]);
                        (result, self.e.fb.ins().bxor_imm_u(overflow, 1))
                    }
                    false => (self.e.fb.ins().ineg(values[0]), self.e.fb.ins().icmp(IntCC::Equal, values[0], zero)),
                };
                self.write_option(place, ty, present, result);
                return None;
            }
            CountOnes => {
                let count = self.e.fb.ins().popcnt(values[0]);
                self.fit_width(count, types::I64)
            }
            LeadingZeros => self.leading_zeros(values[0], bits),
            TrailingZeros if bits < 32 => {
                // A 1 just above the value stops the count at its width.
                let wide = self.e.fb.ins().uextend(types::I32, values[0]);
                let wide = self.e.fb.ins().bor_imm_u(wide, 1 << bits);
                let zeros = self.e.fb.ins().ctz(wide);
                self.e.fb.ins().uextend(types::I64, zeros)
            }
            TrailingZeros => {
                let zeros = self.e.fb.ins().ctz(values[0]);
                self.fit_width(zeros, types::I64)
            }
            RotateLeft | RotateRight => {
                let count = self.e.fb.ins().band_imm_s(values[1], i64::from(bits) - 1);
                match op {
                    RotateLeft => self.e.fb.ins().rotl(values[0], count),
                    _ => self.e.fb.ins().rotr(values[0], count),
                }
            }
            ReverseBytes if bits == 8 => values[0],
            ReverseBytes => self.e.fb.ins().bswap(values[0]),
            ReverseBits => self.e.fb.ins().bitrev(values[0]),
            IsPowerOfTwo => {
                let ones = self.e.fb.ins().popcnt(values[0]);
                self.e.fb.ins().icmp_imm_s(IntCC::Equal, ones, 1)
            }
            NextPowerOfTwo => {
                // 1 for 0 and 1; otherwise the bit above the highest of `x - 1`.
                let at_most_one = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, values[0], 1);
                let two = self.e.int_const(clif, 2);
                let safe = self.e.fb.ins().select(at_most_one, two, values[0]);
                let below = self.e.fb.ins().iadd_imm_s(safe, -1);
                let zeros = self.leading_zeros(below, bits);
                let overflow = self.e.fb.ins().icmp_imm_s(IntCC::Equal, zeros, 0);
                self.panic_values_if(overflow, 5, span, |_| (detail(9), values[0], values[0], own));
                let width = self.e.fb.ins().iconst(types::I64, i64::from(bits));
                let shift = self.e.fb.ins().isub(width, zeros);
                let one = self.e.int_const(clif, 1);
                let power = self.e.fb.ins().ishl(one, shift);
                self.e.fb.ins().select(at_most_one, one, power)
            }
            ToBytes => {
                let value = self.widest(values[0], signed);
                let (low, high) = self.e.fb.ins().isplit(value);
                let count = self.e.fb.ins().iconst(types::I64, i64::from(bits / 8));
                let big = self.e.fb.ins().icmp_imm_s(IntCC::Equal, values[1], 0);
                self.e.call_rt(Rt::IntToBytes, &[low, high, count, big]).unwrap()
            }
            FromBytes => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let slot = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let count = self.e.fb.ins().iconst(types::I64, i64::from(bits / 8));
                let big = self.e.fb.ins().icmp_imm_s(IntCC::Equal, values[2], 0);
                let location = self.location(span);
                self.e.call_rt(Rt::IntFromBytes, &[values[0], values[1], count, big, location, slot]);
                let read = self.e.fb.ins().load(types::I128, flags(), slot, 0);
                self.fit_width(read, clif)
            }
            Wrapping => {
                let from = self.operand_ty(&args[0]);
                self.resize(values[0], from, clif)
            }
            Saturating => {
                let from = self.e.cg.types().repr(self.operand_ty(&args[0]));
                let int = int.unwrap();
                let source = self.e.cg.scalar_type(from);
                if source.is_float() {
                    let value =
                        if source == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                    if wide128 {
                        // Rust's conversion saturates, and gives 0 for NaN.
                        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                        let slot = self.e.fb.create_sized_stack_slot(data);
                        let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        self.e.call_rt(Rt::FloatToInt128, &[value, signed_flag, out]);
                        return Some(Val::Scalar(self.e.fb.ins().load(types::I128, flags(), out, 0)));
                    }
                    let wide = if bits < 32 { types::I64 } else { clif };
                    let converted = match signed {
                        true => self.e.fb.ins().fcvt_to_sint_sat(wide, value),
                        false => self.e.fb.ins().fcvt_to_uint_sat(wide, value),
                    };
                    if wide == clif {
                        return Some(Val::Scalar(converted));
                    }
                    let (min, max) = (
                        self.e.fb.ins().iconst(wide, int.min() as i64),
                        self.e.fb.ins().iconst(wide, int.max() as i64),
                    );
                    let below = self.e.fb.ins().icmp(IntCC::SignedLessThan, converted, min);
                    let raised = self.e.fb.ins().select(below, min, converted);
                    let above = self.e.fb.ins().icmp(IntCC::SignedGreaterThan, raised, max);
                    let clamped = self.e.fb.ins().select(above, max, raised);
                    return Some(Val::Scalar(self.e.fb.ins().ireduce(clif, clamped)));
                }
                // Compare in the source's domain, as a checked conversion does.
                let from_int = match *self.e.cg.types().kind(from) {
                    TyKind::Int(from) => from,
                    _ => IntTy::U8,
                };
                let domain = if source.bits() == 128 || bits == 128 { types::I128 } else { types::I64 };
                let wide = self.resize(values[0], from, domain);
                let mut result = self.resize(values[0], from, clif);
                if int.min() > from_int.min() {
                    let bound = self.e.int_const(domain, int.min());
                    let below = self.e.fb.ins().icmp(IntCC::SignedLessThan, wide, bound);
                    let min = self.e.int_const(clif, int.min());
                    result = self.e.fb.ins().select(below, min, result);
                }
                if int.max() < from_int.max() {
                    let bound = self.e.int_const(domain, int.max() as i128);
                    let cc = if from_int.signed() { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                    let above = self.e.fb.ins().icmp(cc, wide, bound);
                    let max = self.e.int_const(clif, int.max() as i128);
                    result = self.e.fb.ins().select(above, max, result);
                }
                result
            }
            Parse if clif.is_float() => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let single = self.e.fb.ins().iconst(types::I8, (clif == types::F32) as i64);
                let ok = self.e.call_rt(Rt::FloatParse, &[values[0], single, out]).unwrap();
                let read = self.e.fb.ins().load(types::F64, flags(), out, 0);
                let read = if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, read) } else { read };
                let (fine, invalid) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 3));
                let error = self.e.fb.ins().select(ok, fine, invalid);
                self.write_checked(place, ty, read, error);
                return None;
            }
            Parse => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let width = self.e.fb.ins().iconst(types::I32, i64::from(bits));
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let ok = self.e.call_rt(Rt::IntParse, &[values[0], values[1], width, signed_flag, out]).unwrap();
                let read = self.e.fb.ins().load(types::I128, flags(), out, 0);
                let read = self.fit_width(read, clif);
                // `TextError.InvalidData`, the fourth variant.
                let (fine, invalid) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 3));
                let error = self.e.fb.ins().select(ok, fine, invalid);
                self.write_checked(place, ty, read, error);
                return None;
            }
            ToString => {
                let value = self.widest(values[0], signed);
                let (low, high) = self.e.fb.ins().isplit(value);
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                self.e.call_rt(Rt::IntRadix, &[low, high, signed_flag, values[1]]).unwrap()
            }
            Floor => self.e.fb.ins().floor(values[0]),
            Ceil => self.e.fb.ins().ceil(values[0]),
            Round => self.e.fb.ins().nearest(values[0]),
            Truncate => self.e.fb.ins().trunc(values[0]),
            RoundTiesAway => {
                let value =
                    if clif == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                let rounded = self.e.call_rt(Rt::MathRoundAway, &[value]).unwrap();
                if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, rounded) } else { rounded }
            }
            Sqrt => self.e.fb.ins().sqrt(values[0]),
            CheckedSqrt => {
                let zero = self.float_const(clif, 0.0);
                let negative = self.e.fb.ins().fcmp(FloatCC::LessThan, values[0], zero);
                let root = self.e.fb.ins().sqrt(values[0]);
                // `MathError.Domain`, its only variant.
                let (fine, domain) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 0));
                let error = self.e.fb.ins().select(negative, domain, fine);
                self.write_checked(place, ty, root, error);
                return None;
            }
            IsNaN => self.e.fb.ins().fcmp(FloatCC::Unordered, values[0], values[0]),
            IsInfinite => self.is_infinite(values[0], clif),
            IsFinite => {
                let magnitude = self.e.fb.ins().fabs(values[0]);
                let infinity = self.float_const(clif, f64::INFINITY);
                self.e.fb.ins().fcmp(FloatCC::LessThan, magnitude, infinity)
            }
            ToFixed => {
                let value =
                    if clif == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                self.e.call_rt(Rt::TextFixed, &[value, values[1]]).unwrap()
            }
            Bits => {
                let pattern = if clif == types::F32 { types::I32 } else { types::I64 };
                self.e.fb.ins().bitcast(pattern, MemFlagsData::new(), values[0])
            }
            FromBits => self.e.fb.ins().bitcast(clif, MemFlagsData::new(), values[0]),
            FromInt => {
                let (converted, error) = self.convert_checked(values[0], Ty::INT, own);
                let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
                let detail = 10 << 16 | 0x100 | bits;
                self.panic_values_if(failed, 5, span, |_| (detail, values[0], values[0], own));
                converted
            }
            Least => self.e.int_const(clif, int.unwrap().min()),
            Greatest => self.e.int_const(clif, int.unwrap().max() as i128),
            Width => self.e.fb.ins().iconst(types::I64, i64::from(bits)),
        };
        Some(Val::Scalar(value))
    }

    /// Writes `ok(value)` or `err(NumericConversionError)` into `place`.
    pub(super) fn write_checked(&mut self, place: &Place, ty: Ty, value: ir::Value, error: ir::Value) {
        let base = self.pointer_to(place, true);
        self.write_checked_at(base, ty, value, error);
    }

    /// `write_checked` at the address of the place.
    pub(super) fn write_checked_at(&mut self, base: ir::Value, ty: Ty, value: ir::Value, error: ir::Value) {
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
        self.e.fb.ins().store(flags(), failed, base, 0);
        // Both payloads share the same storage: write only the active one.
        let (failure, success, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, failure, &[], success, &[]);
        self.e.fb.switch_to_block(failure);
        let err_slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        self.e.fb.ins().store(flags(), error, base, err_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(success);
        let ok_slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
        self.e.fb.ins().store(flags(), value, base, ok_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }
}
