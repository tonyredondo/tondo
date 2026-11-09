//! What debuggers and live builds read and restore of a frame.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// The site of the last probe, and the addresses of the bindings and
    /// of their bits, as a probe gives them to the runtime.
    pub(super) fn debug_place(&mut self, debug: &DebugFrame) -> (ir::Value, ir::Value, ir::Value) {
        let site = match debug.first {
            Some((first, number)) => {
                let first = self.e.cg.declare_data_in_func(first, self.e.fb.func);
                let first = self.e.fb.ins().symbol_value(POINTER, first);
                let first = self.e.fb.ins().load(types::I64, flags(), first, 0);
                self.e.fb.ins().iadd_imm_s(first, (debug.site - number) as i64)
            }
            None => self.e.cg.site_constant(&mut self.e.fb, debug.site),
        };
        let addresses = self.e.fb.ins().stack_addr(POINTER, debug.addresses, 0);
        let alive = self.e.fb.ins().stack_addr(POINTER, debug.alive, 0);
        (site, addresses, alive)
    }

    /// An error leaves the program's own code where it starts: a `fail`, or
    /// a `?` on what the program's code did not fail with already, as a
    /// call of the standard library or of derived code. The runtime stops there for a
    /// debugger that catches errors, and shows the error.
    pub(super) fn failing(&mut self, error: &Operand, span: Span, called: Option<Span>) {
        let sources = self.e.cg.sources();
        let standard = |span: Span| sources.file_of(span.start).logical.starts_with('<');
        if self.debug.is_none() || standard(span) || called.is_some_and(|called| !standard(called)) {
            return;
        }
        self.probe(span);
        let Some(debug) = self.debug.take() else { return };
        let (site, addresses, alive) = self.debug_place(&debug);
        self.debug = Some(debug);
        let ty = self.operand_ty(error);
        let value = self.operand(error);
        let address = self.in_memory(ty, value);
        let glue = self.e.glue_address(Glue::Show, ty);
        let name = self.e.cg.types().display(ty);
        let name = self.e.string_ptr(&name);
        self.e.call_rt(Rt::DebugFailed, &[site, addresses, alive, glue, address, name]);
    }

    /// A live build's state at a point: each local live there, how it is
    /// kept, and where in the state a frame brings it goes, with the
    /// state's size; the same in the old code that writes it and the new
    /// that reads it, for the runtime to match by the locals' names.
    pub(super) fn point_layout(&mut self, point: u32) -> StateLayout {
        if let Some(layout) = self.e.cg.point_layout(point) {
            return layout;
        }
        let mut items = Vec::new();
        let mut size = 0u32;
        for &local in &self.body.points[point as usize].state {
            let ty = self.body.locals[local.0 as usize];
            let (kept, bytes, align) = match self.storage[local.0 as usize] {
                Storage::Zero => continue,
                Storage::Var(var) => match self.e.cg.class(ty) {
                    Class::Scalar(clif) => (StateKept::Var(var, clif), clif.bytes(), clif.bytes()),
                    _ => (StateKept::Var(var, POINTER), 8, 8),
                },
                Storage::Pointer(var) => (StateKept::Pointer(var), 8, 8),
                Storage::Slot(slot) => {
                    let layout = self.e.cg.layout(ty);
                    (StateKept::Slot(slot), layout.size, layout.align.max(1))
                }
            };
            size = size.next_multiple_of(align);
            items.push((local, kept, size, bytes));
            size += bytes;
        }
        let layout = (items, size);
        self.e.cg.keep_point_layout(point, layout.clone());
        layout
    }

    /// The old code's side of a move: its state at the point, which the
    /// runtime gives back as the new code expects it, or null.
    pub(super) fn transfer(&mut self, point: u32) -> ir::Value {
        let (items, _) = self.point_layout(point);
        // One place for the state of every point: frames stay small, and a
        // deep recursion still meets the stack's guard.
        let slot = match self.state_slot {
            Some(slot) => slot,
            None => {
                let largest = (0..self.body.points.len() as u32).map(|p| self.point_layout(p).1).max().unwrap_or(0);
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, largest.max(8), 4);
                let slot = self.e.fb.create_sized_stack_slot(data);
                self.state_slot = Some(slot);
                slot
            }
        };
        for (_, kept, offset, bytes) in items {
            let address = self.e.fb.ins().stack_addr(POINTER, slot, offset as i32);
            match kept {
                StateKept::Var(var, _) | StateKept::Pointer(var) => {
                    let value = self.e.fb.use_var(var);
                    self.e.fb.ins().store(flags(), value, address, 0);
                }
                StateKept::Slot(kept) => {
                    let source = self.e.fb.ins().stack_addr(POINTER, kept, 0);
                    self.e.copy_memory(address, source, bytes);
                }
            }
        }
        let site = match self.point_sites.get(&point) {
            Some(&site) => site,
            None => self.e.fb.ins().iconst(types::I64, -1),
        };
        let buffer = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        // Where the result goes, when it goes in memory, then each
        // parameter.
        let params = self.params_word_slot();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            let address = self.e.fb.use_var(var);
            self.e.fb.ins().stack_store(POINTER, address, params, 0);
        }
        for (index, &param) in self.body.params.iter().enumerate() {
            let value = match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.e.fb.use_var(var),
                Storage::Slot(slot) => self.e.fb.ins().stack_addr(POINTER, slot, 0),
                Storage::Zero => continue,
            };
            self.e.fb.ins().stack_store(POINTER, value, params, (index as i32 + 1) * 8);
        }
        let params = self.e.fb.ins().stack_addr(POINTER, params, 0);
        self.e.call_rt(Rt::LiveTransfer, &[site, buffer, params]).expect("the new state")
    }

    /// A word for where the result goes, and one for each parameter, as
    /// the call passes them.
    pub(super) fn params_word_slot(&mut self) -> ir::StackSlot {
        if let Some(slot) = self.params_slot {
            return slot;
        }
        let size = (self.body.params.len() as u32 + 1) * 8;
        let slot = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
        self.params_slot = Some(slot);
        slot
    }

    /// The call of the new code a frame moves to, with its parameters as
    /// the runtime gives them, converted, or as they are; what the runtime
    /// converted goes once the call returns.
    pub(super) fn call_osr(
        &mut self,
        id: FnId,
        args: &[Operand],
        state: &Operand,
        result: Option<ir::Value>,
    ) -> Option<ir::Value> {
        let state = self.scalar(state);
        let block = self.e.fb.ins().load(types::I64, flags(), state, 16);
        let slot = self.params_word_slot();
        let params = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let given = self.e.fb.ins().icmp_imm_u(IntCC::NotEqual, block, 0);
        self.osr_params = Some(self.e.fb.ins().select(given, block, params));
        self.osr_arg = Some(state);
        // Stacks show the frame once, in its new code.
        self.e.fb.set_srcloc(ir::SourceLoc::default());
        let value = self.call(Callee::Fn(id), args, result);
        self.e.call_rt(Rt::LiveArgsDone, &[block]);
        value
    }

    /// In a live build, a value with identity the runtime knows of, with
    /// the identity of the type of what it holds: a reload that changes
    /// that type converts it (toolchain 7.10).
    pub(super) fn live_object(&mut self, object: ir::Value, holds: Ty, kind: i64) {
        if !self.e.cg.live() {
            return;
        }
        let identity = self.e.cg.identity(holds);
        let identity = self.e.cg.declare_data_in_func(identity, self.e.fb.func);
        let identity = self.e.fb.ins().symbol_value(POINTER, identity);
        // A `Ref`'s value is released with the glue of its type then, which
        // a reload changes.
        let release = match kind {
            0 => self.e.glue_or_null(Glue::Release, holds),
            _ => self.e.fb.ins().iconst(POINTER, 0),
        };
        let kind = self.e.fb.ins().iconst(types::I8, kind);
        self.e.call_rt(Rt::LiveObject, &[object, identity, kind, release]);
    }

    /// The hidden parameter's value: the state a frame brings.
    pub(super) fn osr_state(&mut self) -> ir::Value {
        let osr = self.body.osr.expect("a live build's function");
        match self.read(&Place::local(osr)) {
            Val::Scalar(value) => value,
            _ => unreachable!("the state is a word"),
        }
    }

    /// The new code's side of a move: the state of the point into the
    /// locals live there, which the frame now owns.
    pub(super) fn osr_load(&mut self, point: u32) {
        let state = self.osr_state();
        let (items, _) = self.point_layout(point);
        for (local, kept, offset, bytes) in items {
            let address = self.e.fb.ins().iadd_imm_s(state, 24 + offset as i64);
            match kept {
                StateKept::Var(var, clif) => {
                    let value = self.e.fb.ins().load(clif, flags(), address, 0);
                    self.define(var, value);
                }
                StateKept::Pointer(var) => {
                    let value = self.e.fb.ins().load(POINTER, flags(), address, 0);
                    self.define(var, value);
                }
                StateKept::Slot(kept) => {
                    let target = self.e.fb.ins().stack_addr(POINTER, kept, 0);
                    self.e.copy_memory(target, address, bytes);
                }
            }
            self.mark(local, true);
        }
        self.e.call_rt(Rt::LiveFree, &[state]);
    }

    pub(super) fn location(&mut self, span: Span) -> ir::Value {
        let location = self.e.cg.sources().location(span);
        self.e.string_ptr(&location)
    }

    // ---- statements and terminators ----------------------------------------
}
