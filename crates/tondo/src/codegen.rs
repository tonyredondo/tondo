//! MIR to native object code with Cranelift.
//!
//! Values have one of three classes. Zero-sized values have no storage.
//! Scalars (integers, `Bool`, string pointers) live in SSA variables.
//! Aggregates (tuples, records) live in memory with a C-like layout: locals
//! get stack slots, parameters arrive as pointers to the caller's memory, and
//! results are written through a pointer the caller passes first.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{Callee, Program, StdFn};
use crate::mir::{Body, Const, Local, Operand, Piece, Place, Rvalue, Statement, Terminator};
use crate::source::{SourceMap, Span};
use crate::types::{IntTy, Ty, TyKind, Types};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    self, AbiParam, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, TrapCode, UserFuncName, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;

const POINTER: ir::Type = types::I64;
const IMMORTAL: u64 = u64::MAX;

/// Runtime entry points (see `tondo-runtime`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Rt {
    StrRetain,
    StrRelease,
    StrEq,
    StrCmp,
    SbNew,
    SbStr,
    SbI64,
    SbU64,
    SbBool,
    SbFinish,
    ConsolePrint,
    ConsolePrintln,
    Panic,
}

impl Rt {
    fn signature(self) -> (&'static str, &'static [ir::Type], &'static [ir::Type]) {
        use types::{I8, I32, I64};
        match self {
            Rt::StrRetain => ("tondo_rt_str_retain", &[I64], &[]),
            Rt::StrRelease => ("tondo_rt_str_release", &[I64], &[]),
            Rt::StrEq => ("tondo_rt_str_eq", &[I64, I64], &[I8]),
            Rt::StrCmp => ("tondo_rt_str_cmp", &[I64, I64], &[I32]),
            Rt::SbNew => ("tondo_rt_sb_new", &[], &[I64]),
            Rt::SbStr => ("tondo_rt_sb_str", &[I64, I64], &[]),
            Rt::SbI64 => ("tondo_rt_sb_i64", &[I64, I64], &[]),
            Rt::SbU64 => ("tondo_rt_sb_u64", &[I64, I64], &[]),
            Rt::SbBool => ("tondo_rt_sb_bool", &[I64, I8], &[]),
            Rt::SbFinish => ("tondo_rt_sb_finish", &[I64], &[I64]),
            Rt::ConsolePrint => ("tondo_rt_console_print", &[I64], &[I64]),
            Rt::ConsolePrintln => ("tondo_rt_console_println", &[I64], &[I64]),
            Rt::Panic => ("tondo_rt_panic", &[I32, I64, I64], &[]),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Class {
    Zero,
    Scalar(ir::Type),
    Memory,
}

#[derive(Clone)]
struct Layout {
    size: u32,
    align: u32,
    offsets: Vec<u32>,
}

/// A value during code generation.
#[derive(Copy, Clone)]
enum Val {
    Zero,
    Scalar(ir::Value),
    /// The address of an aggregate.
    Memory(ir::Value),
}

fn int_type(int: IntTy) -> ir::Type {
    match int.bits() {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    }
}

fn symbol_name(index: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("tondo_f{index}_{clean}")
}

pub fn compile(program: &Program, bodies: &[Body], sources: &SourceMap) -> Result<Vec<u8>, String> {
    let mut flags = settings::builder();
    flags.set("opt_level", "speed").map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    let isa = cranelift_native::builder()
        .map_err(|e| format!("unsupported host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let builder =
        ObjectBuilder::new(isa, "tondo", cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    let mut cg = Codegen {
        module: ObjectModule::new(builder),
        types: &program.types,
        sources,
        functions: Vec::new(),
        runtime: HashMap::new(),
        strings: HashMap::new(),
        layouts: HashMap::new(),
    };

    for (index, body) in bodies.iter().enumerate() {
        let signature = cg.signature(body);
        let id = cg
            .module
            .declare_function(&symbol_name(index, &body.name), Linkage::Local, &signature)
            .map_err(|e| e.to_string())?;
        cg.functions.push(id);
    }
    let mut context = cg.module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    for (index, body) in bodies.iter().enumerate() {
        context.func.signature = cg.signature(body);
        context.func.name = UserFuncName::user(0, index as u32);
        FnCodegen::new(&mut cg, body, &mut context.func, &mut builder_context).run();
        cg.module.define_function(cg.functions[index], &mut context).map_err(|e| format!("{e:?}"))?;
        cg.module.clear_context(&mut context);
    }
    cg.entry_point(program.main.0 as usize, &mut context, &mut builder_context)?;

    let product = cg.module.finish();
    product.emit().map_err(|e| e.to_string())
}

struct Codegen<'a> {
    module: ObjectModule,
    types: &'a Types,
    sources: &'a SourceMap,
    functions: Vec<FuncId>,
    runtime: HashMap<Rt, FuncId>,
    strings: HashMap<String, DataId>,
    layouts: HashMap<Ty, Layout>,
}

impl Codegen<'_> {
    fn class(&mut self, ty: Ty) -> Class {
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Unit | TyKind::Never | TyKind::Error => Class::Zero,
            TyKind::Bool => Class::Scalar(types::I8),
            TyKind::Int(int) => Class::Scalar(int_type(*int)),
            TyKind::String | TyKind::Result(..) | TyKind::Std(_) => Class::Scalar(POINTER),
            TyKind::Tuple(_) | TyKind::Adt(_) => {
                if self.layout(ty).size == 0 {
                    Class::Zero
                } else {
                    Class::Memory
                }
            }
        }
    }

    fn layout(&mut self, ty: Ty) -> Layout {
        let ty = self.types.repr(ty);
        if let Some(layout) = self.layouts.get(&ty) {
            return layout.clone();
        }
        let layout = match self.types.components(ty) {
            Some(components) => {
                let (mut size, mut align, mut offsets) = (0u32, 1u32, Vec::new());
                for component in components {
                    let inner = self.layout(component);
                    let offset = size.next_multiple_of(inner.align);
                    offsets.push(offset);
                    size = offset + inner.size;
                    align = align.max(inner.align);
                }
                Layout { size: size.next_multiple_of(align), align, offsets }
            }
            None => {
                let size = match self.class(ty) {
                    Class::Scalar(t) => t.bytes(),
                    _ => 0,
                };
                Layout { size, align: size.max(1), offsets: Vec::new() }
            }
        };
        self.layouts.insert(ty, layout.clone());
        layout
    }

    fn signature(&mut self, body: &Body) -> ir::Signature {
        let mut signature = self.module.make_signature();
        if self.class(body.result) == Class::Memory {
            signature.params.push(AbiParam::new(POINTER));
        }
        for &param in &body.params {
            match self.class(body.locals[param.0 as usize]) {
                Class::Zero => {}
                Class::Scalar(ty) => signature.params.push(AbiParam::new(ty)),
                Class::Memory => signature.params.push(AbiParam::new(POINTER)),
            }
        }
        if let Class::Scalar(ty) = self.class(body.result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    fn runtime(&mut self, rt: Rt) -> FuncId {
        if let Some(&id) = self.runtime.get(&rt) {
            return id;
        }
        let (name, params, returns) = rt.signature();
        let mut signature = self.module.make_signature();
        signature.params.extend(params.iter().map(|&t| AbiParam::new(t)));
        signature.returns.extend(returns.iter().map(|&t| AbiParam::new(t)));
        let id = self.module.declare_function(name, Linkage::Import, &signature).expect("runtime declaration");
        self.runtime.insert(rt, id);
        id
    }

    /// A static, immortal string object: `[refcount][length][bytes]`.
    fn string(&mut self, text: &str) -> DataId {
        if let Some(&id) = self.strings.get(text) {
            return id;
        }
        let mut bytes = Vec::with_capacity(16 + text.len());
        bytes.extend_from_slice(&IMMORTAL.to_le_bytes());
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
    fn entry_point(
        &mut self,
        main: usize,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let mut signature = self.module.make_signature();
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("tondo_main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 0);
        {
            let mut fb = FunctionBuilder::new(&mut context.func, fbc);
            let block = fb.create_block();
            fb.switch_to_block(block);
            let callee = self.module.declare_func_in_func(self.functions[main], fb.func);
            fb.ins().call(callee, &[]);
            let status = fb.ins().iconst(types::I32, 0);
            fb.ins().return_(&[status]);
            fb.seal_all_blocks();
            fb.finalize(self.module.isa().frontend_config());
        }
        self.module.define_function(id, context).map_err(|e| format!("{e:?}"))?;
        self.module.clear_context(context);
        Ok(())
    }
}

/// Where a MIR local lives.
#[derive(Copy, Clone)]
enum Storage {
    Zero,
    Var(Variable),
    Slot(ir::StackSlot),
    /// An aggregate owned by someone else: a parameter or the result slot.
    Pointer(Variable),
}

struct FnCodegen<'a, 'b> {
    cg: &'a mut Codegen<'b>,
    body: &'a Body,
    fb: FunctionBuilder<'a>,
    blocks: Vec<ir::Block>,
    storage: Vec<Storage>,
}

impl<'a, 'b> FnCodegen<'a, 'b> {
    fn new(
        cg: &'a mut Codegen<'b>,
        body: &'a Body,
        func: &'a mut ir::Function,
        fbc: &'a mut FunctionBuilderContext,
    ) -> Self {
        let mut fb = FunctionBuilder::new(func, fbc);
        let blocks = body.blocks.iter().map(|_| fb.create_block()).collect();
        let mut storage = Vec::with_capacity(body.locals.len());
        for (index, &ty) in body.locals.iter().enumerate() {
            let local = Local(index as u32);
            let borrowed = body.params.contains(&local) || body.ret == Some(local);
            storage.push(match cg.class(ty) {
                Class::Zero => Storage::Zero,
                Class::Scalar(t) => Storage::Var(fb.declare_var(t)),
                Class::Memory if borrowed => Storage::Pointer(fb.declare_var(POINTER)),
                Class::Memory => {
                    let layout = cg.layout(ty);
                    let data = StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        layout.size,
                        layout.align.trailing_zeros() as u8,
                    );
                    Storage::Slot(fb.create_sized_stack_slot(data))
                }
            });
        }
        FnCodegen { cg, body, fb, blocks, storage }
    }

    fn run(mut self) {
        let entry = self.blocks[0];
        self.fb.append_block_params_for_function_params(entry);
        self.fb.switch_to_block(entry);
        let mut incoming = self.fb.block_params(entry).to_vec().into_iter();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            self.fb.def_var(var, incoming.next().unwrap());
        }
        for &param in &self.body.params {
            match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.fb.def_var(var, incoming.next().unwrap()),
                Storage::Zero | Storage::Slot(_) => {}
            }
        }
        for (index, block) in self.body.blocks.iter().enumerate() {
            if index > 0 {
                self.fb.switch_to_block(self.blocks[index]);
            }
            for stmt in &block.stmts {
                self.statement(stmt);
            }
            self.terminator(&block.terminator);
        }
        self.fb.seal_all_blocks();
        let config = self.cg.module.isa().frontend_config();
        self.fb.finalize(config);
    }

    // ---- places and values ---------------------------------------------------

    fn place_ty(&self, place: &Place) -> Ty {
        self.body.place_ty(self.cg.types, place)
    }

    fn operand_ty(&self, operand: &Operand) -> Ty {
        match operand {
            Operand::Copy(place) => self.place_ty(place),
            Operand::Const(Const::Int(_, ty)) => *ty,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
            Operand::Const(Const::Unit) => Ty::UNIT,
        }
    }

    /// Base address and byte offset of a place inside an aggregate local.
    fn address(&mut self, place: &Place) -> (ir::Value, u32) {
        let base = match self.storage[place.local.0 as usize] {
            Storage::Slot(slot) => self.fb.ins().stack_addr(POINTER, slot, 0),
            Storage::Pointer(var) => self.fb.use_var(var),
            Storage::Var(_) | Storage::Zero => unreachable!("fields of a non-aggregate"),
        };
        let mut ty = self.body.locals[place.local.0 as usize];
        let mut offset = 0;
        for &index in &place.path {
            offset += self.cg.layout(ty).offsets[index as usize];
            ty = self.cg.types.components(ty).unwrap()[index as usize];
        }
        (base, offset)
    }

    fn pointer_to(&mut self, place: &Place) -> ir::Value {
        let (base, offset) = self.address(place);
        if offset == 0 { base } else { self.fb.ins().iadd_imm_s(base, offset as i64) }
    }

    fn read(&mut self, place: &Place) -> Val {
        match self.cg.class(self.place_ty(place)) {
            Class::Zero => Val::Zero,
            Class::Memory => Val::Memory(self.pointer_to(place)),
            Class::Scalar(ty) => match self.storage[place.local.0 as usize] {
                Storage::Var(var) if place.path.is_empty() => Val::Scalar(self.fb.use_var(var)),
                _ => {
                    let (base, offset) = self.address(place);
                    Val::Scalar(self.fb.ins().load(ty, MemFlagsData::trusted(), base, offset as i32))
                }
            },
        }
    }

    fn write(&mut self, place: &Place, value: Val) {
        match (value, self.storage[place.local.0 as usize]) {
            (Val::Zero, _) => {}
            (Val::Scalar(value), Storage::Var(var)) if place.path.is_empty() => self.fb.def_var(var, value),
            (Val::Scalar(value), _) => {
                let (base, offset) = self.address(place);
                self.fb.ins().store(MemFlagsData::trusted(), value, base, offset as i32);
            }
            (Val::Memory(source), _) => {
                let size = self.cg.layout(self.place_ty(place)).size;
                let destination = self.pointer_to(place);
                self.copy_memory(destination, source, size);
            }
        }
    }

    /// Reads a component at `offset` inside the aggregate at `base`.
    fn component(&mut self, base: ir::Value, offset: u32, ty: Ty) -> Val {
        match self.cg.class(ty) {
            Class::Zero => Val::Zero,
            Class::Scalar(t) => Val::Scalar(self.fb.ins().load(t, MemFlagsData::trusted(), base, offset as i32)),
            Class::Memory => Val::Memory(self.fb.ins().iadd_imm_s(base, offset as i64)),
        }
    }

    fn copy_memory(&mut self, destination: ir::Value, source: ir::Value, size: u32) {
        if size > 64 {
            let config = self.cg.module.isa().frontend_config();
            let size = self.fb.ins().iconst(POINTER, size as i64);
            self.fb.call_memcpy(config, destination, source, size);
            return;
        }
        let mut offset = 0;
        for (bytes, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while size - offset >= bytes {
                let value = self.fb.ins().load(ty, MemFlagsData::trusted(), source, offset as i32);
                self.fb.ins().store(MemFlagsData::trusted(), value, destination, offset as i32);
                offset += bytes;
            }
        }
    }

    fn operand(&mut self, operand: &Operand) -> Val {
        match operand {
            Operand::Copy(place) => self.read(place),
            Operand::Const(Const::Int(value, ty)) => {
                let Class::Scalar(clif) = self.cg.class(*ty) else { unreachable!() };
                // Narrow immediates are zero-extended bit patterns.
                let bits = clif.bits();
                let pattern = if bits == 64 { *value as i64 } else { (*value as i64) & ((1i64 << bits) - 1) };
                Val::Scalar(self.fb.ins().iconst(clif, pattern))
            }
            Operand::Const(Const::Bool(value)) => Val::Scalar(self.fb.ins().iconst(types::I8, *value as i64)),
            Operand::Const(Const::Str(text)) => Val::Scalar(self.string_ptr(text)),
            Operand::Const(Const::Unit) => Val::Zero,
        }
    }

    fn scalar(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar operand"),
        }
    }

    fn call_rt(&mut self, rt: Rt, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.runtime(rt);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn string_ptr(&mut self, text: &str) -> ir::Value {
        let id = self.cg.string(text);
        let global = self.cg.module.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
    }

    fn location(&mut self, span: Span) -> ir::Value {
        let location = self.cg.sources.location(span);
        self.string_ptr(&location)
    }

    // ---- statements and terminators ----------------------------------------

    fn statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(place, rvalue, span) => self.assign(place, rvalue, *span),
            Statement::Retain(place) | Statement::Release(place) => {
                let ty = self.place_ty(place);
                let value = self.read(place);
                self.refcount(ty, value, matches!(stmt, Statement::Release(_)));
            }
        }
    }

    /// Retains or releases every counted value inside `value`.
    fn refcount(&mut self, ty: Ty, value: Val, release: bool) {
        if !self.cg.types.needs_drop(ty) {
            return;
        }
        match value {
            Val::Scalar(pointer) => {
                self.call_rt(if release { Rt::StrRelease } else { Rt::StrRetain }, &[pointer]);
            }
            Val::Memory(base) => {
                let components = self.cg.types.components(ty).unwrap();
                let offsets = self.cg.layout(ty).offsets;
                for (component, offset) in components.into_iter().zip(offsets) {
                    if self.cg.types.needs_drop(component) {
                        let value = self.component(base, offset, component);
                        self.refcount(component, value, release);
                    }
                }
            }
            Val::Zero => {}
        }
    }

    fn assign(&mut self, place: &Place, rvalue: &Rvalue, span: Span) {
        let ty = self.place_ty(place);
        if self.cg.class(ty) == Class::Zero {
            // Zero-sized values carry no data; only calls have effects.
            if let Rvalue::Call(callee, args) = rvalue {
                self.call(*callee, args, None);
            }
            return;
        }
        let value = match rvalue {
            Rvalue::Call(callee, args) => {
                let result = if self.cg.class(ty) == Class::Memory { Some(self.pointer_to(place)) } else { None };
                match self.call(*callee, args, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::Aggregate(components) => {
                let base = self.pointer_to(place);
                let types = self.cg.types.components(ty).unwrap();
                let offsets = self.cg.layout(ty).offsets;
                for ((component, ty), offset) in components.iter().zip(types).zip(offsets) {
                    match self.operand(component) {
                        Val::Zero => {}
                        Val::Scalar(value) => {
                            self.fb.ins().store(MemFlagsData::trusted(), value, base, offset as i32);
                        }
                        Val::Memory(source) => {
                            let size = self.cg.layout(ty).size;
                            let destination = self.fb.ins().iadd_imm_s(base, offset as i64);
                            self.copy_memory(destination, source, size);
                        }
                    }
                }
                return;
            }
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Unary(op, operand) => Val::Scalar(self.unary(*op, operand, span)),
            Rvalue::Increment(operand) => {
                let value = self.scalar(operand);
                Val::Scalar(self.fb.ins().iadd_imm_s(value, 1))
            }
            Rvalue::Binary(op, left, right) => Val::Scalar(self.binary(*op, left, right, span)),
            Rvalue::Concat(pieces) => Val::Scalar(self.concat(pieces)),
        };
        self.write(place, value);
    }

    fn call(&mut self, callee: Callee, args: &[Operand], result: Option<ir::Value>) -> Option<ir::Value> {
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        for arg in args {
            match self.operand(arg) {
                Val::Scalar(value) | Val::Memory(value) => values.push(value),
                Val::Zero => {}
            }
        }
        match callee {
            Callee::Fn(id) => {
                let func = self.cg.module.declare_func_in_func(self.cg.functions[id.0 as usize], self.fb.func);
                let call = self.fb.ins().call(func, &values);
                self.fb.inst_results(call).first().copied()
            }
            Callee::Std(StdFn::ConsolePrint) => self.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.call_rt(Rt::ConsolePrintln, &values),
        }
    }

    fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                let condition = self.scalar(condition);
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                self.fb.ins().brif(condition, then, &[], otherwise, &[]);
            }
            Terminator::Return(value) => match self.operand(value) {
                Val::Scalar(value) => {
                    self.fb.ins().return_(&[value]);
                }
                // Aggregates were already written through the result pointer.
                Val::Memory(_) | Val::Zero => {
                    self.fb.ins().return_(&[]);
                }
            },
            Terminator::Panic { code, message, span } => {
                let message = match message {
                    Some(message) => self.scalar(message),
                    None => self.fb.ins().iconst(POINTER, 0),
                };
                self.emit_panic(*code, message, *span);
            }
            Terminator::Unreachable => {
                self.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
    }

    fn emit_panic(&mut self, code: u32, message: ir::Value, span: Span) {
        let code = self.fb.ins().iconst(types::I32, code as i64);
        let location = self.location(span);
        self.call_rt(Rt::Panic, &[code, message, location]);
        self.fb.ins().trap(TrapCode::user(1).unwrap());
    }

    /// Branches to a cold block that panics with `code` when `condition` holds.
    fn panic_if(&mut self, condition: ir::Value, code: u32, span: Span) {
        let (failed, next) = (self.fb.create_block(), self.fb.create_block());
        self.fb.set_cold_block(failed);
        self.fb.ins().brif(condition, failed, &[], next, &[]);
        self.fb.switch_to_block(failed);
        let null = self.fb.ins().iconst(POINTER, 0);
        self.emit_panic(code, null, span);
        self.fb.switch_to_block(next);
    }

    // ---- operators -----------------------------------------------------------

    fn unary(&mut self, op: UnaryOp, operand: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.scalar(operand);
        match op {
            UnaryOp::Not => self.fb.ins().bxor_imm_u(value, 1),
            UnaryOp::BitNot => self.fb.ins().bnot(value),
            UnaryOp::Neg => {
                let Class::Scalar(clif) = self.cg.class(ty) else { unreachable!() };
                let zero = self.fb.ins().iconst(clif, 0);
                let (result, overflow) = self.fb.ins().ssub_overflow(zero, value);
                self.panic_if(overflow, 5, span);
                result
            }
        }
    }

    fn concat(&mut self, pieces: &[Piece]) -> ir::Value {
        let builder = self.call_rt(Rt::SbNew, &[]).unwrap();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    let text = self.string_ptr(text);
                    self.call_rt(Rt::SbStr, &[builder, text]);
                }
                Piece::Value(operand, ty) => {
                    let value = self.scalar(operand);
                    match self.cg.types.kind(self.cg.types.repr(*ty)) {
                        TyKind::String => {
                            self.call_rt(Rt::SbStr, &[builder, value]);
                        }
                        TyKind::Bool => {
                            self.call_rt(Rt::SbBool, &[builder, value]);
                        }
                        TyKind::Int(int) => {
                            let wide = match (int.bits(), int.signed()) {
                                (64, _) => value,
                                (_, true) => self.fb.ins().sextend(types::I64, value),
                                (_, false) => self.fb.ins().uextend(types::I64, value),
                            };
                            let rt = if int.signed() { Rt::SbI64 } else { Rt::SbU64 };
                            self.call_rt(rt, &[builder, wide]);
                        }
                        other => unreachable!("no Display for {other:?}"),
                    }
                }
            }
        }
        self.call_rt(Rt::SbFinish, &[builder]).unwrap()
    }

    /// Structural equality (8.11): 1 when equal, 0 otherwise.
    fn equal(&mut self, ty: Ty, a: Val, b: Val) -> ir::Value {
        match (a, b) {
            (Val::Scalar(a), Val::Scalar(b)) => {
                if self.cg.types.repr(ty) == Ty::STRING {
                    self.call_rt(Rt::StrEq, &[a, b]).unwrap()
                } else {
                    self.fb.ins().icmp(IntCC::Equal, a, b)
                }
            }
            (Val::Memory(a), Val::Memory(b)) => {
                let components = self.cg.types.components(ty).unwrap();
                let offsets = self.cg.layout(ty).offsets;
                let mut result = self.fb.ins().iconst(types::I8, 1);
                for (component, offset) in components.into_iter().zip(offsets) {
                    let left = self.component(a, offset, component);
                    let right = self.component(b, offset, component);
                    let same = self.equal(component, left, right);
                    result = self.fb.ins().band(result, same);
                }
                result
            }
            _ => self.fb.ins().iconst(types::I8, 1),
        }
    }

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        if matches!(op, BinOp::Eq | BinOp::Ne) && !matches!(self.cg.class(ty), Class::Scalar(_)) {
            let (a, b) = (self.operand(left), self.operand(right));
            let equal = self.equal(ty, a, b);
            return if op == BinOp::Eq { equal } else { self.fb.ins().bxor_imm_u(equal, 1) };
        }
        let (a, b) = (self.scalar(left), self.scalar(right));
        if self.cg.types.repr(ty) == Ty::STRING {
            return self.string_compare(op, a, b);
        }
        let types = self.cg.types;
        let signed = types.as_int(ty).is_some_and(IntTy::signed);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let ins = self.fb.ins();
                let (result, overflow) = match (op, signed) {
                    (BinOp::Add, true) => ins.sadd_overflow(a, b),
                    (BinOp::Add, false) => ins.uadd_overflow(a, b),
                    (BinOp::Sub, true) => ins.ssub_overflow(a, b),
                    (BinOp::Sub, false) => ins.usub_overflow(a, b),
                    (_, true) => ins.smul_overflow(a, b),
                    (_, false) => ins.umul_overflow(a, b),
                };
                self.panic_if(overflow, 5, span);
                result
            }
            BinOp::Div | BinOp::Rem => {
                let zero = self.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.panic_if(zero, 3, span);
                if !signed {
                    return if op == BinOp::Div { self.fb.ins().udiv(a, b) } else { self.fb.ins().urem(a, b) };
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let int = types.as_int(ty).unwrap();
                let is_min = self.fb.ins().icmp_imm_s(IntCC::Equal, a, int.min() as i64);
                let is_minus_one = self.fb.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let both = self.fb.ins().band(is_min, is_minus_one);
                if op == BinOp::Div {
                    self.panic_if(both, 5, span);
                    self.fb.ins().sdiv(a, b)
                } else {
                    let one = self.fb.ins().iconst(int_type(int), 1);
                    let divisor = self.fb.ins().select(both, one, b);
                    self.fb.ins().srem(a, divisor)
                }
            }
            BinOp::BitAnd => self.fb.ins().band(a, b),
            BinOp::BitOr => self.fb.ins().bor(a, b),
            BinOp::BitXor => self.fb.ins().bxor(a, b),
            BinOp::Shl | BinOp::Shr => {
                let width = types.as_int(ty).unwrap().bits() as i64;
                if types.as_int(right_ty).is_some_and(IntTy::signed) {
                    let negative = self.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                    self.panic_if(negative, 10, span);
                }
                let too_wide = self.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b, width);
                self.panic_if(too_wide, 10, span);
                match (op, signed) {
                    (BinOp::Shl, _) => self.fb.ins().ishl(a, b),
                    (_, true) => self.fb.ins().sshr(a, b),
                    (_, false) => self.fb.ins().ushr(a, b),
                }
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.fb.ins().icmp(int_condition(op, signed), a, b)
            }
            BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
                unreachable!("lowered before codegen")
            }
        }
    }

    fn string_compare(&mut self, op: BinOp, a: ir::Value, b: ir::Value) -> ir::Value {
        match op {
            BinOp::Eq => self.call_rt(Rt::StrEq, &[a, b]).unwrap(),
            BinOp::Ne => {
                let equal = self.call_rt(Rt::StrEq, &[a, b]).unwrap();
                self.fb.ins().bxor_imm_u(equal, 1)
            }
            _ => {
                let order = self.call_rt(Rt::StrCmp, &[a, b]).unwrap();
                self.fb.ins().icmp_imm_s(int_condition(op, true), order, 0)
            }
        }
    }
}

fn int_condition(op: BinOp, signed: bool) -> IntCC {
    match (op, signed) {
        (BinOp::Eq, _) => IntCC::Equal,
        (BinOp::Ne, _) => IntCC::NotEqual,
        (BinOp::Lt, true) => IntCC::SignedLessThan,
        (BinOp::Le, true) => IntCC::SignedLessThanOrEqual,
        (BinOp::Gt, true) => IntCC::SignedGreaterThan,
        (BinOp::Ge, true) => IntCC::SignedGreaterThanOrEqual,
        (BinOp::Lt, false) => IntCC::UnsignedLessThan,
        (BinOp::Le, false) => IntCC::UnsignedLessThanOrEqual,
        (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
        (BinOp::Ge, false) => IntCC::UnsignedGreaterThanOrEqual,
        _ => unreachable!(),
    }
}
