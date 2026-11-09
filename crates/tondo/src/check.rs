//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

mod annotations;
mod declarations;
mod foreign;
mod functions;
mod indexing;
mod natives;
mod owners;
mod program;
mod routes;
mod std_checks;
mod traits;

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn, TraitId, constant_parts};
use crate::parser::CONTEXTUAL_CONSTRUCTORS;
use crate::source::{SourceMap, Span};
use crate::types::{
    AdtId, AdtKind, ClosureInfo, CursorKind, ERR, Field, Handle, IntTy, NONE, OK, Payload, SOME, Shown, TemplateId, Ty,
    TyKind, Types, Variant,
};
use std::collections::{HashMap, HashSet};

/// Checks a linked program; `meta` is the profile of a provider or
/// generator, which has no host capabilities (27.7).
pub fn check<'a>(
    module: &'a ast::Module,
    sources: &'a SourceMap,
    diags: &'a mut Diagnostics,
    meta: bool,
) -> Option<hir::Program> {
    check_with(module, sources, diags, meta, None).0
}

/// Checks a program and records, for an editor, every declaration and
/// every name that refers to one.
pub fn check_indexed<'a>(
    module: &'a ast::Module,
    sources: &'a SourceMap,
    diags: &'a mut Diagnostics,
) -> (Option<hir::Program>, crate::index::Index) {
    let (program, index) = check_with(module, sources, diags, false, Some(crate::index::Index::default()));
    (program, index.unwrap_or_default())
}

fn check_with<'a>(
    module: &'a ast::Module,
    sources: &'a SourceMap,
    diags: &'a mut Diagnostics,
    meta: bool,
    index: Option<crate::index::Index>,
) -> (Option<hir::Program>, Option<crate::index::Index>) {
    // What uses a deprecated declaration is known by the index, which a
    // program that has none does without.
    let deprecated = deprecated_names(module);
    let asked = index.is_some();
    let index = index.or_else(|| (!deprecated.is_empty()).then(crate::index::Index::default));
    let mut checker = Checker {
        index,
        index_exprs: Vec::new(),
        cell_uses: Vec::new(),
        meta,
        types: Types::default(),
        diags,
        sources,
        names: HashMap::new(),
        type_names: HashMap::new(),
        misnamed: HashSet::new(),
        aliases: Vec::new(),
        signatures: Vec::new(),
        methods: HashMap::new(),
        type_params: Vec::new(),
        self_type: None,
        implementing: None,
        traits: Vec::new(),
        trait_names: HashMap::new(),
        try_body: false,
        dropped_calls: Vec::new(),
        impls: Vec::new(),
        impl_heads: HashMap::new(),
        heads_indexed: 0,
        errors_counted: std::cell::Cell::new((0, 0)),
        bounds: HashMap::new(),
        call_bounds: HashMap::new(),
        cursor_templates: HashMap::new(),
        closure_base: 0,
        closure_functions: Vec::new(),
        test_nodes: Vec::new(),
        fn_scopes: Vec::new(),
        duration: None,
        additive: None,
        number: None,
        integer: None,
        ordered: None,
        instant: None,
        datetime: None,
        time_error: None,
        compare_exchange: None,
        fs: None,
        module: 0,
        owners: HashMap::new(),
        method_visibility: HashMap::new(),
        public_fns: HashSet::new(),
        prelude: None,
        contexts: Vec::new(),
        initial_reads: None,
        annotation_decls: HashMap::new(),
        defaulted: Vec::new(),
        field_defaults: HashMap::new(),
        annotation_defaults: HashMap::new(),
        annotation_values: Vec::new(),
        context_of: HashMap::new(),
        module_paths: Vec::new(),
        datetime_module: None,
        pacing_module: None,
        unrepeatable: HashMap::new(),
        checks_models: HashSet::new(),
        calling: None,
        calls: HashMap::new(),
        test_calls: HashMap::new(),
        spawning: HashSet::new(),
        test_spawns: HashSet::new(),
        model_steps: Vec::new(),
        model_names: Vec::new(),
        interpolating: false,
        script_bindings: Vec::new(),
        scripting: false,
        naming: String::new(),
        declared_at: HashMap::new(),
        leaving: None,
        comparing: None,
        panic_datum: None,
        closure_name: None,
        templating: false,
        measuring: None,
        wires: HashMap::new(),
        intrinsic_owners: HashMap::new(),
        trait_modules: HashMap::new(),
        conversion_error: Ty::ERROR,
        collection_error: Ty::ERROR,
        text_error: Ty::ERROR,
        io_error: Ty::ERROR,
        env_error: None,
        built: None,
        elsewhere: Vec::new(),
        process: None,
        externs: Vec::new(),
        c_string: Ty::ERROR,
        void: Ty::ERROR,
        c_arguments: false,
        extern_names: Vec::new(),
        exports: Vec::new(),
        export_symbols: HashMap::new(),
        c_functions: Vec::new(),
        c_decls: HashMap::new(),
        consts: Vec::new(),
        const_uses: HashSet::new(),
    };
    checker.declare_prelude();
    let program = checker.program(module);
    checker.index_finish();
    checker.lost_updates();
    if let Some(index) = checker.index.as_ref().filter(|_| !meta && !deprecated.is_empty()) {
        let mut warned = HashSet::new();
        for &(span, def) in &index.uses {
            let declared = &index.defs[def];
            let inside = declared.decl_span.start <= span.start && span.end <= declared.decl_span.end;
            if let Some(text) = deprecated.get(&declared.name_span).filter(|_| !inside && warned.insert(span)) {
                checker.diags.warning("W1015", span, format!("`{}` is deprecated: {text}", declared.name));
            }
        }
    }
    if let Some(index) = &mut checker.index {
        index.deprecated = deprecated.into_keys().collect();
    }
    (program, checker.index.filter(|_| asked))
}

/// Prelude type names (26.3); they cannot be redeclared unqualified.
pub(crate) const PRELUDE_TYPES: [&str; 49] = [
    "NumericConversionError",
    "ByteOrder",
    "Bool",
    "Int",
    "Float",
    "Byte",
    "Char",
    "String",
    "Unit",
    "Never",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "Int128",
    "UInt",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt128",
    "Float32",
    "Float64",
    "Option",
    "Result",
    "Array",
    "Deque",
    "Map",
    "Set",
    "Range",
    "Iterator",
    "Ref",
    "WeakRef",
    "Pointer",
    "Join",
    "Group",
    "Sender",
    "Receiver",
    "Command",
    "Pipeline",
    "Copy",
    "Equatable",
    "Key",
    "Send",
    "Share",
    "Display",
    "Panic",
    "Failure",
    "Contexts",
];

/// The types of the prelude a program can name its own: what a domain calls
/// `Key` or `Command` wins in its module. The rest is the language's vocabulary.
pub(crate) const OWN_NAMES: [&str; 10] =
    ["Range", "Group", "Command", "Pipeline", "Key", "Join", "Sender", "Receiver", "ByteOrder", "Contexts"];

/// Whether a type of a program cannot take this name.
pub(crate) fn reserved_type(name: &str) -> bool {
    PRELUDE_TYPES.contains(&name) && !OWN_NAMES.contains(&name)
}

enum TypeName {
    Nominal(Ty),
    Alias(usize),
    /// A generic record or enum.
    Template(TemplateId),
}

struct AliasEntry {
    target: ast::TypeExpr,
    span: Span,
    resolved: Option<Ty>,
    resolving: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum StdModule {
    Console,
    Collections,
    Text,
    Time,
    Sync,
    Fs,
    Path,
    Io,
    Env,
    Process,
    /// `std.internal.json`: the runtime's tokenizer and emitter, for
    /// `std.json`.
    InternalJson,
    /// `std.internal.memory`: raw memory, for tests of `unsafe` code.
    InternalMemory,
    /// `std.testing`: what a test tells the runner (testing 11.2).
    Testing,
    /// `std.internal.regex`: the runtime's engine, for `std.regex`.
    InternalRegex,
    /// `std.internal.reflect`: the box of a `reflect.Value`.
    InternalReflect,
    /// `std.internal.text` and `std.internal.math`: the runtime's text and
    /// scalar math, for the prelude and `std.math`.
    InternalText,
    InternalMath,
    /// `std.internal.meta`: the request and response of a provider.
    InternalMeta,
    /// `std.internal.codec`: encodings, digests, entropy and the program's
    /// counters, for `std.encoding`, `std.hash`, `std.random`, `std.uuid`
    /// and `std.diagnostics`.
    InternalCodec,
    /// `std.internal.http`: the runtime's HTTP client, for `std.http`.
    InternalHttp,
    /// `std.internal.channel`: sends that report a closed channel instead
    /// of panicking, for `std.executor`'s actors.
    InternalChannel,
    InternalToml,
    InternalYaml,
    InternalMsgpack,
    InternalProtobuf,
    InternalGrpc,
    InternalCbor,
    InternalNet,
    /// `std.internal.fuzz`: the runtime's side of `testing.fuzz`.
    InternalFuzz,
    /// `std.internal.model`: whether `model.mark` records.
    InternalModel,
    /// `std.internal.panic`: what a panic carries beyond its message.
    InternalPanic,
    InternalTime,
    InternalContext,
    /// `std.internal.fs`: the runtime's side of `std.path` and `std.fs`
    /// written in Tondo.
    InternalFs,
    /// `std.internal.env`: the host's side of what `std.env` writes in
    /// Tondo.
    InternalEnv,
    /// `std.internal.process`: the host's side of what `std.process`
    /// writes in Tondo.
    InternalProcess,
    /// `std.internal.sql`: SQLite, for `std.sql`.
    InternalSql,
    /// `std.internal.image`: decoding, encoding and pixels, for `std.image`.
    InternalImage,
}

/// The types `std.process` declares when imported.
#[derive(Copy, Clone)]
struct ProcessTypes {
    input: Ty,
    output: Ty,
    status: Ty,
    result: Ty,
    error: Ty,
    process: Ty,
    signal: Ty,
}

/// The types `std.fs` declares when imported.
#[derive(Copy, Clone)]
struct FsTypes {
    error: Ty,
    kind: Ty,
    mode: Ty,
    file: Ty,
}

#[derive(Copy, Clone)]
enum ModuleName {
    Fn(FnId),
    /// A constant, by its index in `consts`.
    Const(usize),
    /// An `extern fn`, by its index in `externs`.
    Extern(u32),
    Module(StdModule),
    /// The constructor `Name(value)` of a newtype (8.8).
    Newtype(Ty),
}

#[derive(Copy, Clone)]
struct PatternContext {
    /// In a `match` arm, where a bare `ok`/`err`/`some` binding is rejected.
    in_match: bool,
    /// Bindings introduced by `var`.
    mutable: bool,
    /// The loan bindings the pattern may introduce (14.3).
    loans: Loans,
}

#[derive(Copy, Clone, PartialEq)]
enum Loans {
    None,
    /// `ref`, in a `match` arm.
    Shared,
    /// `ref`, `mut` and `var`, in the header of a `for` over a collection.
    Any,
}

#[derive(Copy, Clone)]
enum FieldRef<'a> {
    Name(&'a str),
    Slot(u32),
}

/// The size of a term of a trait query relative to another (12.6).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Size {
    Less,
    Equal,
    Unknown,
}

/// Rows are the terms of a target query, columns those of the source.
type Matrix = Vec<Vec<Size>>;

/// The relation along `first` then `second`: `second ∘ first`, keeping the
/// strongest relation through any intermediate term.
fn compose(second: &Matrix, first: &Matrix) -> Matrix {
    second
        .iter()
        .map(|row| {
            (0..first.first().map_or(0, Vec::len))
                .map(|column| {
                    let mut best = Size::Unknown;
                    for (middle, &step) in row.iter().enumerate() {
                        let through = match (step, first[middle][column]) {
                            (Size::Less, Size::Less | Size::Equal) | (Size::Equal, Size::Less) => Size::Less,
                            (Size::Equal, Size::Equal) => Size::Equal,
                            _ => Size::Unknown,
                        };
                        if through == Size::Less || (through == Size::Equal && best == Size::Unknown) {
                            best = through;
                        }
                    }
                    best
                })
                .collect()
        })
        .collect()
}

/// Type arguments being inferred for some type parameters (12.1): from
/// the expected type, then from the values given for them.
struct Inference {
    vars: Vec<Ty>,
    map: HashMap<Ty, Ty>,
}

impl Inference {
    fn new(vars: Vec<Ty>) -> Inference {
        Inference { vars, map: HashMap::new() }
    }
}

/// What an annotation of the program annotates (27.13).
#[derive(Copy, Clone, PartialEq)]
enum Annotated {
    Type(Ty),
    /// A record's field, by its index.
    Field(Ty, u32),
    Variant(Ty, u32),
    /// A field of a record variant: the variant's index, then the field's.
    VariantField(Ty, u32, u32),
    Function(FnId),
    /// A function's parameter, by its index.
    Parameter(FnId, u32),
}

/// What a call invokes: the callee, its signature, explicit type arguments
/// (empty when inferred), arguments already known and the type the call is
/// expected to have.
struct CallTarget {
    callee: Callee,
    signature: Signature,
    explicit: Vec<Ty>,
    prebound: HashMap<Ty, Ty>,
    expected: Option<Ty>,
}

impl CallTarget {
    fn new(callee: Callee, signature: Signature, expected: Option<Ty>) -> CallTarget {
        CallTarget { callee, signature, explicit: Vec::new(), prebound: HashMap::new(), expected }
    }
}

/// The key a field has in every format: `@name`, its own, or none with
/// `@ignore`.
fn wire(field: &ast::FieldDecl) -> Option<String> {
    let mut wire = Some(field.name.name.clone());
    for annotation in &field.annotations {
        match (annotation.name.name.as_str(), &annotation.argument) {
            ("ignore", _) => return None,
            ("name", Some(ast::AnnotationArg::Text(text))) => wire = Some(text.clone()),
            _ => {}
        }
    }
    wire
}

/// Whether a declaration is of a type marked `@secret`.
fn secret_type(item: &ast::Item) -> bool {
    matches!(item, ast::Item::Type(decl) if decl.annotations.iter().any(|a| a.name.name == "secret"))
}

const DEPRECATED_TEXT: &str = "`@deprecated` says what to use instead: write `@deprecated(\"use `other`\")`";

/// What `@deprecated("...")` says, if an annotation is that.
fn deprecation(annotation: &ast::Annotation) -> Option<&str> {
    match (annotation.name.name.as_str(), &annotation.argument) {
        ("deprecated", Some(ast::AnnotationArg::Text(text))) => Some(text),
        _ => None,
    }
}

/// The declarations marked `@deprecated`, by where their names are.
fn deprecated_names(module: &ast::Module) -> HashMap<Span, String> {
    let mut found = HashMap::new();
    for item in &module.items {
        let (name, annotations) = match item {
            ast::Item::Fn(f) => (&f.name, &f.annotations),
            ast::Item::Type(t) => (&t.name, &t.annotations),
            _ => continue,
        };
        found.extend(annotations.iter().filter_map(deprecation).map(|text| (name.span, text.to_string())));
    }
    found
}

/// What a function does to a cell of `std.sync`, which `lost_updates` reads:
/// the cell as `Checker::cell` names it and the call.
struct CellUse {
    cell: String,
    span: Span,
    read: bool,
    /// Of a read, where the binding it gave is named; of a write, where its
    /// arguments are.
    within: Span,
    /// The statement of a read that gives a binding; the name of a write's
    /// method; and the parameter of the closure an `update` takes.
    statement: Span,
    method: String,
    parameter: Option<String>,
}

/// That the language's annotation `@name` does not go on `what` (27.13).
fn misplaced_annotation(name: &str, what: &str) -> String {
    format!(
        "the language's annotation `@{name}` does not go on {what}; an annotation of the program has the capital of its type, as `@module.Name(...)`"
    )
}

/// The prelude trait used by interpolation (19.6).
const DISPLAY: TraitId = TraitId(0);
/// The prelude trait `for` consumes (10.17).
const ITERATOR: TraitId = TraitId(1);
/// User traits are numbered after the prelude's.
const PRELUDE_TRAITS: u32 = 2;

/// A trait declaration (12.3). Its methods' signatures are generic over
/// `Self`, the trait's parameters and their own.
struct TraitDef {
    name: String,
    self_param: Ty,
    params: Vec<Ty>,
    methods: Vec<TraitMethod>,
    /// The traits it requires (12.3), with their arguments.
    requires: Vec<(TraitId, Vec<Ty>)>,
}

struct TraitMethod {
    name: String,
    signature: Signature,
    default: Option<FnId>,
    /// Where the trait declares it; none for the prelude's own.
    span: Span,
}

/// An implementation (12.4); its methods are generic over the binders.
struct ImplDef {
    intrinsic: bool,
    span: Span,
    trait_id: TraitId,
    trait_args: Vec<Ty>,
    target: Ty,
    generics: Vec<Ty>,
    methods: Vec<Option<FnId>>,
}

/// The type names a function sees besides the module's: generic parameters
/// in scope and what `Self` means.
#[derive(Clone, Default)]
struct FnScope {
    /// The module of the function (7.5).
    module: u32,
    types: Vec<(String, Ty)>,
    self_type: Option<Ty>,
    /// Inside an `impl`: the trait it implements, whose methods are
    /// available by dot-call on values of type `Self` (11.5).
    implementing: Option<(TraitId, Vec<Ty>)>,
}

#[derive(Clone)]
struct Signature {
    params: Vec<Ty>,
    /// Parameter names, which named arguments use (11.3).
    names: Vec<String>,
    /// The mode of the receiver when the first parameter is `self`.
    receiver: Option<ast::Mode>,
    /// How each parameter is passed: by value or as a `ref`, `mut` or `var`
    /// loan (16.3); a plain `self` receiver is `ref`.
    modes: Vec<ast::Mode>,
    /// Whether the last parameter is variadic; its type is `Array[T]`.
    variadic: bool,
    result: Ty,
    /// Type parameters: a generic owner's binders, then the function's own.
    generics: Vec<Ty>,
    /// The name diagnostics use for the callee.
    name: String,
    /// The value of each parameter a call may omit, by parameter; empty when
    /// none has one.
    defaults: Vec<Option<hir::Expr>>,
    /// `unsafe fn`: calls need an `unsafe` region (16.12).
    is_unsafe: bool,
    /// A C function: a `String` argument passes as a `ffi.CString`.
    foreign: bool,
}

impl Signature {
    /// The signature of a standard function taking `params` by value.
    fn simple(params: Vec<Ty>, result: Ty) -> Signature {
        let names = vec![String::new(); params.len()];
        let modes = vec![ast::Mode::Value; params.len()];
        Signature {
            params,
            names,
            receiver: None,
            modes,
            variadic: false,
            result,
            generics: Vec::new(),
            name: String::new(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        }
    }

    /// Whether a call may omit the parameter at the index.
    fn has_default(&self, index: usize) -> bool {
        self.defaults.get(index).is_some_and(Option::is_some)
    }
}

const INTRINSICS: [&str; 3] = ["panic", "assert", "channel"];

/// A place an assignment writes: the binding, the path in it, the type.
type Target = (LocalId, Vec<hir::PlaceStep>, Ty);

/// A step to a place, as far as two places can be told apart before the
/// program runs (13.8).
#[derive(Copy, Clone, PartialEq, Eq)]
enum PlaceKey {
    Field(u32),
    Constant(i128),
    Binding(LocalId),
    Unknown,
}

/// Why nothing changes the value of a `Ref` (16.10), and what to do.
const REF_UNCHANGED: &str = "the value of a `Ref` never changes: make a new `Ref`, or keep what changes in a `sync.Mutex`, which every copy shares";

/// Whether a value is the same at every evaluation and has no effects, as a
/// default value must be.
/// A field of a context whose initial value is computed (11.16): the
/// function that computes it in a `Ref` of the context's record, the error
/// its `?` may give, and its name, `Context.field`.
struct Initial {
    field: u32,
    function: hir::Function,
    error: Option<Ty>,
    named: String,
}

/// What the checker makes of a context (11.16): its maker, those that show
/// its fields for logs and labels, and its computed fields.
struct ContextMade {
    maker: hir::Function,
    log: Option<hir::Function>,
    label: Option<hir::Function>,
    initials: Vec<Initial>,
}

/// A computed initial value, for the cycles among them: its context and
/// field, where it is, its name and the fields it reads.
struct Reading {
    key: (u32, u32),
    span: Span,
    named: String,
    reads: Vec<(u32, u32)>,
}

fn constant(value: &mut hir::Expr) -> bool {
    constant_parts(value).is_some_and(|parts| parts.into_iter().all(constant))
}

/// What has field defaults: a record, or a generic one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Defaulted {
    Record(Ty),
    Template(TemplateId),
}

/// Whether a declaration is a record whose fields may have defaults, not a
/// context's or an annotation's, which have their own.
fn has_field_defaults(decl: &ast::TypeDecl) -> bool {
    !decl.context
        && !decl.annotation
        && matches!(&decl.body, ast::TypeBody::Record(fields) if fields.iter().any(|f| f.default.is_some()))
}

/// The literal an expression is, if it is one.
fn literal(value: &hir::Expr) -> Option<hir::Literal> {
    match &value.kind {
        hir::ExprKind::Int(v) => Some(hir::Literal::Int(*v)),
        hir::ExprKind::Str(s) => Some(hir::Literal::Str(s.clone())),
        hir::ExprKind::Bool(b) => Some(hir::Literal::Bool(*b)),
        hir::ExprKind::Char(c) => Some(hir::Literal::Char(*c)),
        _ => None,
    }
}

/// The float a constant expression of floats evaluates to.
fn fold_float(types: &Types, value: &hir::Expr) -> Option<f64> {
    if !matches!(types.kind(types.repr(value.ty)), TyKind::Float(_)) {
        return None;
    }
    match &value.kind {
        hir::ExprKind::Float(x) => Some(*x),
        hir::ExprKind::Convert(inner) => match inner.kind {
            hir::ExprKind::Int(n) => Some(n as f64),
            _ => fold_float(types, inner),
        },
        hir::ExprKind::Unary(UnaryOp::Neg, inner) => Some(-fold_float(types, inner)?),
        hir::ExprKind::Binary(op, left, right) => {
            let (a, b) = (fold_float(types, left)?, fold_float(types, right)?);
            match op {
                BinOp::Add => Some(a + b),
                BinOp::Sub => Some(a - b),
                BinOp::Mul => Some(a * b),
                BinOp::Div => Some(a / b),
                BinOp::Rem => Some(a % b),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The integer an expression of integers evaluates to, an error where
/// running it would panic, or `None` for anything else.
fn fold(types: &Types, value: &hir::Expr) -> Option<Result<i128, &'static str>> {
    let int = types.as_int(types.repr(value.ty))?;
    // A `UInt128` computes with its bits as `u128`, every other type exactly.
    let wide = int == IntTy::U128;
    let fits = |n: Option<i128>| match n {
        Some(n) if int.holds(n) => Ok(n),
        _ => Err("the result does not fit in its type"),
    };
    let unsigned = |n: Option<u128>| fits(n.map(|n| n as i128));
    let result = match &value.kind {
        hir::ExprKind::Int(n) => Ok(*n),
        hir::ExprKind::Convert(inner) | hir::ExprKind::Retype(inner) => fold(types, inner)?,
        hir::ExprKind::Unary(UnaryOp::Neg, inner) => fold(types, inner)?.and_then(|n| match wide {
            true => unsigned((n as u128).checked_neg()),
            false => fits(n.checked_neg()),
        }),
        hir::ExprKind::Unary(UnaryOp::BitNot, inner) => fold(types, inner)?.map(|n| int.wrap(!n)),
        hir::ExprKind::Binary(op, left, right) => {
            let (a, b) = (fold(types, left)?, fold(types, right)?);
            let (a, b) = match (a, b) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
            };
            let (x, y) = (a as u128, b as u128);
            // The count of a shift is in its own type, which may be another.
            let count =
                types.as_int(types.repr(right.ty)).map_or(b, |of| if of == IntTy::U128 && b < 0 { -1 } else { b });
            match op {
                BinOp::Add if wide => unsigned(x.checked_add(y)),
                BinOp::Sub if wide => unsigned(x.checked_sub(y)),
                BinOp::Mul if wide => unsigned(x.checked_mul(y)),
                BinOp::Add => fits(a.checked_add(b)),
                BinOp::Sub => fits(a.checked_sub(b)),
                BinOp::Mul => fits(a.checked_mul(b)),
                BinOp::Div | BinOp::Rem if b == 0 => Err("it divides by zero"),
                BinOp::Div if wide => unsigned(Some(x / y)),
                BinOp::Rem if wide => unsigned(Some(x % y)),
                BinOp::Div => fits(a.checked_div(b)),
                BinOp::Rem => Ok(a.checked_rem(b).unwrap_or(0)),
                BinOp::Shl | BinOp::Shr if count < 0 || count >= int.bits() as i128 => {
                    Err("the shift count is out of range")
                }
                BinOp::Shl => Ok(int.wrap(a << count)),
                BinOp::Shr => Ok(if int.signed() { a >> count } else { (x >> count) as i128 }),
                BinOp::BitAnd => Ok(a & b),
                BinOp::BitOr => Ok(a | b),
                BinOp::BitXor => Ok(a ^ b),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(result)
}

/// The function other languages' names for a standard operation mean.
fn std_alias(module: &str, member: &str) -> String {
    let function = match (module, member) {
        ("fs", "readFile" | "readAllText" | "readToString") => "readText",
        ("fs", "writeFile" | "writeAllText") => "writeText",
        ("fs", "readAllBytes") => "readBytes",
        ("fs", "writeAllBytes") => "writeBytes",
        ("fs", "appendFile" | "appendAllText") => "appendText",
        ("fs", "mkdir" | "mkdirAll" | "createDir" | "createDirAll" | "makeDirs" | "createDirectories") => {
            "createDirectory"
        }
        ("fs", "delete" | "unlink" | "removeFile" | "rmdir" | "removeDir") => "remove",
        ("fs", "removeAll" | "rmrf" | "removeDirAll" | "deleteAll" | "removeTree") => "remove(path, recursive: true)",
        ("fs", "readDir" | "readdir" | "listDir" | "listDirectory" | "entries") => "list",
        ("fs", "move" | "mv") => "rename",
        ("fs", "stat" | "isFile" | "isDir" | "isDirectory") => "metadata",
        ("fs", "openFile" | "create") => "open",
        ("path", "combine" | "resolve") => "join",
        ("path", "dirname" | "dirName" | "directoryName") => "parent",
        ("path", "basename" | "baseName") => "fileName",
        ("path", "ext" | "extname") => "extension",
        ("env", "argv" | "arguments") => "args",
        ("env", "getenv" | "getVar" | "variable") => "get",
        ("env", "setenv" | "setVar" | "put") => "set",
        ("env", "environ" | "variables" | "all") => "vars",
        ("env", "unset" | "unsetenv" | "delete") => "remove",
        ("env", "cwd" | "getcwd" | "workingDirectory") => "currentDirectory",
        ("env", "chdir" | "cd") => "setCurrentDirectory",
        ("env", "pid" | "getpid" | "getPid" | "processID") => "processId",
        ("env", "getHostname" | "gethostname" | "hostName" | "machineName") => "hostname",
        ("env", "currentExe" | "executablePath" | "exe") => "executable",
        ("time", "monotonic" | "instant" | "nowMonotonic") => "now",
        ("time", "unix" | "timestamp" | "epoch") => "unixTime",
        ("time", "utcnow" | "nowUtc" | "date" | "today") => "utcNow",
        ("time", "ms" | "millis") => "milliseconds",
        ("time", "secs") => "seconds",
        ("process", "run" | "exec" | "spawn" | "cmd" | "new") => "command",
        ("process", "sh" | "system") => "shell",
        _ => return String::new(),
    };
    format!("; use `{module}.{function}`")
}

/// The variant of `Tree` in `std/files.to` that holds the files of
/// `embed.files`: `Kept(names, contents, tags)`.
const KEPT_TREE: u32 = 2;

/// The outermost part of a type, which an `impl` for it must share.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Head {
    Adt(crate::types::AdtId),
    Template(crate::types::TemplateId),
    Kind(std::mem::Discriminant<TyKind>),
}

struct Checker<'a> {
    /// What an editor asks to know: each declaration and each name that
    /// refers to one.
    index: Option<crate::index::Index>,
    /// For an editor: the type of each expression of the program's own files.
    index_exprs: Vec<(Span, Ty)>,
    /// The `meta` profile: no host capabilities (27.7).
    meta: bool,
    /// Inside the `{...}` of a string, where an unknown name may be text
    /// that meant a brace.
    interpolating: bool,
    /// The bindings of a root script's statements, which its functions do
    /// not see (20.3), and whether its statements are being checked.
    script_bindings: Vec<String>,
    scripting: bool,
    /// The function whose body is checked, as stack traces name it, and
    /// the name the next closure takes instead of `closure in` it.
    naming: String,
    /// Where each name of the module was first declared, which a second
    /// declaration points to.
    declared_at: HashMap<String, Span>,
    /// For an editor: the name of the function whose body is checked, which
    /// its `return`, `fail` and `?` leave.
    leaving: Option<Span>,
    /// While the operands of a comparison meet, whose mismatch is one:
    /// whether the value coerced is the left one.
    comparing: Option<bool>,
    /// While a panic's `data` is checked, the type of its values, whose
    /// mismatch says to write a text instead.
    panic_datum: Option<Ty>,
    closure_name: Option<String>,
    /// Whether the next closure is a compiled template's.
    templating: bool,
    /// The test whose body is checked, which `testing.bench` makes a
    /// benchmark.
    measuring: Option<u32>,
    /// The key each field of a record, or of a variant as `Type.Variant`,
    /// has in every format: `@name`, its own, or none with `@ignore`; for
    /// the shapes of fuzz inputs.
    wires: HashMap<String, Vec<Option<String>>>,
    /// The standard functions whose values depend on the moment or the
    /// machine, which the `meta` profile cannot call, with their names.
    unrepeatable: HashMap<FnId, &'static str>,
    /// The functions of `std.model` that make a test that calls them one
    /// that checks a model.
    checks_models: HashSet<FnId>,
    /// The function whose body is being checked, and the functions each
    /// body calls, and each test: a test that reaches `checks_models` through
    /// its own functions checks a model too.
    calling: Option<FnId>,
    calls: HashMap<FnId, HashSet<FnId>>,
    test_calls: HashMap<u32, HashSet<FnId>>,
    /// The names the program gives the steps of its models, `model.step`'s,
    /// and those it marks or makes fair, each as its text up to the first
    /// interpolation, whether that was all of it, and where: a name of the
    /// second kind that is no step's is a mistake (`W1020`).
    model_steps: Vec<(String, bool)>,
    model_names: Vec<(&'static str, String, bool, Span)>,
    /// The functions whose bodies `spawn`, and the tests that do.
    spawning: HashSet<FnId>,
    test_spawns: HashSet<u32>,
    types: Types,
    diags: &'a mut Diagnostics,
    sources: &'a SourceMap,
    names: HashMap<String, ModuleName>,
    type_names: HashMap<String, TypeName>,
    /// The names of the prelude's types that the program declared again, an
    /// error already (E1005): its uses of them are the prelude's, and what
    /// they lack is no news.
    misnamed: HashSet<String>,
    aliases: Vec<AliasEntry>,
    signatures: Vec<Signature>,
    /// Inherent operations by owner type and name (11.5). A generic owner
    /// is keyed by its instance with its own parameters (`owner_key`).
    methods: HashMap<(Ty, String), FnId>,
    /// Type parameters in scope, innermost last.
    type_params: Vec<(String, Ty)>,
    /// What `Self` names here, inside a trait, an impl or a method.
    self_type: Option<Ty>,
    /// The trait the `impl` being checked implements, if any.
    implementing: Option<(TraitId, Vec<Ty>)>,
    traits: Vec<TraitDef>,
    trait_names: HashMap<String, TraitId>,
    /// The next closure is the body of a `try` block.
    try_body: bool,
    /// Calls whose result nothing uses, of functions not all known yet.
    dropped_calls: Vec<(FnId, Span)>,
    impls: Vec<ImplDef>,
    /// The `impl`s of each trait by the head of the type they are for, a
    /// type parameter's as `None`, so that asking whether a type has a
    /// trait looks only at those that may match; and how many are there.
    impl_heads: HashMap<(TraitId, Option<Head>), Vec<usize>>,
    heads_indexed: usize,
    /// How many diagnostics were counted for `error_count`, and how many of
    /// them are errors: the list only grows while a program is checked, the
    /// standard library's warnings with it, so each is counted once.
    errors_counted: std::cell::Cell<(usize, usize)>,
    /// The traits each type parameter is bound by, with their arguments.
    bounds: HashMap<Ty, Vec<(TraitId, Vec<Ty>)>>,
    /// The call protocol bounding a type parameter (11.8), and its
    /// `fn(...)` type.
    call_bounds: HashMap<Ty, (Protocol, Ty)>,
    /// The cursor types of maps and sets, by what they yield and whether
    /// they belong to a set.
    cursor_templates: HashMap<(CursorKind, bool), TemplateId>,
    /// The `FnId` of the first closure body; closures are numbered after
    /// every declared function.
    closure_base: u32,
    closure_functions: Vec<Option<hir::Function>>,
    /// In a test build, the tree of suites and tests (testing 3.2).
    test_nodes: Vec<hir::TestNodeInfo>,
    /// The scope of each function's signature and body.
    fn_scopes: Vec<FnScope>,
    /// `time.Duration`, once `std.time` is imported: nanoseconds.
    duration: Option<Ty>,
    /// The prelude's `Additive`, `Number` and `Integer` (18.10).
    additive: Option<TraitId>,
    number: Option<TraitId>,
    integer: Option<TraitId>,
    /// The prelude's `Ordered`, which the relational operators use (17.8).
    ordered: Option<TraitId>,
    /// `time.Instant`: nanoseconds of the monotonic clock.
    instant: Option<Ty>,
    /// `time.DateTime`: seven `Int` fields, from the year down.
    datetime: Option<Ty>,
    /// `time.TimeError`: a text that is not a date and time.
    time_error: Option<Ty>,
    /// `sync.CompareExchange`, once `std.sync` is imported: the template
    /// and its instance with its own parameter.
    compare_exchange: Option<(TemplateId, Ty)>,
    /// What the code does to cells of `std.sync`, in order.
    cell_uses: Vec<CellUse>,
    /// The types of `std.fs`, once imported.
    fs: Option<FsTypes>,
    /// The module whose declaration or code is being checked (7.5).
    module: u32,
    /// Who declares each nominal type, by name.
    owners: HashMap<String, Owner>,
    /// The module of each inherent operation, and whether it is `pub`.
    method_visibility: HashMap<FnId, (u32, bool)>,
    /// The module functions declared `pub`.
    public_fns: HashSet<FnId>,
    /// The module `std.prelude`, which declares the methods of the intrinsic
    /// types, and their keys by name.
    prelude: Option<u32>,
    /// The contexts the program declares (11.16), by index: each one's
    /// record type and declaration; and the index of each such type.
    contexts: Vec<(Ty, &'a ast::TypeDecl)>,
    context_of: HashMap<Ty, u32>,
    /// While the initial value of a context's field is checked, the fields
    /// of contexts it reads, for the cycles among them (11.16).
    initial_reads: Option<Vec<(u32, u32)>>,
    /// The annotations the program declares (27.13), by their types, and
    /// the values of their fields a use may leave out.
    annotation_decls: HashMap<Ty, &'a ast::TypeDecl>,
    /// The records with fields that have defaults not computed yet, with
    /// their types and the parameters of a generic one.
    defaulted: Vec<(Defaulted, Ty, &'a ast::TypeDecl, Vec<Ty>)>,
    /// The default of each field of a record, a constant, by its type or by
    /// its template; a literal may leave out the fields that have one.
    field_defaults: HashMap<Defaulted, Vec<Option<hir::Expr>>>,
    annotation_defaults: HashMap<Ty, Vec<Option<hir::Expr>>>,
    /// The program's annotations of each thing they annotate.
    annotation_values: Vec<(Annotated, Vec<hir::Expr>)>,
    /// The path of each module of the program, which an unknown name may
    /// be the last part of.
    module_paths: Vec<Vec<String>>,
    /// The module `std.datetime`, the part of `std.time` written in Tondo,
    /// which owns the types of `std.time` as their module would.
    datetime_module: Option<u32>,
    /// `std.pacing`, the companion of `std.sync`, which implements traits
    /// for its shared values.
    pacing_module: Option<u32>,
    intrinsic_owners: HashMap<String, Ty>,
    /// The module of each user trait.
    trait_modules: HashMap<TraitId, u32>,
    /// The prelude's `NumericConversionError`.
    conversion_error: Ty,
    /// `std.collections.CollectionError`.
    collection_error: Ty,
    /// `std.text.TextError`.
    text_error: Ty,
    /// `std.io.IoError`, which the console and files also use.
    io_error: Ty,
    /// `std.env.EnvError`, once `std.env` is imported.
    env_error: Option<Ty>,
    /// What `env.build()` reports of the program (standard library 14.3.6).
    built: Option<ast::Built>,
    /// What the files of the other systems declare (language 6.8).
    elsewhere: Vec<ast::Elsewhere>,
    /// The types of `std.process`, once imported.
    process: Option<ProcessTypes>,
    /// The `extern fn` declarations (FFI).
    externs: Vec<hir::Extern>,
    /// `ffi.CString` and `ffi.Void`, which `std.ffi` names after
    /// `std.internal.memory`.
    c_string: Ty,
    void: Ty,
    /// While the arguments of a C function are coerced: a `String` passes
    /// where it takes a `ffi.CString`, as a copy for the call.
    c_arguments: bool,
    /// The parameter names of each `extern fn`, for named arguments.
    extern_names: Vec<Vec<String>>,
    /// The functions C calls by their symbol, and where each symbol is
    /// defined.
    exports: Vec<hir::Export>,
    export_symbols: HashMap<String, Span>,
    /// What a library's header declares, and each `extern type` with a body.
    c_functions: Vec<hir::CFunction>,
    c_decls: HashMap<crate::types::AdtId, &'a ast::TypeDecl>,
    /// The module's constants (6.5), evaluated when first used.
    consts: Vec<ConstEntry<'a>>,
    /// Where an integer constant is used: its value fits any integer type
    /// that holds it, as a literal does (8.6).
    const_uses: HashSet<Span>,
}

/// The methods of the collections that change their extent: they take `var self`.
const MUTATIONS: [&str; 8] = ["push", "pushFront", "pop", "popFront", "insert", "removeAt", "remove", "clear"];

/// A constant: its declaration, and its value once evaluated.
struct ConstEntry<'a> {
    decl: &'a ast::ConstDecl,
    value: Option<hir::Expr>,
    resolving: bool,
}

/// The bindings of a function's open blocks, innermost last, and each
/// name's, so that finding one takes no search however many there are.
struct Scopes {
    blocks: Vec<Vec<(String, LocalId)>>,
    by_name: HashMap<String, Vec<LocalId>>,
}

impl Scopes {
    /// With the function's own block open.
    fn opened() -> Scopes {
        Scopes { blocks: vec![Vec::new()], by_name: HashMap::new() }
    }

    fn open(&mut self) {
        self.blocks.push(Vec::new());
    }

    /// Ends the innermost block: its bindings go out of view.
    fn close(&mut self) {
        for (name, _) in self.blocks.pop().unwrap_or_default() {
            if let Some(ids) = self.by_name.get_mut(&name) {
                ids.pop();
                if ids.is_empty() {
                    self.by_name.remove(&name);
                }
            }
        }
    }

    fn bind(&mut self, name: String, id: LocalId) {
        self.by_name.entry(name.clone()).or_default().push(id);
        if let Some(block) = self.blocks.last_mut() {
            block.push((name, id));
        }
    }

    /// The innermost binding of `name`.
    fn find(&self, name: &str) -> Option<LocalId> {
        self.by_name.get(name).and_then(|ids| ids.last().copied())
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().flatten().map(|(name, _)| name.as_str())
    }
}

/// Per-function checking state.
struct FnCx {
    locals: Vec<hir::Local>,
    /// The `with`s that hold where the code is checked (11.16): the
    /// context, its fields changed, a bit each, and where it is.
    withs: Vec<(u32, u64, Span)>,
    /// For an editor: the declaration of each named local.
    local_defs: HashMap<u32, usize>,
    /// Where the function's result type is written, which a repair may
    /// widen, and whether the function is public.
    result_span: Option<(Span, bool)>,
    mutable: Vec<bool>,
    scopes: Scopes,
    /// Success type that `return` produces.
    result: Ty,
    /// Error type of a fallible function (`T ! E`), or of a script whose
    /// errors are still being inferred.
    error: Option<Ty>,
    /// A root script and `main` infer their error type from `?` and
    /// `fail` (20.3).
    infer_error: bool,
    /// What a function that declares no error propagates, and where it
    /// first does: one diagnostic at its end says how to declare it.
    undeclared: Option<(Ty, Span, &'static str)>,
    /// Whether this is a declared function's body, which gathers
    /// `undeclared` rather than reporting each `?`.
    gathers: bool,
    /// How many errors there were when the body began: one in it may have
    /// left uses of its bindings unchecked.
    errors_before: usize,
    /// One entry per enclosing loop: whether a `break` targets it.
    loops: Vec<bool>,
    /// The current path has unconditionally left the enclosing block.
    diverged: bool,
    /// For a closure body: the enclosing context, whose bindings it
    /// captures by value when first used (11.8).
    parent: Option<Box<FnCx>>,
    /// The closure itself, the first parameter of its body.
    env: Option<LocalId>,
    captures: Vec<Capture>,
    /// Whether the body changes a capture (`CallMut`).
    mutates: bool,
    /// A closure without a declared result infers it from its body.
    infer_result: bool,
    /// The body of a `try` block, which ends with its value (15.2).
    try_body: bool,
    /// For each local: how it was passed if it is a parameter (16.3), or
    /// how it borrows if it is a `ref`, `mut` or `var` binding (14.3).
    params: Vec<Option<ast::Mode>>,
    /// The body of a `defer` block, which cannot leave its scope (13.7).
    deferred: bool,
    /// Set while the next block checked is a function's body, whose value
    /// the function returns.
    body: bool,
    /// Inside an `unsafe` function, closure or block: raw operations and
    /// calls of `unsafe` functions are allowed (16.12).
    unsafe_region: bool,
    /// The body of an `unsafe` function or closure, which may capture raw
    /// pointers.
    unsafe_body: bool,
    /// Collections lent to the bindings of enclosing `for` loops (13.3),
    /// and whether exclusively.
    lent: Vec<(hir::Place, bool)>,
    /// The `ref`, `mut` and `var` bindings of patterns, as opposed to
    /// parameters.
    loan_bindings: HashSet<LocalId>,
    /// The closure that runs a test or a suite, which uses only the `let`
    /// bindings of `Copy` types of its suites (testing 4.3).
    test_member: bool,
    /// The setup of a suite, which `return` cannot leave (testing 4.1).
    setup: bool,
    /// The bindings the program names, and those it reads (22.8).
    named: Vec<LocalId>,
    reads: HashSet<LocalId>,
    /// The closure of a compiled template (standard library 14.31): it
    /// captures nothing, and a name that is no binding is a field of its
    /// data, `self`.
    template: bool,
}

/// What a closure is: a `defer` block, `unsafe`, or the closure of a test
/// or a suite.
#[derive(Copy, Clone)]
struct ClosureKind {
    deferred: bool,
    is_unsafe: bool,
    test: bool,
    /// Infers its errors, as the closure of `testing.withVirtualTime`.
    infer_error: bool,
}

/// The protocols a call bound names (11.8), each implying the next.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Protocol {
    Call,
    CallMut,
    CallOnce,
}

/// How a callable value is called (11.7, 11.8): its parameters and their
/// modes, its result, whether the last parameter is variadic, and whether
/// calling it may change it.
struct Callable {
    params: Vec<Ty>,
    result: Ty,
    variadic: bool,
    mutates: bool,
    /// A call consumes it (`CallOnce`).
    once: bool,
    modes: Vec<ast::Mode>,
    /// Calling it needs an `unsafe` region (16.12).
    is_unsafe: bool,
}

/// The locked state of a `std.sync` value, inside the block `locked`
/// builds: `inner` is a loan of the state, and `args` the arguments,
/// evaluated before taking the lock.
struct Lock {
    inner: LocalId,
    guard: LocalId,
    ty: Ty,
    args: Vec<(LocalId, Ty)>,
    span: Span,
}

impl Lock {
    fn mk(&self, kind: hir::ExprKind, ty: Ty) -> hir::Expr {
        hir::Expr { kind, ty, span: self.span }
    }

    /// The state, read where it is.
    fn state(&self) -> hir::Expr {
        self.mk(hir::ExprKind::Local(self.inner), self.ty)
    }

    fn place(&self) -> hir::Place {
        hir::Place { local: self.inner, path: Vec::new() }
    }

    fn loan(&self) -> hir::Expr {
        self.mk(hir::ExprKind::Loan(self.place()), self.ty)
    }

    fn arg(&self, index: usize) -> hir::Expr {
        let (local, ty) = self.args[index];
        self.mk(hir::ExprKind::Local(local), ty)
    }

    /// `state = value`, or `state op= value`.
    fn assign(&self, path: Vec<hir::PlaceStep>, op: Option<BinOp>, value: hir::Expr) -> hir::Stmt {
        hir::Stmt::Assign { local: self.inner, path, op, value, span: self.span }
    }
}

/// What declares a nominal type (7.5–7.7): its module, whether it is `pub`,
/// and which fields of a record are `priv`.
struct Owner {
    module: u32,
    public: bool,
    private_fields: Vec<bool>,
}

/// A binding a closure captures: read from `source` in the enclosing
/// context when the closure is created.
struct Capture {
    name: String,
    ty: Ty,
    source: hir::Expr,
    mutable: bool,
}

impl<'a> Checker<'a> {
    /// A local that no name refers to, such as the value of `for _ in ...`.
    fn hidden_local(&mut self, cx: &mut FnCx, ty: Ty) -> LocalId {
        cx.locals.push(hir::Local { name: "_".into(), ty, mutable: false, span: Span::default() });
        cx.mutable.push(false);
        cx.params.push(None);
        LocalId(cx.locals.len() as u32 - 1)
    }

    fn declare(&mut self, cx: &mut FnCx, name: &ast::Ident, ty: Ty, mutable: bool) -> LocalId {
        self.declare_with(cx, name, ty, mutable, None)
    }

    /// Declares a binding, recorded for an editor under `key` when it is a
    /// parameter, which calls name too.
    fn declare_with(
        &mut self,
        cx: &mut FnCx,
        name: &ast::Ident,
        ty: Ty,
        mutable: bool,
        key: Option<crate::index::Key>,
    ) -> LocalId {
        // `_` takes a value without binding it (5.4).
        let discard = name.name == "_";
        // Another module's bindings may reuse the root's names, which only
        // the root sees unprefixed; the linker checks each module's own.
        if !discard
            && (self.lookup(cx, &name.name).is_some()
                || self.visible_capture(cx, &name.name)
                || (self.module == 0 && self.names.contains_key(&name.name)))
        {
            let message = format!(
                "`{0}` would shadow a visible binding with the same name: choose another name, or assign `{0} = ...` if it is a `var`",
                name.name
            );
            self.diags.error("E1003", name.span, message);
            if let Some(earlier) = self.binding_span(cx, &name.name) {
                self.diags.related(earlier, format!("`{}` is declared here", name.name));
            }
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        }
        let id = LocalId(cx.locals.len() as u32);
        let kind = if key.is_some() { crate::index::DefKind::Parameter } else { crate::index::DefKind::Local };
        if let Some(def) = self.index_define(key, kind, name, name.span, Some(ty)) {
            cx.local_defs.insert(id.0, def);
        }
        cx.locals.push(hir::Local { name: name.name.clone(), ty, mutable, span: name.span });
        cx.mutable.push(mutable);
        cx.params.push(None);
        if !discard {
            cx.scopes.bind(name.name.clone(), id);
            cx.named.push(id);
        }
        id
    }

    /// Whether `param` is there only for its type: it alone gives a `T` of
    /// the function, and the body names that `T`.
    fn gives_type(&self, f: &ast::FnDecl, param: &ast::Param) -> bool {
        let (Some(ty), Some(body)) = (&param.ty, &f.body) else { return false };
        let file = self.sources.file_of(ty.span.start);
        let name = file.slice(ty.span);
        f.generics.iter().any(|generic| generic.name.name == name)
            && crate::lints::names(file, body.span, name)
            && !f.params.iter().any(|other| {
                !std::ptr::eq(other, param)
                    && other.ty.as_ref().is_some_and(|ty| crate::lints::names(file, ty.span, name))
            })
    }

    /// Warns of the bindings and parameters a body never reads (`W1002`,
    /// `W1003`); `self` and names that start with `_` are exempt. A body
    /// with an error has none: the error may be where a binding is used,
    /// in code the checker could not follow.
    fn unused(&mut self, cx: &FnCx, params: &[LocalId]) {
        if self.error_count() > cx.errors_before {
            return;
        }
        for &local in &cx.named {
            let hir::Local { name, span, .. } = &cx.locals[local.0 as usize];
            if cx.reads.contains(&local) || name == "self" || !name.starts_with(char::is_alphabetic) {
                continue;
            }
            if params.contains(&local) {
                let message = format!(
                    "the parameter `{name}` is never used: write `_` instead, or `_{name}` to hold its value until the function ends"
                );
                self.diags.warning("W1003", *span, message);
                self.diags.fix(format!("rename it `_{name}`"), false, vec![(*span, format!("_{name}"))]);
            } else {
                let message = format!(
                    "`{name}` is never used: write `_` instead, or `_{name}` to hold its value until the block ends"
                );
                self.diags.warning("W1002", *span, message);
                self.diags.fix(format!("rename it `_{name}`"), false, vec![(*span, format!("_{name}"))]);
            }
        }
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self, cx: &mut FnCx, b: &ast::Block, expected: Option<Ty>) -> hir::Block {
        let body = std::mem::take(&mut cx.body);
        cx.scopes.open();
        let withs = cx.withs.len();
        let outer = std::mem::replace(&mut cx.diverged, false);
        let stmts = self.statements(cx, &b.stmts);
        let tail = b.tail.as_ref().map(|t| {
            if cx.diverged {
                self.diags.warning("W1006", t.span, "unreachable expression");
            }
            let value = self.expr(cx, t, expected);
            if expected == Some(Ty::UNIT) {
                self.dropped_value(t, &value);
            }
            Box::new(match expected {
                Some(expected) if body => self.returned(cx, value, expected),
                Some(expected) => self.coerce(value, expected),
                None => value,
            })
        });
        let ty = match &tail {
            _ if cx.diverged => Ty::NEVER,
            Some(tail) => tail.ty,
            None => match expected.filter(|&t| t != Ty::UNIT && t != Ty::ERROR) {
                Some(expected) => {
                    let mut message = match body {
                        true => format!(
                            "this function returns {}, but its body ends without a value: end it with one, or `return` one",
                            self.display(expected)
                        ),
                        false => format!("expected {}, found `Unit`", self.display(expected)),
                    };
                    // `{}` written for an empty collection.
                    let empty = match self.types.kind(expected) {
                        _ if !stmts.is_empty() => None,
                        TyKind::Map(..) => Some("an empty map is `[:]`"),
                        TyKind::Set(_) => Some("an empty set is `Set[]`"),
                        TyKind::Array(_) => Some("an empty array is `[]`"),
                        _ => None,
                    };
                    if let Some(empty) = empty {
                        message.push_str(&format!(": `{{}}` is an empty block; {empty}"));
                    }
                    let end = Span::new(b.span.end - 1, b.span.end);
                    self.diags.error("E1102", end, message);
                    // Reported once, here.
                    Ty::ERROR
                }
                None => Ty::UNIT,
            },
        };
        cx.diverged = outer;
        cx.scopes.close();
        cx.withs.truncate(withs);
        hir::Block { stmts, tail, ty }
    }

    fn statements(&mut self, cx: &mut FnCx, stmts: &[ast::Stmt]) -> Vec<hir::Stmt> {
        let mut out = Vec::new();
        let mut warned = false;
        for stmt in stmts {
            let reachable = !cx.diverged;
            if !reachable && !warned {
                self.diags.warning("W1006", stmt.span, "unreachable statement");
                warned = true;
            }
            if let Some(stmt) = self.statement(cx, stmt)
                && reachable
            {
                out.push(stmt);
            }
        }
        out
    }

    fn statement(&mut self, cx: &mut FnCx, stmt: &ast::Stmt) -> Option<hir::Stmt> {
        match &stmt.kind {
            ast::StmtKind::With { path, fields } => {
                let name = Self::path_text(path);
                let Some((ty, context)) = self.context_named(path) else {
                    let message = format!(
                        "`with {name} {{ ... }}` changes a context, but `{name}` is not one: a context is declared `context {name} = {{ field: Type = value }}`, and a record is updated with `value with {{ field: value }}`"
                    );
                    self.diags.error("E1617", stmt.span, message);
                    return None;
                };
                let reference = self.types.handle(Handle::Ref, ty);
                let base = Self::mk(hir::ExprKind::ContextRead(context, reference), ty, stmt.span);
                let value = self.update(cx, base, fields, true, stmt.span);
                let value = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![value]), reference, stmt.span);
                let names = self.record_fields(ty, stmt.span).unwrap_or_default();
                let changed = fields.iter().filter_map(|f| names.iter().position(|(name, _)| *name == f.name.name));
                let fields = changed.fold(0u64, |set, index| set | 1 << index.min(63));
                cx.withs.push((context, fields, stmt.span));
                Some(hir::Stmt::ContextSet { context, value, fields, span: stmt.span })
            }
            // `with captured` gives back the contexts of a `Contexts`.
            ast::StmtKind::Activate(captured) => {
                let contexts = match self.type_names.get("Contexts") {
                    Some(&TypeName::Nominal(ty)) => ty,
                    _ => return None,
                };
                let value = self.expr(cx, captured, Some(contexts));
                if value.ty != contexts {
                    if !self.types.has_error(value.ty) {
                        let message = format!(
                            "`with` takes a context's fields, as `with Trace {{ span }}`, or contexts that `Contexts.current()` captured, as `with captured`; this is {}",
                            self.display(value.ty)
                        );
                        self.diags.error("E1617", captured.span, message);
                    }
                    return None;
                }
                let Some(&ModuleName::Fn(activate)) = self.names.get("prelude.activateContexts") else { return None };
                let activate =
                    Self::mk(hir::ExprKind::Call(Callee::Fn(activate), Vec::new(), vec![value]), Ty::INT, stmt.span);
                Some(hir::Stmt::ContextsActivate { activate, span: stmt.span })
            }
            ast::StmtKind::Let { mutable, pattern, ty, value } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let Some(value) = value else {
                    self.diags.error("E1109", stmt.span, "a binding must be initialized where it is declared");
                    return None;
                };
                let value = self.expr(cx, value, declared);
                let value = match declared {
                    Some(declared) => self.coerce(value, declared),
                    None => value,
                };
                cx.diverged |= value.ty == Ty::NEVER;
                let ty = declared.unwrap_or(value.ty);
                match &pattern.kind {
                    ast::PatternKind::Wildcard => Some(hir::Stmt::Expr(value)),
                    ast::PatternKind::Binding(name) => {
                        if let Some(read) =
                            self.cell_uses.last_mut().filter(|used| used.read && used.span == value.span)
                        {
                            read.within = name.span;
                            read.statement = stmt.span;
                        }
                        let local = self.declare(cx, name, ty, *mutable);
                        Some(hir::Stmt::Let(local, value))
                    }
                    _ => {
                        let pattern = self.pattern(
                            cx,
                            pattern,
                            ty,
                            PatternContext { in_match: false, mutable: *mutable, loans: Loans::None },
                        );
                        if let Some(missing) = crate::exhaust::refutation(&self.types, ty, &pattern) {
                            let message = format!("this pattern can fail, for example on `{missing}`; use `match`");
                            self.diags.error("E1201", pattern.span, message);
                        }
                        Some(hir::Stmt::Destructure(pattern, value))
                    }
                }
            }
            ast::StmtKind::Assign { target, op, value } => self.assign(cx, target, *op, value, stmt.span),
            ast::StmtKind::Discard(value) => {
                let value = self.expr(cx, value, None);
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Expr(call @ ast::Expr { kind: ast::ExprKind::Spawn(_, false), .. }) => {
                Some(self.spawn_statement(cx, call))
            }
            ast::StmtKind::Expr(value) => {
                // A line that starts with `-` is a value of its own (5.2):
                // a subtraction split before its `-` becomes one.
                if let ast::ExprKind::Unary(ast::UnaryOp::Neg, _) = value.kind {
                    let message = "this line starts with `-`, so it is a negative value of its own that nothing uses, not a subtraction from the line before: to subtract, end that line with `-`";
                    self.diags.warning("W1014", value.span, message);
                }
                // The value of a block, `if` or `match` statement is dropped.
                let expected = matches!(
                    value.kind,
                    ast::ExprKind::Block(_) | ast::ExprKind::If { .. } | ast::ExprKind::Match { .. }
                )
                .then_some(Ty::UNIT);
                let written = value;
                let value = self.expr(cx, written, expected);
                self.dropped_value(written, &value);
                // Only a result or an option must be used: it may be a
                // failure or an absence the program has to see (11.9).
                if self.must_use(value.ty) {
                    let message = format!(
                        "a value of type {} is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    );
                    self.diags.error("E1303", value.span, message);
                    let start = Span::new(value.span.start, value.span.start);
                    let end = Span::new(value.span.end, value.span.end);
                    if matches!(self.types.kind(value.ty), TyKind::Result(..)) && cx.error.is_some() {
                        self.diags.fix("pass the failure on with `?`", false, vec![(end, "?".into())]);
                    }
                    self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
                }
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Return(_) | ast::StmtKind::Fail(_) if cx.deferred => {
                let message = "a defer block cannot leave its scope; handle the case inside the block";
                self.diags.error("E1410", stmt.span, message);
                None
            }
            ast::StmtKind::Defer(_) if cx.deferred => {
                self.diags.error("E1410", stmt.span, "a defer block cannot register another `defer`");
                None
            }
            ast::StmtKind::Defer(deferred) => self.defer(cx, deferred, stmt.span),
            ast::StmtKind::Return(_) if cx.try_body => {
                let message = "a `try` block ends with its value, and `return` would only leave the block: \
                    end it, or take `?` out of the block";
                self.diags.error("E1410", stmt.span, message);
                None
            }
            ast::StmtKind::Return(_) if cx.setup => {
                let message = "the setup of a suite cannot `return`, or its tests would never run; use `fail` or `testing.skip(reason)`";
                self.diags.error("E1205", stmt.span, message);
                None
            }
            ast::StmtKind::Return(value) => {
                self.note_exit(cx, Span::new(stmt.span.start, stmt.span.start + 6));
                let result = cx.result;
                let value = match value {
                    // A closure without a declared result takes it from its first `return`.
                    Some(value) if cx.infer_result && result == Ty::ERROR => {
                        let value = self.expr(cx, value, None);
                        cx.result = value.ty;
                        Some(value)
                    }
                    // `return err(e)` in a function that fails: its error
                    // leaves by `fail`, as `return` gives its success.
                    Some(ast::Expr { kind: ast::ExprKind::Construct(constructor, error), .. })
                        if *constructor == "err"
                            && cx.error.is_some()
                            && !matches!(self.types.kind(result), TyKind::Result(..)) =>
                    {
                        let written = format!("fail {}", self.sources.slice(error.span));
                        let message = format!(
                            "`return` gives a function's success, also when it can fail; its error leaves with `fail`: \
                             write `{written}`"
                        );
                        self.diags.error("E1304", stmt.span, message);
                        self.diags.fix(format!("write `{written}`"), true, vec![(stmt.span, written)]);
                        self.expr(cx, error, None);
                        None
                    }
                    Some(value) => {
                        let value = self.expr(cx, value, Some(result));
                        if result == Ty::UNIT && ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&value.ty) {
                            let message = format!(
                                "this function has no result, so its `return` takes no value: declare one, as `: {}` after the parameters, or write `return` alone",
                                self.display(value.ty).trim_matches('`')
                            );
                            self.diags.error("E1102", value.span, message);
                        }
                        Some(self.returned(cx, value, result))
                    }
                    None => {
                        if result != Ty::UNIT && result != Ty::ERROR {
                            let message =
                                format!("`return` without a value in a function returning {}", self.display(result));
                            self.diags.error("E1102", stmt.span, message);
                        }
                        None
                    }
                };
                cx.diverged = true;
                Some(hir::Stmt::Return(value))
            }
            ast::StmtKind::Fail(value) => {
                let declared = cx.error;
                let value = self.expr(cx, value, declared);
                // It ends the function, even where it cannot.
                cx.diverged = true;
                let error = self.error_channel(cx, value.ty, stmt.span, "E1302")?;
                self.note_propagation(cx, value.ty, Span::new(stmt.span.start, stmt.span.start + 4), "fail");
                self.note_exit(cx, Span::new(stmt.span.start, stmt.span.start + 4));
                let value = self.coerce(value, error);
                Some(hir::Stmt::Fail(value))
            }
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                let is_break = matches!(stmt.kind, ast::StmtKind::Break);
                let Some(saw_break) = cx.loops.last_mut() else {
                    let keyword = if is_break { "break" } else { "continue" };
                    let message = format!("`{keyword}` outside of a loop");
                    self.diags.error("E1205", stmt.span, message);
                    return None;
                };
                *saw_break |= is_break;
                cx.diverged = true;
                Some(if is_break { hir::Stmt::Break } else { hir::Stmt::Continue })
            }
            ast::StmtKind::For { header, body } => self.for_loop(cx, header, body),
        }
    }

    fn assign(
        &mut self,
        cx: &mut FnCx,
        target: &ast::Expr,
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        if let Some(at) = self.ref_value_target(cx, target) {
            self.diags.error("E1411", at, REF_UNCHANGED);
            return None;
        }
        if let ast::ExprKind::Tuple(targets) = &target.kind {
            return self.multiple_assign(cx, targets, op, value, span);
        }
        if let ast::ExprKind::Slice { base, start, end, step } = &target.kind {
            return self.assign_slice(cx, base, [start, end, step], op, value, span);
        }
        if let ast::ExprKind::Index { base, index } = &target.kind {
            let (local, path, ty) = self.place(cx, base)?;
            if let TyKind::Map(key, entry) = *self.types.kind(ty) {
                if path.is_empty() && cx.params[local.0 as usize] == Some(ast::Mode::Mut) {
                    let name = &cx.locals[local.0 as usize].name;
                    let message = format!(
                        "`{name}[key] = value` can add a key to `{name}`, a `mut` loan that keeps its keys; take it as `var`"
                    );
                    self.diags.error("E1411", span, message);
                }
                // `map[key] = value` inserts or replaces (10.11); there is no
                // compound form because the key may be absent.
                if op.is_some() {
                    self.diags.error("E1411", span, "a map entry has no compound assignment; read it with `get` first");
                    return None;
                }
                let key_value = self.expr(cx, index, Some(key));
                let key_value = self.coerce(key_value, key);
                let new = self.expr(cx, value, Some(entry));
                let new = self.coerce(new, entry);
                let result = self.types.intern(TyKind::Option(entry));
                self.check_lent_write(cx, local, &path, span);
                let place = hir::Place { local, path };
                let insert = hir::ExprKind::Mutate(hir::Mutation::Insert, place, vec![key_value, new]);
                return Some(hir::Stmt::Expr(Self::mk(insert, result, span)));
            }
            let (local, path, ty) = self.index_place(cx, (local, path, ty), index, base.span)?;
            return self.assign_to(cx, (local, path, ty), op, value, span);
        }
        let place = self.place(cx, target)?;
        self.assign_to(cx, place, op, value, span)
    }

    /// `(a, b) = value` (13.8): the places are resolved left to right, with
    /// their indices evaluated then; the value is evaluated whole into
    /// temporaries; and each place is written, left to right. `_` drops its
    /// component. Swapping values that are not `Copy` works, as writing a
    /// moved binding fills it again.
    fn multiple_assign(
        &mut self,
        cx: &mut FnCx,
        targets: &[ast::Expr],
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        if let Some(op) = op {
            let message = format!("a tuple of places has no `{}=`; assign each place on its own", op.as_str());
            self.diags.error("E1411", span, message);
            return None;
        }
        let mut stmts = Vec::new();
        // Each place, and how it is reached, to find places assigned twice.
        let mut places: Vec<Option<(Target, Vec<PlaceKey>, Span)>> = Vec::new();
        for target in targets {
            if matches!(&target.kind, ast::ExprKind::Path(segments) if segments.len() == 1 && segments[0].name == "_") {
                places.push(None);
                continue;
            }
            if let Some(at) = self.ref_value_target(cx, target) {
                self.diags.error("E1411", at, REF_UNCHANGED);
                return None;
            }
            let (local, path, ty) = match &target.kind {
                ast::ExprKind::Index { base, index } => {
                    let base_place = self.place(cx, base)?;
                    if let TyKind::Map(..) = self.types.kind(base_place.2) {
                        let message =
                            "a map entry cannot be one of several places assigned at once; assign it on its own";
                        self.diags.error("E1411", target.span, message);
                        return None;
                    }
                    self.index_place(cx, base_place, index, base.span)?
                }
                ast::ExprKind::Slice { .. } => {
                    let message = "a slice cannot be one of several places assigned at once; assign it on its own";
                    self.diags.error("E1411", target.span, message);
                    return None;
                }
                _ => self.place(cx, target)?,
            };
            let keys = path
                .iter()
                .map(|step| match step {
                    hir::PlaceStep::Field(index) => PlaceKey::Field(*index),
                    hir::PlaceStep::Index(index) => match &index.kind {
                        hir::ExprKind::Int(value) => PlaceKey::Constant(*value),
                        hir::ExprKind::Local(local) => PlaceKey::Binding(*local),
                        _ => PlaceKey::Unknown,
                    },
                })
                .collect();
            // Indices are evaluated now, before the value.
            let path = path
                .into_iter()
                .map(|step| match step {
                    hir::PlaceStep::Index(index) => {
                        let held = self.hidden_local(cx, Ty::INT);
                        let read = Self::mk(hir::ExprKind::Local(held), Ty::INT, index.span);
                        stmts.push(hir::Stmt::Let(held, index));
                        hir::PlaceStep::Index(read)
                    }
                    step => step,
                })
                .collect();
            places.push(Some(((local, path, ty), keys, target.span)));
        }
        for (index, first) in places.iter().enumerate() {
            let Some(((local, _, _), keys, _)) = first else { continue };
            for second in places[index + 1..].iter().flatten() {
                let ((other, _, _), other_keys, at) = second;
                let same = keys.iter().zip(other_keys).all(|(a, b)| a == b && *a != PlaceKey::Unknown);
                if local == other && same {
                    let name = &cx.locals[local.0 as usize].name;
                    let message = format!(
                        "this place overlaps another one assigned in the same statement, inside `{name}`; give each component its own place"
                    );
                    self.diags.error("E1405", *at, message);
                    return None;
                }
            }
        }
        let wanted: Option<Vec<Ty>> = places.iter().map(|p| p.as_ref().map(|((_, _, ty), _, _)| *ty)).collect();
        let expected = wanted.map(|items| self.types.intern(TyKind::Tuple(items)));
        let value = self.expr(cx, value, expected);
        let value = match expected {
            Some(expected) if matches!(self.types.kind(value.ty), TyKind::Tuple(items) if items.len() == targets.len()) => {
                self.coerce(value, expected)
            }
            _ => value,
        };
        let items = match self.types.kind(value.ty).clone() {
            TyKind::Tuple(items) if items.len() == targets.len() => items,
            TyKind::Error => return None,
            _ => {
                let message = format!(
                    "{} places are assigned a value of type {}; the value must be a tuple of {} components",
                    targets.len(),
                    self.display(value.ty),
                    targets.len()
                );
                self.diags.error("E1102", value.span, message);
                return None;
            }
        };
        // `let (t0, t1, ...) = value`, then each place from its temporary.
        let mut parts = Vec::new();
        let mut writes = Vec::new();
        for (place, &ty) in places.into_iter().zip(&items) {
            let pattern_span = place.as_ref().map_or(value.span, |(_, _, at)| *at);
            let kind = match place {
                Some(((local, path, place_ty), _, at)) => {
                    let held = self.hidden_local(cx, ty);
                    let read = Self::mk(hir::ExprKind::Local(held), ty, at);
                    let read = self.coerce(read, place_ty);
                    self.check_lent_write(cx, local, &path, at);
                    writes.push(hir::Stmt::Assign { local, path, op: None, value: read, span: at });
                    hir::PatKind::Binding(held)
                }
                None => hir::PatKind::Wildcard,
            };
            parts.push(hir::Pattern { kind, ty, span: pattern_span });
        }
        let pattern = hir::Pattern { kind: hir::PatKind::Fields(parts), ty: value.ty, span: value.span };
        stmts.push(hir::Stmt::Destructure(pattern, value));
        stmts.extend(writes);
        let block = hir::Block { stmts, tail: None, ty: Ty::UNIT };
        Some(hir::Stmt::Expr(Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span)))
    }

    /// `place op= value` once the target is resolved.
    fn assign_to(
        &mut self,
        cx: &mut FnCx,
        place: (LocalId, Vec<hir::PlaceStep>, Ty),
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        let (local, path, ty) = place;
        self.check_lent_write(cx, local, &path, span);
        // Replacing the root of a `mut` loan must keep its extent (16.3).
        if op.is_none()
            && path.is_empty()
            && cx.params[local.0 as usize] == Some(ast::Mode::Mut)
            && !self.fixed_extent(ty)
        {
            let name = &cx.locals[local.0 as usize].name;
            let message = format!(
                "assigning `{name}` may change its extent, but it is a `mut` loan; take it as `var`, or assign elements or a slice such as `{name}[:] = replacement`"
            );
            self.diags.error("E1411", span, message);
        }
        let value = match op {
            Some(BinOp::Shl | BinOp::Shr) => {
                if self.types.as_int(ty).is_none() && ty != Ty::ERROR && !self.bound_by(ty, self.integer) {
                    let message =
                        format!("`{}=` requires an integer target, found {}", op.unwrap().as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, None);
                self.require_int(&value);
                value
            }
            // `place = Trait.method(place, value)` on a type that takes the
            // operator by a trait (18.10).
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div))
                if let Some((trait_id, index)) = self.operator_trait(op, ty) =>
            {
                let value = self.expr(cx, value, Some(ty));
                let value = self.coerce(value, ty);
                let read = self.place_read(cx, &hir::Place { local, path: path.clone() }, span);
                let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![read, value]);
                let value = Self::mk(call, ty, span);
                cx.diverged |= value.ty == Ty::NEVER;
                return Some(hir::Stmt::Assign { local, path, op: None, value, span });
            }
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem)) => {
                self.compound_value(cx, op, ty, value, span)
            }
            Some(op) => {
                let bitwise = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error)
                    || self.bound_by(ty, self.integer);
                if !bitwise {
                    let message = format!("`{}=` cannot be applied to {}", op.as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
            None => {
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
        };
        cx.diverged |= value.ty == Ty::NEVER;
        Some(hir::Stmt::Assign { local, path, op, value, span })
    }

    /// Where an assignment target reaches into the value of a `Ref`, which
    /// never changes (16.10).
    fn ref_value_target(&self, cx: &FnCx, target: &ast::Expr) -> Option<Span> {
        match &target.kind {
            ast::ExprKind::Path(segments) => {
                let local = self.lookup(cx, &segments[0].name)?;
                let mut ty = cx.locals[local.0 as usize].ty;
                for segment in &segments[1..] {
                    if let TyKind::Handle(Handle::Ref, _) = self.types.kind(ty) {
                        return (segment.name == "value").then_some(segment.span);
                    }
                    ty = self.field_type(ty, &segment.name)?;
                }
                None
            }
            ast::ExprKind::Field(base, name) => {
                if name.name == "value"
                    && let Some(ty) = self.target_type(cx, base)
                    && let TyKind::Handle(Handle::Ref, _) = self.types.kind(ty)
                {
                    return Some(name.span);
                }
                self.ref_value_target(cx, base)
            }
            ast::ExprKind::Index { base, .. }
            | ast::ExprKind::Slice { base, .. }
            | ast::ExprKind::TupleSlot(base, _) => self.ref_value_target(cx, base),
            _ => None,
        }
    }

    /// The type of an assignment target, without reporting anything.
    fn target_type(&self, cx: &FnCx, target: &ast::Expr) -> Option<Ty> {
        let ty = match &target.kind {
            ast::ExprKind::Path(segments) => {
                let local = self.lookup(cx, &segments[0].name)?;
                let mut ty = cx.locals[local.0 as usize].ty;
                for segment in &segments[1..] {
                    ty = self.field_type(ty, &segment.name)?;
                }
                return Some(ty);
            }
            ast::ExprKind::Field(base, name) => return self.field_type(self.target_type(cx, base)?, &name.name),
            ast::ExprKind::TupleSlot(base, slot) => match self.types.kind(self.target_type(cx, base)?) {
                TyKind::Tuple(items) => return items.get(*slot as usize).copied(),
                _ => return None,
            },
            ast::ExprKind::Index { base, .. } => self.target_type(cx, base)?,
            _ => return None,
        };
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) => Some(*element),
            TyKind::Map(_, value) => Some(*value),
            _ => None,
        }
    }

    /// The type of a field, of `value` of a newtype or a `Ref`.
    fn field_type(&self, ty: Ty, name: &str) -> Option<Ty> {
        match self.types.kind(ty) {
            TyKind::Handle(Handle::Ref, value) if name == "value" => Some(*value),
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => fields.iter().find(|f| f.name == name).map(|f| f.ty),
                AdtKind::Newtype(inner) if name == "value" => Some(*inner),
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolves an assignment target to a `var` binding and a field path.
    fn place(&mut self, cx: &mut FnCx, target: &ast::Expr) -> Option<(LocalId, Vec<hir::PlaceStep>, Ty)> {
        let (base, field) = match &target.kind {
            ast::ExprKind::Path(segments) => {
                let root = &segments[0];
                // For an editor, the binding written is a reference too.
                self.index_local(cx, root);
                if self.lookup(cx, &root.name).is_none()
                    && let Some(captured) = self.capture(cx, &root.name, root.span)
                {
                    // Assigning a capture changes the closure's own copy (11.8).
                    let hir::ExprKind::Field(_, index) = captured.kind else { unreachable!() };
                    if !cx.captures[index as usize].mutable {
                        let message = format!(
                            "cannot assign to `{0}`: it is not declared with `var`; declare it as `var {0}`",
                            root.name
                        );
                        self.diags.error("E1411", root.span, message);
                    }
                    cx.mutates = true;
                    let mut ty = captured.ty;
                    let mut path = vec![hir::PlaceStep::Field(index)];
                    for segment in &segments[1..] {
                        let (index, field_ty) = self.field_of(ty, FieldRef::Name(&segment.name), segment.span)?;
                        path.extend(index.map(hir::PlaceStep::Field));
                        ty = field_ty;
                    }
                    return Some((cx.env.unwrap(), path, ty));
                }
                let Some(local) = self.lookup(cx, &root.name) else {
                    let known = self.names.contains_key(&root.name);
                    let (code, message) = if known {
                        ("E1411", format!("`{}` is not a mutable binding", root.name))
                    } else {
                        ("E1001", format!("unknown name `{}`", root.name))
                    };
                    self.diags.error(code, root.span, message);
                    return None;
                };
                // Writing a loan is what it is for.
                if matches!(cx.params[local.0 as usize], Some(ast::Mode::Mut | ast::Mode::Var)) {
                    cx.reads.insert(local);
                }
                if !cx.mutable[local.0 as usize] {
                    let ty = self.types.display(cx.locals[local.0 as usize].ty);
                    let message = match cx.params[local.0 as usize] {
                        Some(ast::Mode::Ref) if cx.loan_bindings.contains(&local) => format!(
                            "cannot assign to `{0}`: it is a `ref` binding that only observes its part; borrow it with `mut {0}` in a `for` header",
                            root.name
                        ),
                        Some(ast::Mode::Ref) if root.name == "self" => {
                            "cannot assign to `self`: it is a shared loan; take it as `mut self` to change the caller's value".to_string()
                        }
                        Some(ast::Mode::Ref) => format!(
                            "cannot assign to `{0}`: it is a `ref` loan; take it as `{0}: mut {ty}` to change the caller's value",
                            root.name
                        ),
                        Some(_) => format!(
                            "cannot assign to `{0}`: parameters are immutable; take it as `{0}: mut {ty}` to change the caller's value, or copy it into a `var`",
                            root.name
                        ),
                        None => format!(
                            "cannot assign to `{0}`: it is not declared with `var`; declare it as `var {0}`",
                            root.name
                        ),
                    };
                    self.diags.error("E1411", root.span, message);
                }
                let mut ty = cx.locals[local.0 as usize].ty;
                let mut path = Vec::new();
                for segment in &segments[1..] {
                    let (index, field_ty) = self.field_of(ty, FieldRef::Name(&segment.name), segment.span)?;
                    path.extend(index.map(hir::PlaceStep::Field));
                    ty = field_ty;
                }
                return Some((local, path, ty));
            }
            ast::ExprKind::Index { base, index } => {
                let (local, path, ty) = self.place(cx, base)?;
                return self.index_place(cx, (local, path, ty), index, base.span);
            }
            ast::ExprKind::Field(base, name) => (base, (FieldRef::Name(&name.name), name.span)),
            ast::ExprKind::TupleSlot(base, slot) => (base, (FieldRef::Slot(*slot), target.span)),
            _ => {
                self.diags.error("E1411", target.span, "this expression cannot be assigned to");
                return None;
            }
        };
        let (local, mut path, ty) = self.place(cx, base)?;
        let (index, field_ty) = self.field_of(ty, field.0, field.1)?;
        path.extend(index.map(hir::PlaceStep::Field));
        Some((local, path, field_ty))
    }

    /// An exclusive loan of a call must not overlap its other loans (16.3,
    /// 16.4): a provable overlap is `E1403`; indices and regions known only
    /// at run time are checked then (`P0004`) between exclusive loans.
    /// `args` tells which arguments are shared `ref` loans.
    fn check_loan_overlap<'e>(&mut self, args: impl Iterator<Item = (&'e hir::Expr, bool)>, span: Span) {
        // Each loan, whether it lends a region of its place, and whether it
        // is exclusive.
        let loans: Vec<(hir::Place, bool, bool)> = args
            .filter_map(|(a, shared)| match &a.kind {
                hir::ExprKind::Loan(place) => Some((place.clone(), false, true)),
                hir::ExprKind::Region(place, _) => Some((place.clone(), true, true)),
                hir::ExprKind::Slice(base, _) if shared => Some((hir::Place::of((**base).clone())?, true, false)),
                _ if shared => Some((hir::Place::of(a.clone())?, false, false)),
                _ => None,
            })
            .collect();
        for (index, (first, first_region, first_exclusive)) in loans.iter().enumerate() {
            for (second, second_region, second_exclusive) in &loans[index + 1..] {
                let (first_region, second_region) = (*first_region, *second_region);
                if first.local != second.local || !first_exclusive && !second_exclusive {
                    continue;
                }
                // The first step where the places part, if any, tells them
                // apart: different fields, or indices that are checked when
                // the call runs unless they are the same constant.
                let parted = first.path.iter().zip(&second.path).find_map(|steps| match steps {
                    (hir::PlaceStep::Field(x), hir::PlaceStep::Field(y)) => (x != y).then_some(false),
                    (hir::PlaceStep::Index(x), hir::PlaceStep::Index(y)) => match (&x.kind, &y.kind) {
                        (hir::ExprKind::Int(i), hir::ExprKind::Int(j)) if i == j => None,
                        _ => Some(false),
                    },
                    _ => None,
                });
                // Otherwise one place holds the other, which only a region
                // may leave disjoint, decided when the call runs.
                let overlap = parted.unwrap_or(match first.path.len().cmp(&second.path.len()) {
                    std::cmp::Ordering::Equal => !(first_region && second_region),
                    std::cmp::Ordering::Less => !first_region,
                    std::cmp::Ordering::Greater => !second_region,
                });
                if overlap {
                    let message = if *first_exclusive && *second_exclusive {
                        "two exclusive loans of one call overlap; lend disjoint parts, or one at a time"
                    } else {
                        "a `ref` loan overlaps an exclusive loan of the same call; copy the value it observes into a binding first"
                    };
                    self.diags.error("E1403", span, message);
                    return;
                }
            }
        }
    }

    /// The element `base[index]` of the array or deque place `base`.
    fn index_place(
        &mut self,
        cx: &mut FnCx,
        base: (LocalId, Vec<hir::PlaceStep>, Ty),
        index: &ast::Expr,
        base_span: Span,
    ) -> Option<(LocalId, Vec<hir::PlaceStep>, Ty)> {
        let (local, mut path, ty) = base;
        let (TyKind::Array(element) | TyKind::Deque(element) | TyKind::CArray(element, _)) = *self.types.kind(ty)
        else {
            let message = if ty == Ty::STRING {
                "a `String` is immutable: build a new one, for example with interpolation".to_string()
            } else {
                format!("{} cannot be indexed", self.display(ty))
            };
            self.diags.error("E1102", base_span, message);
            return None;
        };
        let index = self.expr(cx, index, Some(Ty::INT));
        path.push(hir::PlaceStep::Index(self.coerce(index, Ty::INT)));
        Some((local, path, element))
    }

    /// Whether every value of `ty` has the same structural extent, so a
    /// `mut` loan may replace it whole (16.3).
    fn fixed_extent(&self, ty: Ty) -> bool {
        !matches!(
            self.types.kind(self.types.repr(ty)),
            TyKind::Array(_)
                | TyKind::Deque(_)
                | TyKind::Map(..)
                | TyKind::Set(_)
                | TyKind::Param(_)
                | TyKind::Opaque(..)
        )
    }

    /// The value of `target op= value` for an arithmetic operator; the result
    /// of `target op value` must keep the target's type.
    fn compound_value(&mut self, cx: &mut FnCx, op: BinOp, target: Ty, value: &ast::Expr, span: Span) -> hir::Expr {
        let expected = self.arithmetic_expected(value, target).or(Some(target));
        let value = self.expr(cx, value, expected);
        let value = if self.number_leaf(target).1 == 0 && self.number_leaf(value.ty).1 == 0 {
            self.coerce(value, target)
        } else {
            value
        };
        let result = self.arithmetic(op, target, value.ty, span);
        if result != target && result != Ty::ERROR && target != Ty::ERROR {
            let message = format!(
                "`{0}=` would change the target from {1} to {2}; bind the result instead: `let result = target {0} value`",
                op.as_str(),
                self.display(target),
                self.display(result),
            );
            self.diags.error("E1102", span, message);
        }
        value
    }

    /// `values[start:end:step] = value` and its compound forms (10.5).
    fn assign_slice(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        bounds: [&Option<Box<ast::Expr>>; 3],
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        let (local, path, ty) = self.place(cx, base)?;
        self.check_lent_write(cx, local, &path, span);
        if !matches!(self.types.kind(ty), TyKind::Array(_)) {
            let message = if ty == Ty::STRING {
                "a `String` is immutable: build a new one, for example with interpolation".to_string()
            } else {
                format!("{} cannot be sliced", self.display(ty))
            };
            self.diags.error("E1102", base.span, message);
            return None;
        }
        let bounds = bounds.map(|bound| {
            bound.as_ref().map(|b| {
                let value = self.expr(cx, b, Some(Ty::INT));
                self.coerce(value, Ty::INT)
            })
        });
        let value = match op {
            None => {
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem)) => {
                self.compound_value(cx, op, ty, value, span)
            }
            Some(op) => {
                let message = format!("`{}=` cannot be applied to a slice", op.as_str());
                self.diags.error("E1102", span, message);
                return None;
            }
        };
        Some(hir::Stmt::AssignSlice { local, path, bounds, op, value, span })
    }

    /// The position and type of a field. The position is `None` for a
    /// newtype's `.value`, which shares the newtype's representation.
    fn field_of(&mut self, ty: Ty, field: FieldRef, span: Span) -> Option<(Option<u32>, Ty)> {
        if let (FieldRef::Name(name), Some(hidden)) = (field, self.hidden_fields(ty))
            && let Some(index) = self.types.components(ty).and(self.field_index(ty, name))
            && hidden[index]
        {
            let message = format!("field `{name}` of {} is private to its module", self.display(ty));
            self.diags.error("E1501", span, message);
        }
        if let FieldRef::Name(name) = field {
            self.index_field(ty, name, span);
        }
        let found = match (self.types.kind(ty), field) {
            (TyKind::Error, _) => return None,
            (TyKind::Tuple(items), FieldRef::Slot(slot)) => items.get(slot as usize).map(|&t| (Some(slot), t)),
            (TyKind::Adt(id), FieldRef::Name(name)) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => {
                    fields.iter().position(|f| f.name == name).map(|i| (Some(i as u32), fields[i].ty))
                }
                AdtKind::Newtype(inner) => (name == "value").then_some((None, *inner)),
                AdtKind::Enum(_) => None,
            },
            _ => None,
        };
        if found.is_none() {
            let what = match field {
                FieldRef::Name(name) => format!("field `{name}`"),
                FieldRef::Slot(slot) => format!("element {slot}"),
            };
            let mut message = format!("{} has no {what}", self.display(ty));
            if let FieldRef::Name(name) = field {
                let fields: Vec<String> = match self.types.kind(ty) {
                    TyKind::Adt(id) => match &self.types.adt(*id).kind {
                        AdtKind::Record(fields) => fields.iter().map(|f| f.name.clone()).collect(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                // `values.length`: a method read as a field.
                let intrinsic = ["length", "isEmpty", "byteLength", "keys", "values", "entries", "chars", "bytes"];
                let key = (self.owner_key(ty), name.to_string());
                if intrinsic.contains(&name) || self.methods.contains_key(&key) {
                    message.push_str(&format!("; `{name}` is a method: call it, `value.{name}()`"));
                } else if let TyKind::Option(inner) = *self.types.kind(ty) {
                    let inner = self.display(inner);
                    message.push_str(&format!(
                        "; it may be `none`: reach the {inner} inside with `match`, `?`, or `.map((value) {{ value.{name} }})`"
                    ));
                } else if let Some(near) = closest(name, fields.iter().map(String::as_str)) {
                    message.push_str(&format!("; did you mean `{near}`?"));
                } else if !fields.is_empty() {
                    message.push_str(&format!("; its fields are {}", listed(&fields)));
                }
            }
            self.diags.error("E1102", span, message);
            self.offer_meant(span);
        }
        found
    }

    /// Who declares a nominal type, if the program does.
    fn owner(&self, ty: Ty) -> Option<&Owner> {
        let name = match self.types.instance_of(ty) {
            Some((template, _)) => &self.types.template(template).name,
            None => match self.types.kind(ty) {
                TyKind::Adt(id) => &self.types.adt(*id).name,
                _ => return None,
            },
        };
        self.owners.get(name)
    }

    /// Which fields of a record are private, when it is declared in another
    /// module and has some (7.6).
    fn hidden_fields(&self, ty: Ty) -> Option<&[bool]> {
        let owner = self.owner(ty)?;
        (owner.module != self.module && owner.private_fields.contains(&true)).then_some(&owner.private_fields)
    }

    fn field_index(&self, ty: Ty, name: &str) -> Option<usize> {
        let TyKind::Adt(id) = self.types.kind(ty) else { return None };
        match &self.types.adt(*id).kind {
            AdtKind::Record(fields) => fields.iter().position(|f| f.name == name),
            _ => None,
        }
    }

    /// A type that `ty` mentions and that is private to its module.
    fn private_in(&self, ty: Ty) -> Option<Ty> {
        if self.owner(ty).is_some_and(|o| !o.public) {
            return Some(ty);
        }
        let parts: Vec<Ty> = match self.types.kind(ty) {
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => vec![*t],
            TyKind::Map(a, b) | TyKind::Result(a, b) => vec![*a, *b],
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            TyKind::Fn(params, result, ..) => params.iter().copied().chain([*result]).collect(),
            _ => self.types.instance_of(ty).map_or(Vec::new(), |(_, args)| args.to_vec()),
        };
        parts.into_iter().find_map(|t| self.private_in(t))
    }

    /// A `pub` function or operation exposes only `pub` types (7.9).
    fn check_public_api(&mut self, params: &[Ty], result: Ty, f: &ast::FnDecl) {
        let exposed = params.iter().chain([&result]).find_map(|&t| self.private_in(t));
        if let Some(private) = exposed {
            let (shown, name) = (self.display(private), &f.name.name);
            let message = if f.owner.is_some() && params.first() == Some(&private) {
                format!(
                    "`pub` operation `{name}` belongs to {shown}, which is private; declare the type `pub`, or the operation private"
                )
            } else {
                format!("`pub fn {name}` exposes {shown}, which is private; declare it `pub`, or the function private")
            };
            self.diags.error("E1503", f.name.span, message);
        }
    }

    /// Reports a call of an inherent operation that is private to another
    /// module (7.8).
    fn check_method_visible(&mut self, id: FnId, span: Span) {
        if let Some(&(module, public)) = self.method_visibility.get(&id)
            && module != self.module
            && !public
        {
            let name = self.signatures[id.0 as usize].name.clone();
            let message = format!("`{name}` is private to its module; declare it `pub` to use it here");
            self.diags.error("E1501", span, message);
        }
    }

    fn for_loop(&mut self, cx: &mut FnCx, header: &ast::ForHeader, body: &ast::Block) -> Option<hir::Stmt> {
        match header {
            ast::ForHeader::Infinite => {
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                let saw_break = cx.loops.pop().unwrap();
                cx.diverged |= !saw_break;
                Some(hir::Stmt::Loop { condition: None, body })
            }
            ast::ForHeader::Condition(condition) => {
                let condition = self.expr(cx, condition, Some(Ty::BOOL));
                let condition = self.coerce(condition, Ty::BOOL);
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                Some(hir::Stmt::Loop { condition: Some(condition), body })
            }
            ast::ForHeader::In { pattern, source } => {
                let ast::ExprKind::Binary(op @ (BinOp::Range | BinOp::RangeInclusive), start, end) = &source.kind
                else {
                    let source = self.expr(cx, source, None);
                    // A shared collection is iterated through a snapshot.
                    let source = match *self.types.kind(source.ty) {
                        TyKind::Handle(handle, _) if handle.collection() => {
                            if loan_kind(pattern).is_some() {
                                let message = "another task may change a shared collection, so a `for` over it binds values: remove `ref`, `mut` or `var`";
                                self.diags.error("E1206", pattern.span, message);
                                return None;
                            }
                            let (span, state) = (source.span, self.types.handle_result(source.ty));
                            let snapshot = hir::TaskOp::Shared(hir::SharedOp::Snapshot);
                            Self::mk(hir::ExprKind::TaskOp(snapshot, vec![source]), state, span)
                        }
                        _ => source,
                    };
                    let element = match *self.types.kind(source.ty) {
                        TyKind::Handle(Handle::Receiver, element) => {
                            return Some(self.for_receiver(cx, pattern, source, element, body));
                        }
                        TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => element,
                        TyKind::String => Ty::CHAR,
                        // A map yields its entries as `(key, value)` (10.13).
                        TyKind::Map(key, value) => self.types.intern(TyKind::Tuple(vec![key, value])),
                        TyKind::Error => Ty::ERROR,
                        _ => match self.iterator_element(source.ty) {
                            Some(element) => return Some(self.for_cursor(cx, pattern, source, element, body)),
                            None => {
                                let message = format!(
                                    "{} cannot be iterated: implement `Iterator[T]` for it, or iterate a collection",
                                    self.display(source.ty)
                                );
                                self.diags.error("E1206", source.span, message);
                                return None;
                            }
                        },
                    };
                    if let Some(exclusive) = loan_kind(pattern) {
                        return self.borrowed_for(cx, pattern, source, element, body, exclusive);
                    }
                    cx.scopes.open();
                    let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
                    let pattern = self.pattern(cx, pattern, element, context);
                    if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
                        let message = format!("this pattern can fail, for example on `{missing}`");
                        self.diags.error("E1201", pattern.span, message);
                    }
                    cx.loops.push(false);
                    let body = self.block(cx, body, Some(Ty::UNIT));
                    cx.loops.pop();
                    cx.scopes.close();
                    return Some(hir::Stmt::ForEach { pattern, source, body });
                };
                let (start, end) = self.operands(cx, start, end, None, false);
                // Characters count through the prelude's `Range`.
                if start.ty == Ty::CHAR {
                    let range = self.range_value(start, end, *op == BinOp::RangeInclusive, source.span);
                    return Some(self.for_cursor(cx, pattern, range, Ty::CHAR, body));
                }
                self.require_int(&start);
                cx.scopes.open();
                let local = match &pattern.kind {
                    ast::PatternKind::Binding(binding) => self.declare(cx, binding, start.ty, false),
                    ast::PatternKind::Wildcard => self.hidden_local(cx, start.ty),
                    ast::PatternKind::Loan(..) => {
                        let message = "a range yields integers, which are not borrowed; bind them by value";
                        self.diags.error("E1206", pattern.span, message);
                        self.hidden_local(cx, start.ty)
                    }
                    _ => {
                        self.diags.error("E1206", pattern.span, "a range yields integers; bind them to a name");
                        self.hidden_local(cx, start.ty)
                    }
                };
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                cx.scopes.close();
                Some(hir::Stmt::ForRange { local, start, end, inclusive: *op == BinOp::RangeInclusive, body })
            }
        }
    }

    /// A `for` whose pattern borrows the elements of a collection (13.3):
    /// `ref` bindings observe them, and `mut` or `var` bindings change them
    /// in place. The collection must be a place, and stays lent for the
    /// whole loop: exclusively when anything is borrowed with `mut` or `var`.
    fn borrowed_for(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
        exclusive: bool,
    ) -> Option<hir::Stmt> {
        let (ty, span) = (source.ty, source.span);
        let fits = match self.types.kind(ty) {
            TyKind::Array(_) | TyKind::Map(..) => true,
            TyKind::Set(_) => !exclusive,
            TyKind::Error => return None,
            _ => false,
        };
        if !fits {
            let message = match self.types.kind(ty) {
                TyKind::Set(_) => "the elements of a set are its keys and cannot change; borrow them with `ref`".into(),
                _ if exclusive => format!(
                    "`mut` and `var` bindings change the elements of an array or map in place, but {} has no such elements",
                    self.display(ty)
                ),
                _ => format!(
                    "`ref` bindings borrow the elements of an array, map or set, but {} produces values; bind them by value",
                    self.display(ty)
                ),
            };
            self.diags.error("E1206", span, message);
            return None;
        }
        // The collection is evaluated once: a place with fixed indices.
        let place = if exclusive {
            self.loan_place(cx, source, "lending its elements as `mut` or `var`", false)?
        } else {
            let Some(place) = hir::Place::of(source) else {
                let message = "a `for` with `ref` bindings borrows its collection, which must be a binding or part of one; bind it first";
                self.diags.error("E1206", span, message);
                return None;
            };
            place
        };
        let mut stmts = Vec::new();
        let mut path = Vec::new();
        for step in place.path {
            path.push(match step {
                hir::PlaceStep::Index(index)
                    if !matches!(index.kind, hir::ExprKind::Local(_) | hir::ExprKind::Int(_)) =>
                {
                    let local = self.hidden_local(cx, Ty::INT);
                    let read = Self::mk(hir::ExprKind::Local(local), Ty::INT, index.span);
                    stmts.push(hir::Stmt::Let(local, index));
                    hir::PlaceStep::Index(read)
                }
                step => step,
            });
        }
        let place = hir::Place { local: place.local, path };
        let read = self.place_read(cx, &place, span);

        cx.scopes.open();
        let context = PatternContext { in_match: false, mutable: false, loans: Loans::Any };
        let pattern = self.pattern(cx, pattern, element, context);
        if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
            let message = format!("this pattern can fail, for example on `{missing}`");
            self.diags.error("E1201", pattern.span, message);
        }
        let map = matches!(self.types.kind(ty), TyKind::Map(..));
        self.borrowed_bindings(cx, &pattern, map, false);
        cx.lent.push((place.clone(), exclusive));
        cx.loops.push(false);
        let mut body = self.block(cx, body, Some(Ty::UNIT));
        cx.loops.pop();
        cx.lent.pop();
        cx.scopes.close();
        if !exclusive {
            stmts.push(hir::Stmt::ForEach { pattern, source: read, body });
            return Some(hir::Stmt::Expr(Self::mk(
                hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }),
                Ty::UNIT,
                span,
            )));
        }
        // Each iteration binds the parts of the element at the next
        // position; the length cannot change while the loop runs.
        let (position, count) = (self.hidden_local(cx, Ty::INT), self.hidden_local(cx, Ty::INT));
        let at = Self::mk(hir::ExprKind::Local(position), Ty::INT, span);
        let mut element_place = place.clone();
        element_place.path.push(hir::PlaceStep::Index(at));
        let mut lends = Vec::new();
        self.lends(&pattern, element_place, &mut lends);
        lends.append(&mut body.stmts);
        body.stmts = lends;
        let length = Self::mk(hir::ExprKind::Length(Box::new(read)), Ty::INT, span);
        stmts.push(hir::Stmt::Let(count, length));
        stmts.push(hir::Stmt::ForRange {
            local: position,
            start: Self::mk(hir::ExprKind::Int(0), Ty::INT, span),
            end: Self::mk(hir::ExprKind::Local(count), Ty::INT, span),
            inclusive: false,
            body,
        });
        Some(hir::Stmt::Expr(Self::mk(
            hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }),
            Ty::UNIT,
            span,
        )))
    }

    /// Checks the bindings of a borrowed `for` header: one by value copies
    /// its part, which must be `Copy`, and only the value of a map entry
    /// (`entry.1`) can be borrowed with `mut` or `var`.
    fn borrowed_bindings(&mut self, cx: &FnCx, pattern: &hir::Pattern, map: bool, in_value: bool) {
        match &pattern.kind {
            hir::PatKind::Binding(local) => {
                let name = &cx.locals[local.0 as usize].name;
                match cx.params[local.0 as usize] {
                    None if !self.types.is_copy(pattern.ty) => {
                        let message = format!(
                            "`{name}` would copy its part of type {}, which is not `Copy`; borrow it with `ref {name}`",
                            self.display(pattern.ty)
                        );
                        self.diags.error("E1406", pattern.span, message);
                    }
                    Some(ast::Mode::Mut | ast::Mode::Var) if map && !in_value => {
                        let message = "only the value of a map entry can change: write `(key, mut value)`";
                        self.diags.error("E1206", pattern.span, message);
                    }
                    _ => {}
                }
            }
            hir::PatKind::Fields(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.borrowed_bindings(cx, item, map, in_value || map && index == 1);
                }
            }
            hir::PatKind::Newtype(inner) => self.borrowed_bindings(cx, inner, map, in_value),
            hir::PatKind::Variant(_, items) | hir::PatKind::Array { items, .. } => {
                items.iter().for_each(|item| self.borrowed_bindings(cx, item, map, in_value))
            }
            hir::PatKind::Wildcard | hir::PatKind::Literal(_) => {}
        }
    }

    /// The statements that bind each binding of a borrowed `for` header to
    /// its part of the element at `place`.
    fn lends(&mut self, pattern: &hir::Pattern, place: hir::Place, out: &mut Vec<hir::Stmt>) {
        match &pattern.kind {
            hir::PatKind::Binding(local) => out.push(hir::Stmt::Lend { local: *local, place }),
            hir::PatKind::Fields(items) => {
                for (index, item) in items.iter().enumerate() {
                    let mut part = place.clone();
                    part.path.push(hir::PlaceStep::Field(index as u32));
                    self.lends(item, part, out);
                }
            }
            hir::PatKind::Newtype(inner) => self.lends(inner, place, out),
            hir::PatKind::Wildcard | hir::PatKind::Literal(_) => {}
            hir::PatKind::Variant(..) | hir::PatKind::Array { .. } => {
                self.diags.unsupported(pattern.span, "borrowing parts of an enum or array element in a `for` header");
            }
        }
    }

    /// Reports a write to a place that overlaps a collection lent to the
    /// bindings of an enclosing `for`.
    fn check_lent_write(&mut self, cx: &FnCx, local: LocalId, path: &[hir::PlaceStep], span: Span) {
        let place = hir::Place { local, path: path.to_vec() };
        if let Some((lent, exclusive)) = lent_conflict(cx, &place) {
            let lent = self.place_name(cx, lent);
            self.lent_error(&lent, exclusive, span);
        }
    }

    /// A place as written, such as `self.items`.
    fn place_name(&self, cx: &FnCx, place: &hir::Place) -> String {
        let mut ty = cx.locals[place.local.0 as usize].ty;
        let mut name = cx.locals[place.local.0 as usize].name.clone();
        for step in &place.path {
            match step {
                hir::PlaceStep::Field(index) => {
                    let field = match self.types.kind(self.types.repr(ty)) {
                        TyKind::Adt(id) => match &self.types.adt(*id).kind {
                            AdtKind::Record(fields) => fields[*index as usize].name.clone(),
                            _ => index.to_string(),
                        },
                        _ => index.to_string(),
                    };
                    name = format!("{name}.{field}");
                    ty = self.types.components(ty).map_or(Ty::ERROR, |c| c[*index as usize]);
                }
                hir::PlaceStep::Index(_) => {
                    name.push_str("[..]");
                    ty = match *self.types.kind(self.types.repr(ty)) {
                        TyKind::Array(element) | TyKind::Deque(element) => element,
                        _ => Ty::ERROR,
                    };
                }
            }
        }
        name
    }

    /// Reports a use of a collection that the bindings of an enclosing
    /// `for` borrow (13.3): any use while they borrow it exclusively, and a
    /// change while they observe it.
    fn lent_error(&mut self, name: &str, exclusive: bool, span: Span) {
        let message = if exclusive {
            format!(
                "`{name}` is lent to the `mut` or `var` bindings of the loop until it ends; reach its elements through them"
            )
        } else {
            format!("`{name}` is lent to the `ref` bindings of the loop until it ends, so it cannot change inside it")
        };
        self.diags.error("E1403", span, message);
    }

    /// An expression reading a place.
    fn place_read(&mut self, cx: &FnCx, place: &hir::Place, span: Span) -> hir::Expr {
        let ty = cx.locals[place.local.0 as usize].ty;
        let mut value = Self::mk(hir::ExprKind::Local(place.local), ty, span);
        for step in &place.path {
            value = match step {
                hir::PlaceStep::Field(index) => {
                    let ty = self.types.components(value.ty).map_or(Ty::ERROR, |c| c[*index as usize]);
                    Self::mk(hir::ExprKind::Field(Box::new(value), *index), ty, span)
                }
                hir::PlaceStep::Index(index) => {
                    let ty = match *self.types.kind(value.ty) {
                        TyKind::Array(element) | TyKind::Deque(element) => element,
                        _ => Ty::ERROR,
                    };
                    Self::mk(hir::ExprKind::Index(Box::new(value), Box::new(index.clone())), ty, span)
                }
            };
        }
        value
    }

    // ---- expressions -----------------------------------------------------

    fn mk(kind: hir::ExprKind, ty: Ty, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
    }

    /// The part of a type that is not `Send`, if any: a `Pointer`, or a
    /// value holding one.
    fn not_send(&self, ty: Ty) -> Option<Ty> {
        if self.types.is_send(ty) {
            return None;
        }
        let repr = self.types.repr(ty);
        let parts = match self.types.kind(repr) {
            TyKind::Pointer(_) => return Some(ty),
            TyKind::Handle(_, value) => vec![*value],
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) | TyKind::CArray(t, _) => vec![*t],
            TyKind::Map(a, b) | TyKind::Result(a, b) => vec![*a, *b],
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            _ => match (self.types.components(repr), self.types.variants(repr)) {
                (Some(items), _) => items,
                (None, Some(variants)) => variants.concat(),
                (None, None) => Vec::new(),
            },
        };
        parts.into_iter().find_map(|part| self.not_send(part)).or(Some(ty))
    }

    /// Whether a value may be a failure or an absence the program must see
    /// (11.9): an option, or a result that can fail.
    fn must_use(&self, ty: Ty) -> bool {
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Result(_, error) => *error != Ty::NEVER,
            TyKind::Option(_) => true,
            _ => false,
        }
    }

    fn error_expr(span: Span) -> hir::Expr {
        Self::mk(hir::ExprKind::Error, Ty::ERROR, span)
    }

    /// Reports a mismatch unless `value` already has (or diverges into)
    /// `expected`. A `T` where `T?` is expected becomes `some(T)` (8.9).
    fn coerce(&mut self, value: hir::Expr, expected: Ty) -> hir::Expr {
        if value.ty == expected {
            return value;
        }
        // Where nothing is expected, a value is dropped, unless it is a
        // result or an option the program has to see (11.9).
        if expected == Ty::UNIT && value.ty != Ty::NEVER && !self.types.has_error(value.ty) {
            if self.must_use(value.ty) {
                let message = match value.kind {
                    hir::ExprKind::Recover(..) => format!(
                        "what `recover` gives, of type {}, is not used; read it with `match`, `.unwrapOr(...)` or `.isErr()`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    ),
                    _ => format!(
                        "a value of type {} is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    ),
                };
                self.diags.error("E1303", value.span, message);
                let start = Span::new(value.span.start, value.span.start);
                self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
                return value;
            }
            let span = value.span;
            let block = hir::Block { stmts: vec![hir::Stmt::Expr(value)], tail: None, ty: Ty::UNIT };
            return Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span);
        }
        // A closure becomes a `fn(...)` value when it never changes its
        // captures (11.8).
        if let TyKind::Fn(..) = self.types.kind(expected)
            && let Some((body, _, info)) = self.types.closure(value.ty)
        {
            let found =
                self.types.intern(TyKind::Fn(info.params, info.result, info.variadic, info.modes, info.is_unsafe));
            // Any task may call a `fn(...)` value (16.13).
            if found == expected
                && !info.mutates
                && let Some(part) = self.not_send(value.ty)
            {
                let message = format!(
                    "this closure captures {}, which is not `Send`, so it cannot become {}, which any task may call; call the closure where it is instead",
                    self.display(part),
                    self.display(expected)
                );
                self.diags.error("E1605", value.span, message);
                return value;
            }
            if found == expected && info.once {
                let message = format!(
                    "this closure gives a capture away, so it can be called once and cannot become {}; take it as a type parameter bound by `CallOnce[{}]`",
                    self.display(expected),
                    self.types.display(expected)
                );
                self.diags.error("E1108", value.span, message);
                return value;
            }
            if found == expected && !info.mutates {
                let span = value.span;
                return Self::mk(hir::ExprKind::Erase(Box::new(value), FnId(body)), expected, span);
            }
            if found == expected {
                let message = format!(
                    "this closure changes its captures, so it cannot become {}; take it as a type parameter bound by `CallMut[{}]`",
                    self.display(expected),
                    self.types.display(expected)
                );
                self.diags.error("E1108", value.span, message);
                return value;
            }
        }
        // A `String` passes to C as a copy that ends with a zero byte.
        if self.c_arguments
            && value.ty == Ty::STRING
            && (expected == self.c_string || self.types.lookup(&TyKind::Option(self.c_string)) == Some(expected))
        {
            return value;
        }
        // An integer constant's value is checked like a literal's.
        if let hir::ExprKind::Int(number) = value.kind
            && self.const_uses.contains(&value.span)
            && let Some(int) = self.types.as_int(expected)
        {
            let from = self.types.as_int(self.types.repr(value.ty)).unwrap_or(IntTy::I64);
            if int.holds_from(from, number) {
                return Self::mk(hir::ExprKind::Int(number), expected, value.span);
            }
            let number = from.show(number);
            let message = format!("the constant's value {number} does not fit in {}", self.display(expected));
            self.diags.error("E1102", value.span, message);
            return value;
        }
        let value = match self.implicit(value, expected) {
            Ok(converted) => return converted,
            Err(value) => value,
        };
        if let TyKind::CFn(params, result, variadic) = self.types.kind(expected).clone() {
            return self.callback(value, (params, result, variadic), expected);
        }
        // A value of a member's exact type is injected into a union (9.5).
        if let Some(member) = self.types.members(expected).and_then(|m| m.iter().position(|&t| t == value.ty)) {
            let span = value.span;
            return Self::mk(hir::ExprKind::Variant(member as u32, vec![(0, value)]), expected, span);
        }
        if let TyKind::Option(inner) = *self.types.kind(expected)
            && (value.ty == inner
                || self.types.members(inner).is_some_and(|m| m.contains(&value.ty))
                || self.converts(value.ty, inner)
                || matches!(self.types.kind(inner), TyKind::CFn(..))
                    && matches!(self.types.kind(value.ty), TyKind::Fn(..) | TyKind::Adt(_))
                || matches!(self.types.kind(inner), TyKind::Fn(..)) && self.types.closure(value.ty).is_some())
        {
            let span = value.span;
            let value = self.coerce(value, inner);
            return Self::mk(hir::ExprKind::Variant(SOME, vec![(0, value)]), expected, span);
        }
        if value.ty != expected
            && value.ty != Ty::NEVER
            && !self.types.has_error(value.ty)
            && !self.types.has_error(expected)
        {
            let wrapped = match *self.types.kind(value.ty) {
                TyKind::Result(ok, _) | TyKind::Option(ok) => ok == expected,
                _ => false,
            };
            let narrowed = self.conversion_checked(value.ty, expected);
            let message = match value.kind {
                hir::ExprKind::Int(literal) if expected == Ty::BYTE && (0..256).contains(&literal) => {
                    format!("an integer literal is not a `Byte`: write `Byte({literal}u8)`")
                }
                // Each closure has a type of its own, which only it has.
                _ if self.types.is_closure(value.ty) && self.types.is_closure(expected) => {
                    let shown = self.types.display(expected);
                    let signature = shown.trim_start_matches("closure ");
                    format!(
                        "each closure is a type of its own, so this one is not the other: give the place the function type `{signature}`, which any closure of that signature fits"
                    )
                }
                _ if narrowed.is_some() && expected != Ty::BYTE && value.ty != Ty::BYTE => {
                    let text = self.sources.slice(value.span);
                    let target = self.types.display(expected);
                    match narrowed {
                        Some(true) => format!(
                            "expected {}, found {}, which may not fit: convert it with `{target}({text})?`, or handle the error with `match`",
                            self.display(expected),
                            self.display(value.ty)
                        ),
                        _ => format!(
                            "expected {}, found {}: convert it with `{target}({text})`",
                            self.display(expected),
                            self.display(value.ty)
                        ),
                    }
                }
                _ if wrapped && matches!(self.types.kind(value.ty), TyKind::Option(_)) => format!(
                    "expected {}, found {}, which may be `none`: give a fallback with `.unwrapOr(value)`, handle it with `match`, or pass it on with `?` in a function that returns an option",
                    self.display(expected),
                    self.display(value.ty)
                ),
                _ if wrapped => format!(
                    "expected {}, found {}: add `?` to pass the failure on, give a fallback with `.unwrapOr(value)`, or handle it with `match`",
                    self.display(expected),
                    self.display(value.ty)
                ),

                _ if self.comparing.is_some() => {
                    let (found, other) = (self.display(value.ty), self.display(expected));
                    let (left, right) = if self.comparing == Some(true) { (found, other) } else { (other, found) };
                    format!("cannot compare {left} with {right}: both sides of a comparison have one type")
                }
                _ if self.panic_datum == Some(expected) => format!(
                    "a panic's data holds an `Int`, a `Float`, a `Bool` or a `String`, not {}: write its text, as `\"{{value}}\"`",
                    self.display(value.ty)
                ),
                _ if expected == Ty::STRING && value.ty == Ty::CHAR => {
                    "expected `String`, found `Char`: interpolate it, as `\"{c}\"`, for a `String`".to_string()
                }
                _ if expected == Ty::CHAR && value.ty == Ty::STRING => {
                    "expected `Char`, found `String`: a `Char` literal is in single quotes, `'a'`".to_string()
                }
                _ if value.ty == Ty::STRING && self.query_type() == Some(expected) => {
                    "expected `sql.Query`, found `String`: a query is a string literal written where a `sql.Query` is expected, whose values are parameters; write the literal here, keep it as `let query: sql.Query = \"...\"`, or, for SQL made while the program runs, `sql.raw(text)`".to_string()
                }
                _ => format!("expected {}, found {}", self.display(expected), self.display(value.ty)),
            };
            self.diags.error("E1102", value.span, message);
        }
        value
    }

    /// The conversions that never lose information and so need no syntax:
    /// a number to a wider one of its kind (8.6), any pointer to a
    /// `Pointer[ffi.Void]`, and between `ffi.CString` and `Pointer[Byte]`.
    fn implicit(&mut self, value: hir::Expr, expected: Ty) -> Result<hir::Expr, hir::Expr> {
        if !self.converts(value.ty, expected) {
            return Err(value);
        }
        let span = value.span;
        let kind = match self.types.kind(expected) {
            TyKind::Int(_) | TyKind::Float(_) => hir::ExprKind::Convert(Box::new(value)),
            TyKind::Array(_) | TyKind::CArray(..) => hir::ExprKind::Fixed(Box::new(value)),
            _ => hir::ExprKind::Retype(Box::new(value)),
        };
        Ok(Self::mk(kind, expected, span))
    }

    /// Whether a value of `from` becomes a `to` implicitly (`implicit`).
    fn converts(&self, from: Ty, to: Ty) -> bool {
        if from == to {
            return false;
        }
        let byte_pointer = self.types.lookup(&TyKind::Pointer(Ty::BYTE));
        match (self.types.kind(from), self.types.kind(to)) {
            (TyKind::Int(a), TyKind::Int(b)) => a.min() >= b.min() && a.max() <= b.max(),
            (TyKind::Float(_), TyKind::Float(_)) => to == Ty::F64,
            (TyKind::Pointer(_), TyKind::Pointer(target)) => *target == self.void,
            (TyKind::Array(a), TyKind::CArray(b, _)) | (TyKind::CArray(a, _), TyKind::Array(b)) => a == b,
            _ if from == self.c_string => {
                Some(to) == byte_pointer || to == self.types.lookup(&TyKind::Pointer(self.void)).unwrap_or(Ty::ERROR)
            }
            _ if to == self.c_string => Some(from) == byte_pointer,
            _ => false,
        }
    }

    /// A named function, or a closure without captures, as the address of
    /// a C function that runs it (FFI): C passes no context, so a function
    /// that needs one takes it through a `Pointer[ffi.Void]`.
    fn callback(
        &mut self,
        value: hir::Expr,
        (params, result, variadic): (Vec<Ty>, Ty, bool),
        expected: Ty,
    ) -> hir::Expr {
        let span = value.span;
        if self.types.has_error(value.ty) || value.ty == Ty::NEVER {
            return value;
        }
        let signature = match self.types.kind(value.ty).clone() {
            TyKind::Fn(p, r, v, modes, _) if matches!(value.kind, hir::ExprKind::FnValue(..)) => {
                Some((p, r, v, modes, None))
            }
            _ => self.types.closure(value.ty).map(|(_, _, info)| {
                let captures = self.types.components(value.ty).map_or(0, |c| c.len());
                (info.params, info.result, info.variadic, info.modes, Some(captures))
            }),
        };
        let Some((p, r, v, modes, captures)) = signature else {
            let message = format!(
                "{} cannot become {}: only a named function or a closure without captures can, since C keeps no captures",
                self.display(value.ty),
                self.display(expected)
            );
            self.diags.error("E1705", span, message);
            return Self::error_expr(span);
        };
        let found = self.types.intern(TyKind::Fn(p.clone(), r, v, modes.clone(), false));
        if variadic || v || p != params || r != result || modes.iter().any(|&m| m != ast::Mode::Value) {
            let message = format!(
                "expected {}, found {}: a function C calls takes and returns exactly its types",
                self.display(expected),
                self.types.display(found)
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        if captures.is_some_and(|c| c > 0) {
            let message = "a closure that C calls cannot capture values, since C keeps no captures; pass what it needs through a `Pointer[ffi.Void]` made with `ffi.context(value)`";
            self.diags.error("E1705", span, message);
            return Self::error_expr(span);
        }
        Self::mk(hir::ExprKind::Callback(Box::new(value), FnId(u32::MAX)), expected, span)
    }

    /// The prelude's `Range[T]` of integers or characters (10.16).
    fn range_type(&mut self, element: Ty, span: Span) -> Ty {
        let Some(&TypeName::Template(range)) = self.type_names.get("prelude.Range") else { return Ty::ERROR };
        if !matches!(
            self.types.kind(element),
            TyKind::Int(_) | TyKind::Char | TyKind::Param(_) | TyKind::Error | TyKind::Never
        ) {
            let message = format!("a range counts through integers or characters, not {}", self.display(element));
            self.diags.error("E1102", span, message);
            return Ty::ERROR;
        }
        self.types.instantiate(range, vec![element])
    }

    /// What a `Range[T]` yields, if `ty` is one.
    fn range_element(&self, ty: Ty) -> Option<Ty> {
        let Some(&TypeName::Template(range)) = self.type_names.get("prelude.Range") else { return None };
        match self.types.instance_of(ty) {
            Some((template, args)) if template == range => args.first().copied(),
            _ => None,
        }
    }

    /// `start..end` or `start..=end` as a value.
    fn range_value(&mut self, start: hir::Expr, end: hir::Expr, inclusive: bool, span: Span) -> hir::Expr {
        let ty = self.range_type(start.ty, span);
        if ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let flag = |value| Self::mk(hir::ExprKind::Bool(value), Ty::BOOL, span);
        let fields = vec![(0, start), (1, end), (2, flag(inclusive)), (3, flag(false))];
        Self::mk(hir::ExprKind::Record(fields), ty, span)
    }

    fn require_int(&mut self, value: &hir::Expr) {
        if self.types.as_int(value.ty).is_none()
            && ![Ty::NEVER, Ty::ERROR].contains(&value.ty)
            && !self.bound_by(value.ty, self.integer)
        {
            let message = format!("expected an integer, found {}", self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
    }

    fn expr(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
        let value = self.expr_kind(cx, e, expected);
        self.index_expr(e.span, value.ty);
        value
    }

    /// For an editor: the type of an expression of the program's own files.
    fn index_expr(&mut self, span: Span, ty: Ty) {
        if self.index.is_some() && ty != Ty::ERROR && !self.sources.file_of(span.start).logical.starts_with("<std>/") {
            self.index_exprs.push((span, ty));
        }
    }

    fn expr_kind(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::Int { value, suffix } => self.int_literal(false, *value, *suffix, expected, span),
            ast::ExprKind::Bool(b) => Self::mk(hir::ExprKind::Bool(*b), Ty::BOOL, span),
            ast::ExprKind::Unit => Self::mk(hir::ExprKind::Unit, Ty::UNIT, span),
            ast::ExprKind::Str(segments) => {
                match expected.and_then(|ty| self.query_type().filter(|&query| query == ty)) {
                    Some(query) => self.query_literal(cx, segments, query, span),
                    None => self.string(cx, segments, span),
                }
            }
            ast::ExprKind::Path(segments) => self.path(cx, segments, expected, span),
            ast::ExprKind::Call { callee, args } => self.call(cx, callee, args, expected, span),
            ast::ExprKind::Unary(op, operand) => self.unary(cx, *op, operand, expected, span),
            ast::ExprKind::Binary(op, left, right) => self.binary(cx, *op, left, right, expected, span),
            ast::ExprKind::If { condition, then, otherwise } => {
                self.if_expr(cx, condition, then, otherwise.as_deref(), expected, span)
            }
            ast::ExprKind::Block(block) => {
                let block = self.block(cx, block, expected);
                let ty = block.ty;
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            ast::ExprKind::Float { text, suffix } => self.float_literal(text, *suffix, false, expected, span),
            ast::ExprKind::BigInt(digits) => self.big_literal(digits, expected, span),
            ast::ExprKind::Char(c) => Self::mk(hir::ExprKind::Char(*c), Ty::CHAR, span),
            ast::ExprKind::Field(base, name) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Name(&name.name), name.span, span)
            }
            ast::ExprKind::TupleSlot(base, slot) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Slot(*slot), span, span)
            }
            ast::ExprKind::Tuple(items) => self.tuple(cx, items, expected, span),
            ast::ExprKind::Array(items) => self.array_literal(cx, items, expected, span),
            ast::ExprKind::Map(pairs) => self.map_literal(cx, pairs, expected, span),
            ast::ExprKind::Set(items) => self.set_literal(cx, items, expected, span),
            ast::ExprKind::Index { base: function, index: argument } if self.specialization(cx, function).is_some() => {
                // `identity[Int]` used as a value (11.7).
                let id = self.specialization(cx, function).unwrap();
                match Self::type_of_expr(argument) {
                    Some(ty) => {
                        let ty = self.resolve_type(&ty);
                        self.fn_value(id, vec![ty], expected, span)
                    }
                    None => {
                        self.diags.error("E1104", argument.span, "type arguments are types, such as `identity[Int]`");
                        Self::error_expr(span)
                    }
                }
            }
            ast::ExprKind::Index { base, index } => {
                let base = self.expr(cx, base, None);
                // `document["key"]` and `document[index]` of a `json.Value`
                // read a member or an element, or `Null` (14.10).
                if let Some(&TypeName::Nominal(json)) = self.type_names.get("json.Value")
                    && base.ty == json
                {
                    let index = self.expr(cx, index, None);
                    let (method, ty) = if index.ty == Ty::STRING { ("field", Ty::STRING) } else { ("item", Ty::INT) };
                    let index = self.coerce(index, ty);
                    let key = self.owner_key(json);
                    let id = self.methods[&(key, method.to_string())];
                    return Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![base, index]), json, span);
                }
                if let TyKind::Map(key, value) = *self.types.kind(base.ty) {
                    // Reading a map entry yields `V?`: absence is normal (10.10).
                    let index = self.expr(cx, index, Some(key));
                    let index = self.coerce(index, key);
                    let ty = self.types.intern(TyKind::Option(value));
                    return Self::mk(hir::ExprKind::MapOp(hir::MapOp::Get, vec![base, index]), ty, span);
                }
                if base.ty == Ty::STRING {
                    let index = self.expr(cx, index, Some(Ty::INT));
                    let index = self.coerce(index, Ty::INT);
                    return Self::mk(hir::ExprKind::StrOp(hir::StrOp::At, vec![base, index]), Ty::CHAR, span);
                }
                let (TyKind::Array(element) | TyKind::Deque(element) | TyKind::CArray(element, _)) =
                    *self.types.kind(base.ty)
                else {
                    return self.not_indexable(base, span);
                };
                let index = self.expr(cx, index, Some(Ty::INT));
                let index = self.coerce(index, Ty::INT);
                Self::mk(hir::ExprKind::Index(Box::new(base), Box::new(index)), element, span)
            }
            ast::ExprKind::Slice { base, start, end, step } => {
                let base = self.expr(cx, base, None);
                if !matches!(self.types.kind(base.ty), TyKind::Array(_) | TyKind::String) {
                    return self.not_indexable(base, span);
                }
                let mut bound = |this: &mut Self, part: &Option<Box<ast::Expr>>| {
                    part.as_ref().map(|p| {
                        let value = this.expr(cx, p, Some(Ty::INT));
                        Box::new(this.coerce(value, Ty::INT))
                    })
                };
                let parts = [bound(self, start), bound(self, end), bound(self, step)];
                let ty = base.ty;
                Self::mk(hir::ExprKind::Slice(Box::new(base), parts), ty, span)
            }
            ast::ExprKind::Generic { base, args } => match &base.kind {
                // `identity[Int]`: a specialized generic function as a value (11.7).
                ast::ExprKind::Path(segments)
                    if segments.len() == 1 && self.lookup(cx, &segments[0].name).is_none() =>
                {
                    match self.names.get(&segments[0].name).copied() {
                        Some(ModuleName::Fn(id)) => {
                            let explicit = args.iter().map(|a| self.resolve_type(a)).collect();
                            self.fn_value(id, explicit, expected, span)
                        }
                        _ => {
                            self.diags.error("E1102", span, "type arguments must be followed by a call");
                            Self::error_expr(span)
                        }
                    }
                }
                _ => {
                    self.diags.error("E1102", span, "type arguments must be followed by a call");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Match { scrutinee, arms } => self.match_expr(cx, scrutinee, arms, expected, span),
            ast::ExprKind::Select(arms) => self.select(cx, arms, expected, span),
            ast::ExprKind::SyncLiteral(path, literal) => self.sync_literal(cx, path, literal, expected, span),
            ast::ExprKind::Record { path, type_args, fields } => {
                self.record_literal(cx, path, type_args, fields, expected, span)
            }
            ast::ExprKind::With { base, updates } => self.with_expr(cx, base, updates, span),
            ast::ExprKind::Try(inner) => self.try_expr(cx, inner, expected, span),
            ast::ExprKind::Closure(closure) => self.closure(cx, closure, expected, span),
            ast::ExprKind::Recover(block) => self.recover(cx, block, expected, span),
            ast::ExprKind::TryBlock(block) => self.try_block(cx, block, expected, span),
            ast::ExprKind::Within(limit, block) => self.within(cx, limit, block, span),
            // The text of `template.html` or `template.text`, which the call
            // compiles (standard library 14.31).
            ast::ExprKind::Template(_) => Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span),
            ast::ExprKind::TemplateWrite { value, format, place } => {
                self.template_write(cx, value, format.as_ref(), *place, span)
            }
            ast::ExprKind::TemplateInclude { data, named, body } => {
                self.template_include(cx, data.as_deref(), named, body, span)
            }
            ast::ExprKind::Unsafe(block) => {
                self.meta_denied(span, "`unsafe`");
                let outer = std::mem::replace(&mut cx.unsafe_region, true);
                let block = self.block(cx, block, expected);
                cx.unsafe_region = outer;
                let ty = block.ty;
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            ast::ExprKind::None => match self.union_member(expected, "none").map(|t| (t, self.types.kind(t))) {
                Some((ty, TyKind::Option(_))) => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), ty, span),
                _ => {
                    self.diags.error("E1304", span, "`none` needs an expected `T?` type");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Construct(constructor, value) => self.construct(cx, constructor, value, expected, span),
            // A task whose `Join` nobody uses belongs to the enclosing block.
            ast::ExprKind::Spawn(_, false) if expected == Some(Ty::UNIT) => {
                let stmts = vec![self.spawn_statement(cx, e)];
                Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }), Ty::UNIT, span)
            }
            ast::ExprKind::Spawn(call, false) => self.spawn(cx, call, span),
            ast::ExprKind::Spawn(call, true) => {
                let join = self.spawn(cx, call, span);
                if join.ty == Ty::ERROR {
                    return join;
                }
                Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Detach, vec![join]), Ty::UNIT, span)
            }
            ast::ExprKind::Error => Self::error_expr(span),
        }
    }

    fn field_access(&mut self, base: hir::Expr, field: FieldRef, field_span: Span, span: Span) -> hir::Expr {
        if let TyKind::Handle(Handle::Ref, value) = *self.types.kind(base.ty) {
            if let FieldRef::Name("value") = field {
                return Self::mk(hir::ExprKind::RefValue(Box::new(base)), value, span);
            }
            let message = format!("{} has one field, `value`, the value it identifies", self.display(base.ty));
            self.diags.error("E1102", field_span, message);
            return Self::error_expr(span);
        }
        let Some((index, ty)) = self.field_of(base.ty, field, field_span) else {
            return Self::error_expr(span);
        };
        let kind = match index {
            Some(index) => hir::ExprKind::Field(Box::new(base), index),
            None => hir::ExprKind::Retype(Box::new(base)),
        };
        Self::mk(kind, ty, span)
    }

    fn tuple(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let expected_items = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Tuple(types)) if types.len() == items.len() => types.clone(),
            _ => Vec::new(),
        };
        let items: Vec<hir::Expr> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let expected = expected_items.get(i).copied();
                let value = self.expr(cx, item, expected);
                match expected {
                    Some(expected) => self.coerce(value, expected),
                    None => value,
                }
            })
            .collect();
        let ty = self.types.intern(TyKind::Tuple(items.iter().map(|i| i.ty).collect()));
        Self::mk(hir::ExprKind::Aggregate(items), ty, span)
    }

    /// The fields of a record type, or an error at `span`.
    fn record_fields(&mut self, ty: Ty, span: Span) -> Option<Vec<(String, Ty)>> {
        if let TyKind::Adt(id) = self.types.kind(ty)
            && let AdtKind::Record(fields) = &self.types.adt(*id).kind
        {
            return Some(fields.iter().map(|f| (f.name.clone(), f.ty)).collect());
        }
        if ty != Ty::ERROR {
            let message = format!("{} is not a record type", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        None
    }

    /// Checks field initializers against `fields`, in source order.
    /// The values of `inits` for `fields`. With an inference, field types
    /// may mention parameters being inferred and values are not coerced yet.
    fn field_values(
        &mut self,
        cx: &mut FnCx,
        record: Ty,
        fields: &[(String, Ty)],
        inits: &[ast::FieldInit],
        allow_shorthand: bool,
        mut inference: Option<&mut Inference>,
    ) -> (Vec<(u32, hir::Expr)>, Vec<bool>) {
        let mut seen = vec![false; fields.len()];
        let mut values = Vec::new();
        for init in inits {
            let index = fields.iter().position(|(name, _)| *name == init.name.name);
            let expected = match (index, inference.as_deref()) {
                (Some(i), Some(inference)) => self.inferred(inference, fields[i].1),
                (Some(i), None) => Some(fields[i].1),
                (None, _) => None,
            };
            let value = match &init.value {
                Some(value) => self.expr(cx, value, expected),
                None if allow_shorthand && crate::lexer::Kw::lookup(&init.name.name).is_none() => {
                    self.path(cx, std::slice::from_ref(&init.name), None, init.name.span)
                }
                None => {
                    let message =
                        format!("field `{}` needs an explicit value: `{}: value`", init.name.name, init.name.name);
                    self.diags.error("E1102", init.name.span, message);
                    Self::error_expr(init.name.span)
                }
            };
            let Some(index) = index else {
                let mut message = format!("{} has no field `{}`", self.display(record), init.name.name);
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                match closest(&init.name.name, names.iter().map(String::as_str)) {
                    Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
                    None => message.push_str(&format!("; its fields are {}", listed(&names))),
                }
                self.diags.error("E1102", init.name.span, message);
                self.offer_meant(init.name.span);
                continue;
            };
            if std::mem::replace(&mut seen[index], true) {
                let message = format!("field `{}` is initialized more than once", init.name.name);
                self.diags.error("E1102", init.name.span, message);
            }
            let value = match inference.as_deref_mut() {
                Some(inference) => {
                    self.unify(inference, fields[index].1, value.ty);
                    value
                }
                None => self.coerce(value, fields[index].1),
            };
            values.push((index as u32, value));
        }
        (values, seen)
    }

    /// `ty` with the inferred arguments, unless it still mentions an
    /// unknown parameter.
    fn inferred(&mut self, inference: &Inference, ty: Ty) -> Option<Ty> {
        let mut params = Vec::new();
        self.types.params_in(ty, &mut params);
        let unknown = params.iter().any(|p| inference.vars.contains(p) && !inference.map.contains_key(p));
        (!unknown).then(|| self.types.subst(ty, &inference.map))
    }

    /// Binds the parameters in `pattern` so that it matches `actual`.
    fn unify(&mut self, inference: &mut Inference, pattern: Ty, actual: Ty) {
        if actual == Ty::ERROR || actual == Ty::NEVER {
            return;
        }
        if inference.vars.contains(&pattern) {
            inference.map.entry(pattern).or_insert(actual);
            return;
        }
        let (expected, found) = (self.types.kind(pattern).clone(), self.types.kind(actual).clone());
        match (expected, found) {
            (TyKind::Tuple(items), TyKind::Tuple(values)) if items.len() == values.len() => {
                for (item, value) in items.into_iter().zip(values) {
                    self.unify(inference, item, value);
                }
            }
            (TyKind::Option(a), TyKind::Option(b))
            | (TyKind::Array(a), TyKind::Array(b))
            | (TyKind::Pointer(a), TyKind::Pointer(b))
            | (TyKind::Deque(a), TyKind::Deque(b))
            | (TyKind::Set(a), TyKind::Set(b)) => self.unify(inference, a, b),
            (TyKind::Handle(h, a), TyKind::Handle(k, b)) if h == k => self.unify_argument(inference, a, b),
            // A value lifts into an option.
            (TyKind::Option(a), _) => self.unify(inference, a, actual),
            // An error type of `Never` is one: what cannot fail.
            (TyKind::Result(a, b), TyKind::Result(c, Ty::NEVER)) if inference.vars.contains(&b) => {
                self.unify(inference, a, c);
                inference.map.entry(b).or_insert(Ty::NEVER);
            }
            // A value of `Never ! E` is one, as `recover { panic(...) }`.
            (TyKind::Result(a, b), TyKind::Result(c, d)) => {
                self.unify_argument(inference, a, c);
                self.unify_argument(inference, b, d);
            }
            (TyKind::Map(a, b), TyKind::Map(c, d)) => {
                self.unify(inference, a, c);
                self.unify(inference, b, d);
            }
            (TyKind::Fn(params, result, ..), TyKind::Fn(values, value, ..))
            | (TyKind::CFn(params, result, _), TyKind::CFn(values, value, _))
                if params.len() == values.len() =>
            {
                for (param, value) in params.into_iter().zip(values) {
                    self.unify(inference, param, value);
                }
                self.unify(inference, result, value);
            }
            (TyKind::Adt(_), TyKind::Adt(_)) => {
                if let (Some((template, args)), Some((other, values))) =
                    (self.types.instance_of(pattern), self.types.instance_of(actual))
                    && template == other
                {
                    let pairs: Vec<(Ty, Ty)> = args.iter().copied().zip(values.iter().copied()).collect();
                    for (arg, value) in pairs {
                        self.unify_argument(inference, arg, value);
                    }
                }
            }
            _ => {}
        }
    }

    /// `unify` for a type argument, where `Never` is a type like any other
    /// rather than the type of an expression that does not finish.
    fn unify_argument(&mut self, inference: &mut Inference, pattern: Ty, actual: Ty) {
        if actual == Ty::NEVER && inference.vars.contains(&pattern) {
            inference.map.entry(pattern).or_insert(Ty::NEVER);
        } else {
            self.unify(inference, pattern, actual);
        }
    }

    /// For a generic record or enum named without type arguments (its
    /// instance with its own parameters), the inference of its arguments,
    /// seeded from the expected type.
    fn generic_inference(&mut self, ty: Ty, expected: Option<Ty>) -> Option<Inference> {
        let (template, args) = self.types.instance_of(ty)?;
        let params = self.types.template(template).params.clone();
        if args != params.as_slice() {
            return None;
        }
        let mut inference = Inference::new(params);
        let expected = expected.map(|t| match self.types.kind(t) {
            TyKind::Option(inner) => *inner,
            _ => t,
        });
        if let Some(expected) = expected {
            self.unify(&mut inference, ty, expected);
        }
        Some(inference)
    }

    /// The instance of a generic type once every argument is inferred.
    fn finish_inference(&mut self, inference: &Inference, generic: Ty, span: Span) -> Ty {
        let unknown: Vec<String> =
            inference.vars.iter().filter(|v| !inference.map.contains_key(v)).map(|&v| self.types.display(v)).collect();
        if !unknown.is_empty() {
            let (template, _) = self.types.instance_of(generic).unwrap();
            let name = self.types.template(template).name.clone();
            let message = format!(
                "cannot infer {} of `{name}`; annotate the expected type, such as `let value: {} = ...`",
                unknown.join(", "),
                self.types.display(generic)
            );
            self.diags.error("E1101", span, message);
            return Ty::ERROR;
        }
        self.types.subst(generic, &inference.map)
    }

    /// Whether two nominal types are the same, or instances of one generic type.
    fn same_nominal(&self, a: Ty, b: Ty) -> bool {
        a == b
            || matches!((self.types.instance_of(a), self.types.instance_of(b)), (Some((x, _)), Some((y, _))) if x == y)
    }

    fn record_literal(
        &mut self,
        cx: &mut FnCx,
        path: &[ast::Ident],
        type_args: &[ast::TypeExpr],
        inits: &[ast::FieldInit],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if let Some(resolved) = self.variant_path(path) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            if !type_args.is_empty() {
                return self.unsupported_expr(span, "type arguments on a variant");
            }
            let Payload::Record(fields) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take named fields", Self::path_text(path));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            let fields: Vec<(String, Ty)> = fields.into_iter().map(|f| (f.name, f.ty)).collect();
            let owner = self.index_type_name(ty).map(|e| format!("{e}.{}", path.last().unwrap().name));
            self.index_fields(owner, inits.iter().map(|i| (&i.name, i.value.is_none())));
            let (ty, values, seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
            self.report_missing(&fields, &seen, inits, span);
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        let name = path.last().expect("a path has a name");
        let ty = match self.type_names.get(&name.name) {
            // A type of another module, as `time.DateTime { ... }`.
            _ if path.len() > 1 => {
                let explicit = ast::TypeExpr {
                    kind: ast::TypeKind::Path { segments: path.to_vec(), args: type_args.to_vec() },
                    span: name.span,
                };
                match self.resolve_type(&explicit) {
                    Ty::ERROR => return Self::error_expr(span),
                    ty => ty,
                }
            }
            Some(TypeName::Nominal(ty)) if type_args.is_empty() => *ty,
            Some(TypeName::Alias(index)) if type_args.is_empty() => {
                let index = *index;
                self.resolve_alias(index)
            }
            Some(TypeName::Template(_)) if !type_args.is_empty() => {
                let explicit = ast::TypeExpr {
                    kind: ast::TypeKind::Path { segments: path.to_vec(), args: type_args.to_vec() },
                    span: name.span,
                };
                self.resolve_type(&explicit)
            }
            Some(TypeName::Template(template)) => {
                let template = *template;
                let params = self.types.template(template).params.clone();
                self.types.instantiate(template, params)
            }
            Some(_) => {
                let message = format!("`{}` is not generic: write `{} {{ ... }}`", name.name, name.name);
                self.diags.error("E1104", name.span, message);
                return Self::error_expr(span);
            }
            None => {
                self.diags.error("E1001", name.span, unknown_type(&name.name));
                return Self::error_expr(span);
            }
        };
        if path.len() == 1 {
            self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        }
        if self.types.c_layout(ty).is_some_and(|l| l.opaque) {
            let message =
                format!("{} is opaque: only C makes one, and Tondo holds a `Pointer` to it", self.display(ty));
            self.diags.error("E1703", name.span, message);
            return Self::error_expr(span);
        }
        let Some(fields) = self.record_fields(ty, name.span) else {
            return Self::error_expr(span);
        };
        // Outside its module, a literal leaves out the private fields, which
        // then need defaults; one that cannot is refused, and nothing more
        // is said of its fields.
        let mut refused = false;
        if let Some(hidden) = self.hidden_fields(ty).map(<[bool]>::to_vec) {
            let defaults = self.defaults_of(ty).unwrap_or_default();
            let written = inits.iter().find(|i| self.field_index(ty, &i.name.name).is_some_and(|k| hidden[k]));
            if hidden.iter().enumerate().any(|(k, &private)| private && !defaults.get(k).is_some_and(Option::is_some)) {
                let message = format!(
                    "{} has private fields, so only its module can build it with a literal; call a function of that module",
                    self.display(ty)
                );
                self.diags.error("E1502", name.span, message);
                refused = true;
            } else if let Some(init) = written {
                let message = format!(
                    "field `{}` of {} is private to its module: leave it out, and it has its default value",
                    init.name.name,
                    self.display(ty)
                );
                self.diags.error("E1501", init.name.span, message);
            }
        }
        self.index_fields(Some(name.name.clone()), inits.iter().map(|i| (&i.name, i.value.is_none())));
        let (ty, mut values, mut seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
        if self.types.c_layout(ty).is_some_and(|l| l.union) {
            return self.union_literal(cx, ty, values, span);
        }
        // A field a literal leaves out has its default: an annotation's
        // (27.13), or a record's.
        let defaults = match self.annotation_defaults.get(&ty) {
            Some(defaults) => Some(defaults.clone()),
            None => self.defaults_of(ty),
        };
        if let Some(defaults) = defaults {
            for (index, default) in defaults.into_iter().enumerate() {
                if let (false, Some(mut default)) = (seen[index], default) {
                    default.span = span;
                    values.push((index as u32, default));
                    seen[index] = true;
                }
            }
        }
        if !refused {
            self.report_missing(&fields, &seen, inits, span);
        }
        Self::mk(hir::ExprKind::Record(values), ty, span)
    }

    /// A C union from one of its fields, the rest of its bytes zero.
    fn union_literal(&mut self, cx: &mut FnCx, ty: Ty, mut values: Vec<(u32, hir::Expr)>, span: Span) -> hir::Expr {
        if values.len() != 1 {
            let message = format!(
                "{} is a union: give one of its fields, which all share its memory, as in `{} {{ field: value }}`",
                self.display(ty),
                self.types.display(ty)
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let (index, value) = values.pop().unwrap();
        let local = self.hidden_local(cx, ty);
        let zeroed = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), ty, span);
        let stmts = vec![
            hir::Stmt::Let(local, zeroed),
            hir::Stmt::Assign { local, path: vec![hir::PlaceStep::Field(index)], op: None, value, span },
        ];
        let tail = Self::mk(hir::ExprKind::Local(local), ty, span);
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span)
    }

    /// The field values of a record or record variant of type `ty`. For a
    /// generic type named without arguments, they are inferred from the
    /// expected type and the values, which are then coerced.
    fn generic_fields(
        &mut self,
        cx: &mut FnCx,
        ty: Ty,
        fields: &[(String, Ty)],
        inits: &[ast::FieldInit],
        expected: Option<Ty>,
        span: Span,
    ) -> (Ty, Vec<(u32, hir::Expr)>, Vec<bool>) {
        let Some(mut inference) = self.generic_inference(ty, expected) else {
            let (values, seen) = self.field_values(cx, ty, fields, inits, true, None);
            return (ty, values, seen);
        };
        let (values, seen) = self.field_values(cx, ty, fields, inits, true, Some(&mut inference));
        let concrete = self.finish_inference(&inference, ty, span);
        let values = values
            .into_iter()
            .map(|(index, value)| {
                let field = self.types.subst(fields[index as usize].1, &inference.map);
                (index, self.coerce(value, field))
            })
            .collect();
        (concrete, values, seen)
    }

    /// The fields a literal leaves out, but for one that a misspelled field
    /// of it was already told to be, as `emial` for `email`.
    fn report_missing(&mut self, fields: &[(String, Ty)], seen: &[bool], inits: &[ast::FieldInit], span: Span) {
        let names = || fields.iter().map(|(n, _)| n.as_str());
        let meant: Vec<&str> = inits
            .iter()
            .filter(|init| !names().any(|n| n == init.name.name))
            .filter_map(|init| closest(&init.name.name, names()))
            .collect();
        let missing: Vec<(&String, &Ty)> = fields
            .iter()
            .zip(seen)
            .filter(|((n, _), s)| !**s && !meant.contains(&n.as_str()))
            .map(|((n, t), _)| (n, t))
            .collect();
        if !missing.is_empty() {
            let typed: Vec<String> = missing.iter().map(|(n, t)| format!("{n}: {}", self.types.display(**t))).collect();
            let what = if missing.len() == 1 { "the field" } else { "the fields" };
            let message = format!("this value is missing {what} {}", listed(&typed));
            self.diags.error("E1102", span, message);
            // Each with the value a new one starts from, when every one has
            // such a value.
            let values: Option<Vec<String>> =
                missing.iter().map(|(n, t)| Some(format!("{n}: {}", self.empty_value(**t)?))).collect();
            if let Some(values) = values
                && let Some(edit) = self.field_edit(span, &values)
            {
                let title =
                    format!("add {} {}", if values.len() == 1 { "the field" } else { "the fields" }, listed(&typed));
                self.diags.fix(title, false, vec![edit]);
            }
        }
    }

    /// The value a field of a type starts from when nothing says another:
    /// a zero, an empty text or collection, `none`, `false`.
    fn empty_value(&self, ty: Ty) -> Option<&'static str> {
        Some(match self.types.kind(ty) {
            TyKind::Int(_) => "0",
            TyKind::Float(_) => "0.0",
            TyKind::Bool => "false",
            TyKind::String => "\"\"",
            TyKind::Option(_) => "none",
            TyKind::Array(_) => "[]",
            TyKind::Map(..) => "[:]",
            _ => return None,
        })
    }

    /// The edit that adds fields to a record literal before its closing
    /// brace: on their own lines in a literal of several lines, else after
    /// a comma.
    fn field_edit(&self, literal: Span, fields: &[String]) -> Option<(Span, String)> {
        let file = self.sources.file_of(literal.start);
        let start = (literal.start - file.base) as usize;
        let text = &file.text[start..(literal.end - file.base) as usize];
        let open = text.find('{')?;
        if !text.ends_with('}') {
            return None;
        }
        let inside = &text[open + 1..text.len() - 1];
        let close = literal.end - 1;
        if inside.trim().is_empty() {
            let span = Span::new(literal.start + open as u32 + 1, close);
            return Some((span, format!(" {} ", fields.join(", "))));
        }
        if inside.contains('\n') {
            let line = file.text[..(close - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
            let indent: String = file.text[line..].chars().take_while(|c| *c == ' ').collect();
            let lines: String = fields.iter().map(|field| format!("{indent}    {field}\n")).collect();
            return Some((Span::new(file.base + line as u32, file.base + line as u32), lines));
        }
        let end = literal.start + (open + 1 + inside.trim_end().len()) as u32;
        let separator = if inside.trim_end().ends_with(',') { " " } else { ", " };
        Some((Span::new(end, end), format!("{separator}{}", fields.join(", "))))
    }

    /// Whether a name is a context's (11.16).
    fn is_context(&self, name: &str) -> bool {
        matches!(self.type_names.get(name), Some(TypeName::Nominal(ty)) if self.context_of.contains_key(ty))
    }

    /// The context a path names, and its index (11.16).
    fn context_named(&mut self, path: &[ast::Ident]) -> Option<(Ty, u32)> {
        let [name] = path else { return None };
        let Some(&TypeName::Nominal(ty)) = self.type_names.get(&name.name) else { return None };
        let context = *self.context_of.get(&ty)?;
        self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        Some((ty, context))
    }

    /// Each context's maker (11.16): the function that gives its initial
    /// value in a `Ref`, which the runtime calls when a task first reads the
    /// context, with its constant initial values and zeros for the computed
    /// ones, each of which a function of its own computes when it is first
    /// read. The fields are `Send` and `Copy`, since tasks share them and
    /// reading one copies it, and those shown as text have `Display`.
    fn context_makers(&mut self) -> Vec<ContextMade> {
        let mut makers = Vec::new();
        let mut reading = Vec::new();
        for (ty, decl) in self.contexts.clone() {
            let module = std::mem::replace(&mut self.module, decl.module);
            let ast::TypeBody::Record(declared) = &decl.body else { unreachable!("a context is a record") };
            let fields = self.record_fields(ty, decl.span).unwrap_or_default();
            let types: Vec<Ty> = fields.iter().map(|&(_, ty)| ty).collect();
            let reference = self.types.handle(Handle::Ref, ty);
            let context = self.context_of[&ty];
            let mut values = Vec::new();
            let mut initials = Vec::new();
            for (index, (field, (name, field_ty))) in declared.iter().zip(fields).enumerate() {
                let what = format!("the field `{name}` of the context `{}`", decl.name.name);
                if let Some(part) = self.not_send(field_ty) {
                    let message = format!("tasks share {what}, but {} is not `Send`", self.display(part));
                    self.diags.error("E1616", field.ty.span, message);
                }
                if !self.types.is_copy(field_ty) {
                    let message = format!(
                        "reading {what} copies it, but {} is not `Copy`: hold it in a `Ref`, or keep a handle to it",
                        self.display(field_ty)
                    );
                    self.diags.error("E1616", field.ty.span, message);
                }
                let shown = match *self.types.kind(field_ty) {
                    TyKind::Option(inner) => inner,
                    _ => field_ty,
                };
                let told = |a: &&ast::Annotation| a.name.name != "secret";
                let hidden = field.annotations.iter().any(|a| a.name.name == "secret") || self.types.secret(shown);
                for annotation in field.annotations.iter().filter(told) {
                    if hidden {
                        let message = format!(
                            "`@{}` writes {what} where anyone reads it, and it is `@secret`: leave one of the two",
                            annotation.name.name
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                    if !self.types.has_error(shown) && !self.satisfies(shown, DISPLAY, &[]) {
                        let message = format!(
                            "`@{}` shows {what} as text, but {} has no `Display`",
                            annotation.name.name,
                            self.display(shown)
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                }
                let Some(default) = &field.default else { continue };
                // Its own function's, in case it is computed, where `?` may
                // fail with what it propagates.
                let mut own = self.new_cx(field_ty);
                own.infer_error = true;
                self.initial_reads = Some(Vec::new());
                let value = self.expr(&mut own, default, Some(field_ty));
                let reads = self.initial_reads.take().unwrap_or_default();
                let mut value = self.coerce(value, field_ty);
                if self.types.has_error(value.ty) || (own.error.is_none() && constant(&mut value)) {
                    values.push((index as u32, value));
                    continue;
                }
                if let Some(annotation) = field.annotations.iter().find(told) {
                    let message = format!(
                        "`@{}` shows {what} when it is not its initial value, which must then be a constant, and this one is computed: give it a constant, or leave out `@{}`",
                        annotation.name.name, annotation.name.name
                    );
                    self.diags.error("E1616", annotation.span, message);
                }
                let zeroed = |ty: Ty| Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), ty, decl.span);
                values.push((index as u32, zeroed(field_ty)));
                let mut alone = Some(value);
                let parts = (0..types.len())
                    .map(|i| (i as u32, if i == index { alone.take().expect("once") } else { zeroed(types[i]) }))
                    .collect();
                let record = Self::mk(hir::ExprKind::Record(parts), ty, default.span);
                let made = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![record]), reference, default.span);
                let named = format!("{}.{name}", decl.name.name);
                let mut function = hir::Function {
                    name: format!("context {named}"),
                    generics: Vec::new(),
                    params: Vec::new(),
                    loans: Vec::new(),
                    fallible: own.error.is_some(),
                    locals: own.locals,
                    result: match own.error {
                        Some(error) => self.types.intern(TyKind::Result(reference, error)),
                        None => reference,
                    },
                    body: hir::Block { stmts: Vec::new(), tail: Some(Box::new(made)), ty: reference },
                    moves: HashSet::new(),
                    test: false,
                    span: default.span,
                };
                self.ownership(&mut function, &own.params);
                reading.push(Reading { key: (context, index as u32), span: default.span, named: named.clone(), reads });
                initials.push(Initial { field: index as u32, function, error: own.error, named });
            }
            let record = Self::mk(hir::ExprKind::Record(values), ty, decl.span);
            let made = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![record]), reference, decl.span);
            let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(made)), ty: reference };
            let mut maker = hir::Function {
                name: format!("context {}", decl.name.name),
                generics: Vec::new(),
                params: Vec::new(),
                loans: Vec::new(),
                fallible: false,
                locals: Vec::new(),
                result: reference,
                body,
                moves: HashSet::new(),
                test: false,
                span: Span::default(),
            };
            self.ownership(&mut maker, &[]);
            let log = self.context_shown(decl, "log");
            let label = self.context_shown(decl, "label");
            self.module = module;
            makers.push(ContextMade { maker, log, label, initials });
        }
        self.initial_cycles(&reading);
        makers
    }

    /// Computed initial values that read each other, which no order can
    /// compute: `E1616` with the cycle, once (11.16).
    fn initial_cycles(&mut self, reading: &[Reading]) {
        let at: HashMap<(u32, u32), usize> = reading.iter().enumerate().map(|(i, r)| (r.key, i)).collect();
        let next = |i: usize| reading[i].reads.iter().filter_map(|key| at.get(key).copied()).collect::<Vec<_>>();
        // 0 unseen, 1 on the path, 2 done.
        let mut state = vec![0u8; reading.len()];
        for start in 0..reading.len() {
            if state[start] != 0 {
                continue;
            }
            let mut path = vec![start];
            let mut pending = vec![next(start)];
            state[start] = 1;
            while let Some(children) = pending.last_mut() {
                let Some(child) = children.pop() else {
                    state[path.pop().expect("as deep as pending")] = 2;
                    pending.pop();
                    continue;
                };
                match state[child] {
                    0 => {
                        state[child] = 1;
                        path.push(child);
                        pending.push(next(child));
                    }
                    1 => {
                        let from = path.iter().position(|&i| i == child).expect("on the path");
                        let cycle: Vec<String> =
                            path[from..].iter().chain([&child]).map(|&i| format!("`{}`", reading[i].named)).collect();
                        let message = format!(
                            "the initial value of `{}` reads itself: {}; compute one of them from what the others are computed from",
                            reading[child].named,
                            cycle.join(" reads ")
                        );
                        self.diags.error("E1616", reading[child].span, message);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The function that gives a computed initial value whose `?` may fail
    /// (11.16): the record `computes` makes, or a panic that names the field
    /// and describes the error.
    fn initial_failing(&mut self, computes: FnId, reference: Ty, error: Ty, named: &str) -> hir::Function {
        let span = Span::default();
        let mut cx = self.new_cx(reference);
        let (made, failure) = (self.hidden_local(&mut cx, reference), self.hidden_local(&mut cx, error));
        let result = self.types.intern(TyKind::Result(reference, error));
        let mk = |kind, ty| Self::mk(kind, ty, span);
        let pattern = |variant, local, ty| hir::Pattern {
            kind: hir::PatKind::Variant(variant, vec![hir::Pattern { kind: hir::PatKind::Binding(local), ty, span }]),
            ty: result,
            span,
        };
        let described = mk(
            hir::ExprKind::Reflect(hir::ReflectOp::Describe, error, vec![mk(hir::ExprKind::Local(failure), error)]),
            Ty::STRING,
        );
        let message = mk(
            hir::ExprKind::Interpolate(vec![
                hir::Segment::Text(format!("the initial value of `{named}` failed: ")),
                hir::Segment::Value(described),
            ]),
            Ty::STRING,
        );
        let arms = vec![
            hir::Arm {
                pattern: pattern(OK, made, reference),
                guard: None,
                body: mk(hir::ExprKind::Local(made), reference),
            },
            hir::Arm {
                pattern: pattern(ERR, failure, error),
                guard: None,
                body: mk(hir::ExprKind::Panic(Box::new(message), 8), Ty::NEVER),
            },
        ];
        let call = mk(hir::ExprKind::Call(Callee::Fn(computes), Vec::new(), Vec::new()), result);
        let value = mk(hir::ExprKind::Match(Box::new(call), arms), reference);
        let mut function = hir::Function {
            name: format!("context {named}"),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: reference,
            body: hir::Block { stmts: Vec::new(), tail: Some(Box::new(value)), ty: reference },
            moves: HashSet::new(),
            test: false,
            span,
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// The function that shows a context's fields marked `@log` or
    /// `@label` (11.16): for each that does not have its initial value, its
    /// name and its text, each followed by a zero character. A `T?` counts
    /// when it is `some`, with the text of what it holds.
    fn context_shown(&mut self, decl: &'a ast::TypeDecl, kind: &str) -> Option<hir::Function> {
        let ast::TypeBody::Record(fields) = &decl.body else { return None };
        let span = decl.span;
        let ident = |name: &str| ast::Ident { name: name.to_string(), span };
        let expr = |kind: ast::ExprKind| ast::Expr { kind, span };
        let mut parts = Vec::new();
        for field in fields {
            let Some(annotation) = field.annotations.iter().find(|a| a.name.name == kind) else { continue };
            let key = match &annotation.argument {
                Some(ast::AnnotationArg::Text(text)) => text.clone(),
                _ => field.name.name.clone(),
            };
            let read = expr(ast::ExprKind::Path(vec![decl.name.clone(), field.name.clone()]));
            let shown = |value: ast::Expr| {
                let segments = vec![
                    ast::StrSegment::Text(format!("{key}\u{0}")),
                    ast::StrSegment::Expr(value, None),
                    ast::StrSegment::Text("\u{0}".into()),
                ];
                expr(ast::ExprKind::Str(segments))
            };
            let empty = || expr(ast::ExprKind::Str(Vec::new()));
            let ty = self.resolve_type(&field.ty);
            let part = if matches!(self.types.kind(ty), TyKind::Option(_)) {
                // `match field { some(value) => "name\0{value}\0", none => "" }`
                let value = ident("value");
                let pattern = |kind: ast::PatternKind| ast::Pattern { kind, span };
                let arm = |pattern: ast::Pattern, body: ast::Expr| ast::MatchArm {
                    pattern,
                    guard: None,
                    body: ast::ArmBody::Expr(body),
                    span,
                };
                let some = pattern(ast::PatternKind::Constructor {
                    path: vec![ident("some")],
                    args: vec![pattern(ast::PatternKind::Binding(value.clone()))],
                });
                let arms = vec![
                    arm(some, shown(expr(ast::ExprKind::Path(vec![value])))),
                    arm(pattern(ast::PatternKind::None), empty()),
                ];
                expr(ast::ExprKind::Match { scrutinee: Box::new(read), arms })
            } else if let (true, Some(initial)) = (self.types.is_equatable(ty), &field.default) {
                // `if field != initial { "name\0{field}\0" } else { "" }`
                let differs =
                    expr(ast::ExprKind::Binary(ast::BinOp::Ne, Box::new(read.clone()), Box::new(initial.clone())));
                let then = ast::Block { stmts: Vec::new(), tail: Some(Box::new(shown(read))), span };
                let otherwise = ast::Block { stmts: Vec::new(), tail: Some(Box::new(empty())), span };
                expr(ast::ExprKind::If {
                    condition: Box::new(differs),
                    then,
                    otherwise: Some(Box::new(expr(ast::ExprKind::Block(otherwise)))),
                })
            } else {
                shown(read)
            };
            parts.push(ast::StrSegment::Expr(part, None));
        }
        if parts.is_empty() {
            return None;
        }
        let mut cx = self.new_cx(Ty::STRING);
        let value = self.expr(&mut cx, &expr(ast::ExprKind::Str(parts)), Some(Ty::STRING));
        let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(value)), ty: Ty::STRING };
        let mut function = hir::Function {
            name: format!("context {} {kind}", decl.name.name),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: Ty::STRING,
            body,
            moves: HashSet::new(),
            test: false,
            span: Span::default(),
        };
        self.ownership(&mut function, &cx.params);
        Some(function)
    }

    fn path_text(path: &[ast::Ident]) -> String {
        path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
    }

    /// Resolves `Enum.Variant`. Returns `None` when the path does not name a
    /// type, and `Some(None)` after reporting a bad variant.
    fn variant_path(&mut self, path: &[ast::Ident]) -> Option<Option<(Ty, u32)>> {
        let (variant, type_path) = path.split_last()?;
        let type_name = type_path.last()?;
        let ty = match type_path {
            [name] => match self.type_names.get(&name.name)? {
                TypeName::Nominal(ty) => *ty,
                TypeName::Alias(index) => {
                    let index = *index;
                    self.resolve_alias(index)
                }
                // The arguments of a generic enum are inferred at each use.
                TypeName::Template(template) => {
                    let template = *template;
                    let params = self.types.template(template).params.clone();
                    self.types.instantiate(template, params)
                }
            },
            [module, name] => self.module_type(&module.name, &name.name)?,
            _ => return None,
        };
        let found = match self.types.kind(ty) {
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Enum(variants) => Some(variants.iter().position(|v| v.name == variant.name)),
                _ => None,
            },
            _ => None,
        };
        match found {
            Some(Some(index)) => {
                if let Some(owner) = self.index_type_name(ty) {
                    self.index_refer(type_name, crate::index::Key::Type(type_name.name.clone()));
                    self.index_refer(variant, crate::index::Key::Variant(owner, variant.name.clone()));
                }
                Some(Some((ty, index as u32)))
            }
            Some(None) => {
                let mut message = format!("enum `{}` has no variant `{}`", type_name.name, variant.name);
                if let TyKind::Adt(id) = self.types.kind(ty)
                    && let AdtKind::Enum(variants) = &self.types.adt(*id).kind
                {
                    let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                    match closest(&variant.name, names.iter().map(String::as_str)) {
                        Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
                        None => message.push_str(&format!("; its variants are {}", listed(&names))),
                    }
                }
                self.diags.error("E1001", variant.span, message);
                self.offer_meant(variant.span);
                Some(None)
            }
            // A prelude name declared again is an error already (E1005).
            None if ty == Ty::ERROR || self.misnamed.contains(&type_name.name) => Some(None),
            None => {
                let message = format!("`{}` is not an enum", type_name.name);
                self.diags.error("E1102", type_name.span, message);
                Some(None)
            }
        }
    }

    fn payload(&self, ty: Ty, variant: u32) -> Payload {
        let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
        let AdtKind::Enum(variants) = &self.types.adt(*id).kind else { unreachable!() };
        variants[variant as usize].payload.clone()
    }

    fn match_expr(
        &mut self,
        cx: &mut FnCx,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let scrutinee = self.expr(cx, scrutinee, None);
        let mut result = expected;
        let mut out = Vec::new();
        // An arm whose value is an empty literal, `[]`, before any other with
        // a value: the others give it its type.
        let mut later = Vec::new();
        for arm in arms {
            cx.scopes.open();
            let outer = std::mem::replace(&mut cx.diverged, false);
            let pattern = self.pattern(
                cx,
                &arm.pattern,
                scrutinee.ty,
                PatternContext { in_match: true, mutable: false, loans: Loans::Shared },
            );
            let guard = arm.guard.as_ref().map(|guard| {
                let guard = self.expr(cx, guard, Some(Ty::BOOL));
                self.coerce(guard, Ty::BOOL)
            });
            let body = match &arm.body {
                ast::ArmBody::Expr(body) if result.is_none() && empty_literal(body) => {
                    later.push(out.len());
                    Self::error_expr(body.span)
                }
                _ => self.arm_body(cx, &arm.body, &mut result),
            };
            cx.diverged = outer;
            cx.scopes.close();
            out.push(hir::Arm { pattern, guard, body });
        }
        for index in later {
            out[index].body = self.arm_body(cx, &arms[index].body, &mut result);
        }
        let checked: Vec<(&hir::Pattern, bool)> = out.iter().map(|a| (&a.pattern, a.guard.is_some())).collect();
        if scrutinee.ty != Ty::ERROR && !checked.iter().any(|(p, _)| p.ty == Ty::ERROR) {
            let report = crate::exhaust::check_match(&self.types, scrutinee.ty, &checked);
            for index in report.unreachable {
                self.diags.error("E1203", out[index].pattern.span, "this arm is covered by earlier arms");
            }
            if let Some(missing) = report.missing {
                let message = format!("`match` is not exhaustive: `{missing}` is not covered");
                self.diags.error("E1204", span, message);
                // An arm for it before the closing brace, at the arms'
                // indentation, that says it is not written yet.
                let file = self.sources.file_of(span.start);
                let text = &file.text[(span.start - file.base) as usize..(span.end - file.base) as usize];
                if text.ends_with('}') {
                    let close = span.end - 1;
                    let line = file.text[..(close - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
                    let before = &file.text[line..(close - file.base) as usize];
                    let indent: String = file.text[line..].chars().take_while(|c| *c == ' ').collect();
                    let at = |arms: String| match before.trim().is_empty() {
                        true => (Span::new(file.base + line as u32, file.base + line as u32), arms),
                        false => (Span::new(close, close), format!("\n{arms}{indent}")),
                    };
                    let arm =
                        |missing: &str| format!("{indent}    {missing} => panic(\"`{missing}` is not handled yet\")\n");
                    self.diags.fix(format!("add an arm for `{missing}`"), false, vec![at(arm(&missing))]);
                    if report.every_missing.len() > 1 {
                        let arms: String = report.every_missing.iter().map(|m| arm(m)).collect();
                        let title = format!("add an arm for each of the {} cases missing", report.every_missing.len());
                        self.diags.fix(title, false, vec![at(arms)]);
                    }
                }
            }
        }
        let all_diverge = out.iter().all(|a| a.body.ty == Ty::NEVER);
        let ty = if all_diverge { Ty::NEVER } else { result.unwrap_or(Ty::UNIT) };
        Self::mk(hir::ExprKind::Match(Box::new(scrutinee), out), ty, span)
    }

    /// The body of a `match` or `select` arm, of the type the first arm
    /// with a value set in `result`.
    fn arm_body(&mut self, cx: &mut FnCx, body: &ast::ArmBody, result: &mut Option<Ty>) -> hir::Expr {
        let body = match body {
            ast::ArmBody::Expr(body) => {
                let value = self.expr(cx, body, *result);
                match *result {
                    Some(expected) => self.coerce(value, expected),
                    None => value,
                }
            }
            ast::ArmBody::Statement(stmt) => {
                // An assignment or `_ =` gives `Unit`; a transfer leaves.
                let ty = match stmt.kind {
                    ast::StmtKind::Assign { .. } | ast::StmtKind::Discard(_) => Ty::UNIT,
                    _ => Ty::NEVER,
                };
                let stmts = self.statement(cx, stmt).into_iter().collect();
                let block = hir::Block { stmts, tail: None, ty };
                Self::mk(hir::ExprKind::Block(block), ty, stmt.span)
            }
        };
        if result.is_none() && body.ty != Ty::NEVER && body.ty != Ty::ERROR {
            *result = Some(body.ty);
        }
        body
    }

    /// `select { ... }` (11.14): waits for the first of its operations and
    /// runs that arm, or the `else` arm when none is ready.
    fn select(&mut self, cx: &mut FnCx, arms: &[ast::SelectArm], expected: Option<Ty>, span: Span) -> hir::Expr {
        let mut result = expected;
        let mut out: Vec<hir::SelectArm> = Vec::new();
        let mut otherwise: Option<hir::Expr> = None;
        let mut taken: Vec<LocalId> = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            let outer = std::mem::replace(&mut cx.diverged, false);
            cx.scopes.open();
            match &arm.operation {
                None => {
                    if otherwise.is_some() {
                        self.diags.error("E1613", arm.span, "`select` has one `else` arm at most");
                    } else if index + 1 != arms.len() {
                        self.diags.error("E1613", arm.span, "the `else` arm of `select` comes last");
                    }
                    otherwise = Some(self.arm_body(cx, &arm.body, &mut result));
                }
                Some(operation) => {
                    let operation = self.expr(cx, operation, None);
                    let valid = matches!(
                        operation.kind,
                        hir::ExprKind::TaskOp(
                            hir::TaskOp::Receive | hir::TaskOp::Send | hir::TaskOp::Wait | hir::TaskOp::GroupNext,
                            _
                        ) | hir::ExprKind::Call(Callee::Std(StdFn::TimeSleep), ..)
                            | hir::ExprKind::Error
                    );
                    if let hir::ExprKind::TaskOp(hir::TaskOp::ReceiveTimeout, _) = operation.kind {
                        let message = "in `select`, a timeout is an arm of its own: `time.sleep(delay) => ...`";
                        self.diags.error("E1612", operation.span, message);
                    } else if !valid {
                        let message = "`select` waits for `receiver.receive()`, `sender.send(value)`, `job.wait()`, `group.next()` or `time.sleep(delay)`";
                        self.diags.error("E1612", operation.span, message);
                    }
                    // An affine value moves only in the arm whose operation
                    // completes, so no two arms may take the same one.
                    if let hir::ExprKind::TaskOp(op, operands) = &operation.kind {
                        for (position, operand) in operands.iter().enumerate() {
                            let Some(place) = hir::Place::of(operand.clone()) else { continue };
                            if !op.takes(position) || self.types.is_copy(operand.ty) {
                                continue;
                            }
                            if taken.contains(&place.local) {
                                let name = &cx.locals[place.local.0 as usize].name;
                                let message = format!(
                                    "`{name}` is taken by two arms of this `select`, but only one arm runs; give each arm its own value"
                                );
                                self.diags.error("E1615", operand.span, message);
                            }
                            taken.push(place.local);
                        }
                    }
                    let pattern = arm.pattern.as_ref().map(|p| {
                        let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
                        let pattern = self.pattern(cx, p, operation.ty, context);
                        if let Some(missing) = crate::exhaust::refutation(&self.types, operation.ty, &pattern) {
                            let message =
                                format!("this pattern can fail, for example on `{missing}`; match it in the body");
                            self.diags.error("E1201", pattern.span, message);
                        }
                        pattern
                    });
                    let body = self.arm_body(cx, &arm.body, &mut result);
                    out.push(hir::SelectArm { operation, pattern, body });
                }
            }
            cx.scopes.close();
            cx.diverged = outer;
        }
        if out.is_empty() {
            self.diags.error("E1613", span, "`select` needs at least one operation to wait for");
            return Self::error_expr(span);
        }
        let all_diverge = out.iter().map(|a| &a.body).chain(otherwise.as_ref()).all(|b| b.ty == Ty::NEVER);
        let ty = if all_diverge { Ty::NEVER } else { result.unwrap_or(Ty::UNIT) };
        Self::mk(hir::ExprKind::Select(out, otherwise.map(Box::new)), ty, span)
    }

    fn pattern_error(&mut self, span: Span, message: String) -> hir::Pattern {
        self.diags.error("E1202", span, message);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    /// Checks a pattern against a value of type `ty`, declaring its bindings
    /// in the current scope. Those of a pattern that does not fit, or of a
    /// value already wrong, are declared as wrong too, so that their uses
    /// add no errors of their own.
    fn pattern(&mut self, cx: &mut FnCx, p: &ast::Pattern, ty: Ty, context: PatternContext) -> hir::Pattern {
        let simple =
            matches!(p.kind, ast::PatternKind::Binding(_) | ast::PatternKind::Loan(..) | ast::PatternKind::Wildcard);
        if matches!(self.types.kind(ty), TyKind::Error) && !simple {
            self.declare_failed(cx, p);
            return Self::wild(ty, p.span);
        }
        let before = self.diags.list.len();
        let checked = self.checked_pattern(cx, p, ty, context);
        let failed = self.diags.list[before..].iter().any(|d| d.severity == crate::diag::Severity::Error);
        if failed && matches!(checked.kind, hir::PatKind::Wildcard) {
            self.declare_failed(cx, p);
        }
        checked
    }

    /// Declares the bindings of a pattern that failed, as wrong.
    fn declare_failed(&mut self, cx: &mut FnCx, p: &ast::Pattern) {
        let mut names = Vec::new();
        bound_idents(p, &mut names);
        for name in names {
            if name.name != "_" && self.lookup(cx, &name.name).is_none() {
                self.declare(cx, name, Ty::ERROR, false);
            }
        }
    }

    fn checked_pattern(&mut self, cx: &mut FnCx, p: &ast::Pattern, ty: Ty, context: PatternContext) -> hir::Pattern {
        let span = p.span;
        if let Some(members) = self.types.members(ty).map(<[Ty]>::to_vec)
            && !matches!(p.kind, ast::PatternKind::Wildcard)
            && let Some(pattern) = self.union_pattern(cx, p, ty, &members, context)
        {
            return pattern;
        }
        let mismatch = |this: &Self, what: &str| format!("{what} cannot match a value of type {}", this.display(ty));
        let kind = match &p.kind {
            ast::PatternKind::Wildcard => hir::PatKind::Wildcard,
            ast::PatternKind::Binding(name) => {
                let message = match name.name.as_str() {
                    "None" => Some("`None` is not a Tondo pattern; write `none`".to_string()),
                    "Some" | "Ok" | "Err" => Some(format!(
                        "`{}` is not a Tondo pattern; write `{}(...)`",
                        name.name,
                        name.name.to_lowercase()
                    )),
                    n if context.in_match && CONTEXTUAL_CONSTRUCTORS.contains(&n) => {
                        Some(format!("`{n}` without parentheses would match any value; write `{n}(...)` or rename it"))
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    return self.pattern_error(name.span, message);
                }
                hir::PatKind::Binding(self.declare(cx, name, ty, context.mutable))
            }
            ast::PatternKind::Loan(mode, name) => {
                let allowed = match context.loans {
                    Loans::Any => true,
                    Loans::Shared => *mode == ast::Mode::Ref,
                    Loans::None => false,
                };
                if !allowed {
                    let message = if context.loans == Loans::Shared {
                        format!(
                            "a `match` arm only observes its value, so it cannot bind `{} {}`: `mut` and `var` bindings belong in `for` headers",
                            mode.keyword(),
                            name.name
                        )
                    } else {
                        format!(
                            "`{} {}` borrows a part, which only `match` arms and `for` loops over collections do; bind it by value",
                            mode.keyword(),
                            name.name
                        )
                    };
                    return self.pattern_error(span, message);
                }
                let local = self.declare(cx, name, ty, *mode != ast::Mode::Ref);
                cx.params[local.0 as usize] = Some(*mode);
                cx.loan_bindings.insert(local);
                hir::PatKind::Binding(local)
            }
            ast::PatternKind::Literal(expr) => {
                let value = self.expr(cx, expr, Some(ty));
                let literal = match value.kind {
                    hir::ExprKind::Int(v) => hir::Literal::Int(v),
                    hir::ExprKind::Float(v) => hir::Literal::Float(v.to_bits()),
                    hir::ExprKind::Char(c) => hir::Literal::Char(c),
                    hir::ExprKind::Bool(b) => hir::Literal::Bool(b),
                    hir::ExprKind::Str(s) => hir::Literal::Str(s),
                    hir::ExprKind::Unit => hir::Literal::Unit,
                    _ => return self.unsupported_pattern(span, "this literal pattern"),
                };
                if value.ty != ty && ty != Ty::ERROR && value.ty != Ty::ERROR {
                    let message = mismatch(self, &format!("a {} literal", self.display(value.ty)));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Literal(literal)
            }
            ast::PatternKind::None if !context.in_match => {
                return self.pattern_error(span, "`none` is a reserved keyword and cannot name a binding".into());
            }
            ast::PatternKind::None => match self.types.kind(ty) {
                TyKind::Option(_) => hir::PatKind::Variant(NONE, Vec::new()),
                TyKind::Error => hir::PatKind::Wildcard,
                _ => return self.pattern_error(span, mismatch(self, "`none`")),
            },
            ast::PatternKind::Tuple(items) => match self.types.kind(ty) {
                TyKind::Tuple(types) if types.len() == items.len() => {
                    let types = types.clone();
                    let fields = items.iter().zip(types).map(|(item, t)| self.pattern(cx, item, t, context)).collect();
                    hir::PatKind::Fields(fields)
                }
                TyKind::Error => hir::PatKind::Wildcard,
                _ => {
                    let message = mismatch(self, &format!("a tuple pattern of {} elements", items.len()));
                    return self.pattern_error(span, message);
                }
            },
            ast::PatternKind::Constructor { path, args } => {
                if let [name] = path.as_slice()
                    && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str())
                {
                    let (variant, payload) = match (name.name.as_str(), self.types.kind(ty)) {
                        ("some", TyKind::Option(inner)) => (SOME, *inner),
                        ("ok", TyKind::Result(ok, _)) => (OK, *ok),
                        ("err", TyKind::Result(_, err)) => (ERR, *err),
                        (_, TyKind::Error) => return Self::wild(ty, span),
                        _ => return self.pattern_error(span, mismatch(self, &format!("`{}(...)`", name.name))),
                    };
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let inner = self.pattern(cx, arg, payload, context);
                    return hir::Pattern { kind: hir::PatKind::Variant(variant, vec![inner]), ty, span };
                }
                if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    if !self.same_nominal(enum_ty, ty) {
                        let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                        return self.pattern_error(span, message);
                    }
                    let Payload::Tuple(types) = self.payload(ty, variant) else {
                        let message = format!("`{}` has no positional payload", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    if types.len() != args.len() {
                        let message = format!("`{}` has {} payload values", Self::path_text(path), types.len());
                        return self.pattern_error(span, message);
                    }
                    let fields = args.iter().zip(types).map(|(arg, t)| self.pattern(cx, arg, t, context)).collect();
                    hir::PatKind::Variant(variant, fields)
                } else if let [name] = path.as_slice()
                    && let Some(&ModuleName::Newtype(newtype)) = self.names.get(&name.name)
                {
                    if newtype != ty {
                        let message = mismatch(self, &format!("a `{}` pattern", name.name));
                        return self.pattern_error(span, message);
                    }
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                    let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                    hir::PatKind::Newtype(Box::new(self.pattern(cx, arg, inner, context)))
                } else {
                    let message = format!("`{}` is not a constructor", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
            }
            ast::PatternKind::Record { path, fields, rest } => {
                let (record_ty, declared, variant) = if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    // A generic enum is matched as the scrutinee's instance.
                    let enum_ty = if self.same_nominal(enum_ty, ty) { ty } else { enum_ty };
                    let Payload::Record(fields) = self.payload(enum_ty, variant) else {
                        let message = format!("`{}` has no named fields", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    (enum_ty, fields.into_iter().map(|f| (f.name, f.ty)).collect(), Some(variant))
                } else if let [name] = path.as_slice()
                    && let Some(record) = self.type_named(&name.name)
                {
                    let record = if self.same_nominal(record, ty) { ty } else { record };
                    let Some(fields) = self.record_fields(record, name.span) else { return Self::wild(ty, span) };
                    if let Some(hidden) = self.hidden_fields(record).map(<[bool]>::to_vec) {
                        let ast::PatternKind::Record { fields: written, .. } = &p.kind else { unreachable!() };
                        let named = written.iter().find(|w| {
                            fields.iter().zip(&hidden).any(|((name, _), private)| *private && *name == w.name.name)
                        });
                        let message = match named {
                            Some(field) => Some(format!(
                                "field `{}` of {} is private to its module",
                                field.name.name,
                                self.display(record)
                            )),
                            None if !*rest => {
                                Some(format!("{} has private fields; end the pattern with `..`", self.display(record)))
                            }
                            None => None,
                        };
                        if let Some(message) = message {
                            self.diags.error("E1501", span, message);
                            return Self::wild(ty, span);
                        }
                    }
                    (record, fields, None)
                } else {
                    let message = format!("`{}` is not a record type or variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                if record_ty != ty {
                    let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                let owner = self.index_type_name(record_ty).map(|owner| match variant {
                    Some(_) => format!("{owner}.{}", path.last().unwrap().name),
                    None => owner,
                });
                if variant.is_none()
                    && let [name] = path.as_slice()
                {
                    self.index_refer(name, crate::index::Key::Type(name.name.clone()));
                }
                self.index_fields(owner, fields.iter().map(|f| (&f.name, f.pattern.is_none())));
                let patterns = self.field_patterns(cx, &declared, fields, *rest, context, span);
                match variant {
                    Some(variant) => hir::PatKind::Variant(variant, patterns),
                    None => hir::PatKind::Fields(patterns),
                }
            }
            ast::PatternKind::Member { .. } => {
                return self.pattern_error(span, mismatch(self, "a member pattern"));
            }
            ast::PatternKind::Array { items, rest } => {
                let element = match *self.types.kind(ty) {
                    TyKind::Array(element) => element,
                    TyKind::Error => return Self::wild(ty, span),
                    _ => return self.pattern_error(span, mismatch(self, "an array pattern")),
                };
                let items = items.iter().map(|item| self.pattern(cx, item, element, context)).collect();
                let mut bound = None;
                if let Some(Some((mode, name))) = rest {
                    if *mode == ast::Mode::Ref && context.loans == Loans::None {
                        let message = format!(
                            "`..ref {}` borrows the rest, which only `match` arms and `for` loops do; write `..{0}`",
                            name.name
                        );
                        return self.pattern_error(span, message);
                    }
                    let local = self.declare(cx, name, ty, context.mutable && *mode == ast::Mode::Value);
                    if *mode == ast::Mode::Ref {
                        cx.params[local.0 as usize] = Some(ast::Mode::Ref);
                        cx.loan_bindings.insert(local);
                    }
                    bound = Some(local);
                }
                let rest = rest.as_ref().map(|_| bound);
                hir::PatKind::Array { items, rest }
            }
            ast::PatternKind::Path(path) => {
                let Some(resolved) = self.variant_path(path) else {
                    let message = format!("`{}` is not a variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                if !self.same_nominal(enum_ty, ty) {
                    let message = mismatch(self, &format!("`{}`", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                if !matches!(self.payload(ty, variant), Payload::Unit) {
                    let message = format!("`{}` has a payload; match it explicitly", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Variant(variant, Vec::new())
            }
        };
        hir::Pattern { kind, ty, span }
    }

    /// A pattern against a union (9.6): `Member(pattern)` tests the tag and
    /// matches the whole member, and a pattern of a member's own type (a
    /// record, variant, option or literal) selects that member. `None` lets
    /// a binding match the whole union.
    fn union_pattern(
        &mut self,
        cx: &mut FnCx,
        p: &ast::Pattern,
        ty: Ty,
        members: &[Ty],
        context: PatternContext,
    ) -> Option<hir::Pattern> {
        let span = p.span;
        let explicit = match &p.kind {
            ast::PatternKind::Member { ty: member, inner } => Some((self.resolve_type(member), &**inner)),
            // `Int(n)`, or a type of a module, `time.DateTime(at)`; not a
            // variant, `Shape.Circle(r)`.
            ast::PatternKind::Constructor { path, args } if path.len() <= 2 => {
                let member = match path.as_slice() {
                    [name] => self.type_named(&name.name),
                    [module, name] if !self.type_names.contains_key(&module.name) => {
                        self.module_type(&module.name, &name.name)
                    }
                    _ => None,
                };
                match member {
                    Some(member) => {
                        let [inner] = args.as_slice() else {
                            let name = Self::path_text(path);
                            return Some(self.pattern_error(span, format!("`{name}(...)` takes one pattern")));
                        };
                        Some((member, inner))
                    }
                    None => None,
                }
            }
            ast::PatternKind::Binding(name) if self.type_named(&name.name).is_some() => {
                let message = format!(
                    "`{0}` alone would bind any value; test the member with `{0}(_)` or `{0}(name)`",
                    name.name
                );
                return Some(self.pattern_error(name.span, message));
            }
            _ => None,
        };
        let (member, inner) = match explicit {
            Some((member, inner)) => (member, Some(inner)),
            None => (self.pattern_member_type(p, ty, members)?, None),
        };
        if member == Ty::ERROR {
            return Some(Self::wild(ty, span));
        }
        let Some(index) = members.iter().position(|&m| m == member) else {
            let message = format!("{} is not a member of {}", self.display(member), self.display(ty));
            return Some(self.pattern_error(span, message));
        };
        let inner = self.pattern(cx, inner.unwrap_or(p), member, context);
        Some(hir::Pattern { kind: hir::PatKind::Variant(index as u32, vec![inner]), ty, span })
    }

    /// The member type a nominal, option or literal pattern belongs to.
    fn pattern_member_type(&mut self, p: &ast::Pattern, ty: Ty, members: &[Ty]) -> Option<Ty> {
        let only = |this: &Self, test: fn(&TyKind) -> bool| {
            let found: Vec<Ty> = members.iter().copied().filter(|&m| test(this.types.kind(m))).collect();
            match found.as_slice() {
                [only] => Some(*only),
                _ => None,
            }
        };
        match &p.kind {
            ast::PatternKind::Record { path, .. } | ast::PatternKind::Path(path) => match self.variant_path(path) {
                Some(resolved) => Some(resolved.map_or(Ty::ERROR, |(enum_ty, _)| enum_ty)),
                None if path.len() == 1 => self.type_named(&path[0].name),
                None => None,
            },
            ast::PatternKind::Constructor { path, .. } => match path.as_slice() {
                [name] if matches!(name.name.as_str(), "some") => only(self, |k| matches!(k, TyKind::Option(_))),
                [name] if matches!(name.name.as_str(), "ok" | "err") => only(self, |k| matches!(k, TyKind::Result(..))),
                _ => self.variant_path(path).map(|resolved| resolved.map_or(Ty::ERROR, |(enum_ty, _)| enum_ty)),
            },
            ast::PatternKind::None => only(self, |k| matches!(k, TyKind::Option(_))),
            ast::PatternKind::Literal(expr) => self.literal_type(expr, ty),
            _ => None,
        }
    }

    /// The type a literal pattern has when matched against `ty`.
    fn literal_type(&self, expr: &ast::Expr, ty: Ty) -> Option<Ty> {
        Some(match &expr.kind {
            ast::ExprKind::Int { suffix: Some(suffix), .. } => Ty::int(IntTy::from_suffix(suffix)),
            ast::ExprKind::Int { .. } if ty == Ty::BYTE => Ty::BYTE,
            ast::ExprKind::Int { .. } => self.expected_number(Some(ty), false).unwrap_or(Ty::INT),
            ast::ExprKind::Float { suffix: Some(suffix), .. } => {
                if *suffix == "f32" {
                    Ty::F32
                } else {
                    Ty::F64
                }
            }
            ast::ExprKind::Float { .. } => self.expected_number(Some(ty), true).unwrap_or(Ty::F64),
            ast::ExprKind::Unary(_, inner) => return self.literal_type(inner, ty),
            ast::ExprKind::Str(_) => Ty::STRING,
            ast::ExprKind::Char(_) => Ty::CHAR,
            ast::ExprKind::Bool(_) => Ty::BOOL,
            _ => return None,
        })
    }

    /// The type a single name denotes, without reporting unknown names.
    fn type_named(&mut self, name: &str) -> Option<Ty> {
        // A type parameter in scope, as `T` in `T.max`.
        if let Some(&(_, param)) = self.type_params.iter().rev().find(|(n, _)| n == name) {
            return Some(param);
        }
        // A generic type stands for its instance with its own parameters.
        if let Some(&TypeName::Template(template)) = self.type_names.get(name) {
            let params = self.types.template(template).params.clone();
            return Some(self.types.instantiate(template, params));
        }
        let path = ast::TypeExpr {
            kind: ast::TypeKind::Path {
                segments: vec![ast::Ident { name: name.into(), span: Span::default() }],
                args: Vec::new(),
            },
            span: Span::default(),
        };
        let builtin = matches!(
            name,
            "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "Int128"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "UInt128"
                | "Bool"
                | "String"
                | "Float"
                | "Float32"
                | "Float64"
                | "Char"
                | "Byte"
        );
        (builtin || self.type_names.contains_key(name)).then(|| self.resolve_type(&path))
    }

    fn wild(ty: Ty, span: Span) -> hir::Pattern {
        hir::Pattern { kind: hir::PatKind::Wildcard, ty, span }
    }

    fn unsupported_pattern(&mut self, span: Span, what: &str) -> hir::Pattern {
        self.diags.unsupported(span, what);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    fn field_patterns(
        &mut self,
        cx: &mut FnCx,
        declared: &[(String, Ty)],
        fields: &[ast::FieldPattern],
        rest: bool,
        context: PatternContext,
        span: Span,
    ) -> Vec<hir::Pattern> {
        let mut patterns: Vec<Option<hir::Pattern>> = (0..declared.len()).map(|_| None).collect();
        for field in fields {
            let Some(index) = declared.iter().position(|(n, _)| *n == field.name.name) else {
                let message = format!("there is no field `{}`", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            };
            if patterns[index].is_some() {
                let message = format!("field `{}` appears more than once", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            }
            let ty = declared[index].1;
            let pattern = match &field.pattern {
                Some(pattern) => self.pattern(cx, pattern, ty, context),
                None => {
                    let binding =
                        ast::Pattern { kind: ast::PatternKind::Binding(field.name.clone()), span: field.name.span };
                    self.pattern(cx, &binding, ty, context)
                }
            };
            patterns[index] = Some(pattern);
        }
        let missing: Vec<&str> =
            declared.iter().zip(&patterns).filter(|(_, p)| p.is_none()).map(|((n, _), _)| n.as_str()).collect();
        if !rest && !missing.is_empty() {
            let message = format!("pattern does not mention {}; add `..` to ignore them", missing.join(", "));
            self.diags.error("E1202", span, message);
        }
        patterns
            .into_iter()
            .zip(declared)
            .map(|(p, (_, ty))| p.unwrap_or(hir::Pattern { kind: hir::PatKind::Wildcard, ty: *ty, span }))
            .collect()
    }

    fn with_expr(&mut self, cx: &mut FnCx, base: &ast::Expr, updates: &[ast::FieldInit], span: Span) -> hir::Expr {
        let base = self.expr(cx, base, None);
        self.update(cx, base, updates, true, span)
    }

    /// `base` with the fields of `updates` changed; with `shorthand`, a
    /// field may be named alone, as the binding of its name.
    fn update(
        &mut self,
        cx: &mut FnCx,
        base: hir::Expr,
        updates: &[ast::FieldInit],
        shorthand: bool,
        span: Span,
    ) -> hir::Expr {
        let ty = base.ty;
        let Some(fields) = self.record_fields(ty, base.span) else {
            return Self::error_expr(span);
        };
        if updates.is_empty() {
            self.diags.error("E1102", span, "`with` must update at least one field");
        }
        if let Some(hidden) = self.hidden_fields(ty).map(<[bool]>::to_vec) {
            for update in updates {
                if let Some(index) = self.field_index(ty, &update.name.name)
                    && hidden[index]
                {
                    let message =
                        format!("field `{}` of {} is private to its module", update.name.name, self.display(ty));
                    self.diags.error("E1501", update.name.span, message);
                }
            }
        }
        let owner = self.index_type_name(ty);
        self.index_fields(owner, updates.iter().map(|u| (&u.name, false)));
        let (values, _) = self.field_values(cx, ty, &fields, updates, shorthand, None);
        if self.types.c_layout(ty).is_some_and(|l| l.union) {
            // The fields of a union overlap: a copy, then the fields written.
            let local = self.hidden_local(cx, ty);
            let mut stmts = vec![hir::Stmt::Let(local, base)];
            for (index, value) in values {
                stmts.push(hir::Stmt::Assign {
                    local,
                    path: vec![hir::PlaceStep::Field(index)],
                    op: None,
                    value,
                    span,
                });
            }
            let tail = Self::mk(hir::ExprKind::Local(local), ty, span);
            return Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span);
        }
        Self::mk(hir::ExprKind::With(Box::new(base), values), ty, span)
    }

    /// `some(value)`, `ok(value)` or `err(value)`.
    fn construct(
        &mut self,
        cx: &mut FnCx,
        constructor: &str,
        value: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let expected = self.union_member(expected, constructor);
        let expected_kind = expected.map(|t| self.types.kind(t).clone());
        let (variant, payload_ty, ty) = match (constructor, expected_kind) {
            ("some", Some(TyKind::Option(inner))) => (SOME, Some(inner), expected),
            ("some", _) => (SOME, None, None),
            ("ok", Some(TyKind::Result(ok, _))) => (OK, Some(ok), expected),
            ("err", Some(TyKind::Result(_, err))) => (ERR, Some(err), expected),
            _ => {
                let message = format!("`{constructor}(...)` needs an expected `T ! E` type");
                self.diags.error("E1304", span, message);
                self.expr(cx, value, None);
                return Self::error_expr(span);
            }
        };
        let value = self.expr(cx, value, payload_ty);
        let value = match payload_ty {
            Some(payload) => self.coerce(value, payload),
            None => value,
        };
        let ty = ty.unwrap_or_else(|| self.types.intern(TyKind::Option(value.ty)));
        Self::mk(hir::ExprKind::Variant(variant, vec![(0, value)]), ty, span)
    }

    /// The type `none`, `some(...)`, `ok(...)` or `err(...)` builds: the
    /// expected type, or the only option or result member of an expected
    /// union, into which the value is then injected.
    fn union_member(&self, expected: Option<Ty>, constructor: &str) -> Option<Ty> {
        let Some(members) = expected.and_then(|t| self.types.members(t)) else { return expected };
        let option = matches!(constructor, "none" | "some");
        let found: Vec<Ty> = members
            .iter()
            .copied()
            .filter(|&m| match self.types.kind(m) {
                TyKind::Option(_) => option,
                TyKind::Result(..) => !option,
                _ => false,
            })
            .collect();
        match found.as_slice() {
            [only] => Some(*only),
            _ => expected,
        }
    }

    /// `value?` propagates `none` or an error out of the enclosing function
    /// and otherwise yields the payload (15.2).
    /// `try { ... }` (15.2): a closure called there, whose `?` ends the block
    /// and not the function. What it gives comes from where it goes.
    fn try_block(&mut self, cx: &mut FnCx, block: &ast::Block, expected: Option<Ty>, span: Span) -> hir::Expr {
        let gives = expected.filter(|&ty| matches!(self.types.kind(ty), TyKind::Option(_) | TyKind::Result(..)));
        let Some(gives) = gives else {
            let message = "a `try` block gives an option or a result: say which, as `let city: String? = try { ... }`";
            self.diags.error("E1301", span, message);
            return Self::error_expr(span);
        };
        let function = self.types.intern(TyKind::Fn(Vec::new(), gives, false, Vec::new(), false));
        let closure = ast::Closure { is_unsafe: false, params: Vec::new(), result: None, body: block.clone() };
        self.try_body = true;
        let value = self.closure(cx, &closure, Some(function), span);
        let local = self.hidden_local(cx, value.ty);
        let read = Self::mk(hir::ExprKind::Local(local), value.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), gives, span);
        let stmts = vec![hir::Stmt::Let(local, value)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(call)), ty: gives }), gives, span)
    }

    fn try_expr(&mut self, cx: &mut FnCx, inner: &ast::Expr, expected: Option<Ty>, span: Span) -> hir::Expr {
        if cx.deferred {
            self.diags.error("E1410", span, "a defer block cannot propagate an error; handle it inside the block");
        }
        // A call learns the success type it should give: `decode(text)?`.
        let hint = match &inner.kind {
            ast::ExprKind::Call { .. } => expected,
            _ => None,
        };
        let value = self.expr(cx, inner, hint);
        self.note_propagation(cx, value.ty, Span::new(span.end - 1, span.end), "?");
        self.note_exit(cx, Span::new(span.end - 1, span.end));
        match self.types.kind(value.ty).clone() {
            TyKind::Option(payload) => {
                if !matches!(self.types.kind(cx.result), TyKind::Option(_)) {
                    // A function that fails with `Failure` says what is missing.
                    let message = match cx.error {
                        Some(error) if Some(error) == self.failure_type() => {
                            "`?` on an option needs the function to return an option; this one fails with `Failure`, so say what is missing with `.context(\"...\")?`".to_string()
                        }
                        _ => format!(
                            "`?` on an option needs the function to return an option, but it returns {}",
                            self.display(cx.result)
                        ),
                    };
                    self.diags.error("E1301", span, message);
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), payload, span)
            }
            // What cannot fail propagates nothing: its success comes out.
            TyKind::Result(ok, Ty::NEVER) => Self::mk(hir::ExprKind::Try(Box::new(value)), ok, span),
            TyKind::Result(ok, error) => {
                if let Some(channel) = self.error_channel(cx, error, span, "E1301")
                    && channel != error
                    && channel != Ty::ERROR
                    && !self.types.has_error(error)
                {
                    // Widening into an error union is a closed set inclusion (15.3).
                    let produced = self.types.members(error).map_or(vec![error], <[Ty]>::to_vec);
                    let declared = self.types.members(channel).map_or(vec![channel], <[Ty]>::to_vec);
                    let missing: Vec<Ty> = produced.iter().copied().filter(|t| !declared.contains(t)).collect();
                    if !missing.is_empty() && Some(channel) == self.failure_type() {
                        let message = format!(
                            "`?` would propagate {} but the function fails with `Failure`: say what it was doing with `.context(\"...\")?`, which keeps the error as the cause",
                            self.display(error)
                        );
                        self.diags.error("E1301", span, message);
                    } else if !missing.is_empty() {
                        let missing_text: Vec<String> = missing.iter().map(|&t| self.display(t)).collect();
                        let widened = self.types.union(&[channel, error]);
                        let result = self.types.intern(TyKind::Result(cx.result, widened));
                        let message = format!(
                            "`?` would propagate {} but the function fails with {}; {} is missing: declare the result as {}",
                            self.display(error),
                            self.display(channel),
                            missing_text.join(", "),
                            self.display(result)
                        );
                        self.diags.error("E1301", span, message);
                        // Widening a public function's errors changes its interface.
                        if let Some((written, public)) = cx.result_span {
                            let widened = self.types.display(result);
                            let title = format!("declare the result as `{widened}`");
                            let mut edits = vec![(written, widened)];
                            edits.extend(self.imports_naming(error, span));
                            self.diags.fix(title, !public, edits);
                        }
                    }
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), ok, span)
            }
            TyKind::Error => Self::error_expr(span),
            _ => {
                let message = format!("`?` applies to `T?` or `T ! E`, not {}", self.display(value.ty));
                self.diags.error("E1301", span, message);
                Self::error_expr(span)
            }
        }
    }

    /// For an editor: a place where the function being checked may leave,
    /// but not a closure in it, which leaves only itself.
    fn note_exit(&mut self, cx: &FnCx, at: Span) {
        if cx.parent.is_none()
            && let (Some(function), Some(index)) = (self.leaving, self.index.as_mut())
        {
            index.exits.push((function, at));
        }
    }

    /// For an editor's hover: what a `?` or a `fail` does with a value of
    /// type `ty` where it is written.
    fn note_propagation(&mut self, cx: &FnCx, ty: Ty, at: Span, what: &str) {
        if self.index.is_none() || ty == Ty::ERROR {
            return;
        }
        let from = match (cx.parent.is_some(), cx.infer_error) {
            (true, _) => "the closure".to_string(),
            (false, true) if self.naming == "main" => "`main`".to_string(),
            _ => format!("`{}`", self.naming),
        };
        // What the error does once out of the function.
        let error = match self.types.kind(ty).clone() {
            TyKind::Result(_, error) => Some(error),
            TyKind::Option(_) => None,
            _ => Some(ty),
        };
        let then = match cx.error {
            _ if cx.infer_error && cx.parent.is_none() && self.naming == "main" => {
                ", which ends the program: it prints `error: ` and the error's `Display`, and exits with 1".to_string()
            }
            Some(channel) if Some(channel) != error => format!(", which fails with `{}`", self.types.display(channel)),
            _ => String::new(),
        };
        let note = match (what, self.types.kind(ty).clone()) {
            ("fail", _) => format!("`fail` ends {from} with the `{}`{then}", self.types.display(ty)),
            (_, TyKind::Option(payload)) => {
                format!("`?` gives the `{}`, or returns `none` from {from}", self.types.display(payload))
            }
            (_, TyKind::Result(ok, Ty::NEVER)) => {
                format!("`?` gives the `{}`: what it applies to cannot fail", self.types.display(ok))
            }
            (_, TyKind::Result(ok, error)) => format!(
                "`?` gives the `{}`, or returns the `{}` from {from}{then}",
                self.types.display(ok),
                self.types.display(error)
            ),
            _ => return,
        };
        if let Some(index) = self.index.as_mut() {
            index.notes.push((at, note));
        }
    }

    /// The prelude's `Failure` (15.10).
    fn failure_type(&self) -> Option<Ty> {
        match self.type_names.get("Failure") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A map type; its entry tuple `(K, V)` is interned too, for iteration.
    fn map_type(&mut self, key: Ty, value: Ty) -> Ty {
        self.types.intern(TyKind::Tuple(vec![key, value]));
        self.types.intern(TyKind::Option(value));
        self.types.intern(TyKind::Map(key, value))
    }

    fn require_key(&mut self, ty: Ty, span: Span) {
        if !self.types.is_key(ty) {
            let message = format!("{} cannot be a map key or set element: it is not `Key`", self.display(ty));
            self.diags.error("E1105", span, message);
        }
    }

    fn map_literal(
        &mut self,
        cx: &mut FnCx,
        pairs: &[(ast::Expr, ast::Expr)],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let (mut key, mut value) = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Map(key, value)) => (Some(*key), Some(*value)),
            _ => (None, None),
        };
        let mut entries = Vec::new();
        let mut constants: Vec<(hir::Literal, Span)> = Vec::new();
        for (k, v) in pairs {
            let k = self.expr(cx, k, key);
            let k = match key {
                Some(key) => self.coerce(k, key),
                None => {
                    key = Some(k.ty);
                    k
                }
            };
            if let Some(constant) = literal(&k) {
                if constants.iter().any(|(c, _)| *c == constant) {
                    self.diags.error("E1116", k.span, "this key appears more than once in the literal");
                }
                constants.push((constant, k.span));
            }
            let v = self.expr(cx, v, value);
            let v = match value {
                Some(value) => self.coerce(v, value),
                None => {
                    value = Some(v.ty);
                    v
                }
            };
            entries.push((k, v));
        }
        let (Some(key), Some(value)) = (key, value) else {
            self.diags.error("E1101", span, "an empty map literal needs an expected `Map[K, V]` type");
            return Self::error_expr(span);
        };
        self.require_key(key, span);
        let ty = self.map_type(key, value);
        Self::mk(hir::ExprKind::Map(entries), ty, span)
    }

    fn set_literal(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Set(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        let mut constants = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            if let Some(constant) = literal(&value) {
                if constants.contains(&constant) {
                    self.diags.warning("W1011", value.span, "this entry appears earlier in the set: remove it");
                }
                constants.push(constant);
            }
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty set literal needs an expected `Set[T]` type");
            return Self::error_expr(span);
        };
        self.require_key(element, span);
        let ty = self.types.intern(TyKind::Set(element));
        Self::mk(hir::ExprKind::Set(values), ty, span)
    }

    fn not_indexable(&mut self, base: hir::Expr, span: Span) -> hir::Expr {
        if base.ty != Ty::ERROR {
            let message = format!("{} cannot be indexed", self.display(base.ty));
            self.diags.error("E1102", base.span, message);
        }
        Self::error_expr(span)
    }

    fn array_literal(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Array(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty array literal needs an expected `Array[T]` type");
            return Self::error_expr(span);
        };
        let ty = self.types.intern(TyKind::Array(element));
        Self::mk(hir::ExprKind::Array(values), ty, span)
    }

    /// `value.method[T](...)`: a method's own type parameters, given; its
    /// type's come from the value.
    #[allow(clippy::too_many_arguments)]
    fn generic_method_call(
        &mut self,
        cx: &mut FnCx,
        receiver: &ast::Expr,
        method: &ast::Ident,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, receiver, None);
        let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let key = self.owner_key(value.ty);
        let Some(&id) = self.methods.get(&(key, method.name.clone())) else {
            let message = format!(
                "{} has no method `{}` with type parameters of its own: call it as `value.{}(...)`",
                self.display(value.ty),
                method.name,
                method.name
            );
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        };
        self.index_refer(method, crate::index::Key::Fn(id.0));
        self.check_method_visible(id, method.span);
        let signature = self.signatures[id.0 as usize].clone();
        // The owner's own parameters come first: a generic type's, or an
        // intrinsic one's, as `T` of `Array[T]`.
        let owner = match self.types.instance_of(key) {
            Some((template, _)) => self.types.template(template).params.len(),
            None => match self.types.kind(value.ty) {
                TyKind::Array(_) | TyKind::Deque(_) | TyKind::Set(_) | TyKind::Option(_) => 1,
                TyKind::Map(..) | TyKind::Result(..) => 2,
                _ => 0,
            },
        };
        let own = &signature.generics[owner.min(signature.generics.len())..];
        if own.len() != types.len() {
            let message = match own.len() {
                0 => format!("`{0}` takes no type arguments: call it as `value.{0}(...)`", method.name),
                n => format!("`{}` takes {n} type argument(s), and {} are given", method.name, types.len()),
            };
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        }
        let prebound = own.iter().copied().zip(types).collect();
        self.dot_call(cx, Callee::Fn(id), signature, prebound, value, method, args, expected, span)
    }

    /// Type arguments given to what takes none: a path of an unknown name,
    /// as a module not imported, says so; any other, that it takes none.
    fn not_generic(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) -> hir::Expr {
        let first = segments[0].name.as_str();
        if self.lookup(cx, first).is_some()
            || self.visible_capture(cx, first)
            || self.names.contains_key(first)
            || self.intrinsic_owner(first).is_some()
            || self.type_named(first).is_some()
        {
            let path = segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".");
            let message = format!("`{path}` takes no type arguments: call it as `{path}(...)`");
            self.diags.error("E1104", span, message);
        } else {
            self.unknown_name(cx, first, segments[0].span);
        }
        Self::error_expr(span)
    }

    /// `Array[T]()` and `Array.withCapacity[T](capacity)`.
    fn generic_call(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        // `value.cast[U]()` of any expression, such as a call's result.
        if let ast::ExprKind::Field(receiver, method) = &base.kind
            && matches!(method.name.as_str(), "cast" | "toPointer" | "toFunction")
        {
            let receiver = self.expr(cx, receiver, None);
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.raw(cx, receiver, method, types, args, span);
        }
        if let ast::ExprKind::Field(receiver, method) = &base.kind {
            return self.generic_method_call(cx, receiver, method, type_args, args, expected, span);
        }
        let ast::ExprKind::Path(segments) = &base.kind else {
            return self.unsupported_expr(span, "generic calls");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        // `pointer.cast[U]()`, `address.toPointer[T]()` and their qualified
        // forms `Pointer.cast[U](pointer)` and `UInt64.toPointer[T](address)`.
        // `record.field.cast[U]()`: a receiver of several segments.
        if names.len() > 2
            && matches!(names[names.len() - 1], "cast" | "toPointer" | "toFunction")
            && (self.lookup(cx, names[0]).is_some()
                || self.visible_capture(cx, names[0])
                || matches!(self.names.get(names[0]), Some(ModuleName::Const(_))))
        {
            let (method, prefix) = segments.split_last().unwrap();
            let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
            let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
            let receiver = self.expr(cx, &receiver, None);
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.raw(cx, receiver, method, types, args, span);
        }
        if let [first, method @ ("cast" | "toPointer" | "toFunction")] = names.as_slice() {
            let raw = match *first {
                "Pointer" | "UInt" | "UInt64" if self.lookup(cx, first).is_none() => None,
                _ if self.lookup(cx, first).is_some() || self.visible_capture(cx, first) => {
                    let receiver =
                        ast::Expr { kind: ast::ExprKind::Path(vec![segments[0].clone()]), span: segments[0].span };
                    Some(self.expr(cx, &receiver, None))
                }
                _ => return self.not_generic(cx, segments, span),
            };
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let method = ast::Ident { name: (*method).to_string(), span: segments[1].span };
            return match raw {
                Some(receiver) => self.raw(cx, receiver, &method, types, args, span),
                None => {
                    let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                    let receiver = self.expr(cx, &first.value, None);
                    self.raw(cx, receiver, &method, types, rest, span)
                }
            };
        }
        // `value.method[T](...)` of a local value, as `value.field.method[T](...)`.
        if names.len() >= 2
            && (self.lookup(cx, names[0]).is_some()
                || self.visible_capture(cx, names[0])
                || matches!(self.names.get(names[0]), Some(ModuleName::Const(_))))
        {
            let (method, prefix) = segments.split_last().unwrap();
            let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
            let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
            return self.generic_method_call(cx, &receiver, method, type_args, args, expected, span);
        }
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
            && !self.type_params.iter().any(|(name, _)| name == trait_name)
        {
            let explicit: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            self.index_refer(&segments[0], crate::index::Key::Trait(trait_id.0));
            return self.trait_call(cx, trait_id, &segments[1], explicit, args, expected, span);
        }
        if let [name] = names.as_slice()
            && self.lookup(cx, name).is_none()
            && let Some(ModuleName::Fn(id)) = self.names.get(*name).copied()
        {
            let explicit = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let signature = self.signatures[id.0 as usize].clone();
            let target = CallTarget { explicit, ..CallTarget::new(Callee::Fn(id), signature, expected) };
            return self.call_signature(cx, target, None, args, span);
        }
        if let ["channel"] = names.as_slice()
            && self.lookup(cx, "channel").is_none()
        {
            return self.channel(cx, type_args, args, span);
        }
        if let ["Ref"] = names.as_slice()
            && self.lookup(cx, "Ref").is_none()
        {
            let [value] = type_args else {
                self.diags.error("E1104", span, "`Ref` takes one type argument: `Ref[T](value)`");
                return Self::error_expr(span);
            };
            let value = self.resolve_type(value);
            return self.ref_new(cx, args, Some(value), expected, span);
        }
        // `memory.take[T](pointer)`: the value, moved out of the memory.
        if let [module, "take"] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalMemory)) = self.names.get(*module)
        {
            let ([ty], [arg]) = (type_args, args) else {
                self.diags.error("E1104", span, "`take` takes one type argument and a pointer");
                return Self::error_expr(span);
            };
            let ty = self.resolve_type(ty);
            let pointer = self.types.intern(TyKind::Pointer(ty));
            let value = self.expr(cx, &arg.value, Some(pointer));
            let value = self.coerce(value, pointer);
            if !cx.unsafe_region {
                self.diags.error("E1701", span, "`take` is a raw operation: write it inside `unsafe { ... }`");
            }
            return Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Take, vec![value]), ty, span);
        }
        // `memory.sizeOf[T]()` and `memory.alignOf[T]()`, which `std.ffi` exposes.
        if let [module, query @ ("sizeOf" | "alignOf")] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalMemory)) = self.names.get(*module)
        {
            let [ty] = type_args else {
                self.diags.error("E1104", span, format!("`{query}` takes one type argument"));
                return Self::error_expr(span);
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let ty = self.resolve_type(ty);
            if self.types.c_layout(ty).is_some_and(|l| l.opaque) {
                let message = format!("{} is opaque: C alone knows its size", self.display(ty));
                self.diags.error("E1703", span, message);
            }
            return Self::mk(hir::ExprKind::Layout(ty, *query == "alignOf"), Ty::INT, span);
        }
        // `native.unbox[T](box)`, `native.is[T](box)` and `native.typeName[T]()`
        // of `std.internal.reflect`, which `std.reflect` is written on.
        if let [module, op @ ("unbox" | "is" | "typeName" | "identity" | "capabilities")] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalReflect)) = self.names.get(*module)
        {
            let boxed = self.types.handle(Handle::ReflectBox, Ty::UNIT);
            if let Some((op, result)) = match *op {
                "typeName" => Some((hir::ReflectOp::Name, Ty::STRING)),
                "identity" => Some((hir::ReflectOp::Identity, Ty::INT)),
                "capabilities" => Some((hir::ReflectOp::Capabilities, Ty::INT)),
                _ => None,
            } {
                let [ty] = type_args else {
                    self.diags.error("E1104", span, "this operation takes one type argument");
                    return Self::error_expr(span);
                };
                let ty = self.resolve_type(ty);
                return Self::mk(hir::ExprKind::Reflect(op, ty, Vec::new()), result, span);
            }
            let [arg] = args else {
                return self.arity_error(span, 1, args.len());
            };
            let [ty] = type_args else {
                self.diags.error("E1104", span, format!("`{op}` takes one type argument"));
                return Self::error_expr(span);
            };
            let ty = self.resolve_type(ty);
            let value = self.expr(cx, &arg.value, Some(boxed));
            let value = self.coerce(value, boxed);
            let (op, result) =
                if *op == "unbox" { (hir::ReflectOp::Unbox, ty) } else { (hir::ReflectOp::Is, Ty::BOOL) };
            return Self::mk(hir::ExprKind::Reflect(op, ty, vec![value]), result, span);
        }
        if let Some(retired) = self.retired_new(cx, segments, span) {
            return retired;
        }
        if let [module, _] = names.as_slice()
            && args.is_empty()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::Sync)) = self.names.get(*module)
        {
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.sync_empty(&segments[1], types, args, span);
        }
        if let ["Group"] = names.as_slice() {
            let [result] = type_args else {
                self.diags.error("E1104", span, "`Group` takes one type argument, the tasks' result");
                return Self::error_expr(span);
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let result = self.resolve_type(result);
            let ty = self.types.handle(Handle::Group, result);
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::GroupNew, Vec::new()), ty, span);
        }
        if let ["Map" | "Set"] = names.as_slice() {
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let ty = match (names[0], types.as_slice()) {
                ("Map", [key, value]) => {
                    self.require_key(*key, span);
                    self.map_type(*key, *value)
                }
                ("Set", [element]) => {
                    self.require_key(*element, span);
                    self.types.intern(TyKind::Set(*element))
                }
                _ => {
                    self.diags.error("E1104", span, "wrong number of type arguments");
                    return Self::error_expr(span);
                }
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            return Self::mk(hir::ExprKind::MapOp(hir::MapOp::New, Vec::new()), ty, span);
        }
        let (op, params) = match names.as_slice() {
            ["Array" | "Deque"] => (hir::ArrayOp::New, Vec::new()),
            ["Array", "withCapacity"] => (hir::ArrayOp::WithCapacity, vec![Ty::INT]),
            _ => return self.not_generic(cx, segments, span),
        };
        let [element] = type_args else {
            self.diags.error("E1104", span, format!("`{}` takes one type argument", names[0]));
            return Self::error_expr(span);
        };
        let element = self.resolve_type(element);
        let array = if names[0] == "Deque" {
            self.types.intern(TyKind::Deque(element))
        } else {
            self.types.intern(TyKind::Array(element))
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let values = args
            .iter()
            .zip(params)
            .map(|(a, t)| {
                let v = self.expr(cx, &a.value, Some(t));
                self.coerce(v, t)
            })
            .collect();
        let ty = match op {
            hir::ArrayOp::New => array,
            _ => self.types.intern(TyKind::Result(array, self.collection_error)),
        };
        Self::mk(hir::ExprKind::ArrayOp(op, values), ty, span)
    }

    /// `receiver.method(args)` on an intrinsic collection (std.collections).
    fn method_call(
        &mut self,
        cx: &mut FnCx,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, receiver, None);
        self.method_of(cx, value, method, args, expected, span)
    }

    /// `Array.concat(a, b)`: a method the language gives, with its receiver
    /// written first and the loan of one that changes it in sight (11.5).
    #[allow(clippy::too_many_arguments)]
    fn language_call(
        &mut self,
        cx: &mut FnCx,
        owner: Ty,
        receiver: &ast::Arg,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let changes = MUTATIONS.contains(&method.name.as_str());
        let value = self.expr(cx, &receiver.value, None);
        if self.owner_key(value.ty) != owner && !self.types.has_error(value.ty) {
            let message = format!("expected {}, found {}", self.display(owner), self.display(value.ty));
            self.diags.error("E1102", value.span, message);
            return Self::error_expr(span);
        }
        match (changes, receiver.mode) {
            (true, ast::Mode::Var) | (false, ast::Mode::Value) => {}
            (true, _) => {
                let text = self.sources.file_of(value.span.start).slice(value.span);
                let message = format!("`{}` changes its receiver: write `var {text}`", method.name);
                self.diags.error("E1407", receiver.span, message);
            }
            (false, mode) => {
                let message = format!("the receiver `self` is already a shared loan: remove `{}`", mode.keyword());
                self.diags.error("E1407", receiver.span, message);
            }
        }
        self.method_of(cx, value, method, args, expected, span)
    }

    /// `method` called on a value already checked.
    fn method_of(
        &mut self,
        cx: &mut FnCx,
        value: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = value.ty;
        if self.is_http_route(ty, method, args) {
            return self.http_route(cx, value, method, args, expected, span);
        }
        // The named operations of a number (18.9), and of any integer on a
        // `T: Integer` (18.10).
        let entry = match self.number_kind(ty) {
            Some(kind) => crate::numbers::find(kind, &method.name, true),
            None if self.bound_by(ty, self.integer) => crate::numbers::find_integer(&method.name, true),
            None => None,
        };
        if let Some(entry) = entry {
            let signature = self.number_signature(ty, entry);
            let callee = Callee::Std(StdFn::Number(entry.op));
            return self.dot_call(cx, callee, signature, HashMap::new(), value, method, args, expected, span);
        }
        let key = self.owner_key(ty);
        if let Some(&id) = self.methods.get(&(key, method.name.clone())) {
            self.index_refer(method, crate::index::Key::Fn(id.0));
            self.check_method_visible(id, method.span);
            let signature = self.signatures[id.0 as usize].clone();
            return self.dot_call(cx, Callee::Fn(id), signature, HashMap::new(), value, method, args, expected, span);
        }
        if self.is_abstract(ty) {
            // A type parameter has the methods of the traits it is bound by (12.7).
            let bounds = self.bounds_of(ty);
            let found: Vec<(TraitId, Vec<Ty>, usize)> = bounds
                .iter()
                .filter_map(|(t, a)| {
                    let index = self.traits[t.0 as usize].methods.iter().position(|m| m.name == method.name)?;
                    Some((*t, a.clone(), index))
                })
                .collect();
            return match found.as_slice() {
                [(trait_id, trait_args, index)] => {
                    self.trait_dot_call(cx, *trait_id, trait_args.clone(), *index, value, method, args, expected, span)
                }
                [] => self.no_method(ty, method, span),
                _ => {
                    let names: Vec<String> = found
                        .iter()
                        .map(|(t, ..)| format!("`{}.{}(value)`", self.traits[t.0 as usize].name, method.name))
                        .collect();
                    let message =
                        format!("`{}` comes from more than one bound; write {}", method.name, names.join(" or "));
                    self.diags.error("E1004", method.span, message);
                    Self::error_expr(span)
                }
            };
        }
        // Inside an `impl`, the implemented trait's methods on `Self` (11.5).
        if let Some((trait_id, trait_args)) = self.implementing.clone()
            && Some(ty) == self.self_type
            && let Some(index) = self.traits[trait_id.0 as usize].methods.iter().position(|m| m.name == method.name)
        {
            return self.trait_dot_call(cx, trait_id, trait_args, index, value, method, args, expected, span);
        }
        // `Iterator`'s methods on any type implementing it (11.5).
        if !self.is_abstract(ty)
            && let Some(index) = self.traits[ITERATOR.0 as usize].methods.iter().position(|m| m.name == method.name)
            && let Some(element) = self.iterator_element(ty)
        {
            return self.trait_dot_call(cx, ITERATOR, vec![element], index, value, method, args, expected, span);
        }
        // A trait's method on a concrete value, when one trait gives it (12.7);
        // those of the type itself and of the language go first.
        if !self.is_abstract(ty) && !self.language_method(ty, &method.name) {
            let givers = self.traits_with(ty, &method.name);
            match givers.as_slice() {
                [] => {}
                [giver] => {
                    let index = self.traits[giver.0 as usize].methods.iter().position(|m| m.name == method.name);
                    let index = index.expect("a trait that has the method");
                    return self.trait_dot_call(cx, *giver, Vec::new(), index, value, method, args, expected, span);
                }
                _ => {
                    let name = |t: &TraitId| format!("`{}.{}(value)`", self.traits[t.0 as usize].name, method.name);
                    let names: Vec<String> = givers.iter().map(name).collect();
                    let message =
                        format!("`{}` comes from more than one trait; write {}", method.name, names.join(" or "));
                    self.diags.error("E1004", method.span, message);
                    return Self::error_expr(span);
                }
            }
        }
        // `DateTime`'s methods, written in Tondo (14.3.4): the functions of
        // `std.time`'s companions that take a `DateTime` first.
        if Some(ty) == self.datetime
            && let Some(id) = Self::companions(StdModule::Time).iter().find_map(|companion| {
                match self.names.get(&format!("{companion}.{}", method.name)) {
                    Some(&ModuleName::Fn(id)) if self.signatures[id.0 as usize].params.first() == Some(&ty) => Some(id),
                    _ => None,
                }
            })
        {
            let signature = self.signatures[id.0 as usize].clone();
            let target = CallTarget::new(Callee::Fn(id), signature, expected);
            return self.call_signature(cx, target, Some(value), args, span);
        }
        if Some(ty) == self.duration || Some(ty) == self.instant {
            return self.time_method(cx, value, method, args, span);
        }
        if matches!(self.types.kind(ty), TyKind::Pointer(_))
            || (ty == Ty::int(IntTy::U64) && matches!(method.name.as_str(), "toPointer" | "toFunction"))
        {
            return self.raw(cx, value, method, Vec::new(), args, span);
        }
        if let Some(types) = self.process
            && ty == types.result
            && method.name == "text"
        {
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let stdout = self.field_access(value, FieldRef::Name("stdout"), method.span, span);
            let result = self.types.intern(TyKind::Result(Ty::STRING, types.error));
            let callee = Callee::Std(StdFn::Process(hir::ProcessFn::Text));
            return Self::mk(hir::ExprKind::Call(callee, Vec::new(), vec![stdout]), result, span);
        }
        if matches!(self.types.kind(ty), TyKind::Adt(_)) {
            // A field holding a function is called (11.5).
            if self.record_has_field(ty, &method.name) {
                let field = self.field_access(value, FieldRef::Name(&method.name), method.span, span);
                if self.callable(field.ty).is_some() {
                    return self.call_value(cx, field, args, None, span);
                }
                return self.no_method(ty, method, span);
            }
            return self.no_method(ty, method, span);
        }
        if MUTATIONS.contains(&method.name.as_str())
            && matches!(self.types.kind(ty), TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_))
        {
            return self.mutation_call(cx, value, method, args, span);
        }
        if ty == Ty::STRING {
            return self.string_method(cx, value, method, args, span);
        }
        if let TyKind::Handle(handle, state) = *self.types.kind(ty) {
            if handle == Handle::File {
                return self.file_method(cx, value, method, args, span);
            }
            if matches!(handle, Handle::Writer | Handle::Reader | Handle::Buffer) {
                return self.io_method(cx, value, handle, method, args, span);
            }
            if matches!(handle, Handle::Command | Handle::Pipeline | Handle::Process) {
                return self.process_method(cx, value, handle, method, args, span);
            }
            if matches!(handle, Handle::JsonReader | Handle::JsonWriter | Handle::Regex) {
                let receiver = match handle {
                    Handle::JsonReader => hir::NativeTy::JsonReader,
                    Handle::JsonWriter => hir::NativeTy::JsonWriter,
                    _ => hir::NativeTy::Regex,
                };
                let found = hir::NATIVES.iter().position(|n| n.receiver == Some(receiver) && n.name == method.name);
                let Some(index) = found else { return self.no_method(ty, method, span) };
                let native = &hir::NATIVES[index];
                if args.len() != native.params.len() {
                    return self.arity_error(span, native.params.len(), args.len());
                }
                let mut values = vec![value];
                for (arg, &param) in args.iter().zip(native.params) {
                    let param = self.native_ty(param);
                    let argument = self.expr(cx, &arg.value, Some(param));
                    values.push(self.coerce(argument, param));
                }
                let result = self.native_ty(native.result);
                let callee = Callee::Std(StdFn::Native(index as u16));
                return Self::mk(hir::ExprKind::Call(callee, Vec::new(), values), result, span);
            }
            if handle == Handle::Clock {
                return self.clock_method(cx, value, method, args, span);
            }
            // A cancelled task's `defer` ends at a wait: a lock another holds.
            if cx.deferred && matches!(handle, Handle::Mutex | Handle::RwLock) && method.name != "tryUpdate" {
                let message = "in a cancelled task this `defer` ends here if another task holds the lock, and what it \
                               does is lost: use an `Atomic`, or send to a channel with room, which need no wait";
                self.diags.warning("W1017", span, message);
                let receiver = Span::new(span.start, method.span.start.saturating_sub(1));
                if let Some(edits) = self.as_counter(self.sources.slice(receiver), span) {
                    self.diags.fix("make it a counter: an `Atomic`, which needs no lock", false, edits);
                }
            }
            if handle.locked() || matches!(handle, Handle::Semaphore | Handle::Permit | Handle::Barrier) {
                let cell = Self::cell(cx, &value)
                    .filter(|_| matches!(handle, Handle::Mutex | Handle::RwLock | Handle::Atomic));
                let checked = self.sync_method(cx, value, handle, state, method, args, span);
                if let Some(cell) = cell {
                    self.cell_use(cell, method, args, span);
                }
                return checked;
            }
            return self.handle_method(cx, value, handle, state, method, args, span);
        }
        let kind = self.types.kind(ty).clone();
        let (element, sequence) = match kind {
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => {
                (element, !matches!(kind, TyKind::Set(_)))
            }
            TyKind::Map(key, _) => (key, false),
            TyKind::Error => return Self::error_expr(span),
            _ => return self.no_method(ty, method, span),
        };
        if let Some(cursor) = match (method.name.as_str(), &kind) {
            ("entries", TyKind::Map(..)) => Some(CursorKind::Entries),
            ("keys", TyKind::Map(..)) => Some(CursorKind::Keys),
            ("values", TyKind::Map(..) | TyKind::Set(_)) => Some(CursorKind::Values),
            _ => None,
        } {
            // A lazy cursor over the collection, in insertion order (10.2.3).
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let (set, args) = match kind {
                TyKind::Set(element) => (true, vec![element]),
                TyKind::Map(key, value) => (false, vec![key, value]),
                _ => unreachable!("a map or set"),
            };
            let template = self.cursor_templates[&(cursor, set)];
            let cursor = self.types.instantiate(template, args);
            let start = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
            return Self::mk(hir::ExprKind::Record(vec![(0, value), (1, start)]), cursor, span);
        }
        match method.name.as_str() {
            "length" | "isEmpty" => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                let length = Self::mk(hir::ExprKind::Length(Box::new(value)), Ty::INT, span);
                if method.name == "length" {
                    return length;
                }
                let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                let kind = hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(zero));
                return Self::mk(kind, Ty::BOOL, span);
            }
            "get" | "contains" if !sequence => {
                let result = match kind {
                    TyKind::Map(_, entry) if method.name == "get" => self.types.intern(TyKind::Option(entry)),
                    _ if method.name == "contains" => Ty::BOOL,
                    _ => return self.no_method(ty, method, span),
                };
                let op = if method.name == "get" { hir::MapOp::Get } else { hir::MapOp::Contains };
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let arg = self.expr(cx, &arg.value, Some(element));
                let arg = self.coerce(arg, element);
                return Self::mk(hir::ExprKind::MapOp(op, vec![value, arg]), result, span);
            }
            _ => {}
        }
        let is_array = matches!(kind, TyKind::Array(_));
        let (op, params, result) = match method.name.as_str() {
            "get" if sequence => (hir::ArrayOp::Get, vec![Ty::INT], self.types.intern(TyKind::Option(element))),
            "slice" if is_array => {
                let result = self.types.intern(TyKind::Result(ty, self.collection_error));
                (hir::ArrayOp::SliceChecked, vec![Ty::INT, Ty::INT], result)
            }
            "concat" if is_array => (hir::ArrayOp::Concat, vec![ty], ty),
            "repeat" if is_array => (hir::ArrayOp::Repeat, vec![Ty::INT], ty),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![value];
        for (arg, ty) in args.iter().zip(params) {
            let arg = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(arg, ty));
        }
        Self::mk(hir::ExprKind::ArrayOp(op, values), result, span)
    }

    /// The intrinsic methods of `String` (19.2, 19.6).
    fn string_method(
        &mut self,
        cx: &mut FnCx,
        text: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let char_option = self.types.intern(TyKind::Option(Ty::CHAR));
        let int_option = self.types.intern(TyKind::Option(Ty::INT));
        let checked = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
        use hir::StrOp as S;
        let (op, params, result) = match method.name.as_str() {
            "length" | "byteLength" | "isEmpty" => (S::ByteLength, vec![], Ty::INT),
            "get" => (S::Get, vec![Ty::INT], char_option),
            "concat" => (S::Concat, vec![Ty::STRING], Ty::STRING),
            "contains" => (S::Contains, vec![Ty::STRING], Ty::BOOL),
            "startsWith" => (S::StartsWith, vec![Ty::STRING], Ty::BOOL),
            "endsWith" => (S::EndsWith, vec![Ty::STRING], Ty::BOOL),
            "find" => (S::Find, vec![Ty::STRING], int_option),
            "replace" => (S::Replace, vec![Ty::STRING, Ty::STRING], Ty::STRING),
            "toLowerAscii" => (S::LowerAscii, vec![], Ty::STRING),
            "toUpperAscii" => (S::UpperAscii, vec![], Ty::STRING),
            "slice" => (S::SliceChecked, vec![Ty::INT, Ty::INT], checked),
            "bytes" => (S::Bytes, vec![], self.types.intern(TyKind::Array(Ty::BYTE))),
            _ => return self.no_method(Ty::STRING, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![text];
        for (arg, ty) in args.iter().zip(params) {
            let arg = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(arg, ty));
        }
        let kind = match method.name.as_str() {
            "length" => hir::ExprKind::Length(Box::new(values.remove(0))),
            "isEmpty" => {
                let length = Self::mk(hir::ExprKind::StrOp(S::ByteLength, values), Ty::INT, span);
                let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                let kind = hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(zero));
                return Self::mk(kind, Ty::BOOL, span);
            }
            _ => hir::ExprKind::StrOp(op, values),
        };
        Self::mk(kind, result, span)
    }

    /// The generic function a name refers to, when it is not a binding.
    fn specialization(&self, cx: &FnCx, e: &ast::Expr) -> Option<FnId> {
        let ast::ExprKind::Path(segments) = &e.kind else { return None };
        let [name] = segments.as_slice() else { return None };
        if self.lookup(cx, &name.name).is_some() || self.visible_capture(cx, &name.name) {
            return None;
        }
        match self.names.get(&name.name) {
            Some(&ModuleName::Fn(id)) if !self.signatures[id.0 as usize].generics.is_empty() => Some(id),
            _ => None,
        }
    }

    /// An expression written where a type is meant, as a type.
    fn type_of_expr(e: &ast::Expr) -> Option<ast::TypeExpr> {
        let kind = match &e.kind {
            ast::ExprKind::Path(segments) => ast::TypeKind::Path { segments: segments.clone(), args: Vec::new() },
            ast::ExprKind::Index { base, index } => {
                let ast::TypeKind::Path { segments, .. } = Self::type_of_expr(base)?.kind else { return None };
                ast::TypeKind::Path { segments, args: vec![Self::type_of_expr(index)?] }
            }
            _ => return None,
        };
        Some(ast::TypeExpr { kind, span: e.span })
    }

    /// A named function as a `fn(...)` value (11.7). A generic function is
    /// specialized by explicit arguments or by the expected function type.
    fn fn_value(&mut self, id: FnId, explicit: Vec<Ty>, expected: Option<Ty>, span: Span) -> hir::Expr {
        self.repeatable(id, span);
        let signature = self.signatures[id.0 as usize].clone();
        if signature.receiver.is_some() {
            let message = format!(
                "a method is not a value; write a closure that calls it, such as `(x) {{ value.{}(x) }}`",
                signature.name.rsplit('.').next().unwrap_or_default()
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let mut inference = Inference::new(signature.generics.clone());
        if !explicit.is_empty() {
            if explicit.len() != signature.generics.len() {
                let message =
                    format!("expected {} type argument(s), found {}", signature.generics.len(), explicit.len());
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            inference.map.extend(signature.generics.iter().copied().zip(explicit));
        }
        let generic = self.types.intern(TyKind::Fn(
            signature.params.clone(),
            signature.result,
            signature.variadic,
            signature.modes.clone(),
            signature.is_unsafe,
        ));
        if let Some(expected) = expected {
            self.unify(&mut inference, generic, expected);
        }
        let Some(ty) = self.inferred(&inference, generic) else {
            let message = format!(
                "`{}` is generic: specialize it, as in `{}[Int]`, or give the expected `fn(...)` type",
                signature.name, signature.name
            );
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        };
        let type_args = signature.generics.iter().map(|g| inference.map[g]).collect();
        Self::mk(hir::ExprKind::FnValue(id, type_args), ty, span)
    }

    /// `value(args)` for a `fn(...)`, a closure or a type parameter with a
    /// call bound (11.7, 11.8). Arguments are positional.
    fn call_value(
        &mut self,
        cx: &mut FnCx,
        callee: hir::Expr,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if callee.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        if let TyKind::CFn(params, result, variadic) = self.types.kind(callee.ty).clone() {
            return self.c_value_call(cx, callee, (params, result, variadic), args, expected, span);
        }
        let Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe }) = self.callable(callee.ty)
        else {
            let message = format!("{} cannot be called", self.display(callee.ty));
            self.diags.error("E1102", callee.span, message);
            return Self::error_expr(span);
        };
        if is_unsafe && !cx.unsafe_region {
            let message =
                "this function value is `unsafe`: call it inside `unsafe { ... }`, once its preconditions hold";
            self.diags.error("E1701", span, message);
        }
        if mutates && self.shared_place(cx, &callee) && !self.types.is_copy(callee.ty) {
            let text = self.sources.file_of(callee.span.start).slice(callee.span);
            let message = match once {
                true => format!("`{text}` is a shared loan, and calling it would consume it: take it by value"),
                false => format!("`{text}` is a shared loan, and calling it can change it: take it as `mut`"),
            };
            self.diags.error("E1407", callee.span, message);
            return Self::error_expr(span);
        }
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            self.diags.error("E1102", arg.span, "a call through a function value takes positional arguments");
            return Self::error_expr(span);
        }
        let names = (0..params.len()).map(|i| format!("argument{}", i + 1)).collect();
        let signature = Signature {
            params,
            names,
            receiver: None,
            modes,
            variadic,
            result,
            generics: Vec::new(),
            name: "this function".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        // A placeholder callee; the checked arguments are what matters.
        let target = CallTarget::new(Callee::Fn(FnId(u32::MAX)), signature, expected);
        let call = self.call_signature(cx, target, None, args, span);
        let hir::ExprKind::Call(_, _, values) = call.kind else { return call };
        // A closure is called through a place it may change; a `var` binding
        // keeps the changes, and anything else is copied first (11.8).
        let direct = matches!(self.types.kind(callee.ty), TyKind::Fn(..));
        let keep = !direct && self.var_place(cx, &callee) && mutates || !direct && !mutates && self.is_place(&callee);
        if direct || keep {
            return Self::mk(hir::ExprKind::CallValue(Box::new(callee), values), call.ty, span);
        }
        let local = self.hidden_local(cx, callee.ty);
        let (ty, callee_span, result) = (callee.ty, callee.span, call.ty);
        let read = Self::mk(hir::ExprKind::Local(local), ty, callee_span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), values), result, span);
        let block = hir::Block { stmts: vec![hir::Stmt::Let(local, callee)], tail: Some(Box::new(call)), ty: result };
        Self::mk(hir::ExprKind::Block(block), result, span)
    }

    /// Whether a value expression reads a place.
    fn is_place(&self, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(_) => true,
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.is_place(base)
            }
            _ => false,
        }
    }

    /// Whether a value expression reads a place of a `var` binding.
    fn var_place(&self, cx: &FnCx, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(local) => cx.mutable[local.0 as usize] && Some(*local) != cx.env,
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.var_place(cx, base)
            }
            _ => false,
        }
    }

    /// Whether `value` is reached through a `ref` parameter.
    fn shared_place(&self, cx: &FnCx, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(local) => {
                cx.params[local.0 as usize] == Some(ast::Mode::Ref) && Some(*local) != cx.env
            }
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.shared_place(cx, base)
            }
            _ => false,
        }
    }

    /// `App()`, which is `App()`: a type is built by calling it (11.5). A
    /// type of the program may have a `new` of its own.
    fn retired_new(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) -> Option<hir::Expr> {
        let [prefix @ .., owner, last] = segments else { return None };
        if last.name != "new" || self.lookup(cx, &segments[0].name).is_some() {
            return None;
        }
        let ty = match prefix {
            [] => self.type_named(&owner.name),
            [module] => self.module_type(&module.name, &owner.name),
            _ => None,
        };
        if let Some(ty) = ty {
            let key = self.owner_key(ty);
            if self.methods.contains_key(&(key, "new".to_string())) {
                return None;
            }
        }
        let qualified = !prefix.is_empty() || owner.name.contains('.');
        if !qualified && !matches!(owner.name.as_str(), "Array" | "Deque" | "Group" | "Map" | "Set") {
            return None;
        }
        let name = Self::path_text(&segments[..segments.len() - 1]);
        let message = format!("a type is built by calling it: `{name}(...)`, without `.new`");
        self.diags.error("E1001", last.span, message);
        let dot = Span::new(owner.span.end, last.span.end);
        self.diags.fix(format!("write `{name}(...)`"), true, vec![(dot, String::new())]);
        Some(Self::error_expr(span))
    }

    /// `value.method(args)` for method `index` of a trait with `Self` set to
    /// the receiver's type.
    #[allow(clippy::too_many_arguments)]
    fn trait_dot_call(
        &mut self,
        cx: &mut FnCx,
        trait_id: TraitId,
        trait_args: Vec<Ty>,
        index: usize,
        value: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        self.index_refer(method, crate::index::Key::TraitMethod(trait_id.0, method.name.clone()));
        // The prelude's methods of an iterator copy it (10.17).
        if trait_id == ITERATOR && index > 0 && !self.types.is_copy(value.ty) && !self.types.has_error(value.ty) {
            let message = format!(
                "`{}` copies the iterator, and {} is not `Copy`: go through it with `for`",
                method.name,
                self.display(value.ty)
            );
            self.diags.error("E1406", method.span, message);
            return Self::error_expr(span);
        }
        let def = &self.traits[trait_id.0 as usize];
        let signature = def.methods[index].signature.clone();
        let mut prebound = HashMap::from([(def.self_param, value.ty)]);
        prebound.extend(def.params.iter().copied().zip(trait_args));
        let callee = Callee::Trait(trait_id, index as u32);
        self.dot_call(cx, callee, signature, prebound, value, method, args, expected, span)
    }

    /// `Trait.method(...)`, with `Self` given first in `explicit` or taken
    /// from the receiver argument (12.7).
    #[allow(clippy::too_many_arguments)]
    fn trait_call(
        &mut self,
        cx: &mut FnCx,
        trait_id: TraitId,
        method: &ast::Ident,
        explicit: Vec<Ty>,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        self.index_refer(method, crate::index::Key::TraitMethod(trait_id.0, method.name.clone()));
        let def = &self.traits[trait_id.0 as usize];
        let Some(index) = def.methods.iter().position(|m| m.name == method.name) else {
            let message = format!("trait `{}` has no method `{}`", def.name, method.name);
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        };
        let signature = def.methods[index].signature.clone();
        // `Trait.op[Self, Own...]`: the trait's parameters come from its bounds.
        let mut prebound = HashMap::new();
        if let Some((&self_ty, own)) = explicit.split_first() {
            prebound.insert(def.self_param, self_ty);
            let own_params = &signature.generics[1 + def.params.len()..];
            prebound.extend(own_params.iter().copied().zip(own.iter().copied()));
        }
        let target =
            CallTarget { prebound, ..CallTarget::new(Callee::Trait(trait_id, index as u32), signature, expected) };
        self.call_signature(cx, target, None, args, span)
    }

    /// The `for` loop over a cursor of type `ty` yielding `element` (10.17):
    /// the loop owns a copy and calls `Iterator.next` until `none`.
    fn for_cursor(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
    ) -> hir::Stmt {
        let (ty, span) = (source.ty, source.span);
        let cursor = self.hidden_local(cx, ty);
        cx.mutable[cursor.0 as usize] = true;
        let option = self.types.intern(TyKind::Option(element));
        let loan = Self::mk(hir::ExprKind::Loan(hir::Place { local: cursor, path: Vec::new() }), ty, span);
        let next =
            Self::mk(hir::ExprKind::Call(Callee::Trait(ITERATOR, 0), vec![ty, element], vec![loan]), option, span);
        self.until_none(cx, pattern, element, body, (cursor, source), next)
    }

    /// `for value in receiver` (11.13): receives until the channel is
    /// closed and empty.
    fn for_receiver(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
    ) -> hir::Stmt {
        let (ty, span) = (source.ty, source.span);
        let receiver = self.hidden_local(cx, ty);
        let option = self.types.intern(TyKind::Option(element));
        let read = Self::mk(hir::ExprKind::Local(receiver), ty, span);
        let next = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Receive, vec![read]), option, span);
        self.until_none(cx, pattern, element, body, (receiver, source), next)
    }

    /// A loop that binds `pattern` to each value `next` gives until `none`,
    /// after storing `source` in a hidden local.
    fn until_none(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        element: Ty,
        body: &ast::Block,
        (cursor, source): (LocalId, hir::Expr),
        next: hir::Expr,
    ) -> hir::Stmt {
        let span = source.span;
        let option = next.ty;
        cx.scopes.open();
        let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
        let pattern = self.pattern(cx, pattern, element, context);
        if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
            let message = format!("this pattern can fail, for example on `{missing}`");
            self.diags.error("E1201", pattern.span, message);
        }
        cx.loops.push(false);
        let body = self.block(cx, body, Some(Ty::UNIT));
        cx.loops.pop();
        cx.scopes.close();
        let pattern_span = pattern.span;
        let some = hir::Pattern { kind: hir::PatKind::Variant(SOME, vec![pattern]), ty: option, span: pattern_span };
        let none = hir::Pattern { kind: hir::PatKind::Variant(NONE, Vec::new()), ty: option, span: pattern_span };
        let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
        let arms = vec![
            hir::Arm { pattern: some, guard: None, body: Self::mk(hir::ExprKind::Block(body), Ty::UNIT, span) },
            hir::Arm { pattern: none, guard: None, body: Self::mk(hir::ExprKind::Block(stop), Ty::NEVER, span) },
        ];
        let step = Self::mk(hir::ExprKind::Match(Box::new(next), arms), Ty::UNIT, span);
        let body = hir::Block { stmts: vec![hir::Stmt::Expr(step)], tail: None, ty: Ty::UNIT };
        let stmts = vec![hir::Stmt::Let(cursor, source), hir::Stmt::Loop { condition: None, body }];
        hir::Stmt::Expr(Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }), Ty::UNIT, span))
    }

    /// The element type of a cursor: a type implementing `Iterator[T]` or a
    /// type parameter bound by it.
    fn iterator_element(&mut self, ty: Ty) -> Option<Ty> {
        if self.is_abstract(ty) {
            return self.bounds_of(ty).into_iter().find(|(t, _)| *t == ITERATOR).map(|(_, args)| args[0]);
        }
        let candidates: Vec<(Ty, Ty, Vec<Ty>)> = self
            .impls
            .iter()
            .filter(|i| i.trait_id == ITERATOR)
            .map(|i| (i.target, i.trait_args[0], i.generics.clone()))
            .collect();
        for (target, element, generics) in candidates {
            let mut map = HashMap::new();
            if self.types.matches(target, ty, &generics, &mut map) {
                return Some(self.types.subst(element, &map));
            }
        }
        None
    }

    /// A method that changes a collection's extent or contents; the
    /// receiver must be a `var` place (10.5).
    fn mutation_call(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::Mutation as M;
        let ty = receiver.ty;
        let operation = format!("`{}`", method.name);
        let Some(hir::Place { local, path }) = self.loan_place(cx, receiver, &operation, true) else {
            return Self::error_expr(span);
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        let (mutation, params, result) = match (method.name.as_str(), self.types.kind(ty).clone()) {
            ("push", TyKind::Array(e) | TyKind::Deque(e)) => (M::Push, vec![e], Ty::UNIT),
            ("pushFront", TyKind::Deque(e)) => (M::PushFront, vec![e], Ty::UNIT),
            ("pop", TyKind::Array(e) | TyKind::Deque(e)) => (M::Pop, vec![], option(self, e)),
            ("popFront", TyKind::Deque(e)) => (M::PopFront, vec![], option(self, e)),
            ("insert", TyKind::Array(e)) => (M::InsertAt, vec![Ty::INT, e], Ty::UNIT),
            ("removeAt", TyKind::Array(e)) => (M::RemoveAt, vec![Ty::INT], option(self, e)),
            ("clear", TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_)) => {
                (M::Clear, vec![], Ty::UNIT)
            }
            ("insert", TyKind::Map(k, v)) => (M::Insert, vec![k, v], option(self, v)),
            ("remove", TyKind::Map(k, v)) => (M::Remove, vec![k], option(self, v)),
            ("insert", TyKind::Set(k)) => (M::Insert, vec![k], Ty::BOOL),
            ("remove", TyKind::Set(k)) => (M::Remove, vec![k], Ty::BOOL),
            (_, TyKind::Error) => return Self::error_expr(span),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let values = args
            .iter()
            .zip(params)
            .map(|(a, t)| {
                let v = self.expr(cx, &a.value, Some(t));
                self.coerce(v, t)
            })
            .collect();
        Self::mk(hir::ExprKind::Mutate(mutation, hir::Place { local, path }, values), result, span)
    }

    /// `value.method(args)` for an inherent method (11.5): the receiver is
    /// borrowed, or loaned for `mut self` and `var self`.
    #[allow(clippy::too_many_arguments)]
    fn dot_call(
        &mut self,
        cx: &mut FnCx,
        callee: Callee,
        signature: Signature,
        prebound: HashMap<Ty, Ty>,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = receiver.ty;
        let receiver = match signature.receiver {
            None => {
                let owner = self.types.display(ty);
                let message = format!(
                    "`{}` is an operation of `{owner}` without a receiver: call it as `{owner}.{}(...)`",
                    method.name, method.name
                );
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            Some(mode @ (ast::Mode::Mut | ast::Mode::Var)) => {
                let receiver_span = receiver.span;
                let operation = format!("`{}`", method.name);
                match self.loan_place(cx, receiver, &operation, mode == ast::Mode::Var) {
                    Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, receiver_span),
                    None => return Self::error_expr(span),
                }
            }
            Some(_) => receiver,
        };
        let target = CallTarget { prebound, ..CallTarget::new(callee, signature, expected) };
        self.call_signature(cx, target, Some(receiver), args, span)
    }

    fn no_method(&mut self, ty: Ty, method: &ast::Ident, span: Span) -> hir::Expr {
        let mut message = format!("{} has no method `{}`", self.display(ty), method.name);
        if let Some(hint) = self.method_hint(ty, &method.name) {
            message = format!("{message}: {hint}");
        } else if let Some(trait_name) = self.implemented_trait_with(ty, &method.name) {
            // Implementations do not add methods to dot-calls on concrete types (12.7).
            message = format!(
                "{message}: `{0}` comes from the trait `{trait_name}`; call `{trait_name}.{0}(value, ...)`",
                method.name
            );
        } else if let Some(near) = self.closest_method(ty, &method.name) {
            message = format!("{message}; did you mean `{near}`?");
            self.diags.error("E1001", method.span, message);
            self.diags.fix(format!("call `{near}`"), false, vec![(method.span, near)]);
            return Self::error_expr(span);
        } else if let Some(shown) = self.api_name(ty) {
            message = format!("{message}: `tondo api {shown}` shows the methods it has");
        }
        self.diags.error("E1001", method.span, message);
        Self::error_expr(span)
    }

    /// The name `tondo api` shows a type of the prelude or of a standard
    /// module by, with its methods: `String`, `Option`, `http.Response`;
    /// `none` for a type of the program.
    fn api_name(&mut self, ty: Ty) -> Option<String> {
        let shown = match self.types.kind(ty) {
            TyKind::Option(_) => "Option".to_string(),
            TyKind::Result(..) => "Result".to_string(),
            _ => self.types.display(ty).split('[').next().unwrap_or_default().to_string(),
        };
        let known = match shown.split_once('.') {
            Some((module, _)) => {
                module == "prelude" || crate::api::standard_modules().contains(&format!("std.{module}"))
            }
            None => crate::api::standard("prelude").and_then(|api| crate::api::declared(&api, &shown)).is_some(),
        };
        known.then_some(shown)
    }

    /// The method of `ty` whose name looks like a typo of `name`: one the
    /// prelude or the program declares, or one the compiler implements.
    fn closest_method(&mut self, ty: Ty, name: &str) -> Option<String> {
        let key = self.owner_key(ty);
        let owner = match self.types.kind(ty) {
            TyKind::String => "String".to_string(),
            TyKind::Array(_) => "Array".to_string(),
            TyKind::Deque(_) => "Deque".to_string(),
            TyKind::Map(..) => "Map".to_string(),
            TyKind::Set(_) => "Set".to_string(),
            _ => self.types.display(ty),
        };
        let declared = self.methods.keys().filter(|(k, _)| *k == key).map(|(_, n)| n.clone());
        let names: Vec<String> =
            declared.chain(crate::api::builtin_methods(&owner).iter().map(|name| name.to_string())).collect();
        closest(name, names.iter().map(String::as_str)).map(String::from)
    }

    /// A trait with a method named `name` that `ty` implements, if any.
    fn implemented_trait_with(&mut self, ty: Ty, name: &str) -> Option<String> {
        let found = (0..self.traits.len() as u32).map(TraitId).filter(|t| self.traits[t.0 as usize].params.is_empty());
        let found: Vec<TraitId> =
            found.filter(|t| self.traits[t.0 as usize].methods.iter().any(|m| m.name == name)).collect();
        found.into_iter().find(|&t| self.satisfies(ty, t, &[])).map(|t| self.traits[t.0 as usize].name.clone())
    }

    /// The traits without parameters that give `ty` a method `name`.
    fn traits_with(&mut self, ty: Ty, name: &str) -> Vec<TraitId> {
        let candidates: Vec<TraitId> = (0..self.traits.len() as u32)
            .map(TraitId)
            .filter(|t| self.traits[t.0 as usize].params.is_empty())
            .filter(|t| self.traits[t.0 as usize].methods.iter().any(|m| m.name == name))
            .collect();
        candidates.into_iter().filter(|&t| self.satisfies(ty, t, &[])).collect()
    }

    /// Whether the language gives a type of its own a method `name`. Of the
    /// types it has no list of, all it knows.
    fn language_method(&self, ty: Ty, name: &str) -> bool {
        let owner = match self.types.kind(ty) {
            TyKind::Adt(_) | TyKind::Tuple(_) | TyKind::Bool | TyKind::Char | TyKind::Byte => return false,
            TyKind::Int(_) | TyKind::Float(_) => return false,
            TyKind::String => "String",
            TyKind::Array(_) => "Array",
            TyKind::Deque(_) => "Deque",
            TyKind::Map(..) => "Map",
            TyKind::Set(_) => "Set",
            _ => return true,
        };
        crate::api::builtin_methods(owner).contains(&name)
    }

    /// The canonical spelling of a method that other languages name
    /// differently (10.2.3).
    fn method_hint(&self, ty: Ty, name: &str) -> Option<&'static str> {
        let hint = match (self.types.kind(ty), name) {
            (TyKind::Option(_), "unwrap" | "get" | "value" | "orElseThrow") => {
                "use `expect(message)`, which panics with the message on `none`; or `unwrapOr(fallback)`, `?` or `match`"
            }
            (TyKind::Option(_) | TyKind::Result(..), "unwrap_or" | "getOrElse" | "orElseGet" | "valueOr") => {
                "use `unwrapOr(fallback)`"
            }
            (TyKind::Option(_), "isPresent" | "is_some" | "hasValue") => "use `isSome()`",
            (TyKind::Option(_), "is_none" | "isEmpty" | "isAbsent") => "use `isNone()`",
            (TyKind::Result(..), "unwrap" | "get" | "value") => {
                "use `expect(message)`, which panics with the message on an error; or `unwrapOr(fallback)`, `?` or `match`"
            }
            (TyKind::Result(..), "is_ok" | "isSuccess") => "use `isOk()`",
            (TyKind::Result(..), "is_err" | "isFailure" | "isError") => "use `isErr()`",
            (TyKind::Result(..), "map_err" | "mapError") => "use `mapErr(transform)`",
            (TyKind::String, "toInt" | "parseInt" | "toInteger" | "toNumber" | "parse" | "atoi" | "toLong") => {
                "use `Int.parse(text)`, which gives `Int ! TextError`, or `Float.parse(text)`"
            }
            (TyKind::String, "toFloat" | "toDouble" | "parseFloat") => "use `Float.parse(text)`",
            (TyKind::String, "toLowerCase" | "lower" | "to_lowercase" | "downcase" | "lowercased") => "use `toLower()`",
            (TyKind::String, "toUpperCase" | "upper" | "to_uppercase" | "upcase" | "uppercased") => "use `toUpper()`",
            (TyKind::String, "strip" | "trimmed" | "trimSpace") => "use `trim()`, `trimStart()` or `trimEnd()`",
            (TyKind::String, "trimChars" | "trimMatches" | "trim_matches" | "trimFunc") => {
                "use `trim(characters)`, `trimStart(characters)` or `trimEnd(characters)`"
            }
            (TyKind::String, "cut" | "partition" | "split_once" | "splitAt") => {
                "use `splitOnce(separator)`, which gives `(String, String)?`"
            }
            (TyKind::String, "splitN" | "splitn") => "use `split(separator, limit: count)`",
            (TyKind::String, "indexOf" | "index" | "firstIndex") => "use `find(text)`, which gives `Int?`",
            (TyKind::String, "includes" | "has") => "use `contains(text)`",
            (TyKind::String, "starts_with" | "hasPrefix") => "use `startsWith(prefix)`",
            (TyKind::String, "ends_with" | "hasSuffix") => "use `endsWith(suffix)`",
            (TyKind::String, "format") => "interpolate the values: `\"{first} and {second}\"`",
            (TyKind::String, "equalsIgnoreCase" | "equalFold" | "eq_ignore_ascii_case" | "caseInsensitiveEquals") => {
                "use `equalsIgnoringCase(other)`"
            }
            (_, "toString" | "to_string" | "str" | "toStr" | "description") => "interpolate it: `\"{value}\"`",
            (_, "equals" | "eq" | "isEqual") => "compare with `==`",
            (_, "clone" | "copy" | "dup" | "deepCopy") => "values are copied on assignment: `let copy = value`",
            (TyKind::Array(_), "forEach" | "for_each" | "each") => "use `for value in array { ... }`",
            (TyKind::Array(_), "sort_by" | "sortWith" | "sorted_by") => "use `sortBy(key)`, or `sortedWith(compare)`",
            (TyKind::Array(_), "addAll" | "append_all" | "push_all") => "use `extend(other)`",
            (TyKind::Map(..), "forEach" | "each" | "items" | "iter") => "use `for (key, value) in map { ... }`",
            (TyKind::Map(..), "getOrDefault" | "get_or_default" | "get_or") => "use `getOr(key, fallback)`",
            (_, "size" | "count" | "len") => "use `length()`",
            (_, "empty") => "use `isEmpty()`",
            (TyKind::String, "charAt" | "at") => "use `text[index]`, or `get(index)` for a `Char?`",
            (TyKind::String, "substring" | "substr" | "slice") => "use `text[start:end]`",
            (TyKind::Array(_) | TyKind::Deque(_), "append" | "add" | "enqueue" | "pushBack") => "use `push(value)`",
            (TyKind::Array(_) | TyKind::Deque(_), "peek" | "last" | "back") => "use `get(-1)`, which returns `T?`",
            (TyKind::Array(_) | TyKind::Deque(_), "first" | "front") => "use `get(0)`, which returns `T?`",
            (TyKind::Array(_) | TyKind::Deque(_), "popBack") => "use `pop()`",
            (TyKind::Deque(_), "dequeue" | "shift") => "use `popFront()`",
            (TyKind::Deque(_), "unshift" | "prepend") => "use `pushFront(value)`",
            (TyKind::Array(_), "dequeue" | "shift" | "popFront") => {
                "use `removeAt(0)`, or a `Deque` for a queue: `Deque[T]()` has `popFront()`"
            }
            (TyKind::Array(_), "unshift" | "prepend" | "pushFront") => {
                "use `insert(0, value)`, or a `Deque` for a queue: `Deque[T]()` has `pushFront(value)`"
            }
            (TyKind::Array(_), "remove") => "use `removeAt(index)`",
            (TyKind::Array(_), "indexOf" | "includes") => "use `value in array`",
            (TyKind::Deque(_), "insert" | "removeAt") => "a `Deque` only changes at its ends",
            (TyKind::Map(..), "put" | "set") => "use `insert(key, value)` or `map[key] = value`",
            (TyKind::Map(..), "containsKey" | "has") => "use `contains(key)`",
            (TyKind::Map(..) | TyKind::Set(_), "delete") => "use `remove(key)`",
            (TyKind::Set(_), "add") => "use `insert(value)`",
            (TyKind::Set(_), "has" | "includes") => "use `contains(value)`",
            _ => return None,
        };
        Some(hint)
    }

    fn unsupported_expr(&mut self, span: Span, what: &str) -> hir::Expr {
        self.diags.unsupported(span, what);
        Self::error_expr(span)
    }

    /// A literal, `-` before it when `negative`: the least value of a
    /// signed type is written so (18.8).
    fn int_literal(
        &mut self,
        negative: bool,
        magnitude: u128,
        suffix: Option<&str>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(exact) =
                self.exact_literal(&format!("{}{magnitude}", if negative { "-" } else { "" }), target, span)
        {
            return exact;
        }
        // A literal where a program's `Number` or a `T: Number` is expected is
        // `Number.fromInt[T](literal)` (18.10).
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(number) = self.number
            && self.number_kind(target).is_none()
            && !matches!(self.types.kind(target), TyKind::Error | TyKind::Never)
            && self.satisfies(target, number, &[])
            && let Some(index) = self.traits[number.0 as usize].methods.iter().position(|m| m.name == "fromInt")
        {
            let literal = self.int_literal(negative, magnitude, None, Some(Ty::INT), span);
            let call = hir::ExprKind::Call(Callee::Trait(number, index as u32), vec![target], vec![literal]);
            return Self::mk(call, target, span);
        }
        // Where a `Byte` is expected, a literal from 0 to 255 is one (5.7).
        let unwrapped = expected.map(|t| match *self.types.kind(t) {
            TyKind::Option(inner) => inner,
            _ => t,
        });
        if suffix.is_none() && unwrapped == Some(Ty::BYTE) {
            if negative && magnitude != 0 || magnitude > 255 {
                let sign = if negative { "-" } else { "" };
                let message = format!("literal `{sign}{magnitude}` is not a `Byte`, which holds 0 to 255");
                self.diags.error("E1102", span, message);
            }
            return Self::mk(hir::ExprKind::Int(magnitude.min(255) as i128), Ty::BYTE, span);
        }
        let int = match suffix {
            Some(suffix) => IntTy::from_suffix(suffix),
            None => self.expected_number(expected, false).and_then(|t| self.types.as_int(t)).unwrap_or(IntTy::I64),
        };
        let fits = match negative {
            true => magnitude == 0 || (int.signed() && magnitude <= int.min().unsigned_abs()),
            false => magnitude <= int.max(),
        };
        if !fits {
            let sign = if negative { "-" } else { "" };
            let message = format!("literal `{sign}{magnitude}` does not fit in `{}`", int.name());
            self.diags.error("E1102", span, message);
        }
        let value = if negative { (magnitude as i128).wrapping_neg() } else { magnitude as i128 };
        Self::mk(hir::ExprKind::Int(value), Ty::int(int), span)
    }

    /// A literal where a `math.BigInt` or a `math.Decimal` is expected: that
    /// number, exact, from the literal's text (standard library 14.5).
    fn exact_literal(&mut self, text: &str, target: Ty, span: Span) -> Option<hir::Expr> {
        let named =
            |this: &Self, name: &str| matches!(this.type_names.get(name), Some(&TypeName::Nominal(ty)) if ty == target);
        let maker = if named(self, "math.BigInt") && !text.contains(['.', 'e', 'E']) {
            "math.bigIntOf"
        } else if named(self, "math.Decimal") {
            "math.decimalOf"
        } else {
            return None;
        };
        let Some(&ModuleName::Fn(id)) = self.names.get(maker) else { return None };
        let literal = Self::mk(hir::ExprKind::Str(text.to_string()), Ty::STRING, span);
        Some(Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![literal]), target, span))
    }

    /// An integer literal wider than `u128`, which only a `math.BigInt` or a
    /// `math.Decimal` holds.
    fn big_literal(&mut self, digits: &str, expected: Option<Ty>, span: Span) -> hir::Expr {
        if let Some(target) = expected
            && let Some(exact) = self.exact_literal(digits, target, span)
        {
            return exact;
        }
        let message = "integer literal is too large: a `UInt128` (`u128` suffix), the widest integer, holds up to 340282366920938463463374607431768211455, and a `math.BigInt` any integer";
        self.diags.error("E0003", span, message);
        Self::error_expr(span)
    }

    /// The integer or float type an unsuffixed literal takes from its
    /// context: the expected type, or the only such member of an expected
    /// union or option.
    fn expected_number(&self, expected: Option<Ty>, float: bool) -> Option<Ty> {
        let fits = |t: &Ty| match self.types.kind(*t) {
            TyKind::Int(_) => !float,
            TyKind::Float(_) => float,
            _ => false,
        };
        let mut ty = expected?;
        if let TyKind::Option(inner) = self.types.kind(ty) {
            ty = *inner;
        }
        match self.types.members(ty) {
            Some(members) => match members.iter().filter(|t| fits(t)).collect::<Vec<_>>().as_slice() {
                [only] => Some(**only),
                _ => None,
            },
            None => fits(&ty).then_some(ty),
        }
    }

    fn float_literal(
        &mut self,
        text: &str,
        suffix: Option<&str>,
        negative: bool,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(exact) = self.exact_literal(&format!("{}{text}", if negative { "-" } else { "" }), target, span)
        {
            return exact;
        }
        let ty = match suffix {
            Some("f32") => Ty::F32,
            Some(_) => Ty::F64,
            None if self.expected_number(expected, true) == Some(Ty::F32) => Ty::F32,
            None => Ty::F64,
        };
        // Parse in the target format so rounding happens once (5.8).
        let value = if ty == Ty::F32 { text.parse::<f32>().map(f64::from) } else { text.parse::<f64>() };
        let value = value.unwrap_or(f64::INFINITY);
        if value.is_infinite() {
            let message = format!("literal `{text}` does not fit in {}", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        Self::mk(hir::ExprKind::Float(if negative { -value } else { value }), ty, span)
    }

    /// A comparison asserted, with each side bound first so that a failure
    /// can show both (15.8): the bindings, the condition on them and what
    /// the failure adds. Sides that cannot be displayed are left alone.
    fn compared(&mut self, cx: &mut FnCx, condition: hir::Expr) -> (Vec<hir::Stmt>, hir::Expr, Option<Box<hir::Expr>>) {
        let hir::ExprKind::Binary(op, left, right) = &condition.kind else {
            return (Vec::new(), condition, None);
        };
        let comparison = matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge);
        // `a < b` of an `Ordered` type is `compare(a, b) < 0`: its sides are
        // the call's.
        let ordered = match (&left.kind, &right.kind) {
            (hir::ExprKind::Call(Callee::Trait(t, _), _, args), hir::ExprKind::Int(0))
                if Some(*t) == self.ordered && args.len() == 2 =>
            {
                Some((args[0].ty, args[1].ty))
            }
            _ => None,
        };
        let (left_ty, right_ty) = ordered.unwrap_or((left.ty, right.ty));
        let shown = |this: &mut Self, ty: Ty| !this.types.has_error(ty) && this.satisfies(ty, DISPLAY, &[]);
        if !comparison || !shown(self, left_ty) || !shown(self, right_ty) {
            return (Vec::new(), condition, None);
        }
        let hir::ExprKind::Binary(op, left, right) = condition.kind else { unreachable!() };
        let (span, ty) = (condition.span, condition.ty);
        let (zero, call) = (*right, *left);
        let (left, right, rebuild) = match (ordered, call.kind) {
            (Some(_), hir::ExprKind::Call(callee, generics, args)) => {
                let [a, b] = <[hir::Expr; 2]>::try_from(args).ok().expect("two arguments");
                (Box::new(a), Box::new(b), Some((callee, generics, call.ty, call.span)))
            }
            (_, kind) => (Box::new(Self::mk(kind, call.ty, call.span)), Box::new(zero.clone()), None),
        };
        let mut sides = Vec::new();
        let mut reads = Vec::new();
        let mut segments = Vec::new();
        for (label, side) in [("\n  left:  ", *left), ("\n  right: ", *right)] {
            let (side_ty, side_span) = (side.ty, side.span);
            let local = self.hidden_local(cx, side_ty);
            sides.push(hir::Stmt::Let(local, side));
            let read = Self::mk(hir::ExprKind::Local(local), side_ty, side_span);
            reads.push(read.clone());
            segments.push(hir::Segment::Text(label.into()));
            // Quotes show where a string starts and ends.
            let quoted = side_ty == Ty::STRING;
            if quoted {
                segments.push(hir::Segment::Text("\"".into()));
            }
            segments.push(hir::Segment::Value(self.displayed(read)));
            if quoted {
                segments.push(hir::Segment::Text("\"".into()));
            }
        }
        let [left, right] = <[hir::Expr; 2]>::try_from(reads).ok().expect("two sides");
        let (left, right) = match rebuild {
            Some((callee, generics, call_ty, call_span)) => {
                let call = Self::mk(hir::ExprKind::Call(callee, generics, vec![left, right]), call_ty, call_span);
                (call, zero)
            }
            None => (left, right),
        };
        let condition = Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span);
        let detail = Self::mk(hir::ExprKind::Interpolate(segments), Ty::STRING, span);
        (sides, condition, Some(Box::new(detail)))
    }

    /// The field of a template's data that a name which is no binding
    /// reads, if it has one: the closure of a compiled template, or one in
    /// it, sees the fields of its `self` (standard library 14.31).
    fn template_field(&self, cx: &FnCx, name: &str) -> Option<String> {
        // The linker names another module's functions after it.
        let bare = name.rsplit('.').next().unwrap_or(name);
        let (_, fields) = self.template_data(cx)?;
        fields.contains(&bare).then(|| bare.to_string())
    }

    /// The type of the data of the template whose code is checked, and
    /// the names of its fields.
    fn template_data(&self, cx: &FnCx) -> Option<(Ty, Vec<&str>)> {
        let mut at = Some(cx);
        let data = loop {
            let here = at?;
            if here.template {
                let local = self.lookup(here, "self")?;
                break here.locals[local.0 as usize].ty;
            }
            at = here.parent.as_deref();
        };
        let fields = match self.types.kind(data) {
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => fields.iter().map(|f| f.name.as_str()).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        Some((data, fields))
    }

    /// `template.html[T](...)` or `template.text[T](...)` with the code of
    /// its template, which the compiler wrote from its text (standard
    /// library 14.31): a `Template[T]` that holds the closure that writes
    /// it.
    fn compiled_template(&mut self, cx: &mut FnCx, args: &[ast::Arg], result: Ty, span: Span) -> hir::Expr {
        let Some(ast::ExprKind::Template(body)) = args.first().map(|a| &a.value.kind) else {
            let message = "a template compiled with the program has its text when the program compiles: write it as a raw string, `r\"...\"`, or read it with `embed.text(\"...\")`; a `template.Set` reads templates while the program runs";
            self.diags.error("E1906", args.first().map_or(span, |a| a.value.span), message);
            return Self::error_expr(span);
        };
        let data = match self.types.kind(result) {
            TyKind::Adt(id) => {
                self.types.adt(*id).instance.as_ref().and_then(|(_, args)| args.first().copied()).unwrap_or(Ty::ERROR)
            }
            _ => Ty::ERROR,
        };
        let render = self.template_closure(cx, body, data, span);
        let ty = self.types.intern(TyKind::Fn(vec![data], Ty::STRING, false, vec![ast::Mode::Value], false));
        let render = self.coerce(render, ty);
        Self::mk(hir::ExprKind::Record(vec![(0, render)]), result, span)
    }

    /// The closure that writes a compiled template for its data, `self`:
    /// checked in a context of its own, which captures nothing.
    fn template_closure(&mut self, cx: &mut FnCx, body: &ast::Block, data: Ty, span: Span) -> hir::Expr {
        let param = ast::ClosureParam {
            name: ast::Ident { name: "self".into(), span },
            ty: None,
            mode: ast::Mode::Value,
            variadic: false,
        };
        let expected = self.types.intern(TyKind::Fn(vec![data], Ty::STRING, false, vec![ast::Mode::Value], false));
        let kind = ClosureKind { deferred: false, is_unsafe: false, test: false, infer_error: false };
        self.templating = true;
        self.closure_with(cx, (&[param], None), Some(expected), kind, span, |this, inner, _| {
            let name = ast::Ident { name: crate::templates::OUT.into(), span };
            let out = this.declare(inner, &name, Ty::STRING, true);
            let empty = Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
            let written = this.block(inner, body, Some(Ty::UNIT));
            let written = Self::mk(hir::ExprKind::Block(written), Ty::UNIT, span);
            let tail = Self::mk(hir::ExprKind::Local(out), Ty::STRING, span);
            hir::Block {
                stmts: vec![hir::Stmt::Let(out, empty), hir::Stmt::Expr(written)],
                tail: Some(Box::new(tail)),
                ty: Ty::STRING,
            }
        })
    }

    /// Another template a compiled one writes, with its data, with values
    /// by name, or with the same `self`: a closure of its own, called here.
    /// Values by name are a record of their own, which the included
    /// template reads by those names.
    fn template_include(
        &mut self,
        cx: &mut FnCx,
        data: Option<&ast::Expr>,
        named: &[(ast::Ident, ast::Expr)],
        body: &ast::Block,
        span: Span,
    ) -> hir::Expr {
        let data = match data {
            Some(data) => self.expr(cx, data, None),
            None if !named.is_empty() => {
                let values: Vec<hir::Expr> = named.iter().map(|(_, value)| self.expr(cx, value, None)).collect();
                if values.iter().any(|value| value.ty == Ty::ERROR) {
                    return Self::error_expr(span);
                }
                let fields: Vec<Field> = named
                    .iter()
                    .zip(&values)
                    .map(|((name, _), value)| Field {
                        name: name.name.clone(),
                        ty: value.ty,
                        private: false,
                        secret: false,
                    })
                    .collect();
                let shown: Vec<String> =
                    fields.iter().map(|f| format!("{}: {}", f.name, self.types.display(f.ty))).collect();
                let (id, ty) = self.types.declare_adt(format!("{{ {} }}", shown.join(", ")));
                self.types.define_adt(id, AdtKind::Record(fields));
                let values = values.into_iter().enumerate().map(|(index, value)| (index as u32, value)).collect();
                Self::mk(hir::ExprKind::Record(values), ty, span)
            }
            None => self.path(cx, &[ast::Ident { name: "self".into(), span }], None, span),
        };
        let closure = self.template_closure(cx, body, data.ty, span);
        let local = self.hidden_local(cx, closure.ty);
        let read = Self::mk(hir::ExprKind::Local(local), closure.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), vec![data]), Ty::STRING, span);
        let block =
            hir::Block { stmts: vec![hir::Stmt::Let(local, closure)], tail: Some(Box::new(call)), ty: Ty::STRING };
        Self::mk(hir::ExprKind::Block(block), Ty::STRING, span)
    }

    /// A value a compiled template writes, with its format, and escaped for
    /// its place of HTML: in a script as its JSON, which `Encode` gives; in
    /// any other place, its text, but a `template.Html` in the text of HTML.
    fn template_write(
        &mut self,
        cx: &mut FnCx,
        value: &ast::Expr,
        format: Option<&ast::Format>,
        place: Option<u8>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, value, None);
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let place_code = |place: u8| Self::mk(hir::ExprKind::Int(place.into()), Ty::INT, span);
        let function = |this: &Self, name: &str| match this.names.get(name) {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        };
        // Code of a script: the value as JSON.
        if let Some(place @ (6 | 7)) = place {
            let encodes = match self.trait_names.get("serialization.Encode") {
                Some(&encode) => self.satisfies(value.ty, encode, &[]),
                None => false,
            };
            if !encodes {
                let message = format!(
                    "{} does not implement `serialization.Encode`, which a value in a script needs, as its JSON: write one of its fields, or a value of a record, enum or newtype",
                    self.display(value.ty)
                );
                self.diags.error("E1105", value.span, message);
                return Self::error_expr(span);
            }
            let Some(id) = function(self, "template.scriptValue") else { return Self::error_expr(span) };
            let ty = value.ty;
            let call = hir::ExprKind::Call(Callee::Fn(id), vec![ty], vec![value, place_code(place)]);
            return Self::mk(call, Ty::STRING, span);
        }
        if !self.satisfies(value.ty, DISPLAY, &[]) {
            let message = format!(
                "{} does not implement `Display`, so a template cannot write it: write one of its fields, as `{{{{ post.title }}}}`",
                self.display(value.ty)
            );
            self.diags.error("E1105", value.span, message);
            return Self::error_expr(span);
        }
        let trusted =
            matches!(self.type_names.get("template.Html"), Some(&TypeName::Nominal(html)) if html == value.ty);
        let shown = match format {
            Some(format) => self.formatted(value, format),
            None => self.displayed(value),
        };
        let text = Self::mk(hir::ExprKind::Interpolate(vec![hir::Segment::Value(shown)]), Ty::STRING, span);
        match place {
            None => text,
            Some(0) if trusted => text,
            Some(place) => {
                let Some(id) = function(self, "template.escape") else { return Self::error_expr(span) };
                let call = hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![text, place_code(place)]);
                Self::mk(call, Ty::STRING, span)
            }
        }
    }

    /// A value to interpolate: a user type or a type parameter through
    /// `Display.display`, any other as it is.
    fn displayed(&mut self, value: hir::Expr) -> hir::Expr {
        if matches!(self.types.kind(value.ty), TyKind::Adt(_) | TyKind::Param(_) | TyKind::Opaque(..)) {
            let (ty, span) = (value.ty, value.span);
            let call = hir::ExprKind::Call(Callee::Trait(DISPLAY, 0), vec![ty], vec![value]);
            Self::mk(call, Ty::STRING, span)
        } else {
            value
        }
    }

    /// A value written with the format of its interpolation (language
    /// 19.6), by the prelude's function for an integer, a float or the
    /// text of any other value.
    fn formatted(&mut self, value: hir::Expr, format: &ast::Format) -> hir::Expr {
        let (ty, span) = (value.ty, value.span);
        let integer = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte) || self.bound_by(ty, self.integer);
        let float = matches!(self.types.kind(ty), TyKind::Float(_));
        // A program's number, as a `math.Decimal`, takes a float's options
        // but scientific notation, on the digits of its `Display`.
        let numeric = !integer
            && !float
            && ty != Ty::STRING
            && !self.types.has_error(ty)
            && self.number.is_some_and(|number| self.satisfies(ty, number, &[]));
        let base = format.spec.kind.filter(|k| "xXob".contains(*k));
        let scientific = format.spec.kind.filter(|k| "eE".contains(*k));
        let refused = [
            (format.spec.plus && !integer && !float && !numeric, "a sign".to_string()),
            (format.spec.zero && !integer && !float && !numeric, "`0`".into()),
            (format.spec.alternate && base.is_none() && integer, "`#` without `x`, `X`, `o` or `b`".into()),
            (format.spec.alternate && !integer, "`#`".into()),
            (
                format.spec.precision.is_some() && integer,
                format!("a precision, `.{}`", format.spec.precision.unwrap_or(0)),
            ),
            (base.is_some() && !integer, format!("the base `{}`", base.unwrap_or('x'))),
            (scientific.is_some() && !float, format!("scientific notation, `{}`", scientific.unwrap_or('e'))),
        ];
        if let Some((_, option)) = refused.iter().find(|(refused, _)| *refused) {
            let takes = if integer {
                "`[[fill]align][+][#][0][width][x|X|o|b]`"
            } else if float {
                "`[[fill]align][+][0][width][.precision][e|E]`"
            } else {
                "`[[fill]align][width][.precision]`"
            };
            let message = format!("a format of {} cannot have {option}: write {takes}", self.display(ty));
            self.diags.error("E1118", format.span, message);
            return self.displayed(value);
        }
        let int = |n: i128| Self::mk(hir::ExprKind::Int(n), Ty::INT, span);
        let flag = |b: bool| Self::mk(hir::ExprKind::Bool(b), Ty::BOOL, span);
        let fill = Self::mk(hir::ExprKind::Char(format.spec.fill), Ty::CHAR, span);
        let align = |default: char| Self::mk(hir::ExprKind::Char(format.spec.align.unwrap_or(default)), Ty::CHAR, span);
        let width = int(format.spec.width.unwrap_or(0).into());
        let precision = int(format.spec.precision.map_or(-1, i128::from));
        let upper = format.spec.kind.is_some_and(|k| k.is_ascii_uppercase());
        let (name, generics, args) = if integer {
            let (value, ty) = match self.types.kind(ty) {
                TyKind::Byte => (Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::INT, span), Ty::INT),
                _ => (value, ty),
            };
            let radix = match base {
                Some('x' | 'X') => 16,
                Some('o') => 8,
                Some('b') => 2,
                _ => 10,
            };
            let (plus, zero, prefixed) = (flag(format.spec.plus), flag(format.spec.zero), flag(format.spec.alternate));
            let args = vec![value, int(radix), flag(upper), prefixed, plus, zero, width, fill, align('>')];
            ("formats.formatInteger", vec![ty], args)
        } else if float {
            let single = flag(ty == Ty::F32);
            let value = match ty == Ty::F32 {
                true => Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::F64, span),
                false => value,
            };
            let (exponent, plus, zero) = (flag(scientific.is_some()), flag(format.spec.plus), flag(format.spec.zero));
            let args = vec![value, single, precision, exponent, flag(upper), plus, zero, width, fill, align('>')];
            ("formats.formatFloat", Vec::new(), args)
        } else if numeric {
            let shown = Self::mk(
                hir::ExprKind::Interpolate(vec![hir::Segment::Value(self.displayed(value))]),
                Ty::STRING,
                span,
            );
            let (plus, zero) = (flag(format.spec.plus), flag(format.spec.zero));
            ("formats.formatNumeric", Vec::new(), vec![shown, precision, plus, zero, width, fill, align('>')])
        } else {
            let text = match ty == Ty::STRING {
                true => value,
                false => {
                    let shown = hir::ExprKind::Interpolate(vec![hir::Segment::Value(self.displayed(value))]);
                    Self::mk(shown, Ty::STRING, span)
                }
            };
            ("formats.formatText", Vec::new(), vec![text, precision, width, fill, align('<')])
        };
        let Some(&ModuleName::Fn(function)) = self.names.get(name) else { return Self::error_expr(span) };
        Self::mk(hir::ExprKind::Call(Callee::Fn(function), generics, args), Ty::STRING, span)
    }

    /// `sql.Query`, when the program has `std.sql`.
    fn query_type(&self) -> Option<Ty> {
        match self.type_names.get("sql.Query") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A string literal where a `sql.Query` is expected: its text is SQL,
    /// each value it interpolates a parameter, a query spliced in, and an
    /// array the list of its elements (standard library 14.37), so a value
    /// never becomes SQL.
    fn query_literal(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], query: Ty, span: Span) -> hir::Expr {
        let function = |this: &Self, name: &str| match this.names.get(name) {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        };
        let (Some(made), Some(text), Some(nested), Some(bound)) = (
            function(self, "sql.made"),
            function(self, "sql.literalText"),
            function(self, "sql.nested"),
            function(self, "sql.bound"),
        ) else {
            return Self::error_expr(span);
        };
        let piece = self.signatures[text.0 as usize].result;
        let encode = self.trait_names.get("serialization.Encode").copied();
        let mut pieces = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(written) => {
                    let literal = Self::mk(hir::ExprKind::Str(written.clone()), Ty::STRING, span);
                    pieces.push(Self::mk(
                        hir::ExprKind::Call(Callee::Fn(text), Vec::new(), vec![literal]),
                        piece,
                        span,
                    ));
                }
                ast::StrSegment::Expr(e, format) => {
                    if let Some(format) = format {
                        let message = "a value of a query is a parameter, which a format does not change: format it before, or drop the format";
                        self.diags.error("E1118", format.span, message);
                    }
                    let value = self.expr(cx, e, None);
                    if value.ty == query {
                        pieces.push(Self::mk(
                            hir::ExprKind::Call(Callee::Fn(nested), Vec::new(), vec![value]),
                            piece,
                            span,
                        ));
                        continue;
                    }
                    // Bytes are one parameter, not a list of numbers.
                    let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                    let optional = self.types.intern(TyKind::Option(bytes));
                    let binary = match value.ty {
                        ty if ty == bytes => function(self, "sql.boundBytes"),
                        ty if ty == optional => function(self, "sql.boundOptionalBytes"),
                        _ => None,
                    };
                    if let Some(binary) = binary {
                        pieces.push(Self::mk(
                            hir::ExprKind::Call(Callee::Fn(binary), Vec::new(), vec![value]),
                            piece,
                            span,
                        ));
                        continue;
                    }
                    if !self.types.has_error(value.ty)
                        && let Some(encode) = encode
                        && !self.satisfies(value.ty, encode, &[])
                    {
                        let message = format!(
                            "a value a query interpolates is a parameter, and {} is not one: a parameter is a number, a text, bytes, a time, an option or an array of them; interpolate a field, or its JSON as a text",
                            self.display(value.ty)
                        );
                        self.diags.error("E1105", e.span, message);
                        continue;
                    }
                    let ty = value.ty;
                    pieces.push(Self::mk(hir::ExprKind::Call(Callee::Fn(bound), vec![ty], vec![value]), piece, span));
                }
            }
        }
        let array = self.types.intern(TyKind::Array(piece));
        let list = Self::mk(hir::ExprKind::Array(pieces), array, span);
        Self::mk(hir::ExprKind::Call(Callee::Fn(made), Vec::new(), vec![list]), query, span)
    }

    fn string(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], span: Span) -> hir::Expr {
        if let [ast::StrSegment::Text(text)] = segments {
            return Self::mk(hir::ExprKind::Str(text.clone()), Ty::STRING, span);
        }
        let mut out = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(text) => out.push(hir::Segment::Text(text.clone())),
                ast::StrSegment::Expr(e, format) => {
                    // Only a lone name in braces may have meant the braces.
                    let lone = matches!(&e.kind, ast::ExprKind::Path(segments) if segments.len() == 1);
                    let outer = std::mem::replace(&mut self.interpolating, lone);
                    let value = self.expr(cx, e, None);
                    self.interpolating = outer;
                    if !self.satisfies(value.ty, DISPLAY, &[]) {
                        let mut message = format!("{} does not implement `Display`", self.display(value.ty));
                        if let Some(members) = self.types.members(value.ty).map(<[Ty]>::to_vec) {
                            let lacking: Vec<Ty> =
                                members.into_iter().filter(|&m| !self.satisfies(m, DISPLAY, &[])).collect();
                            let lacking: Vec<String> =
                                lacking.into_iter().map(|m| format!("`{}`", self.types.display(m))).collect();
                            message.push_str(&format!(
                                ": {} does not; `match` on its members to format each one",
                                lacking.join(" and ")
                            ));
                        } else if let TyKind::Handle(Handle::Ref, _) = self.types.kind(value.ty) {
                            message.push_str(": an identity has no text; show what it identifies with `.value`");
                        } else if matches!(self.types.kind(value.ty), TyKind::Adt(_))
                            && self.owner(value.ty).is_some_and(|o| o.module == self.module)
                        {
                            let name = self.types.display(value.ty);
                            message.push_str(&format!(
                                ": add `impl Display for {name} {{ fn display(self): String {{ ... }} }}`"
                            ));
                        } else if matches!(self.types.kind(value.ty), TyKind::Adt(_)) {
                            // Another module's: only that module may implement it.
                            message.push_str(": another module declares it, so write what to show of it, its fields or a `match` on it");
                        }
                        self.diags.error("E1105", value.span, message);
                        out.push(hir::Segment::Value(value));
                        continue;
                    }
                    let value = match format {
                        Some(format) => self.formatted(value, format),
                        None => self.displayed(value),
                    };
                    out.push(hir::Segment::Value(value));
                }
            }
        }
        Self::mk(hir::ExprKind::Interpolate(out), Ty::STRING, span)
    }

    fn path(&mut self, cx: &mut FnCx, segments: &[ast::Ident], expected: Option<Ty>, span: Span) -> hir::Expr {
        let first = &segments[0];
        let captured = match self.lookup(cx, &first.name) {
            Some(_) => None,
            None => self.capture(cx, &first.name, first.span),
        };
        if let Some(mut value) = captured {
            self.index_local(cx, first);
            for segment in &segments[1..] {
                self.index_expr(value.span, value.ty);
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        if let Some(local) = self.lookup(cx, &first.name) {
            self.index_local(cx, first);
            cx.reads.insert(local);
            let ty = cx.locals[local.0 as usize].ty;
            let mut value = Self::mk(hir::ExprKind::Local(local), ty, first.span);
            for segment in &segments[1..] {
                self.index_expr(value.span, value.ty);
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            if let Some(place) = hir::Place::of(value.clone())
                && let Some((lent, true)) = lent_conflict(cx, &place)
            {
                let lent = self.place_name(cx, lent);
                self.lent_error(&lent, true, value.span);
            }
            return value;
        }
        // In a template, a name that is no binding is a field of its data,
        // before a function or a constant (standard library 14.31).
        if let Some(field) = self.template_field(cx, &first.name) {
            let mut written = vec![
                ast::Ident { name: "self".into(), span: first.span },
                ast::Ident { name: field, span: first.span },
            ];
            written.extend(segments[1..].iter().cloned());
            return self.path(cx, &written, expected, span);
        }
        // The constants of the intrinsic numeric types: `Int.max`, `Float.pi`.
        if let [type_name, constant] = segments
            && matches!(self.names.get(&type_name.name), None | Some(ModuleName::Fn(_)))
            && let Some(ty) = self.intrinsic_owner(&type_name.name)
            && let Some(kind) = self.intrinsic_constant(ty, &constant.name)
        {
            let ty = if matches!(kind, hir::ExprKind::Int(_)) && constant.name == "bits" { Ty::INT } else { ty };
            return Self::mk(kind, ty, span);
        }
        // `T.min`, `T.max` and `T.bits` of a `T: Integer` (18.10).
        if let [type_name, constant] = segments
            && matches!(constant.name.as_str(), "min" | "max" | "bits")
            && self.lookup(cx, &type_name.name).is_none()
            && let Some(ty) = self.type_named(&type_name.name)
            && self.bound_by(ty, self.integer)
        {
            use crate::numbers::NumOp;
            let constant_of = |op: NumOp| Callee::Std(StdFn::Number(op));
            let least = Self::mk(hir::ExprKind::Call(constant_of(NumOp::Least), Vec::new(), Vec::new()), ty, span);
            return match constant.name.as_str() {
                "min" => least,
                "max" => Self::mk(hir::ExprKind::Call(constant_of(NumOp::Greatest), Vec::new(), Vec::new()), ty, span),
                _ => Self::mk(hir::ExprKind::Call(constant_of(NumOp::Width), Vec::new(), vec![least]), Ty::INT, span),
            };
        }
        // `Context.field`: its value where the code runs (11.16).
        if let [name, field, rest @ ..] = segments
            && let Some((ty, context)) = self.context_named(std::slice::from_ref(name))
        {
            let reference = self.types.handle(Handle::Ref, ty);
            // For an editor: the `with` that gave the field its value here.
            if let Some(at) =
                self.record_fields(ty, field.span).and_then(|f| f.iter().position(|(n, _)| *n == field.name))
            {
                let bit = 1u64 << at.min(63);
                let set = cx.withs.iter().rev().find(|&&(c, fields, _)| c == context && fields & bit != 0).map(|w| w.2);
                if let Some(index) = self.index.as_mut() {
                    index.contexts.push((field.span, set));
                }
                if let Some(reads) = self.initial_reads.as_mut() {
                    reads.push((context, at as u32));
                }
            }
            let mut value = Self::mk(hir::ExprKind::ContextRead(context, reference), ty, name.span);
            for segment in std::iter::once(field).chain(rest) {
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        // `Type.operation` as a value: an associated operation (11.7).
        if let [type_name, operation] = segments
            && matches!(self.names.get(&type_name.name), None | Some(ModuleName::Fn(_)))
            && let Some(owner) = self.type_named(&type_name.name)
            && let Some(&id) = self.methods.get(&(owner, operation.name.clone()))
        {
            self.index_refer(type_name, crate::index::Key::Type(type_name.name.clone()));
            self.index_refer(operation, crate::index::Key::Fn(id.0));
            self.check_method_visible(id, operation.span);
            return self.fn_value(id, Vec::new(), expected, span);
        }
        if let Some(resolved) = self.variant_path(segments) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            if let Payload::Unit = self.payload(ty, variant) {
                let ty = match self.generic_inference(ty, expected) {
                    Some(inference) => self.finish_inference(&inference, ty, span),
                    None => ty,
                };
                return Self::mk(hir::ExprKind::Variant(variant, Vec::new()), ty, span);
            }
            let message = format!("`{}` needs a payload", Self::path_text(segments));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        match self.names.get(&first.name) {
            Some(&ModuleName::Fn(id)) if Some(id) == self.main_fn() => self.main_called(span),
            Some(&ModuleName::Fn(id)) if segments.len() == 1 => {
                self.index_refer(first, crate::index::Key::Fn(id.0));
                self.fn_value(id, Vec::new(), expected, span)
            }
            Some(&ModuleName::Const(id)) => {
                self.index_refer(first, crate::index::Key::Value(first.name.clone()));
                let mut value = self.const_value(id, first.span);
                for segment in &segments[1..] {
                    let span = value.span.to(segment.span);
                    value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
                }
                value
            }
            Some(&ModuleName::Extern(id)) if segments.len() == 1 => {
                let declared = &self.externs[id as usize];
                let (params, result, variadic) = (declared.params.clone(), declared.result, declared.variadic);
                let ty = self.types.intern(TyKind::CFn(params, result, variadic));
                Self::mk(hir::ExprKind::ExternFn(id), ty, span)
            }
            Some(ModuleName::Extern(_)) => {
                self.diags.error("E1102", span, "a function has no fields");
                Self::error_expr(span)
            }
            Some(ModuleName::Fn(_)) => {
                self.diags.error("E1102", span, "a function has no fields");
                Self::error_expr(span)
            }
            Some(ModuleName::Newtype(_)) => {
                let message = format!("`{}` constructs a value: write `{}(value)`", first.name, first.name);
                self.diags.error("E1102", span, message);
                Self::error_expr(span)
            }
            Some(ModuleName::Module(_)) => {
                let (code, message) = match segments {
                    [module] => ("E1102", format!("`{}` is a module, not a value", module.name)),
                    [module, member] => ("E1001", format!("module `{}` has no value `{}`", module.name, member.name)),
                    [module, member, ..] => {
                        ("E1001", format!("module `{}` has no type `{}`", module.name, member.name))
                    }
                    [] => unreachable!("a path has a segment"),
                };
                self.diags.error(code, span, message);
                Self::error_expr(span)
            }
            None => {
                self.unknown_path(cx, segments, span);
                Self::error_expr(span)
            }
        }
    }

    /// Reports a path whose first name is unknown as a value: a method
    /// called on its type, as `Array.repeat(...)`, or an unknown name.
    fn unknown_path(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) {
        let first = &segments[0];
        if segments.len() > 1 && (self.intrinsic_owner(&first.name).is_some() || self.type_named(&first.name).is_some())
        {
            let (owner, member) = (&first.name, &segments[1].name);
            let message = format!(
                "`{owner}` has no function `{member}`; if `{member}` is a method, call it on a value: `value.{member}(...)`"
            );
            self.diags.error("E1001", span, message);
        } else {
            self.unknown_name(cx, &first.name, first.span);
        }
    }

    /// Offers to write the name the last diagnostic says the program may
    /// have meant, in place of `span`.
    fn offer_meant(&mut self, span: Span) {
        let Some(last) = self.diags.list.last() else { return };
        let Some(near) = last.message.split("did you mean `").nth(1).and_then(|rest| rest.split('`').next()) else {
            return;
        };
        let near = near.to_string();
        self.diags.fix(format!("write `{near}`"), false, vec![(span, near)]);
    }

    /// Reports `E1001`, pointing habits from other languages at the canonical form.
    fn unknown_name(&mut self, cx: &FnCx, name: &str, span: Span) {
        // A module of the program this file does not import.
        let here = self.sources.file_of(span.start).module.clone();
        let module = self
            .module_paths
            .iter()
            .filter(|p| p.first().is_some_and(|first| first != "std") && p.last().is_some_and(|last| last == name))
            .map(|p| p.join("."))
            .find(|path| *path != here);
        let hint = match name {
            "Some" | "Ok" | "Err" => format!("Tondo writes `{}(...)`", name.to_lowercase()),
            "None" | "null" | "nil" | "undefined" | "NULL" | "nullptr" | "Nothing" => {
                "Tondo writes `none`, the `T?` without a value".into()
            }
            "True" | "TRUE" => "Tondo writes `true`".into(),
            "False" | "FALSE" => "Tondo writes `false`".into(),
            "println" | "print" | "printf" | "puts" | "echo" | "printLn" | "log" => {
                "`console.println(text)` writes a line, after `import std.console`".into()
            }
            "range" => "a range is `0..10`, or `0..=10` with its end".into(),
            "len" | "length" | "size" | "count" | "sizeof" | "strlen" => "use `value.length()`".into(),
            "self" => "a method takes `self` as its first parameter: `fn Type.name(self)`".into(),
            "this" => "a method's receiver is `self`, its first parameter: `fn Type.name(self)`".into(),
            "await" => {
                "there is no `await`: call the function, which waits, or `spawn f()` and later `wait()` its `Join`"
                    .into()
            }
            "HashMap" | "Dictionary" | "dict" | "Dict" | "TreeMap" | "BTreeMap" => {
                "a map is `Map[K, V]`: `[:]` is an empty one, `[\"a\": 1]` one with an entry".into()
            }
            "Vec" | "List" | "ArrayList" | "vector" | "Vector" | "list" => {
                "an array is `Array[T]`: `[]` is an empty one, `[1, 2]` one with elements".into()
            }
            "HashSet" | "TreeSet" | "BTreeSet" => "a set is `Set[T]`, as in `Set[1, 2]`".into(),
            "str" | "toString" | "string" => "interpolate the value: `\"{value}\"`".into(),
            "int" | "float" => format!("a conversion is a call of the type: `{}(value)`", capitalized(name)),
            "assert_eq" | "assertEquals" | "assertEqual" | "expect" => {
                "`assert(actual == expected)` shows both sides when it fails".into()
            }
            "sleep" => "`time.sleep(time.milliseconds(10))` waits, after `import std.time`".into(),
            "exit" => "`process.exit(code)` ends the program, after `import std.process`".into(),
            "recover" | "catch_unwind" | "rescue" => {
                "`recover { ... }` stops a panic of its block: `ok` with its value, or `err(p)` with the `Panic`".into()
            }
            _ if crate::api::standard_modules().contains(&format!("std.{name}")) => {
                format!("`{name}` is a standard module: add `import std.{name}` at the top of the file")
            }
            _ if let Some(found) = self.elsewhere.iter().find(|e| e.name == name && e.module == here) => {
                let system = crate::driver::system();
                format!(
                    "`{name}` is in `{}`, for {}: write it for {} too, in a file `*_{}.to` or a common one, as `{}`",
                    found.file,
                    found.systems,
                    system.shown(),
                    system.name(),
                    found.head
                )
            }
            _ if module.is_some() => {
                format!(
                    "`{name}` is the module `{}`: add `import {}` at the top of the file",
                    module.as_deref().unwrap_or_default(),
                    module.as_deref().unwrap_or_default()
                )
            }
            _ if self.script_bindings.iter().any(|bound| bound == name) => match self.scripting {
                true => format!("the script declares `{name}` further down: a binding is seen from its `let` on"),
                false => format!(
                    "`{name}` is a binding of the script, which its functions do not see: pass it to the function as a parameter"
                ),
            },
            _ if self.interpolating => format!(
                "inside a string, `{{{name}}}` interpolates it: write `{{{{{name}}}}}` for the braces themselves"
            ),
            _ if let Some((data, fields)) = self.template_data(cx) => {
                let near =
                    closest(name, fields.iter().copied()).map(|n| format!("did you mean `{n}`? ")).unwrap_or_default();
                let fields: Vec<String> = fields.iter().map(|f| format!("`{f}`")).collect();
                match fields.is_empty() {
                    true => format!("it is no binding, and the template's data, {}, has no fields", self.display(data)),
                    false => format!(
                        "{near}it is no binding, nor a field of the template's data, {}, whose fields are {}",
                        self.display(data),
                        fields.join(", ")
                    ),
                }
            }
            // A declaration of a standard module, written without it.
            _ if !crate::api::declaring(name).is_empty() => {
                let owners = self.declaring(span, name);
                let first = &owners[0];
                let written = format!("{}.{name}", self.imported_as(span, first));
                let imported = self.imported(span, first);
                match owners.len() {
                    1 if imported => format!("`{name}` is declared by `{first}`: write `{written}`"),
                    1 => format!("`{name}` is declared by `{first}`: write `{written}`, after `import {first}`"),
                    count => {
                        let shown: Vec<String> = owners.iter().take(3).map(|o| format!("`{o}`")).collect();
                        let more = if count > 3 { format!(" and {} more", count - 3) } else { String::new() };
                        format!(
                            "`{name}` is declared by {}{more}: write it with its module, as `{written}`",
                            shown.join(", ")
                        )
                    }
                }
            }
            _ => {
                // The bindings in sight and the module's names.
                let mut names: Vec<&str> = self.names.keys().map(String::as_str).collect();
                let mut scope = Some(cx);
                while let Some(found) = scope {
                    names.extend(found.scopes.names());
                    scope = found.parent.as_deref();
                }
                match closest(name, names) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => String::new(),
                }
            }
        };
        let message = match hint.is_empty() {
            true => format!("unknown name `{name}`"),
            false => format!("unknown name `{name}`; {hint}"),
        };
        self.diags.error("E1001", span, message);
        let replacement = match name {
            "Some" | "Ok" | "Err" => Some((true, name.to_lowercase())),
            "None" | "null" | "nil" | "undefined" | "NULL" | "nullptr" | "Nothing" => Some((false, "none".into())),
            "True" | "TRUE" => Some((true, "true".into())),
            "False" | "FALSE" => Some((true, "false".into())),
            "this" => Some((false, "self".into())),
            _ => hint
                .strip_prefix("did you mean `")
                .and_then(|h| h.strip_suffix("`?"))
                .map(|near| (false, near.to_string())),
        };
        let owners = self.declaring(span, name);
        if let Some((safe, replacement)) = replacement {
            self.diags.fix(format!("write `{replacement}`"), safe, vec![(span, replacement)]);
        } else if !owners.is_empty() {
            // Each module that declares it, with its import when the file
            // lacks it; the one meaning there is when only one does.
            for owner in owners.iter().take(6) {
                let written = format!("{}.{name}", self.imported_as(span, owner));
                let mut edits = vec![(span, written.clone())];
                if !self.imported(span, owner) {
                    edits.push((self.import_place(span, owner), format!("import {owner}\n")));
                }
                self.diags.fix(format!("write `{written}`"), owners.len() == 1, edits);
            }
        } else if let Some(path) = module
            .or_else(|| crate::api::standard_modules().contains(&format!("std.{name}")).then(|| format!("std.{name}")))
        {
            let place = self.import_place(span, &path);
            self.diags.fix(format!("add `import {path}`"), true, vec![(place, format!("import {path}\n"))]);
        }
    }

    fn call(
        &mut self,
        cx: &mut FnCx,
        callee: &ast::Expr,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        match &callee.kind {
            ast::ExprKind::Field(receiver, method) => {
                return self.method_call(cx, receiver, method, args, expected, span);
            }
            ast::ExprKind::Path(segments)
                if segments.len() > 1
                    && (self.lookup(cx, &segments[0].name).is_some()
                        || self.visible_capture(cx, &segments[0].name)
                        || self.template_field(cx, &segments[0].name).is_some()
                        || matches!(self.names.get(&segments[0].name), Some(ModuleName::Const(_)))) =>
            {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Request.user.unwrapOr("")`: a method of a context's field.
            ast::ExprKind::Path(segments) if segments.len() > 2 && self.is_context(&segments[0].name) => {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Float.nan.isNaN()`: a method of an intrinsic constant.
            ast::ExprKind::Path(segments)
                if segments.len() == 3
                    && !self.names.contains_key(&segments[0].name)
                    && self.lookup(cx, &segments[0].name).is_none()
                    && self
                        .intrinsic_owner(&segments[0].name)
                        .is_some_and(|ty| self.intrinsic_constant(ty, &segments[1].name).is_some()) =>
            {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix[1].span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Color.Red.describe()`: a method of a variant without payload.
            ast::ExprKind::Path(segments) if segments.len() == 3 && self.unit_variant(&segments[..2]) => {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix[1].span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            ast::ExprKind::Generic { base, args: type_args } => {
                return self.generic_call(cx, base, type_args, args, expected, span);
            }
            // `call().cast[U]()`: written after a call, the type argument
            // parses as an index.
            ast::ExprKind::Index { base, index } if matches!(&base.kind, ast::ExprKind::Field(_, m) if matches!(m.name.as_str(), "cast" | "toPointer" | "toFunction")) => {
                if let Some(ty) = Self::type_of_expr(index) {
                    return self.generic_call(cx, base, &[ty], args, expected, span);
                }
            }
            _ => {}
        }
        let ast::ExprKind::Path(segments) = &callee.kind else {
            // Any other expression is a value to call (11.7).
            let value = self.expr(cx, callee, None);
            return self.call_value(cx, value, args, expected, span);
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if self.lookup(cx, names[0]).is_some() || self.visible_capture(cx, names[0]) {
            let value = self.expr(cx, callee, None);
            return self.call_value(cx, value, args, expected, span);
        }
        match names.as_slice() {
            ["Pointer", operation @ ("read" | "write" | "offset" | "address")] => {
                // The qualified form takes the pointer first (16.12).
                let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                let receiver = self.expr(cx, &first.value, None);
                let method = ast::Ident { name: (*operation).to_string(), span: segments[1].span };
                if !matches!(self.types.kind(receiver.ty), TyKind::Pointer(_)) && !self.types.has_error(receiver.ty) {
                    let message = format!("expected a `Pointer[T]`, found {}", self.display(receiver.ty));
                    self.diags.error("E1102", receiver.span, message);
                    return Self::error_expr(span);
                }
                return self.raw(cx, receiver, &method, Vec::new(), rest, span);
            }
            ["String", "empty"] => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                return Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
            }
            ["String", "fromBytes"] => {
                // `ok(text)`, or `InvalidData` when the bytes are not UTF-8.
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                let value = self.expr(cx, &arg.value, Some(bytes));
                let value = self.coerce(value, bytes);
                let maybe = self.types.intern(TyKind::Option(Ty::STRING));
                let ty = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
                let decoded = Self::mk(hir::ExprKind::StrOp(hir::StrOp::FromBytes, vec![value]), maybe, span);
                let text = self.hidden_local(cx, Ty::STRING);
                let pattern = |kind: hir::PatKind, ty: Ty| hir::Pattern { kind, ty, span };
                let read = Self::mk(hir::ExprKind::Local(text), Ty::STRING, span);
                let invalid = Self::mk(hir::ExprKind::Variant(3, Vec::new()), self.text_error, span);
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(
                            hir::PatKind::Variant(SOME, vec![pattern(hir::PatKind::Binding(text), Ty::STRING)]),
                            maybe,
                        ),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(OK, vec![(0, read)]), ty, span),
                    },
                    hir::Arm {
                        pattern: pattern(hir::PatKind::Variant(NONE, Vec::new()), maybe),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), ty, span),
                    },
                ];
                return Self::mk(hir::ExprKind::Match(Box::new(decoded), arms), ty, span);
            }
            ["String", "fromChars"] => {
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let chars = self.types.intern(TyKind::Array(Ty::CHAR));
                let value = self.expr(cx, &arg.value, Some(chars));
                let value = self.coerce(value, chars);
                // Building the text cannot fail short of resource exhaustion.
                let text = Self::mk(hir::ExprKind::StrOp(hir::StrOp::FromChars, vec![value]), Ty::STRING, span);
                let ty = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
                return Self::mk(hir::ExprKind::Variant(OK, vec![(0, text)]), ty, span);
            }
            _ => {}
        }
        if let Some(retired) = self.retired_new(cx, segments, span) {
            return retired;
        }
        if let ["Array", "withCapacity"] = names.as_slice() {
            let message = "`Array.withCapacity` needs its element type: `Array.withCapacity[T](...)`";
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        if let [kind @ ("Array" | "Deque" | "Group")] = names.as_slice() {
            self.diags.error("E1101", span, format!("`{kind}` needs its element type: `{kind}[T]()`"));
            return Self::error_expr(span);
        }
        if let [kind @ ("Map" | "Set")] = names.as_slice() {
            let types = if *kind == "Map" { "K, V" } else { "T" };
            self.diags.error("E1101", span, format!("`{kind}` needs what it holds: `{kind}[{types}]()`"));
            return Self::error_expr(span);
        }
        if let ["Ref"] = names.as_slice()
            && self.lookup(cx, "Ref").is_none()
        {
            return self.ref_new(cx, args, None, expected, span);
        }
        if let ["WeakRef"] = names.as_slice()
            && self.lookup(cx, "WeakRef").is_none()
        {
            return self.weak_new(cx, args, expected, span);
        }
        if let ["channel"] = names.as_slice() {
            let message = "`channel` needs the type of its values: `channel[T]()`, or `channel[T](capacity)`";
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        // `Trait.method(value, ...)` calls an implementation explicitly (12.7).
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
            && !self.type_params.iter().any(|(name, _)| name == trait_name)
        {
            self.index_refer(&segments[0], crate::index::Key::Trait(trait_id.0));
            return self.trait_call(cx, trait_id, &segments[1], Vec::new(), args, expected, span);
        }
        // A method of a type of the language, with its receiver written first (11.5).
        if let [type_name, operation] = names.as_slice()
            && !self.names.contains_key(*type_name)
            && let Some(owner) = self.intrinsic_owner(type_name)
            && self.number_kind(owner).is_none()
        {
            if let Some(&id) = self.methods.get(&(owner, operation.to_string()))
                && self.signatures[id.0 as usize].receiver.is_some()
            {
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                return self.call_signature(cx, target, None, args, span);
            }
            if crate::api::builtin_methods(type_name).contains(operation) {
                let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                return self.language_call(cx, owner, first, &segments[1], rest, expected, span);
            }
        }
        // `Type.operation(...)`: an associated operation, or a method with
        // its receiver written first (11.5). Operations and variants never
        // share a name.
        if let [type_name, operation] = names.as_slice()
            && matches!(self.names.get(*type_name), None | Some(ModuleName::Fn(_)))
            && let Some(owner) = self.type_named(type_name)
        {
            let integer = self.bound_by(owner, self.integer);
            if let Some(kind) = self.number_kind(owner).or(integer.then_some(crate::numbers::Kind::Int(IntTy::I64))) {
                let find = |receiver: bool| match integer {
                    true => crate::numbers::find_integer(operation, receiver),
                    false => crate::numbers::find(kind, operation, receiver),
                };
                if let Some(entry) = find(false) {
                    return self.number_function(cx, owner, entry, args, expected, span);
                }
                // A method with its receiver written first: `Int.abs(x)` (11.5).
                if let Some(entry) = find(true)
                    && let Some((first, rest)) = args.split_first()
                {
                    let receiver = self.expr(cx, &first.value, Some(owner));
                    let receiver = self.coerce(receiver, owner);
                    let signature = self.number_signature(owner, entry);
                    let callee = Callee::Std(StdFn::Number(entry.op));
                    let target = CallTarget::new(callee, signature, expected);
                    return self.call_signature(cx, target, Some(receiver), rest, span);
                }
            }
            if let Some(id) = self.methods.get(&(owner, operation.to_string())).copied() {
                self.index_refer(&segments[0], crate::index::Key::Type(segments[0].name.clone()));
                self.index_refer(&segments[1], crate::index::Key::Fn(id.0));
                self.check_method_visible(id, span);
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                return self.call_signature(cx, target, None, args, span);
            }
            if self.types.variants(owner).is_none()
                || matches!(self.types.kind(owner), TyKind::Option(_) | TyKind::Result(..))
            {
                let message = format!("`{type_name}` has no operation named `{operation}`");
                self.diags.error("E1001", segments[1].span, message);
                return Self::error_expr(span);
            }
        }
        if let Some(resolved) = self.variant_path(segments) {
            if let Some(arg) = args.iter().find(|a| a.name.is_some() || a.mode != ast::Mode::Value || a.spread) {
                return self.unsupported_expr(arg.span, "this argument form in a variant constructor");
            }
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            let Payload::Tuple(types) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take positional values", Self::path_text(segments));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            if types.len() != args.len() {
                return self.arity_error(span, types.len(), args.len());
            }
            // A generic enum's arguments come from the expected type and the
            // payload; values that need context wait for the others.
            let mut inference = self.generic_inference(ty, expected);
            let mut checked: Vec<Option<hir::Expr>> = (0..args.len()).map(|_| None).collect();
            for pass in 0..2 {
                for (index, (arg, &payload)) in args.iter().zip(&types).enumerate() {
                    if checked[index].is_some() {
                        continue;
                    }
                    let expected = match &inference {
                        Some(inference) => self.inferred(inference, payload),
                        None => Some(payload),
                    };
                    if pass == 0 && expected.is_none() && self.needs_context(&arg.value) {
                        continue;
                    }
                    let value = self.expr(cx, &arg.value, expected);
                    if let Some(inference) = &mut inference {
                        self.unify(inference, payload, value.ty);
                    }
                    checked[index] = Some(value);
                }
            }
            let values: Vec<hir::Expr> = checked.into_iter().map(Option::unwrap).collect();
            let (ty, map) = match inference {
                Some(inference) => (self.finish_inference(&inference, ty, span), inference.map),
                None => (ty, HashMap::new()),
            };
            let values = values
                .into_iter()
                .zip(types)
                .enumerate()
                .map(|(index, (value, payload))| {
                    let payload = self.types.subst(payload, &map);
                    (index as u32, self.coerce(value, payload))
                })
                .collect();
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        match (names.as_slice(), self.names.get(names[0]).copied()) {
            ([_, member], Some(ModuleName::Module(module))) if let Some(id) = self.companion_fn(module, member) => {
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            (["panic"], _) => self.panic_call(cx, args, span),
            (["assert"], _) => {
                let Some((condition, parts)) = args.split_first() else {
                    return self.arity_error(span, 1, 0);
                };
                let source = self.sources.slice(condition.value.span).to_string();
                let written = matches!(condition.value.kind, ast::ExprKind::Binary(..));
                let condition = self.expr(cx, &condition.value, Some(Ty::BOOL));
                let condition = self.coerce(condition, Ty::BOOL);
                let (sides, condition, detail) = match written {
                    true => self.compared(cx, condition),
                    false => (Vec::new(), condition, None),
                };
                let parts = parts
                    .iter()
                    .map(|p| {
                        let part = self.expr(cx, &p.value, Some(Ty::STRING));
                        self.coerce(part, Ty::STRING)
                    })
                    .collect();
                let condition = Box::new(condition);
                let assert = Self::mk(hir::ExprKind::Assert { condition, parts, source, detail }, Ty::UNIT, span);
                if sides.is_empty() {
                    return assert;
                }
                let block = hir::Block { stmts: sides, tail: Some(Box::new(assert)), ty: Ty::UNIT };
                Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span)
            }
            ([_], Some(ModuleName::Newtype(ty))) => {
                self.index_refer(&segments[0], crate::index::Key::Type(segments[0].name.clone()));
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                let value = self.expr(cx, &arg.value, Some(inner));
                let value = self.coerce(value, inner);
                Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span)
            }
            ([_], Some(ModuleName::Fn(id))) if Some(id) == self.main_fn() => self.main_called(span),
            ([_], Some(ModuleName::Fn(id))) => {
                self.index_refer(&segments[0], crate::index::Key::Fn(id.0));
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_], Some(ModuleName::Extern(id))) => self.extern_call(cx, id, args, expected, span),
            ([_, member], Some(ModuleName::Module(StdModule::Console))) => {
                // `print` and `println` only append to the output buffer and
                // cannot fail; delivery errors surface through `flush` (19.1).
                let writer = self.types.handle(Handle::Writer, Ty::UNIT);
                let reader = self.types.handle(Handle::Reader, Ty::UNIT);
                let (function, params, result) = match *member {
                    "print" => (StdFn::ConsolePrint, vec![Ty::STRING], Ty::UNIT),
                    "println" => (StdFn::ConsolePrintln, vec![Ty::STRING], Ty::UNIT),
                    "eprint" => (StdFn::ConsoleEprint, vec![Ty::STRING], Ty::UNIT),
                    "eprintln" => (StdFn::ConsoleEprintln, vec![Ty::STRING], Ty::UNIT),
                    "flush" => {
                        let failing = self.types.intern(TyKind::Result(Ty::UNIT, self.io_error));
                        (StdFn::ConsoleFlush, Vec::new(), failing)
                    }
                    "readLine" => {
                        let line = self.types.intern(TyKind::Option(Ty::STRING));
                        (StdFn::ConsoleReadLine, Vec::new(), line)
                    }
                    "stdout" => (StdFn::ConsoleStream(0), Vec::new(), writer),
                    "stderr" => (StdFn::ConsoleStream(1), Vec::new(), writer),
                    "stdin" => (StdFn::ConsoleStream(2), Vec::new(), reader),
                    "setOutput" => (StdFn::ConsoleSet(0), vec![writer], Ty::UNIT),
                    "setError" => (StdFn::ConsoleSet(1), vec![writer], Ty::UNIT),
                    "setInput" => (StdFn::ConsoleSet(2), vec![reader], Ty::UNIT),
                    _ => {
                        let message = format!("module `console` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Testing))) => {
                let (function, param) = match *member {
                    // An assertion that fails, or a skip: both end the test
                    // or suite where they are called.
                    "failNow" | "skip" => {
                        let [arg] = args else {
                            return self.arity_error(span, 1, args.len());
                        };
                        let message = self.expr(cx, &arg.value, Some(Ty::STRING));
                        let message = Box::new(self.coerce(message, Ty::STRING));
                        // Cleanup runs to its end: a `skip` there is a panic (testing 7.4).
                        let code = match (*member, cx.deferred) {
                            ("failNow", _) => hir::FAIL_NOW,
                            (_, true) => hir::SKIP_IN_CLEANUP,
                            _ => hir::SKIP,
                        };
                        return Self::mk(hir::ExprKind::Panic(message, code), Ty::NEVER, span);
                    }
                    "withVirtualTime" => return self.with_virtual_time(cx, args, span),
                    "bench" => return self.bench(cx, args, span),
                    "fuzz" => return self.fuzz(cx, args, span),
                    "keep" => {
                        let [arg] = args else {
                            return self.arity_error(span, 1, args.len());
                        };
                        let value = self.expr(cx, &arg.value, None);
                        let call = hir::ExprKind::Call(Callee::Std(StdFn::Keep), vec![value.ty], vec![value]);
                        return Self::mk(call, Ty::UNIT, span);
                    }
                    "log" => (StdFn::TestLog, vec![Ty::STRING]),
                    "tags" => (StdFn::TestTags, vec![self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING))]),
                    "attach" => {
                        (StdFn::TestAttach, vec![Ty::STRING, Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))])
                    }
                    "snapshot" => return self.snapshot(cx, args, span),
                    _ => {
                        let message = format!("module `testing` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(param, Ty::UNIT);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member @ ("offer" | "tryOffer")], Some(ModuleName::Module(StdModule::InternalChannel))) => {
                let [sender, value] = args else { return self.arity_error(span, 2, args.len()) };
                let sender = self.expr(cx, &sender.value, None);
                let TyKind::Handle(Handle::Sender, item) = *self.types.kind(sender.ty) else {
                    let message = format!("`{member}` sends through a `Sender`, found {}", self.display(sender.ty));
                    self.diags.error("E1102", sender.span, message);
                    return Self::error_expr(span);
                };
                let value = self.expr(cx, &value.value, Some(item));
                let value = self.coerce(value, item);
                let op = if *member == "offer" { hir::TaskOp::Offer } else { hir::TaskOp::TryOffer };
                Self::mk(hir::ExprKind::TaskOp(op, vec![sender, value]), Ty::INT, span)
            }
            (
                [_, member],
                Some(ModuleName::Module(
                    module @ (StdModule::InternalJson
                    | StdModule::InternalMemory
                    | StdModule::InternalRegex
                    | StdModule::InternalReflect
                    | StdModule::InternalText
                    | StdModule::InternalMath
                    | StdModule::InternalMeta
                    | StdModule::InternalCodec
                    | StdModule::InternalHttp
                    | StdModule::InternalToml
                    | StdModule::InternalYaml
                    | StdModule::InternalMsgpack
                    | StdModule::InternalProtobuf
                    | StdModule::InternalGrpc
                    | StdModule::InternalCbor
                    | StdModule::InternalNet
                    | StdModule::InternalFuzz
                    | StdModule::InternalModel
                    | StdModule::InternalPanic
                    | StdModule::InternalTime
                    | StdModule::InternalContext
                    | StdModule::InternalFs
                    | StdModule::InternalEnv
                    | StdModule::InternalProcess
                    | StdModule::InternalSql
                    | StdModule::InternalImage),
                )),
            ) => {
                // `built()`: what `env.build()` reports, read now and kept as
                // a constant (standard library 14.3.6).
                if module == StdModule::InternalEnv && *member == "built" {
                    let described = self.built.as_ref().map(crate::built::describe).unwrap_or_default();
                    return Self::mk(hir::ExprKind::Str(described), Ty::STRING, span);
                }
                // `joined(values, separator)`: the elements' `Display`, with
                // the separator between, into one text, as `Array.join`.
                if module == StdModule::InternalText && *member == "joined" {
                    let [values, separator] = args else { return self.arity_error(span, 2, args.len()) };
                    let values = self.expr(cx, &values.value, None);
                    let separator = self.expr(cx, &separator.value, Some(Ty::STRING));
                    let separator = self.coerce(separator, Ty::STRING);
                    let joined = hir::ExprKind::ArrayOp(hir::ArrayOp::Join, vec![values, separator]);
                    return Self::mk(joined, Ty::STRING, span);
                }
                // `nativeOrder(values)` and `sortedNatively(values)`: what
                // the runtime sorts, as `Array.sorted` does.
                if module == StdModule::InternalText && matches!(*member, "nativeOrder" | "sortedNatively") {
                    let [values] = args else { return self.arity_error(span, 1, args.len()) };
                    let values = self.expr(cx, &values.value, None);
                    let (op, ty) = match *member {
                        "nativeOrder" => (hir::ArrayOp::NativeOrder, Ty::BOOL),
                        _ => (hir::ArrayOp::SortedNatively, values.ty),
                    };
                    return Self::mk(hir::ExprKind::ArrayOp(op, vec![values]), ty, span);
                }
                if module == StdModule::InternalReflect && matches!(*member, "box" | "describe") {
                    let [arg] = args else {
                        return self.arity_error(span, 1, args.len());
                    };
                    let value = self.expr(cx, &arg.value, None);
                    let (op, result) = match *member {
                        "box" => (hir::ReflectOp::Box, self.types.handle(Handle::ReflectBox, Ty::UNIT)),
                        _ => (hir::ReflectOp::Describe, Ty::STRING),
                    };
                    return Self::mk(hir::ExprKind::Reflect(op, value.ty, vec![value]), result, span);
                }
                let module = match module {
                    StdModule::InternalJson => "json",
                    StdModule::InternalMemory => "memory",
                    StdModule::InternalRegex => "regex",
                    StdModule::InternalText => "text",
                    StdModule::InternalMath => "math",
                    StdModule::InternalMeta => "meta",
                    StdModule::InternalCodec => "codec",
                    StdModule::InternalHttp => "http",
                    StdModule::InternalToml => "toml",
                    StdModule::InternalYaml => "yaml",
                    StdModule::InternalMsgpack => "msgpack",
                    StdModule::InternalProtobuf => "protobuf",
                    StdModule::InternalGrpc => "grpc",
                    StdModule::InternalCbor => "cbor",
                    StdModule::InternalNet => "net",
                    StdModule::InternalFuzz => "fuzz",
                    StdModule::InternalModel => "model",
                    StdModule::InternalPanic => "panic",
                    StdModule::InternalTime => "time",
                    StdModule::InternalContext => "context",
                    StdModule::InternalFs => "fs",
                    StdModule::InternalEnv => "env",
                    StdModule::InternalProcess => "process",
                    StdModule::InternalSql => "sql",
                    StdModule::InternalImage => "image",
                    _ => "reflect",
                };
                let found =
                    hir::NATIVES.iter().position(|n| n.receiver.is_none() && n.module == module && n.name == *member);
                let Some(index) = found else {
                    let message = format!("module `{}` has no function `{member}`", segments[0].name);
                    self.diags.error("E1001", segments[1].span, message);
                    return Self::error_expr(span);
                };
                let native = &hir::NATIVES[index];
                let params = native.params.iter().map(|&t| self.native_ty(t)).collect();
                let signature = Signature::simple(params, self.native_ty(native.result));
                let target = CallTarget::new(Callee::Std(StdFn::Native(index as u16)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Process))) => {
                use hir::ProcessFn as P;
                let command = self.types.handle(Handle::Command, Ty::UNIT);
                let s = Ty::STRING;
                let mut signature = match *member {
                    "command" => {
                        let arguments = self.types.intern(TyKind::Array(s));
                        let mut signature = Signature::simple(vec![s, arguments], command);
                        signature.names = vec!["program".into(), "arguments".into()];
                        signature.variadic = true;
                        signature
                    }
                    "shell" => Signature::simple(vec![s], command),
                    "exit" => Signature::simple(vec![Ty::INT], Ty::NEVER),
                    "signals" => {
                        let signal = self.process.expect("declared with the module").signal;
                        let wanted = self.types.intern(TyKind::Array(signal));
                        let mut signature =
                            Signature::simple(vec![wanted], self.types.handle(Handle::Receiver, signal));
                        signature.names = vec!["wanted".into()];
                        signature.variadic = true;
                        signature
                    }
                    _ => {
                        let message =
                            format!("module `process` has no function `{member}`{}", std_alias("process", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let function = match *member {
                    "command" => P::Command,
                    "shell" => P::Shell,
                    "signals" => P::Signals,
                    _ => P::Exit,
                };
                if function == P::Shell {
                    signature.names = vec!["text".into()];
                }
                let target = CallTarget::new(Callee::Std(StdFn::Process(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Env))) => {
                use hir::EnvFn as E;
                let s = Ty::STRING;
                let names = self.types.intern(TyKind::Array(s));
                let map = self.types.intern(TyKind::Map(s, s));
                let (function, params, result) = match *member {
                    "args" => (E::Args, vec![], names),
                    "get" => (E::Get, vec![("name", s)], self.types.intern(TyKind::Option(s))),
                    "vars" => (E::Vars, vec![], map),
                    "set" => (E::Set, vec![("name", s), ("value", s)], Ty::UNIT),
                    "setAll" => (E::SetAll, vec![("values", map)], Ty::UNIT),
                    "remove" => (E::Remove, vec![("name", s)], Ty::UNIT),
                    "currentDirectory" => (E::CurrentDirectory, vec![], s),
                    "setCurrentDirectory" => {
                        let error = self.env_error.expect("declared with the module");
                        (E::SetCurrentDirectory, vec![("path", s)], self.types.intern(TyKind::Result(Ty::UNIT, error)))
                    }
                    "syncNative" => (E::SyncNative, vec![], Ty::UNIT),
                    "hostname" => (E::Hostname, vec![], s),
                    "processId" => (E::ProcessId, vec![], Ty::INT),
                    "executable" => (E::Executable, vec![], s),
                    "invokedAs" => (E::InvokedAs, vec![], s),
                    _ => {
                        let message = format!("module `env` has no function `{member}`{}", std_alias("env", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let (names, params): (Vec<&str>, Vec<Ty>) = params.into_iter().unzip();
                let mut signature = Signature::simple(params, result);
                signature.names = names.into_iter().map(String::from).collect();
                let target = CallTarget::new(Callee::Std(StdFn::Env(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Io))) => {
                use hir::IoFn as I;
                let (function, params, result) = match *member {
                    "reader" => (I::Reader, vec![Ty::STRING], self.types.handle(Handle::Reader, Ty::UNIT)),
                    "discard" => (I::Discard, Vec::new(), self.types.handle(Handle::Writer, Ty::UNIT)),
                    "Buffer" => (I::BufferNew, Vec::new(), self.types.handle(Handle::Buffer, Ty::UNIT)),
                    _ => {
                        let message = format!("module `io` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(StdFn::Io(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Sync))) if args.is_empty() => {
                self.sync_empty_expected(&segments[1], args, expected, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Sync))) => self.sync_new(cx, member, args, expected, span),
            ([_, member], Some(ModuleName::Module(StdModule::Fs))) => {
                let fs = self.fs.expect("declared with the module");
                let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, fs.error));
                let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                let names = self.types.intern(TyKind::Array(Ty::STRING));
                let s = Ty::STRING;
                use hir::FsFn as F;
                let (function, params, result) = match *member {
                    "readText" => (F::ReadText, vec![("path", s)], failing(self, s)),
                    "readBytes" => (F::ReadBytes, vec![("path", s)], failing(self, bytes)),
                    "writeText" => (F::WriteText, vec![("path", s), ("text", s)], failing(self, Ty::UNIT)),
                    "writeBytes" => (F::WriteBytes, vec![("path", s), ("data", bytes)], failing(self, Ty::UNIT)),
                    "appendText" => (F::AppendText, vec![("path", s), ("text", s)], failing(self, Ty::UNIT)),
                    "exists" => (F::Exists, vec![("path", s)], Ty::BOOL),
                    "list" => (F::List, vec![("path", s)], failing(self, names)),
                    "createDirectory" => (F::CreateDirectory, vec![("path", s)], failing(self, Ty::UNIT)),
                    "rename" => (F::Rename, vec![("from", s), ("to", s)], failing(self, Ty::UNIT)),
                    "copy" => (F::Copy, vec![("from", s), ("to", s)], failing(self, Ty::UNIT)),
                    "remove" => (F::Remove, vec![("path", s), ("recursive", Ty::BOOL)], failing(self, Ty::UNIT)),
                    "open" => (F::Open, vec![("path", s), ("mode", fs.mode)], failing(self, fs.file)),
                    _ => {
                        let message = format!("module `fs` has no function `{member}`{}", std_alias("fs", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let (names, params): (Vec<&str>, Vec<Ty>) = params.into_iter().unzip();
                let mut signature = Signature::simple(params, result);
                signature.names = names.into_iter().map(String::from).collect();
                if function == F::Remove {
                    let recursive = Self::mk(hir::ExprKind::Bool(false), Ty::BOOL, span);
                    signature.defaults = vec![None, Some(recursive)];
                }
                let target = CallTarget::new(Callee::Std(StdFn::Fs(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Path))) => {
                let part = self.types.intern(TyKind::Option(Ty::STRING));
                use hir::PathFn as P;
                let (function, params, result) = match *member {
                    "join" => (P::Join, vec![Ty::STRING, Ty::STRING], Ty::STRING),
                    "parent" => (P::Parent, vec![Ty::STRING], part),
                    "fileName" => (P::FileName, vec![Ty::STRING], part),
                    "extension" => (P::Extension, vec![Ty::STRING], part),
                    "isAbsolute" => (P::IsAbsolute, vec![Ty::STRING], Ty::BOOL),
                    _ => {
                        let message = format!("module `path` has no function `{member}`{}", std_alias("path", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(StdFn::Path(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Time))) => {
                let (duration, instant, datetime) =
                    (self.duration.unwrap(), self.instant.unwrap(), self.datetime.unwrap());
                let (function, params, result) = match *member {
                    "sleep" => (StdFn::TimeSleep, vec![duration], Ty::UNIT),
                    "now" => (StdFn::TimeNow, vec![], instant),
                    "unixTime" => (StdFn::TimeUnix, vec![], duration),
                    "utcNow" => (StdFn::TimeUtcNow, vec![], datetime),
                    "utc" => (StdFn::TimeUtc, vec![duration], datetime),
                    _ => {
                        let Some(factor) = Self::time_unit(member) else {
                            let message =
                                format!("module `time` has no function `{member}`{}", std_alias("time", member));
                            self.diags.error("E1001", segments[1].span, message);
                            return Self::error_expr(span);
                        };
                        // A count in nanoseconds; one that does not fit
                        // panics with `P0005` (14.3.1).
                        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                        let count = self.expr(cx, &arg.value, Some(Ty::INT));
                        let count = self.coerce(count, Ty::INT);
                        let factor = Self::mk(hir::ExprKind::Int(factor), Ty::INT, span);
                        let nanos = Self::mk(
                            hir::ExprKind::Binary(BinOp::Mul, Box::new(count), Box::new(factor)),
                            Ty::INT,
                            span,
                        );
                        return Self::mk(hir::ExprKind::Retype(Box::new(nanos)), duration, span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([name], None) if Self::numeric_type(name).is_some() => {
                let to = Self::numeric_type(name).unwrap();
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let value = self.expr(cx, &arg.value, None);
                if value.ty == to {
                    let message = format!("the value is already `{name}`: remove `{name}(...)` around it");
                    self.diags.warning("W1007", span, message);
                }
                match self.conversion_checked(value.ty, to) {
                    _ if value.ty == Ty::ERROR => Self::error_expr(span),
                    Some(false) => Self::mk(hir::ExprKind::Convert(Box::new(value)), to, span),
                    Some(true) => {
                        let ty = self.types.intern(TyKind::Result(to, self.conversion_error));
                        Self::mk(hir::ExprKind::Convert(Box::new(value)), ty, span)
                    }
                    None => {
                        let message = format!("there is no conversion from {} to `{name}`", self.display(value.ty));
                        self.diags.error("E1103", span, message);
                        Self::error_expr(span)
                    }
                }
            }
            (_, None) => {
                self.unknown_path(cx, segments, span);
                // The arguments' unknown names too, as other modules not
                // imported, and the bindings they use; not what follows from
                // a call of nothing.
                let before = self.diags.list.len();
                for arg in args.iter().filter(|a| !matches!(a.value.kind, ast::ExprKind::Closure(_))) {
                    self.expr(cx, &arg.value, None);
                }
                let mut index = 0;
                self.diags.list.retain(|d| {
                    index += 1;
                    index <= before || d.code == Some("E1001")
                });
                self.errors_counted.set((0, 0));
                Self::error_expr(span)
            }
            _ => self.unsupported_expr(callee.span, "calling this expression"),
        }
    }

    fn arity_error(&mut self, span: Span, expected: usize, found: usize) -> hir::Expr {
        let noun = if expected == 1 { "argument" } else { "arguments" };
        let message = format!("expected {expected} {noun}, found {found}");
        self.diags.error("E1102", span, message);
        Self::error_expr(span)
    }

    /// A call of `callee` with `signature`. A dot-call passes its receiver
    /// already checked; otherwise every parameter comes from `args`, which
    /// may be named, variadic or spread (11.3). Arguments are evaluated in
    /// the order they are written.
    /// Whether `generic` is in `ty` as the error of a result, and whether it is
    /// anywhere else.
    fn where_mentioned(&mut self, ty: Ty, generic: Ty) -> (bool, bool) {
        let both = |a: (bool, bool), b: (bool, bool)| (a.0 || b.0, a.1 || b.1);
        match self.types.kind(ty).clone() {
            TyKind::Result(ok, error) if error == generic => (true, self.where_mentioned(ok, generic).1),
            TyKind::Result(ok, error) => {
                let ok = self.where_mentioned(ok, generic);
                let error = self.where_mentioned(error, generic);
                both(ok, error)
            }
            TyKind::Fn(params, result, ..) => {
                let mut found = self.where_mentioned(result, generic);
                for param in params {
                    let inner = self.where_mentioned(param, generic);
                    found = both(found, inner);
                }
                found
            }
            TyKind::Option(inner) | TyKind::Array(inner) | TyKind::Deque(inner) | TyKind::Set(inner) => {
                self.where_mentioned(inner, generic)
            }
            _ if ty == generic => (false, true),
            _ => {
                let marked = self.types.subst(ty, &HashMap::from([(generic, Ty::UNIT)]));
                (false, marked != ty)
            }
        }
    }

    fn call_signature(
        &mut self,
        cx: &mut FnCx,
        target: CallTarget,
        receiver: Option<hir::Expr>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if let Callee::Fn(id) = target.callee {
            match (self.measuring, self.calling) {
                (Some(test), _) => _ = self.test_calls.entry(test).or_default().insert(id),
                (None, Some(caller)) => _ = self.calls.entry(caller).or_default().insert(id),
                (None, None) => {}
            }
        }
        let CallTarget { callee, signature, explicit, prebound, expected } = target;
        if let Callee::Fn(id) = callee {
            self.repeatable(id, span);
            self.check_time_literals(id, &signature, receiver.is_some() as usize, args);
            self.check_template_literals(id, args);
            self.check_regex_literal(id, args);
            self.note_model_name(id, args);
            if self.is_std_fn(id, "cli", "cli.markup")
                && let Some(arg) = args.first()
                && let ast::ExprKind::Str(segments) = &arg.value.kind
                && let Some(problem) = markup_problem(segments)
            {
                self.diags.error("E1907", arg.value.span, problem);
            }
        }
        let signature = &signature;
        if signature.is_unsafe && !cx.unsafe_region {
            let message = if signature.foreign {
                format!(
                    "`{}` is a C function: call it inside `unsafe {{ ... }}`, once its preconditions hold",
                    signature.name
                )
            } else {
                format!(
                    "`{}` is `unsafe`: call it inside `unsafe {{ ... }}`, once its preconditions hold",
                    signature.name
                )
            };
            self.diags.error("E1701", span, message);
        }
        let first = receiver.is_some() as usize;
        let count = signature.params.len() - first;
        let fixed = count - signature.variadic as usize;
        // Which argument fills each parameter, then the variadic elements.
        let mut slots: Vec<Option<usize>> = vec![None; fixed];
        let (mut pack, mut spread) = (Vec::new(), None);
        let mut named = false;
        for (index, arg) in args.iter().enumerate() {
            let position = match &arg.name {
                None if named => {
                    self.diags.error("E1102", arg.span, "positional arguments must come before named ones");
                    return Self::error_expr(span);
                }
                None => index,
                Some(name) => {
                    named = true;
                    match signature.names[first..].iter().position(|n| *n == name.name) {
                        Some(position) => {
                            if let Callee::Fn(id) = callee {
                                self.index_refer(name, crate::index::Key::Param(id.0, name.name.clone()));
                            }
                            position
                        }
                        None if signature.names[first..].iter().all(String::is_empty) => {
                            let message = "a standard function takes positional arguments; remove the name";
                            self.diags.error("E1102", name.span, message);
                            return Self::error_expr(span);
                        }
                        None => {
                            let known: Vec<String> =
                                signature.names[first..].iter().map(|n| format!("`{n}`")).collect();
                            let message = format!(
                                "no parameter is named `{}`; the parameters are {}",
                                name.name,
                                known.join(", ")
                            );
                            self.diags.error("E1001", name.span, message);
                            return Self::error_expr(span);
                        }
                    }
                }
            };
            if position >= fixed && signature.variadic {
                // Single values may come first; one spread goes last (11.3).
                if spread.is_some() {
                    let message = "a spread `...values` is the last argument, and a call has one at most";
                    self.diags.error("E1102", arg.span, message);
                    return Self::error_expr(span);
                }
                if arg.name.is_some() && !arg.spread {
                    let name = &signature.names[first + fixed];
                    let message = format!("a named variadic argument is an array spread: `{name}: ...[a, b]`");
                    self.diags.error("E1102", arg.span, message);
                    return Self::error_expr(span);
                }
                if arg.spread {
                    spread = Some(index)
                } else {
                    pack.push(index)
                }
                continue;
            }
            if arg.spread {
                self.diags.error("E1102", arg.span, "`...` expands an array only into a variadic parameter");
                return Self::error_expr(span);
            }
            if position >= fixed {
                return self.arity_error(span, fixed, args.len());
            }
            if slots[position].replace(index).is_some() {
                let message = format!("parameter `{}` is given twice", signature.names[first + position]);
                self.diags.error("E1102", arg.span, message);
                return Self::error_expr(span);
            }
        }
        let omitted = |position: usize| slots[position].is_none() && !signature.has_default(first + position);
        if let Some(missing) = (0..fixed).find(|&position| omitted(position)) {
            let message = match signature.names[first + missing].as_str() {
                // A standard function's parameters have no names.
                "" => format!(
                    "missing argument {}, of type {}",
                    missing + 1,
                    self.display(signature.params[first + missing])
                ),
                name => {
                    format!("missing argument `{name}`, of type {}", self.display(signature.params[first + missing]))
                }
            };
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        // Type arguments come from explicit arguments, then from the values.
        let mut inference = Inference::new(signature.generics.clone());
        inference.map.extend(prebound);
        if !explicit.is_empty() {
            if explicit.len() != signature.generics.len() {
                let message =
                    format!("expected {} type argument(s), found {}", signature.generics.len(), explicit.len());
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            inference.map.extend(signature.generics.iter().copied().zip(explicit));
        }
        if let Some(receiver) = &receiver {
            self.unify(&mut inference, signature.params[0], receiver.ty);
        }
        let element = signature.variadic.then(|| match self.types.kind(signature.params[signature.params.len() - 1]) {
            TyKind::Array(element) => *element,
            _ => Ty::ERROR,
        });
        let parameter_of = |index: usize| match slots.iter().position(|&s| s == Some(index)) {
            Some(position) => first + position,
            None => signature.params.len() - 1,
        };
        let type_of = |index: usize| match (slots.contains(&Some(index)), spread == Some(index)) {
            (true, _) | (false, true) => signature.params[parameter_of(index)],
            (false, false) => element.unwrap_or(Ty::ERROR),
        };
        // Variadic elements are values; every other argument has its parameter's mode.
        let mode_of = |index: usize| {
            if slots.contains(&Some(index)) { signature.modes[parameter_of(index)] } else { ast::Mode::Value }
        };
        // Check the arguments in the order they are written; an unsuffixed
        // literal waits until the others have fixed its type.
        let mut values: Vec<Option<hir::Expr>> = (0..args.len()).map(|_| None).collect();
        for pass in 0..2 {
            // What the call is expected to give fixes what the arguments
            // that wait for context, such as closures, may not.
            if pass == 1
                && let Some(expected) = expected
                && values.iter().any(Option::is_none)
            {
                self.unify(&mut inference, signature.result, expected);
            }
            for (index, arg) in args.iter().enumerate() {
                if values[index].is_some() {
                    continue;
                }
                let ty = type_of(index);
                // Any value becomes a `reflect.Value` where one is a parameter (27.10).
                let dynamic = self.reflect_value_ty() == Some(ty);
                // A partly known `fn(A): B` waits, for an argument that needs
                // context, until the other arguments and the expected result
                // have fixed what they can.
                let waits = pass == 0 && self.needs_context(&arg.value);
                let expected = match dynamic {
                    true => None,
                    false => self
                        .inferred(&inference, ty)
                        .or_else(|| self.call_expectation(&inference, ty))
                        .or_else(|| {
                            let annotated = matches!(&arg.value.kind, ast::ExprKind::Closure(c) if c.params.iter().all(|p| p.ty.is_some()));
                            if waits { None } else { self.fn_expectation(&inference, ty, annotated) }
                        }),
                };
                if pass == 0 && expected.is_none() && self.needs_context(&arg.value) {
                    continue;
                }
                let mode = mode_of(index);
                let total = if arg.mode == ast::Mode::Value { self.total_as_fallible(cx, arg, ty) } else { None };
                let value = if let Some(value) = total {
                    value
                } else {
                    self.argument_value(
                        cx,
                        arg,
                        expected,
                        mode,
                        parameter_of(index) < first + usize::from(signature.receiver.is_some()) && first == 0,
                    )
                };
                let value = if dynamic && value.ty != ty && !arg.spread { self.reflect_value(value) } else { value };
                self.unify(&mut inference, ty, value.ty);
                // A closure for a `fn(A): B` parameter fixes what `B` mentions.
                if matches!(self.types.kind(ty), TyKind::Fn(..))
                    && let Some(Callable { params, result, variadic, modes, is_unsafe, .. }) = self.callable(value.ty)
                {
                    let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                    self.unify(&mut inference, ty, found);
                }
                // A callable bound to `F: Call[fn(A): B]` also fixes `A` and `B`.
                if let Some(&(_, bound)) = self.call_bounds.get(&ty)
                    && let Some(Callable { params, result, variadic, modes, is_unsafe, .. }) = self.callable(value.ty)
                {
                    let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                    self.unify(&mut inference, bound, found);
                }
                values[index] = Some(value);
            }
        }
        if let Some(expected) = expected {
            self.unify(&mut inference, signature.result, expected);
            // `call(...)?` where its success type is expected, which may be
            // an option when the call gives a result.
            match (self.types.kind(signature.result).clone(), self.types.kind(expected)) {
                (TyKind::Result(..), TyKind::Result(..))
                | (TyKind::Option(_), TyKind::Result(..) | TyKind::Option(_)) => {}
                (TyKind::Result(success, _) | TyKind::Option(success), _) => {
                    self.unify(&mut inference, success, expected)
                }
                _ => {}
            }
        }
        self.infer_from_bounds(&mut inference);
        // An error nothing fixes is that of what does not fail (12.1).
        for &generic in &signature.generics {
            if let std::collections::hash_map::Entry::Vacant(slot) = inference.map.entry(generic) {
                let types: Vec<Ty> = signature.params.iter().copied().chain([signature.result]).collect();
                let (error, elsewhere) = types.iter().fold((false, false), |(error, elsewhere), &ty| {
                    let (e, o) = self.where_mentioned(ty, generic);
                    (error || e, elsewhere || o)
                });
                if error && !elsewhere {
                    slot.insert(Ty::NEVER);
                }
            }
        }
        let unknown: Vec<String> = signature
            .generics
            .iter()
            .filter(|g| !inference.map.contains_key(g))
            .map(|&g| format!("`{}`", self.types.display(g)))
            .collect();
        if !unknown.is_empty() {
            let message = format!(
                "cannot infer {} from the arguments; write the type arguments: `name[...](...)`",
                unknown.join(", ")
            );
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        let c_arguments = std::mem::replace(&mut self.c_arguments, signature.foreign);
        for (index, value) in values.iter_mut().enumerate() {
            let ty = self.types.subst(type_of(index), &inference.map);
            let raw = value.take().unwrap();
            *value = Some(self.finish_argument(cx, raw, ty, mode_of(index)));
        }
        self.c_arguments = c_arguments;
        // Arguments with whether they are shared `ref` loans; a plain
        // `self` receiver is one.
        let lent = values.iter().enumerate().filter_map(|(i, v)| v.as_ref().map(|v| (v, mode_of(i) == ast::Mode::Ref)));
        let receiver_shared = signature.modes.first() == Some(&ast::Mode::Ref);
        self.check_loan_overlap(lent.chain(receiver.iter().map(|r| (r, receiver_shared))), span);
        let type_args: Vec<Ty> = signature.generics.iter().map(|g| inference.map[g]).collect();
        // The arguments must meet the bounds of the parameters they replace.
        for (&param, &actual) in signature.generics.iter().zip(&type_args) {
            if let Some(missing) = self.bound_holds(param, actual, &inference.map, 0) {
                // The traits the compiler implements by itself say why it did not.
                let automatic = matches!(
                    missing.as_str(),
                    "`reflect.Reflect`" | "`serialization.Encode`" | "`serialization.Decode`"
                );
                let lacking = automatic.then(|| self.lacking_field(actual, param, &inference.map)).flatten();
                // A type named like the prelude's has its error already.
                let misnamed = |ty: Ty| self.misnamed.contains(&self.types.display(ty));
                if misnamed(actual) || lacking.as_ref().is_some_and(|(_, ty)| misnamed(*ty)) {
                    return Self::error_expr(span);
                }
                let why = match lacking {
                    Some((field, ty)) => format!(
                        "; the compiler implements it for records whose fields all implement it, and its field `{field}` is {}, which does not",
                        self.display(ty)
                    ),
                    None if automatic => {
                        "; the compiler implements it for records, enums and newtypes whose parts all implement it"
                            .into()
                    }
                    None => String::new(),
                };
                let message = format!(
                    "{} does not implement {missing}, which `{}` requires of `{}`{why}",
                    self.display(actual),
                    signature.name,
                    self.types.display(param),
                );
                self.diags.error("E1105", span, message);
                return Self::error_expr(span);
            }
        }
        if let Callee::Trait(trait_id, _) = callee {
            let def = &self.traits[trait_id.0 as usize];
            let self_ty = inference.map[&def.self_param];
            let trait_args: Vec<Ty> = def.params.iter().map(|p| inference.map[p]).collect();
            if !self.satisfies(self_ty, trait_id, &trait_args) {
                let trait_name = self.traits[trait_id.0 as usize].name.clone();
                // Only the module of the trait or of the type may implement it.
                let own = self.owner(self_ty).is_some_and(|o| o.module == self.module)
                    || self.trait_modules.get(&trait_id) == Some(&self.module);
                let message = if own {
                    format!(
                        "{} does not implement `{trait_name}`: add `impl {trait_name} for {}`",
                        self.display(self_ty),
                        self.types.display(self_ty)
                    )
                } else {
                    format!(
                        "{} does not implement `{trait_name}`, and only its module or the trait's can implement it: wrap it in a newtype of this module, `type Name = {}`, and implement it for that",
                        self.display(self_ty),
                        self.types.display(self_ty)
                    )
                };
                self.diags.error("E1105", span, message);
                return Self::error_expr(span);
            }
        }
        // The values a literal markup interpolates are text, whatever
        // brackets they hold (standard library 14.32).
        if let Callee::Fn(id) = callee
            && self.is_std_fn(id, "cli", "cli.markup")
            && let Some(Some(text)) = values.first_mut()
        {
            self.markup_values(text);
        }
        if matches!(signature.name.as_str(), "reflect.get" | "reflect.set" | "reflect.call" | "reflect.callMut") {
            self.check_reflection(&signature.name, &type_args, args);
        }
        if signature.name == "reflect.annotationsOf" {
            let result = self.types.subst(signature.result, &inference.map);
            return self.annotations_of(type_args[0], args, result, span);
        }
        if signature.name == "protobuf.knownMessages" {
            let result = self.types.subst(signature.result, &inference.map);
            return self.known_messages(result, span);
        }
        if let Callee::Fn(id) = callee
            && signature.receiver.is_none()
            && let Some(kind) = signature.name.rsplit('.').next().and_then(crate::embed::Kind::of)
            && let Some(scope) = self.fn_scopes.get(id.0 as usize)
            && self.module_paths.get(scope.module as usize).is_some_and(|p| p == &["std", "embed"])
        {
            let result = self.types.subst(signature.result, &inference.map);
            return self.embedded(kind, args, result, span);
        }
        if let Callee::Fn(id) = callee
            && signature.receiver.is_none()
            && matches!(signature.name.as_str(), "template.html" | "template.text")
            && let Some(scope) = self.fn_scopes.get(id.0 as usize)
            && self.module_paths.get(scope.module as usize).is_some_and(|p| p == &["std", "template"])
        {
            let result = self.types.subst(signature.result, &inference.map);
            return self.compiled_template(cx, args, result, span);
        }
        // `format.apply` writes an integer or a float as one, as an
        // interpolation does (standard library 10.6).
        if let Callee::Fn(id) = callee
            && self.is_std_fn(id, "format", "format.apply")
            && let &[Some(value), Some(spec)] = slots.as_slice()
            && let Some(typed) = self.typed_apply(type_args[0])
        {
            let (value, spec) = (values[value].take().unwrap(), values[spec].take().unwrap());
            let result = self.types.subst(signature.result, &inference.map);
            return self.apply_typed(typed, value, spec, result, span);
        }
        let result = self.types.subst(signature.result, &inference.map);
        let mut ordered: Vec<(Option<usize>, hir::Expr)> = Vec::new();
        if let Some(receiver) = receiver {
            ordered.push((None, receiver));
        }
        for (position, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(index) => ordered.push((Some(index), values[index].take().unwrap())),
                None => {
                    let mut value =
                        signature.defaults[first + position].clone().expect("an omitted argument has a default");
                    self.instantiate(&mut value, &inference.map);
                    value.span = span;
                    ordered.push((None, value));
                }
            }
        }
        if let Some(array) = signature.variadic.then(|| signature.params[signature.params.len() - 1]) {
            let array = self.types.subst(array, &inference.map);
            let items: Vec<hir::Expr> = pack.iter().map(|&i| values[i].take().unwrap()).collect();
            let value = match spread {
                Some(index) if items.is_empty() => values[index].take().unwrap(),
                // The single values, then the spread array's.
                Some(index) => {
                    let rest = values[index].take().unwrap();
                    let element = match self.types.kind(array) {
                        TyKind::Array(element) => *element,
                        _ => Ty::ERROR,
                    };
                    if !self.types.is_copy(element) {
                        let message = format!(
                            "a spread of {} values, which are not `Copy`, is the only variadic argument: put the single values into its array first",
                            self.display(element)
                        );
                        self.diags.error("E1102", rest.span, message);
                        return Self::error_expr(span);
                    }
                    let front = Self::mk(hir::ExprKind::Array(items), array, span);
                    Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Concat, vec![front, rest]), array, span)
                }
                None => Self::mk(hir::ExprKind::Array(items), array, span),
            };
            ordered.push((pack.first().copied().or(spread), value));
        }
        // Named arguments out of declaration order are evaluated into
        // temporaries first, in the order they are written.
        let in_order = ordered.iter().filter_map(|(i, _)| *i).collect::<Vec<_>>().is_sorted();
        if in_order {
            let args = ordered.into_iter().map(|(_, value)| value).collect();
            return Self::mk(hir::ExprKind::Call(callee, type_args, args), result, span);
        }
        let mut stmts = Vec::new();
        let mut args = Vec::new();
        for (index, value) in &mut ordered {
            let mut value = std::mem::replace(value, Self::error_expr(span));
            match (index, &mut value.kind) {
                (Some(index), hir::ExprKind::Region(_, bounds)) => {
                    // A region is lent where the call is; its bounds are
                    // evaluated where they are written.
                    for bound in bounds.iter_mut().flatten() {
                        let local = self.hidden_local(cx, bound.ty);
                        let read = Self::mk(hir::ExprKind::Local(local), bound.ty, bound.span);
                        stmts.push((*index, hir::Stmt::Let(local, std::mem::replace(&mut **bound, read))));
                    }
                    args.push(value);
                }
                (Some(index), kind) if !matches!(kind, hir::ExprKind::Loan(_)) => {
                    let local = self.hidden_local(cx, value.ty);
                    args.push(Self::mk(hir::ExprKind::Local(local), value.ty, value.span));
                    stmts.push((*index, hir::Stmt::Let(local, value)));
                }
                _ => args.push(value),
            }
        }
        stmts.sort_by_key(|(index, _)| *index);
        let call = Self::mk(hir::ExprKind::Call(callee, type_args, args), result, span);
        let block =
            hir::Block { stmts: stmts.into_iter().map(|(_, s)| s).collect(), tail: Some(Box::new(call)), ty: result };
        Self::mk(hir::ExprKind::Block(block), result, span)
    }

    /// One argument as written, before its parameter type is final; a
    /// `mut self` or `var self` receiver in a qualified call is a loan.
    /// One argument as written for a parameter passed as `mode`. A plain
    /// `self` receiver written first in a qualified call takes no keyword.
    /// A function that does not fail, named where one that may is expected:
    /// a closure that calls it, whose value is `ok` (12.1).
    fn total_as_fallible(&mut self, cx: &mut FnCx, arg: &ast::Arg, param: Ty) -> Option<hir::Expr> {
        if let TyKind::Fn(params, result, false, ..) = self.types.kind(param).clone()
            && matches!(self.types.kind(result), TyKind::Result(..))
            && let ast::ExprKind::Path(segments) = &arg.value.kind
            && let [name] = segments.as_slice()
            && self.lookup(cx, &name.name).is_none()
            && let Some(ModuleName::Fn(id)) = self.names.get(&name.name).copied()
            && let Some(signature) = self.signatures.get(id.0 as usize)
            && signature.params.len() == params.len()
            && signature.receiver.is_none()
            && signature.generics.is_empty()
            && !matches!(self.types.kind(signature.result), TyKind::Result(..))
        {
            // What the function takes is known, though the call may not know
            // it yet.
            // Its error, if the call gives one; one it leaves open is `Never`.
            let TyKind::Result(_, error) = self.types.kind(result).clone() else { unreachable!("matched") };
            let error = if matches!(self.types.kind(error), TyKind::Param(..)) { Ty::NEVER } else { error };
            let gives = self.types.intern(TyKind::Result(signature.result, error));
            let modes = signature.modes.clone();
            let expected = self.types.intern(TyKind::Fn(signature.params.clone(), gives, false, modes, false));
            let span = arg.value.span;
            let names: Vec<ast::Ident> =
                (0..params.len()).map(|at| ast::Ident { name: format!("#{at}"), span }).collect();
            let call = ast::Expr {
                kind: ast::ExprKind::Call {
                    callee: Box::new(arg.value.clone()),
                    args: names
                        .iter()
                        .map(|name| ast::Arg {
                            name: None,
                            value: ast::Expr { kind: ast::ExprKind::Path(vec![name.clone()]), span },
                            mode: ast::Mode::Value,
                            spread: false,
                            span,
                        })
                        .collect(),
                },
                span,
            };
            let params = names
                .into_iter()
                .map(|name| ast::ClosureParam { name, ty: None, mode: ast::Mode::Value, variadic: false })
                .collect();
            let body = ast::Block { stmts: Vec::new(), tail: Some(Box::new(call)), span };
            let closure = ast::Closure { is_unsafe: false, params, result: None, body };
            let wrapped = ast::Expr { kind: ast::ExprKind::Closure(Box::new(closure)), span };
            return Some(self.expr(cx, &wrapped, Some(expected)));
        }
        None
    }

    fn argument_value(
        &mut self,
        cx: &mut FnCx,
        arg: &ast::Arg,
        expected: Option<Ty>,
        mode: ast::Mode,
        receiver: bool,
    ) -> hir::Expr {
        let written = if receiver && mode == ast::Mode::Ref { ast::Mode::Value } else { mode };
        if arg.mode != written {
            let message = match (written, receiver) {
                (ast::Mode::Value, true) => {
                    format!("the receiver `self` is already a shared loan: remove `{}`", arg.mode.keyword())
                }
                (ast::Mode::Value, false) => format!("this parameter takes a value: remove `{}`", arg.mode.keyword()),
                (mode, _) => {
                    let text = self.sources.file_of(arg.value.span.start).slice(arg.value.span);
                    format!("this parameter is a `{0}` loan: write `{0} {text}`", mode.keyword())
                }
            };
            let code = if receiver { "E1407" } else { "E1102" };
            self.diags.error(code, arg.span, message);
        }
        self.expr(cx, &arg.value, expected)
    }

    /// An argument coerced to its final parameter type, or the loan of its
    /// place for a `mut self` or `var self` receiver.
    fn finish_argument(&mut self, cx: &mut FnCx, value: hir::Expr, ty: Ty, mode: ast::Mode) -> hir::Expr {
        let value = self.coerce(value, ty);
        if !matches!(mode, ast::Mode::Mut | ast::Mode::Var) || matches!(value.kind, hir::ExprKind::Loan(_)) {
            return value;
        }
        if let hir::ExprKind::Slice(..) = value.kind {
            return self.region(cx, value, mode);
        }
        let span = value.span;
        let operation = format!("passing it as `{}`", mode.keyword());
        match self.loan_place(cx, value, &operation, mode == ast::Mode::Var) {
            Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, span),
            None => Self::error_expr(span),
        }
    }

    /// `mut values[start:end:step]`: a region of an array lent to a `mut`
    /// parameter, which changes its elements but not its length (16.4).
    fn region(&mut self, cx: &mut FnCx, value: hir::Expr, mode: ast::Mode) -> hir::Expr {
        let (span, ty) = (value.span, value.ty);
        let hir::ExprKind::Slice(base, bounds) = value.kind else { unreachable!("called on a slice") };
        if !matches!(self.types.kind(base.ty), TyKind::Array(_)) {
            let message = format!("only a region of an array can be lent, not of {}", self.display(base.ty));
            self.diags.error("E1411", span, message);
            return Self::error_expr(span);
        }
        if mode == ast::Mode::Var {
            let message = format!(
                "a `var` loan takes a whole place, and a region keeps its length: take the parameter as `mut {}`",
                self.types.display(ty)
            );
            self.diags.error("E1411", span, message);
            return Self::error_expr(span);
        }
        match self.loan_place(cx, *base, "lending a region of it as `mut`", false) {
            Some(place) => Self::mk(hir::ExprKind::Region(place, bounds), ty, span),
            None => Self::error_expr(span),
        }
    }

    /// The `var` place a value expression denotes, for an operation that
    /// changes it (11.6); reports values that are not such a place.
    /// `structural` operations may change the extent of the root (`var`);
    /// the others keep it (`mut`, 16.3).
    fn loan_place(&mut self, cx: &mut FnCx, value: hir::Expr, operation: &str, structural: bool) -> Option<hir::Place> {
        let span = value.span;
        let mut steps = Vec::new();
        let mut current = value;
        let local = loop {
            match current.kind {
                hir::ExprKind::Local(local) => break local,
                hir::ExprKind::Field(base, index) => {
                    steps.push(hir::PlaceStep::Field(index));
                    current = *base;
                }
                hir::ExprKind::Retype(base) => current = *base,
                hir::ExprKind::Index(base, index) => {
                    steps.push(hir::PlaceStep::Index(*index));
                    current = *base;
                }
                hir::ExprKind::Error => return None,
                hir::ExprKind::RefValue(_) => {
                    self.diags.error("E1411", span, format!("{operation} would change it, but {REF_UNCHANGED}"));
                    return None;
                }
                _ => {
                    let message =
                        format!("{operation} changes its receiver, which must be a `var` binding or part of one");
                    self.diags.error("E1411", span, message);
                    return None;
                }
            }
        };
        steps.reverse();
        let place = hir::Place { local, path: steps };
        if let Some((lent, false)) = lent_conflict(cx, &place) {
            let lent = self.place_name(cx, lent);
            self.lent_error(&lent, false, span);
        }
        let hir::Place { local, path: steps } = place;
        if Some(local) == cx.env
            && let Some(hir::PlaceStep::Field(index)) = steps.first()
        {
            // Changing a capture changes the closure's own copy (11.8).
            let capture = &cx.captures[*index as usize];
            if !capture.mutable {
                let message = format!(
                    "{operation} changes `{0}`, which is not declared with `var`: declare it as `var {0}`",
                    capture.name
                );
                self.diags.error("E1411", span, message);
            }
            cx.mutates = true;
            return Some(hir::Place { local, path: steps });
        }
        let name = cx.locals[local.0 as usize].name.clone();
        let ty = self.display(cx.locals[local.0 as usize].ty);
        let binding = cx.loan_bindings.contains(&local);
        match cx.params[local.0 as usize] {
            Some(ast::Mode::Mut) if structural && steps.is_empty() && binding => {
                let message = format!(
                    "{operation} can change the length of `{name}`, a `mut` binding that keeps its extent; bind it as `var {name}`"
                );
                self.diags.error("E1411", span, message);
            }
            Some(ast::Mode::Mut) if structural && steps.is_empty() => {
                let message = format!(
                    "{operation} can change the length of `{name}`, a `mut` loan that keeps its extent; take it as `{name}: var {}`",
                    ty.trim_matches('`')
                );
                self.diags.error("E1411", span, message);
            }
            Some(ast::Mode::Mut | ast::Mode::Var) => {}
            Some(_) if binding => {
                let message = format!(
                    "{operation} changes `{name}`, a `ref` binding that only observes its part; borrow it with `mut {name}` in a `for` header"
                );
                self.diags.error("E1411", span, message);
            }
            Some(mode) if !cx.mutable[local.0 as usize] => {
                let what = if mode == ast::Mode::Ref { "a `ref` loan" } else { "a parameter" };
                let message = format!(
                    "{operation} changes `{name}`, which is {what}; take it as `{name}: {} {}` to change the caller's value",
                    if structural { "var" } else { "mut" },
                    ty.trim_matches('`')
                );
                self.diags.error("E1411", span, message);
            }
            _ if !cx.mutable[local.0 as usize] => {
                let message = format!(
                    "{operation} changes `{name}`, which is not declared with `var`: declare it as `var {name}`"
                );
                self.diags.error("E1411", span, message);
            }
            _ => {}
        }
        Some(hir::Place { local, path: steps })
    }

    fn unary(
        &mut self,
        cx: &mut FnCx,
        op: UnaryOp,
        operand: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        match op {
            UnaryOp::Neg => {
                if let ast::ExprKind::Int { value, suffix } = operand.kind {
                    let literal = self.int_literal(true, value, suffix, expected, span);
                    if self.types.as_int(literal.ty).is_some_and(|i| !i.signed()) {
                        let message = format!("negation is not defined for {}", self.display(literal.ty));
                        self.diags.error("E1102", span, message);
                    }
                    return literal;
                }
                if let ast::ExprKind::Float { text, suffix } = &operand.kind {
                    return self.float_literal(text, *suffix, true, expected, span);
                }
                if let ast::ExprKind::BigInt(digits) = &operand.kind {
                    return self.big_literal(&format!("-{digits}"), expected, span);
                }
                let value = self.expr(cx, operand, expected);
                let negatable = match self.types.kind(value.ty) {
                    TyKind::Int(int) => int.signed(),
                    TyKind::Float(_) | TyKind::Error | TyKind::Never => true,
                    _ => false,
                };
                if !negatable {
                    let shown = self.types.display(value.ty);
                    let mut message = format!("negation is not defined for {}", self.display(value.ty));
                    if self.number.is_some_and(|n| self.satisfies(value.ty, n, &[])) {
                        message.push_str(": write `0 - value`");
                    } else if self.additive.is_some_and(|a| self.satisfies(value.ty, a, &[])) {
                        message.push_str(&format!(": write `Additive.zero[{shown}]() - value`"));
                    }
                    self.diags.error("E1102", span, message);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
            UnaryOp::Not => {
                let value = self.expr(cx, operand, Some(Ty::BOOL));
                let value = self.coerce(value, Ty::BOOL);
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), Ty::BOOL, span)
            }
            UnaryOp::BitNot => {
                let value = self.expr(cx, operand, expected);
                if value.ty != Ty::BYTE && !self.bound_by(value.ty, self.integer) {
                    self.require_int(&value);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
        }
    }

    /// Whether an expression takes its type from the context: a literal
    /// without suffix, `none`, an empty collection or a construction of a
    /// generic enum. Inference checks such arguments last.
    fn needs_context(&self, e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::None | ast::ExprKind::Construct(..) | ast::ExprKind::Map(_) => true,
            ast::ExprKind::Array(items) => items.is_empty() || items.iter().all(|item| self.needs_context(item)),
            ast::ExprKind::Path(segments) | ast::ExprKind::Record { path: segments, .. } => {
                matches!(self.type_names.get(&segments[0].name), Some(TypeName::Template(_)))
            }
            ast::ExprKind::Call { callee, .. } => self.needs_context(callee),
            // A closure whose parameters take their types from the call.
            ast::ExprKind::Closure(closure) => closure.params.iter().any(|p| p.ty.is_none()),
            _ => Self::is_untyped_literal(e),
        }
    }

    fn is_untyped_literal(e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Int { suffix: None, .. } | ast::ExprKind::Float { suffix: None, .. } => true,
            ast::ExprKind::Unary(UnaryOp::Neg, inner) => Self::is_untyped_literal(inner),
            _ => false,
        }
    }

    /// The numbers inside nested arrays, and how many arrays deep they are.
    fn number_leaf(&self, ty: Ty) -> (Ty, u32) {
        match *self.types.kind(ty) {
            TyKind::Array(element) => {
                let (leaf, depth) = self.number_leaf(element);
                (leaf, depth + 1)
            }
            _ => (ty, 0),
        }
    }

    /// What an operand of arithmetic is expected to be given its peer: the
    /// peer's number type in the operand's own literal shape (10.6). Only
    /// literals take an expectation; other operands have their own type.
    fn arithmetic_expected(&mut self, operand: &ast::Expr, peer: Ty) -> Option<Ty> {
        match &operand.kind {
            ast::ExprKind::Array(items) => match items.first() {
                Some(first) => {
                    let inner = self.arithmetic_expected(first, peer)?;
                    Some(self.types.intern(TyKind::Array(inner)))
                }
                None => Some(peer),
            },
            _ if Self::is_untyped_literal(operand) => Some(self.number_leaf(peer).0),
            _ => None,
        }
    }

    /// Types the operands of an arithmetic operator. Without arrays they
    /// share a type, as in `operands`; with arrays only their numbers must.
    fn arithmetic_operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
    ) -> (hir::Expr, hir::Expr) {
        let (left, right) = if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let shape = self.arithmetic_expected(left, right.ty);
            (self.expr(cx, left, shape), right)
        } else {
            let left = self.expr(cx, left, expected);
            let shape = self.arithmetic_expected(right, left.ty).or(Some(left.ty));
            let right = self.expr(cx, right, shape);
            (left, right)
        };
        if self.number_leaf(left.ty).1 > 0 || self.number_leaf(right.ty).1 > 0 || left.ty == Ty::NEVER {
            return (left, right);
        }
        // A left operand that is not a number: the operator's error says
        // so, without another for the right operand. A duration and a
        // `T: Integer` compute as numbers do (18.10).
        let computes = matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Float(_) | TyKind::Error)
            || Some(left.ty) == self.duration
            || self.bound_by(left.ty, self.integer);
        if !computes {
            return (left, right);
        }
        self.common(left, right)
    }

    /// Two operands of one type: the right one coerced to the left's, or
    /// the narrower number widened to the other's (8.6).
    fn common(&mut self, left: hir::Expr, right: hir::Expr) -> (hir::Expr, hir::Expr) {
        if matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Float(_)) && self.converts(left.ty, right.ty) {
            let ty = right.ty;
            return (self.coerce(left, ty), right);
        }
        let ty = left.ty;
        (left, self.coerce(right, ty))
    }

    /// The type of `left op right` for `+ - * / %`, lifted over arrays
    /// (10.6): arrays pair element by element and a number pairs with every
    /// element. Reports operands that do not combine.
    fn arithmetic(&mut self, op: BinOp, left: Ty, right: Ty, span: Span) -> Ty {
        let ((number, left_depth), (right_number, right_depth)) = (self.number_leaf(left), self.number_leaf(right));
        if [number, right_number].iter().any(|&t| matches!(self.types.kind(t), TyKind::Error | TyKind::Never)) {
            return if left_depth >= right_depth { left } else { right };
        }
        // Floats have no `%` operator; their remainder is a named operation (17.4).
        // A duration adds and subtracts, and a `T: Integer` computes as an
        // integer does (18.10).
        let numeric = match self.types.kind(number) {
            TyKind::Int(_) => true,
            TyKind::Float(_) => op != BinOp::Rem,
            _ if Some(number) == self.duration => {
                matches!(op, BinOp::Add | BinOp::Sub) && left_depth + right_depth == 0
            }
            _ => self.bound_by(number, self.integer),
        };
        if !numeric {
            let mut message = format!("operator `{}` cannot be applied to {}", op.as_str(), self.display(left));
            let takes = match op {
                BinOp::Add | BinOp::Sub => Some("Additive"),
                BinOp::Mul | BinOp::Div => Some("Number"),
                _ => None,
            };
            if let Some(takes) = takes
                && left_depth == 0
            {
                if self.is_abstract(left) {
                    let shown = self.types.display(left);
                    message.push_str(&format!(": add the bound `{shown}: {takes}`, which gives it `{}`", op.as_str()));
                } else if matches!(self.types.kind(left), TyKind::Adt(_)) {
                    message
                        .push_str(&format!(": implement `{takes}` for it, which gives it `{}` (18.10)", op.as_str()));
                }
            }
            match (op, left_depth, right_depth) {
                (BinOp::Add, _, _) if left == Ty::STRING => {
                    message.push_str(": use interpolation, `\"{a}{b}\"`, or `a.concat(b)`")
                }
                (BinOp::Add, 1.., 1..) => message.push_str(": to join arrays use `a.concat(b)`"),
                (BinOp::Mul, 1.., 0) if right == Ty::INT => message.push_str(": to repeat an array use `a.repeat(n)`"),
                (BinOp::Mul, 0, 0) if left == Ty::STRING => message.push_str(": to repeat a string use `text.repeat(n)`"),
                (BinOp::Rem, 0, 0) if matches!(self.types.kind(left), TyKind::Float(_)) => message.push_str(
                    ": floats have no `%`, since remainders follow several conventions; the truncated one is `a - b * (a / b).truncate()`",
                ),
                _ if matches!(self.types.kind(left), TyKind::Result(..) | TyKind::Option(_)) => message.push_str(
                    ": the value may be missing or an error; get the number first with `?`, `.unwrapOr(value)` or `match`",
                ),
                _ => {}
            }
            self.diags.error("E1102", span, message);
            return Ty::ERROR;
        }
        if number != right_number {
            // Without arrays, `arithmetic_operands` already reported this.
            if left_depth > 0 || right_depth > 0 {
                let message = format!(
                    "operator `{}` cannot combine {} and {}: their numbers must have the same type",
                    op.as_str(),
                    self.display(left),
                    self.display(right)
                );
                self.diags.error("E1102", span, message);
            }
            return Ty::ERROR;
        }
        self.lift(left, right)
    }

    fn lift(&mut self, left: Ty, right: Ty) -> Ty {
        let element = |ty: Ty| match self.types.kind(ty) {
            TyKind::Array(element) => Some(*element),
            _ => None,
        };
        let inner = match (element(left), element(right)) {
            (Some(a), Some(b)) => self.lift(a, b),
            (Some(a), None) => self.lift(a, right),
            (None, Some(b)) => self.lift(left, b),
            (None, None) => return left,
        };
        self.types.intern(TyKind::Array(inner))
    }

    /// Types two operands that must share a type. An unsuffixed literal on
    /// the left adopts the right operand's type; evaluation order is unchanged.
    /// Two operands of one type; `comparison` says that a mismatch is one.
    fn operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
        comparison: bool,
    ) -> (hir::Expr, hir::Expr) {
        if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let left = self.expr(cx, left, Some(right.ty));
            let outer = std::mem::replace(&mut self.comparing, comparison.then_some(true));
            let left = self.coerce(left, right.ty);
            self.comparing = outer;
            (left, right)
        } else {
            let left = self.expr(cx, left, expected);
            let right = self.expr(cx, right, Some(left.ty));
            if left.ty == Ty::NEVER {
                return (left, right);
            }
            let outer = std::mem::replace(&mut self.comparing, comparison.then_some(false));
            let operands = self.common(left, right);
            self.comparing = outer;
            operands
        }
    }

    /// Warns of a comparison with a constant that is NaN (`W1008`).
    fn nan_comparison(&mut self, left: &hir::Expr, right: &hir::Expr, span: Span) {
        if [left, right].into_iter().any(|side| fold_float(&self.types, side).is_some_and(f64::is_nan)) {
            let message = "NaN is not equal to any value, itself included, nor less or greater than one: test it with `value.isNaN()`";
            self.diags.warning("W1008", span, message);
        }
    }

    fn binary(
        &mut self,
        cx: &mut FnCx,
        op: BinOp,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let operator_error = |this: &mut Self, ty: Ty| {
            let mut message = format!("operator `{}` cannot be applied to {}", op.as_str(), this.display(ty));
            if let (BinOp::Eq | BinOp::Ne, TyKind::Handle(Handle::Weak, _)) = (op, this.types.kind(ty)) {
                message.push_str(": compare the references they give, `left.get() == right.get()`");
            }
            this.diags.error("E1102", span, message);
        };
        let (left, right, ty) = match op {
            BinOp::And | BinOp::Or => {
                let left = self.expr(cx, left, Some(Ty::BOOL));
                let left = self.coerce(left, Ty::BOOL);
                let right = self.expr(cx, right, Some(Ty::BOOL));
                let right = self.coerce(right, Ty::BOOL);
                (left, right, Ty::BOOL)
            }
            BinOp::Shl | BinOp::Shr => {
                let left = self.expr(cx, left, expected);
                if left.ty != Ty::BYTE && !self.bound_by(left.ty, self.integer) {
                    self.require_int(&left);
                }
                let right = self.expr(cx, right, None);
                self.require_int(&right);
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let (left, right) = self.arithmetic_operands(cx, left, right, expected);
                // The trait's method, on a type that takes the operator by one (18.10).
                if let Some((trait_id, index)) = self.operator_trait(op, left.ty) {
                    let ty = left.ty;
                    let right = self.coerce(right, ty);
                    let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![left, right]);
                    return Self::mk(call, ty, span);
                }
                let ty = self.arithmetic(op, left.ty, right.ty, span);
                (left, right, ty)
            }
            // `Command | Pipeline` composes process plans (17.6).
            BinOp::BitOr if !Self::is_untyped_literal(left) => {
                let plan = |this: &Self, ty: Ty| {
                    matches!(this.types.kind(ty), TyKind::Handle(Handle::Command | Handle::Pipeline, _))
                };
                let hint = expected.filter(|&ty| !plan(self, ty));
                let left = self.expr(cx, left, hint);
                if plan(self, left.ty) {
                    let right = self.expr(cx, right, None);
                    if !plan(self, right.ty) && !self.types.has_error(right.ty) {
                        let message = format!(
                            "`|` joins a `Command` or `Pipeline` with another, not with {}",
                            self.display(right.ty)
                        );
                        self.diags.error("E1102", right.span, message);
                    }
                    let pipeline = self.types.handle(Handle::Pipeline, Ty::UNIT);
                    let callee = Callee::Std(StdFn::Process(hir::ProcessFn::Pipe));
                    return Self::mk(hir::ExprKind::Call(callee, Vec::new(), vec![left, right]), pipeline, span);
                }
                let right = self.expr(cx, right, Some(left.ty));
                let (left, right) = if left.ty == Ty::NEVER { (left, right) } else { self.common(left, right) };
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never)
                        || self.bound_by(left.ty, self.integer);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected, false);
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never)
                        || self.bound_by(left.ty, self.integer);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.operands(cx, left, right, None, true);
                if matches!(self.types.kind(left.ty), TyKind::Pointer(_)) {
                    let message =
                        "raw pointers have no `==`: compare `left.address() == right.address()` inside `unsafe`";
                    self.diags.error("E1102", span, message);
                } else if !self.types.is_equatable(left.ty) {
                    operator_error(self, left.ty);
                }
                self.nan_comparison(&left, &right, span);
                (left, right, Ty::BOOL)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.operands(cx, left, right, None, true);
                let ordered = matches!(
                    self.types.kind(left.ty),
                    TyKind::Int(_)
                        | TyKind::Float(_)
                        | TyKind::Char
                        | TyKind::Byte
                        | TyKind::String
                        | TyKind::Error
                        | TyKind::Never
                ) || self.bound_by(left.ty, self.integer);
                if !ordered {
                    // `a < b` is `Ordered.compare(a, b) < 0`.
                    if let Some((trait_id, index)) = self.ordering(left.ty) {
                        let ty = left.ty;
                        let right = self.coerce(right, ty);
                        let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![left, right]);
                        let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                        let compared =
                            hir::ExprKind::Binary(op, Box::new(Self::mk(call, Ty::INT, span)), Box::new(zero));
                        return Self::mk(compared, Ty::BOOL, span);
                    }
                    // Only the module that declares a type implements it there.
                    let local = self.owner(left.ty).is_some_and(|owner| owner.module == self.module);
                    if local && matches!(self.types.kind(left.ty), TyKind::Adt(_)) {
                        let name = self.types.display(left.ty);
                        let message = format!(
                            "operator `{}` cannot be applied to {}: implement `Ordered` for it, `impl Ordered for {name} {{ fn compare(self, other: {name}): Int {{ ... }} }}`, and the operators compare by it",
                            op.as_str(),
                            self.display(left.ty),
                        );
                        self.diags.error("E1102", span, message);
                    } else {
                        operator_error(self, left.ty);
                    }
                }
                self.nan_comparison(&left, &right, span);
                (left, right, Ty::BOOL)
            }
            BinOp::In => {
                let container = self.expr(cx, right, None);
                // A range or a string asks the prelude, the element first.
                let asked = match self.range_element(container.ty) {
                    Some(element) => Some(("prelude.rangeHas", element, vec![element])),
                    None if container.ty == Ty::STRING => Some(("prelude.textHas", Ty::CHAR, Vec::new())),
                    None => None,
                };
                if let Some((name, element, generics)) = asked
                    && let Some(&ModuleName::Fn(has)) = self.names.get(name)
                {
                    let item = self.expr(cx, left, Some(element));
                    let item = self.coerce(item, element);
                    let call = hir::ExprKind::Call(Callee::Fn(has), generics, vec![item, container]);
                    return Self::mk(call, Ty::BOOL, span);
                }
                if let TyKind::Map(element, _) | TyKind::Set(element) = *self.types.kind(container.ty) {
                    let item = self.expr(cx, left, Some(element));
                    let item = self.coerce(item, element);
                    let op = hir::MapOp::Contains;
                    return Self::mk(hir::ExprKind::MapOp(op, vec![container, item]), Ty::BOOL, span);
                }
                let (TyKind::Array(element) | TyKind::Deque(element)) = *self.types.kind(container.ty) else {
                    if container.ty != Ty::ERROR {
                        let message = format!("`in` is not defined for {}", self.display(container.ty));
                        self.diags.error("E1102", container.span, message);
                    }
                    return Self::error_expr(span);
                };
                if !self.types.is_equatable(element) {
                    operator_error(self, element);
                }
                // Membership observes the element first (5.12 orders operands
                // textually; the left operand is the element).
                let item = self.expr(cx, left, Some(element));
                let item = self.coerce(item, element);
                return Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Contains, vec![item, container]), Ty::BOOL, span);
            }
            BinOp::Range | BinOp::RangeInclusive => {
                let hint = expected.and_then(|ty| self.range_element(ty));
                let (start, end) = self.operands(cx, left, right, hint, false);
                return self.range_value(start, end, op == BinOp::RangeInclusive, span);
            }
        };
        Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span)
    }

    fn if_expr(
        &mut self,
        cx: &mut FnCx,
        condition: &ast::Expr,
        then: &ast::Block,
        otherwise: Option<&ast::Expr>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let condition = self.expr(cx, condition, Some(Ty::BOOL));
        let condition = Box::new(self.coerce(condition, Ty::BOOL));
        let Some(otherwise) = otherwise else {
            // Its value is used, but there is none when the condition is false.
            let used = expected != Some(Ty::UNIT);
            let then = self.block(cx, then, if used { None } else { Some(Ty::UNIT) });
            if used && ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&then.ty) {
                let message = "an `if` without `else` has no value when its condition is false: add `else { ... }` with a value of the same type";
                self.diags.error("E1102", span, message);
            }
            return Self::mk(hir::ExprKind::If(condition, then, None), Ty::UNIT, span);
        };
        // `if c { [] } else { items }`: the other branch gives the type an
        // empty literal needs, as the first does the other way round.
        if expected.is_none() && then.tail.as_deref().is_some_and(empty_literal) {
            let other = self.expr(cx, otherwise, None);
            let then_expected = (![Ty::NEVER, Ty::ERROR].contains(&other.ty)).then_some(other.ty);
            let then = self.block(cx, then, then_expected);
            let ty = if then.ty == Ty::NEVER { other.ty } else { then.ty };
            return Self::mk(hir::ExprKind::If(condition, then, Some(Box::new(other))), ty, span);
        }
        let then = self.block(cx, then, expected);
        let other_expected = expected.or((then.ty != Ty::NEVER).then_some(then.ty));
        let other = self.expr(cx, otherwise, other_expected);
        let ty = if then.ty == Ty::NEVER {
            other.ty
        } else if other.ty == Ty::NEVER {
            then.ty
        } else {
            if expected.is_none() && other.ty != then.ty && ![then.ty, other.ty].contains(&Ty::ERROR) {
                let message = format!(
                    "`if` branches have different types: {} and {}",
                    self.display(then.ty),
                    self.display(other.ty)
                );
                self.diags.error("E1102", other.span, message);
            }
            then.ty
        };
        Self::mk(hir::ExprKind::If(condition, then, Some(Box::new(other))), ty, span)
    }
}

/// `[]`, `Set[]` or `[:]`, whose type only what is expected of it can give.
fn empty_literal(expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::Array(items) | ast::ExprKind::Set(items) => items.is_empty(),
        ast::ExprKind::Map(entries) => entries.is_empty(),
        _ => false,
    }
}

/// Whether a `sync.Atomic` of `ty` uses atomic instructions rather than a
/// lock: integers of up to 64 bits, `Bool`, `Char` and `Byte`.
fn atomic_scalar(types: &Types, ty: Ty) -> bool {
    types.as_int(ty).is_some_and(|int| int.bits() <= 64) || [Ty::BOOL, Ty::CHAR, Ty::BYTE].contains(&ty)
}

/// Whether a `for` pattern borrows: `Some(true)` with any `mut` or `var`
/// binding, `Some(false)` with only `ref` ones (13.3).
fn loan_kind(pattern: &ast::Pattern) -> Option<bool> {
    let mut found = None;
    let mut note = |mode: ast::Mode| {
        let exclusive = mode != ast::Mode::Ref;
        found = Some(found.unwrap_or(false) || exclusive);
    };
    fn walk(pattern: &ast::Pattern, note: &mut dyn FnMut(ast::Mode)) {
        match &pattern.kind {
            ast::PatternKind::Loan(mode, _) => note(*mode),
            ast::PatternKind::Tuple(items) | ast::PatternKind::Constructor { args: items, .. } => {
                items.iter().for_each(|item| walk(item, note))
            }
            ast::PatternKind::Record { fields, .. } => {
                fields.iter().filter_map(|f| f.pattern.as_ref()).for_each(|item| walk(item, note))
            }
            ast::PatternKind::Array { items, rest } => {
                items.iter().for_each(|item| walk(item, note));
                if let Some(Some((mode, _))) = rest
                    && *mode != ast::Mode::Value
                {
                    note(*mode);
                }
            }
            ast::PatternKind::Member { inner, .. } => walk(inner, note),
            _ => {}
        }
    }
    walk(pattern, &mut note);
    found
}

/// Whether a use of `place` overlaps a collection lent to the bindings of
/// an enclosing `for`, and then whether that loan is exclusive.
fn lent_conflict<'c>(cx: &'c FnCx, place: &hir::Place) -> Option<(&'c hir::Place, bool)> {
    let overlaps = |lent: &hir::Place| {
        lent.local == place.local
            && lent.path.iter().zip(&place.path).all(|steps| match steps {
                (hir::PlaceStep::Field(x), hir::PlaceStep::Field(y)) => x == y,
                (hir::PlaceStep::Index(x), hir::PlaceStep::Index(y)) => {
                    !matches!((&x.kind, &y.kind), (hir::ExprKind::Int(i), hir::ExprKind::Int(j)) if i != j)
                }
                _ => true,
            })
    };
    cx.lent.iter().filter(|(lent, _)| overlaps(lent)).max_by_key(|(_, exclusive)| *exclusive).map(|(l, e)| (l, *e))
}

/// `E1001` for an unknown type, pointing names of other languages at
/// Tondo's.
fn unknown_type(name: &str) -> String {
    let hint = match name {
        "int" | "integer" | "Integer" | "long" | "Long" | "i64" | "isize" | "number" | "Number" => "Tondo writes `Int`",
        "i8" => "Tondo writes `Int8`",
        "i16" | "short" => "Tondo writes `Int16`",
        "i32" => "Tondo writes `Int32`",
        "u8" | "byte" => "Tondo writes `Byte`, or `UInt8` for arithmetic",
        "u16" => "Tondo writes `UInt16`",
        "u32" => "Tondo writes `UInt32`",
        "u64" | "usize" | "ulong" | "uint" => "Tondo writes `UInt`",
        "i128" => "Tondo writes `Int128`",
        "u128" => "Tondo writes `UInt128`",
        "float" | "double" | "Double" | "f64" => "Tondo writes `Float`",
        "f32" => "Tondo writes `Float32`",
        "string" | "str" | "&str" => "Tondo writes `String`",
        "char" | "rune" | "Character" => "Tondo writes `Char`",
        "bool" | "boolean" | "Boolean" => "Tondo writes `Bool`",
        "void" | "unit" | "None" => "Tondo writes `Unit`, or leaves out the result type",
        "Vec" | "List" | "ArrayList" | "vector" | "Vector" | "list" | "Slice" => "an array is `Array[T]`",
        "HashMap" | "Dictionary" | "dict" | "Dict" | "TreeMap" | "BTreeMap" | "Hashtable" => "a map is `Map[K, V]`",
        "HashSet" | "TreeSet" | "BTreeSet" => "a set is `Set[T]`",
        "VecDeque" | "Queue" | "LinkedList" | "ArrayDeque" => "a double-ended queue is `Deque[T]`",
        "Option" | "Optional" | "Maybe" => "an optional value is `T?`",
        "Result" | "Either" => "a value or an error is `T ! E`",
        "Box" | "Rc" | "Arc" | "RefCell" | "Cell" => {
            "write the type itself: values are copied, and their storage is shared, by the language"
        }
        "any" | "Any" | "object" | "Object" | "dynamic" | "interface{}" => {
            "Tondo has no dynamic type: use a type parameter with a bound, as in `fn show[T: Display](value: T)`"
        }
        _ if name.len() <= 2 && name.chars().all(|c| c.is_ascii_uppercase()) => {
            "a type parameter is declared in brackets after the function's name: `fn first[T](values: Array[T]): T`"
        }
        _ => return format!("unknown type `{name}`"),
    };
    format!("unknown type `{name}`: {hint}")
}

/// `int` as `Int`.
fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The candidate closest to `name` when it looks like a typo of it: a few
/// edits away, fewer for a short name.
pub(crate) fn closest<'n>(name: &str, candidates: impl IntoIterator<Item = &'n str>) -> Option<&'n str> {
    let limit = match name.chars().count() {
        0..=2 => 0,
        3..=5 => 1,
        6..=8 => 2,
        _ => 3,
    };
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (distance(name, c), c))
        .filter(|&(d, _)| d <= limit)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c)
}

/// The edits, as insertions, deletions, substitutions and swaps of two
/// neighbours, that turn `a` into `b`; a different case is no edit.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let same = |x: char, y: char| x.eq_ignore_ascii_case(&y);
    let mut d: Vec<Vec<usize>> =
        (0..=a.len()).map(|i| (0..=b.len()).map(|j| if i == 0 { j } else { i }).collect()).collect();
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(!same(a[i - 1], b[j - 1]));
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && same(a[i - 1], b[j - 2]) && same(a[i - 2], b[j - 1]) {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// `a`, `a and b`, or `a, b and c`, each in backticks.
fn listed(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("`{i}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The names a pattern binds.
fn bound_idents<'p>(pattern: &'p ast::Pattern, names: &mut Vec<&'p ast::Ident>) {
    match &pattern.kind {
        ast::PatternKind::Binding(name) | ast::PatternKind::Loan(_, name) => names.push(name),
        ast::PatternKind::Tuple(parts) | ast::PatternKind::Constructor { args: parts, .. } => {
            parts.iter().for_each(|part| bound_idents(part, names))
        }
        ast::PatternKind::Array { items, rest } => {
            items.iter().for_each(|item| bound_idents(item, names));
            if let Some(Some((_, name))) = rest {
                names.push(name);
            }
        }
        ast::PatternKind::Record { fields, .. } => {
            for field in fields {
                match &field.pattern {
                    Some(inner) => bound_idents(inner, names),
                    None => names.push(&field.name),
                }
            }
        }
        ast::PatternKind::Member { inner, .. } => bound_idents(inner, names),
        _ => {}
    }
}

/// Whether a panic's code is the program's own: segments of a lowercase
/// letter followed by letters and digits, separated by dots (15.7).
fn own_panic_code(code: &str) -> bool {
    !code.is_empty()
        && code.split('.').all(|segment| {
            let mut chars = segment.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
        })
}

/// What is wrong with the styles of a literal markup of `std.cli`, if
/// anything (standard library 14.32): its values, interpolated, are text.
fn markup_problem(segments: &[ast::StrSegment]) -> Option<String> {
    const VALUE: char = '\u{0}';
    let text: String = segments
        .iter()
        .map(|s| match s {
            ast::StrSegment::Text(text) => text.clone(),
            ast::StrSegment::Expr(..) => VALUE.to_string(),
        })
        .collect();
    let colors = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "gray"];
    let decorations = ["bold", "dim", "italic", "underline", "strikethrough"];
    let color = |word: &str| {
        colors.contains(&word)
            || (word.len() == 7 && word.starts_with('#') && word[1..].chars().all(|c| c.is_ascii_hexdigit()))
    };
    let mut open: Vec<String> = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find(['[', ']']) {
        let after = &rest[at + 1..];
        if after.starts_with(&rest[at..at + 1]) {
            rest = &after[1..];
            continue;
        }
        if &rest[at..at + 1] == "]" {
            rest = after;
            continue;
        }
        let Some(close) = after.find(']') else {
            return Some("`[` starts a style that does not end: close it with `]`, or write `[[` for a bracket".into());
        };
        let tag = &after[..close];
        rest = &after[close + 1..];
        if tag.contains(VALUE) {
            return Some("a style is written in the markup, not given by a value: write it as `[bold]`".into());
        }
        if tag == "/" {
            if open.pop().is_none() {
                return Some("`[/]` closes no style: each closes the last `[style]` still open".into());
            }
            continue;
        }
        let words: Vec<&str> = tag.split_whitespace().collect();
        let mut valid = !words.is_empty();
        let mut background = false;
        for word in &words {
            valid &= match *word {
                "on" if !background => {
                    background = true;
                    continue;
                }
                word if background => color(word),
                word => color(word) || decorations.contains(&word),
            };
            background = false;
        }
        if !valid || background {
            return Some(format!(
                "`[{tag}]` is not a style: write colors ({}, or `#rrggbb`), `on` and a color for the background, and {}; `[[` writes a bracket",
                colors.join(", "),
                decorations.join(", ")
            ));
        }
        open.push(tag.to_string());
    }
    open.pop().map(|tag| format!("`[{tag}]` is not closed: close it with `[/]`"))
}
