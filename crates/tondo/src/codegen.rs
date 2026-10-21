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
use crate::hir::{ArrayOp, Callee, MapOp, Mutation, Program, StdFn, StrOp};
use crate::mir::{Body, Const, Local, Operand, Piece, Place, Proj, Rvalue, Statement, Terminator, project};
use crate::source::{SourceMap, Span};
use crate::types::{AdtKind, ERR, FloatTy, IntTy, OK, SOME, Ty, TyKind, Types};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
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
/// Size of an array header in the runtime (`ArrayHeader`): `len` is at 8 and
/// a deque's `head` at 24. An array's `head` is always 0.
const ARRAY_HEADER: i64 = 32;

/// Runtime entry points (see `tondo-runtime`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Rt {
    StrRetain,
    StrRelease,
    StrEq,
    StrCmp,
    StrLength,
    StrGet,
    StrAt,
    StrSlice,
    StrConcat,
    StrDecode,
    StrNext,
    StrTest,
    StrFind,
    StrReplace,
    StrTransform,
    StrSliceChecked,
    StrFromChars,
    SbNew,
    SbStr,
    SbI64,
    SbU64,
    SbBool,
    SbF64,
    SbF32,
    SbChar,
    SbFinish,
    ConsolePrint,
    ConsolePrintln,
    ConsoleFlush,
    Panic,
    BoxNew,
    BoxRetain,
    BoxRelease,
    BoxUnique,
    ReportError,
    ArrayNew,
    ArrayRetain,
    ArrayRelease,
    ArrayUnique,
    ArrayPush,
    ArrayPop,
    ArraySlice,
    ArraySliceChecked,
    ArrayConcat,
    ArrayRepeat,
    ArrayEq,
    ArrayContains,
    ArrayAssignSlice,
    ArrayArith,
    ArrayInsert,
    ArrayRemoveAt,
    ArrayClear,
    DequePush,
    DequePopFront,
    SbArray,
    MapNew,
    MapRetain,
    MapRelease,
    MapFind,
    MapInsert,
    MapRemove,
    MapLen,
    MapEntry,
    MapEq,
    MapClear,
    SbMap,
    StrHash,
}

impl Rt {
    fn signature(self) -> (&'static str, &'static [ir::Type], &'static [ir::Type]) {
        use types::{I8, I32, I64};
        match self {
            Rt::StrRetain => ("tondo_rt_str_retain", &[I64], &[]),
            Rt::StrRelease => ("tondo_rt_str_release", &[I64], &[]),
            Rt::StrEq => ("tondo_rt_str_eq", &[I64, I64], &[I8]),
            Rt::StrCmp => ("tondo_rt_str_cmp", &[I64, I64], &[I32]),
            Rt::StrLength => ("tondo_rt_str_length", &[I64], &[I64]),
            Rt::StrGet => ("tondo_rt_str_get", &[I64, I64], &[I64]),
            Rt::StrAt => ("tondo_rt_str_at", &[I64, I64, I64], &[I32]),
            Rt::StrSlice => ("tondo_rt_str_slice", &[I64, I64, I8, I64, I8, I64, I64], &[I64]),
            Rt::StrConcat => ("tondo_rt_str_concat", &[I64, I64], &[I64]),
            Rt::StrDecode => ("tondo_rt_str_decode", &[I64, I64], &[I32]),
            Rt::StrNext => ("tondo_rt_str_next", &[I64, I64], &[I64]),
            Rt::StrTest => ("tondo_rt_str_test", &[I64, I64, I32], &[I8]),
            Rt::StrFind => ("tondo_rt_str_find", &[I64, I64], &[I64]),
            Rt::StrReplace => ("tondo_rt_str_replace", &[I64, I64, I64], &[I64]),
            Rt::StrTransform => ("tondo_rt_str_transform", &[I64, I32], &[I64]),
            Rt::StrSliceChecked => ("tondo_rt_str_slice_checked", &[I64, I64, I64, I64], &[I64]),
            Rt::StrFromChars => ("tondo_rt_str_from_chars", &[I64], &[I64]),
            Rt::SbNew => ("tondo_rt_sb_new", &[], &[I64]),
            Rt::SbStr => ("tondo_rt_sb_str", &[I64, I64], &[]),
            Rt::SbI64 => ("tondo_rt_sb_i64", &[I64, I64], &[]),
            Rt::SbU64 => ("tondo_rt_sb_u64", &[I64, I64], &[]),
            Rt::SbBool => ("tondo_rt_sb_bool", &[I64, I8], &[]),
            Rt::SbF64 => ("tondo_rt_sb_f64", &[I64, types::F64], &[]),
            Rt::SbF32 => ("tondo_rt_sb_f32", &[I64, types::F32], &[]),
            Rt::SbChar => ("tondo_rt_sb_char", &[I64, I32], &[]),
            Rt::SbFinish => ("tondo_rt_sb_finish", &[I64], &[I64]),
            Rt::ConsolePrint => ("tondo_rt_console_print", &[I64], &[]),
            Rt::ConsolePrintln => ("tondo_rt_console_println", &[I64], &[]),
            Rt::ConsoleFlush => ("tondo_rt_console_flush", &[], &[I64]),
            Rt::Panic => ("tondo_rt_panic", &[I32, I64, I64], &[]),
            Rt::BoxNew => ("tondo_rt_box_new", &[I64], &[I64]),
            Rt::BoxRetain => ("tondo_rt_box_retain", &[I64], &[]),
            Rt::BoxRelease => ("tondo_rt_box_release", &[I64, I64], &[]),
            Rt::BoxUnique => ("tondo_rt_box_unique", &[I64, I64, I64], &[]),
            Rt::ReportError => ("tondo_rt_report_error", &[I64], &[]),
            Rt::ArrayNew => ("tondo_rt_array_new", &[I64, I64], &[I64]),
            Rt::ArrayRetain => ("tondo_rt_array_retain", &[I64], &[]),
            Rt::ArrayRelease => ("tondo_rt_array_release", &[I64, I64, I64], &[]),
            Rt::ArrayUnique => ("tondo_rt_array_unique", &[I64, I64, I64, I64, I64], &[]),
            Rt::ArrayPush => ("tondo_rt_array_push", &[I64, I64, I64, I64, I64], &[]),
            Rt::ArrayPop => ("tondo_rt_array_pop", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::ArraySlice => ("tondo_rt_array_slice", &[I64, I64, I64, I64, I8, I64, I8, I64, I64], &[I64]),
            Rt::ArraySliceChecked => ("tondo_rt_array_slice_checked", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayConcat => ("tondo_rt_array_concat", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayRepeat => ("tondo_rt_array_repeat", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayEq => ("tondo_rt_array_eq", &[I64, I64, I64, I64], &[I8]),
            Rt::ArrayContains => ("tondo_rt_array_contains", &[I64, I64, I64, I64], &[I8]),
            Rt::ArrayAssignSlice => {
                ("tondo_rt_array_assign_slice", &[I64, I64, I8, I64, I8, I64, I64, I64, I64, I64, I64], &[])
            }
            Rt::ArrayArith => ("tondo_rt_array_arith", &[I64, I32, I64, I32, I32, I32, I64], &[I64]),
            Rt::ArrayInsert => ("tondo_rt_array_insert", &[I64, I64, I64, I64, I64, I64, I64], &[]),
            Rt::ArrayRemoveAt => ("tondo_rt_array_remove_at", &[I64, I64, I64, I64, I64, I64], &[I8]),
            Rt::ArrayClear => ("tondo_rt_array_clear", &[I64, I64, I64], &[]),
            Rt::DequePush => ("tondo_rt_deque_push", &[I64, I64, I64, I8, I64, I64], &[]),
            Rt::DequePopFront => ("tondo_rt_deque_pop_front", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::SbArray => ("tondo_rt_sb_array", &[I64, I64, I64, I64], &[]),
            Rt::MapNew => ("tondo_rt_map_new", &[I64, I64], &[I64]),
            Rt::MapRetain => ("tondo_rt_map_retain", &[I64], &[]),
            Rt::MapRelease => ("tondo_rt_map_release", &[I64, I64], &[]),
            Rt::MapFind => ("tondo_rt_map_find", &[I64, I64, I64], &[I64]),
            Rt::MapInsert => ("tondo_rt_map_insert", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::MapRemove => ("tondo_rt_map_remove", &[I64, I64, I64, I64], &[I8]),
            Rt::MapLen => ("tondo_rt_map_len", &[I64], &[I64]),
            Rt::MapEntry => ("tondo_rt_map_entry", &[I64, I64], &[I64]),
            Rt::MapEq => ("tondo_rt_map_eq", &[I64, I64, I64, I64], &[I8]),
            Rt::MapClear => ("tondo_rt_map_clear", &[I64, I64], &[]),
            Rt::SbMap => ("tondo_rt_sb_map", &[I64, I64, I64, I64], &[]),
            Rt::StrHash => ("tondo_rt_str_hash", &[I64], &[I64]),
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
    /// Appends the `Display` text of a value to a string builder.
    Display,
    /// Hashes a `Key` value.
    Hash,
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
        map_glue: HashMap::new(),
        loans: bodies.iter().map(|body| body.loans.clone()).collect(),
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
    let main = program.main.0 as usize;
    cg.entry_point(main, bodies[main].result, &mut context, &mut builder_context)?;
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
    map_glue: HashMap<Ty, DataId>,
    /// Which parameters of each function are loans (`Body::loans`).
    loans: Vec<Vec<bool>>,
}

impl Codegen<'_> {
    fn class(&mut self, ty: Ty) -> Class {
        let repr = self.types.repr(ty);
        match self.types.kind(repr) {
            TyKind::Unit | TyKind::Never | TyKind::Error => Class::Zero,
            TyKind::Bool => Class::Scalar(types::I8),
            TyKind::Int(int) => Class::Scalar(int_type(*int)),
            TyKind::Float(FloatTy::F32) => Class::Scalar(types::F32),
            TyKind::Float(FloatTy::F64) => Class::Scalar(types::F64),
            TyKind::Char => Class::Scalar(types::I32),
            TyKind::Byte => Class::Scalar(types::I8),
            TyKind::String | TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_) => {
                Class::Scalar(POINTER)
            }
            TyKind::Tuple(_) | TyKind::Adt(_) | TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) => {
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
        for (index, &param) in body.params.iter().enumerate() {
            match self.class(body.locals[param.0 as usize]) {
                // A loan is the address of the caller's place.
                _ if body.loans[index] => signature.params.push(AbiParam::new(POINTER)),
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
        if kind == Glue::Display {
            signature.params.push(AbiParam::new(POINTER));
        }
        if kind == Glue::Equal {
            signature.params.push(AbiParam::new(POINTER));
            signature.returns.push(AbiParam::new(types::I8));
        }
        if kind == Glue::Hash {
            signature.returns.push(AbiParam::new(types::I64));
        }
        signature
    }

    /// The element type of an array or deque, and whether it is a deque.
    fn sequence_parts(&self, ty: Ty) -> (Ty, bool) {
        match *self.types.kind(self.types.repr(ty)) {
            TyKind::Array(element) => (element, false),
            TyKind::Deque(element) => (element, true),
            _ => unreachable!("not an array or deque"),
        }
    }

    /// The key and value of a map or set: `(K, V)`, with `Unit` for sets.
    fn map_parts(&self, ty: Ty) -> (Ty, Ty) {
        match *self.types.kind(self.types.repr(ty)) {
            TyKind::Map(key, value) => (key, value),
            TyKind::Set(element) => (element, Ty::UNIT),
            _ => unreachable!("not a map or set"),
        }
    }

    /// Entry size and value offset, laid out like the tuple `(K, V)`.
    fn entry_layout(&mut self, ty: Ty) -> (u32, u32) {
        let (key, value) = self.map_parts(ty);
        let (key, value) = (self.layout(key), self.layout(value));
        let offset = key.size.next_multiple_of(value.align);
        ((offset + value.size).next_multiple_of(key.align.max(value.align)), offset)
    }

    /// The static table of glue functions the runtime uses for a map or set.
    fn map_glue(&mut self, ty: Ty) -> DataId {
        let ty = self.types.repr(ty);
        if let Some(&id) = self.map_glue.get(&ty) {
            return id;
        }
        let (key, value) = self.map_parts(ty);
        let name = format!("tondo_map_glue{}", self.map_glue.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("glue table");
        let mut data = DataDescription::new();
        data.define(vec![0u8; 48].into_boxed_slice());
        data.set_align(8);
        let entries = [
            (Glue::Hash, key, true),
            (Glue::Equal, key, true),
            (Glue::Retain, key, self.types.needs_drop(key)),
            (Glue::Release, key, self.types.needs_drop(key)),
            (Glue::Retain, value, self.types.needs_drop(value)),
            (Glue::Release, value, self.types.needs_drop(value)),
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
        // The C ABI as LLVM implements it expects narrow integer arguments
        // extended to 32 bits by the caller.
        signature.params.extend(
            params.iter().map(
                |&t| {
                    if t.is_int() && t.bits() < 32 { AbiParam::new(t).uext() } else { AbiParam::new(t) }
                },
            ),
        );
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
    /// `tondo_main`, called by the runtime's C `main`. It returns the process
    /// status: 0, or 1 after reporting an error that a fallible `main`
    /// returned (20.10).
    fn entry_point(
        &mut self,
        main: usize,
        result: Ty,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let mut signature = self.module.make_signature();
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("tondo_main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 0);
        {
            let fb = FunctionBuilder::new(&mut context.func, fbc);
            let mut e = Emit { cg: self, fb };
            let block = e.fb.create_block();
            e.fb.switch_to_block(block);
            let callee = e.cg.module.declare_func_in_func(e.cg.functions[main], e.fb.func);
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
                    e.fb.ins().call(callee, &[address]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = e.fb.ins().call(callee, &[]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    e.fb.ins().call(callee, &[]);
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
                let description = e.describe(error, error_value);
                e.call_rt(Rt::ReportError, &[description]);
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

    /// A glue function's address, or null when a retain or release of `ty`
    /// has nothing to do.
    fn glue_or_null(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        if matches!(kind, Glue::Retain | Glue::Release) && !self.cg.types.needs_drop(ty) {
            return self.fb.ins().iconst(POINTER, 0);
        }
        self.glue_address(kind, ty)
    }

    fn map_glue_address(&mut self, ty: Ty) -> ir::Value {
        let id = self.cg.map_glue(ty);
        let global = self.cg.module.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
    }

    /// Mixes `value` into a running hash.
    fn mix(&mut self, hash: ir::Value, value: ir::Value) -> ir::Value {
        let combined = self.fb.ins().bxor(hash, value);
        self.fb.ins().imul_imm_s(combined, 0x100_0000_01b3)
    }

    /// A 64-bit hash of a `Key` value (8.12).
    fn hash(&mut self, ty: Ty, value: Val) -> ir::Value {
        let seed = self.fb.ins().iconst(types::I64, 0xcbf2_9ce4_8422_2325u64 as i64);
        match value {
            Val::Zero => seed,
            Val::Scalar(value) => {
                let word = match *self.cg.types.kind(self.cg.types.repr(ty)) {
                    TyKind::String => self.call_rt(Rt::StrHash, &[value]).unwrap(),
                    TyKind::Int(int) if int.signed() && int.bits() < 64 => self.fb.ins().sextend(types::I64, value),
                    _ => match self.cg.scalar_type(ty).bits() {
                        64 => value,
                        _ => self.fb.ins().uextend(types::I64, value),
                    },
                };
                self.mix(seed, word)
            }
            Val::Memory(address) => match self.cg.layout(ty).shape {
                Shape::Struct(_) => {
                    let count = self.cg.types.components(ty).unwrap().len() as u32;
                    let mut hash = seed;
                    for index in 0..count {
                        let slot = self.cg.slot(ty, Proj::Field(index));
                        let part = self.component_address(address, &slot);
                        let part = if slot.boxed {
                            self.call_glue(Glue::Hash, slot.ty, &[part]).unwrap()
                        } else {
                            let value = self.value_at(part, slot.ty);
                            self.hash(slot.ty, value)
                        };
                        hash = self.mix(hash, part);
                    }
                    hash
                }
                _ => self.call_glue(Glue::Hash, ty, &[address]).unwrap(),
            },
        }
    }

    fn element_size(&mut self, element: Ty) -> ir::Value {
        let size = self.cg.layout(element).size;
        self.fb.ins().iconst(POINTER, size as i64)
    }

    /// Appends the `Display` text of `value` to `builder`.
    fn append_display(&mut self, builder: ir::Value, ty: Ty, value: Val) {
        let Val::Scalar(value) = value else { unreachable!("no Display for aggregates") };
        match *self.cg.types.kind(self.cg.types.repr(ty)) {
            TyKind::String => {
                self.call_rt(Rt::SbStr, &[builder, value]);
            }
            TyKind::Bool => {
                self.call_rt(Rt::SbBool, &[builder, value]);
            }
            TyKind::Float(FloatTy::F64) => {
                self.call_rt(Rt::SbF64, &[builder, value]);
            }
            TyKind::Float(FloatTy::F32) => {
                self.call_rt(Rt::SbF32, &[builder, value]);
            }
            TyKind::Char => {
                self.call_rt(Rt::SbChar, &[builder, value]);
            }
            TyKind::Byte => {
                let wide = self.fb.ins().uextend(types::I64, value);
                self.call_rt(Rt::SbU64, &[builder, wide]);
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
            TyKind::Array(element) | TyKind::Deque(element) => {
                let size = self.element_size(element);
                let display = self.glue_address(Glue::Display, element);
                self.call_rt(Rt::SbArray, &[builder, value, size, display]);
            }
            TyKind::Map(key, entry) => {
                let key = self.glue_address(Glue::Display, key);
                let entry = self.glue_address(Glue::Display, entry);
                self.call_rt(Rt::SbMap, &[builder, value, key, entry]);
            }
            TyKind::Set(element) => {
                let element = self.glue_address(Glue::Display, element);
                let none = self.fb.ins().iconst(POINTER, 0);
                self.call_rt(Rt::SbMap, &[builder, value, element, none]);
            }
            ref other => unreachable!("no Display for {other:?}"),
        }
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

    /// A string describing an error that reached the end of `main`: its text
    /// for a `String`, `Type.Variant` for an enum, otherwise the type name.
    fn describe(&mut self, ty: Ty, value: Val) -> ir::Value {
        let types = self.cg.types;
        let repr = types.repr(ty);
        if repr == Ty::STRING
            && let Val::Scalar(text) = value
        {
            return text;
        }
        if let Some(members) = types.members(repr) {
            // Describe the member the union holds.
            let tag = self.tag(ty, value);
            let done = self.fb.create_block();
            self.fb.append_block_param(done, POINTER);
            for (variant, &member) in members.iter().enumerate() {
                let (this, next) = (self.fb.create_block(), self.fb.create_block());
                let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                self.fb.ins().brif(is_this, this, &[], next, &[]);
                self.fb.switch_to_block(this);
                let payload = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Variant(variant as u32, 0));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, member)
                    }
                    _ => Val::Zero,
                };
                let description = self.describe(member, payload);
                self.fb.ins().jump(done, &[description.into()]);
                self.fb.switch_to_block(next);
            }
            let fallback = self.string_ptr(&types.display(ty));
            self.fb.ins().jump(done, &[fallback.into()]);
            self.fb.switch_to_block(done);
            return self.fb.block_params(done)[0];
        }
        let names: Vec<String> = match types.kind(repr) {
            TyKind::Adt(id) => match &types.adt(*id).kind {
                AdtKind::Enum(variants) => {
                    variants.iter().map(|v| format!("{}.{}", types.adt(*id).name, v.name)).collect()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        if names.is_empty() || matches!(value, Val::Zero) {
            return self.string_ptr(&types.display(ty));
        }
        let tag = self.tag(ty, value);
        let mut description = self.string_ptr(&names[0]);
        for (index, name) in names.iter().enumerate().skip(1) {
            let text = self.string_ptr(name);
            let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
            description = self.fb.ins().select(is_this, text, description);
        }
        description
    }

    /// Retains or releases every counted value inside `value`.
    fn refcount(&mut self, ty: Ty, value: Val, release: bool) {
        if !self.cg.types.needs_drop(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => match *self.cg.types.kind(self.cg.types.repr(ty)) {
                TyKind::Array(element) | TyKind::Deque(element) if release => {
                    let size = self.element_size(element);
                    let glue = self.glue_or_null(Glue::Release, element);
                    self.call_rt(Rt::ArrayRelease, &[pointer, size, glue]);
                }
                TyKind::Array(_) | TyKind::Deque(_) => {
                    self.call_rt(Rt::ArrayRetain, &[pointer]);
                }
                TyKind::Map(..) | TyKind::Set(_) if release => {
                    let glue = self.map_glue_address(ty);
                    self.call_rt(Rt::MapRelease, &[pointer, glue]);
                }
                TyKind::Map(..) | TyKind::Set(_) => {
                    self.call_rt(Rt::MapRetain, &[pointer]);
                }
                _ => {
                    self.call_rt(if release { Rt::StrRelease } else { Rt::StrRetain }, &[pointer]);
                }
            },
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
            (Val::Scalar(a), Val::Scalar(b)) => match *self.cg.types.kind(self.cg.types.repr(ty)) {
                TyKind::String => self.call_rt(Rt::StrEq, &[a, b]).unwrap(),
                TyKind::Array(element) | TyKind::Deque(element) => {
                    let size = self.element_size(element);
                    let eq = self.glue_address(Glue::Equal, element);
                    self.call_rt(Rt::ArrayEq, &[a, b, size, eq]).unwrap()
                }
                TyKind::Map(_, value) => {
                    let glue = self.map_glue_address(ty);
                    let value_eq = self.glue_address(Glue::Equal, value);
                    self.call_rt(Rt::MapEq, &[a, b, glue, value_eq]).unwrap()
                }
                TyKind::Set(_) => {
                    let glue = self.map_glue_address(ty);
                    let none = self.fb.ins().iconst(POINTER, 0);
                    self.call_rt(Rt::MapEq, &[a, b, glue, none]).unwrap()
                }
                // IEEE equality: NaN equals nothing (8.11).
                TyKind::Float(_) => self.fb.ins().fcmp(FloatCC::Equal, a, b),
                _ => self.fb.ins().icmp(IntCC::Equal, a, b),
            },
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
        (Glue::Display, _) => {
            let value = e.value_at(params[1], ty);
            e.append_display(params[0], ty, value);
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
            let loan = body.params.iter().position(|&p| p == local).is_some_and(|i| body.loans[i]);
            storage.push(match cg.class(ty) {
                Class::Zero => Storage::Zero,
                _ if loan => Storage::Pointer(fb.declare_var(POINTER)),
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
        for (index, &param) in self.body.params.iter().enumerate() {
            match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.e.fb.def_var(var, incoming.next().unwrap()),
                // A loan of a zero-sized place still has its address parameter.
                Storage::Zero if self.body.loans[index] => {
                    incoming.next();
                }
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
            Operand::Const(Const::Int(_, ty) | Const::Float(_, ty)) => *ty,
            Operand::Const(Const::Char(_)) => Ty::CHAR,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
            Operand::Const(Const::Unit) => Ty::UNIT,
        }
    }

    /// The address of a place inside an aggregate local. When `write` is
    /// set, every box on the way is made unique first (copy-on-write).
    fn pointer_to(&mut self, place: &Place, write: bool) -> ir::Value {
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
                && matches!(self.e.cg.types.kind(self.e.cg.types.repr(ty)), TyKind::Map(..) | TyKind::Set(_))
            {
                // Iteration over a map or set: the entry at a valid position.
                let map = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                let position = self.scalar(&Operand::local(index));
                base = Some(self.e.call_rt(Rt::MapEntry, &[map, position]).unwrap());
                ty = project(self.e.cg.types, ty, proj);
                continue;
            }
            if let Proj::Index(index, span) = proj {
                let element = project(self.e.cg.types, ty, proj);
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
                base = Some(self.element_address(sequence, ty, index, span));
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
    fn element_address(&mut self, sequence: ir::Value, ty: Ty, index: ir::Value, span: Span) -> ir::Value {
        let (position, inside) = self.position(sequence, index);
        let outside = self.e.fb.ins().bxor_imm_u(inside, 1);
        self.panic_if(outside, 1, span);
        self.element_at(sequence, ty, position)
    }

    /// An index as a position, counting from the end when negative (10.3),
    /// and whether that position holds an element.
    fn position(&mut self, sequence: ir::Value, index: ir::Value) -> (ir::Value, ir::Value) {
        let length = self.e.fb.ins().load(types::I64, flags(), sequence, 8);
        let wrapped = self.e.fb.ins().iadd(index, length);
        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
        let position = self.e.fb.ins().select(negative, wrapped, index);
        let inside = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, position, length);
        (position, inside)
    }

    /// The address of the element at a valid position of an array or deque.
    fn element_at(&mut self, sequence: ir::Value, ty: Ty, position: ir::Value) -> ir::Value {
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
    fn slice_range(&mut self, parts: &[Option<Operand>; 3], span: Span) -> [ir::Value; 6] {
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
    fn assign_slice(&mut self, target: &Place, parts: &[Option<Operand>; 3], value: &Operand, span: Span) {
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

    /// Lifted arithmetic over arrays (10.6), done by the runtime.
    fn array_arithmetic(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let mut args = Vec::new();
        let mut number = Ty::ERROR;
        for operand in [left, right] {
            let mut ty = self.operand_ty(operand);
            let value = self.operand(operand);
            args.push(self.in_memory(ty, value));
            let mut depth = 0;
            while let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) {
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
        let code = match *self.e.cg.types.kind(self.e.cg.types.repr(number)) {
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
    fn scratch(&mut self, ty: Ty) -> ir::Value {
        let size = self.e.cg.layout(ty).size.max(1);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_addr(POINTER, slot, 0)
    }

    /// Makes the array stored at `slot` unique before a write (copy-on-write).
    fn make_unique(&mut self, slot: ir::Value, element: Ty) {
        let size = self.e.element_size(element);
        let extra = self.e.fb.ins().iconst(POINTER, 0);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        self.e.call_rt(Rt::ArrayUnique, &[slot, size, extra, retain, release]);
    }

    /// Stores a variable into a fresh stack slot so the runtime can update it.
    fn spill(&mut self, var: Variable) -> ir::Value {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let value = self.e.fb.use_var(var);
        self.e.fb.ins().store(flags(), value, address, 0);
        address
    }

    fn unspill(&mut self, var: Variable, address: ir::Value) {
        let value = self.e.fb.ins().load(POINTER, flags(), address, 0);
        self.e.fb.def_var(var, value);
    }

    /// The address of the storage holding the array at `place`, spilled to
    /// the stack when it lives in a variable; `unspill` it after the call.
    fn array_slot(&mut self, place: &Place) -> (ir::Value, Option<Variable>) {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            return (self.spill(var), Some(var));
        }
        (self.pointer_to(place, true), None)
    }

    /// The address of a value, spilling a scalar to the stack.
    fn in_memory(&mut self, ty: Ty, value: Val) -> ir::Value {
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
            // Zero-sized values carry no data; only calls and mutations have
            // effects.
            match rvalue {
                Rvalue::Call(callee, args) => {
                    self.call(*callee, args, None);
                }
                Rvalue::Mutate(mutation, target, operands, retain, span) => {
                    self.mutate(*mutation, target, operands, retain, place, *span);
                }
                Rvalue::AssignSlice(target, bounds, value, span) => self.assign_slice(target, bounds, value, *span),
                _ => {}
            }
            return;
        }
        let value = match rvalue {
            Rvalue::Call(callee @ Callee::Std(_), args) => {
                let status = self.call(*callee, args, None).unwrap();
                self.std_result(place, ty, status);
                return;
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
            Rvalue::Convert(operand) => {
                let from = self.operand_ty(operand);
                let value = self.scalar(operand);
                match self.e.cg.types.kind(ty) {
                    TyKind::Result(target, _) => {
                        let target = *target;
                        let (converted, error) = self.convert_checked(value, from, target);
                        self.write_checked(place, ty, converted, error);
                        return;
                    }
                    _ => Val::Scalar(self.convert_total(value, from, ty)),
                }
            }
            Rvalue::Array(values, retain) => {
                let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) else { unreachable!() };
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
                    let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) else {
                        unreachable!()
                    };
                    let size = self.e.element_size(element);
                    let retain = self.e.glue_or_null(Glue::Retain, element);
                    self.e.call_rt(Rt::ArraySlice, &[&[sequence, size, retain], &range[..]].concat()).unwrap()
                })
            }
            Rvalue::Length(collection) => {
                let collection_ty = self.operand_ty(collection);
                let value = self.scalar(collection);
                Val::Scalar(match self.e.cg.types.kind(self.e.cg.types.repr(collection_ty)) {
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
                    StrOp::Trim | StrOp::LowerAscii | StrOp::UpperAscii => {
                        let transform = match op {
                            StrOp::Trim => 0,
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

    /// An intrinsic array operation. Returns the value for scalar results;
    /// results in memory are written to `place` directly.
    fn array_op(&mut self, op: ArrayOp, operands: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        let types = self.e.cg.types;
        let element_of = |ty: Ty| match *types.kind(types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) => element,
            _ => unreachable!("not an array or deque"),
        };
        Some(match op {
            ArrayOp::New => {
                let size = self.e.element_size(element_of(ty));
                let zero = self.e.fb.ins().iconst(POINTER, 0);
                Val::Scalar(self.e.call_rt(Rt::ArrayNew, &[size, zero]).unwrap())
            }
            ArrayOp::WithCapacity => {
                let TyKind::Result(array_ty, _) = *types.kind(ty) else { unreachable!() };
                let capacity = self.scalar(&operands[0]);
                let size = self.e.element_size(element_of(array_ty));
                let zero = self.e.fb.ins().iconst(POINTER, 0);
                // The capacity is a hint; storage grows on demand.
                let array = self.e.call_rt(Rt::ArrayNew, &[size, zero]).unwrap();
                let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, capacity, 0);
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
    fn mutate(
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
        if let TyKind::Map(..) | TyKind::Set(_) = self.e.cg.types.kind(self.e.cg.types.repr(collection_ty)) {
            return self.mutate_map(mutation, target, &values, collection_ty, place, ty);
        }
        let (element, deque) = self.e.cg.sequence_parts(collection_ty);
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

    /// The address of a copy of `value` on the stack.
    fn copy_to_stack(&mut self, ty: Ty, value: Val) -> ir::Value {
        let Val::Memory(source) = value else { return self.in_memory(ty, value) };
        let address = self.scratch(ty);
        let size = self.e.cg.layout(ty).size;
        self.e.copy_memory(address, source, size);
        address
    }

    /// `insert`, `remove` or `clear` on a map or set.
    fn mutate_map(
        &mut self,
        mutation: Mutation,
        target: &Place,
        values: &[Val],
        map_ty: Ty,
        place: &Place,
        ty: Ty,
    ) -> Option<Val> {
        let (key, value) = self.e.cg.map_parts(map_ty);
        let is_set = matches!(self.e.cg.types.kind(self.e.cg.types.repr(map_ty)), TyKind::Set(_));
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

    /// The `T?` result of a removal: `some` with the element the runtime
    /// moved to `out` when `found` is 1, otherwise `none`.
    fn removal_result(&mut self, place: &Place, ty: Ty, element: Ty, out: ir::Value, found: ir::Value) -> Option<Val> {
        if self.e.cg.class(ty) != Class::Memory {
            // The element has no size: the option is just its tag.
            return Some(Val::Scalar(found));
        }
        let base = self.pointer_to(place, true);
        self.e.fb.ins().store(flags(), found, base, 0);
        let (some, done) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(found, some, &[], done, &[]);
        self.e.fb.switch_to_block(some);
        let value = self.e.value_at(out, element);
        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
        self.e.init_component(base, &payload, value);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
        None
    }

    fn scalar_of(&mut self, value: Val) -> ir::Value {
        match value {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar"),
        }
    }

    /// Builds the `!ConsoleError` result of a console operation from the
    /// runtime's status: 0 for `ok`, 1 plus a variant without payload, or 4
    /// plus the `IoError` of `ConsoleError.Io` (variant 3).
    fn std_result(&mut self, place: &Place, ty: Ty, status: ir::Value) {
        const IO_VARIANT: i64 = 3;
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.e.fb.ins().store(flags(), failed, base, 0);
        let TyKind::Result(_, error) = *self.e.cg.types.kind(ty) else { unreachable!() };
        let slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        let error_base = self.e.offset(base, slot.offset);
        let io = self.e.fb.ins().icmp_imm_s(IntCC::SignedGreaterThan, status, IO_VARIANT);
        let plain = self.e.fb.ins().iadd_imm_s(status, -1);
        let io_tag = self.e.fb.ins().iconst(types::I64, IO_VARIANT);
        let tag = self.e.fb.ins().select(io, io_tag, plain);
        let tag = self.e.fb.ins().ireduce(types::I8, tag);
        self.e.fb.ins().store(flags(), tag, error_base, 0);
        // The payload area is unused by the other variants.
        let payload = self.e.cg.slot(error, Proj::Variant(IO_VARIANT as u32, 0));
        let io_error = self.e.fb.ins().iadd_imm_s(status, -(IO_VARIANT + 1));
        let io_error = self.e.fb.ins().ireduce(types::I8, io_error);
        self.e.fb.ins().store(flags(), io_error, error_base, payload.offset as i32);
    }

    /// A conversion that cannot fail (18.6).
    fn convert_total(&mut self, value: ir::Value, from: Ty, to: Ty) -> ir::Value {
        let types = self.e.cg.types;
        let to_clif = self.e.cg.scalar_type(to);
        let from_clif = self.e.cg.scalar_type(from);
        match (types.kind(from), types.kind(to)) {
            (TyKind::Float(_), TyKind::Float(_)) if from_clif == to_clif => value,
            (TyKind::Float(_), TyKind::Float(_)) => self.e.fb.ins().fpromote(to_clif, value),
            (_, TyKind::Float(_)) if types.as_int(from).is_some_and(IntTy::signed) => {
                self.e.fb.ins().fcvt_from_sint(to_clif, value)
            }
            (_, TyKind::Float(_)) => self.e.fb.ins().fcvt_from_uint(to_clif, value),
            _ => self.resize(value, from, to_clif),
        }
    }

    /// Resizes an integer bit pattern, extending by the source's signedness.
    fn resize(&mut self, value: ir::Value, from: Ty, to: ir::Type) -> ir::Value {
        let from_clif = self.e.cg.scalar_type(from);
        let signed = self.e.cg.types.as_int(from).is_some_and(IntTy::signed);
        if from_clif.bits() == to.bits() {
            value
        } else if from_clif.bits() > to.bits() {
            self.e.fb.ins().ireduce(to, value)
        } else if signed {
            self.e.fb.ins().sextend(to, value)
        } else {
            self.e.fb.ins().uextend(to, value)
        }
    }

    /// A conversion that can fail: the converted value (meaningful only on
    /// success) and the error variant, or -1 for success.
    fn convert_checked(&mut self, value: ir::Value, from: Ty, to: Ty) -> (ir::Value, ir::Value) {
        const OUT_OF_RANGE: i64 = 0;
        const NOT_FINITE: i64 = 1;
        const NOT_INTEGRAL: i64 = 2;
        let types = self.e.cg.types;
        let to_clif = self.e.cg.scalar_type(to);
        let range = |ty: Ty| match types.kind(ty) {
            TyKind::Int(int) => (int.min(), int.max()),
            _ => (0, 255),
        };
        let ok = self.e.fb.ins().iconst(types::I8, -1);
        let pick = |e: &mut Emit, failed: ir::Value, code: i64, current: ir::Value| {
            let code = e.fb.ins().iconst(types::I8, code);
            e.fb.ins().select(failed, code, current)
        };
        match types.kind(from) {
            TyKind::Float(_) if matches!(types.kind(to), TyKind::Float(_)) => {
                // Float64 to Float32: a finite value that rounds to infinity fails.
                let narrowed = self.e.fb.ins().fdemote(to_clif, value);
                let infinite_before = self.is_infinite(value, types::F64);
                let infinite_after = self.is_infinite(narrowed, types::F32);
                let finite_before = self.e.fb.ins().bxor_imm_u(infinite_before, 1);
                let overflowed = self.e.fb.ins().band(finite_before, infinite_after);
                let error = pick(&mut self.e, overflowed, OUT_OF_RANGE, ok);
                (narrowed, error)
            }
            TyKind::Float(_) => {
                // Classified in order: NotFinite, NotIntegral, OutOfRange (18.6).
                let from_clif = self.e.cg.scalar_type(from);
                let (min, max) = range(to);
                let not_nan = self.e.fb.ins().fcmp(FloatCC::Equal, value, value);
                let infinite = self.is_infinite(value, from_clif);
                let finite = self.e.fb.ins().bxor_imm_u(infinite, 1);
                let finite = self.e.fb.ins().band(finite, not_nan);
                let truncated = self.e.fb.ins().trunc(value);
                let integral = self.e.fb.ins().fcmp(FloatCC::Equal, truncated, value);
                // `max + 1` and `min` are powers of two, exact in both formats.
                let low = self.float_const(from_clif, min as f64);
                let high = self.float_const(from_clif, (max + 1) as f64);
                let below = self.e.fb.ins().fcmp(FloatCC::LessThan, value, low);
                let above = self.e.fb.ins().fcmp(FloatCC::GreaterThanOrEqual, value, high);
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                let not_integral = self.e.fb.ins().bxor_imm_u(integral, 1);
                let error = pick(&mut self.e, not_integral, NOT_INTEGRAL, error);
                let not_finite = self.e.fb.ins().bxor_imm_u(finite, 1);
                let error = pick(&mut self.e, not_finite, NOT_FINITE, error);
                // Convert a safe placeholder when the value is invalid.
                let invalid = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
                let zero = self.float_const(from_clif, 0.0);
                let safe = self.e.fb.ins().select(invalid, zero, value);
                // x86 converts to 32 or 64 bits only; the range is already checked.
                let wide = if to_clif.bits() < 32 { types::I32 } else { to_clif };
                let converted = if min < 0 {
                    self.e.fb.ins().fcvt_to_sint(wide, safe)
                } else {
                    self.e.fb.ins().fcvt_to_uint(wide, safe)
                };
                let converted = if wide == to_clif { converted } else { self.e.fb.ins().ireduce(to_clif, converted) };
                (converted, error)
            }
            _ => {
                let (min, max) = range(to);
                let wide = self.resize(value, from, types::I64);
                let signed = types.as_int(from).is_some_and(IntTy::signed);
                // Compare in the source's domain, widened to 64 bits.
                let (below, above) = if signed {
                    let below =
                        self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, wide, min.max(i64::MIN as i128) as i64);
                    let above = if max >= i64::MAX as i128 {
                        self.e.fb.ins().iconst(types::I8, 0)
                    } else {
                        self.e.fb.ins().icmp_imm_s(IntCC::SignedGreaterThan, wide, max as i64)
                    };
                    (below, above)
                } else {
                    let below = self.e.fb.ins().iconst(types::I8, 0);
                    let above = if max >= u64::MAX as i128 {
                        self.e.fb.ins().iconst(types::I8, 0)
                    } else {
                        self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThan, wide, max as i64)
                    };
                    (below, above)
                };
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                (self.resize(value, from, to_clif), error)
            }
        }
    }

    fn float_const(&mut self, ty: ir::Type, value: f64) -> ir::Value {
        if ty == types::F32 { self.e.fb.ins().f32const(value as f32) } else { self.e.fb.ins().f64const(value) }
    }

    fn is_infinite(&mut self, value: ir::Value, ty: ir::Type) -> ir::Value {
        let magnitude = self.e.fb.ins().fabs(value);
        let infinity = self.float_const(ty, f64::INFINITY);
        self.e.fb.ins().fcmp(FloatCC::Equal, magnitude, infinity)
    }

    /// Writes `ok(value)` or `err(NumericConversionError)` into `place`.
    fn write_checked(&mut self, place: &Place, ty: Ty, value: ir::Value, error: ir::Value) {
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
        self.e.fb.ins().store(flags(), failed, base, 0);
        // Both payloads share the same storage: write only the active one.
        let (failure, success, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, failure, &[], success, &[]);
        self.e.fb.switch_to_block(failure);
        let err_slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        self.e.fb.ins().store(flags(), error, base, err_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(success);
        let ok_slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
        self.e.fb.ins().store(flags(), value, base, ok_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    fn call(&mut self, callee: Callee, args: &[Operand], result: Option<ir::Value>) -> Option<ir::Value> {
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        let loans = match callee {
            Callee::Fn(id) => self.e.cg.loans[id.0 as usize].clone(),
            Callee::Std(_) => Vec::new(),
        };
        let mut spilled = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if loans.get(index).copied().unwrap_or(false) {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                values.push(self.loan_address(place, &mut spilled));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) | Val::Memory(value) => values.push(value),
                Val::Zero => {}
            }
        }
        let result = match callee {
            Callee::Fn(id) => {
                let func = self.e.cg.module.declare_func_in_func(self.e.cg.functions[id.0 as usize], self.e.fb.func);
                let call = self.e.fb.ins().call(func, &values);
                self.e.fb.inst_results(call).first().copied()
            }
            Callee::Std(StdFn::ConsolePrint) => self.e.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.e.call_rt(Rt::ConsolePrintln, &values),
            Callee::Std(StdFn::ConsoleFlush) => self.e.call_rt(Rt::ConsoleFlush, &values),
        };
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
    }

    /// The address a loan of `place` passes; a local held in a variable is
    /// spilled to the stack and recorded in `spilled` to reload after the call.
    fn loan_address(&mut self, place: &Place, spilled: &mut Vec<(Variable, ir::Value)>) -> ir::Value {
        if self.e.cg.class(self.place_ty(place)) == Class::Zero {
            return self.e.fb.ins().iconst(POINTER, 0);
        }
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            let slot = self.spill(var);
            spilled.push((var, slot));
            return slot;
        }
        self.pointer_to(place, true)
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
            UnaryOp::Neg if matches!(self.e.cg.types.kind(ty), TyKind::Float(_)) => self.e.fb.ins().fneg(value),
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
                    let value = self.operand(operand);
                    self.e.append_display(builder, *ty, value);
                }
            }
        }
        self.e.call_rt(Rt::SbFinish, &[builder]).unwrap()
    }

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        let is_array = |ty: Ty| matches!(self.e.cg.types.kind(self.e.cg.types.repr(ty)), TyKind::Array(_));
        if !matches!(op, BinOp::Eq | BinOp::Ne) && (is_array(ty) || is_array(right_ty)) {
            return self.array_arithmetic(op, left, right, span);
        }
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
        if let TyKind::Float(_) = types.kind(ty) {
            // IEEE 754 without traps (18.5); relational comparisons with NaN are false.
            let ins = self.e.fb.ins();
            return match op {
                BinOp::Add => ins.fadd(a, b),
                BinOp::Sub => ins.fsub(a, b),
                BinOp::Mul => ins.fmul(a, b),
                BinOp::Div => ins.fdiv(a, b),
                BinOp::Lt => ins.fcmp(FloatCC::LessThan, a, b),
                BinOp::Le => ins.fcmp(FloatCC::LessThanOrEqual, a, b),
                BinOp::Gt => ins.fcmp(FloatCC::GreaterThan, a, b),
                BinOp::Ge => ins.fcmp(FloatCC::GreaterThanOrEqual, a, b),
                _ => unreachable!("rejected by the checker"),
            };
        }
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
                let width = self.e.cg.scalar_type(ty).bits() as i64;
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
