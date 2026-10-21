//! MIR to native object code with Cranelift.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{Callee, Program, StdFn};
use crate::mir::{Body, Const, Operand, Piece, Rvalue, Statement, Terminator};
use crate::source::{SourceMap, Span};
use crate::types::{IntTy, Ty, TyKind, Types};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, TrapCode, UserFuncName, types};
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

/// Machine type of a value, or `None` for zero-sized types.
fn clif_type(types: &Types, ty: Ty) -> Option<ir::Type> {
    match types.kind(ty) {
        TyKind::Unit | TyKind::Never | TyKind::Error => None,
        TyKind::Bool => Some(types::I8),
        TyKind::Int(int) => Some(int_type(*int)),
        TyKind::String | TyKind::Result(..) | TyKind::Std(_) => Some(POINTER),
    }
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
}

impl Codegen<'_> {
    fn signature(&self, body: &Body) -> ir::Signature {
        let mut signature = self.module.make_signature();
        for &param in &body.params {
            if let Some(ty) = clif_type(self.types, body.locals[param.0 as usize]) {
                signature.params.push(AbiParam::new(ty));
            }
        }
        if let Some(ty) = clif_type(self.types, body.result) {
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

struct FnCodegen<'a, 'b> {
    cg: &'a mut Codegen<'b>,
    body: &'a Body,
    fb: FunctionBuilder<'a>,
    blocks: Vec<ir::Block>,
    vars: Vec<Option<Variable>>,
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
        let vars = body.locals.iter().map(|&ty| clif_type(cg.types, ty).map(|t| fb.declare_var(t))).collect();
        FnCodegen { cg, body, fb, blocks, vars }
    }

    fn run(mut self) {
        let entry = self.blocks[0];
        self.fb.append_block_params_for_function_params(entry);
        self.fb.switch_to_block(entry);
        let params: Vec<ir::Value> = self.fb.block_params(entry).to_vec();
        let mut incoming = params.into_iter();
        for &param in &self.body.params {
            if let Some(var) = self.vars[param.0 as usize] {
                self.fb.def_var(var, incoming.next().unwrap());
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

    fn ty_of(&self, operand: &Operand) -> Ty {
        match operand {
            Operand::Copy(local) => self.body.locals[local.0 as usize],
            Operand::Const(Const::Int(_, ty)) => *ty,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
            Operand::Const(Const::Unit) => Ty::UNIT,
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

    fn operand(&mut self, operand: &Operand) -> Option<ir::Value> {
        match operand {
            Operand::Copy(local) => self.vars[local.0 as usize].map(|var| self.fb.use_var(var)),
            Operand::Const(Const::Int(value, ty)) => {
                let clif = clif_type(self.cg.types, *ty).unwrap();
                // Narrow immediates are zero-extended bit patterns.
                let bits = clif.bits();
                let pattern = if bits == 64 { *value as i64 } else { (*value as i64) & ((1i64 << bits) - 1) };
                Some(self.fb.ins().iconst(clif, pattern))
            }
            Operand::Const(Const::Bool(value)) => Some(self.fb.ins().iconst(types::I8, *value as i64)),
            Operand::Const(Const::Str(text)) => Some(self.string_ptr(text)),
            Operand::Const(Const::Unit) => None,
        }
    }

    fn location(&mut self, span: Span) -> ir::Value {
        let location = self.cg.sources.location(span);
        self.string_ptr(&location)
    }

    fn statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(local, rvalue, span) => {
                let value = self.rvalue(rvalue, *span);
                if let (Some(var), Some(value)) = (self.vars[local.0 as usize], value) {
                    self.fb.def_var(var, value);
                }
            }
            Statement::Retain(local) => {
                let value = self.fb.use_var(self.vars[local.0 as usize].unwrap());
                self.call_rt(Rt::StrRetain, &[value]);
            }
            Statement::Release(local) => {
                let value = self.fb.use_var(self.vars[local.0 as usize].unwrap());
                self.call_rt(Rt::StrRelease, &[value]);
            }
        }
    }

    fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                let condition = self.operand(condition).unwrap();
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                self.fb.ins().brif(condition, then, &[], otherwise, &[]);
            }
            Terminator::Return(value) => {
                let values: Vec<ir::Value> = self.operand(value).into_iter().collect();
                self.fb.ins().return_(&values);
            }
            Terminator::Panic { code, message, span } => {
                let message = match message {
                    Some(message) => self.operand(message).unwrap(),
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

    fn rvalue(&mut self, rvalue: &Rvalue, span: Span) -> Option<ir::Value> {
        match rvalue {
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Unary(op, operand) => {
                let ty = self.ty_of(operand);
                let value = self.operand(operand).unwrap();
                Some(match op {
                    UnaryOp::Not => self.fb.ins().bxor_imm_u(value, 1),
                    UnaryOp::BitNot => self.fb.ins().bnot(value),
                    UnaryOp::Neg => {
                        let clif = clif_type(self.cg.types, ty).unwrap();
                        let zero = self.fb.ins().iconst(clif, 0);
                        let (result, overflow) = self.fb.ins().ssub_overflow(zero, value);
                        self.panic_if(overflow, 5, span);
                        result
                    }
                })
            }
            Rvalue::Increment(operand) => {
                let value = self.operand(operand).unwrap();
                Some(self.fb.ins().iadd_imm_s(value, 1))
            }
            Rvalue::Binary(op, left, right) => self.binary(*op, left, right, span),
            Rvalue::Call(callee, args) => {
                let args: Vec<ir::Value> = args.iter().filter_map(|a| self.operand(a)).collect();
                match callee {
                    Callee::Fn(id) => {
                        let func = self.cg.module.declare_func_in_func(self.cg.functions[id.0 as usize], self.fb.func);
                        let call = self.fb.ins().call(func, &args);
                        self.fb.inst_results(call).first().copied()
                    }
                    Callee::Std(StdFn::ConsolePrint) => self.call_rt(Rt::ConsolePrint, &args),
                    Callee::Std(StdFn::ConsolePrintln) => self.call_rt(Rt::ConsolePrintln, &args),
                }
            }
            Rvalue::Concat(pieces) => {
                let builder = self.call_rt(Rt::SbNew, &[]).unwrap();
                for piece in pieces {
                    match piece {
                        Piece::Text(text) => {
                            let text = self.string_ptr(text);
                            self.call_rt(Rt::SbStr, &[builder, text]);
                        }
                        Piece::Value(operand, ty) => self.append(builder, operand, *ty),
                    }
                }
                self.call_rt(Rt::SbFinish, &[builder])
            }
        }
    }

    fn append(&mut self, builder: ir::Value, operand: &Operand, ty: Ty) {
        let value = self.operand(operand).unwrap();
        match self.cg.types.kind(ty) {
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

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> Option<ir::Value> {
        let ty = self.ty_of(left);
        let right_ty = self.ty_of(right);
        let types = self.cg.types;
        let (Some(a), Some(b)) = (self.operand(left), self.operand(right)) else {
            // Unit operands: only equality is defined.
            return Some(self.fb.ins().iconst(types::I8, (op == BinOp::Eq) as i64));
        };
        if ty == Ty::STRING {
            return Some(self.string_compare(op, a, b));
        }
        let signed = types.as_int(ty).is_some_and(IntTy::signed);
        let ins = self.fb.ins();
        Some(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
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
                    return Some(if op == BinOp::Div { self.fb.ins().udiv(a, b) } else { self.fb.ins().urem(a, b) });
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let int = types.as_int(ty).unwrap();
                let clif = int_type(int);
                let is_min = self.fb.ins().icmp_imm_s(IntCC::Equal, a, int.min() as i64);
                let is_minus_one = self.fb.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let both = self.fb.ins().band(is_min, is_minus_one);
                if op == BinOp::Div {
                    self.panic_if(both, 5, span);
                    self.fb.ins().sdiv(a, b)
                } else {
                    let one = self.fb.ins().iconst(clif, 1);
                    let divisor = self.fb.ins().select(both, one, b);
                    self.fb.ins().srem(a, divisor)
                }
            }
            BinOp::BitAnd => ins.band(a, b),
            BinOp::BitOr => ins.bor(a, b),
            BinOp::BitXor => ins.bxor(a, b),
            BinOp::Shl | BinOp::Shr => {
                let width = types.as_int(ty).unwrap().bits() as i64;
                let count_signed = types.as_int(right_ty).is_some_and(IntTy::signed);
                if count_signed {
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
                ins.icmp(int_condition(op, signed), a, b)
            }
            BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
                unreachable!("lowered before codegen")
            }
        })
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
