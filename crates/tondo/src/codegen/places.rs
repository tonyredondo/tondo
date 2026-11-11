//! Places and values in memory: operands, slots, arrays and slices, reads and writes.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    pub(super) fn place_ty(&self, place: &Place) -> Ty {
        self.body.place_ty(self.e.cg.types(), place)
    }

    pub(super) fn operand_ty(&self, operand: &Operand) -> Ty {
        match operand {
            Operand::Copy(place) => self.place_ty(place),
            Operand::Const(Const::Int(_, ty) | Const::Float(_, ty)) => *ty,
            Operand::Const(Const::Char(_)) => Ty::CHAR,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
            Operand::Const(Const::Bytes(_, ty)) => *ty,
            Operand::Const(Const::Unit) => Ty::UNIT,
        }
    }

    /// The address of a place inside an aggregate local. When `write` is
    /// set, every box on the way is made unique first (copy-on-write).
    pub(super) fn pointer_to(&mut self, place: &Place, write: bool) -> ir::Value {
        // Where the value reached so far lives: in memory, or (only for an
        // array held by the root local) in a variable.
        let mut base = match self.storage[place.local.0 as usize] {
            Storage::Slot(slot) => Some(self.e.fb.ins().stack_addr(POINTER, slot, 0)),
            Storage::Pointer(var) => Some(self.e.fb.use_var(var)),
            Storage::Var(_) => None,
            Storage::Zero => unreachable!("address of a zero-sized value"),
        };
        let mut ty = self.body.locals[place.local.0 as usize];
        for &proj in &place.path {
            if let Proj::Index(index, _) = proj
                && matches!(self.e.cg.types().kind(self.e.cg.types().repr(ty)), TyKind::Map(..) | TyKind::Set(_))
            {
                // Iteration over a map or set: the entry at a valid position,
                // in storage of its own when it changes (13.3).
                let map = match base {
                    Some(address) => {
                        if write {
                            let glue = self.e.map_glue_address(ty);
                            self.e.call_rt(Rt::MapUnique, &[address, glue]);
                        }
                        self.e.fb.ins().load(POINTER, flags(), address, 0)
                    }
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        if write {
                            let slot = self.spill(var);
                            let glue = self.e.map_glue_address(ty);
                            self.e.call_rt(Rt::MapUnique, &[slot, glue]);
                            self.unspill(var, slot);
                        }
                        self.e.fb.use_var(var)
                    }
                };
                let position = self.scalar(&Operand::local(index));
                base = Some(self.e.call_rt(Rt::MapEntry, &[map, position]).unwrap());
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            if let Proj::Index(index, span) = proj
                && let TyKind::CArray(element, count) = *self.e.cg.types().kind(ty)
            {
                // A C array is stored in place.
                let address = base.expect("a C array lives in memory");
                let index = self.scalar(&Operand::local(index));
                let outside = self.e.fb.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, index, count as i64);
                self.panic_values_if(outside, 1, span, |this| {
                    let count = this.e.fb.ins().iconst(types::I64, count as i64);
                    (1 << 16 | 0x140, index, count, ty)
                });
                let size = self.e.cg.layout(element).size;
                let offset = self.e.fb.ins().imul_imm_u(index, size as i64);
                base = Some(self.e.fb.ins().iadd(address, offset));
                ty = element;
                continue;
            }
            if let Proj::Captures(captures) = proj {
                let object = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                base = Some(self.e.fb.ins().iadd_imm_s(object, FN_HEADER));
                ty = captures;
                continue;
            }
            if let Proj::Deref = proj {
                assert!(!write, "the value of a `Ref` is never written");
                let reference = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                base = Some(self.e.ref_value(reference));
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            // The value inside an option of a reference is the option.
            if base.is_none()
                && let Proj::Variant(SOME, 0) = proj
                && let Shape::Niche = self.e.cg.layout(ty).shape
            {
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            if let Proj::Index(index, _) | Proj::InBounds(index) = proj {
                let element = project(self.e.cg.types(), ty, proj);
                let sequence = match base {
                    Some(address) => {
                        if write {
                            self.make_unique(address, element);
                        }
                        self.e.fb.ins().load(POINTER, flags(), address, 0)
                    }
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        if write {
                            let slot = self.spill(var);
                            self.make_unique(slot, element);
                            self.unspill(var, slot);
                        }
                        self.e.fb.use_var(var)
                    }
                };
                let index = self.scalar(&Operand::local(index));
                base = Some(match proj {
                    Proj::Index(_, span) => self.element_address(sequence, ty, index, span),
                    _ => self.element_at(sequence, ty, index),
                });
                ty = element;
                continue;
            }
            let address = base.expect("fields live in memory");
            let slot = self.e.cg.slot(ty, proj);
            if slot.boxed && write {
                let cell = self.e.offset(address, slot.offset);
                let retain = self.e.glue_address(Glue::Retain, slot.ty);
                let release = self.e.glue_address(Glue::Release, slot.ty);
                self.e.call_rt(Rt::BoxUnique, &[cell, retain, release]);
            }
            base = Some(self.e.component_address(address, &slot));
            ty = slot.ty;
        }
        base.expect("address of a scalar local")
    }

    /// The address of `sequence[index]`, panicking with `P0001` out of
    /// bounds.
    pub(super) fn element_address(&mut self, sequence: ir::Value, ty: Ty, index: ir::Value, span: Span) -> ir::Value {
        // An index from the start, in bounds, takes one comparison; one
        // from the end, or out of bounds, takes the cold path.
        let length = self.e.fb.ins().load(types::I64, flags(), sequence, 8);
        let (found, from_end, next) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        let address = self.e.fb.append_block_param(next, POINTER);
        let inside = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, index, length);
        self.e.fb.ins().brif(inside, found, &[], from_end, &[]);
        self.e.fb.switch_to_block(found);
        let element = self.element_at(sequence, ty, index);
        self.e.fb.ins().jump(next, &[element.into()]);
        self.e.fb.set_cold_block(from_end);
        self.e.fb.switch_to_block(from_end);
        let (wrapped, inside) = self.position(sequence, index);
        let outside = self.e.fb.ins().bxor_imm_u(inside, 1);
        let deque = self.e.cg.sequence_parts(ty).1;
        self.panic_values_if(outside, 1, span, |_| ((deque as u32) << 17 | 0x140, index, length, ty));
        let rest = self.e.fb.current_block().unwrap();
        self.e.fb.set_cold_block(rest);
        let element = self.element_at(sequence, ty, wrapped);
        self.e.fb.ins().jump(next, &[element.into()]);
        self.e.fb.switch_to_block(next);
        address
    }

    /// An index as a position, counting from the end when negative (10.3),
    /// and whether that position holds an element.
    pub(super) fn position(&mut self, sequence: ir::Value, index: ir::Value) -> (ir::Value, ir::Value) {
        let length = self.e.fb.ins().load(types::I64, flags(), sequence, 8);
        let wrapped = self.e.fb.ins().iadd(index, length);
        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
        let position = self.e.fb.ins().select(negative, wrapped, index);
        let inside = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, position, length);
        (position, inside)
    }

    /// The address of the element at a valid position of an array or deque.
    pub(super) fn element_at(&mut self, sequence: ir::Value, ty: Ty, position: ir::Value) -> ir::Value {
        let (element, deque) = self.e.cg.sequence_parts(ty);
        let position = if deque {
            let head = self.e.fb.ins().load(types::I64, flags(), sequence, 24);
            self.e.fb.ins().iadd(position, head)
        } else {
            position
        };
        let size = self.e.cg.layout(element).size as i64;
        let offset = self.e.fb.ins().imul_imm_s(position, size);
        let address = self.e.fb.ins().iadd(sequence, offset);
        self.e.fb.ins().iadd_imm_s(address, ARRAY_HEADER)
    }

    /// The runtime arguments of `[start:end:step]`: each bound with a flag
    /// for its presence, the step, and the location for panics.
    pub(super) fn slice_range(&mut self, parts: &[Option<Operand>; 3], span: Span) -> [ir::Value; 6] {
        let bound = |this: &mut Self, part: &Option<Operand>, default: i64| match part {
            Some(part) => (this.scalar(part), this.e.fb.ins().iconst(types::I8, 1)),
            None => (this.e.fb.ins().iconst(types::I64, default), this.e.fb.ins().iconst(types::I8, 0)),
        };
        let (start, has_start) = bound(self, &parts[0], 0);
        let (end, has_end) = bound(self, &parts[1], 0);
        let (step, _) = bound(self, &parts[2], 1);
        let location = self.location(span);
        [start, has_start, end, has_end, step, location]
    }

    /// `values[start:end:step] = value` (10.5).
    pub(super) fn assign_slice(&mut self, target: &Place, parts: &[Option<Operand>; 3], value: &Operand, span: Span) {
        let (element, _) = self.e.cg.sequence_parts(self.place_ty(target));
        let range = self.slice_range(parts, span);
        let source = self.scalar(value);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let (slot, spilled) = self.array_slot(target);
        let [start, has_start, end, has_end, step, location] = range;
        let args = [slot, start, has_start, end, has_end, step, source, size, retain, release, location];
        self.e.call_rt(Rt::ArrayAssignSlice, &args);
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
    }

    /// Two loans of one call that select elements of the same array (16.4).
    pub(super) fn check_region(&mut self, array: &Operand, region: &[Option<Operand>; 3], other: &Overlap, span: Span) {
        let array = self.scalar(array);
        let len = self.e.fb.ins().load(types::I64, flags(), array, 8);
        let [start, has_start, end, has_end, step, location] = self.slice_range(region, span);
        let (index, second) = match other {
            Overlap::Region(parts) => (0, self.slice_range(parts, span)),
            Overlap::Index(at) => {
                let at = self.scalar(at);
                let (zero, none) = (self.e.fb.ins().iconst(types::I64, 0), self.e.fb.ins().iconst(types::I8, 0));
                (1, [at, none, zero, none, zero, location])
            }
        };
        let index = self.e.fb.ins().iconst(types::I8, index);
        let [start2, has_start2, end2, has_end2, step2, _] = second;
        let args =
            [len, start, has_start, end, has_end, step, index, start2, has_start2, end2, has_end2, step2, location];
        self.e.call_rt(Rt::ArrayCheckRegion, &args);
    }

    /// Lifted arithmetic over arrays (10.6), done by the runtime.
    pub(super) fn array_arithmetic(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let mut args = Vec::new();
        let mut number = Ty::ERROR;
        for operand in [left, right] {
            let mut ty = self.operand_ty(operand);
            let value = self.operand(operand);
            args.push(self.in_memory(ty, value));
            let mut depth = 0;
            while let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) {
                ty = element;
                depth += 1;
            }
            args.push(self.e.fb.ins().iconst(types::I32, depth));
            number = ty;
        }
        let op = match op {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            _ => 4,
        };
        // The runtime's codes: the integers of 8 to 64 bits, `Float32`,
        // `Float`, `Int128` and `UInt128`.
        let code = match *self.e.cg.types().kind(self.e.cg.types().repr(number)) {
            TyKind::Int(IntTy::I128) => 10,
            TyKind::Int(IntTy::U128) => 11,
            TyKind::Int(int) => IntTy::ALL.iter().position(|&i| i == int).unwrap() as i64,
            TyKind::Float(FloatTy::F32) => 8,
            _ => 9,
        };
        args.push(self.e.fb.ins().iconst(types::I32, op));
        args.push(self.e.fb.ins().iconst(types::I32, code));
        args.push(self.location(span));
        self.e.call_rt(Rt::ArrayArith, &args).unwrap()
    }

    /// Stack space for a value of `ty` that the runtime writes.
    pub(super) fn scratch(&mut self, ty: Ty) -> ir::Value {
        let size = self.e.cg.layout(ty).size.max(1);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_addr(POINTER, slot, 0)
    }

    /// Makes the array stored at `slot` unique before a write (copy-on-write).
    pub(super) fn make_unique(&mut self, slot: ir::Value, element: Ty) {
        // Nearly always the only reference already: no call.
        let array = self.e.fb.ins().load(POINTER, flags(), slot, 0);
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), array);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let (copy, done) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(unique, done, &[], copy, &[]);
        self.e.fb.set_cold_block(copy);
        self.e.fb.switch_to_block(copy);
        let size = self.e.element_size(element);
        let extra = self.e.fb.ins().iconst(POINTER, 0);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        self.e.call_rt(Rt::ArrayUnique, &[slot, size, extra, retain, release]);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// Stores a variable into a fresh stack slot so the runtime can update it.
    pub(super) fn spill(&mut self, var: Variable) -> ir::Value {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let value = self.e.fb.use_var(var);
        self.e.fb.ins().store(flags(), value, address, 0);
        address
    }

    pub(super) fn unspill(&mut self, var: Variable, address: ir::Value) {
        let current = self.e.fb.use_var(var);
        let ty = self.e.fb.func.dfg.value_type(current);
        let value = self.e.fb.ins().load(ty, flags(), address, 0);
        self.define(var, value);
    }

    /// The address of the storage holding the array at `place`, spilled to
    /// the stack when it lives in a variable; `unspill` it after the call.
    pub(super) fn array_slot(&mut self, place: &Place) -> (ir::Value, Option<Variable>) {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            return (self.spill(var), Some(var));
        }
        (self.pointer_to(place, true), None)
    }

    /// The address of a value, spilling a scalar to the stack.
    pub(super) fn in_memory(&mut self, ty: Ty, value: Val) -> ir::Value {
        match value {
            Val::Memory(address) => address,
            Val::Scalar(value) => {
                let size = self.e.cg.layout(ty).size.max(1);
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                self.e.fb.ins().store(flags(), value, address, 0);
                address
            }
            // The runtime copies nothing, but needs a valid address.
            Val::Zero => self.scratch(ty),
        }
    }

    pub(super) fn read(&mut self, place: &Place) -> Val {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            return Val::Scalar(self.e.fb.use_var(var));
        }
        // The address inside an option of one is the option itself.
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && let [Proj::Variant(SOME, 0)] = place.path[..]
            && let Shape::Niche = self.e.cg.layout(self.body.locals[place.local.0 as usize]).shape
        {
            return Val::Scalar(self.e.fb.use_var(var));
        }
        match self.e.cg.class(self.place_ty(place)) {
            Class::Zero => Val::Zero,
            _ => {
                let address = self.pointer_to(place, false);
                self.e.value_at(address, self.place_ty(place))
            }
        }
    }

    pub(super) fn write(&mut self, place: &Place, value: Val) {
        if let (Val::Scalar(value), Storage::Var(var)) = (value, self.storage[place.local.0 as usize])
            && place.path.is_empty()
        {
            self.define(var, value);
            return;
        }
        if let Val::Zero = value {
            return;
        }
        let address = self.pointer_to(place, true);
        let ty = self.place_ty(place);
        self.e.store_val(address, ty, value);
    }

    /// The elements of an array literal when all are constants of its
    /// element's type that the program's data can hold; never in a live
    /// build, whose reloads replace the arrays they convert and free them.
    pub(super) fn constants(&mut self, values: &[Operand], ty: Ty) -> Option<Vec<Const>> {
        if self.e.cg.live() {
            return None;
        }
        let types = self.e.cg.types();
        let TyKind::Array(element) = *types.kind(types.repr(ty)) else { return None };
        // An empty one too, as the notes of a derived encoder at every call:
        // the first change copies it, as it would have allocated anyway.
        values
            .iter()
            .map(|value| match value {
                Operand::Const(constant) if in_data(types, element, constant) => Some(constant.clone()),
                _ => None,
            })
            .collect()
    }

    pub(super) fn operand(&mut self, operand: &Operand) -> Val {
        match operand {
            Operand::Copy(place) => self.read(place),
            Operand::Const(Const::Int(value, ty)) => {
                let clif = self.e.cg.scalar_type(*ty);
                Val::Scalar(self.e.int_const(clif, *value))
            }
            Operand::Const(Const::Float(bits, ty)) => {
                let value = f64::from_bits(*bits);
                Val::Scalar(if *ty == Ty::F32 {
                    self.e.fb.ins().f32const(value as f32)
                } else {
                    self.e.fb.ins().f64const(value)
                })
            }
            Operand::Const(Const::Char(c)) => Val::Scalar(self.e.fb.ins().iconst(types::I32, *c as i64)),
            Operand::Const(Const::Bool(value)) => Val::Scalar(self.e.fb.ins().iconst(types::I8, *value as i64)),
            Operand::Const(Const::Str(text)) => Val::Scalar(self.e.string_ptr(text)),
            Operand::Const(Const::Bytes(bytes, _)) => {
                let id = self.e.cg.bytes(bytes);
                let global = self.e.global(id);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Operand::Const(Const::Unit) => Val::Zero,
        }
    }

    pub(super) fn scalar(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar operand"),
        }
    }

    /// Compiles what follows as the source at `span`: runtime panics report
    /// it, and stack traces map the calls to it.
    pub(super) fn at(&mut self, span: Span) {
        self.at_typed(span, None);
    }

    /// The same for a statement that assigns a local of type `ty`.
    pub(super) fn at_typed(&mut self, span: Span, ty: Option<Ty>) {
        self.span = span;
        // A statement the compiler made up stays at the last one's.
        if span != Span::default() {
            let site = self.site(span.start, ty);
            self.e.fb.set_srcloc(site);
        }
    }

    /// The source location of code at `start` that assigns `ty`.
    pub(super) fn site(&mut self, start: u32, ty: Option<Ty>) -> ir::SourceLoc {
        self.sites.push((start, ty, self.scope));
        ir::SourceLoc::new(self.sites.len() as u32)
    }

    /// In a build without probes, a place a logpoint can use at the start of
    /// a line: a call that is nothing until then, and leaves every register
    /// as it was. Not in a loop that calls nothing, where even that shows.
    pub(super) fn latent_site(&mut self, span: Span) {
        let tight = self.tight_loops.get(self.block).copied().unwrap_or(false);
        if !self.e.cg.dumps() || tight || span == Span::default() || !crate::driver::logpoint_sites() {
            return;
        }
        let (_, line, _) = self.e.cg.sources().position(span.start);
        if self.latent_line == Some(line) {
            return;
        }
        self.latent_line = Some(line);
        let callee = match self.latent_callee {
            Some(callee) => callee,
            None => {
                let id = self.e.cg.runtime(Rt::Site);
                let callee = self.e.cg.declare_func_in_func(id, self.e.fb.func);
                let data = &mut self.e.fb.func.dfg.ext_funcs[callee];
                data.patchable = true;
                let signature = data.signature;
                self.e.fb.func.dfg.signatures[signature].call_conv = cranelift_codegen::isa::CallConv::PreserveAll;
                self.latent_callee = Some(callee);
                callee
            }
        };
        self.e.fb.ins().call(callee, &[]);
    }
}

/// Whether a constant element of an array literal goes, with the array,
/// in the program's data rather than on the heap (`constants`).
pub(crate) fn in_data(types: &crate::types::Types, element: Ty, constant: &Const) -> bool {
    let element = types.repr(element);
    match constant {
        Const::Int(_, of) | Const::Float(_, of) => types.repr(*of) == element,
        Const::Bool(_) => element == Ty::BOOL,
        Const::Char(_) => element == Ty::CHAR,
        Const::Str(_) => element == Ty::STRING,
        Const::Bytes(_, of) => types.repr(*of) == element,
        Const::Unit => false,
    }
}
