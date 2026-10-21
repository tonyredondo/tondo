//! MIR to native object code with Cranelift.
//!
//! Values have one of three classes. Zero-sized values have no storage.
//! Scalars (integers, `Bool`, string pointers, payload-free enums) live in SSA
//! variables. Aggregates (tuples, records, enums with payloads) live in memory
//! with a C-like layout: locals get stack slots, parameters arrive as pointers
//! to the caller's memory, and results are written through a pointer the
//! caller passes first.
//!
//! Where a recursive type contains itself, the component lives in a box: a
//! reference-counted heap cell (see `Types::is_boxed`). Boxes are shared on
//! copy and made unique before a write through them, which keeps value
//! semantics. Per-type "glue" functions retain, release and compare values
//! that contain boxes or enums; the runtime calls the release glue when the
//! last reference to a box goes away.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{Callee, Program, StdFn};
use crate::mir::{Body, Const, Local, Operand, Piece, Place, Proj, Rvalue, Statement, Terminator, project};
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
/// Size of a box header in the runtime (`BoxHeader`).
const BOX_HEADER: i64 = 16;

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
    BoxNew,
    BoxRetain,
    BoxRelease,
    BoxUnique,
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
            Rt::BoxNew => ("tondo_rt_box_new", &[I64], &[I64]),
            Rt::BoxRetain => ("tondo_rt_box_retain", &[I64], &[]),
            Rt::BoxRelease => ("tondo_rt_box_release", &[I64, I64], &[]),
            Rt::BoxUnique => ("tondo_rt_box_unique", &[I64, I64, I64], &[]),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Class {
    Zero,
    Scalar(ir::Type),
    Memory,
}

/// Placement of the components of a struct-like value.
#[derive(Clone, Default)]
struct Fields {
    offsets: Vec<u32>,
    boxed: Vec<bool>,
}

#[derive(Clone)]
enum Shape {
    Scalar,
    Struct(Fields),
    /// The tag is at offset 0; each variant's payload starts at `payload`.
    Enum {
        tag: ir::Type,
        payload: u32,
        variants: Vec<Fields>,
    },
}

#[derive(Clone)]
struct Layout {
    size: u32,
    align: u32,
    shape: Shape,
}

/// Where a component lives inside its container.
struct Slot {
    offset: u32,
    ty: Ty,
    boxed: bool,
}

/// A value during code generation.
#[derive(Copy, Clone)]
enum Val {
    Zero,
    Scalar(ir::Value),
    /// The address of an aggregate.
    Memory(ir::Value),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Glue {
    Retain,
    Release,
    Equal,
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

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
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
        glue: HashMap::new(),
        pending_glue: Vec::new(),
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
    while let Some((kind, ty, id)) = cg.pending_glue.pop() {
        context.func.signature = cg.glue_signature(kind);
        context.func.name = UserFuncName::user(2, id.as_u32());
        define_glue(&mut cg, kind, ty, &mut context.func, &mut builder_context);
        cg.module.define_function(id, &mut context).map_err(|e| format!("{e:?}"))?;
        cg.module.clear_context(&mut context);
    }

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
    glue: HashMap<(Glue, Ty), FuncId>,
    pending_glue: Vec<(Glue, Ty, FuncId)>,
}

impl Codegen<'_> {
    fn class(&mut self, ty: Ty) -> Class {
        let repr = self.types.repr(ty);
        match self.types.kind(repr) {
            TyKind::Unit | TyKind::Never | TyKind::Error => Class::Zero,
            TyKind::Bool => Class::Scalar(types::I8),
            TyKind::Int(int) => Class::Scalar(int_type(*int)),
            TyKind::String | TyKind::Result(..) | TyKind::Std(_) => Class::Scalar(POINTER),
            TyKind::Tuple(_) | TyKind::Adt(_) => {
                let layout = self.layout(repr);
                match layout.shape {
                    Shape::Enum { tag, .. } if layout.size == tag.bytes() => Class::Scalar(tag),
                    _ if layout.size == 0 => Class::Zero,
                    _ => Class::Memory,
                }
            }
        }
    }

    fn fields(&mut self, container: Ty, components: &[Ty]) -> (Fields, u32, u32) {
        let (mut size, mut align, mut fields) = (0u32, 1u32, Fields::default());
        for &component in components {
            let boxed = self.types.is_boxed(container, component);
            let (component_size, component_align) = if boxed {
                (8, 8)
            } else {
                let layout = self.layout(component);
                (layout.size, layout.align)
            };
            let offset = size.next_multiple_of(component_align);
            fields.offsets.push(offset);
            fields.boxed.push(boxed);
            size = offset + component_size;
            align = align.max(component_align);
        }
        (fields, size, align)
    }

    fn layout(&mut self, ty: Ty) -> Layout {
        let ty = self.types.repr(ty);
        if let Some(layout) = self.layouts.get(&ty) {
            return layout.clone();
        }
        let layout = if let Some(components) = self.types.components(ty) {
            let (fields, size, align) = self.fields(ty, &components);
            Layout { size: size.next_multiple_of(align), align, shape: Shape::Struct(fields) }
        } else if let Some(variants) = self.types.variants(ty) {
            let tag = self.scalar_type(self.types.tag_ty(ty));
            let (mut payload_size, mut payload_align, mut placed) = (0, 1, Vec::new());
            for components in &variants {
                let (fields, size, align) = self.fields(ty, components);
                payload_size = payload_size.max(size);
                payload_align = payload_align.max(align);
                placed.push(fields);
            }
            let payload = tag.bytes().next_multiple_of(payload_align);
            let align = payload_align.max(tag.bytes());
            let size = if payload_size == 0 { tag.bytes() } else { (payload + payload_size).next_multiple_of(align) };
            Layout { size, align, shape: Shape::Enum { tag, payload, variants: placed } }
        } else {
            let size = match self.class(ty) {
                Class::Scalar(t) => t.bytes(),
                _ => 0,
            };
            Layout { size, align: size.max(1), shape: Shape::Scalar }
        };
        self.layouts.insert(ty, layout.clone());
        layout
    }

    fn scalar_type(&mut self, ty: Ty) -> ir::Type {
        match self.class(ty) {
            Class::Scalar(t) => t,
            other => unreachable!("not a scalar: {other:?}"),
        }
    }

    /// The component that `proj` selects inside a value of type `ty`.
    fn slot(&mut self, ty: Ty, proj: Proj) -> Slot {
        let component = project(self.types, ty, proj);
        match (self.layout(ty).shape, proj) {
            (Shape::Struct(fields), Proj::Field(index)) => {
                Slot { offset: fields.offsets[index as usize], ty: component, boxed: fields.boxed[index as usize] }
            }
            (Shape::Enum { payload, variants, .. }, Proj::Variant(variant, index)) => {
                let fields = &variants[variant as usize];
                Slot {
                    offset: payload + fields.offsets[index as usize],
                    ty: component,
                    boxed: fields.boxed[index as usize],
                }
            }
            _ => unreachable!("projection does not match the layout"),
        }
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

    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        let mut signature = self.module.make_signature();
        signature.params.push(AbiParam::new(POINTER));
        if kind == Glue::Equal {
            signature.params.push(AbiParam::new(POINTER));
            signature.returns.push(AbiParam::new(types::I8));
        }
        signature
    }

    /// The glue function of `kind` for values of type `ty`, defined later.
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        let ty = self.types.repr(ty);
        if let Some(&id) = self.glue.get(&(kind, ty)) {
            return id;
        }
        let signature = self.glue_signature(kind);
        let name = format!("tondo_glue_{kind:?}_{}", self.glue.len()).to_lowercase();
        let id = self.module.declare_function(&name, Linkage::Local, &signature).expect("glue declaration");
        self.glue.insert((kind, ty), id);
        self.pending_glue.push((kind, ty, id));
        id
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

/// Code emission shared by function bodies and glue functions.
struct Emit<'a, 'b> {
    cg: &'a mut Codegen<'b>,
    fb: FunctionBuilder<'a>,
}

impl Emit<'_, '_> {
    fn call_rt(&mut self, rt: Rt, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.runtime(rt);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn call_glue(&mut self, kind: Glue, ty: Ty, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn glue_address(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        self.fb.ins().func_addr(POINTER, callee)
    }

    fn string_ptr(&mut self, text: &str) -> ir::Value {
        let id = self.cg.string(text);
        let global = self.cg.module.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
    }

    fn offset(&mut self, base: ir::Value, offset: u32) -> ir::Value {
        if offset == 0 { base } else { self.fb.ins().iadd_imm_s(base, offset as i64) }
    }

    /// The value of type `ty` stored at `address`.
    fn value_at(&mut self, address: ir::Value, ty: Ty) -> Val {
        match self.cg.class(ty) {
            Class::Zero => Val::Zero,
            Class::Scalar(t) => Val::Scalar(self.fb.ins().load(t, flags(), address, 0)),
            Class::Memory => Val::Memory(address),
        }
    }

    /// Where the component lives: inside the container, or inside its box.
    fn component_address(&mut self, base: ir::Value, slot: &Slot) -> ir::Value {
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            self.fb.ins().iadd_imm_s(cell, BOX_HEADER)
        } else {
            self.offset(base, slot.offset)
        }
    }

    fn store_val(&mut self, address: ir::Value, ty: Ty, value: Val) {
        match value {
            Val::Zero => {}
            Val::Scalar(value) => {
                self.fb.ins().store(flags(), value, address, 0);
            }
            Val::Memory(source) => {
                let size = self.cg.layout(ty).size;
                self.copy_memory(address, source, size);
            }
        }
    }

    /// Writes a component into a container under construction, allocating a
    /// box for boxed components.
    fn init_component(&mut self, base: ir::Value, slot: &Slot, value: Val) {
        if slot.boxed {
            let size = self.fb.ins().iconst(POINTER, self.cg.layout(slot.ty).size as i64);
            let cell = self.call_rt(Rt::BoxNew, &[size]).unwrap();
            let inner = self.fb.ins().iadd_imm_s(cell, BOX_HEADER);
            self.store_val(inner, slot.ty, value);
            self.fb.ins().store(flags(), cell, base, slot.offset as i32);
        } else {
            let address = self.offset(base, slot.offset);
            self.store_val(address, slot.ty, value);
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
                let value = self.fb.ins().load(ty, flags(), source, offset as i32);
                self.fb.ins().store(flags(), value, destination, offset as i32);
                offset += bytes;
            }
        }
    }

    fn tag(&mut self, ty: Ty, value: Val) -> ir::Value {
        match value {
            Val::Scalar(tag) => tag,
            Val::Memory(address) => {
                let tag = self.cg.scalar_type(self.cg.types.tag_ty(ty));
                self.fb.ins().load(tag, flags(), address, 0)
            }
            Val::Zero => self.fb.ins().iconst(types::I8, 0),
        }
    }

    /// Retains or releases every counted value inside `value`.
    fn refcount(&mut self, ty: Ty, value: Val, release: bool) {
        if !self.cg.types.needs_drop(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => {
                self.call_rt(if release { Rt::StrRelease } else { Rt::StrRetain }, &[pointer]);
            }
            (Val::Memory(base), Shape::Struct(_)) => {
                let count = self.cg.types.components(ty).unwrap().len() as u32;
                for index in 0..count {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    self.refcount_slot(base, &slot, release);
                }
            }
            (Val::Memory(address), Shape::Enum { .. }) => {
                let kind = if release { Glue::Release } else { Glue::Retain };
                self.call_glue(kind, ty, &[address]);
            }
            _ => {}
        }
    }

    fn refcount_slot(&mut self, base: ir::Value, slot: &Slot, release: bool) {
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            if release {
                let glue = self.glue_address(Glue::Release, slot.ty);
                self.call_rt(Rt::BoxRelease, &[cell, glue]);
            } else {
                self.call_rt(Rt::BoxRetain, &[cell]);
            }
        } else if self.cg.types.needs_drop(slot.ty) {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.refcount(slot.ty, value, release);
        }
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
            (Val::Memory(a), Val::Memory(b)) => match self.cg.layout(ty).shape {
                Shape::Struct(_) => {
                    let count = self.cg.types.components(ty).unwrap().len() as u32;
                    let mut result = self.fb.ins().iconst(types::I8, 1);
                    for index in 0..count {
                        let slot = self.cg.slot(ty, Proj::Field(index));
                        let same = self.equal_slot(a, b, &slot);
                        result = self.fb.ins().band(result, same);
                    }
                    result
                }
                _ => self.call_glue(Glue::Equal, ty, &[a, b]).unwrap(),
            },
            _ => self.fb.ins().iconst(types::I8, 1),
        }
    }

    fn equal_slot(&mut self, a: ir::Value, b: ir::Value, slot: &Slot) -> ir::Value {
        let left = self.component_address(a, slot);
        let right = self.component_address(b, slot);
        if slot.boxed {
            return self.call_glue(Glue::Equal, slot.ty, &[left, right]).unwrap();
        }
        let left = self.value_at(left, slot.ty);
        let right = self.value_at(right, slot.ty);
        self.equal(slot.ty, left, right)
    }
}

/// Defines a glue function: retain, release or compare values of `ty` given
/// their addresses.
fn define_glue(cg: &mut Codegen, kind: Glue, ty: Ty, func: &mut ir::Function, fbc: &mut FunctionBuilderContext) {
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit { cg, fb };
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let params = e.fb.block_params(entry).to_vec();
    let enum_shape = match e.cg.layout(ty).shape {
        Shape::Enum { payload, variants, .. } => Some((payload, variants.len() as u32)),
        _ => None,
    };
    match (kind, enum_shape) {
        (Glue::Retain | Glue::Release, None) => {
            let value = e.value_at(params[0], ty);
            e.refcount(ty, value, kind == Glue::Release);
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
    e: Emit<'a, 'b>,
    body: &'a Body,
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
                    let align = layout.align.trailing_zeros() as u8;
                    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size, align);
                    Storage::Slot(fb.create_sized_stack_slot(data))
                }
            });
        }
        FnCodegen { e: Emit { cg, fb }, body, blocks, storage }
    }

    fn run(mut self) {
        let entry = self.blocks[0];
        self.e.fb.append_block_params_for_function_params(entry);
        self.e.fb.switch_to_block(entry);
        let mut incoming = self.e.fb.block_params(entry).to_vec().into_iter();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            self.e.fb.def_var(var, incoming.next().unwrap());
        }
        for &param in &self.body.params {
            match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.e.fb.def_var(var, incoming.next().unwrap()),
                Storage::Zero | Storage::Slot(_) => {}
            }
        }
        for (index, block) in self.body.blocks.iter().enumerate() {
            if index > 0 {
                self.e.fb.switch_to_block(self.blocks[index]);
            }
            for stmt in &block.stmts {
                self.statement(stmt);
            }
            self.terminator(&block.terminator);
        }
        self.e.fb.seal_all_blocks();
        let config = self.e.cg.module.isa().frontend_config();
        self.e.fb.finalize(config);
    }

    // ---- places and values ---------------------------------------------------

    fn place_ty(&self, place: &Place) -> Ty {
        self.body.place_ty(self.e.cg.types, place)
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

    /// The address of a place inside an aggregate local. When `write` is
    /// set, every box on the way is made unique first (copy-on-write).
    fn pointer_to(&mut self, place: &Place, write: bool) -> ir::Value {
        let mut base = match self.storage[place.local.0 as usize] {
            Storage::Slot(slot) => self.e.fb.ins().stack_addr(POINTER, slot, 0),
            Storage::Pointer(var) => self.e.fb.use_var(var),
            Storage::Var(_) | Storage::Zero => unreachable!("address of a non-aggregate"),
        };
        let mut ty = self.body.locals[place.local.0 as usize];
        for &proj in &place.path {
            let slot = self.e.cg.slot(ty, proj);
            if slot.boxed && write {
                let cell = self.e.offset(base, slot.offset);
                let retain = self.e.glue_address(Glue::Retain, slot.ty);
                let release = self.e.glue_address(Glue::Release, slot.ty);
                self.e.call_rt(Rt::BoxUnique, &[cell, retain, release]);
            }
            base = self.e.component_address(base, &slot);
            ty = slot.ty;
        }
        base
    }

    fn read(&mut self, place: &Place) -> Val {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
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

    fn write(&mut self, place: &Place, value: Val) {
        if let (Val::Scalar(value), Storage::Var(var)) = (value, self.storage[place.local.0 as usize])
            && place.path.is_empty()
        {
            self.e.fb.def_var(var, value);
            return;
        }
        if let Val::Zero = value {
            return;
        }
        let address = self.pointer_to(place, true);
        let ty = self.place_ty(place);
        self.e.store_val(address, ty, value);
    }

    fn operand(&mut self, operand: &Operand) -> Val {
        match operand {
            Operand::Copy(place) => self.read(place),
            Operand::Const(Const::Int(value, ty)) => {
                let clif = self.e.cg.scalar_type(*ty);
                // Narrow immediates are zero-extended bit patterns.
                let bits = clif.bits();
                let pattern = if bits == 64 { *value as i64 } else { (*value as i64) & ((1i64 << bits) - 1) };
                Val::Scalar(self.e.fb.ins().iconst(clif, pattern))
            }
            Operand::Const(Const::Bool(value)) => Val::Scalar(self.e.fb.ins().iconst(types::I8, *value as i64)),
            Operand::Const(Const::Str(text)) => Val::Scalar(self.e.string_ptr(text)),
            Operand::Const(Const::Unit) => Val::Zero,
        }
    }

    fn scalar(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar operand"),
        }
    }

    fn location(&mut self, span: Span) -> ir::Value {
        let location = self.e.cg.sources.location(span);
        self.e.string_ptr(&location)
    }

    // ---- statements and terminators ----------------------------------------

    fn statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(place, rvalue, span) => self.assign(place, rvalue, *span),
            Statement::Retain(place) | Statement::Release(place) => {
                let ty = self.place_ty(place);
                if !self.e.cg.types.needs_drop(ty) {
                    return;
                }
                let value = self.read(place);
                self.e.refcount(ty, value, matches!(stmt, Statement::Release(_)));
            }
        }
    }

    fn assign(&mut self, place: &Place, rvalue: &Rvalue, span: Span) {
        let ty = self.place_ty(place);
        let class = self.e.cg.class(ty);
        if class == Class::Zero {
            // Zero-sized values carry no data; only calls have effects.
            if let Rvalue::Call(callee, args) = rvalue {
                self.call(*callee, args, None);
            }
            return;
        }
        let value = match rvalue {
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
            Rvalue::Variant(variant, components) => {
                let tag_ty = self.e.cg.types.tag_ty(ty);
                let tag = self.operand(&Operand::Const(Const::Int(*variant as i128, tag_ty)));
                if let Class::Scalar(_) = class {
                    tag
                } else {
                    let base = self.pointer_to(place, true);
                    self.e.store_val(base, tag_ty, tag);
                    for (index, component) in components.iter().enumerate() {
                        let slot = self.e.cg.slot(ty, Proj::Variant(*variant, index as u32));
                        let value = self.operand(component);
                        self.e.init_component(base, &slot, value);
                    }
                    return;
                }
            }
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
                let func = self.e.cg.module.declare_func_in_func(self.e.cg.functions[id.0 as usize], self.e.fb.func);
                let call = self.e.fb.ins().call(func, &values);
                self.e.fb.inst_results(call).first().copied()
            }
            Callee::Std(StdFn::ConsolePrint) => self.e.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.e.call_rt(Rt::ConsolePrintln, &values),
        }
    }

    fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.e.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                let condition = self.scalar(condition);
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                self.e.fb.ins().brif(condition, then, &[], otherwise, &[]);
            }
            Terminator::Return(value) => match self.operand(value) {
                Val::Scalar(value) => {
                    self.e.fb.ins().return_(&[value]);
                }
                // Aggregates were already written through the result pointer.
                Val::Memory(_) | Val::Zero => {
                    self.e.fb.ins().return_(&[]);
                }
            },
            Terminator::Panic { code, message, span } => {
                let message = match message {
                    Some(message) => self.scalar(message),
                    None => self.e.fb.ins().iconst(POINTER, 0),
                };
                self.emit_panic(*code, message, *span);
            }
            Terminator::Unreachable => {
                self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
    }

    fn emit_panic(&mut self, code: u32, message: ir::Value, span: Span) {
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let location = self.location(span);
        self.e.call_rt(Rt::Panic, &[code, message, location]);
        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
    }

    /// Branches to a cold block that panics with `code` when `condition` holds.
    fn panic_if(&mut self, condition: ir::Value, code: u32, span: Span) {
        let (failed, next) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.set_cold_block(failed);
        self.e.fb.ins().brif(condition, failed, &[], next, &[]);
        self.e.fb.switch_to_block(failed);
        let null = self.e.fb.ins().iconst(POINTER, 0);
        self.emit_panic(code, null, span);
        self.e.fb.switch_to_block(next);
    }

    // ---- operators -----------------------------------------------------------

    fn unary(&mut self, op: UnaryOp, operand: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.scalar(operand);
        match op {
            UnaryOp::Not => self.e.fb.ins().bxor_imm_u(value, 1),
            UnaryOp::BitNot => self.e.fb.ins().bnot(value),
            UnaryOp::Neg => {
                let clif = self.e.cg.scalar_type(ty);
                let zero = self.e.fb.ins().iconst(clif, 0);
                let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, value);
                self.panic_if(overflow, 5, span);
                result
            }
        }
    }

    fn concat(&mut self, pieces: &[Piece]) -> ir::Value {
        let builder = self.e.call_rt(Rt::SbNew, &[]).unwrap();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    let text = self.e.string_ptr(text);
                    self.e.call_rt(Rt::SbStr, &[builder, text]);
                }
                Piece::Value(operand, ty) => {
                    let value = self.scalar(operand);
                    match self.e.cg.types.kind(self.e.cg.types.repr(*ty)) {
                        TyKind::String => {
                            self.e.call_rt(Rt::SbStr, &[builder, value]);
                        }
                        TyKind::Bool => {
                            self.e.call_rt(Rt::SbBool, &[builder, value]);
                        }
                        TyKind::Int(int) => {
                            let wide = match (int.bits(), int.signed()) {
                                (64, _) => value,
                                (_, true) => self.e.fb.ins().sextend(types::I64, value),
                                (_, false) => self.e.fb.ins().uextend(types::I64, value),
                            };
                            let rt = if int.signed() { Rt::SbI64 } else { Rt::SbU64 };
                            self.e.call_rt(rt, &[builder, wide]);
                        }
                        other => unreachable!("no Display for {other:?}"),
                    }
                }
            }
        }
        self.e.call_rt(Rt::SbFinish, &[builder]).unwrap()
    }

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let (a, b) = (self.operand(left), self.operand(right));
            let equal = self.e.equal(ty, a, b);
            return if op == BinOp::Eq { equal } else { self.e.fb.ins().bxor_imm_u(equal, 1) };
        }
        let (a, b) = (self.scalar(left), self.scalar(right));
        if self.e.cg.types.repr(ty) == Ty::STRING {
            let order = self.e.call_rt(Rt::StrCmp, &[a, b]).unwrap();
            return self.e.fb.ins().icmp_imm_s(int_condition(op, true), order, 0);
        }
        let types = self.e.cg.types;
        let signed = types.as_int(ty).is_some_and(IntTy::signed);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let ins = self.e.fb.ins();
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
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.panic_if(zero, 3, span);
                if !signed {
                    return if op == BinOp::Div { self.e.fb.ins().udiv(a, b) } else { self.e.fb.ins().urem(a, b) };
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let int = types.as_int(ty).unwrap();
                let is_min = self.e.fb.ins().icmp_imm_s(IntCC::Equal, a, int.min() as i64);
                let is_minus_one = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let both = self.e.fb.ins().band(is_min, is_minus_one);
                if op == BinOp::Div {
                    self.panic_if(both, 5, span);
                    self.e.fb.ins().sdiv(a, b)
                } else {
                    let one = self.e.fb.ins().iconst(int_type(int), 1);
                    let divisor = self.e.fb.ins().select(both, one, b);
                    self.e.fb.ins().srem(a, divisor)
                }
            }
            BinOp::BitAnd => self.e.fb.ins().band(a, b),
            BinOp::BitOr => self.e.fb.ins().bor(a, b),
            BinOp::BitXor => self.e.fb.ins().bxor(a, b),
            BinOp::Shl | BinOp::Shr => {
                let width = types.as_int(ty).unwrap().bits() as i64;
                if types.as_int(right_ty).is_some_and(IntTy::signed) {
                    let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                    self.panic_if(negative, 10, span);
                }
                let too_wide = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b, width);
                self.panic_if(too_wide, 10, span);
                match (op, signed) {
                    (BinOp::Shl, _) => self.e.fb.ins().ishl(a, b),
                    (_, true) => self.e.fb.ins().sshr(a, b),
                    (_, false) => self.e.fb.ins().ushr(a, b),
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => self.e.fb.ins().icmp(int_condition(op, signed), a, b),
            BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
                unreachable!("handled above or lowered before codegen")
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
