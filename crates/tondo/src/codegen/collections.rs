//! Arrays, maps and their changes.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// An intrinsic array operation. Returns the value for scalar results;
    /// results in memory are written to `place` directly.
    pub(super) fn array_op(
        &mut self,
        op: ArrayOp,
        operands: &[Operand],
        place: &Place,
        ty: Ty,
        span: Span,
    ) -> Option<Val> {
        let types = self.e.cg.types();
        let element_of = |ty: Ty| match *types.kind(types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) => element,
            _ => unreachable!("not an array or deque"),
        };
        Some(match op {
            ArrayOp::New => {
                // The empty array in the program's data, as `[]` is: the
                // first change makes it one of its own.
                let size = self.e.cg.layout(element_of(ty)).size as usize;
                let id = self.e.cg.static_array(size, Vec::new());
                let global = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            ArrayOp::WithCapacity => {
                let TyKind::Result(array_ty, _) = *types.kind(ty) else { unreachable!() };
                let capacity = self.scalar(&operands[0]);
                let size = self.e.element_size(element_of(array_ty));
                let zero = self.e.fb.ins().iconst(POINTER, 0);
                let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, capacity, 0);
                // The capacity is a hint, which a negative one fails; storage
                // grows on demand.
                let room = self.e.fb.ins().select(negative, zero, capacity);
                let array = self.e.call_rt(Rt::ArrayReserved, &[size, room]).unwrap();
                let invalid = self.e.fb.ins().iconst(types::I8, 0);
                let fine = self.e.fb.ins().iconst(types::I8, -1);
                let error = self.e.fb.ins().select(negative, invalid, fine);
                self.write_checked(place, ty, array, error);
                // On failure the unused array is released.
                let (release, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                self.e.fb.ins().brif(negative, release, &[], done, &[]);
                self.e.fb.switch_to_block(release);
                self.e.refcount(array_ty, Val::Scalar(array), true);
                self.e.fb.ins().jump(done, &[]);
                self.e.fb.switch_to_block(done);
                return None;
            }
            ArrayOp::Get => {
                let sequence_ty = self.operand_ty(&operands[0]);
                let element = element_of(sequence_ty);
                let sequence = self.scalar(&operands[0]);
                let index = self.scalar(&operands[1]);
                let (position, inside) = self.position(sequence, index);
                if self.e.cg.class(ty) != Class::Memory {
                    return Some(Val::Scalar(inside));
                }
                let base = self.pointer_to(place, true);
                self.e.fb.ins().store(flags(), inside, base, 0);
                let (found, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                self.e.fb.ins().brif(inside, found, &[], done, &[]);
                self.e.fb.switch_to_block(found);
                let address = self.element_at(sequence, sequence_ty, position);
                let value = self.e.value_at(address, element);
                let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                self.e.init_component(base, &payload, value);
                // The option owns a copy of the element.
                self.e.refcount(element, value, false);
                self.e.fb.ins().jump(done, &[]);
                self.e.fb.switch_to_block(done);
                return None;
            }
            ArrayOp::SliceChecked => {
                let array_ty = self.operand_ty(&operands[0]);
                let element = element_of(array_ty);
                let array = self.scalar(&operands[0]);
                let (start, end) = (self.scalar(&operands[1]), self.scalar(&operands[2]));
                let size = self.e.element_size(element);
                let retain = self.e.glue_or_null(Glue::Retain, element);
                let slice = self.e.call_rt(Rt::ArraySliceChecked, &[array, size, retain, start, end]).unwrap();
                let failed = self.e.fb.ins().icmp_imm_s(IntCC::Equal, slice, 0);
                let invalid_index = self.e.fb.ins().iconst(types::I8, 1);
                let fine = self.e.fb.ins().iconst(types::I8, -1);
                let error = self.e.fb.ins().select(failed, invalid_index, fine);
                self.write_checked(place, ty, slice, error);
                return None;
            }
            ArrayOp::Concat | ArrayOp::Repeat => {
                let element = element_of(ty);
                let (a, b) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let size = self.e.element_size(element);
                let retain = self.e.glue_or_null(Glue::Retain, element);
                let location = self.location(span);
                let rt = if op == ArrayOp::Concat { Rt::ArrayConcat } else { Rt::ArrayRepeat };
                Val::Scalar(self.e.call_rt(rt, &[a, b, size, retain, location]).unwrap())
            }
            ArrayOp::Join => {
                let element = element_of(self.operand_ty(&operands[0]));
                let (array, separator) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let builder = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let size = self.e.element_size(element);
                let display = self.e.glue_address(Glue::Display, element);
                self.e.call_rt(Rt::SbJoin, &[builder, array, size, display, separator]);
                Val::Scalar(self.e.call_rt(Rt::SbFinish, &[builder]).unwrap())
            }
            ArrayOp::NativeOrder | ArrayOp::SortedNatively => {
                let element = self.e.cg.types().repr(element_of(self.operand_ty(&operands[0])));
                let rt = match element {
                    Ty::INT => Some(Rt::ArraySortedInts),
                    Ty::STRING => Some(Rt::ArraySortedStrings),
                    _ => None,
                };
                match (op, rt) {
                    (ArrayOp::NativeOrder, _) => Val::Scalar(self.e.fb.ins().iconst(types::I8, rt.is_some() as i64)),
                    (_, Some(rt)) => {
                        let array = self.scalar(&operands[0]);
                        Val::Scalar(self.e.call_rt(rt, &[array]).unwrap())
                    }
                    // Behind a `nativeOrder` that is false.
                    (_, None) => {
                        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
                        let block = self.e.fb.create_block();
                        self.e.fb.switch_to_block(block);
                        Val::Scalar(self.e.fb.ins().iconst(POINTER, 0))
                    }
                }
            }
            ArrayOp::Contains => {
                let element = self.operand_ty(&operands[0]);
                let value = self.operand(&operands[0]);
                let address = self.in_memory(element, value);
                let array = self.scalar(&operands[1]);
                let size = self.e.element_size(element);
                let eq = self.e.glue_address(Glue::Equal, element);
                Val::Scalar(self.e.call_rt(Rt::ArrayContains, &[array, address, size, eq]).unwrap())
            }
        })
    }

    /// A method that changes the collection at `target`. Returns the value
    /// for scalar results; results in memory are written to `place`.
    pub(super) fn mutate(
        &mut self,
        mutation: Mutation,
        target: &Place,
        operands: &[Operand],
        retain: &[bool],
        place: &Place,
        span: Span,
    ) -> Option<Val> {
        let ty = self.place_ty(place);
        let collection_ty = self.place_ty(target);
        let values: Vec<Val> = operands.iter().map(|o| self.operand(o)).collect();
        for ((value, &retain), operand) in values.iter().zip(retain).zip(operands) {
            if retain {
                let ty = self.operand_ty(operand);
                self.e.refcount(ty, *value, false);
            }
        }
        if let TyKind::Map(..) | TyKind::Set(_) = self.e.cg.types().kind(self.e.cg.types().repr(collection_ty)) {
            return self.mutate_map(mutation, target, &values, collection_ty, place, ty);
        }
        let (element, deque) = self.e.cg.sequence_parts(collection_ty);
        if mutation == Mutation::Push && !deque {
            self.push(target, element, values[0]);
            return None;
        }
        let size = self.e.element_size(element);
        let retain_glue = self.e.glue_or_null(Glue::Retain, element);
        let release_glue = self.e.glue_or_null(Glue::Release, element);
        // An inserted value read in place may live in the storage the
        // mutation moves (`values.insert(0, values[1])`): copy it first.
        let source = match mutation {
            Mutation::Push | Mutation::PushFront => Some(self.copy_to_stack(element, values[0])),
            Mutation::InsertAt => Some(self.copy_to_stack(element, values[1])),
            _ => None,
        };
        let removes = matches!(mutation, Mutation::Pop | Mutation::PopFront | Mutation::RemoveAt);
        let out = removes.then(|| self.scratch(element));
        let (slot, spilled) = self.array_slot(target);
        let found = match (mutation, source, out) {
            (Mutation::Push, Some(source), _) if !deque => {
                self.e.call_rt(Rt::ArrayPush, &[slot, source, size, retain_glue, release_glue])
            }
            (Mutation::Push | Mutation::PushFront, Some(source), _) => {
                let front = self.e.fb.ins().iconst(types::I8, (mutation == Mutation::PushFront) as i64);
                self.e.call_rt(Rt::DequePush, &[slot, source, size, front, retain_glue, release_glue])
            }
            (Mutation::InsertAt, Some(source), _) => {
                let at = self.scalar_of(values[0]);
                let location = self.location(span);
                self.e.call_rt(Rt::ArrayInsert, &[slot, at, source, size, retain_glue, release_glue, location])
            }
            (Mutation::Pop, _, Some(out)) => {
                self.e.call_rt(Rt::ArrayPop, &[slot, out, size, retain_glue, release_glue])
            }
            (Mutation::PopFront, _, Some(out)) => {
                self.e.call_rt(Rt::DequePopFront, &[slot, out, size, retain_glue, release_glue])
            }
            (Mutation::RemoveAt, _, Some(out)) => {
                let at = self.scalar_of(values[0]);
                self.e.call_rt(Rt::ArrayRemoveAt, &[slot, at, out, size, retain_glue, release_glue])
            }
            (Mutation::Clear, ..) => self.e.call_rt(Rt::ArrayClear, &[slot, size, release_glue]),
            (Mutation::Reserve, ..) => {
                let extra = self.scalar_of(values[0]);
                self.e.call_rt(Rt::ArrayReserve, &[slot, size, extra, retain_glue, release_glue])
            }
            _ => unreachable!("not a sequence mutation"),
        };
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
        match (found, out) {
            (Some(found), Some(out)) => self.removal_result(place, ty, element, out, found),
            _ => None,
        }
    }

    /// `values.push(value)`: in place when the array is the only reference
    /// to its storage and has room, as nearly always; else by the runtime,
    /// which copies or grows it.
    pub(super) fn push(&mut self, target: &Place, element: Ty, value: Val) {
        let var = match self.storage[target.local.0 as usize] {
            Storage::Var(var) if target.path.is_empty() => Some(var),
            _ => None,
        };
        let address = match var {
            Some(_) => None,
            None => Some(self.pointer_to(target, true)),
        };
        let array = match (var, address) {
            (Some(var), _) => self.e.fb.use_var(var),
            (None, Some(address)) => self.e.fb.ins().load(POINTER, flags(), address, 0),
            (None, None) => unreachable!(),
        };
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), array);
        let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
        let capacity = self.e.fb.ins().load(types::I64, flags(), array, 16);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let room = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, length, capacity);
        let in_place = self.e.fb.ins().band(unique, room);
        let (fast, slow, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(in_place, fast, &[], slow, &[]);
        self.e.fb.switch_to_block(fast);
        let size = self.e.cg.layout(element).size as i64;
        let offset = self.e.fb.ins().imul_imm_s(length, size);
        let at = self.e.fb.ins().iadd(array, offset);
        let at = self.e.fb.ins().iadd_imm_s(at, ARRAY_HEADER);
        self.e.store_val(at, element, value);
        let longer = self.e.fb.ins().iadd_imm_s(length, 1);
        self.e.fb.ins().store(flags(), longer, array, 8);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.set_cold_block(slow);
        self.e.fb.switch_to_block(slow);
        // The value may live in the storage the runtime moves.
        let source = self.copy_to_stack(element, value);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let slot = match (var, address) {
            (Some(var), _) => self.spill(var),
            (None, Some(address)) => address,
            (None, None) => unreachable!(),
        };
        self.e.call_rt(Rt::ArrayPush, &[slot, source, size, retain, release]);
        if let Some(var) = var {
            self.unspill(var, slot);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// The address of a copy of `value` on the stack.
    pub(super) fn copy_to_stack(&mut self, ty: Ty, value: Val) -> ir::Value {
        let Val::Memory(source) = value else { return self.in_memory(ty, value) };
        let address = self.scratch(ty);
        let size = self.e.cg.layout(ty).size;
        self.e.copy_memory(address, source, size);
        address
    }

    /// `insert`, `remove` or `clear` on a map or set.
    pub(super) fn mutate_map(
        &mut self,
        mutation: Mutation,
        target: &Place,
        values: &[Val],
        map_ty: Ty,
        place: &Place,
        ty: Ty,
    ) -> Option<Val> {
        let (key, value) = self.e.cg.map_parts(map_ty);
        let is_set = matches!(self.e.cg.types().kind(self.e.cg.types().repr(map_ty)), TyKind::Set(_));
        let glue = self.e.map_glue_address(map_ty);
        if mutation == Mutation::Clear {
            let (slot, spilled) = self.array_slot(target);
            self.e.call_rt(Rt::MapClear, &[slot, glue]);
            if let Some(var) = spilled {
                self.unspill(var, slot);
            }
            return None;
        }
        let key_address = self.in_memory(key, values[0]);
        // A map's result is `V?` with the replaced or removed value. A set's
        // result is a `Bool`.
        let out = if is_set { key_address } else { self.scratch(value) };
        let (slot, spilled) = self.array_slot(target);
        let flag = match mutation {
            Mutation::Insert => {
                let value_address = match values.get(1) {
                    Some(&v) => self.in_memory(value, v),
                    None => key_address,
                };
                self.e.call_rt(Rt::MapInsert, &[slot, key_address, value_address, out, glue]).unwrap()
            }
            _ => self.e.call_rt(Rt::MapRemove, &[slot, key_address, out, glue]).unwrap(),
        };
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
        match (is_set, mutation) {
            (false, _) => self.removal_result(place, ty, value, out, flag),
            // Set insert reports whether the value was new.
            (true, Mutation::Insert) => Some(Val::Scalar(self.e.fb.ins().bxor_imm_u(flag, 1))),
            (true, _) => Some(Val::Scalar(flag)),
        }
    }
}
