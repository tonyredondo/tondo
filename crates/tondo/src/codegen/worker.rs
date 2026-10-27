//! The IR of the program's functions, built on every core. A `Worker`
//! builds one function as `Codegen` would, but what it would declare (a
//! string, glue, a function of the runtime, a debug build's sites) it only
//! names, in the order it asks for it; `replay` then declares it all in the
//! functions' order, which is the order `Codegen` would, and names the
//! function's references after what was declared. The object is the same,
//! byte for byte, whatever the cores did first.

use super::*;
use cranelift_codegen::ir::{ExtFuncData, ExternalName, GlobalValueData, InstructionData, Opcode, UserExternalName};

/// Where the ids a worker hands out start: above any the module gives.
const NAMED: u32 = 1 << 31;

/// What a function's IR asked to declare, or to record, in order.
enum Asked {
    String(String),
    StaticArray(usize, Vec<Const>),
    Identity(Ty),
    Glue(Glue, Ty),
    Runtime(Rt),
    FnValue(FnId),
    MapGlue(Ty),
    ClosureCode(FnId),
    Callback(FnId, Ty, bool),
    /// A function of C, whose import depends on those before: the function
    /// is built again in its turn.
    CFunction(ir::Signature),
    DebugFunction(String, Vec<DebugBinding>),
    FunctionSites,
    Attention,
    DebugSite(u32, u32),
    PointSite(u32, u32),
    PointLayout(u32, StateLayout),
    Counter(Counted),
}

/// What every worker reads of the program and of what is declared already.
pub(super) struct Shared<'s, 't> {
    types: &'t Types,
    sources: &'t SourceMap,
    display: &'t HashMap<Ty, FnId>,
    externs: &'t [crate::hir::Extern],
    meta: bool,
    isa: &'s dyn cranelift_codegen::isa::TargetIsa,
    loans: &'s [Vec<bool>],
    functions: &'s [FuncId],
    takes_osr: &'s [bool],
    /// Each function declared so far: its signature and whether it is in
    /// the object; and each data object: whether it is, and is thread-local.
    declared: HashMap<FuncId, (ir::Signature, bool)>,
    data: HashMap<DataId, (bool, bool)>,
    /// In a live build, the table of slots, each function's slot, and the
    /// number a patch's sites start from.
    live: Option<(DataId, &'s [u32], u32)>,
    debug: bool,
    coverage: Option<DataId>,
}

impl<'s, 't> Shared<'s, 't> {
    pub(super) fn new(cg: &'s Codegen<'t>) -> Shared<'s, 't> {
        let declarations = cg.module.declarations();
        let declared = declarations
            .get_functions()
            .map(|(id, decl)| (id, (decl.signature.clone(), decl.linkage.is_final())))
            .collect();
        let data =
            declarations.get_data_objects().map(|(id, decl)| (id, (decl.linkage.is_final(), decl.tls))).collect();
        Shared {
            types: cg.types,
            sources: cg.sources,
            display: cg.display,
            externs: cg.externs,
            meta: cg.meta,
            isa: cg.module.isa(),
            loans: &cg.loans,
            functions: &cg.functions,
            takes_osr: &cg.takes_osr,
            declared,
            data,
            live: cg.live.as_ref().map(|live| (live.slots, &live.slot_of[..], Gen::site_base(cg))),
            debug: cg.debug.is_some(),
            coverage: Gen::coverage(cg),
        }
    }
}

/// A function's IR, built on some core, and what it asked for.
pub(super) struct Built {
    pub(super) func: ir::Function,
    pub(super) sites: Vec<super::Site>,
    asked: Vec<Asked>,
    /// The references to what was asked: functions and data by the index
    /// of what was asked, and the instructions that hold a site's number.
    func_refs: Vec<(ir::FuncRef, usize)>,
    data_refs: Vec<(ir::GlobalValue, usize)>,
    site_constants: Vec<(ir::Inst, usize)>,
    /// The instructions that hold where a counter is, by its number among
    /// the function's.
    counter_offsets: Vec<(ir::Inst, usize)>,
    /// A call of C, whose import depends on the calls before it: the
    /// function is built again in its turn.
    pub(super) serial: bool,
}

struct Worker<'w, 's, 't> {
    shared: &'w Shared<'s, 't>,
    shapes: Shapes<'t>,
    current: usize,
    asked: Vec<Asked>,
    func_refs: Vec<(ir::FuncRef, usize)>,
    data_refs: Vec<(ir::GlobalValue, usize)>,
    site_constants: Vec<(ir::Inst, usize)>,
    counter_offsets: Vec<(ir::Inst, usize)>,
    point_layouts: HashMap<u32, StateLayout>,
    sites: usize,
    counters: usize,
    serial: bool,
}

/// Builds the IR of the function `index`.
pub(super) fn build(shared: &Shared, bodies: &[Body], index: usize) -> Built {
    thread_local! {
        static BUILDER: std::cell::RefCell<FunctionBuilderContext> = std::cell::RefCell::new(FunctionBuilderContext::new());
    }
    let mut worker = Worker {
        shared,
        shapes: Shapes::new(shared.types, shared.isa),
        current: index,
        asked: Vec::new(),
        func_refs: Vec::new(),
        data_refs: Vec::new(),
        site_constants: Vec::new(),
        counter_offsets: Vec::new(),
        point_layouts: HashMap::new(),
        sites: 0,
        counters: 0,
        serial: false,
    };
    let body = &bodies[index];
    let signature = worker.shapes.signature(body);
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, index as u32), signature);
    let sites =
        BUILDER.with(|builder| FnCodegen::new(&mut worker, body, bodies, &mut func, &mut builder.borrow_mut()).run());
    Built {
        func,
        sites,
        asked: worker.asked,
        func_refs: worker.func_refs,
        data_refs: worker.data_refs,
        site_constants: worker.site_constants,
        counter_offsets: worker.counter_offsets,
        serial: worker.serial,
    }
}

impl Worker<'_, '_, '_> {
    fn ask(&mut self, asked: Asked) -> u32 {
        self.asked.push(asked);
        NAMED + self.asked.len() as u32 - 1
    }
}

impl<'t> Gen<'t> for Worker<'_, '_, 't> {
    fn shapes(&mut self) -> &mut Shapes<'t> {
        &mut self.shapes
    }
    fn types(&self) -> &'t Types {
        self.shared.types
    }
    fn sources(&self) -> &'t SourceMap {
        self.shared.sources
    }
    fn display(&self) -> &'t HashMap<Ty, FnId> {
        self.shared.display
    }
    fn externs(&self) -> &'t [crate::hir::Extern] {
        self.shared.externs
    }
    fn meta(&self) -> bool {
        self.shared.meta
    }
    fn isa(&self) -> &dyn cranelift_codegen::isa::TargetIsa {
        self.shared.isa
    }
    fn loans(&self, id: FnId) -> Vec<bool> {
        self.shared.loans[id.0 as usize].clone()
    }
    fn function(&self, id: FnId) -> FuncId {
        self.shared.functions[id.0 as usize]
    }
    fn takes_osr(&self, id: FnId) -> bool {
        self.shared.takes_osr[id.0 as usize]
    }
    fn function_signature(&self, id: FuncId) -> ir::Signature {
        self.shared.declared[&id].0.clone()
    }
    fn live(&self) -> bool {
        self.shared.live.is_some()
    }
    fn slot_of(&self, id: FnId) -> Option<(DataId, u32)> {
        self.shared.live.map(|(slots, slot_of, _)| (slots, slot_of[id.0 as usize]))
    }
    fn site_base(&self) -> u32 {
        self.shared.live.map_or(0, |(_, _, base)| base)
    }
    fn debug(&self) -> bool {
        self.shared.debug
    }
    fn coverage(&self) -> Option<DataId> {
        self.shared.coverage
    }
    fn counter(&mut self, counted: Counted) -> usize {
        self.ask(Asked::Counter(counted));
        self.counters += 1;
        self.counters - 1
    }
    fn counter_offset(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        let value = fb.ins().iconst(types::I64, index as i64 * 8);
        let inst = fb.func.dfg.value_def(value).unwrap_inst();
        self.counter_offsets.push((inst, index));
        value
    }
    fn string(&mut self, text: &str) -> DataId {
        DataId::from_u32(self.ask(Asked::String(text.to_string())))
    }
    fn static_array(&mut self, size: usize, elements: Vec<Const>) -> DataId {
        DataId::from_u32(self.ask(Asked::StaticArray(size, elements)))
    }
    fn identity(&mut self, ty: Ty) -> DataId {
        DataId::from_u32(self.ask(Asked::Identity(ty)))
    }
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        FuncId::from_u32(self.ask(Asked::Glue(kind, ty)))
    }
    fn runtime(&mut self, rt: Rt) -> FuncId {
        FuncId::from_u32(self.ask(Asked::Runtime(rt)))
    }
    fn fn_value(&mut self, id: FnId, _body: &Body) -> DataId {
        DataId::from_u32(self.ask(Asked::FnValue(id)))
    }
    fn map_glue(&mut self, ty: Ty) -> DataId {
        DataId::from_u32(self.ask(Asked::MapGlue(ty)))
    }
    fn attention(&mut self) -> DataId {
        DataId::from_u32(self.ask(Asked::Attention))
    }
    fn closure_code(&mut self, body: FnId) -> FuncId {
        // Without a live build, a closure keeps its body itself.
        match self.shared.live {
            None => self.function(body),
            Some(_) => FuncId::from_u32(self.ask(Asked::ClosureCode(body))),
        }
    }
    fn c_function(&mut self, _symbol: &str, signature: &ir::Signature) -> (FuncId, bool) {
        self.serial = true;
        (FuncId::from_u32(self.ask(Asked::CFunction(signature.clone()))), true)
    }
    fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId {
        FuncId::from_u32(self.ask(Asked::Callback(target, ty, closure)))
    }
    fn declare_func_in_func(&mut self, id: FuncId, func: &mut ir::Function) -> ir::FuncRef {
        let index = id.as_u32().checked_sub(NAMED).map(|n| n as usize);
        let (signature, colocated) = match index.map(|n| self.asked.get(n)) {
            None => self.shared.declared[&id].clone(),
            Some(Some(Asked::Glue(kind, _))) => (self.shapes.glue_signature(*kind), false),
            Some(Some(Asked::Runtime(rt))) => (runtime_signature(*rt, self.shapes.make_signature()).1, false),
            Some(Some(&Asked::ClosureCode(body))) => {
                let mut signature = self.function_signature(self.function(body));
                if self.takes_osr(body) {
                    signature.params.pop();
                }
                (signature, false)
            }
            Some(Some(&Asked::Callback(_, ty, _))) => {
                let TyKind::CFn(params, result, _) = self.shared.types.kind(ty).clone() else {
                    unreachable!("a C function type")
                };
                (self.shapes.c_signature(&params, result), false)
            }
            Some(Some(Asked::CFunction(signature))) => (signature.clone(), false),
            Some(_) => unreachable!("not a function"),
        };
        let signature = func.import_signature(signature);
        let name = func.declare_imported_user_function(UserExternalName { namespace: 0, index: id.as_u32() });
        let reference = func.import_function(ExtFuncData {
            name: ExternalName::user(name),
            signature,
            colocated,
            patchable: false,
        });
        if let Some(n) = index {
            self.func_refs.push((reference, n));
        }
        reference
    }
    fn declare_data_in_func(&mut self, id: DataId, func: &mut ir::Function) -> ir::GlobalValue {
        let index = id.as_u32().checked_sub(NAMED).map(|n| n as usize);
        let (colocated, tls) = match index {
            None => self.shared.data[&id],
            Some(_) => (false, false),
        };
        let name = func.declare_imported_user_function(UserExternalName { namespace: 1, index: id.as_u32() });
        let value = func.create_global_value(GlobalValueData::Symbol {
            name: ExternalName::user(name),
            offset: ir::immediates::Imm64::new(0),
            colocated,
            tls,
        });
        if let Some(n) = index {
            self.data_refs.push((value, n));
        }
        value
    }
    fn debug_function(&mut self, name: String, described: Vec<DebugBinding>) -> u32 {
        self.ask(Asked::DebugFunction(name, described));
        // Each function of the program has one, in order.
        self.current as u32
    }
    fn function_sites(&mut self) -> Option<(DataId, usize)> {
        self.shared.live?;
        Some((DataId::from_u32(self.ask(Asked::FunctionSites)), 0))
    }
    fn debug_site(&mut self, start: u32, function: u32) -> usize {
        self.ask(Asked::DebugSite(start, function));
        self.sites += 1;
        self.sites - 1
    }
    fn site_constant(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        let value = fb.ins().iconst(types::I64, index as i64);
        let inst = fb.func.dfg.value_def(value).unwrap_inst();
        self.site_constants.push((inst, index));
        value
    }
    fn point_site(&mut self, point: u32, number: u32) {
        self.ask(Asked::PointSite(point, number));
    }
    fn point_layout(&self, point: u32) -> Option<StateLayout> {
        self.point_layouts.get(&point).cloned()
    }
    fn keep_point_layout(&mut self, point: u32, layout: StateLayout) {
        if self.shared.live.is_some() {
            self.point_layouts.insert(point, layout.clone());
            self.ask(Asked::PointLayout(point, layout));
        }
    }
}

/// Declares what a function built on some core asked for, as `Codegen`
/// would have while building it, and names its references after that.
pub(super) fn replay(cg: &mut Codegen, bodies: &[Body], built: &mut Built) {
    let mut ids = vec![0u32; built.asked.len()];
    let mut first_site: Option<usize> = None;
    let mut first_counter: Option<usize> = None;
    for (n, asked) in std::mem::take(&mut built.asked).into_iter().enumerate() {
        let real = |id: FuncId, ids: &[u32]| match id.as_u32().checked_sub(NAMED) {
            Some(n) => FuncId::from_u32(ids[n as usize]),
            None => id,
        };
        ids[n] = match asked {
            Asked::String(text) => cg.string(&text).as_u32(),
            Asked::StaticArray(size, elements) => cg.static_array(size, &elements).as_u32(),
            Asked::Identity(ty) => cg.identity(ty).as_u32(),
            Asked::Glue(kind, ty) => Codegen::glue(cg, kind, ty).as_u32(),
            Asked::Runtime(rt) => Codegen::runtime(cg, rt).as_u32(),
            Asked::FnValue(id) => Codegen::fn_value(cg, id, &bodies[id.0 as usize]).as_u32(),
            Asked::MapGlue(ty) => Codegen::map_glue(cg, ty).as_u32(),
            Asked::ClosureCode(body) => Codegen::closure_code(cg, body).as_u32(),
            Asked::Callback(target, ty, closure) => Codegen::callback(cg, target, ty, closure).as_u32(),
            Asked::DebugFunction(name, described) => {
                let described = described.into_iter().map(|(name, ty, glue)| (name, ty, real(glue, &ids))).collect();
                let function = Gen::debug_function(cg, name, described);
                debug_assert_eq!(function as usize, cg.current);
                0
            }
            Asked::FunctionSites => Gen::function_sites(cg).expect("a live build").0.as_u32(),
            Asked::Attention => Gen::attention(cg).as_u32(),
            Asked::DebugSite(start, function) => {
                let index = Gen::debug_site(cg, start, function);
                first_site.get_or_insert(index);
                0
            }
            Asked::PointSite(point, number) => {
                Gen::point_site(cg, point, number + first_site.unwrap_or(0) as u32);
                0
            }
            Asked::CFunction(_) => unreachable!("a function that calls C is built again in its turn"),
            Asked::PointLayout(point, layout) => {
                Gen::keep_point_layout(cg, point, layout);
                0
            }
            Asked::Counter(counted) => {
                let index = Gen::counter(cg, counted);
                first_counter.get_or_insert(index);
                0
            }
        };
    }
    let func = &mut built.func;
    let declarations = cg.module.declarations();
    for &(reference, n) in &built.func_refs {
        let id = FuncId::from_u32(ids[n]);
        let ExternalName::User(name) = func.dfg.ext_funcs[reference].name else { unreachable!("a named function") };
        func.params.reset_user_func_name(name, UserExternalName { namespace: 0, index: id.as_u32() });
        func.dfg.ext_funcs[reference].colocated = declarations.get_function_decl(id).linkage.is_final();
    }
    for &(value, n) in &built.data_refs {
        let id = DataId::from_u32(ids[n]);
        let decl = declarations.get_data_decl(id);
        let GlobalValueData::Symbol { name: ExternalName::User(name), colocated, tls, .. } =
            &mut func.global_values[value]
        else {
            unreachable!("a named data object")
        };
        *colocated = decl.linkage.is_final();
        *tls = decl.tls;
        let name = *name;
        func.params.reset_user_func_name(name, UserExternalName { namespace: 1, index: id.as_u32() });
    }
    let first = first_site.unwrap_or(0) as i64;
    for &(inst, index) in &built.site_constants {
        func.dfg.insts[inst] =
            InstructionData::UnaryImm { opcode: Opcode::Iconst, imm: ir::immediates::Imm64::new(first + index as i64) };
    }
    let first = first_counter.unwrap_or(0) as i64;
    for &(inst, index) in &built.counter_offsets {
        let offset = (first + index as i64) * 8;
        func.dfg.insts[inst] =
            InstructionData::UnaryImm { opcode: Opcode::Iconst, imm: ir::immediates::Imm64::new(offset) };
    }
}
