//! Statements and assignments.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    pub(super) fn statement(&mut self, stmt: &Statement) {
        // A probe's answer is for the statement right after it.
        self.probed = None;
        match stmt {
            Statement::OsrLoad(point) => self.osr_load(*point),
            Statement::Failing(error, span, called) => self.failing(error, *span, *called),
            Statement::Assign(place, rvalue, span) => {
                // What it allocates: the collection a method changes, or the
                // value it makes.
                let made = match &**rvalue {
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => target,
                    _ => place,
                };
                self.at_typed(*span, Some(self.body.place_ty(self.e.cg.types(), made)));
                self.probe(*span);
                self.assign(place, rvalue, *span);
                if place.path.is_empty() {
                    self.mark(place.local, true);
                }
            }
            Statement::DeferPush(body, place) => {
                let code = self.e.cg.closure_code(*body);
                let code = self.e.cg.declare_func_in_func(code, self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, code);
                let mut spilled = Vec::new();
                let closure = self.loan_address(place, &mut spilled);
                self.e.call_rt(Rt::DeferPush, &[code, closure]);
            }
            Statement::DeferPop => {
                self.e.call_rt(Rt::DeferPop, &[]);
            }
            Statement::Clear(local) => {
                self.mark(*local, false);
                self.zero(&Place::local(*local), false);
            }
            Statement::Moved(place) => self.zero(place, true),
            Statement::Lend(local, place) => {
                if let Storage::Pointer(var) = self.storage[local.0 as usize] {
                    let address = self.pointer_to(place, true);
                    self.define(var, address);
                }
            }
            Statement::LendAt(local, guard) => {
                if let Storage::Pointer(var) = self.storage[local.0 as usize] {
                    // The cell's value, read where the cell keeps it; none
                    // for an empty guard, as a `tryUpdate` that found the
                    // lock taken has.
                    let guard = self.scalar(guard);
                    let (read, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                    self.e.fb.append_block_param(done, POINTER);
                    let none = self.e.fb.ins().iconst(POINTER, 0);
                    self.e.fb.ins().brif(guard, read, &[], done, &[none.into()]);
                    self.e.fb.switch_to_block(read);
                    let cell = self.e.fb.ins().band_imm_s(guard, !1);
                    let address = self.e.fb.ins().load(POINTER, flags(), cell, SYNC_VALUE);
                    self.e.fb.ins().jump(done, &[address.into()]);
                    self.e.fb.switch_to_block(done);
                    let address = self.e.fb.block_params(done)[0];
                    self.define(var, address);
                }
            }
            Statement::Retain(place) | Statement::Release(place) => {
                if matches!(stmt, Statement::Release(_)) && place.path.is_empty() {
                    self.mark(place.local, false);
                }
                let ty = self.place_ty(place);
                // A function's value the frame made: its captures go, its
                // room is the frame's.
                if matches!(stmt, Statement::Release(_))
                    && place.path.is_empty()
                    && self.body.framed.contains(&place.local)
                    && matches!(self.e.cg.types().kind(ty), TyKind::Fn(..))
                {
                    let value = self.scalar(&Operand::Copy(place.clone()));
                    self.e.call_rt(Rt::FnLetGo, &[value]);
                    return;
                }
                if !self.e.cg.types().needs_drop(ty) || place.path.is_empty() && self.literal.contains_key(&place.local)
                {
                    return;
                }
                let value = self.read(place);
                self.e.refcount(ty, value, matches!(stmt, Statement::Release(_)));
            }
        }
    }

    /// Zeroes a place whose value moved away: releasing it releases nothing.
    pub(super) fn zero(&mut self, place: &Place, write: bool) {
        let ty = self.place_ty(place);
        match self.e.cg.class(ty) {
            Class::Zero => {}
            Class::Scalar(clif) if clif.is_float() => {
                let zero = self.e.fb.ins().f64const(0.0);
                let zero = if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, zero) } else { zero };
                self.write(place, Val::Scalar(zero));
            }
            Class::Scalar(clif) => {
                let zero = self.e.int_const(clif, 0);
                self.write(place, Val::Scalar(zero));
            }
            Class::Memory => {
                let address = self.pointer_to(place, write);
                let size = self.e.cg.layout(ty).size;
                let mut offset = 0;
                for width in [types::I64, types::I32, types::I16, types::I8] {
                    let zero = self.e.fb.ins().iconst(width, 0);
                    while offset + width.bytes() <= size {
                        self.e.fb.ins().store(flags(), zero, address, offset as i32);
                        offset += width.bytes();
                    }
                }
            }
        }
    }

    pub(super) fn assign(&mut self, place: &Place, rvalue: &Rvalue, span: Span) {
        let ty = self.place_ty(place);
        let class = self.e.cg.class(ty);
        if class == Class::Zero {
            // Zero-sized values carry no data; only calls and mutations have
            // effects.
            match rvalue {
                Rvalue::Call(Callee::Std(StdFn::Env(function)), args) => {
                    let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                    // An invalid name panics where the call is.
                    if matches!(function, EnvFn::Set | EnvFn::SetAll | EnvFn::Remove) {
                        values.push(self.location(span));
                    }
                    self.e.call_rt(Rt::Env(*function), &values);
                }
                Rvalue::Call(callee, args) => {
                    self.call(*callee, args, None);
                }
                Rvalue::CallOsr(id, args, state) => {
                    self.call_osr(*id, args, state, None);
                }
                Rvalue::CallIndirect(callee, args) => {
                    self.call_indirect(callee, args, None);
                }
                Rvalue::Pointer(PointerOp::Write, operands, retain) => {
                    // The memory takes the value, and one reference to a
                    // value that was copied.
                    let address = self.scalar(&operands[0]);
                    let value_ty = self.operand_ty(&operands[1]);
                    let value = self.operand(&operands[1]);
                    if *retain {
                        self.e.refcount(value_ty, value, false);
                    }
                    self.e.store_val(address, value_ty, value);
                }
                Rvalue::Mutate(mutation, target, operands, retain, span) => {
                    self.mutate(*mutation, target, operands, retain, place, *span);
                }
                Rvalue::TaskOp(op, operands, retain, span) => {
                    self.task_op(*op, operands, retain, place, *span);
                }
                Rvalue::AssignSlice(target, bounds, value, span) => self.assign_slice(target, bounds, value, *span),
                Rvalue::CheckRegion(array, region, other, span) => self.check_region(array, region, other, *span),
                Rvalue::TestMembers(members) => self.test_members(members),
                _ => {}
            }
            return;
        }
        let value = match rvalue {
            Rvalue::Call(Callee::Std(StdFn::Number(op)), args) => match self.number(*op, args, place, ty, span) {
                Some(value) => value,
                None => return,
            },
            Rvalue::Call(Callee::Std(StdFn::Fs(FsFn::Exists)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                Val::Scalar(self.e.call_rt(Rt::Fs(FsFn::Exists), &values).unwrap())
            }
            Rvalue::Call(Callee::Std(StdFn::Fs(function)), args) => {
                self.status_call(Rt::Fs(*function), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Io(function)), args) if function.fails() => {
                self.status_call(Rt::Io(*function), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleFlush), args) => {
                self.status_call(Rt::ConsoleFlush, args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Io(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                Val::Scalar(self.e.call_rt(Rt::Io(*function), &values).unwrap())
            }
            Rvalue::Call(Callee::Std(StdFn::Env(EnvFn::SetCurrentDirectory)), args) => {
                self.status_call(Rt::Env(EnvFn::SetCurrentDirectory), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Env(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                let rt = Rt::Env(*function);
                match function {
                    EnvFn::Get => {
                        let value = self.e.call_rt(rt, &values).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, value) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                    EnvFn::Vars => {
                        let glue = self.e.map_glue_address(ty);
                        Val::Scalar(self.e.call_rt(rt, &[glue]).unwrap())
                    }
                    _ => Val::Scalar(self.e.call_rt(rt, &values).unwrap()),
                }
            }
            Rvalue::Call(Callee::Std(StdFn::TimeNow), _) => Val::Scalar(self.e.call_rt(Rt::TimeNow, &[]).unwrap()),
            Rvalue::Call(Callee::Std(StdFn::TimeUnix), _) => Val::Scalar(self.e.call_rt(Rt::TimeUnix, &[]).unwrap()),
            Rvalue::Call(Callee::Std(function @ (StdFn::TimeUtc | StdFn::TimeUtcNow)), args) => {
                // Seven words, from the year down to the nanosecond.
                let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 56, 3));
                let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
                let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                values.push(out);
                let rt = if *function == StdFn::TimeUtc { Rt::TimeUtc } else { Rt::TimeUtcNow };
                self.e.call_rt(rt, &values);
                let address = self.scratch(ty);
                for index in 0..7i32 {
                    let word = self.e.fb.ins().load(types::I64, flags(), out, index * 8);
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    self.e.fb.ins().store(flags(), word, address, slot.offset as i32);
                }
                Val::Memory(address)
            }
            Rvalue::Call(Callee::Std(StdFn::Process(function)), args) => {
                match self.process_call(*function, args, place) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleReadLine), _) => {
                let line = self.e.call_rt(Rt::ConsoleReadLine, &[]).unwrap();
                match self.pointer_option(place, ty, Ty::STRING, line) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleStream(index)), _) => {
                let index = self.e.fb.ins().iconst(types::I64, *index as i64);
                Val::Scalar(self.e.call_rt(Rt::ConsoleStream, &[index]).unwrap())
            }

            Rvalue::Call(Callee::Std(StdFn::Path(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                match function {
                    PathFn::Join => Val::Scalar(self.e.call_rt(Rt::PathJoin, &values).unwrap()),
                    PathFn::IsAbsolute => Val::Scalar(self.e.call_rt(Rt::PathIsAbsolute, &values).unwrap()),
                    _ => {
                        let which = match function {
                            PathFn::Parent => 0,
                            PathFn::FileName => 1,
                            _ => 2,
                        };
                        let which = self.e.fb.ins().iconst(types::I8, which);
                        let part = self.e.call_rt(Rt::PathPart, &[values[0], which]).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, part) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                }
            }
            Rvalue::Call(callee, args) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call(*callee, args, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::Aggregate(components) => {
                let base = self.pointer_to(place, true);
                for (index, component) in components.iter().enumerate() {
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    let value = self.operand(component);
                    self.e.init_component(base, &slot, value);
                }
                return;
            }
            Rvalue::Variant(_, components) if matches!(self.e.cg.layout(ty).shape, Shape::Niche) => {
                match components.first() {
                    Some(address) => self.operand(address),
                    None => Val::Scalar(self.e.fb.ins().iconst(POINTER, 0)),
                }
            }
            Rvalue::Variant(variant, components) => {
                let tag_ty = self.e.cg.types().tag_ty(ty);
                let tag = self.operand(&Operand::Const(Const::Int(*variant as i128, tag_ty)));
                if let Class::Scalar(_) = class {
                    tag
                } else {
                    let base = self.pointer_to(place, true);
                    // A whole word when the payloads start past it, so that
                    // a copy of the value reads what was just stored as
                    // stored, without waiting for the store to land.
                    let payloads = self
                        .e
                        .cg
                        .types()
                        .variants(ty)
                        .expect("an enum")
                        .iter()
                        .enumerate()
                        .filter(|(_, components)| !components.is_empty())
                        .map(|(variant, _)| self.e.cg.slot(ty, Proj::Variant(variant as u32, 0)).offset)
                        .min();
                    match tag {
                        Val::Scalar(tag)
                            if payloads.is_none_or(|offset| offset >= 8) && self.e.cg.layout(ty).size >= 8 =>
                        {
                            let word = self.wide(tag, false);
                            self.e.fb.ins().store(flags(), word, base, 0);
                        }
                        _ => self.e.store_val(base, tag_ty, tag),
                    }
                    for (index, component) in components.iter().enumerate() {
                        let slot = self.e.cg.slot(ty, Proj::Variant(*variant, index as u32));
                        let value = self.operand(component);
                        self.e.init_component(base, &slot, value);
                    }
                    return;
                }
            }
            Rvalue::Convert(operand) => {
                let from = self.operand_ty(operand);
                let value = self.scalar(operand);
                match self.e.cg.types().kind(ty) {
                    TyKind::Result(target, _) => {
                        let target = *target;
                        let (converted, error) = self.convert_checked(value, from, target);
                        self.write_checked(place, ty, converted, error);
                        return;
                    }
                    _ => Val::Scalar(self.convert_total(value, from, ty)),
                }
            }
            Rvalue::Array(values, _) if let Some(elements) = self.constants(values, ty) => {
                let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                    unreachable!()
                };
                // Made once, in the program's data, rather than each time.
                let size = self.e.cg.layout(element).size as usize;
                let id = self.e.cg.static_array(size, elements);
                let global = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Rvalue::Array(values, retain) => {
                let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                    unreachable!()
                };
                let size = self.e.cg.layout(element).size as i64;
                let size_value = self.e.fb.ins().iconst(POINTER, size);
                let count = self.e.fb.ins().iconst(POINTER, values.len() as i64);
                let array = self.e.call_rt(Rt::ArrayNew, &[size_value, count]).unwrap();
                for (index, (value, retain)) in values.iter().zip(retain).enumerate() {
                    let address = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER + index as i64 * size);
                    let value = self.operand(value);
                    self.e.store_val(address, element, value);
                    if *retain {
                        let stored = self.e.value_at(address, element);
                        self.e.refcount(element, stored, false);
                    }
                }
                Val::Scalar(array)
            }
            Rvalue::ArrayOp(op, operands, span) => {
                if let Some(value) = self.array_op(*op, operands, place, ty, *span) {
                    value
                } else {
                    return;
                }
            }
            Rvalue::Slice(base, parts, span) => {
                let sequence = self.scalar(base);
                let range = self.slice_range(parts, *span);
                Val::Scalar(if ty == Ty::STRING {
                    self.e.call_rt(Rt::StrSlice, &[&[sequence], &range[..]].concat()).unwrap()
                } else {
                    let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                        unreachable!()
                    };
                    let size = self.e.element_size(element);
                    let retain = self.e.glue_or_null(Glue::Retain, element);
                    self.e.call_rt(Rt::ArraySlice, &[&[sequence, size, retain], &range[..]].concat()).unwrap()
                })
            }
            Rvalue::Pointer(PointerOp::Read, operands, _) => {
                // A copy of the value, which the memory keeps too.
                let address = self.scalar(&operands[0]);
                let value = self.e.value_at(address, ty);
                self.e.refcount(ty, value, false);
                value
            }
            Rvalue::Pointer(PointerOp::Take, operands, _) => {
                let address = self.scalar(&operands[0]);
                self.e.value_at(address, ty)
            }
            Rvalue::Pointer(PointerOp::Offset, operands, _) => {
                let TyKind::Pointer(element) = *self.e.cg.types().kind(ty) else { unreachable!("a pointer") };
                let (address, count) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let size = self.e.element_size(element);
                let bytes = self.e.fb.ins().imul(count, size);
                Val::Scalar(self.e.fb.ins().iadd(address, bytes))
            }
            Rvalue::Pointer(PointerOp::Write, ..) => unreachable!("a write gives `Unit`"),
            Rvalue::Length(collection) => {
                let collection_ty = self.operand_ty(collection);
                let value = self.scalar(collection);
                Val::Scalar(match self.e.cg.types().kind(self.e.cg.types().repr(collection_ty)) {
                    TyKind::Array(_) | TyKind::Deque(_) => self.e.fb.ins().load(types::I64, flags(), value, 8),
                    TyKind::String => self.e.call_rt(Rt::StrLength, &[value]).unwrap(),
                    _ => self.e.call_rt(Rt::MapLen, &[value]).unwrap(),
                })
            }
            Rvalue::StrOp(op, operands, span) => {
                let values: Vec<ir::Value> = operands.iter().map(|o| self.scalar(o)).collect();
                match op {
                    StrOp::At => {
                        let location = self.location(*span);
                        Val::Scalar(self.e.call_rt(Rt::StrAt, &[values[0], values[1], location]).unwrap())
                    }
                    StrOp::Bytes => Val::Scalar(self.e.call_rt(Rt::StrToBytes, &values).unwrap()),
                    StrOp::FromBytes => {
                        let text = self.e.call_rt(Rt::StrFromBytes, &values).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, text) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                    StrOp::Get | StrOp::Find => {
                        // A position, or -1 for `none`.
                        let rt = if *op == StrOp::Get { Rt::StrGet } else { Rt::StrFind };
                        let found = self.e.call_rt(rt, &values).unwrap();
                        let present = self.e.fb.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, found, 0);
                        let base = self.pointer_to(place, true);
                        self.e.fb.ins().store(flags(), present, base, 0);
                        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                        let value = if *op == StrOp::Get { self.e.fb.ins().ireduce(types::I32, found) } else { found };
                        self.e.fb.ins().store(flags(), value, base, payload.offset as i32);
                        return;
                    }
                    StrOp::Contains | StrOp::StartsWith | StrOp::EndsWith => {
                        let test = match op {
                            StrOp::Contains => 0,
                            StrOp::StartsWith => 1,
                            _ => 2,
                        };
                        let test = self.e.fb.ins().iconst(types::I32, test);
                        Val::Scalar(self.e.call_rt(Rt::StrTest, &[values[0], values[1], test]).unwrap())
                    }
                    StrOp::LowerAscii | StrOp::UpperAscii => {
                        let transform = match op {
                            StrOp::LowerAscii => 1,
                            _ => 2,
                        };
                        let transform = self.e.fb.ins().iconst(types::I32, transform);
                        Val::Scalar(self.e.call_rt(Rt::StrTransform, &[values[0], transform]).unwrap())
                    }
                    StrOp::Replace => Val::Scalar(self.e.call_rt(Rt::StrReplace, &values).unwrap()),
                    StrOp::FromChars => Val::Scalar(self.e.call_rt(Rt::StrFromChars, &values).unwrap()),
                    StrOp::SliceChecked => {
                        let error = self.scratch(Ty::BYTE);
                        let slice = self.e.call_rt(Rt::StrSliceChecked, &[values[0], values[1], values[2], error]);
                        let code = self.e.fb.ins().load(types::I8, flags(), error, 0);
                        self.write_checked(place, ty, slice.unwrap(), code);
                        return;
                    }
                    StrOp::ByteLength => Val::Scalar(self.e.fb.ins().load(types::I64, flags(), values[0], 8)),
                    StrOp::Concat => Val::Scalar(self.e.call_rt(Rt::StrConcat, &values).unwrap()),
                    StrOp::Decode => Val::Scalar(self.e.call_rt(Rt::StrDecode, &values).unwrap()),
                    StrOp::Next => Val::Scalar(self.e.call_rt(Rt::StrNext, &values).unwrap()),
                }
            }
            Rvalue::MapOp(op, operands, _) => match op {
                MapOp::New => {
                    let (size, offset) = self.e.cg.entry_layout(ty);
                    let size = self.e.fb.ins().iconst(POINTER, size as i64);
                    let offset = self.e.fb.ins().iconst(POINTER, offset as i64);
                    Val::Scalar(self.e.call_rt(Rt::MapNew, &[size, offset]).unwrap())
                }
                MapOp::Contains | MapOp::Get => {
                    let map_ty = self.operand_ty(&operands[0]);
                    let (key, value) = self.e.cg.map_parts(map_ty);
                    let map = self.scalar(&operands[0]);
                    let key_value = self.operand(&operands[1]);
                    let key_address = self.in_memory(key, key_value);
                    let glue = self.e.map_glue_address(map_ty);
                    let found = self.e.call_rt(Rt::MapFind, &[map, key_address, glue]).unwrap();
                    let present = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, found, 0);
                    if *op == MapOp::Contains {
                        Val::Scalar(present)
                    } else {
                        // `some(value)` owns a copy of the stored value.
                        let base = self.pointer_to(place, true);
                        self.e.fb.ins().store(flags(), present, base, 0);
                        let (copy, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                        self.e.fb.ins().brif(present, copy, &[], done, &[]);
                        self.e.fb.switch_to_block(copy);
                        let stored = self.e.value_at(found, value);
                        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                        self.e.init_component(base, &payload, stored);
                        self.e.refcount(value, stored, false);
                        self.e.fb.ins().jump(done, &[]);
                        self.e.fb.switch_to_block(done);
                        return;
                    }
                }
            },
            Rvalue::Mutate(mutation, target, operands, retain, span) => {
                match self.mutate(*mutation, target, operands, retain, place, *span) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::AssignSlice(..) => unreachable!("a slice assignment produces `Unit`"),
            Rvalue::CheckRegion(..) => unreachable!("a region check produces `Unit`"),
            Rvalue::CallIndirect(callee, args) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call_indirect(callee, args, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::Callback(target, closure) => {
                let adapter = self.e.cg.callback(*target, ty, *closure);
                let func = self.e.cg.declare_func_in_func(adapter, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::ExternFn(id) => {
                let declared = &self.e.cg.externs()[*id as usize];
                let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
                let signature = self.e.cg.c_signature(&params, result_ty);
                let (function, _) = self.e.cg.c_function(&symbol, &signature);
                let func = self.e.cg.declare_func_in_func(function, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::Fixed(value, span) => {
                let from = self.operand_ty(value);
                let types = self.e.cg.types();
                match (types.kind(types.repr(from)), types.kind(types.repr(ty))) {
                    (TyKind::Array(element), TyKind::CArray(_, count)) => {
                        // An array of another length has no place there (`P0006`).
                        let (element, count) = (*element, *count);
                        let array = self.scalar(value);
                        let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
                        let differs = self.e.fb.ins().icmp_imm_u(IntCC::NotEqual, length, count as i64);
                        self.panic_values_if(differs, 6, *span, |this| {
                            let count = this.e.fb.ins().iconst(types::I64, count as i64);
                            (0x140, length, count, ty)
                        });
                        let destination = self.pointer_to(place, true);
                        let source = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER);
                        let size = self.e.cg.layout(element).size * count;
                        self.e.copy_memory(destination, source, size);
                        return;
                    }
                    (TyKind::CArray(element, count), _) => {
                        let (element, count) = (*element, *count);
                        let Val::Memory(source) = self.operand(value) else { unreachable!("a C array is in memory") };
                        let size = self.e.cg.layout(element).size;
                        let size_value = self.e.fb.ins().iconst(POINTER, size as i64);
                        let count_value = self.e.fb.ins().iconst(POINTER, count as i64);
                        let array = self.e.call_rt(Rt::ArrayNew, &[size_value, count_value]).unwrap();
                        let destination = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER);
                        self.e.copy_memory(destination, source, size * count);
                        Val::Scalar(array)
                    }
                    _ => unreachable!("between an array and a C array"),
                }
            }
            Rvalue::Reflect(op, ty, operands, retain) => {
                let identity = self.e.cg.identity(*ty);
                let global = self.e.cg.declare_data_in_func(identity, self.e.fb.func);
                let identity = self.e.fb.ins().symbol_value(POINTER, global);
                match op {
                    ReflectOp::Box => {
                        let value = self.operand(&operands[0]);
                        if *retain {
                            self.e.refcount(*ty, value, false);
                        }
                        let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(*ty).size as i64);
                        let release = self.e.glue_or_null(Glue::Release, *ty);
                        let visit = self.e.glue_or_null(Glue::Visit, *ty);
                        let object = self.e.call_rt(Rt::ReflectBox, &[size, release, identity, visit]).unwrap();
                        let address = self.e.fb.ins().iadd_imm_s(object, REFLECT_VALUE);
                        self.e.store_val(address, *ty, value);
                        Val::Scalar(object)
                    }
                    ReflectOp::Unbox => {
                        let object = self.scalar(&operands[0]);
                        let address = self.e.fb.ins().iadd_imm_s(object, REFLECT_VALUE);
                        let value = self.e.value_at(address, *ty);
                        // The box keeps its own.
                        self.e.refcount(*ty, value, false);
                        value
                    }
                    ReflectOp::Is => {
                        let object = self.scalar(&operands[0]);
                        Val::Scalar(self.e.call_rt(Rt::ReflectIs, &[object, identity]).unwrap())
                    }
                    // The identity holds the name, an immortal string.
                    ReflectOp::Name => Val::Scalar(self.e.fb.ins().load(POINTER, flags(), identity, 0)),
                    ReflectOp::Identity => Val::Scalar(identity),
                    ReflectOp::Describe => {
                        let value = self.operand(&operands[0]);
                        Val::Scalar(self.e.describe(*ty, value, true))
                    }
                    ReflectOp::Capabilities => {
                        let types = self.e.cg.types();
                        let bits = types.is_copy(*ty) as i64
                            | (types.is_equatable(*ty) as i64) << 1
                            | (types.is_key(*ty) as i64) << 2
                            | (types.is_send(*ty) as i64) << 3
                            | (types.is_share(*ty) as i64) << 4;
                        Val::Scalar(self.e.fb.ins().iconst(types::I64, bits))
                    }
                }
            }
            Rvalue::Layout(measured, align) => {
                let layout = self.e.cg.layout(*measured);
                let value = if *align { layout.align } else { layout.size };
                Val::Scalar(self.e.fb.ins().iconst(types::I64, value as i64))
            }
            Rvalue::FnValue(id) => {
                let data = self.e.cg.fn_value(*id, &self.bodies[id.0 as usize]);
                let global = self.e.cg.declare_data_in_func(data, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Rvalue::Erase(closure, body, retain) => {
                let env = self.operand_ty(closure);
                let value = self.operand(closure);
                if *retain {
                    self.e.refcount(env, value, false);
                }
                let code = self.e.cg.closure_code(*body);
                let code = self.e.cg.declare_func_in_func(code, self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, code);
                let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(env).size as i64);
                let release = self.e.glue_or_null(Glue::Release, env);
                let visit = self.e.glue_or_null(Glue::Visit, env);
                let object = match place.path.is_empty() && self.body.framed.contains(&place.local) {
                    // A value only this frame reads, in room the frame keeps.
                    true => {
                        let room = FN_HEADER as u32 + self.e.cg.layout(env).size;
                        let slot = *self.framed.entry(place.local).or_insert_with(|| {
                            let data = StackSlotData::new(StackSlotKind::ExplicitSlot, room, 3);
                            self.e.fb.create_sized_stack_slot(data)
                        });
                        let room = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                        self.e.call_rt(Rt::FnIn, &[room, code, size, release, visit]).unwrap()
                    }
                    false => self.e.call_rt(Rt::FnNew, &[code, size, release, visit]).unwrap(),
                };
                let captures = self.e.fb.ins().iadd_imm_s(object, FN_HEADER);
                self.e.store_val(captures, env, value);
                Val::Scalar(object)
            }
            Rvalue::TestMembers(members) => {
                self.test_members(members);
                Val::Zero
            }
            Rvalue::Spawn(body, retain) => {
                let TyKind::Fn(_, result, ..) = *self.e.cg.types().kind(self.operand_ty(body)) else {
                    unreachable!("a task runs a `fn()` value")
                };
                let object = self.scalar(body);
                if *retain {
                    self.e.call_rt(Rt::FnRetain, &[object]);
                }
                // How the body returns its result: see `ResultKind` in the runtime.
                let kind = match self.e.cg.class(result) {
                    Class::Zero => 0,
                    Class::Scalar(types::F64) => 2,
                    Class::Scalar(types::F32) => 3,
                    Class::Scalar(_) => 1,
                    Class::Memory => 4,
                };
                let layout = self.e.cg.layout(result);
                let size = if kind == 0 { 0 } else { layout.size };
                let kind = self.e.fb.ins().iconst(types::I8, kind);
                let size = self.e.fb.ins().iconst(POINTER, size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, result);
                let visit = self.e.glue_or_null(Glue::Visit, result);
                Val::Scalar(self.e.call_rt(Rt::Spawn, &[object, kind, size, align, release, visit]).unwrap())
            }
            Rvalue::Select(cases, blocking, span) => Val::Scalar(self.select(cases, *blocking, *span)),
            // A live build's patches link only the runtime's functions.
            Rvalue::Cancelled if self.e.cg.live() => Val::Scalar(self.e.call_rt(Rt::Cancelled, &[]).unwrap()),
            // After a statement whose panics jumped to the unwinding
            // themselves, nothing to ask.
            Rvalue::Unwinding if self.e.cg.live() && !self.quiet_check => {
                Val::Scalar(self.e.call_rt(Rt::Unwinding, &[]).unwrap())
            }
            Rvalue::Unwinding if self.quiet_check => Val::Scalar(self.e.fb.ins().iconst(types::I8, 0)),
            Rvalue::Unwinding => {
                let id = self.e.cg.attention();
                let attention = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                let address = self.e.fb.ins().symbol_value(POINTER, attention);
                let reasons = self.e.fb.ins().load(types::I32, flags(), address, 0);
                let (ask, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                let answer = self.e.fb.append_block_param(done, types::I8);
                let none = self.e.fb.ins().iconst(types::I8, 0);
                self.e.fb.ins().brif(reasons, ask, &[], done, &[none.into()]);
                self.e.fb.set_cold_block(ask);
                self.e.fb.switch_to_block(ask);
                let asked = self.e.call_rt(Rt::Unwinding, &[]).unwrap();
                self.e.fb.ins().jump(done, &[asked.into()]);
                self.e.fb.switch_to_block(done);
                Val::Scalar(answer)
            }
            Rvalue::Cancelled => {
                // Without a reason to ask the runtime, nearly always, no call.
                let id = self.e.cg.attention();
                let attention = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                let address = self.e.fb.ins().symbol_value(POINTER, attention);
                let reasons = self.e.fb.ins().load(types::I32, flags(), address, 0);
                let (ask, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                let answer = self.e.fb.append_block_param(done, types::I8);
                let none = self.e.fb.ins().iconst(types::I8, 0);
                self.e.fb.ins().brif(reasons, ask, &[], done, &[none.into()]);
                self.e.fb.set_cold_block(ask);
                self.e.fb.switch_to_block(ask);
                let asked = self.e.call_rt(Rt::Cancelled, &[]).unwrap();
                self.e.fb.ins().jump(done, &[asked.into()]);
                self.e.fb.switch_to_block(done);
                Val::Scalar(answer)
            }
            // What the probe of the line that starts here said; no line
            // starts here, no restart.
            Rvalue::Restart(point) => Val::Scalar(match self.probed.take() {
                Some(asked) => {
                    self.carrying = true;
                    if let Some((value, number)) = self.site_now {
                        self.point_sites.insert(*point, value);
                        self.e.cg.point_site(*point, number);
                    }
                    self.e.fb.ins().uextend(types::I64, asked)
                }
                None => self.e.fb.ins().iconst(types::I64, 0),
            }),
            Rvalue::Transfer(point) => Val::Scalar(self.transfer(*point)),
            Rvalue::OsrPoint => {
                let state = self.osr_state();
                Val::Scalar(self.e.fb.ins().load(types::I64, flags(), state, 0))
            }
            Rvalue::CallOsr(id, args, state) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call_osr(*id, args, state, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::TaskOp(op, operands, retain, span) => match self.task_op(*op, operands, retain, place, *span) {
                Some(value) => value,
                None => return,
            },
            Rvalue::Discriminant(source) => {
                let source_ty = self.place_ty(source);
                let value = self.read(source);
                Val::Scalar(self.e.tag(source_ty, value))
            }
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Unary(op, operand) => Val::Scalar(self.unary(*op, operand, span)),
            Rvalue::Increment(operand) => {
                let value = self.scalar(operand);
                Val::Scalar(self.e.fb.ins().iadd_imm_s(value, 1))
            }
            Rvalue::Exact(op, left, right) => {
                let (a, b) = (self.scalar(left), self.scalar(right));
                let ins = self.e.fb.ins();
                Val::Scalar(match op {
                    BinOp::Add => ins.iadd(a, b),
                    BinOp::Sub => ins.isub(a, b),
                    BinOp::Div => ins.udiv(a, b),
                    BinOp::Rem => ins.urem(a, b),
                    _ => ins.imul(a, b),
                })
            }
            Rvalue::Binary(op, left, right) => Val::Scalar(self.binary(*op, left, right, span)),
            Rvalue::Concat(pieces) => {
                let framed = self.framed_room(place);
                Val::Scalar(self.concat(pieces, framed))
            }
        };
        self.write(place, value);
    }
}
