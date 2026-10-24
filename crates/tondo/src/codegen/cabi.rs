//! The C calling convention of the target (FFI): how calls of `extern fn`
//! pass their arguments and receive their results, and how the functions C
//! calls back receive and return them.
//!
//! - x86-64 (System V): a struct of up to 16 bytes goes in one register per
//!   eightbyte, integer unless all it holds are floats; a larger or
//!   misaligned one is copied to the stack, as is one that no longer fits
//!   in the registers left. A call with `...` goes through the runtime,
//!   which tells the callee in `al` how many vector registers it uses.
//! - arm64 (AAPCS64): a struct of one to four floats of one type goes in
//!   vector registers, one per member; another of up to 16 bytes in integer
//!   registers, one per eightbyte; a larger one by the address of a copy. A
//!   struct that no longer fits goes on the stack and its class of
//!   registers is used up. Apple passes the arguments of `...` on the stack.
//!
//! Integers narrower than 32 bits are extended to 32 bits both ways, as C
//! compilers expect.

use super::*;
use cranelift_codegen::isa::CallConv;

#[derive(Copy, Clone, PartialEq)]
pub(super) enum Abi {
    X64,
    Arm64 { apple: bool },
}

/// What fills one parameter of a C signature.
#[derive(Copy, Clone)]
enum Source {
    /// The scalar argument at the index.
    Scalar(usize),
    /// A part of the struct argument at the index, at the offset.
    Part(usize, u32),
    /// The address of a copy of the struct argument at the index.
    Address(usize),
    /// Where the struct result goes.
    Result,
    /// A register used up, so that the following values go on the stack.
    Filler,
}

/// How a C function's result comes back.
enum Returned {
    Nothing,
    Scalar(AbiParam),
    /// A struct in registers: each part's type and offset.
    Parts(Vec<(ir::Type, u32)>),
    /// A struct written where the `Source::Result` parameter points.
    Memory,
}

/// The parameters of a C signature with what fills each, and its result.
struct Plan {
    params: Vec<(AbiParam, Source)>,
    returned: Returned,
}

impl Plan {
    fn signature(&self, cg: &Codegen) -> ir::Signature {
        let mut signature = cg.module.make_signature();
        signature.params = self.params.iter().map(|(abi, _)| *abi).collect();
        signature.returns = match &self.returned {
            Returned::Scalar(abi) => vec![*abi],
            Returned::Parts(parts) => parts.iter().map(|&(ty, _)| AbiParam::new(ty)).collect(),
            Returned::Nothing | Returned::Memory => Vec::new(),
        };
        signature
    }
}

/// A function C calls: `adapter` has C's convention and stores its
/// arguments in a frame, and `inner` calls `target` with them.
pub(super) struct Callback {
    pub(super) target: FnId,
    ty: Ty,
    closure: bool,
    pub(super) adapter: FuncId,
    pub(super) inner: FuncId,
}

/// What a call of C calls: a declared symbol, or an `extern fn(...)` value.
pub(super) enum CTarget {
    Symbol(String),
    Address(ir::Value),
}

/// A value fills `ty` (`I32` for a narrow integer), extended as `abi` says.
fn widen(fb: &mut FunctionBuilder, value: ir::Value, abi: AbiParam) -> ir::Value {
    let from = fb.func.dfg.value_type(value);
    match abi.extension {
        _ if from == abi.value_type || !from.is_int() => value,
        ir::ArgumentExtension::Sext => fb.ins().sextend(abi.value_type, value),
        _ => fb.ins().uextend(abi.value_type, value),
    }
}

/// A zero of a register's type, for a filler.
fn zero(fb: &mut FunctionBuilder, ty: ir::Type) -> ir::Value {
    if ty.is_float() { fb.ins().f64const(0.0) } else { fb.ins().iconst(ty, 0) }
}

/// A stack slot for a struct's parts, padded so that whole eightbytes can be
/// loaded.
fn parts_slot(fb: &mut FunctionBuilder, size: u32) -> ir::Value {
    let size = size.next_multiple_of(8).max(16);
    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 4);
    let slot = fb.create_sized_stack_slot(data);
    fb.ins().stack_addr(POINTER, slot, 0)
}

impl Codegen<'_> {
    pub(super) fn abi(&self) -> Abi {
        match (self.module.isa().name(), self.module.isa().default_call_conv()) {
            ("aarch64", call_conv) => Abi::Arm64 { apple: call_conv == CallConv::AppleAarch64 },
            _ => Abi::X64,
        }
    }

    /// How C passes a scalar of `ty`: narrow integers as extended `int`s.
    fn c_scalar(&mut self, ty: Ty) -> AbiParam {
        let clif = self.scalar_type(ty);
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Int(int) if int.bits() < 32 && int.signed() => AbiParam::new(types::I32).sext(),
            _ if clif.is_int() && clif.bits() < 32 => AbiParam::new(types::I32).uext(),
            _ => AbiParam::new(clif),
        }
    }

    /// The registers a struct of `ty` passes in, each part's type and
    /// offset, or `None` when it passes in memory.
    fn c_struct(&mut self, ty: Ty, abi: Abi) -> Option<Vec<(ir::Type, u32)>> {
        let size = self.layout(ty).size;
        let mut leaves = Vec::new();
        let aligned = self.c_leaves(ty, 0, &mut leaves);
        let chunks = |float: &dyn Fn(u32) -> bool| {
            (0..size.div_ceil(8)).map(|k| (if float(k) { types::F64 } else { types::I64 }, k * 8)).collect()
        };
        match abi {
            Abi::X64 if size > 16 || !aligned => None,
            Abi::X64 => {
                let float = |k: u32| leaves.iter().filter(|(offset, _)| offset / 8 == k).all(|(_, t)| t.is_float());
                Some(chunks(&float))
            }
            Abi::Arm64 { .. } => {
                // One to four floats of one type: an HFA. The fields of a
                // union overlap, and count once.
                leaves.sort_by_key(|&(offset, _)| offset);
                leaves.dedup();
                let first = leaves.first().map(|&(_, t)| t);
                if first.is_some_and(|t| t.is_float())
                    && leaves.len() <= 4
                    && leaves.iter().all(|&(_, t)| Some(t) == first)
                {
                    return Some(leaves.into_iter().map(|(offset, t)| (t, offset)).collect());
                }
                (size <= 16).then(|| chunks(&|_| false))
            }
        }
    }

    /// The scalars a C value holds and where, or false when one is not at
    /// its natural alignment, as in a packed struct.
    fn c_leaves(&mut self, ty: Ty, base: u32, out: &mut Vec<(u32, ir::Type)>) -> bool {
        let repr = self.types.repr(ty);
        // Every leaf is collected, aligned or not.
        let mut aligned = true;
        if let TyKind::CArray(element, count) = *self.types.kind(repr) {
            let size = self.layout(element).size;
            for i in 0..count {
                aligned &= self.c_leaves(element, base + i * size, out);
            }
            return aligned;
        }
        if let Some(components) = self.types.components(repr) {
            let Shape::Struct(fields) = self.layout(repr).shape else { unreachable!("a struct") };
            for (&component, &offset) in components.iter().zip(&fields.offsets) {
                aligned &= offset % self.layout(component).align == 0;
                aligned &= self.c_leaves(component, base + offset, out);
            }
            return aligned;
        }
        out.push((base, self.scalar_type(ty)));
        true
    }

    /// How a call passes values of `args`, the first `fixed` for the
    /// declared parameters and the rest for `...` (already promoted), and
    /// receives `result`.
    fn c_plan(&mut self, args: &[Ty], fixed: usize, result: Ty) -> Plan {
        let abi = self.abi();
        // The registers left of each class.
        let (mut ints, mut floats) = (if abi == Abi::X64 { 6 } else { 8 }, 8usize);
        let mut params = Vec::new();
        let fill = |params: &mut Vec<(AbiParam, Source)>, left: &mut usize, ty: ir::Type| {
            params.extend((0..*left).map(|_| (AbiParam::new(ty), Source::Filler)));
            *left = 0;
        };
        let returned = match self.class(result) {
            Class::Zero => Returned::Nothing,
            Class::Scalar(_) => Returned::Scalar(self.c_scalar(result)),
            Class::Memory => match self.c_struct(result, abi) {
                Some(parts) => Returned::Parts(parts),
                None => {
                    params.push((AbiParam::special(POINTER, ArgumentPurpose::StructReturn), Source::Result));
                    // On arm64 the address goes in `x8`, not an argument register.
                    if abi == Abi::X64 {
                        ints -= 1;
                    }
                    Returned::Memory
                }
            },
        };
        for (index, &ty) in args.iter().enumerate() {
            if index == fixed && abi == (Abi::Arm64 { apple: true }) {
                // Apple passes the arguments of `...` on the stack.
                fill(&mut params, &mut ints, types::I64);
                fill(&mut params, &mut floats, types::F64);
            }
            if self.class(ty) != Class::Memory {
                let mut scalar = self.c_scalar(ty);
                // Each argument of Apple's `...` fills an 8-byte slot.
                if index >= fixed && abi == (Abi::Arm64 { apple: true }) && scalar.value_type.bits() < 64 {
                    let signed = matches!(self.types.kind(self.types.repr(ty)), TyKind::Int(int) if int.signed());
                    scalar = if signed { AbiParam::new(types::I64).sext() } else { AbiParam::new(types::I64).uext() };
                }
                let left = if scalar.value_type.is_float() { &mut floats } else { &mut ints };
                *left = left.saturating_sub(1);
                params.push((scalar, Source::Scalar(index)));
                continue;
            }
            let size = self.layout(ty).size;
            let Some(parts) = self.c_struct(ty, abi) else {
                if abi == Abi::X64 {
                    params.push((
                        AbiParam::special(POINTER, ArgumentPurpose::StructArgument(size)),
                        Source::Address(index),
                    ));
                } else {
                    ints = ints.saturating_sub(1);
                    params.push((AbiParam::new(POINTER), Source::Address(index)));
                }
                continue;
            };
            let integer = parts.iter().filter(|(t, _)| t.is_int()).count();
            if integer <= ints && parts.len() - integer <= floats {
                ints -= integer;
                floats -= parts.len() - integer;
                params.extend(parts.into_iter().map(|(t, offset)| (AbiParam::new(t), Source::Part(index, offset))));
            } else if abi == Abi::X64 {
                params
                    .push((AbiParam::special(POINTER, ArgumentPurpose::StructArgument(size)), Source::Address(index)));
            } else {
                // On the stack, in eightbytes of the class whose registers
                // it uses up, so later arguments of the other class still
                // take registers.
                let float = integer == 0;
                let chunk = if float { types::F64 } else { types::I64 };
                if float {
                    fill(&mut params, &mut floats, types::F64);
                } else {
                    fill(&mut params, &mut ints, types::I64);
                }
                params.extend((0..size.div_ceil(8)).map(|k| (AbiParam::new(chunk), Source::Part(index, k * 8))));
            }
        }
        Plan { params, returned }
    }

    /// The signature of a C function of these parameter and result types.
    pub(super) fn c_signature(&mut self, params: &[Ty], result: Ty) -> ir::Signature {
        let plan = self.c_plan(params, params.len(), result);
        plan.signature(self)
    }

    /// The import of a C function by symbol. A symbol imported again with
    /// another signature is called through its address.
    pub(super) fn c_function(&mut self, symbol: &str, signature: &ir::Signature) -> (FuncId, bool) {
        if let Some((id, declared)) = self.c_functions.get(symbol) {
            return (*id, declared == signature);
        }
        let id = self.module.declare_function(symbol, Linkage::Import, signature).expect("C function declaration");
        self.c_functions.insert(symbol.to_string(), (id, signature.clone()));
        (id, true)
    }

    /// The C function that runs `target` when C calls it as `ty`.
    pub(super) fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId {
        if let Some(&id) = self.callbacks.get(&(target, ty)) {
            return id;
        }
        let name = format!("tondo_callback{}", self.callbacks.len());
        self.declare_callback(target, ty, closure, &name, Linkage::Local, &format!("{name}_body"))
            .expect("callback declaration")
    }

    /// The C function `symbol` of an `extern fn` with a body, the same
    /// one where the program passes the function to C.
    pub(super) fn export(&mut self, index: usize, export: &crate::hir::Export) -> Result<(), String> {
        let body = format!("tondo_export{index}_body");
        self.declare_callback(export.function, export.ty, false, &export.symbol, Linkage::Export, &body).map(|_| ())
    }

    fn declare_callback(
        &mut self,
        target: FnId,
        ty: Ty,
        closure: bool,
        name: &str,
        linkage: Linkage,
        body: &str,
    ) -> Result<FuncId, String> {
        let TyKind::CFn(params, result, _) = self.types.kind(ty).clone() else { unreachable!("a C function type") };
        let signature = self.c_signature(&params, result);
        let adapter = self.module.declare_function(name, linkage, &signature).map_err(|e| e.to_string())?;
        let mut frame = self.module.make_signature();
        frame.params.push(AbiParam::new(POINTER));
        let inner = self.module.declare_function(body, Linkage::Local, &frame).map_err(|e| e.to_string())?;
        self.callbacks.insert((target, ty), adapter);
        self.pending_callbacks.push(Callback { target, ty, closure, adapter, inner });
        Ok(adapter)
    }

    /// Where the result, then each argument, of a callback of `ty` lives in
    /// its frame, and the frame's size.
    fn callback_frame(&mut self, ty: Ty) -> (Vec<u32>, u32) {
        let TyKind::CFn(params, result, _) = self.types.kind(ty).clone() else { unreachable!("a C function type") };
        let mut offsets = Vec::new();
        let mut size = 0u32;
        for part in std::iter::once(result).chain(params) {
            let layout = self.layout(part);
            let offset = size.next_multiple_of(layout.align.max(1));
            offsets.push(offset);
            size = offset + layout.size;
        }
        (offsets, size.max(8))
    }
}

/// An argument of a call of C: a scalar, or the address of a struct.
#[derive(Copy, Clone)]
enum Argument {
    Scalar(ir::Value),
    Memory(ir::Value),
}

impl FnCodegen<'_, '_> {
    /// A call of C (FFI): arguments as C receives them, a `String` as a
    /// copy that ends with a zero byte, and those of C's `...` promoted as
    /// C promotes them. The worker is marked as in C meanwhile.
    pub(super) fn c_call(
        &mut self,
        target: CTarget,
        params: &[Ty],
        result_ty: Ty,
        args: &[Operand],
        result: Option<ir::Value>,
    ) -> Option<ir::Value> {
        let mut values = Vec::new();
        let mut types = Vec::new();
        let mut copies = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            let mut ty = self.operand_ty(arg);
            let value = match self.operand(arg) {
                Val::Scalar(text) if self.e.cg.types.repr(ty) == Ty::STRING => {
                    let location = self.location(self.span);
                    let copy = self.e.call_rt(Rt::CString, &[text, location]).unwrap();
                    copies.push(copy);
                    Argument::Scalar(copy)
                }
                // C's `...` takes a `float` as a `double`.
                Val::Scalar(value) if index >= params.len() && ty == Ty::F32 => {
                    ty = Ty::F64;
                    Argument::Scalar(self.e.fb.ins().fpromote(types::F64, value))
                }
                Val::Scalar(value) => Argument::Scalar(value),
                Val::Memory(address) => Argument::Memory(address),
                Val::Zero => unreachable!("C takes no zero-sized values"),
            };
            values.push(value);
            types.push(ty);
        }
        let plan = self.e.cg.c_plan(&types, params.len(), result_ty);
        let mut padded: HashMap<usize, ir::Value> = HashMap::new();
        let mut call_values = Vec::new();
        for &(abi, source) in &plan.params {
            let value = match source {
                Source::Scalar(index) => {
                    let Argument::Scalar(value) = values[index] else { unreachable!("a scalar") };
                    widen(&mut self.e.fb, value, abi)
                }
                Source::Part(index, offset) => {
                    let Argument::Memory(address) = values[index] else { unreachable!("a struct") };
                    let size = self.e.cg.layout(types[index]).size;
                    let parts = *padded.entry(index).or_insert_with(|| {
                        let parts = parts_slot(&mut self.e.fb, size);
                        let zero = self.e.fb.ins().iconst(types::I64, 0);
                        for word in 0..size.div_ceil(8).max(2) {
                            self.e.fb.ins().store(flags(), zero, parts, word as i32 * 8);
                        }
                        self.e.copy_memory(parts, address, size);
                        parts
                    });
                    self.e.fb.ins().load(abi.value_type, flags(), parts, offset as i32)
                }
                Source::Address(index) => {
                    // A copy, which C may change.
                    let Argument::Memory(address) = values[index] else { unreachable!("a struct") };
                    let layout = self.e.cg.layout(types[index]);
                    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size, 4);
                    let slot = self.e.fb.create_sized_stack_slot(data);
                    let copy = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                    self.e.copy_memory(copy, address, layout.size);
                    copy
                }
                Source::Result => result.expect("a struct result has a place"),
                Source::Filler => zero(&mut self.e.fb, abi.value_type),
            };
            call_values.push(value);
        }
        let variadic = args.len() > params.len()
            || matches!(&target, CTarget::Symbol(symbol) if self.e.cg.externs.iter().any(|e| &e.symbol == symbol && e.variadic));
        let signature = plan.signature(self.e.cg);
        self.e.call_rt(Rt::ForeignEnter, &[]);
        let results = if variadic && self.e.cg.abi() == Abi::X64 {
            self.c_variadic(target, &call_values, &signature)
        } else {
            let call = match target {
                CTarget::Symbol(symbol) => {
                    let (id, direct) = self.e.cg.c_function(&symbol, &signature);
                    let func = self.e.cg.module.declare_func_in_func(id, self.e.fb.func);
                    if direct {
                        self.e.fb.ins().call(func, &call_values)
                    } else {
                        let code = self.e.fb.ins().func_addr(POINTER, func);
                        let signature = self.e.fb.import_signature(signature);
                        self.e.fb.ins().call_indirect(signature, code, &call_values)
                    }
                }
                CTarget::Address(code) => {
                    let signature = self.e.fb.import_signature(signature);
                    self.e.fb.ins().call_indirect(signature, code, &call_values)
                }
            };
            self.e.fb.inst_results(call).to_vec()
        };
        self.e.call_rt(Rt::ForeignLeave, &[]);
        for copy in copies {
            self.e.call_rt(Rt::CStringFree, &[copy]);
        }
        match plan.returned {
            Returned::Scalar(_) => {
                let clif = self.e.cg.scalar_type(result_ty);
                let value = results[0];
                let narrow = self.e.fb.func.dfg.value_type(value) != clif;
                Some(if narrow { self.e.fb.ins().ireduce(clif, value) } else { value })
            }
            // A struct in registers goes where the result belongs.
            Returned::Parts(parts) => {
                let size = self.e.cg.layout(result_ty).size;
                let slot = parts_slot(&mut self.e.fb, size);
                for (&value, &(_, offset)) in results.iter().zip(&parts) {
                    self.e.fb.ins().store(flags(), value, slot, offset as i32);
                }
                self.e.copy_memory(result.expect("a struct result has a place"), slot, size);
                None
            }
            Returned::Memory | Returned::Nothing => None,
        }
    }

    /// A call of a C function with `...` on x86-64, through the runtime,
    /// which loads the registers from a frame and sets `al`, which compiled
    /// code cannot.
    fn c_variadic(&mut self, target: CTarget, values: &[ir::Value], signature: &ir::Signature) -> Vec<ir::Value> {
        let code = match target {
            CTarget::Address(code) => code,
            CTarget::Symbol(symbol) => {
                let (id, _) = self.e.cg.c_function(&symbol, signature);
                let func = self.e.cg.module.declare_func_in_func(id, self.e.fb.func);
                self.e.fb.ins().func_addr(POINTER, func)
            }
        };
        // 6 integer registers, 8 vector registers, the counts of stack words
        // and vector registers, then the stack words.
        let (mut integers, mut vectors, mut stack) = (0, 0, Vec::new());
        let mut placed = Vec::new();
        for (&value, param) in values.iter().zip(&signature.params) {
            let float = param.value_type.is_float();
            let offset = match (float, integers, vectors) {
                (false, i, _) if i < 6 => {
                    integers += 1;
                    i * 8
                }
                (true, _, v) if v < 8 => {
                    vectors += 1;
                    48 + v * 8
                }
                _ => {
                    stack.push(value);
                    continue;
                }
            };
            placed.push((offset, value));
        }
        let size = 128 + 8 * stack.len() as u32;
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let frame = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let store = |e: &mut Emit, value: ir::Value, offset: u32| {
            let ty = e.fb.func.dfg.value_type(value);
            let value = if ty.is_int() && ty.bits() < 64 { e.fb.ins().uextend(types::I64, value) } else { value };
            e.fb.ins().store(flags(), value, frame, offset as i32);
        };
        for (offset, value) in placed {
            store(&mut self.e, value, offset);
        }
        for (index, &value) in stack.iter().enumerate() {
            store(&mut self.e, value, 128 + 8 * index as u32);
        }
        let count = self.e.fb.ins().iconst(types::I64, stack.len() as i64);
        self.e.fb.ins().store(flags(), count, frame, 112);
        let count = self.e.fb.ins().iconst(types::I64, vectors as i64);
        self.e.fb.ins().store(flags(), count, frame, 120);
        self.e.call_rt(Rt::CVariadic, &[code, frame]);
        signature
            .returns
            .iter()
            .map(|r| {
                let offset = if r.value_type.is_float() { 48 } else { 0 };
                self.e.fb.ins().load(r.value_type, flags(), frame, offset)
            })
            .collect()
    }
}

/// The C side of a callback: stores the arguments C passes in a frame,
/// in Tondo's representation, has the runtime run the body on them (on
/// this thread inside a task, or as a task when a thread of C calls), and
/// returns the result as C expects it.
pub(super) fn define_adapter(
    cg: &mut Codegen,
    callback: &Callback,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let TyKind::CFn(params, result, _) = cg.types.kind(callback.ty).clone() else { unreachable!() };
    let (offsets, size) = cg.callback_frame(callback.ty);
    let plan = cg.c_plan(&params, params.len(), result);
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit { cg, fb };
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let incoming = e.fb.block_params(entry).to_vec();
    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 4);
    let slot = e.fb.create_sized_stack_slot(data);
    let frame = e.fb.ins().stack_addr(POINTER, slot, 0);
    let mut padded: Vec<(usize, ir::Value)> = Vec::new();
    let mut sret = None;
    for (&value, &(_, source)) in incoming.iter().zip(&plan.params) {
        match source {
            Source::Scalar(index) => {
                let at = e.offset(frame, offsets[index + 1]);
                let clif = e.cg.scalar_type(params[index]);
                let value =
                    if e.fb.func.dfg.value_type(value) != clif { e.fb.ins().ireduce(clif, value) } else { value };
                e.fb.ins().store(flags(), value, at, 0);
            }
            Source::Part(index, offset) => {
                let parts = match padded.iter().find(|(i, _)| *i == index) {
                    Some(&(_, parts)) => parts,
                    None => {
                        let size = e.cg.layout(params[index]).size;
                        let parts = parts_slot(&mut e.fb, size);
                        padded.push((index, parts));
                        parts
                    }
                };
                e.fb.ins().store(flags(), value, parts, offset as i32);
            }
            Source::Address(index) => {
                let at = e.offset(frame, offsets[index + 1]);
                let size = e.cg.layout(params[index]).size;
                e.copy_memory(at, value, size);
            }
            Source::Result => sret = Some(value),
            Source::Filler => {}
        }
    }
    for (index, parts) in padded {
        let at = e.offset(frame, offsets[index + 1]);
        let size = e.cg.layout(params[index]).size;
        e.copy_memory(at, parts, size);
    }
    let inner = e.cg.module.declare_func_in_func(callback.inner, e.fb.func);
    let inner = e.fb.ins().func_addr(POINTER, inner);
    // A library starts at the first call from C.
    let start = match e.cg.library {
        Some(start) => {
            let start = e.cg.module.declare_func_in_func(start, e.fb.func);
            e.fb.ins().func_addr(POINTER, start)
        }
        None => e.fb.ins().iconst(POINTER, 0),
    };
    e.call_rt(Rt::Callback, &[inner, frame, start]);
    let at = e.offset(frame, offsets[0]);
    match plan.returned {
        Returned::Scalar(abi) => {
            let clif = e.cg.scalar_type(result);
            let value = e.fb.ins().load(clif, flags(), at, 0);
            let value = widen(&mut e.fb, value, abi);
            e.fb.ins().return_(&[value]);
        }
        Returned::Parts(parts) => {
            let size = e.cg.layout(result).size;
            let slot = parts_slot(&mut e.fb, size);
            e.copy_memory(slot, at, size);
            let values: Vec<ir::Value> =
                parts.iter().map(|&(ty, offset)| e.fb.ins().load(ty, flags(), slot, offset as i32)).collect();
            e.fb.ins().return_(&values);
        }
        Returned::Memory => {
            let size = e.cg.layout(result).size;
            e.copy_memory(sret.expect("a struct returned in memory"), at, size);
            e.fb.ins().return_(&[]);
        }
        Returned::Nothing => {
            e.fb.ins().return_(&[]);
        }
    }
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}

/// The Tondo side of a callback: calls the function with the arguments
/// of the frame and stores its result there.
pub(super) fn define_inner(
    cg: &mut Codegen,
    body: &Body,
    callback: &Callback,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let TyKind::CFn(params, result, _) = cg.types.kind(callback.ty).clone() else { unreachable!() };
    let (offsets, _) = cg.callback_frame(callback.ty);
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit { cg, fb };
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let frame = e.fb.block_params(entry)[0];
    let mut values = Vec::new();
    let result_at = e.offset(frame, offsets[0]);
    if e.cg.class(result) == Class::Memory {
        values.push(result_at);
    }
    // A closure's body takes the closure first, which holds nothing here.
    if callback.closure {
        let env = body.locals[body.params[0].0 as usize];
        if body.loans[0] || e.cg.class(env) != Class::Zero {
            values.push(e.fb.ins().iconst(POINTER, 0));
        }
    }
    for (&param, &offset) in params.iter().zip(&offsets[1..]) {
        let at = e.offset(frame, offset);
        match e.value_at(at, param) {
            Val::Scalar(value) | Val::Memory(value) => values.push(value),
            Val::Zero => {}
        }
    }
    let callee = e.cg.module.declare_func_in_func(e.cg.functions[callback.target.0 as usize], e.fb.func);
    let call = e.fb.ins().call(callee, &values);
    if let Class::Scalar(_) = e.cg.class(result) {
        let value = e.fb.inst_results(call)[0];
        e.fb.ins().store(flags(), value, result_at, 0);
    }
    e.fb.ins().return_(&[]);
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}
