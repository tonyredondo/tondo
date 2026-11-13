//! The program's entry: its identity, constants, `main` and a library's start.

use super::*;

impl Codegen<'_> {
    /// The identity of a type for `std.reflect`: a static holding its name,
    /// whose address tells the type apart.
    pub(super) fn identity(&mut self, ty: Ty) -> DataId {
        if let Some(&id) = self.identities.get(&ty) {
            return id;
        }
        let shown = self.types.display(ty);
        // A patch shares the running program's identity of a type it has,
        // of the same shape.
        let key = self.identity_key(ty);
        if let Some(index) = self.live.as_ref().and_then(|live| live.patch.as_ref()?.0.iter().position(|t| *t == key)) {
            let symbol = format!("tondo_live_data{index}");
            let id = self.module.declare_data(&symbol, Linkage::Import, false, false).expect("type identity");
            self.identities.insert(ty, id);
            return id;
        }
        let name = self.string(&shown);
        let symbol = format!("tondo_type{}", self.identities.len());
        let id = self.module.declare_data(&symbol, Linkage::Local, false, false).expect("type identity");
        let mut data = DataDescription::new();
        data.define(vec![0u8; 8].into_boxed_slice());
        data.set_align(8);
        let name = self.module.declare_data_in_data(name, &mut data);
        data.write_data_addr(0, name, 0);
        self.module.define_data(id, &data).expect("type identity definition");
        self.identities.insert(ty, id);
        id
    }

    /// How a live build's tables name a type's identity: by its name and
    /// the digest of its shape.
    pub(super) fn identity_key(&self, ty: Ty) -> String {
        crate::live::type_key(self.types, ty)
    }

    /// A static array of constants: `[count][length][room][head]`, then
    /// each element's bytes, a string's the address of its literal. Its
    /// count is written to as it is shared, never freed.
    pub(super) fn static_array(&mut self, size: usize, elements: &[Const]) -> DataId {
        if elements.is_empty()
            && let Some(id) = self.empty_array
        {
            return id;
        }
        let mut bytes = Vec::with_capacity(ARRAY_HEADER as usize + size * elements.len());
        let length = elements.len() as u64;
        for word in [SHARED_FOREVER, length, length, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut texts = Vec::new();
        for element in elements {
            let at = bytes.len();
            match element {
                Const::Int(value, _) => bytes.extend_from_slice(&value.to_le_bytes()[..size]),
                Const::Float(bits, _) if size == 4 => {
                    bytes.extend_from_slice(&(f64::from_bits(*bits) as f32).to_bits().to_le_bytes())
                }
                Const::Float(bits, _) => bytes.extend_from_slice(&bits.to_le_bytes()),
                Const::Bool(value) => bytes.push(*value as u8),
                Const::Char(c) => bytes.extend_from_slice(&(*c as u32).to_le_bytes()),
                Const::Str(text) => {
                    texts.push((at, self.string(text)));
                    bytes.extend_from_slice(&[0; 8]);
                }
                Const::Bytes(contents, _) => {
                    texts.push((at, self.bytes(contents)));
                    bytes.extend_from_slice(&[0; 8]);
                }
                Const::Unit => {}
            }
            bytes.resize(at + size, 0);
        }
        let name = format!("tondo_array{}", self.arrays);
        self.arrays += 1;
        let id = self.module.declare_data(&name, Linkage::Local, true, false).expect("array declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        for (at, text) in texts {
            let text = self.module.declare_data_in_data(text, &mut data);
            data.write_data_addr(at as u32, text, 0);
        }
        self.module.define_data(id, &data).expect("array definition");
        if elements.is_empty() {
            self.empty_array = Some(id);
        }
        id
    }

    /// The static `Array[Byte]` of an embedded file's bytes, shared as any
    /// static array is.
    pub(super) fn bytes(&mut self, contents: &std::sync::Arc<[u8]>) -> DataId {
        if let Some(&id) = self.byte_arrays.get(contents) {
            return id;
        }
        let length = contents.len() as u64;
        let mut bytes = Vec::with_capacity(ARRAY_HEADER as usize + contents.len());
        for word in [SHARED_FOREVER, length, length, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(contents);
        let name = format!("tondo_array{}", self.arrays);
        self.arrays += 1;
        let id = self.module.declare_data(&name, Linkage::Local, true, false).expect("array declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        self.module.define_data(id, &data).expect("array definition");
        self.byte_arrays.insert(contents.clone(), id);
        id
    }

    /// A static, immortal string object: `[refcount][length][room][bytes]`.
    pub(super) fn string(&mut self, text: &str) -> DataId {
        if let Some(&id) = self.strings.get(text) {
            return id;
        }
        let mut bytes = Vec::with_capacity(24 + text.len());
        bytes.extend_from_slice(&IMMORTAL.to_le_bytes());
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        let name = format!("tondo_str{}", self.strings.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("string declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        self.module.define_data(id, &data).expect("string definition");
        self.strings.insert(text.to_string(), id);
        id
    }

    /// `tondo_main`, called by the runtime's C `main`.
    /// `tondo_main`, called by the runtime's C `main`. It returns the process
    /// status: 0, or 1 after reporting an error that a fallible `main`
    /// returned (20.10).
    /// What a library runs before the first call from C: it hands the
    /// runtime its tables.
    pub(super) fn library_start(
        &mut self,
        trace: DataId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let signature = self.module.make_signature();
        let id =
            self.module.declare_function("tondo_library", Linkage::Local, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 2);
        let fb = FunctionBuilder::new(&mut context.func, fbc);
        let mut e = Emit::new(self, fb);
        let block = e.fb.create_block();
        e.fb.switch_to_block(block);
        e.check_cpu();
        let trace = e.cg.module.declare_data_in_func(trace, e.fb.func);
        let trace = e.fb.ins().symbol_value(POINTER, trace);
        e.call_rt(Rt::Trace, &[trace]);
        e.register_contexts();
        if let Some(debug) = e.cg.debug {
            let debug = e.cg.module.declare_data_in_func(debug, e.fb.func);
            let debug = e.fb.ins().symbol_value(POINTER, debug);
            e.call_rt(Rt::DebugTable, &[debug]);
        }
        if let Some(dump) = e.cg.dump {
            let dump = e.cg.module.declare_data_in_func(dump, e.fb.func);
            let dump = e.fb.ins().symbol_value(POINTER, dump);
            e.call_rt(Rt::DumpTable, &[dump]);
        }
        if !e.cg.roots.is_empty() {
            let text = e.string_ptr(e.cg.roots);
            e.call_rt(Rt::SourceRoots, &[text]);
        }
        e.fb.ins().return_(&[]);
        e.fb.seal_all_blocks();
        let config = e.cg.module.isa().frontend_config();
        e.fb.finalize(config);
        Ok(id)
    }

    /// The process's `main`, which C's start calls: the runtime runs
    /// `tondo_main` and gives its status.
    pub(super) fn process_main(
        &mut self,
        entry: FuncId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let mut signature = self.module.make_signature();
        signature.params = vec![AbiParam::new(types::I32), AbiParam::new(POINTER)];
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 1);
        let fb = FunctionBuilder::new(&mut context.func, fbc);
        let mut e = Emit::new(self, fb);
        let block = e.fb.create_block();
        e.fb.append_block_params_for_function_params(block);
        e.fb.switch_to_block(block);
        e.check_cpu();
        // A live build's tables, and then a debug build's, before a
        // debugger can ask for them.
        if let Some(info) = e.cg.live.as_ref().map(|live| live.info) {
            let info = e.cg.module.declare_data_in_func(info, e.fb.func);
            let info = e.fb.ins().symbol_value(POINTER, info);
            e.call_rt(Rt::Live, &[info]);
        }
        if let Some(debug) = e.cg.debug {
            let debug = e.cg.module.declare_data_in_func(debug, e.fb.func);
            let debug = e.fb.ins().symbol_value(POINTER, debug);
            e.call_rt(Rt::DebugTable, &[debug]);
        }
        if let Some(dump) = e.cg.dump {
            let dump = e.cg.module.declare_data_in_func(dump, e.fb.func);
            let dump = e.fb.ins().symbol_value(POINTER, dump);
            e.call_rt(Rt::DumpTable, &[dump]);
        }
        if !e.cg.roots.is_empty() {
            let text = e.string_ptr(e.cg.roots);
            e.call_rt(Rt::SourceRoots, &[text]);
        }
        // A program that reads its metrics counts them from the start
        // (standard library 14.25).
        if e.cg.module.get_name("tondo_rt_metrics").is_some() {
            e.call_rt(Rt::MetricsWatch, &[]);
        }
        let entry = e.cg.module.declare_func_in_func(entry, e.fb.func);
        let entry = e.fb.ins().func_addr(POINTER, entry);
        // The arguments, which the runtime keeps: a static executable gives
        // them to nothing else.
        let (argc, argv) = (e.fb.block_params(block)[0], e.fb.block_params(block)[1]);
        let status = e.call_rt(Rt::Main, &[entry, argc, argv]).expect("an exit status");
        e.fb.ins().return_(&[status]);
        e.fb.seal_all_blocks();
        let config = e.cg.module.isa().frontend_config();
        e.fb.finalize(config);
        Ok(id)
    }

    pub(super) fn entry_point(
        &mut self,
        main: usize,
        result: Ty,
        tests: &[crate::hir::TestNodeInfo],
        trace: DataId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let table = (!tests.is_empty()).then(|| self.test_table(tests));
        let mut signature = self.module.make_signature();
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("tondo_main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 0);
        {
            let fb = FunctionBuilder::new(&mut context.func, fbc);
            let mut e = Emit::new(self, fb);
            let block = e.fb.create_block();
            e.fb.switch_to_block(block);
            let trace = e.cg.module.declare_data_in_func(trace, e.fb.func);
            let trace = e.fb.ins().symbol_value(POINTER, trace);
            e.call_rt(Rt::Trace, &[trace]);
            e.register_contexts();
            // A test build hands the runner its tree before `main` runs it.
            if let Some(table) = table {
                let table = e.cg.module.declare_data_in_func(table, e.fb.func);
                let address = e.fb.ins().symbol_value(POINTER, table);
                let count = e.fb.ins().iconst(types::I64, tests.len() as i64);
                e.call_rt(Rt::TestStart, &[address, count]);
            }
            // With coverage, the counters, which the runtime writes out.
            if let Some((counters, counted)) = &e.cg.coverage {
                let (counters, count) = (*counters, counted.len() as i64);
                let counters = e.cg.module.declare_data_in_func(counters, e.fb.func);
                let address = e.fb.ins().symbol_value(POINTER, counters);
                let count = e.fb.ins().iconst(types::I64, count);
                e.call_rt(Rt::CoverageStart, &[address, count]);
            }
            let main = FnId(main as u32);
            let value = match e.cg.class(result) {
                Class::Memory => {
                    let layout = e.cg.layout(result);
                    let data = StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        layout.size,
                        layout.align.trailing_zeros() as u8,
                    );
                    let slot = e.fb.create_sized_stack_slot(data);
                    let address = e.fb.ins().stack_addr(POINTER, slot, 0);
                    call_body(e.cg, &mut e.fb, main, &[address]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = call_body(e.cg, &mut e.fb, main, &[]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    call_body(e.cg, &mut e.fb, main, &[]);
                    Val::Zero
                }
            };
            if let TyKind::Result(_, error) = *e.cg.types.kind(result) {
                let tag = e.tag(result, value);
                let failed = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, ERR as i64);
                let (report, done) = (e.fb.create_block(), e.fb.create_block());
                e.fb.ins().brif(failed, report, &[], done, &[]);
                e.fb.switch_to_block(report);
                let error_value = match value {
                    Val::Memory(address) => {
                        let slot = e.cg.slot(result, Proj::Variant(ERR, 0));
                        let address = e.component_address(address, &slot);
                        e.value_at(address, error)
                    }
                    _ => Val::Zero,
                };
                let description = e.describe(error, error_value, true);
                let ty = e.type_name(error, error_value);
                e.call_rt(Rt::ReportError, &[description, ty]);
                e.call_rt(Rt::StrRelease, &[description]);
                e.refcount(result, value, true);
                let one = e.fb.ins().iconst(types::I32, 1);
                e.fb.ins().return_(&[one]);
                e.fb.switch_to_block(done);
            }
            // `main` returns an owned value; release it before exiting.
            e.refcount(result, value, true);
            let status = e.fb.ins().iconst(types::I32, 0);
            e.fb.ins().return_(&[status]);
            e.fb.seal_all_blocks();
            let config = e.cg.module.isa().frontend_config();
            e.fb.finalize(config);
        }
        Ok(id)
    }
}
