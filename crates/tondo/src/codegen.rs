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
use crate::hir::{
    ArrayOp, AtomicOp, Callee, EnvFn, FnId, FsFn, IoFn, LockMode, MapOp, Mutation, PathFn, PointerOp, ProcessFn,
    Program, ReflectOp, SharedOp, StdFn, StrOp, TaskOp,
};
use crate::mir::{
    Body, Const, Local, Operand, Origin, Overlap, Piece, Place, Proj, Rvalue, SelectCase, Statement, Terminator,
    project,
};
use crate::numbers::NumOp;
use crate::source::{SourceMap, Span};
use crate::types::{AdtKind, ERR, FloatTy, Handle, IntTy, Mode, OK, SOME, Shown, Ty, TyKind, Types};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    self, AbiParam, ArgumentPurpose, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, TrapCode, UserFuncName,
    types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use std::collections::HashSet;

mod cabi;
use cabi::{CTarget, Callback, define_adapter, define_inner};
mod calls;
mod collections;
mod control;
mod coverage;
mod entry;
mod frames;
mod glue;
mod live;
mod operators;
mod places;
mod runtime;
mod statements;
mod tables;
mod tasks;
mod values;
mod worker;
use glue::*;
use live::*;
pub(crate) use operators::by_range;
pub(crate) use places::in_data;
use runtime::*;

const POINTER: ir::Type = types::I64;
const IMMORTAL: u64 = u64::MAX;
/// Size of a function value's header in the runtime (`FnObject`): the code
/// is at 16 and the captures follow the header.
const FN_HEADER: i64 = 40;
/// Size of a box header in the runtime (`BoxHeader`).
const BOX_HEADER: i64 = 16;
/// The size of the header of a `Ref[T]`'s object: its strong and weak
/// counts, the size of the value that follows, and whether it is garbage.
const REF_HEADER: i64 = 32;
/// Where the value starts in the box of a `reflect.Value` (`BoxObject`).
const REFLECT_VALUE: i64 = 40;
/// Size of an array header in the runtime (`ArrayHeader`): `len` is at 8 and
/// a deque's `head` at 24. An array's `head` is always 0.
const ARRAY_HEADER: i64 = 32;
/// The bytes a frame keeps for a text made there (`stack.rs`), after its
/// header: fewer than those of a text whose positions the runtime keeps by
/// its address, which a frame's room reuses.
const FRAMED_ROOM: u32 = 56;
/// Where a `std.sync` cell (`SyncObject`) keeps the address of its value,
/// after its count and its state.
const SYNC_VALUE: i32 = 16;
/// Where a string's length is in its header (`Str`), after its count, its
/// room after that, and where its bytes start.
const STRING_LENGTH: i32 = 8;
const STRING_ROOM: i32 = 16;
const STRING_BYTES: i64 = 24;
/// The count of a static array: so many references that releases never
/// bring it to one, so that a change copies it, as one of a shared array.
const SHARED_FOREVER: u64 = 1 << 62;

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
    /// `T?` of an address that is never zero: the address, or zero for
    /// `none`, as C writes a null pointer.
    Niche,
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

/// What the tools show for what is `@secret`.
const SECRET: &str = "<secret>";

fn int_type(int: IntTy) -> ir::Type {
    match int.bits() {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        64 => types::I64,
        _ => types::I128,
    }
}

/// `value` in LEB128: seven bits a byte, the lowest first.
fn leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn symbol_name(index: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("tondo_f{index}_{clean}")
}

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

/// How an object is built: `meta` counts a step at each call and each turn
/// of a loop, for a provider or generator (27.7); a debug build for a
/// debugger; for another machine; live; or counting what runs, with what
/// each counter counts.
pub struct Options<'o> {
    pub meta: bool,
    pub debug: bool,
    pub target: Option<&'o str>,
    pub live: Option<&'o mut crate::live::Build>,
    pub coverage: Option<&'o mut Vec<Counted>>,
    /// The build `tondo test --fuzz` searches with (testing 10.6): it counts
    /// the dependencies' code too, and tells the runtime what the counted
    /// code compares.
    pub fuzz: bool,
    /// Where the program comes from (`roots.rs`), or nothing.
    pub roots: &'o str,
}

/// Compiles the program to an object file.
pub fn compile(program: &Program, bodies: &[Body], sources: &SourceMap, options: Options) -> Result<Vec<u8>, String> {
    let building = crate::times::part("IR");
    let Options { meta, debug, target, mut live, coverage, fuzz, roots } = options;
    let mut flags = settings::builder();
    // A debug build keeps the code as written, statement after statement.
    flags.set("opt_level", if debug { "none" } else { "speed" }).map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    // Integers of 128 bits pass as C passes them, in two registers.
    flags.set("enable_llvm_abi_extensions", "true").map_err(|e| e.to_string())?;
    // Cranelift checks the IR of every function it compiles, more than
    // once: the debug build of tondo, which the tests run, keeps checking,
    // and the one that ships compiles without it (docs/speed.md, B1).
    flags.set("enable_verifier", if cfg!(debug_assertions) { "true" } else { "false" }).map_err(|e| e.to_string())?;
    // Code for the machine, with what its processor has, or for a target.
    let builder = match target {
        Some(triple) => cranelift_codegen::isa::lookup_by_name(triple).map_err(|e| format!("{triple}: {e}"))?,
        None => cranelift_native::builder().map_err(|e| format!("unsupported host: {e}"))?,
    };
    let isa = builder.finish(settings::Flags::new(flags)).map_err(|e| e.to_string())?;
    let shapes = Shapes::new(&program.types, &*isa);
    let builder =
        ObjectBuilder::new(isa, "tondo", cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    let mut cg = Codegen {
        roots,
        module: ObjectModule::new(builder),
        types: &program.types,
        sources,
        meta,
        functions: Vec::new(),
        runtime: HashMap::new(),
        strings: HashMap::new(),
        arrays: 0,
        empty_array: None,
        byte_arrays: HashMap::new(),
        shapes,
        glue: HashMap::new(),
        pending_glue: Vec::new(),
        map_glue: HashMap::new(),
        identities: HashMap::new(),
        loans: bodies.iter().map(|body| body.loans.clone()).collect(),
        display: &program.display,
        views: &program.views,
        fn_values: HashMap::new(),
        pending_thunks: Vec::new(),
        externs: &program.externs,
        c_functions: HashMap::new(),
        callbacks: HashMap::new(),
        pending_callbacks: Vec::new(),
        library: None,
        traced: Vec::new(),
        debug: None,
        debug_functions: Vec::new(),
        debug_slots: HashMap::new(),
        debug_sites: Vec::new(),
        dump: None,
        dump_bindings: HashMap::new(),
        live: None,
        current: 0,
        takes_osr: bodies.iter().map(|body| body.osr.is_some()).collect(),
        forwarders: HashMap::new(),
        pending_forwarders: Vec::new(),
        coverage: None,
        fuzz,
        testing: !program.tests.is_empty(),
        contexts: program
            .contexts
            .iter()
            .map(|fns| (fns.clone(), program.types.handle_result(bodies[fns.maker.0 as usize].result)))
            .collect(),
    };
    if coverage.is_some() {
        let counters =
            cg.module.declare_data("tondo_coverage", Linkage::Local, true, false).map_err(|e| e.to_string())?;
        cg.coverage = Some((counters, Vec::new()));
    }

    if debug {
        let table = cg.module.declare_data("tondo_debug", Linkage::Local, false, false).map_err(|e| e.to_string())?;
        cg.debug = Some(table);
    }
    // Any other program says what its dumps show.
    if !debug && cg.live.is_none() && coverage.is_none() && !Gen::fuzzing(&cg) {
        let table = cg.module.declare_data("tondo_dump", Linkage::Local, false, false).map_err(|e| e.to_string())?;
        cg.dump = Some(table);
    }
    for (index, body) in bodies.iter().enumerate() {
        let signature = cg.signature(body);
        let id = cg
            .module
            .declare_function(&symbol_name(index, &body.name), Linkage::Local, &signature)
            .map_err(|e| e.to_string())?;
        cg.functions.push(id);
    }
    if let Some(build) = live.as_deref_mut() {
        cg.live = Some(LiveCodegen::new(&mut cg.module, build, bodies, &program.types)?);
    }
    for (index, export) in program.exports.iter().enumerate() {
        cg.export(index, export)?;
    }
    let mut context = cg.module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    // The IR of every function is built on every core, and what each asks
    // to declare is declared in the functions' order (see `worker`);
    // `compile_all` compiles them on every core.
    let mut jobs = Vec::new();
    let mut job = |id: FuncId, context: &mut cranelift_codegen::Context, traced: Option<&str>, sites, inlined, body| {
        let func = std::mem::replace(&mut context.func, ir::Function::new());
        jobs.push(Job { id, func, traced: traced.map(str::to_string), sites, inlined, body });
    };
    let built = {
        let shared = worker::Shared::new(&cg);
        let indices: Vec<usize> = (0..bodies.len()).collect();
        crate::parallel::map(&indices, 1, |&index| worker::build(&shared, bodies, index))
    };
    for (index, (body, mut built)) in bodies.iter().zip(built).enumerate() {
        cg.current = index;
        // The runner that calls the tests is not the program's; a test's
        // suites are, and a debugger shows their frames.
        let runner = !program.tests.is_empty() && program.main.is_some_and(|main| index == main.0 as usize);
        let traced = (!runner).then_some(body.name.as_str());
        if built.serial {
            context.func.signature = cg.signature(body);
            context.func.name = UserFuncName::user(0, index as u32);
            let sites = FnCodegen::new(&mut cg, body, bodies, &mut context.func, &mut builder_context).run();
            job(cg.functions[index], &mut context, traced, sites, body.inlined.clone(), index);
        } else {
            worker::replay(&mut cg, bodies, &mut built);
            context.func = built.func;
            job(cg.functions[index], &mut context, traced, built.sites, body.inlined.clone(), index);
        }
        cg.module.clear_context(&mut context);
    }
    let trace = cg.module.declare_data("tondo_trace", Linkage::Local, false, false).map_err(|e| e.to_string())?;
    let patch = cg.live.as_ref().is_some_and(|live| live.patch.is_some());
    match program.main {
        // A patch starts nothing: the running program calls into it.
        _ if patch => {}
        Some(main) => {
            let main = main.0 as usize;
            let entry =
                cg.entry_point(main, bodies[main].result, &program.tests, trace, &mut context, &mut builder_context)?;
            job(entry, &mut context, None, Vec::new(), Vec::new(), 0);
            cg.module.clear_context(&mut context);
            let start = cg.process_main(entry, &mut context, &mut builder_context)?;
            job(start, &mut context, None, Vec::new(), Vec::new(), 0);
        }
        None => {
            let init = cg.library_start(trace, &mut context, &mut builder_context)?;
            job(init, &mut context, None, Vec::new(), Vec::new(), 0);
            cg.library = Some(init);
        }
    }
    cg.module.clear_context(&mut context);
    while let Some((target, thunk)) = cg.pending_thunks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(thunk).signature.clone();
        context.func.name = UserFuncName::user(3, thunk.as_u32());
        define_thunk(&mut cg, &bodies[target.0 as usize], target, &mut context.func, &mut builder_context);
        // In the table of the trace, without lines, for its handler.
        job(thunk, &mut context, Some(""), Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
    }
    while let Some((target, forward)) = cg.pending_forwarders.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(forward).signature.clone();
        context.func.name = UserFuncName::user(6, forward.as_u32());
        define_forwarder(&mut cg, target, &mut context.func, &mut builder_context);
        job(forward, &mut context, Some(""), Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
    }
    while let Some(callback) = cg.pending_callbacks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(callback.adapter).signature.clone();
        context.func.name = UserFuncName::user(4, callback.adapter.as_u32());
        define_adapter(&mut cg, &callback, &mut context.func, &mut builder_context);
        job(callback.adapter, &mut context, None, Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
        context.func.signature = cg.module.declarations().get_function_decl(callback.inner).signature.clone();
        context.func.name = UserFuncName::user(5, callback.inner.as_u32());
        define_inner(&mut cg, &bodies[callback.target.0 as usize], &callback, &mut context.func, &mut builder_context);
        job(callback.inner, &mut context, Some(&bodies[callback.target.0 as usize].name), Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
    }
    // A patch's conversions of the running program's values, which the
    // runtime calls as frames move, with the glue that releases what they
    // convert from and to.
    for function in live.as_ref().map(|build| build.migrations.clone()).unwrap_or_default() {
        let body = &bodies[function.0 as usize];
        let mut signature = cg.module.make_signature();
        signature.params.extend([AbiParam::new(POINTER), AbiParam::new(POINTER)]);
        signature.returns.push(AbiParam::new(types::I8));
        let name = format!("tondo_convert{}", function.0);
        let id = cg.module.declare_function(&name, Linkage::Local, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(7, id.as_u32());
        define_conversion(&mut cg, body, function, &mut context.func, &mut builder_context);
        job(id, &mut context, None, Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
        let TyKind::Option(new) = *cg.types.kind(body.result) else { unreachable!("a conversion gives `T?`") };
        let old = body.locals[body.params[0].0 as usize];
        let mut release = |ty: Ty| cg.types.needs_drop(ty).then(|| cg.glue(Glue::Release, ty));
        let entry = (id, release(old), release(new));
        let index = cg.live.as_ref().expect("a live build").conversions.len();
        cg.live.as_mut().expect("a live build").conversions.push(entry);
        if live.as_ref().is_some_and(|build| build.objects.contains(&index)) {
            let (from, to) = (cg.identity(old), cg.identity(new));
            let layout = cg.layout(new);
            let visit = cg.types.can_cycle(new).then(|| cg.glue(Glue::Visit, new));
            let object = (index, from, to, layout.size, layout.align, entry.2, visit);
            cg.live.as_mut().expect("a live build").objects.push(object);
        }
    }
    while let Some((kind, ty, id)) = cg.pending_glue.pop() {
        context.func.signature = cg.glue_signature(kind);
        context.func.name = UserFuncName::user(2, id.as_u32());
        define_glue(&mut cg, kind, ty, &mut context.func, &mut builder_context);
        job(id, &mut context, None, Vec::new(), Vec::new(), 0);
        cg.module.clear_context(&mut context);
    }
    drop(building);
    let compiling = crate::times::part("Cranelift");
    cg.compile_all(jobs)?;
    drop(compiling);
    let _writing = crate::times::part("object");

    if let (Some(out), Some((counters, counted))) = (coverage, cg.coverage.take()) {
        let mut data = cranelift_module::DataDescription::new();
        data.define_zeroinit(counted.len().max(1) * 8);
        cg.module.define_data(counters, &data).map_err(|e| e.to_string())?;
        *out = counted;
    }
    cg.trace_table(trace);
    if let Some(debug) = cg.debug {
        cg.debug_table(debug);
    }
    if let Some(dump) = cg.dump {
        cg.dump_table(dump);
    }
    if let Some(build) = live {
        cg.live_points(build, bodies);
        cg.live_tables(build, trace)?;
    }
    // The DWARF of a debug build, for gdb and lldb.
    let debugged: Vec<_> = match debug {
        true => cg.traced.iter().map(|function| (function.id, cg.dwarf_rows(function))).collect(),
        false => Vec::new(),
    };
    let mut product = cg.module.finish();
    if debug {
        let functions: Vec<crate::debuginfo::Function> = debugged
            .into_iter()
            .zip(&cg.traced)
            .map(|((id, rows), function)| crate::debuginfo::Function {
                symbol: product.function_symbol(id),
                name: function.name.clone(),
                size: function.size,
                rows,
            })
            .collect();
        crate::debuginfo::write(&mut product.object, "program", &functions)?;
    }
    product.emit().map_err(|e| e.to_string())
}

struct Codegen<'a> {
    /// Where the program comes from, which the runtime gives to tools.
    roots: &'a str,
    module: ObjectModule,
    types: &'a Types,
    /// Each context's functions and the type of its record (11.16).
    contexts: Vec<(crate::hir::ContextFns, Ty)>,
    sources: &'a SourceMap,
    /// Counts steps, for the `meta` profile.
    meta: bool,
    functions: Vec<FuncId>,
    runtime: HashMap<Rt, FuncId>,
    strings: HashMap<String, DataId>,
    /// How many static arrays there are, which names the next.
    arrays: usize,
    /// The one empty array, of any element type, that `[]` and `Array[T]()` are.
    empty_array: Option<DataId>,
    /// The static arrays of embedded files, by their bytes.
    byte_arrays: HashMap<std::sync::Arc<[u8]>, DataId>,
    shapes: Shapes<'a>,
    glue: HashMap<(Glue, Ty), FuncId>,
    pending_glue: Vec<(Glue, Ty, FuncId)>,
    map_glue: HashMap<Ty, DataId>,
    /// The identity of each type a `reflect.Value` holds: a static with its
    /// name.
    identities: HashMap<Ty, DataId>,
    /// Which parameters of each function are loans (`Body::loans`).
    loans: Vec<Vec<bool>>,
    /// The user `Display.display` of types displayed inside collections.
    display: &'a HashMap<Ty, FnId>,
    views: &'a HashMap<Ty, (FnId, Ty)>,
    /// The static `fn(...)` object of each named function used as a value,
    /// and its thunk with the uniform calling convention.
    fn_values: HashMap<FnId, (DataId, FuncId)>,
    pending_thunks: Vec<(FnId, FuncId)>,
    /// The `extern fn` declarations, and the C functions imported so far by
    /// symbol, with the signature of their import.
    externs: &'a [crate::hir::Extern],
    c_functions: HashMap<String, (FuncId, ir::Signature)>,
    /// The C function that runs a function or closure body for C, by the
    /// `extern fn(...)` type it has, and those still to define with the
    /// function that makes the call from a frame.
    callbacks: HashMap<(FnId, Ty), FuncId>,
    pending_callbacks: Vec<Callback>,
    /// In a library, what starts it: the first call from C runs it.
    library: Option<FuncId>,
    /// The functions of the stack trace table, with their calls.
    traced: Vec<Traced>,
    /// A debug build's probes (toolchain 7.6): each function's name and
    /// the bindings it shows, and each probe's source and function.
    debug: Option<DataId>,
    /// Each function's name, whether it takes a loan of its caller's (a
    /// `mut` or `var` parameter, or a closure's environment), and its
    /// bindings.
    debug_functions: Vec<(String, bool, Vec<DebugBinding>)>,
    /// Where each debug function's slots are, by its number.
    debug_slots: HashMap<u32, (i64, i64)>,
    /// A debug build's probes: where each is, its function, and whether its
    /// line goes on there from the code before.
    debug_sites: Vec<(u32, u32, bool)>,
    /// A build without probes: the table of what its dumps show, and the
    /// bindings of each function, by its number.
    dump: Option<DataId>,
    dump_bindings: HashMap<usize, Vec<DumpBinding>>,
    live: Option<LiveCodegen>,
    /// The body whose IR is being built.
    current: usize,
    /// Which bodies take a live build's hidden parameter.
    takes_osr: Vec<bool>,
    /// In a live build, what closures call their bodies through.
    forwarders: HashMap<FnId, FuncId>,
    pending_forwarders: Vec<(FnId, FuncId)>,
    /// A test build that counts what runs (testing 10.3): its counters,
    /// and what each counts.
    coverage: Option<(DataId, Vec<Counted>)>,
    /// A build for the search of `--fuzz`.
    fuzz: bool,
    testing: bool,
}

/// What a counter of a test build with coverage counts (testing 10.3):
/// the runs of a line of the project's code, the calls of a function, or
/// the times a branch went one way, with where its branch is and what the
/// way is called.
#[derive(Clone, Debug)]
pub enum Counted {
    Line { file: String, line: usize },
    Function { file: String, line: usize, name: String },
    Way { file: String, line: usize, column: usize, branch: (usize, usize), label: String },
}

/// Calls one of the program's functions: directly, or in a live build
/// through its slot, which a patch may point at new code.
fn call_body<'t, G: Gen<'t> + ?Sized>(cg: &mut G, fb: &mut FunctionBuilder, id: FnId, args: &[ir::Value]) -> ir::Inst {
    call_body_with(cg, fb, id, args, None)
}

/// A call with the state a frame brings to the new code, or none.
fn call_body_with<'t, G: Gen<'t> + ?Sized>(
    cg: &mut G,
    fb: &mut FunctionBuilder,
    id: FnId,
    args: &[ir::Value],
    state: Option<ir::Value>,
) -> ir::Inst {
    let mut args = args.to_vec();
    if cg.takes_osr(id) {
        args.push(state.unwrap_or_else(|| fb.ins().iconst(types::I64, 0)));
    }
    let args = &args[..];
    let func = cg.function(id);
    let Some((table, slot)) = cg.slot_of(id) else {
        let callee = cg.declare_func_in_func(func, fb.func);
        return fb.ins().call(callee, args);
    };
    // A frame that moves enters the code its state is for, which a patch
    // that comes meanwhile does not change.
    let code = match state {
        Some(state) => fb.ins().load(POINTER, flags(), state, 8),
        None => {
            let table = cg.declare_data_in_func(table, fb.func);
            let base = fb.ins().symbol_value(POINTER, table);
            fb.ins().load(POINTER, flags(), base, (slot * 8) as i32)
        }
    };
    let signature = cg.function_signature(func);
    let signature = fb.import_signature(signature);
    fb.ins().call_indirect(signature, code, args)
}

/// For each block, the block before it when only one jumps to it.
fn only_before(blocks: &[crate::mir::BlockData]) -> Vec<Option<usize>> {
    let mut before: Vec<(usize, Option<usize>)> = vec![(0, None); blocks.len()];
    let mut jump = |from: usize, to: &crate::mir::BlockId| {
        let target = &mut before[to.0 as usize];
        *target = (target.0 + 1, Some(from));
    };
    for (index, block) in blocks.iter().enumerate() {
        match &block.terminator {
            Terminator::Goto(next) => jump(index, next),
            Terminator::Branch(_, then, otherwise) => {
                jump(index, then);
                jump(index, otherwise);
            }
            _ => {}
        }
    }
    before.into_iter().map(|(count, from)| from.filter(|_| count == 1)).collect()
}

/// A binding a debug build's probe shows: its name, its type and the glue
/// that shows it.
/// A binding the debugger shows: its name, type, how it shows, and for a
/// type with a view, how it shows as it is.
type DebugBinding = (String, Ty, FuncId, Option<FuncId>);

/// A binding of a build without probes, as a dump shows it: its name, its
/// type, the glue that shows it, where it is, its place among those an
/// explicit panic keeps, and the inlined call it is of.
type DumpBinding = (String, Ty, FuncId, DumpPlace, Option<u32>, Option<u32>, Option<Span>);

/// Where a dump finds a binding: in what a value label says, in a stack
/// slot, or behind a pointer a value label says.
#[derive(Clone, Copy)]
enum DumpPlace {
    Value(u32),
    Slot(u32),
    Pointer(u32),
}

/// A compiled function for stack traces: its name, size and, for each
/// call, the offset it returns to and where its source starts; and where
/// the code of each statement starts, for profiles.
struct Traced {
    id: FuncId,
    name: String,
    size: u32,
    calls: Vec<(u32, u32)>,
    handlers: Vec<(u32, u32)>,
    lines: Vec<(u32, u32)>,
    sites: Vec<Site>,
    inlined: Vec<crate::mir::Inlined>,
    /// What a dump finds in it, by the function's number, and where.
    body: usize,
    labelled: Vec<(u32, u32, u32, Located)>,
    slots: Vec<(u32, i64)>,
    saved: Vec<(u16, i64)>,
    /// Its latent sites: where each starts, how long it is, and its site.
    latent: Vec<(u32, u32, u32)>,
}

/// Where a statement's code is in the source, the type of the local it
/// assigns, and the inlined call it comes from (`Body::inlined`).
type Site = (u32, Option<Ty>, Option<u32>);

/// A function whose IR is built, to compile; `traced` names it in stack
/// traces.
struct Job {
    id: FuncId,
    func: ir::Function,
    traced: Option<String>,
    /// Where each statement's code is in the source, and the type of the
    /// local it assigns, so that a sample of the memory it allocates names
    /// its type: the source location of an instruction indexes this, from 1.
    sites: Vec<Site>,
    inlined: Vec<crate::mir::Inlined>,
    /// The body it is of.
    body: usize,
}

/// A function's machine code, its relocations, and its size and calls for
/// stack traces.
struct Compiled {
    /// The IR as it was and as Cranelift left it, when `tondo asm` asks.
    ir: Option<(String, String)>,
    /// Where its instructions end and the constants they read start.
    code_end: u32,
    alignment: u64,
    bytes: Vec<u8>,
    relocs: Vec<cranelift_module::ModuleReloc>,
    size: u32,
    calls: Vec<(u32, u32)>,
    /// Each `try_call`'s return address and handler.
    handlers: Vec<(u32, u32)>,
    /// Where each run of code of one statement starts, and the statement's
    /// site, `0` for code of none.
    lines: Vec<(u32, u32)>,
    /// In a debug build, where its bindings' addresses and which of them
    /// hold values are in its frame, from the frame pointer, by the
    /// function's number in the debugger's table.
    debug_slots: Option<(u32, i64, i64)>,
    /// For a dump, where each value label is over the code, and where each
    /// stack slot is from the frame pointer.
    labelled: Vec<(u32, u32, u32, Located)>,
    slots: Vec<(u32, i64)>,
    saved: Vec<(u16, i64)>,
    /// The calls left as nothing until a logpoint needs them: where each
    /// starts, how long it is, and its statement's site.
    latent: Vec<(u32, u32, u32)>,
}

/// Where a value label is: in a register, by its number and whether it is
/// an integer's, or at an offset from the canonical frame address.
#[derive(Clone, Copy)]
enum Located {
    Register(u8, bool),
    Frame(i64),
}

/// Where a compiled function keeps the registers it saves, from the
/// canonical frame address: a dump reads the caller's values there.
fn saved_registers(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    code: &cranelift_codegen::CompiledCode,
) -> Vec<(u16, i64)> {
    use gimli::UnwindSection;
    use gimli::write::{Address, EhFrame, EndianVec, FrameTable};
    let (Ok(Some(cranelift_codegen::isa::unwind::UnwindInfo::SystemV(info))), Some(cie)) =
        (code.create_unwind_info(isa), isa.create_systemv_cie())
    else {
        return Vec::new();
    };
    let mut table = FrameTable::default();
    let cie = table.add_cie(cie);
    table.add_fde(cie, info.to_fde(Address::Constant(0)));
    let mut written = EhFrame(EndianVec::new(gimli::LittleEndian));
    if table.write_eh_frame(&mut written).is_err() {
        return Vec::new();
    }
    let bytes = written.0.into_vec();
    let frame = gimli::read::EhFrame::new(&bytes, gimli::LittleEndian);
    let bases = gimli::read::BaseAddresses::default();
    let mut entries = frame.entries(&bases);
    let mut saved = Vec::new();
    while let Ok(Some(entry)) = entries.next() {
        let gimli::read::CieOrFde::Fde(partial) = entry else { continue };
        let Ok(fde) = partial.parse(gimli::read::EhFrame::cie_from_offset) else { continue };
        let mut context = gimli::read::UnwindContext::new();
        let Ok(mut rows) = fde.rows(&frame, &bases, &mut context) else { continue };
        while let Ok(Some(row)) = rows.next_row() {
            for (register, rule) in row.registers() {
                if let gimli::read::RegisterRule::Offset(offset) = rule
                    && !saved.iter().any(|&(known, _)| known == register.0)
                {
                    saved.push((register.0, *offset));
                }
            }
        }
    }
    saved
}

/// Where each value label of a compiled function is.
fn labelled(ranges: &cranelift_codegen::ValueLabelsRanges) -> Vec<(u32, u32, u32, Located)> {
    let mut out = Vec::new();
    for (label, ranges) in ranges {
        for range in ranges {
            let located = match range.loc {
                cranelift_codegen::LabelValueLoc::CFAOffset(offset) => Some(Located::Frame(offset)),
                cranelift_codegen::LabelValueLoc::Reg(reg) => {
                    reg.to_real_reg().map(|reg| Located::Register(reg.hw_enc(), format!("{:?}", reg.class()) == "Int"))
                }
            };
            out.extend(located.map(|located| (label.as_u32(), range.start, range.end, located)));
        }
    }
    out.sort_by_key(|&(label, start, ..)| (label, start));
    out
}

/// The functions whose code `tondo asm` asks codegen to keep, by what
/// their names hold, and whether with their IR (toolchain 7.19).
static ASKED: std::sync::Mutex<Option<Asked>> = std::sync::Mutex::new(None);

/// What `tondo asm` asks codegen to keep: the functions, by name, whether
/// with their IR, and whether with the ISLE rules applied to them.
#[derive(Clone)]
pub struct Asked {
    pub names: std::collections::HashSet<String>,
    pub ir: bool,
    pub rules: bool,
}
static KEPT: std::sync::Mutex<Vec<Kept>> = std::sync::Mutex::new(Vec::new());

/// The code of a function `tondo asm` asked for: its machine code, what
/// each relocation in it names, where each run of a statement's code
/// starts with where the statement starts in the source, and its IR before
/// and after Cranelift optimizes it.
pub struct Kept {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Where the instructions end, and the constants they read start.
    pub code_end: usize,
    pub targets: Vec<(u32, String)>,
    pub lines: Vec<(u32, Option<u32>)>,
    pub ir: Option<(String, String)>,
    /// The ISLE rules applied to it, with `rules` (`crate::rules`).
    pub rules: Vec<(String, String, u64, bool)>,
}

/// Asks the next builds to keep the code of the functions named; or no
/// longer.
pub fn keep_code(asked: Option<Asked>) {
    #[cfg(feature = "rules")]
    crate::rules::counting(asked.as_ref().is_some_and(|asked| asked.rules));
    *ASKED.lock().unwrap_or_else(|e| e.into_inner()) = asked;
}

/// The code kept since it was asked for.
pub fn kept_code() -> Vec<Kept> {
    std::mem::take(&mut *KEPT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Compiles a function's IR to machine code, keeping its IR as it was and
/// as Cranelift left it when `ir` asks.
fn compile_one(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    context: &mut cranelift_codegen::Context,
    id: FuncId,
    function: ir::Function,
    ir: bool,
) -> Result<Compiled, String> {
    context.clear();
    let dumps = function.dfg.values_labels.is_some();
    let before = ir.then(|| function.display().to_string());
    context.func = function;
    // What a latent site calls is the runtime, in the same image.
    for (_, callee) in context.func.dfg.ext_funcs.iter_mut() {
        callee.colocated |= callee.patchable;
    }
    context.compile(isa, &mut Default::default()).map_err(|e| format!("{:?}", e.inner))?;
    let ir = before.map(|before| (before, context.func.display().to_string()));
    let code = context.compiled_code().expect("compiled code");
    let sources = code.buffer.get_srclocs_sorted();
    let site_of = |at: u32| {
        let source = sources.get(sources.partition_point(|s| s.end <= at)).filter(|s| s.start <= at);
        source.map_or(0, |source| source.loc.as_abs().bits())
    };
    let latent: Vec<(u32, u32, u32)> = code
        .buffer
        .patchable_call_sites()
        .map(|call| (call.ret_addr - call.len, call.len, site_of(call.ret_addr - 1)))
        .collect();
    let calls = code
        .buffer
        .call_sites()
        .filter_map(|call| {
            let at = call.ret_addr - 1;
            let source = sources.get(sources.partition_point(|s| s.end <= at)).filter(|s| s.start <= at)?;
            let loc = source.loc.as_abs();
            (!loc.is_default()).then(|| (call.ret_addr, loc.bits()))
        })
        .collect();
    let handlers = code
        .buffer
        .call_sites()
        .filter_map(|call| match call.exception_handlers {
            [cranelift_codegen::MachExceptionHandler::Default(at)] if at.is_offset() => {
                Some((call.ret_addr, at.as_offset()))
            }
            _ => None,
        })
        .collect();
    let in_latent = |at: u32| latent.iter().any(|&(start, length, _)| (start..start + length).contains(&at));
    let relocs = code
        .buffer
        .relocs()
        .iter()
        .filter(|reloc| !in_latent(reloc.offset))
        .map(|reloc| cranelift_module::ModuleReloc::from_mach_reloc(reloc, &context.func, id))
        .collect();
    // A latent site is nothing until a logpoint writes its call.
    let mut bytes = code.buffer.data().to_vec();
    for &(start, length, _) in &latent {
        let mut at = start as usize;
        while at < (start + length) as usize {
            let left = (start + length) as usize - at;
            let nop = code.buffer.nop_units.iter().rev().find(|nop| nop.len() <= left).expect("a nop that fits");
            bytes[at..at + nop.len()].copy_from_slice(nop);
            at += nop.len();
        }
    }
    Ok(Compiled {
        ir,
        code_end: sources.iter().map(|s| s.end).max().unwrap_or(code.buffer.data().len() as u32),
        latent,
        alignment: code.buffer.min_alignment as u64,
        bytes,
        relocs,
        size: code.code_info().total_size,
        calls,
        handlers,
        lines: sources
            .iter()
            .map(|s| (s.start, if s.loc.as_abs().is_default() { 0 } else { s.loc.as_abs().bits() }))
            .collect(),
        debug_slots: debug_slots(code.buffer.frame_layout()),
        labelled: labelled(&code.value_labels_ranges),
        saved: if dumps { saved_registers(isa, code) } else { Vec::new() },
        slots: code.buffer.frame_layout().map_or(Vec::new(), |layout| {
            let at = |offset: u32| i64::from(offset) - i64::from(layout.frame_to_fp_offset);
            layout.stackslots.iter().map(|(slot, data)| (slot.as_u32(), at(data.offset))).collect()
        }),
    })
}

/// Where the keyed slots of a debug build's function are from its frame
/// pointer: its number, then the slot of which bindings hold values, then
/// that of their addresses.
fn debug_slots(layout: Option<&cranelift_codegen::MachBufferFrameLayout>) -> Option<(u32, i64, i64)> {
    let layout = layout?;
    let (mut function, mut alive, mut addresses) = (None, None, None);
    for (_, slot) in layout.stackslots.iter() {
        let Some(key) = slot.key else { continue };
        let at = i64::from(slot.offset) - i64::from(layout.frame_to_fp_offset);
        function = Some((key.bits() >> 1) as u32);
        match key.bits() & 1 {
            0 => alive = Some(at),
            _ => addresses = Some(at),
        }
    }
    Some((function?, alive?, addresses?))
}

/// What the code generator knows of the program's types without declaring
/// anything: their classes and layouts, and the signatures that pass them,
/// which any core can work out.
struct Shapes<'t> {
    types: &'t Types,
    layouts: HashMap<Ty, Layout>,
    call_conv: cranelift_codegen::isa::CallConv,
    abi: cabi::Abi,
}

impl<'t> Shapes<'t> {
    fn new(types: &'t Types, isa: &dyn cranelift_codegen::isa::TargetIsa) -> Shapes<'t> {
        let abi = match (isa.name(), isa.default_call_conv()) {
            ("aarch64", call_conv) => {
                cabi::Abi::Arm64 { apple: call_conv == cranelift_codegen::isa::CallConv::AppleAarch64 }
            }
            _ => cabi::Abi::X64,
        };
        Shapes { types, layouts: HashMap::new(), call_conv: isa.default_call_conv(), abi }
    }

    fn make_signature(&self) -> ir::Signature {
        ir::Signature::new(self.call_conv)
    }

    fn class(&mut self, ty: Ty) -> Class {
        let repr = self.types.repr(ty);
        match self.types.kind(repr) {
            TyKind::Unit | TyKind::Never | TyKind::Error => Class::Zero,
            TyKind::Param(_) | TyKind::Opaque(..) => {
                unreachable!("monomorphization leaves only concrete types")
            }
            TyKind::Bool => Class::Scalar(types::I8),
            TyKind::Int(int) => Class::Scalar(int_type(*int)),
            TyKind::Float(FloatTy::F32) => Class::Scalar(types::F32),
            TyKind::Float(FloatTy::F64) => Class::Scalar(types::F64),
            TyKind::Char => Class::Scalar(types::I32),
            TyKind::Byte => Class::Scalar(types::I8),
            TyKind::String
            | TyKind::Array(_)
            | TyKind::Deque(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Fn(..)
            | TyKind::CFn(..)
            | TyKind::Handle(..)
            | TyKind::Pointer(_) => Class::Scalar(POINTER),
            TyKind::CArray(..) => Class::Memory,
            TyKind::Tuple(_) | TyKind::Adt(_) | TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) => {
                let layout = self.layout(repr);
                match layout.shape {
                    Shape::Niche => Class::Scalar(POINTER),
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
        let layout = if let TyKind::Option(inner) = *self.types.kind(ty)
            && self.types.is_address(inner)
        {
            Layout { size: 8, align: 8, shape: Shape::Niche }
        } else if let TyKind::CArray(element, count) = *self.types.kind(ty) {
            let element = self.layout(element);
            Layout { size: element.size * count, align: element.align, shape: Shape::Scalar }
        } else if let Some(c) = self.types.c_layout(ty) {
            // An `extern type`: C's layout, without padding when packed.
            let (mut size, mut align, mut fields) = (0u32, 1u32, Fields::default());
            for component in self.types.components(ty).expect("a record") {
                let layout = self.layout(component);
                // C aligns an `__int128` to 16 bytes.
                let natural = if self.class(component) == Class::Scalar(types::I128) { 16 } else { layout.align };
                let component_align = if c.packed { 1 } else { natural };
                // A union's fields share its memory.
                let offset = if c.union { 0 } else { size.next_multiple_of(component_align) };
                fields.offsets.push(offset);
                fields.boxed.push(false);
                size = size.max(offset + layout.size);
                align = align.max(component_align);
            }
            let align = align.max(c.align);
            Layout { size: size.next_multiple_of(align), align, shape: Shape::Struct(fields) }
        } else if let Some(components) = self.types.components(ty) {
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
            // An integer of 128 bits is two words, aligned as one.
            Layout { size, align: size.clamp(1, 8), shape: Shape::Scalar }
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
            (Shape::Niche, Proj::Variant(SOME, 0)) => Slot { offset: 0, ty: component, boxed: false },
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
        let mut signature = self.make_signature();
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
        // A live build's hidden parameter: the state a frame brings from
        // the old code.
        if body.osr.is_some() {
            signature.params.push(AbiParam::new(types::I64));
        }
        if let Class::Scalar(ty) = self.class(body.result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    /// The calling convention of a `fn(...)` value: the result slot when
    /// the result lives in memory, the captures' address, then the
    /// parameters. Closure bodies and thunks follow it.
    fn uniform_signature(&mut self, params: &[Ty], loans: &[bool], result: Ty) -> ir::Signature {
        let mut signature = self.make_signature();
        if self.class(result) == Class::Memory {
            signature.params.push(AbiParam::new(POINTER));
        }
        signature.params.push(AbiParam::new(POINTER));
        for (&param, &loan) in params.iter().zip(loans) {
            match self.class(param) {
                _ if loan => signature.params.push(AbiParam::new(POINTER)),
                Class::Zero => {}
                Class::Scalar(ty) => signature.params.push(AbiParam::new(ty)),
                Class::Memory => signature.params.push(AbiParam::new(POINTER)),
            }
        }
        if let Class::Scalar(ty) = self.class(result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        let mut signature = self.make_signature();
        signature.params.push(AbiParam::new(POINTER));
        if matches!(kind, Glue::RetainCold | Glue::ReleaseCold) {
            signature.call_conv = cranelift_codegen::isa::CallConv::PreserveAll;
        }
        if matches!(kind, Glue::Display | Glue::Visit | Glue::Show | Glue::ShowRaw | Glue::Secret) {
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
}

/// What building a function's IR asks of the code generator: what the
/// program is, the shapes of its types, the functions and data the code
/// refers to, and what debug and live builds record of it. `Codegen`
/// declares those as asked; a `Worker`, which builds a function on another
/// core, names them so that `replay` declares them in the same order.
trait Gen<'t> {
    fn shapes(&mut self) -> &mut Shapes<'t>;
    fn types(&self) -> &'t Types;
    fn sources(&self) -> &'t SourceMap;
    fn display(&self) -> &'t HashMap<Ty, FnId>;
    /// The `debug.View.view` of each type that has one, and the type of
    /// the view.
    fn views(&self) -> &'t HashMap<Ty, (FnId, Ty)>;
    fn externs(&self) -> &'t [crate::hir::Extern];
    fn meta(&self) -> bool;
    fn isa(&self) -> &dyn cranelift_codegen::isa::TargetIsa;
    /// Which parameters of a function are loans.
    fn loans(&self, id: FnId) -> Vec<bool>;
    /// A function of the program, and whether it takes a live build's
    /// hidden parameter.
    fn function(&self, id: FnId) -> FuncId;
    fn takes_osr(&self, id: FnId) -> bool;
    fn function_signature(&self, id: FuncId) -> ir::Signature;
    /// In a live build, the table of slots and a function's slot.
    fn live(&self) -> bool;
    fn slot_of(&self, id: FnId) -> Option<(DataId, u32)>;
    /// The number a patch's debugger sites start from.
    fn site_base(&self) -> u32;
    fn debug(&self) -> bool;
    /// In a test build with coverage, its counters.
    fn coverage(&self) -> Option<DataId>;
    /// A build for the search of `--fuzz`.
    fn fuzzing(&self) -> bool;
    /// A build of tests.
    fn testing(&self) -> bool;
    /// A counter of what `counted` says: its number.
    fn counter(&mut self, counted: Counted) -> usize;
    /// Where the counter `index` is among the counters, as an
    /// instruction's constant.
    fn counter_offset(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value;

    fn string(&mut self, text: &str) -> DataId;
    /// A static array of constants, each `size` bytes.
    fn static_array(&mut self, size: usize, elements: Vec<Const>) -> DataId;
    fn bytes(&mut self, bytes: &std::sync::Arc<[u8]>) -> DataId;
    fn identity(&mut self, ty: Ty) -> DataId;
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId;
    fn runtime(&mut self, rt: Rt) -> FuncId;
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId;
    fn map_glue(&mut self, ty: Ty) -> DataId;
    fn closure_code(&mut self, body: FnId) -> FuncId;
    fn c_function(&mut self, symbol: &str, signature: &ir::Signature) -> (FuncId, bool);
    fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId;
    fn declare_func_in_func(&mut self, id: FuncId, func: &mut ir::Function) -> ir::FuncRef;
    fn declare_data_in_func(&mut self, id: DataId, func: &mut ir::Function) -> ir::GlobalValue;
    /// The runtime's count of reasons to check for cancellation.
    fn attention(&mut self) -> DataId;

    /// A debug build's function, whether it takes a loan, and the bindings
    /// it shows: its number.
    fn debug_function(&mut self, name: String, lends: bool, described: Vec<DebugBinding>) -> u32;
    /// A build without probes: whether its dumps show bindings.
    fn dumps(&self) -> bool;
    /// What a dump finds in the function that is being built.
    fn dump_bindings(&mut self, described: Vec<DumpBinding>);
    /// In a live build, the word that holds the number of the function's
    /// first site, and that number.
    fn function_sites(&mut self) -> Option<(DataId, usize)>;
    /// A probe of the function `function` at `start`: its site's number.
    fn debug_site(&mut self, start: u32, function: u32, goes_on: bool) -> usize;
    /// The number of the site `index`, as an instruction's constant.
    fn site_constant(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value;
    /// In a live build, the site of the probe of a point of the function.
    fn point_site(&mut self, point: u32, number: u32);
    /// In a live build, how a point's state is laid out, once worked out.
    fn point_layout(&self, point: u32) -> Option<StateLayout>;
    fn keep_point_layout(&mut self, point: u32, layout: StateLayout);

    fn class(&mut self, ty: Ty) -> Class {
        self.shapes().class(ty)
    }
    fn layout(&mut self, ty: Ty) -> Layout {
        self.shapes().layout(ty)
    }
    fn scalar_type(&mut self, ty: Ty) -> ir::Type {
        self.shapes().scalar_type(ty)
    }
    fn slot(&mut self, ty: Ty, proj: Proj) -> Slot {
        self.shapes().slot(ty, proj)
    }
    fn signature(&mut self, body: &Body) -> ir::Signature {
        self.shapes().signature(body)
    }
    fn uniform_signature(&mut self, params: &[Ty], loans: &[bool], result: Ty) -> ir::Signature {
        self.shapes().uniform_signature(params, loans, result)
    }
    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        self.shapes().glue_signature(kind)
    }
    fn sequence_parts(&mut self, ty: Ty) -> (Ty, bool) {
        self.shapes().sequence_parts(ty)
    }
    fn map_parts(&mut self, ty: Ty) -> (Ty, Ty) {
        self.shapes().map_parts(ty)
    }
    fn entry_layout(&mut self, ty: Ty) -> (u32, u32) {
        self.shapes().entry_layout(ty)
    }
    fn abi(&mut self) -> cabi::Abi {
        self.shapes().abi
    }
    fn c_plan(&mut self, args: &[Ty], fixed: usize, result: Ty) -> cabi::Plan {
        self.shapes().c_plan(args, fixed, result)
    }
    fn c_signature(&mut self, params: &[Ty], result: Ty) -> ir::Signature {
        self.shapes().c_signature(params, result)
    }
    fn callback_frame(&mut self, ty: Ty) -> (Vec<u32>, u32) {
        self.shapes().callback_frame(ty)
    }
}

impl<'t> Gen<'t> for Codegen<'t> {
    fn shapes(&mut self) -> &mut Shapes<'t> {
        &mut self.shapes
    }
    fn types(&self) -> &'t Types {
        self.types
    }
    fn sources(&self) -> &'t SourceMap {
        self.sources
    }
    fn display(&self) -> &'t HashMap<Ty, FnId> {
        self.display
    }
    fn views(&self) -> &'t HashMap<Ty, (FnId, Ty)> {
        self.views
    }
    fn externs(&self) -> &'t [crate::hir::Extern] {
        self.externs
    }
    fn meta(&self) -> bool {
        self.meta
    }
    fn isa(&self) -> &dyn cranelift_codegen::isa::TargetIsa {
        self.module.isa()
    }
    fn loans(&self, id: FnId) -> Vec<bool> {
        self.loans[id.0 as usize].clone()
    }
    fn function(&self, id: FnId) -> FuncId {
        self.functions[id.0 as usize]
    }
    fn takes_osr(&self, id: FnId) -> bool {
        self.takes_osr[id.0 as usize]
    }
    fn function_signature(&self, id: FuncId) -> ir::Signature {
        self.module.declarations().get_function_decl(id).signature.clone()
    }
    fn live(&self) -> bool {
        self.live.is_some()
    }
    fn slot_of(&self, id: FnId) -> Option<(DataId, u32)> {
        self.live.as_ref().map(|live| (live.slots, live.slot_of[id.0 as usize]))
    }
    fn site_base(&self) -> u32 {
        self.live.as_ref().and_then(|live| live.patch.as_ref()).map_or(0, |(_, sites)| *sites)
    }
    fn debug(&self) -> bool {
        self.debug.is_some()
    }
    fn coverage(&self) -> Option<DataId> {
        self.coverage.as_ref().map(|(counters, _)| *counters)
    }
    fn fuzzing(&self) -> bool {
        self.fuzz
    }
    fn testing(&self) -> bool {
        self.testing
    }
    fn counter(&mut self, counted: Counted) -> usize {
        let (_, all) = self.coverage.as_mut().expect("a build with coverage");
        all.push(counted);
        all.len() - 1
    }
    fn counter_offset(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        fb.ins().iconst(types::I64, index as i64 * 8)
    }
    fn string(&mut self, text: &str) -> DataId {
        Codegen::string(self, text)
    }
    fn static_array(&mut self, size: usize, elements: Vec<Const>) -> DataId {
        Codegen::static_array(self, size, &elements)
    }
    fn bytes(&mut self, bytes: &std::sync::Arc<[u8]>) -> DataId {
        Codegen::bytes(self, bytes)
    }
    fn identity(&mut self, ty: Ty) -> DataId {
        Codegen::identity(self, ty)
    }
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        Codegen::glue(self, kind, ty)
    }
    fn runtime(&mut self, rt: Rt) -> FuncId {
        Codegen::runtime(self, rt)
    }
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId {
        Codegen::fn_value(self, id, body)
    }
    fn map_glue(&mut self, ty: Ty) -> DataId {
        Codegen::map_glue(self, ty)
    }
    fn closure_code(&mut self, body: FnId) -> FuncId {
        Codegen::closure_code(self, body)
    }
    fn c_function(&mut self, symbol: &str, signature: &ir::Signature) -> (FuncId, bool) {
        Codegen::c_function(self, symbol, signature)
    }
    fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId {
        Codegen::callback(self, target, ty, closure)
    }
    fn declare_func_in_func(&mut self, id: FuncId, func: &mut ir::Function) -> ir::FuncRef {
        self.module.declare_func_in_func(id, func)
    }
    fn declare_data_in_func(&mut self, id: DataId, func: &mut ir::Function) -> ir::GlobalValue {
        self.module.declare_data_in_func(id, func)
    }
    fn attention(&mut self) -> DataId {
        self.module.declare_data("tondo_rt_attention", Linkage::Import, true, false).expect("the runtime's attention")
    }
    fn debug_function(&mut self, name: String, lends: bool, described: Vec<DebugBinding>) -> u32 {
        self.debug_functions.push((name, lends, described));
        self.debug_functions.len() as u32 - 1
    }
    fn dumps(&self) -> bool {
        self.dump.is_some()
    }
    fn dump_bindings(&mut self, described: Vec<DumpBinding>) {
        self.dump_bindings.insert(self.current, described);
    }
    fn function_sites(&mut self) -> Option<(DataId, usize)> {
        self.live.as_ref()?;
        let name = format!("tondo_sites{}", self.debug_functions.len() - 1);
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("a function's sites");
        let first = self.debug_sites.len();
        self.live.as_mut().expect("a live build").sites.push((id, self.current, first));
        Some((id, first))
    }
    fn debug_site(&mut self, start: u32, function: u32, goes_on: bool) -> usize {
        self.debug_sites.push((start, function, goes_on));
        self.debug_sites.len() - 1
    }
    fn site_constant(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        fb.ins().iconst(types::I64, index as i64)
    }
    fn point_site(&mut self, point: u32, number: u32) {
        let function = self.current;
        if let Some(live) = self.live.as_mut() {
            live.point_sites.insert((function, point), number);
        }
    }
    fn point_layout(&self, point: u32) -> Option<StateLayout> {
        self.live.as_ref().and_then(|live| live.layouts.get(&(self.current, point)).cloned())
    }
    fn keep_point_layout(&mut self, point: u32, layout: StateLayout) {
        let function = self.current;
        if let Some(live) = self.live.as_mut() {
            live.layouts.insert((function, point), layout);
        }
    }
}

impl Codegen<'_> {
    /// The static `fn(...)` object of a named function, created on first use.
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId {
        if let Some(&(data, _)) = self.fn_values.get(&id) {
            return data;
        }
        let params: Vec<Ty> = body.params.iter().map(|p| body.locals[p.0 as usize]).collect();
        let signature = self.uniform_signature(&params, &body.loans, body.result);
        let name = format!("tondo_thunk{}", id.0);
        let thunk = self.module.declare_function(&name, Linkage::Local, &signature).expect("thunk declaration");
        let data =
            self.module.declare_data(&format!("tondo_fn{}", id.0), Linkage::Local, false, false).expect("fn value");
        let mut description = DataDescription::new();
        let mut bytes = vec![0u8; FN_HEADER as usize];
        bytes[..8].copy_from_slice(&IMMORTAL.to_le_bytes());
        description.define(bytes.into_boxed_slice());
        description.set_align(8);
        let reference = self.module.declare_func_in_data(thunk, &mut description);
        description.write_function_addr(16, reference);
        self.module.define_data(data, &description).expect("fn value definition");
        self.fn_values.insert(id, (data, thunk));
        self.pending_thunks.push((id, thunk));
        data
    }

    /// The code a closure or a `defer` keeps: its body, or in a live build
    /// a function that calls the body through its slot, so that a closure
    /// made before a patch runs the patch's body.
    fn closure_code(&mut self, body: FnId) -> FuncId {
        if self.live.is_none() {
            return self.functions[body.0 as usize];
        }
        if let Some(&forward) = self.forwarders.get(&body) {
            return forward;
        }
        // The convention of `fn(...)` values: no hidden parameter.
        let mut signature =
            self.module.declarations().get_function_decl(self.functions[body.0 as usize]).signature.clone();
        if self.takes_osr[body.0 as usize] {
            signature.params.pop();
        }
        let name = format!("tondo_forward{}", body.0);
        let forward = self.module.declare_function(&name, Linkage::Local, &signature).expect("a closure's forwarder");
        self.forwarders.insert(body, forward);
        self.pending_forwarders.push((body, forward));
        forward
    }

    fn runtime(&mut self, rt: Rt) -> FuncId {
        if let Some(&id) = self.runtime.get(&rt) {
            return id;
        }
        let (name, signature) = runtime_signature(rt, self.module.make_signature());
        let id = self.module.declare_function(name, Linkage::Import, &signature).expect("runtime declaration");
        self.runtime.insert(rt, id);
        id
    }

    /// The tree of a test build for the runner: for each node, its parent,
    /// whether it is a suite, and its identity.
    /// Records, for stack traces, where each call of the function just
    /// compiled returns to and the source of the statement that makes it.
    /// Compiles the functions on every core, and defines them in order, so
    /// the object is the same each time.
    fn compile_all(&mut self, mut jobs: Vec<Job>) -> Result<(), String> {
        let isa = self.module.isa();
        let asked = ASKED.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let wanted = |job: &Job| match (&asked, &job.traced) {
            (Some(asked), Some(traced)) if asked.names.contains(traced) => Some(asked.ir),
            _ => None,
        };
        // Each function goes to the thread that compiles it, which says
        // which it compiles when the rules it gets are counted.
        type Compiling = (FuncId, std::sync::Mutex<Option<ir::Function>>, bool, Option<String>);
        let functions: Vec<Compiling> = jobs
            .iter_mut()
            .map(|job| {
                let ir = wanted(job) == Some(true);
                let counted =
                    asked.as_ref().filter(|asked| asked.rules && wanted(job).is_some()).and(job.traced.clone());
                let function = std::sync::Mutex::new(Some(std::mem::replace(&mut job.func, ir::Function::new())));
                (job.id, function, ir, counted)
            })
            .collect();
        let compiled = crate::parallel::map(&functions, 1, |(id, function, ir, _counted)| {
            thread_local! {
                static CONTEXT: std::cell::RefCell<cranelift_codegen::Context> =
                    std::cell::RefCell::new(cranelift_codegen::Context::new());
            }
            let function = function.lock().unwrap_or_else(|e| e.into_inner()).take().expect("a function");
            #[cfg(feature = "rules")]
            crate::rules::compiling(_counted.as_deref());
            let compiled = CONTEXT.with(|context| compile_one(isa, &mut context.borrow_mut(), *id, function, *ir));
            #[cfg(feature = "rules")]
            crate::rules::compiling(None);
            compiled
        });
        // What a call in kept code calls, by the program's names.
        let names: HashMap<u32, String> = match asked.is_some() {
            true => jobs.iter().filter_map(|job| Some((job.id.as_u32(), job.traced.clone()?))).collect(),
            false => HashMap::new(),
        };
        let labels = self.live.is_some().then(|| self.labels());
        let mut laid: Vec<_> = jobs.into_iter().zip(compiled).collect();
        if let Some(seed) = crate::driver::layout_seed() {
            // Another order of the functions, the same for the same seed;
            // their tables follow it, as the runtime looks them up by address.
            let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
            for at in (1..laid.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                laid.swap(at, (state % (at as u64 + 1)) as usize);
            }
        }
        for (job, code) in laid {
            let mut code = code?;
            if wanted(&job).is_some() {
                let target = |reloc: &cranelift_module::ModuleReloc| match reloc.name {
                    cranelift_module::ModuleRelocTarget::User { namespace: 0, index } => names
                        .get(&index)
                        .cloned()
                        .or_else(|| self.module.declarations().get_function_decl(FuncId::from_u32(index)).name.clone()),
                    cranelift_module::ModuleRelocTarget::User { namespace: 1, index } => {
                        self.module.declarations().get_data_decl(DataId::from_u32(index)).name.clone()
                    }
                    _ => None,
                };
                let site = |site: u32| job.sites.get((site as usize).checked_sub(1)?).map(|site| site.0);
                KEPT.lock().unwrap_or_else(|e| e.into_inner()).push(Kept {
                    name: job.traced.clone().unwrap_or_default(),
                    bytes: code.bytes.clone(),
                    code_end: code.code_end as usize,
                    targets: code.relocs.iter().filter_map(|reloc| Some((reloc.offset, target(reloc)?))).collect(),
                    lines: code.lines.iter().map(|&(at, site_index)| (at, site(site_index))).collect(),
                    ir: code.ir.take(),
                    #[cfg(feature = "rules")]
                    rules: job.traced.as_deref().map_or(Vec::new(), crate::rules::applied),
                    #[cfg(not(feature = "rules"))]
                    rules: Vec::new(),
                });
            }
            if let Some(labels) = &labels {
                let hash = code_hash(&code, labels);
                self.live.as_mut().expect("a live build").code.insert(job.id, hash);
            }
            self.module
                .define_function_bytes(job.id, code.alignment, &code.bytes, &code.relocs)
                .map_err(|e| format!("{e:?}"))?;
            if let Some((function, alive, addresses)) = code.debug_slots {
                self.debug_slots.insert(function, (alive, addresses));
            }
            if let Some(name) = job.traced {
                let (size, calls, lines, sites, inlined) = (code.size, code.calls, code.lines, job.sites, job.inlined);
                let (handlers, labelled, slots, body) = (code.handlers, code.labelled, code.slots, job.body);
                let (saved, latent) = (code.saved, code.latent);
                self.traced.push(Traced {
                    latent,
                    id: job.id,
                    name,
                    size,
                    calls,
                    handlers,
                    lines,
                    sites,
                    inlined,
                    body,
                    labelled,
                    slots,
                    saved,
                });
            }
        }
        Ok(())
    }

    /// What each function and data a function's code refers to is, the
    /// same from one build to the next: a function by its identity, glue by
    /// its type, a string by its text; the runtime's by their names.
    fn labels(&self) -> Labels {
        let live = self.live.as_ref().expect("a live build");
        let mut functions: HashMap<FuncId, String> = HashMap::new();
        let mut data: HashMap<DataId, String> = HashMap::new();
        for (index, &id) in self.functions.iter().enumerate() {
            functions.insert(id, live.identities[index].clone());
        }
        for (&body, &(value, thunk)) in &self.fn_values {
            functions.insert(thunk, format!("thunk {}", live.identities[body.0 as usize]));
            data.insert(value, format!("fn {}", live.identities[body.0 as usize]));
        }
        for (&(kind, ty), &id) in &self.glue {
            functions.insert(id, format!("glue {kind:?} {}", self.types.display(ty)));
        }
        for (&body, &id) in &self.forwarders {
            functions.insert(id, format!("forward {}", live.identities[body.0 as usize]));
        }
        for (&ty, &id) in &self.map_glue {
            data.insert(id, format!("map glue {}", self.types.display(ty)));
        }
        for (text, &id) in &self.strings {
            data.insert(id, format!("string {text}"));
        }
        for (&ty, &id) in &self.identities {
            data.insert(id, format!("identity {}", self.types.display(ty)));
        }
        for &(id, body, _) in &live.sites {
            data.insert(id, format!("sites {}", live.identities[body]));
        }
        let declarations = self.module.declarations();
        for (id, decl) in declarations.get_functions() {
            functions.entry(id).or_insert_with(|| decl.name.clone().unwrap_or_default());
        }
        for (id, decl) in declarations.get_data_objects() {
            data.entry(id).or_insert_with(|| decl.name.clone().unwrap_or_default());
        }
        Labels { functions, data }
    }

    /// Where each run of a function's code starts, with the file, line and
    /// column of its statement, for the DWARF: code of no statement is the
    /// statement's before it, and code before the first statement the
    /// first's.
    fn dwarf_rows(&self, function: &Traced) -> Vec<crate::debuginfo::Row> {
        let mut rows: Vec<crate::debuginfo::Row> = Vec::new();
        for (index, &(start, site)) in function.lines.iter().enumerate().filter(|&(_, &(_, site))| site != 0) {
            let end = function.lines.get(index + 1).map_or(u32::MAX, |&(next, _)| next);
            let (path, line, column) = self.sources.position(function.sites[site as usize - 1].0);
            let (line, column) = (line as u32, column as u32);
            let new = rows.last().is_none_or(|row| (&row.file, row.line) != (&path, line));
            // A line starts with its probe, which stores where its bindings
            // are: the line's code for a debugger starts after it.
            let probe =
                new.then(|| function.calls.iter().find(|&&(at, of)| of == site && at > start && at < end)).flatten();
            let row = crate::debuginfo::Row { offset: start, file: path, line, column, statement: probe.is_none() };
            match probe {
                Some(&(after, _)) => {
                    rows.push(row.clone());
                    rows.push(crate::debuginfo::Row { offset: after, statement: true, ..row });
                }
                None => rows.push(row),
            }
        }
        if let Some(first) = rows.first().filter(|first| first.offset > 0).cloned() {
            rows.insert(0, crate::debuginfo::Row { offset: 0, statement: false, ..first });
        }
        rows
    }
}

/// What a `try_call` calls: a function, or code at an address.
#[derive(Clone, Copy)]
enum Called {
    Direct(ir::FuncRef),
    Indirect(ir::SigRef, ir::Value),
}

impl Emit<'_, Codegen<'_>> {
    /// Hands the runtime each context's functions, the maker of its
    /// initial value and those that show its fields, and the release of its
    /// record (11.16).
    fn register_contexts(&mut self) {
        for (index, (fns, record)) in self.cg.contexts.clone().into_iter().enumerate() {
            let address = |e: &mut Self, id: Option<FnId>| match id {
                Some(id) => {
                    let function = e.cg.function(id);
                    let callee = e.cg.declare_func_in_func(function, e.fb.func);
                    e.fb.ins().func_addr(POINTER, callee)
                }
                None => e.fb.ins().iconst(POINTER, 0),
            };
            let maker = address(self, Some(fns.maker));
            let (log, label) = (address(self, fns.log), address(self, fns.label));
            let release = self.glue_or_null(Glue::Release, record);
            let show = self.glue_address(Glue::Show, record);
            let name = self.cg.types().display(record);
            let name = self.string_ptr(&name);
            let index = self.fb.ins().iconst(types::I64, index as i64);
            self.call_rt(Rt::ContextRegister, &[index, maker, release, log, label, name, show]);
            for (field, computes, named) in &fns.computed {
                let computes = address(self, Some(*computes));
                let field = self.fb.ins().iconst(types::I64, i64::from(*field));
                let named = self.string_ptr(named);
                self.call_rt(Rt::ContextComputed, &[index, field, computes, named]);
            }
            // The tools show its fields one by one, the computed ones from
            // where they are, since the record holds zeros for them.
            let fields = match self.cg.types().kind(record) {
                TyKind::Adt(id) => match &self.cg.types().adt(*id).kind {
                    AdtKind::Record(fields) => fields.iter().map(|f| (f.name.clone(), f.secret)).collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let slots: Vec<Slot> = (0..fields.len()).map(|i| self.cg.slot(record, Proj::Field(i as u32))).collect();
            if fns.computed.is_empty() || slots.iter().any(|slot| slot.boxed) {
                continue;
            }
            for ((name, secret), slot) in fields.iter().zip(&slots) {
                let offset = self.fb.ins().iconst(types::I64, i64::from(slot.offset));
                let show = match self.cg.class(slot.ty) {
                    _ if *secret => self.glue_address(Glue::Secret, Ty::UNIT),
                    Class::Zero => self.fb.ins().iconst(POINTER, 0),
                    _ => self.glue_address(Glue::Show, slot.ty),
                };
                let name = self.string_ptr(name);
                self.call_rt(Rt::ContextShows, &[index, offset, show, name]);
            }
        }
    }
}

/// Code emission shared by function bodies and glue functions.
struct Emit<'a, G: ?Sized> {
    cg: &'a mut G,
    fb: FunctionBuilder<'a>,
    /// The data the function has declared, each once, so that Cranelift
    /// takes the uses of one for the same address.
    data: HashMap<DataId, ir::GlobalValue>,
}

impl<'a, G: ?Sized> Emit<'a, G> {
    fn new(cg: &'a mut G, fb: FunctionBuilder<'a>) -> Self {
        Emit { cg, fb, data: HashMap::new() }
    }
}

impl<'t, G: Gen<'t> + ?Sized> Emit<'_, G> {
    /// The data `id` declared in the function, once.
    fn global(&mut self, id: DataId) -> ir::GlobalValue {
        if let Some(&global) = self.data.get(&id) {
            return global;
        }
        let global = self.cg.declare_data_in_func(id, self.fb.func);
        self.data.insert(id, global);
        global
    }

    fn call_rt(&mut self, rt: Rt, args: &[ir::Value]) -> Option<ir::Value> {
        if self.cg.testing() && rt.outside() {
            let id = self.cg.runtime(Rt::Outside);
            let callee = self.cg.declare_func_in_func(id, self.fb.func);
            self.fb.ins().call(callee, &[]);
        }
        let id = self.cg.runtime(rt);
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn call_glue(&mut self, kind: Glue, ty: Ty, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn glue_address(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        // A string's, which an array's release knows: the pieces of one
        // block among its elements go a run at a time.
        let string = matches!(self.cg.types().kind(self.cg.types().repr(ty)), TyKind::String);
        let id =
            if kind == Glue::Release && string { self.cg.runtime(Rt::StrReleaseAt) } else { self.cg.glue(kind, ty) };
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        self.fb.ins().func_addr(POINTER, callee)
    }

    /// A glue function's address, or null when a retain or release of `ty`
    /// has nothing to do.
    fn glue_or_null(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        if matches!(kind, Glue::Retain | Glue::Release) && !self.cg.types().needs_drop(ty)
            || kind == Glue::Visit && !self.cg.types().can_cycle(ty)
        {
            return self.fb.ins().iconst(POINTER, 0);
        }
        self.glue_address(kind, ty)
    }

    fn map_glue_address(&mut self, ty: Ty) -> ir::Value {
        let id = self.cg.map_glue(ty);
        let global = self.global(id);
        self.fb.ins().symbol_value(POINTER, global)
    }

    /// An integer constant of type `ty`, from its bits.
    fn int_const(&mut self, ty: ir::Type, value: i128) -> ir::Value {
        // Narrow immediates are zero-extended bit patterns; one of 128 bits
        // is made of its two halves.
        match ty.bits() {
            128 => {
                let low = self.fb.ins().iconst(types::I64, value as i64);
                let high = self.fb.ins().iconst(types::I64, (value >> 64) as i64);
                self.fb.ins().iconcat(low, high)
            }
            64 => self.fb.ins().iconst(ty, value as i64),
            bits => self.fb.ins().iconst(ty, (value as i64) & ((1i64 << bits) - 1)),
        }
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
                let word = match *self.cg.types().kind(self.cg.types().repr(ty)) {
                    TyKind::String => self.call_rt(Rt::StrHash, &[value]).unwrap(),
                    TyKind::Int(int) if int.bits() == 128 => {
                        let (low, high) = self.fb.ins().isplit(value);
                        let hash = self.mix(seed, low);
                        return self.mix(hash, high);
                    }
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
                    let count = self.cg.types().components(ty).unwrap().len() as u32;
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
        if let Some(&id) = self.cg.display().get(&ty) {
            // A user type: its `Display.display` returns an owned string.
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let call = call_body(self.cg, &mut self.fb, id, &args);
            let text = self.fb.inst_results(call)[0];
            self.call_rt(Rt::SbStr, &[builder, text]);
            self.call_rt(Rt::StrRelease, &[text]);
            return;
        }
        let types = self.cg.types();
        if let TyKind::Result(success, failure) = *types.kind(types.repr(ty)) {
            let tag = self.tag(ty, value);
            let (good, bad, done) = (self.fb.create_block(), self.fb.create_block(), self.fb.create_block());
            let is_ok = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, OK as i64);
            self.fb.ins().brif(is_ok, good, &[], bad, &[]);
            for (block, variant, inner, name) in [(good, OK, success, "ok("), (bad, ERR, failure, "err(")] {
                self.fb.switch_to_block(block);
                let open = self.string_ptr(name);
                self.call_rt(Rt::SbStr, &[builder, open]);
                let payload = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Variant(variant, 0));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, inner)
                    }
                    _ => Val::Zero,
                };
                self.append_display(builder, inner, payload);
                let close = self.string_ptr(")");
                self.call_rt(Rt::SbStr, &[builder, close]);
                self.fb.ins().jump(done, &[]);
            }
            self.fb.switch_to_block(done);
            return;
        }
        // A union: the member it holds, with its own `Display`.
        if let Some(members) = types.members(types.repr(ty)).map(<[Ty]>::to_vec) {
            let tag = self.tag(ty, value);
            let done = self.fb.create_block();
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
                self.append_display(builder, member, payload);
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(next);
            }
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(done);
            return;
        }
        // A tuple: `(a, b)`, each part with its own `Display`.
        if let TyKind::Tuple(items) = types.kind(types.repr(ty)).clone() {
            let open = self.string_ptr("(");
            self.call_rt(Rt::SbStr, &[builder, open]);
            for (index, &item) in items.iter().enumerate() {
                if index > 0 {
                    let comma = self.string_ptr(", ");
                    self.call_rt(Rt::SbStr, &[builder, comma]);
                }
                let part = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Field(index as u32));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, item)
                    }
                    _ => Val::Zero,
                };
                self.append_display(builder, item, part);
            }
            let close = self.string_ptr(")");
            self.call_rt(Rt::SbStr, &[builder, close]);
            return;
        }
        if let TyKind::Option(inner) = *types.kind(types.repr(ty)) {
            let tag = self.tag(ty, value);
            let (some, none, done) = (self.fb.create_block(), self.fb.create_block(), self.fb.create_block());
            let is_some = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, SOME as i64);
            self.fb.ins().brif(is_some, some, &[], none, &[]);
            self.fb.switch_to_block(some);
            let open = self.string_ptr("some(");
            self.call_rt(Rt::SbStr, &[builder, open]);
            let payload = match value {
                Val::Memory(address) => {
                    let slot = self.cg.slot(ty, Proj::Variant(SOME, 0));
                    let address = self.component_address(address, &slot);
                    self.value_at(address, inner)
                }
                _ => Val::Zero,
            };
            self.append_display(builder, inner, payload);
            let close = self.string_ptr(")");
            self.call_rt(Rt::SbStr, &[builder, close]);
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(none);
            let text = self.string_ptr("none");
            self.call_rt(Rt::SbStr, &[builder, text]);
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(done);
            return;
        }
        match types.shown.get(&ty) {
            Some(Shown::Duration) => {
                let Val::Scalar(nanos) = value else { unreachable!("a duration is a scalar") };
                self.call_rt(Rt::SbDuration, &[builder, nanos]);
                return;
            }
            Some(Shown::ProcessError) => {
                // The code of `Failed`, the variant with a payload; other
                // variants ignore it.
                let tag = self.tag(ty, value);
                let tag = self.fb.ins().uextend(types::I64, tag);
                let Val::Memory(address) = value else { unreachable!("an error with payloads is in memory") };
                let payload = self.cg.slot(ty, Proj::Variant(3, 0));
                let status = self.component_address(address, &payload);
                let code_slot = self.cg.slot(payload.ty, Proj::Field(0));
                let code = self.fb.ins().load(types::I64, flags(), status, code_slot.offset as i32);
                self.call_rt(Rt::SbProcessError, &[builder, tag, code]);
                return;
            }
            Some(Shown::DateTime) => {
                let Val::Memory(address) = value else { unreachable!("a date is in memory") };
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 56, 3);
                let slot = self.fb.create_sized_stack_slot(data);
                let fields = self.fb.ins().stack_addr(POINTER, slot, 0);
                for index in 0..7 {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    let word = self.fb.ins().load(types::I64, flags(), address, slot.offset as i32);
                    self.fb.ins().store(flags(), word, fields, index as i32 * 8);
                }
                self.call_rt(Rt::SbDateTime, &[builder, fields]);
                return;
            }
            _ => {}
        }
        if let TyKind::Adt(id) = *types.kind(types.repr(ty))
            && let AdtKind::Enum(variants) = &types.adt(id).kind
        {
            // A standard library error: its variant name in words, or the
            // system's text of the error a variant holds the number of;
            // another enum of plain variants, its variant's name.
            let in_words = types.shown.get(&ty) == Some(&Shown::Words);
            let label = |name: &str| if in_words { words(name) } else { name.to_string() };
            let tag = self.tag(ty, value);
            let host = variants.iter().position(
                |v| matches!(&v.payload, crate::types::Payload::Tuple(items) if items.as_slice() == [Ty::INT]),
            );
            if let (Some(host), Val::Memory(address)) = (host, value) {
                let (is_host, other, done) = (self.fb.create_block(), self.fb.create_block(), self.fb.create_block());
                let matched = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, host as i64);
                self.fb.ins().brif(matched, is_host, &[], other, &[]);
                self.fb.switch_to_block(is_host);
                let slot = self.cg.slot(ty, Proj::Variant(host as u32, 0));
                let number = self.fb.ins().load(types::I64, flags(), address, slot.offset as i32);
                self.call_rt(Rt::SbOsError, &[builder, number]);
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(other);
                let mut text = self.string_ptr(&label(&variants[0].name));
                for (index, variant) in variants.iter().enumerate().skip(1) {
                    let this = self.string_ptr(&label(&variant.name));
                    let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
                    text = self.fb.ins().select(is_this, this, text);
                }
                self.call_rt(Rt::SbStr, &[builder, text]);
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(done);
                return;
            }
            let mut text = self.string_ptr(&label(&variants[0].name));
            for (index, variant) in variants.iter().enumerate().skip(1) {
                let this = self.string_ptr(&label(&variant.name));
                let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
                text = self.fb.ins().select(is_this, this, text);
            }
            self.call_rt(Rt::SbStr, &[builder, text]);
            return;
        }
        let Val::Scalar(value) = value else { unreachable!("no Display for aggregates") };
        match *types.kind(types.repr(ty)) {
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
            TyKind::Int(int) if int.bits() == 128 => {
                let (low, high) = self.fb.ins().isplit(value);
                let signed = self.fb.ins().iconst(types::I8, int.signed() as i64);
                self.call_rt(Rt::SbInt128, &[builder, low, high, signed]);
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

    /// Appends a value as a debugger shows it: its structure, whatever
    /// `Display` it has, with texts in quotes. `address` is where it is.
    /// Components that are not scalars go through their own glue, so a
    /// recursive type does not expand forever.
    fn append_show(&mut self, builder: ir::Value, ty: Ty, value: Val, address: ir::Value) {
        let types = self.cg.types();
        let repr = types.repr(ty);
        let text = |e: &mut Self, text: &str| {
            let text = e.string_ptr(text);
            e.call_rt(Rt::SbStr, &[builder, text]);
        };
        // A component: scalars here, the rest by its glue.
        let part = |e: &mut Self, ty: Ty, slot: &Slot| {
            let at = e.component_address(address, slot);
            match e.cg.class(slot.ty) {
                Class::Zero => text(e, "()"),
                _ => {
                    e.call_glue(Glue::Show, slot.ty, &[builder, at]);
                }
            }
            let _ = ty;
        };
        match types.kind(repr).clone() {
            TyKind::String => {
                let Val::Scalar(value) = value else { unreachable!("a string is a pointer") };
                self.call_rt(Rt::SbQuoted, &[builder, value]);
            }
            TyKind::Char => {
                text(self, "'");
                self.append_display(builder, ty, value);
                text(self, "'");
            }
            TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Byte => self.append_display(builder, ty, value),
            TyKind::Unit => text(self, "()"),
            TyKind::Tuple(items) => {
                text(self, "(");
                for index in 0..items.len() {
                    if index > 0 {
                        text(self, ", ");
                    }
                    let slot = self.cg.slot(ty, Proj::Field(index as u32));
                    part(self, ty, &slot);
                }
                text(self, ")");
            }
            TyKind::Array(element) | TyKind::Deque(element) => {
                let Val::Scalar(value) = value else { unreachable!("a sequence is a pointer") };
                let size = self.element_size(element);
                let show = self.glue_address(Glue::Show, element);
                self.call_rt(Rt::SbArray, &[builder, value, size, show]);
            }
            TyKind::Map(key, entry) => {
                let Val::Scalar(value) = value else { unreachable!("a map is a pointer") };
                let (key, entry) = (self.glue_address(Glue::Show, key), self.glue_address(Glue::Show, entry));
                self.call_rt(Rt::SbMap, &[builder, value, key, entry]);
            }
            TyKind::Set(element) => {
                let Val::Scalar(value) = value else { unreachable!("a set is a pointer") };
                let element = self.glue_address(Glue::Show, element);
                let none = self.fb.ins().iconst(POINTER, 0);
                self.call_rt(Rt::SbMap, &[builder, value, element, none]);
            }
            TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) | TyKind::Adt(_)
                if !matches!(value, Val::Zero) || types.variants(ty).is_some() =>
            {
                // Each variant by its name, then its payload.
                let names: Vec<String> = match types.kind(repr) {
                    TyKind::Option(_) => vec!["none".into(), "some".into()],
                    TyKind::Result(..) => vec!["ok".into(), "err".into()],
                    TyKind::Union(members) => members.iter().map(|&m| format!("{}: ", types.display(m))).collect(),
                    TyKind::Adt(id) => match &types.adt(*id).kind {
                        AdtKind::Enum(variants) => {
                            variants.iter().map(|v| format!("{}.{}", types.adt(*id).name, v.name)).collect()
                        }
                        AdtKind::Record(fields) => {
                            text(self, &format!("{} {{ ", types.adt(*id).name));
                            for (index, field) in fields.iter().enumerate() {
                                text(self, &format!("{}{}: ", if index > 0 { ", " } else { "" }, field.name));
                                if field.secret {
                                    text(self, SECRET);
                                    continue;
                                }
                                let slot = self.cg.slot(ty, Proj::Field(index as u32));
                                part(self, ty, &slot);
                            }
                            text(self, " }");
                            return;
                        }
                        AdtKind::Newtype(_) => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                let Some(variants) = types.variants(ty) else {
                    text(self, &types.display(ty));
                    return;
                };
                // The fields of each variant the tools do not show.
                let hidden = |variant: usize, index: usize| match types.kind(repr) {
                    TyKind::Adt(id) => match &types.adt(*id).kind {
                        AdtKind::Enum(variants) => match &variants[variant].payload {
                            crate::types::Payload::Record(fields) => fields[index].secret,
                            _ => false,
                        },
                        _ => false,
                    },
                    _ => false,
                };
                let tag = self.tag(ty, value);
                let done = self.fb.create_block();
                let union = matches!(types.kind(repr), TyKind::Union(_));
                for (variant, components) in variants.iter().enumerate() {
                    let (this, next) = (self.fb.create_block(), self.fb.create_block());
                    let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                    self.fb.ins().brif(is_this, this, &[], next, &[]);
                    self.fb.switch_to_block(this);
                    text(self, &names[variant]);
                    if !components.is_empty() {
                        if !union {
                            text(self, "(");
                        }
                        for index in 0..components.len() {
                            if index > 0 {
                                text(self, ", ");
                            }
                            if hidden(variant, index) {
                                text(self, SECRET);
                                continue;
                            }
                            let slot = self.cg.slot(ty, Proj::Variant(variant as u32, index as u32));
                            part(self, ty, &slot);
                        }
                        if !union {
                            text(self, ")");
                        }
                    }
                    self.fb.ins().jump(done, &[]);
                    self.fb.switch_to_block(next);
                }
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(done);
            }
            // What a value with an identity holds, read in place, as a
            // record whose field is what `.value` reads in the program.
            TyKind::Handle(handle @ (Handle::Ref | Handle::Mutex | Handle::RwLock | Handle::Atomic), state) => {
                let Val::Scalar(value) = value else { unreachable!("a handle is a pointer") };
                text(self, &format!("{} {{ value: ", handle.name()));
                let kind = self.fb.ins().iconst(types::I8, i64::from(handle == Handle::Ref));
                let at = self.call_rt(Rt::DebugState, &[value, kind]).expect("an address");
                match self.cg.class(state) {
                    Class::Zero => text(self, "()"),
                    _ => {
                        self.call_glue(Glue::Show, state, &[builder, at]);
                    }
                }
                text(self, " }");
            }
            _ => text(self, &types.display(ty)),
        }
    }

    fn string_ptr(&mut self, text: &str) -> ir::Value {
        let id = self.cg.string(text);
        let global = self.global(id);
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

    /// Where a reference's value is: after its header, or in a live build,
    /// which keeps it apart, where the header says.
    fn ref_value(&mut self, reference: ir::Value) -> ir::Value {
        match self.cg.live() {
            true => self.fb.ins().load(POINTER, flags(), reference, REF_HEADER as i32),
            false => self.fb.ins().iadd_imm_s(reference, REF_HEADER),
        }
    }

    fn copy_memory(&mut self, destination: ir::Value, source: ir::Value, size: u32) {
        if size > 64 {
            let config = self.cg.isa().frontend_config();
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
        if let Shape::Niche = self.cg.layout(ty).shape {
            let address = match value {
                Val::Scalar(address) => address,
                Val::Memory(at) => self.fb.ins().load(POINTER, flags(), at, 0),
                Val::Zero => unreachable!("an address is not zero-sized"),
            };
            return self.fb.ins().icmp_imm_u(IntCC::NotEqual, address, 0);
        }
        match value {
            Val::Scalar(tag) => tag,
            Val::Memory(address) => {
                let tag = self.cg.scalar_type(self.cg.types().tag_ty(ty));
                self.fb.ins().load(tag, flags(), address, 0)
            }
            Val::Zero => self.fb.ins().iconst(types::I8, 0),
        }
    }

    /// The name of the type of a value: of the member a union holds.
    fn type_name(&mut self, ty: Ty, value: Val) -> ir::Value {
        let types = self.cg.types();
        let Some(members) = types.members(types.repr(ty)).map(<[Ty]>::to_vec) else {
            let name = types.display(ty);
            return self.string_ptr(&name);
        };
        let tag = self.tag(ty, value);
        let done = self.fb.create_block();
        self.fb.append_block_param(done, POINTER);
        for (variant, &member) in members.iter().enumerate() {
            let (this, next) = (self.fb.create_block(), self.fb.create_block());
            let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
            self.fb.ins().brif(is_this, this, &[], next, &[]);
            self.fb.switch_to_block(this);
            let name = self.cg.types().display(member);
            let name = self.string_ptr(&name);
            self.fb.ins().jump(done, &[name.into()]);
            self.fb.switch_to_block(next);
        }
        let name = self.cg.types().display(ty);
        let fallback = self.string_ptr(&name);
        self.fb.ins().jump(done, &[fallback.into()]);
        self.fb.switch_to_block(done);
        self.fb.block_params(done)[0]
    }

    /// A string describing an error that reached the end of `main`: its text
    /// for a `String`, `Type.Variant` for an enum, otherwise the type name.
    /// With `display`, a type's own `Display` comes first, and the string
    /// is owned.
    fn describe(&mut self, ty: Ty, value: Val, display: bool) -> ir::Value {
        if display && let Some(&id) = self.cg.display().get(&ty) {
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let call = call_body(self.cg, &mut self.fb, id, &args);
            return self.fb.inst_results(call)[0];
        }
        // A standard error, a duration or a date: the text interpolation
        // gives it.
        if display && self.cg.types().shown.contains_key(&ty) {
            let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
            let slot = self.fb.create_sized_stack_slot(data);
            let builder = self.fb.ins().stack_addr(POINTER, slot, 0);
            let room = self.fb.ins().iconst(types::I64, 32);
            self.call_rt(Rt::SbStart, &[builder, room]);
            self.append_display(builder, ty, value);
            return self.call_rt(Rt::SbFinish, &[builder]).unwrap();
        }
        let types = self.cg.types();
        let repr = types.repr(ty);
        if repr == Ty::STRING
            && let Val::Scalar(text) = value
        {
            if display {
                self.call_rt(Rt::StrRetain, &[text]);
            }
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
                let description = self.describe(member, payload, display);
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
        if !self.cg.types().needs_drop(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => match *self.cg.types().kind(self.cg.types().repr(ty)) {
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
                TyKind::Fn(..) => {
                    self.call_rt(if release { Rt::FnRelease } else { Rt::FnRetain }, &[pointer]);
                }
                TyKind::Handle(Handle::Ref, value) if release => {
                    let glue = self.glue_or_null(Glue::Release, value);
                    self.call_rt(Rt::RefRelease, &[pointer, glue]);
                }
                TyKind::Handle(Handle::Weak, _) => {
                    self.call_rt(if release { Rt::WeakRelease } else { Rt::WeakRetain }, &[pointer]);
                }
                TyKind::Handle(handle, _) if release => {
                    let rt = match handle {
                        Handle::Join => Rt::JoinRelease,
                        Handle::Group => Rt::GroupRelease,
                        Handle::Sender => Rt::SenderRelease,
                        Handle::Receiver => Rt::ReceiverRelease,
                        Handle::Semaphore => Rt::SemaphoreRelease,
                        Handle::Permit => Rt::PermitRelease,
                        Handle::Barrier => Rt::BarrierRelease,
                        Handle::Guard => Rt::SyncUnlock,
                        Handle::SyncMap | Handle::SyncSet => Rt::SharedMapRelease,
                        Handle::SyncQueue => Rt::SharedQueueRelease,
                        Handle::SyncStack => Rt::SharedStackRelease,
                        Handle::SyncArray => Rt::SharedArrayRelease,
                        Handle::File | Handle::Writer | Handle::Reader => Rt::StreamRelease,
                        Handle::Buffer => Rt::BufferRelease,
                        Handle::Command | Handle::Pipeline => Rt::PlanRelease,
                        Handle::Process => Rt::ProcessRelease,
                        Handle::JsonReader => Rt::JsonReaderRelease,
                        Handle::JsonWriter => Rt::JsonWriterRelease,
                        Handle::Clock => Rt::ClockRelease,
                        Handle::Regex => Rt::RegexRelease,
                        Handle::Native => Rt::NativeRelease,
                        Handle::ReflectBox => Rt::ReflectRelease,
                        _ => Rt::SyncRelease,
                    };
                    self.call_rt(rt, &[pointer]);
                }
                // A guard is held once and never copied.
                TyKind::Handle(Handle::Guard, _) => {}
                TyKind::Handle(..) => {
                    self.call_rt(Rt::HandleRetain, &[pointer]);
                }
                _ => {
                    self.call_rt(if release { Rt::StrRelease } else { Rt::StrRetain }, &[pointer]);
                }
            },
            (Val::Memory(base), Shape::Struct(_)) => {
                let count = self.cg.types().components(ty).unwrap().len() as u32;
                for index in 0..count {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    self.refcount_slot(base, &slot, release);
                }
            }
            (Val::Memory(address), Shape::Enum { .. }) => {
                let kind = if release { Glue::Release } else { Glue::Retain };
                // A variant that holds nothing counted calls nothing, as an
                // `ok` of a number does: its tag is read here, and only the
                // others go through the glue.
                let variants = self.cg.types().variants(ty).unwrap();
                let counted: Vec<i64> = (0..variants.len())
                    .filter(|&variant| {
                        (0..variants[variant].len()).any(|index| {
                            let slot = self.cg.slot(ty, Proj::Variant(variant as u32, index as u32));
                            slot.boxed || self.cg.types().needs_drop(slot.ty)
                        })
                    })
                    .map(|variant| variant as i64)
                    .collect();
                if counted.len() == variants.len() {
                    self.call_glue(kind, ty, &[address]);
                    return;
                }
                let tag = self.tag(ty, Val::Memory(address));
                let mut holds = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, counted[0]);
                for &variant in &counted[1..] {
                    let this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant);
                    holds = self.fb.ins().bor(holds, this);
                }
                let (call, done) = (self.fb.create_block(), self.fb.create_block());
                self.fb.set_cold_block(call);
                self.fb.ins().brif(holds, call, &[], done, &[]);
                self.fb.switch_to_block(call);
                let cold = if release { Glue::ReleaseCold } else { Glue::RetainCold };
                self.call_glue(cold, ty, &[address]);
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(done);
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
        } else if self.cg.types().needs_drop(slot.ty) {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.refcount(slot.ty, value, release);
        }
    }

    /// Reports to the cycle collector's `visitor` the references inside
    /// `value` that may lead to a cycle (see `Types::can_cycle`).
    fn visit(&mut self, ty: Ty, value: Val, visitor: ir::Value) {
        if !self.cg.types().can_cycle(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => {
                let zero = self.fb.ins().iconst(POINTER, 0);
                // The kinds of `cycles::Kind` in the runtime.
                let (kind, a, b) = match *self.cg.types().kind(self.cg.types().repr(ty)) {
                    TyKind::Array(element) | TyKind::Deque(element) => {
                        (2, self.element_size(element), self.glue_or_null(Glue::Visit, element))
                    }
                    TyKind::Map(..) | TyKind::Set(_) => {
                        let (key, value) = self.cg.map_parts(ty);
                        (3, self.glue_or_null(Glue::Visit, key), self.glue_or_null(Glue::Visit, value))
                    }
                    TyKind::Fn(..) => (4, zero, zero),
                    TyKind::Handle(Handle::Ref, inner) => (1, self.glue_or_null(Glue::Visit, inner), zero),
                    TyKind::Handle(handle, _) => {
                        let kind = match handle {
                            Handle::Mutex | Handle::RwLock | Handle::Atomic | Handle::Once => 5,
                            Handle::Receiver => 6,
                            Handle::Sender => 7,
                            Handle::Join => 8,
                            Handle::Group => 9,
                            Handle::SyncMap | Handle::SyncSet => 10,
                            Handle::SyncQueue => 11,
                            Handle::SyncStack => 12,
                            Handle::SyncArray => 13,
                            Handle::ReflectBox => 14,
                            _ => return,
                        };
                        (kind, zero, zero)
                    }
                    _ => return,
                };
                let kind = self.fb.ins().iconst(types::I8, kind);
                self.call_rt(Rt::GcEdge, &[visitor, pointer, kind, a, b]);
            }
            (Val::Memory(base), Shape::Struct(_)) => {
                let count = self.cg.types().components(ty).unwrap().len() as u32;
                for index in 0..count {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    self.visit_slot(base, &slot, visitor);
                }
            }
            (Val::Memory(address), Shape::Enum { .. }) => {
                self.call_glue(Glue::Visit, ty, &[address, visitor]);
            }
            _ => {}
        }
    }

    fn visit_slot(&mut self, base: ir::Value, slot: &Slot, visitor: ir::Value) {
        if !self.cg.types().can_cycle(slot.ty) {
            return;
        }
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            let glue = self.glue_address(Glue::Visit, slot.ty);
            let kind = self.fb.ins().iconst(types::I8, 0);
            let zero = self.fb.ins().iconst(POINTER, 0);
            self.call_rt(Rt::GcEdge, &[visitor, cell, kind, glue, zero]);
        } else {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.visit(slot.ty, value, visitor);
        }
    }

    /// Structural equality (8.11): 1 when equal, 0 otherwise.
    fn equal(&mut self, ty: Ty, a: Val, b: Val) -> ir::Value {
        match (a, b) {
            (Val::Scalar(a), Val::Scalar(b)) => match *self.cg.types().kind(self.cg.types().repr(ty)) {
                TyKind::String => self.call_rt(Rt::StrEq, &[a, b]).unwrap(),
                TyKind::Array(Ty::BYTE) => self.call_rt(Rt::BytesEq, &[a, b]).unwrap(),
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
                    let count = self.cg.types().components(ty).unwrap().len() as u32;
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

/// A point's state: each local live there, how it is kept, its offset and
/// size; and the state's size.
type StateLayout = (Vec<(Local, StateKept, u32, u32)>, u32);

/// What a patch converts in place in values with identity that hold a
/// type of another shape: its conversion, by its place among the patch's,
/// the identities of the old type and of the new, the new one's size and
/// alignment, and its release and visit glue.
type LiveObject = (usize, DataId, DataId, u32, u32, Option<FuncId>, Option<FuncId>);

/// How a live build keeps a local of a point's state.
#[derive(Clone, Copy)]
enum StateKept {
    Var(Variable, ir::Type),
    Pointer(Variable),
    Slot(ir::StackSlot),
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
    e: Emit<'a, dyn Gen<'b> + 'a>,
    body: &'a Body,
    /// Every function, for named functions used as values.
    bodies: &'a [Body],
    blocks: Vec<ir::Block>,
    storage: Vec<Storage>,
    /// Where the statement being compiled is, for panics of the runtime.
    span: Span,
    sites: Vec<Site>,
    /// The inlined call the block being compiled comes from.
    scope: Option<u32>,
    /// In a debug build, what its probes show.
    debug: Option<DebugFrame>,
    /// In a build without probes, the value label of each named variable.
    labels: HashMap<u32, u32>,
    /// Where an explicit panic keeps the numbers a dump shows, and which.
    keeping: Option<(ir::StackSlot, Vec<(Local, Variable)>)>,
    /// The block being built.
    block: usize,
    /// What the probe just placed answered: whether to restart the frame;
    /// the local that holds it, and the line each block a restart point
    /// leads to is on.
    probed: Option<ir::Value>,
    /// The blocks a latent site stays out of, the line of the last, and
    /// what they call.
    tight_loops: Vec<bool>,
    latent_line: Option<usize>,
    latent_callee: Option<ir::FuncRef>,
    carrying: bool,
    carried: HashMap<usize, (String, usize)>,
    /// The site of the probe just placed, as a value and its number; that
    /// of each point with a probe; and the state of the call being made.
    site_now: Option<(ir::Value, u32)>,
    point_sites: HashMap<u32, ir::Value>,
    osr_arg: Option<ir::Value>,
    state_slot: Option<ir::StackSlot>,
    /// Where the transfer leaves the frame's parameters, a word each, which
    /// the runtime converts for a function of another identity; and where
    /// the call to the new code takes them from.
    params_slot: Option<ir::StackSlot>,
    osr_params: Option<ir::Value>,
    /// With coverage: the counters, the line counted last in the block,
    /// and the ways of branches by the block and statement they start at.
    counters: Option<ir::GlobalValue>,
    counted: Option<(u32, usize)>,
    starts: HashMap<(usize, usize), Vec<Span>>,
    ways: HashMap<(usize, usize), Vec<usize>>,
    /// Locals that only ever hold a string literal, which counts no
    /// references: retaining and releasing them does nothing; and the
    /// literal, when it is always the same.
    literal: HashMap<Local, Option<String>>,
    /// The room in the frame of each text made there (`Body.framed`).
    framed: HashMap<Local, ir::StackSlot>,
    /// While a statement that a check for a panic follows is built: where
    /// its panics unwind to (15.9). And whether that check follows one
    /// whose panics all jumped there, so that it asks nothing.
    unwind_to: Option<ir::Block>,
    quiet_check: bool,
    /// Whether the statement being built called compiled code with a
    /// `try_call`, and whether the block it is in returns: where a panic
    /// that unwinds the function goes to its caller's way.
    tried: bool,
    returns: bool,
}

/// What a function of a debug build keeps for its probes (toolchain 7.6):
/// its index among the debug table's functions; each binding it shows, and
/// for one in a variable the slot its value is copied to when a probe
/// runs; a bit for each that holds a value now; their addresses, for the
/// probe; and the line the last probe was for, in the block.
struct DebugFrame {
    function: u32,
    /// In a live build, the word that holds the number of the function's
    /// first site, and that number: its probes count from it, so that the
    /// code of a function that did not change stays the same.
    first: Option<(DataId, usize)>,
    shown: Vec<(Local, Option<ir::StackSlot>)>,
    /// A closure's captures, after `shown`: where each is in the closure's
    /// environment, its first parameter, and whether it is boxed there.
    captured: Vec<(u32, bool)>,
    alive: ir::StackSlot,
    addresses: ir::StackSlot,
    line: Option<(String, usize)>,
    /// The line the only block that leads to this one ended in.
    before: Option<(String, usize)>,
    /// The site of the last probe.
    site: usize,
}

impl<'a, 'b> FnCodegen<'a, 'b> {
    fn new(
        cg: &'a mut (dyn Gen<'b> + 'a),
        body: &'a Body,
        bodies: &'a [Body],
        func: &'a mut ir::Function,
        fbc: &'a mut FunctionBuilderContext,
    ) -> Self {
        let mut fb = FunctionBuilder::new(func, fbc);
        let blocks = body.blocks.iter().map(|_| fb.create_block()).collect();
        let mut storage = Vec::with_capacity(body.locals.len());
        // Locals that hold the address of an element they borrow.
        let lent: HashSet<Local> = body
            .blocks
            .iter()
            .flat_map(|b| &b.stmts)
            .filter_map(|s| match s {
                Statement::Lend(local, _) | Statement::LendAt(local, _) => Some(*local),
                _ => None,
            })
            .collect();
        for (index, &ty) in body.locals.iter().enumerate() {
            let local = Local(index as u32);
            let borrowed = body.params.contains(&local) || body.ret == Some(local);
            let loan =
                body.params.iter().position(|&p| p == local).is_some_and(|i| body.loans[i]) || lent.contains(&local);
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
        // A build without probes says where the compiler left its named
        // values, for a dump: no code changes for it.
        let mut labels = HashMap::new();
        let mut kept = Vec::new();
        let panics = body.blocks.iter().any(|block| matches!(block.terminator, Terminator::Panic { .. }));
        if cg.dumps() {
            fb.func.collect_debug_info();
            let mut described = Vec::new();
            // A slot nothing writes holds nothing, as a parameter the
            // inliner found unused: its zeros would read as a value.
            let written = crate::mir::written(body);
            let own = body.names.iter().enumerate().filter_map(|(index, name)| Some((index, name.as_ref()?, None)));
            let inlined = body.scoped.iter().map(|(local, name, scope)| (local.0 as usize, name, Some(*scope)));
            for (index, name, scope) in own.chain(inlined) {
                let place = match storage[index] {
                    Storage::Zero => continue,
                    Storage::Var(var) => {
                        labels.insert(var.as_u32(), index as u32);
                        DumpPlace::Value(index as u32)
                    }
                    Storage::Pointer(var) => {
                        labels.insert(var.as_u32(), index as u32);
                        DumpPlace::Pointer(index as u32)
                    }
                    Storage::Slot(_) if written[index].is_none() => continue,
                    Storage::Slot(slot) => DumpPlace::Slot(slot.as_u32()),
                };
                let ty = body.locals[index];
                // A number the code no longer needs is gone by the time
                // it panics: a panic the program wrote keeps it, in code
                // that only that panic runs. What the heap holds may be
                // moved by then, and only what is live is shown.
                let number = matches!(
                    cg.types().kind(ty),
                    TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Byte
                ) && cg.layout(ty).size <= 8;
                let keep = match (number, storage[index]) {
                    (true, Storage::Var(var)) if panics => {
                        let local = Local(index as u32);
                        let at = kept.iter().position(|&(known, _)| known == local).unwrap_or_else(|| {
                            kept.push((local, var));
                            kept.len() - 1
                        });
                        Some(at as u32)
                    }
                    _ => None,
                };
                // What a slot holds before its first line is not the
                // binding's yet.
                let from = written[index].flatten().filter(|_| matches!(storage[index], Storage::Slot(_)));
                described.push((name.clone(), ty, cg.glue(Glue::Show, ty), place, keep, scope, from));
            }
            cg.dump_bindings(described);
        }
        let keeping = (!kept.is_empty()).then(|| {
            let size = (kept.len() as u32 + 1) * 8;
            (fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3)), kept)
        });
        let debug = cg.debug().then(|| {
            let mut shown = Vec::new();
            let mut described = Vec::new();
            for (index, name) in body.names.iter().enumerate() {
                let (Some(name), local) = (name, Local(index as u32)) else { continue };
                let copy = match storage[index] {
                    Storage::Zero => continue,
                    // A copy of a register's value: a word, or two.
                    Storage::Var(_) => {
                        let size = cg.layout(body.locals[index]).size.max(8);
                        Some(fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3)))
                    }
                    Storage::Slot(_) | Storage::Pointer(_) => None,
                };
                shown.push((local, copy));
                let ty = body.locals[index];
                let raw = cg.views().contains_key(&ty).then(|| cg.glue(Glue::ShowRaw, ty));
                described.push((name.clone(), ty, cg.glue(Glue::Show, ty), raw));
            }
            // What a closure captured is a binding of its body too, read
            // from its environment; but not in one that gives its captures
            // away, which may have.
            let mut captured = Vec::new();
            let env = body.params.first().map(|local| body.locals[local.0 as usize]);
            if let Some(env) = env.filter(|&env| cg.types().closure_calls(env).is_some_and(|(_, once)| !once))
                && let TyKind::Adt(id) = *cg.types().kind(env)
                && let AdtKind::Record(fields) = cg.types().adt(id).kind.clone()
            {
                for (index, field) in fields.iter().enumerate() {
                    let slot = cg.slot(env, Proj::Field(index as u32));
                    if matches!(cg.class(slot.ty), Class::Zero) {
                        continue;
                    }
                    captured.push((slot.offset, slot.boxed));
                    let raw = cg.views().contains_key(&slot.ty).then(|| cg.glue(Glue::ShowRaw, slot.ty));
                    described.push((field.name.clone(), slot.ty, cg.glue(Glue::Show, slot.ty), raw));
                }
            }
            let count = shown.len() + captured.len();
            let words = count.div_ceil(64).max(1) as u32;
            let function = cg.debug_function(body.name.clone(), body.loans.contains(&true), described);
            // Keyed by the function and which they are, so that where they
            // are in its frame goes in the debugger's table.
            let slot = |size: u32, which: u64| {
                let key = ir::StackSlotKey::new(u64::from(function) << 1 | which);
                StackSlotData::new_with_key(StackSlotKind::ExplicitSlot, size, 3, key)
            };
            let alive = fb.create_sized_stack_slot(slot(words * 8, 0));
            let addresses = fb.create_sized_stack_slot(slot((count.max(1) * 8) as u32, 1));
            let first = cg.function_sites();
            DebugFrame { function, first, shown, captured, alive, addresses, line: None, before: None, site: 0 }
        });
        FnCodegen {
            e: Emit::new(cg, fb),
            body,
            bodies,
            blocks,
            storage,
            span: Span::default(),
            sites: Vec::new(),
            scope: None,
            debug,
            labels,
            keeping,
            block: 0,
            probed: None,
            tight_loops: crate::mir::tight_loops(&body.blocks),
            latent_line: None,
            latent_callee: None,
            carrying: false,
            carried: HashMap::new(),
            site_now: None,
            point_sites: HashMap::new(),
            osr_arg: None,
            state_slot: None,
            params_slot: None,
            osr_params: None,
            counters: None,
            counted: None,
            starts: HashMap::new(),
            ways: HashMap::new(),
            literal: literals(body),
            framed: HashMap::new(),
            unwind_to: None,
            quiet_check: false,
            tried: false,
            returns: false,
        }
    }

    /// A variable has `value` from here on, which says so for a dump.
    fn define(&mut self, var: Variable, value: ir::Value) {
        self.e.fb.def_var(var, value);
        if let Some(&label) = self.labels.get(&var.as_u32()) {
            self.e.fb.set_val_label(value, ir::ValueLabel::from_u32(label));
        }
    }

    /// Builds the function's IR; gives where its statements are, which the
    /// source locations of its instructions index.
    fn run(mut self) -> Vec<Site> {
        let entry = self.blocks[0];
        self.e.fb.append_block_params_for_function_params(entry);
        self.e.fb.switch_to_block(entry);
        let mut incoming = self.e.fb.block_params(entry).to_vec().into_iter();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            self.define(var, incoming.next().unwrap());
        }
        for (index, &param) in self.body.params.iter().enumerate() {
            match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.define(var, incoming.next().unwrap()),
                // A loan of a zero-sized place still has its address parameter.
                Storage::Zero if self.body.loans[index] => {
                    incoming.next();
                }
                Storage::Zero | Storage::Slot(_) => {}
            }
        }
        if let Some(osr) = self.body.osr {
            let state = incoming.next().expect("the hidden parameter");
            self.write(&Place::local(osr), Val::Scalar(state));
        }
        if let Some(debug) = &self.debug {
            let zero = self.e.fb.ins().iconst(types::I64, 0);
            let (shown, captured, alive) = (debug.shown.len(), debug.captured.len(), debug.alive);
            for word in 0..(shown + captured).div_ceil(64) {
                self.e.fb.ins().stack_store(POINTER, zero, alive, word as i32 * 8);
            }
            for &param in &self.body.params {
                self.mark(param, true);
            }
            // A closure's captures hold their values from its start.
            for index in shown..shown + captured {
                let (offset, bit) = ((index / 64) as i32 * 8, 1i64 << (index % 64));
                let word = self.e.fb.ins().stack_load(POINTER, types::I64, alive, offset);
                let word = self.e.fb.ins().bor_imm_s(word, bit);
                self.e.fb.ins().stack_store(POINTER, word, alive, offset);
            }
        }
        // Where statements start: coverage counts them, and a debug build
        // has a probe at each, also one that computes nothing, as
        // `let ready = true`.
        if self.e.cg.coverage().is_some() || self.debug.is_some() {
            for &(block, at, span) in &self.body.starts {
                self.starts.entry((block.0 as usize, at)).or_default().push(span);
            }
        }
        if self.e.cg.coverage().is_some() {
            for (n, way) in self.body.ways.iter().enumerate() {
                self.ways.entry((way.block.0 as usize, way.at)).or_default().push(n);
            }
        }
        // A value that comes into a block is a value of its own, which says
        // whose it is: only those it reads, so no code changes for it.
        let live = (!self.labels.is_empty()).then(|| crate::mir::live_in(&self.body.blocks, self.body.locals.len()));
        // The block that alone leads to each, and the line each ended in.
        let from = only_before(&self.body.blocks);
        let mut ended: Vec<Option<(String, usize)>> = vec![None; self.body.blocks.len()];
        for (index, block) in self.body.blocks.iter().enumerate() {
            self.scope = self.body.scopes.get(index).copied().flatten();
            if index > 0 {
                self.e.fb.switch_to_block(self.blocks[index]);
            } else if self.e.cg.meta() {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            self.counted = None;
            self.block = index;
            if index == 0 {
                self.count_call();
            } else if let Some(live) = &live {
                for local in &live[index] {
                    if let Storage::Var(var) | Storage::Pointer(var) = self.storage[local.0 as usize]
                        && let Some(&label) = self.labels.get(&var.as_u32())
                    {
                        let value = self.e.fb.use_var(var);
                        self.e.fb.set_val_label(value, ir::ValueLabel::from_u32(label));
                    }
                }
            }
            if let Some(debug) = &mut self.debug {
                debug.line = self.carried.remove(&index);
                // The blocks of a restart point carry its line on; the one
                // the statement goes on in only takes it.
                self.carrying =
                    debug.line.is_some() && !self.body.points.iter().any(|p| p.continues.0 as usize == index);
                // Code that only follows the one before goes on in its line,
                // as after a call that may panic: its probe sees the line
                // go on, for what it changed, and stops nobody there.
                debug.before = from[index].and_then(|before| ended[before].clone());
            }
            let returns = matches!(block.terminator, Terminator::Return(_));
            let last_resume = block.stmts.iter().rposition(resumes);
            let mut quiet = false;
            // A check of the runtime that ends the block and the branch on
            // it are one, emitted after the rest (`fused_branch`).
            let fused = self.fused_check(block);
            let first = fused.as_ref().map(|fused| block.stmts.len() - fused.statements);
            let mut check_quiet = false;
            for (at, stmt) in block.stmts.iter().enumerate() {
                self.count_ways(index, at);
                if first == Some(at) {
                    check_quiet = quiet;
                    break;
                }
                // The panic goes to the caller from the last `UnwindResume`
                // of a block that returns.
                self.returns = returns && last_resume.is_none_or(|last| last <= at);
                // A statement a check for a panic follows: its panics
                // unwind to where the check goes (15.9). The check asks
                // nothing after one whose panics all went there.
                self.unwind_to = unwind_target(block, at).map(|target| self.blocks[target.0 as usize]);
                self.quiet_check = quiet;
                self.tried = false;
                quiet = self.inline_only(stmt);
                self.statement(stmt);
                quiet |= self.tried;
            }
            self.unwind_to = None;
            for at in first.map_or(block.stmts.len(), |first| first + 1)..block.stmts.len() {
                self.count_ways(index, at);
            }
            self.count_ways(index, block.stmts.len());
            // A jump back to an earlier block closes a loop: a step.
            let back = match &block.terminator {
                Terminator::Goto(target) => target.0 as usize <= index,
                Terminator::Branch(_, then, otherwise) => then.0 as usize <= index || otherwise.0 as usize <= index,
                _ => false,
            };
            if back && self.e.cg.meta() {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            if let Some(fused) = fused {
                self.fused_branch(fused, check_quiet);
                ended[index] = self.debug.as_ref().and_then(|debug| debug.line.clone());
                continue;
            }
            match self.loop_test(index, &block.terminator) {
                // The loop's test again at its end, so that a turn takes one
                // jump instead of two.
                Some(head) => {
                    for stmt in &head.stmts {
                        self.statement(stmt);
                    }
                    self.terminator(&head.terminator);
                }
                None => self.terminator(&block.terminator),
            }
            ended[index] = self.debug.as_ref().and_then(|debug| debug.line.clone());
        }
        self.e.fb.seal_all_blocks();
        let config = self.e.cg.isa().frontend_config();
        self.e.fb.finalize(config);
        self.sites
    }

    // ---- places and values ---------------------------------------------------
}

/// The locals a function assigns a string literal and nothing else, nor
/// lends to what could change them: a call, a `select`, a task or a
/// mutation; and the literal, when it is always the same.
fn literals(body: &Body) -> HashMap<Local, Option<String>> {
    let mut assigned: HashMap<Local, bool> = HashMap::new();
    let mut texts: HashMap<Local, Option<String>> = HashMap::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        crate::mir::statement_locals(statement, &mut used, &mut defined);
        let literal = match statement {
            Statement::Assign(place, rvalue, _) if place.path.is_empty() => match &**rvalue {
                Rvalue::Use(Operand::Const(Const::Str(text))) => Some(text),
                _ => None,
            },
            _ => None,
        };
        if let (Some(text), Statement::Assign(place, ..)) = (literal, statement) {
            let same = texts.entry(place.local).or_insert_with(|| Some(text.clone()));
            if same.as_ref() != Some(text) {
                *same = None;
            }
        }
        let literal = literal.is_some();
        let lends = matches!(statement, Statement::Assign(_, rvalue, _) if matches!(**rvalue,
            Rvalue::Call(..) | Rvalue::CallIndirect(..) | Rvalue::CallOsr(..) | Rvalue::Select(..) | Rvalue::TaskOp(..)
            | Rvalue::Spawn(..) | Rvalue::Mutate(..) | Rvalue::Pointer(..)));
        for local in defined {
            *assigned.entry(local).or_insert(literal) &= literal;
        }
        if lends {
            for local in used {
                assigned.insert(local, false);
            }
        }
    }
    let params: HashSet<Local> = body.params.iter().copied().chain(body.ret).collect();
    assigned
        .into_iter()
        .filter(|&(local, literal)| literal && !params.contains(&local))
        .map(|(local, _)| (local, texts.remove(&local).flatten()))
        .collect()
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

/// `NotFound` as `not found`.
fn words(name: &str) -> String {
    let mut text = String::new();
    for c in name.chars() {
        if c.is_uppercase() && !text.is_empty() {
            text.push(' ');
        }
        text.extend(c.to_lowercase());
    }
    text
}

/// Whether a statement lets a panic go on after a frame's cleanups.
fn resumes(statement: &Statement) -> bool {
    matches!(statement, Statement::Assign(_, rvalue, _)
        if matches!(**rvalue, Rvalue::Call(Callee::Std(StdFn::UnwindResume), _)))
}

/// Where the panics of the statement at `at` unwind to: the target of the
/// check that follows it, `asked = Unwinding` and a branch on it (15.9).
fn unwind_target(block: &crate::mir::BlockData, at: usize) -> Option<crate::mir::BlockId> {
    let Some(Statement::Assign(asked, rvalue, _)) = block.stmts.get(at + 1) else { return None };
    if !matches!(**rvalue, Rvalue::Unwinding) || !asked.path.is_empty() || at + 2 != block.stmts.len() {
        return None;
    }
    match &block.terminator {
        Terminator::Branch(Operand::Copy(condition), then, _)
            if condition.path.is_empty() && condition.local == asked.local =>
        {
            Some(*then)
        }
        _ => None,
    }
}

/// `TONDO_COVERAGE_STD`, internal: coverage counts the lines of the
/// standard library too, to measure what of it the toolchain's tests run
/// (docs/refactor.md, 0.1).
fn std_counted() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TONDO_COVERAGE_STD").is_some())
}
