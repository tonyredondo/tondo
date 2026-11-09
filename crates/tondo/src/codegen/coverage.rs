//! What coverage, the fuzzer and the probes of tools count.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// In a debug build, notes whether a binding holds a value: a probe
    /// shows only those that do, and none whose value went away.
    pub(super) fn mark(&mut self, local: Local, holds: bool) {
        let Some(debug) = &self.debug else { return };
        let Some(index) = debug.shown.iter().position(|(l, _)| *l == local) else { return };
        let (alive, offset, bit) = (debug.alive, (index / 64) as i32 * 8, 1i64 << (index % 64));
        let word = self.e.fb.ins().stack_load(POINTER, types::I64, alive, offset);
        let word = if holds { self.e.fb.ins().bor_imm_s(word, bit) } else { self.e.fb.ins().band_imm_s(word, !bit) };
        self.e.fb.ins().stack_store(POINTER, word, alive, offset);
    }

    /// With coverage, one more run of what a counter counts.
    pub(super) fn count(&mut self, counted: Counted) {
        let Some(counters) = self.e.cg.coverage() else { return };
        let index = self.e.cg.counter(counted);
        let counters = match self.counters {
            Some(counters) => counters,
            None => *self.counters.insert(self.e.cg.declare_data_in_func(counters, self.e.fb.func)),
        };
        let base = self.e.fb.ins().symbol_value(POINTER, counters);
        let offset = self.e.cg.counter_offset(&mut self.e.fb, index);
        let address = self.e.fb.ins().iadd(base, offset);
        let count = self.e.fb.ins().load(types::I64, flags(), address, 0);
        let count = self.e.fb.ins().iadd_imm_s(count, 1);
        self.e.fb.ins().store(flags(), count, address, 0);
    }

    /// In a build for `--fuzz`, tells the runtime the operands of a
    /// comparison of integers or texts of the counted code, which the
    /// search takes values from (testing 10.6).
    pub(super) fn compared(&mut self, op: BinOp, left: &Operand, right: &Operand, ty: Ty) {
        if !self.e.cg.fuzzing() || !matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
        {
            return;
        }
        let span = self.body.span;
        if span == Span::default() || !self.counts(self.e.cg.sources().file_of(span.start)) {
            return;
        }
        let types = self.e.cg.types();
        let rt = match types.kind(types.repr(ty)) {
            TyKind::String => Rt::FuzzCompareText,
            TyKind::Int(_) if types.as_int(ty).map(IntTy::bits) == Some(64) => Rt::FuzzCompare,
            _ => return,
        };
        if let (Val::Scalar(a), Val::Scalar(b)) = (self.operand(left), self.operand(right)) {
            self.e.call_rt(rt, &[a, b]);
        }
    }

    /// Whether coverage counts a file: the project's own code, and for the
    /// search of `--fuzz` what it searches.
    pub(super) fn counts(&self, file: &crate::source::SourceFile) -> bool {
        file.own || ((self.e.cg.fuzzing() || std_counted()) && file.searched())
    }

    /// With coverage, a run of the line a statement of the project's code
    /// starts on, once for statements one after another on a line.
    pub(super) fn count_line(&mut self, span: Span) {
        let file = self.e.cg.sources().file_of(span.start);
        let (line, _) = file.line_col(span.start);
        if !self.counts(file) || self.counted == Some((file.base, line)) {
            return;
        }
        self.counted = Some((file.base, line));
        self.count(Counted::Line { file: file.logical.clone(), line });
    }

    /// With coverage, a call of a function of the project's code.
    pub(super) fn count_call(&mut self) {
        let span = self.body.span;
        if self.e.cg.coverage().is_none() || span == Span::default() {
            return;
        }
        let file = self.e.cg.sources().file_of(span.start);
        if self.counts(file) {
            let (line, _) = file.line_col(span.start);
            let name = self.body.name.clone();
            self.count(Counted::Function { file: file.logical.clone(), line, name });
        }
    }

    /// The statements that start at this statement of the block: a debug
    /// build's probe, and with coverage, their lines and the ways of
    /// branches that start there.
    pub(super) fn count_ways(&mut self, block: usize, at: usize) {
        for span in self.starts.remove(&(block, at)).unwrap_or_default() {
            self.probe(span);
            if self.e.cg.coverage().is_some() {
                self.count_line(span);
            }
        }
        let Some(ways) = self.ways.remove(&(block, at)) else { return };
        for n in ways {
            let way = &self.body.ways[n];
            let file = self.e.cg.sources().file_of(way.span.start);
            if !self.counts(file) || way.span == Span::default() {
                continue;
            }
            let (line, column) = file.line_col(way.span.start);
            let branch = file.line_col(way.branch.start);
            let label = match way.label {
                // An arm is called by its pattern.
                "arm" => file.slice(way.span).lines().next().unwrap_or_default().to_string(),
                label => label.to_string(),
            };
            self.count(Counted::Way { file: file.logical.clone(), line, column, branch, label });
        }
    }

    /// In a debug build, a probe where a line starts: the debugger stops
    /// there when it is asked to, and reads the bindings.
    pub(super) fn probe(&mut self, span: Span) {
        if self.debug.is_none() {
            self.latent_site(span);
        }
        let Some(mut debug) = self.debug.take() else { return };
        let (file, line, _) = self.e.cg.sources().position(span.start);
        if span != Span::default() && debug.line.as_ref() != Some(&(file.clone(), line)) {
            let goes_on = debug.before.take().is_some_and(|before| before == (file.clone(), line));
            debug.line = Some((file, line));
            // The line's code starts here, which the DWARF says.
            let start = self.site(span.start, None);
            self.e.fb.set_srcloc(start);
            for (index, &(local, copy)) in debug.shown.iter().enumerate() {
                let address = match (self.storage[local.0 as usize], copy) {
                    (Storage::Var(var), Some(slot)) => {
                        let value = self.e.fb.use_var(var);
                        self.e.fb.ins().stack_store(POINTER, value, slot, 0);
                        self.e.fb.ins().stack_addr(POINTER, slot, 0)
                    }
                    (Storage::Slot(slot), _) => self.e.fb.ins().stack_addr(POINTER, slot, 0),
                    (Storage::Pointer(var), _) => self.e.fb.use_var(var),
                    _ => self.e.fb.ins().iconst(POINTER, 0),
                };
                self.e.fb.ins().stack_store(POINTER, address, debug.addresses, index as i32 * 8);
            }
            if !debug.captured.is_empty() {
                let env = match self.storage[self.body.params[0].0 as usize] {
                    Storage::Pointer(var) => Some(self.e.fb.use_var(var)),
                    Storage::Slot(slot) => Some(self.e.fb.ins().stack_addr(POINTER, slot, 0)),
                    _ => None,
                };
                for (at, &(offset, boxed)) in debug.captured.iter().enumerate() {
                    let address = match env {
                        Some(env) => {
                            let field = self.e.fb.ins().iadd_imm_s(env, i64::from(offset));
                            if boxed { self.e.fb.ins().load(POINTER, flags(), field, 0) } else { field }
                        }
                        None => self.e.fb.ins().iconst(POINTER, 0),
                    };
                    let index = (debug.shown.len() + at) as i32;
                    self.e.fb.ins().stack_store(POINTER, address, debug.addresses, index * 8);
                }
            }
            let index = self.e.cg.debug_site(span.start, debug.function, goes_on);
            debug.site = index;
            let (site, addresses, alive) = self.debug_place(&debug);
            self.probed = self.e.call_rt(Rt::DebugLine, &[site, addresses, alive]);
            // A binding the debugger changed comes back from its copy.
            for &(local, copy) in &debug.shown {
                if let (Storage::Var(var), Some(slot)) = (self.storage[local.0 as usize], copy)
                    && let Class::Scalar(ty) = self.e.cg.class(self.body.locals[local.0 as usize])
                {
                    let value = self.e.fb.ins().stack_load(POINTER, ty, slot, 0);
                    self.define(var, value);
                }
            }
            self.site_now = Some((site, self.e.cg.site_base() + index as u32));
        }
        self.debug = Some(debug);
    }
}
