//! Glue: what releases, retains, compares, hashes and shows a value of each type, and thunks.

use super::*;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum Glue {
    Retain,
    Release,
    /// `Retain` and `Release` for the rare way of code that tests a value
    /// first: they keep every register, so that the code around keeps its
    /// values in them, as a loop's.
    RetainCold,
    ReleaseCold,
    Equal,
    /// Appends the `Display` text of a value to a string builder.
    Display,
    /// The same, from the code of a module, which sees the `priv` fields
    /// of its types.
    DisplayFrom(ModuleId),
    /// Hashes a `Key` value.
    Hash,
    /// Runs a member of the test tree, a `fn()` value, and reports the
    /// error it returns.
    Test,
    /// Reports the references a value holds to the cycle collector.
    Visit,
    /// Appends a value as a debugger shows it to a string builder: as its
    /// `debug.View` when its type has one.
    Show,
    /// The same, without the view: the value as it is (`raw()`).
    ShowRaw,
    /// Appends what the tools show for a value of a `@secret` type or
    /// field; `Show` of such a type is this.
    Secret,
}

/// Which glue function serves `kind` for `ty`: one for all that a type's
/// representation decides, and one for what is secret, which a newtype's
/// representation would lose.
pub(super) fn glue_key(types: &Types, kind: Glue, ty: Ty) -> (Glue, Ty) {
    match kind {
        Glue::Show | Glue::ShowRaw if types.secret(ty) => (Glue::Secret, Ty::UNIT),
        _ => (kind, types.repr(ty)),
    }
}

/// Defines a glue function: retain, release or compare values of `ty` given
/// their addresses.
pub(super) fn define_glue(
    cg: &mut Codegen,
    kind: Glue,
    ty: Ty,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit::new(cg, fb);
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let params = e.fb.block_params(entry).to_vec();
    let enum_shape = match e.cg.layout(ty).shape {
        Shape::Enum { payload, variants, .. } => Some((payload, variants.len() as u32)),
        _ => None,
    };
    match (kind, enum_shape) {
        (Glue::RetainCold | Glue::ReleaseCold, _) => {
            let warm = if kind == Glue::ReleaseCold { Glue::Release } else { Glue::Retain };
            e.call_glue(warm, ty, &[params[0]]);
            e.fb.ins().return_(&[]);
        }
        (Glue::Test, _) => {
            let TyKind::Fn(_, result, ..) = *e.cg.types.kind(ty) else { unreachable!("a test member is a `fn()`") };
            let code = e.fb.ins().load(POINTER, flags(), params[0], 16);
            let captures = e.fb.ins().iadd_imm_s(params[0], FN_HEADER);
            let signature = e.cg.uniform_signature(&[], &[], result);
            let signature = e.fb.import_signature(signature);
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
                    e.fb.ins().call_indirect(signature, code, &[address, captures]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = e.fb.ins().call_indirect(signature, code, &[captures]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    e.fb.ins().call_indirect(signature, code, &[captures]);
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
                let description = e.describe(error, error_value, false);
                let error_type = e.cg.types.display(error);
                let error_type = e.string_ptr(&error_type);
                e.call_rt(Rt::TestError, &[description, error_type]);
                e.fb.ins().jump(done, &[]);
                e.fb.switch_to_block(done);
            }
            e.refcount(result, value, true);
            e.fb.ins().return_(&[]);
        }
        (Glue::Display | Glue::DisplayFrom(_), _) => {
            if let Glue::DisplayFrom(module) = kind {
                e.site = Some(module);
            }
            if e.shows_parts(ty) {
                e.append_parts(params[0], ty, params[1]);
            } else {
                let value = e.value_at(params[1], ty);
                e.append_display(params[0], ty, value);
            }
            e.fb.ins().return_(&[]);
        }
        (Glue::Secret, _) => {
            let secret = e.string_ptr(SECRET);
            e.call_rt(Rt::SbStr, &[params[0], secret]);
            e.fb.ins().return_(&[]);
        }
        // A type the program views otherwise shows as its view.
        (Glue::Show, _) if e.cg.views().contains_key(&ty) => {
            let (view, shown) = e.cg.views()[&ty];
            let value = e.value_at(params[1], ty);
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let size = e.cg.layout(shown).size.max(8);
            let slot = e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
            let at = e.fb.ins().stack_addr(POINTER, slot, 0);
            let viewed = match e.cg.class(shown) {
                Class::Memory => {
                    call_body(e.cg, &mut e.fb, view, &[&[at][..], &args].concat());
                    Val::Memory(at)
                }
                Class::Scalar(_) => {
                    let call = call_body(e.cg, &mut e.fb, view, &args);
                    let result = e.fb.inst_results(call)[0];
                    e.fb.ins().store(flags(), result, at, 0);
                    Val::Scalar(result)
                }
                Class::Zero => {
                    call_body(e.cg, &mut e.fb, view, &args);
                    Val::Zero
                }
            };
            e.call_glue(Glue::Show, shown, &[params[0], at]);
            e.refcount(shown, viewed, true);
            e.fb.ins().return_(&[]);
        }
        (Glue::Show | Glue::ShowRaw, _) => {
            let value = e.value_at(params[1], ty);
            e.append_show(params[0], ty, value, params[1]);
            e.fb.ins().return_(&[]);
        }
        (Glue::Hash, None) => {
            let value = e.value_at(params[0], ty);
            let hash = e.hash(ty, value);
            e.fb.ins().return_(&[hash]);
        }
        (Glue::Hash, Some((_, count))) => {
            // The tag, then the active variant's payload.
            let tag = e.tag(ty, Val::Memory(params[0]));
            let tag_word = e.fb.ins().uextend(types::I64, tag);
            let seed = e.fb.ins().iconst(types::I64, 0xcbf2_9ce4_8422_2325u64 as i64);
            let start = e.mix(seed, tag_word);
            for variant in 0..count {
                let components = e.cg.types.variants(ty).unwrap()[variant as usize].len() as u32;
                let (this, next) = (e.fb.create_block(), e.fb.create_block());
                let is_this = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                e.fb.ins().brif(is_this, this, &[], next, &[]);
                e.fb.switch_to_block(this);
                let mut hash = start;
                for index in 0..components {
                    let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                    let part = e.component_address(params[0], &slot);
                    let part = if slot.boxed {
                        e.call_glue(Glue::Hash, slot.ty, &[part]).unwrap()
                    } else {
                        let value = e.value_at(part, slot.ty);
                        e.hash(slot.ty, value)
                    };
                    hash = e.mix(hash, part);
                }
                e.fb.ins().return_(&[hash]);
                e.fb.switch_to_block(next);
            }
            e.fb.ins().return_(&[start]);
        }
        (Glue::Retain | Glue::Release, None) => {
            let value = e.value_at(params[0], ty);
            e.refcount(ty, value, kind == Glue::Release);
            e.fb.ins().return_(&[]);
        }
        (Glue::Visit, None) => {
            let value = e.value_at(params[0], ty);
            e.visit(ty, value, params[1]);
            e.fb.ins().return_(&[]);
        }
        (Glue::Equal, None) => {
            let (a, b) = (e.value_at(params[0], ty), e.value_at(params[1], ty));
            let result = e.equal(ty, a, b);
            e.fb.ins().return_(&[result]);
        }
        (_, Some((_, count))) => {
            // Dispatch on the tag; only the active variant's payload is valid.
            let tag = e.tag(ty, Val::Memory(params[0]));
            if kind == Glue::Equal {
                let other = e.tag(ty, Val::Memory(params[1]));
                let differs = e.fb.ins().icmp(IntCC::NotEqual, tag, other);
                let (different, same) = (e.fb.create_block(), e.fb.create_block());
                e.fb.ins().brif(differs, different, &[], same, &[]);
                e.fb.switch_to_block(different);
                let zero = e.fb.ins().iconst(types::I8, 0);
                e.fb.ins().return_(&[zero]);
                e.fb.switch_to_block(same);
            }
            for variant in 0..count {
                let components = e.cg.types.variants(ty).unwrap()[variant as usize].len() as u32;
                let (this, next) = (e.fb.create_block(), e.fb.create_block());
                let is_this = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                e.fb.ins().brif(is_this, this, &[], next, &[]);
                e.fb.switch_to_block(this);
                if kind == Glue::Equal {
                    let mut result = e.fb.ins().iconst(types::I8, 1);
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        let same = e.equal_slot(params[0], params[1], &slot);
                        result = e.fb.ins().band(result, same);
                    }
                    e.fb.ins().return_(&[result]);
                } else if kind == Glue::Visit {
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        e.visit_slot(params[0], &slot, params[1]);
                    }
                    e.fb.ins().return_(&[]);
                } else {
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        e.refcount_slot(params[0], &slot, kind == Glue::Release);
                    }
                    e.fb.ins().return_(&[]);
                }
                e.fb.switch_to_block(next);
            }
            if kind == Glue::Equal {
                let one = e.fb.ins().iconst(types::I8, 1);
                e.fb.ins().return_(&[one]);
            } else {
                e.fb.ins().return_(&[]);
            }
        }
    }
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}

/// What a live build's closure calls: its body, through the body's slot,
/// with the same arguments.
pub(super) fn define_forwarder(
    cg: &mut Codegen,
    target: FnId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let params = fb.block_params(entry).to_vec();
    let results = call_passing(cg, &mut fb, target, &params);
    fb.ins().return_(&results);
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
}

/// A call that code with no cleanups of its own makes for another, as a
/// thunk does: a `try_call`, whose handler passes a panic the callee unwinds
/// on to the caller's way (15.9), with zeros for the results.
pub(super) fn call_passing(
    cg: &mut Codegen,
    fb: &mut FunctionBuilder,
    target: FnId,
    args: &[ir::Value],
) -> Vec<ir::Value> {
    if cg.live() || cg.takes_osr(target) || cg.slot_of(target).is_some() {
        let call = call_body(cg, fb, target, args);
        return fb.inst_results(call).to_vec();
    }
    let func = cg.function(target);
    let own = fb.func.signature.returns.iter().map(|r| r.value_type).collect::<Vec<_>>();
    call_passing_to(cg, fb, func, args, &own)
}

/// The same of the function `func`, whose results may differ from those of
/// the function that calls it, `own`, which a panic returns zeros of.
fn call_passing_to(
    cg: &mut Codegen,
    fb: &mut FunctionBuilder,
    func: FuncId,
    args: &[ir::Value],
    own: &[ir::Type],
) -> Vec<ir::Value> {
    use cranelift_codegen::ir::{BlockArg, BlockCall, ExceptionTableData, ExceptionTableItem};
    let callee = cg.declare_func_in_func(func, fb.func);
    let signature = fb.func.dfg.ext_funcs[callee].signature;
    let returns: Vec<ir::Type> = fb.func.dfg.signatures[signature].returns.iter().map(|r| r.value_type).collect();
    let (next, handler) = (fb.create_block(), fb.create_block());
    for &ty in &returns {
        fb.append_block_param(next, ty);
    }
    let pool = &mut fb.func.dfg.value_lists;
    let normal = BlockCall::new(next, (0..returns.len() as u32).map(BlockArg::TryCallRet), pool);
    let caught = BlockCall::new(handler, [], pool);
    let table = ExceptionTableData::new(signature, normal, [ExceptionTableItem::Default(caught)]);
    let table = fb.func.dfg.exception_tables.push(table);
    fb.ins().try_call(callee, args, table);
    fb.switch_to_block(handler);
    fb.set_cold_block(handler);
    let pass = cg.runtime(Rt::UnwindPass);
    let pass = cg.declare_func_in_func(pass, fb.func);
    fb.ins().call(pass, &[]);
    let zero = |fb: &mut FunctionBuilder, ty: ir::Type| match ty {
        types::F32 => fb.ins().f32const(0.0),
        types::F64 => fb.ins().f64const(0.0),
        types::I128 => {
            let half = fb.ins().iconst(types::I64, 0);
            fb.ins().iconcat(half, half)
        }
        ty => fb.ins().iconst(ty, 0),
    };
    let zeros: Vec<ir::Value> = own
        .iter()
        .map(|&ty| match ty.is_vector() {
            true => {
                let lane = zero(fb, ty.lane_type());
                fb.ins().splat(ty, lane)
            }
            false => zero(fb, ty),
        })
        .collect();
    fb.ins().return_(&zeros);
    fb.switch_to_block(next);
    fb.block_params(next).to_vec()
}

/// What calls a function with a version (rule 19 of `std.simd`): the
/// version where the processor has its level, which the runtime found at
/// the start, and else the body for the build's level. It is code of the
/// build's level, which a vector wider than 128 bits does not pass through:
/// the bridge converts it, in code of the version's level.
pub(super) fn define_dispatcher(
    cg: &mut Codegen,
    versions: Versions,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let own: Vec<ir::Type> = func.signature.returns.iter().map(|r| r.value_type).collect();
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let params = fb.block_params(entry).to_vec();
    let flag = cg.module.declare_data("tondo_rt_versions", Linkage::Import, true, false).expect("the runtime's flag");
    let flag = cg.module.declare_data_in_func(flag, fb.func);
    let flag = fb.ins().symbol_value(POINTER, flag);
    let on = fb.ins().load(types::I8, flags(), flag, 0);
    let (wide, narrow) = (fb.create_block(), fb.create_block());
    fb.ins().brif(on, wide, &[], narrow, &[]);
    for (block, target) in [(narrow, versions.base), (wide, versions.bridge.unwrap_or(versions.version))] {
        fb.switch_to_block(block);
        let results = call_passing_to(cg, &mut fb, target, &params, &own);
        fb.ins().return_(&results);
    }
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
}

/// What calls a function's version where it passes a vector differently
/// from the build's level: it takes the arguments as the build's level
/// passes them and converts each vector of more than 128 bits, which the
/// version takes in a register and may give back in one.
pub(super) fn define_bridge(
    cg: &mut Codegen,
    body: &Body,
    version: FuncId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let own: Vec<ir::Type> = func.signature.returns.iter().map(|r| r.value_type).collect();
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let mut incoming = fb.block_params(entry).to_vec().into_iter();
    let (mut shapes, config) = {
        let isa = &cg.isas[&(body.needs | VERSION)];
        (Shapes::new(cg.types, &**isa), isa.frontend_config())
    };
    let returned = (cg.class(body.result) == Class::Memory).then(|| incoming.next().expect("the result's place"));
    let mut args = Vec::new();
    if shapes.class(body.result) == Class::Memory {
        args.push(returned.expect("in memory at both levels"));
    }
    for (index, &param) in body.params.iter().enumerate() {
        let ty = body.locals[param.0 as usize];
        let loan = body.loans[index];
        match (cg.class(ty), shapes.class(ty)) {
            (Class::Zero, _) if !loan => {}
            (Class::Memory, Class::Scalar(clif)) if !loan => {
                let address = incoming.next().expect("a parameter");
                args.push(fb.ins().load(clif, flags(), address, 0));
            }
            _ => args.push(incoming.next().expect("a parameter")),
        }
    }
    let results = call_passing_to(cg, &mut fb, version, &args, &own);
    match (returned, results.as_slice()) {
        (Some(address), [value]) => {
            fb.ins().store(flags(), *value, address, 0);
            fb.ins().return_(&[]);
        }
        _ => {
            fb.ins().return_(&results);
        }
    }
    fb.seal_all_blocks();
    fb.finalize(config);
}

/// What the runtime calls to convert a value a frame brings to the new code
/// (toolchain 7.10): the conversion `function`, given the value at the
/// first address, writes what it gives at the second when that is `some`;
/// the answer is whether it was.
pub(super) fn define_conversion(
    cg: &mut Codegen,
    body: &Body,
    function: FnId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit::new(cg, fb);
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let (from, to) = (e.fb.block_params(entry)[0], e.fb.block_params(entry)[1]);
    let option = body.result;
    let TyKind::Option(new) = *e.cg.types.kind(option) else { unreachable!("a conversion gives `T?`") };
    let old = body.locals[body.params[0].0 as usize];
    let mut values = Vec::new();
    let result = (e.cg.class(option) == Class::Memory).then(|| {
        let layout = e.cg.layout(option);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size, layout.align.trailing_zeros() as u8);
        let slot = e.fb.create_sized_stack_slot(data);
        e.fb.ins().stack_addr(POINTER, slot, 0)
    });
    values.extend(result);
    match e.value_at(from, old) {
        Val::Scalar(value) | Val::Memory(value) => values.push(value),
        Val::Zero => {}
    }
    let call = call_body(e.cg, &mut e.fb, function, &values);
    let value = match result {
        Some(address) => Val::Memory(address),
        None => Val::Scalar(e.fb.inst_results(call)[0]),
    };
    let tag = e.tag(option, value);
    let (some, none) = (e.fb.create_block(), e.fb.create_block());
    e.fb.ins().brif(tag, some, &[], none, &[]);
    e.fb.switch_to_block(some);
    match value {
        // `T?` of an address is the address.
        Val::Scalar(address) if e.cg.class(new) != Class::Zero => {
            e.fb.ins().store(flags(), address, to, 0);
        }
        Val::Memory(base) => {
            let slot = e.cg.slot(option, Proj::Variant(SOME, 0));
            let at = e.component_address(base, &slot);
            let converted = e.value_at(at, new);
            e.store_val(to, new, converted);
            // A boxed value is shared with its box, which goes.
            if slot.boxed {
                let copy = e.value_at(to, new);
                e.refcount(new, copy, false);
                e.refcount(option, value, true);
            }
        }
        _ => {}
    }
    let yes = e.fb.ins().iconst(types::I8, 1);
    e.fb.ins().return_(&[yes]);
    e.fb.switch_to_block(none);
    let no = e.fb.ins().iconst(types::I8, 0);
    e.fb.ins().return_(&[no]);
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}

/// A thunk with the uniform convention of `fn(...)` values that ignores the
/// captures' address and calls the named function `target`.
pub(super) fn define_thunk(
    cg: &mut Codegen,
    body: &Body,
    target: FnId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let mut params = fb.block_params(entry).to_vec();
    // Drop the captures' address, which follows the result slot if any.
    let env = usize::from(cg.class(body.result) == Class::Memory);
    params.remove(env);
    let results = call_passing(cg, &mut fb, target, &params);
    fb.ins().return_(&results);
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
}

impl Codegen<'_> {
    /// The static table of glue functions the runtime uses for a map or set.
    pub(super) fn map_glue(&mut self, ty: Ty) -> DataId {
        let ty = self.types.repr(ty);
        if let Some(&id) = self.map_glue.get(&ty) {
            return id;
        }
        let (key, value) = self.map_parts(ty);
        let name = format!("tondo_map_glue{}", self.map_glue.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("glue table");
        let mut data = DataDescription::new();
        // Eight functions, then whether the runtime lets go of a removed
        // key or value as soon as no reader can see it, and the size of an
        // entry and where its value starts, for a map that is still empty.
        let mut bytes = vec![0u8; 88];
        bytes[64] = (self.types.holds_outside(key) || self.types.holds_outside(value)) as u8;
        let (size, offset) = self.entry_layout(ty);
        bytes[72..80].copy_from_slice(&(size as u64).to_le_bytes());
        bytes[80..88].copy_from_slice(&(offset as u64).to_le_bytes());
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        let entries = [
            (Glue::Hash, key, true),
            (Glue::Equal, key, true),
            (Glue::Retain, key, self.types.needs_drop(key)),
            (Glue::Release, key, self.types.needs_drop(key)),
            (Glue::Retain, value, self.types.needs_drop(value)),
            (Glue::Release, value, self.types.needs_drop(value)),
            (Glue::Visit, key, self.types.can_cycle(key)),
            (Glue::Visit, value, self.types.can_cycle(value)),
        ];
        for (index, (kind, ty, present)) in entries.into_iter().enumerate() {
            if present {
                let function = self.glue(kind, ty);
                let reference = self.module.declare_func_in_data(function, &mut data);
                data.write_function_addr(index as u32 * 8, reference);
            }
        }
        self.module.define_data(id, &data).expect("glue table definition");
        self.map_glue.insert(ty, id);
        id
    }

    /// The glue function of `kind` for values of type `ty`, defined later.
    pub(super) fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        let (kind, ty) = glue_key(self.types, kind, ty);
        if let Some(&id) = self.glue.get(&(kind, ty)) {
            return id;
        }
        let signature = self.glue_signature(kind);
        let kind_name = match kind {
            Glue::DisplayFrom(_) => "DisplayFrom".to_string(),
            _ => format!("{kind:?}"),
        };
        let name = format!("tondo_glue_{kind_name}_{}", self.glue.len()).to_lowercase();
        let id = self.module.declare_function(&name, Linkage::Local, &signature).expect("glue declaration");
        self.glue.insert((kind, ty), id);
        self.pending_glue.push((kind, ty, id));
        id
    }
}
