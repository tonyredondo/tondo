//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

mod annotations;
mod calls;
mod contexts;
mod declarations;
mod expressions;
mod features;
mod foreign;
mod functions;
mod indexing;
mod literals;
mod natives;
mod operators;
mod owners;
mod patterns;
mod plugins;
mod program;
mod reach;
pub(crate) use reach::each_expr;
mod routes;
mod simd;
mod statements;
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
    // An editor, and a test of the standard library, see every body.
    let every_body =
        asked || ["TONDO_CHECK_STD", "TONDO_STD_WARNINGS"].iter().any(|name| std::env::var_os(name).is_some());
    let mut checker = Checker {
        index,
        every_body,
        inferring: 0,
        holes: Vec::new(),
        bare_variants: Vec::new(),
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
        dropped_calls: Vec::new(),
        impls: Vec::new(),
        impl_heads: HashMap::new(),
        heads_indexed: 0,
        showing: HashSet::new(),
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
        fractional: None,
        ordered: None,
        instant: None,
        datetime: None,
        time_error: None,
        compare_exchange: None,
        fs: None,
        module: 0,
        owners: HashMap::new(),
        served: HashSet::new(),
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
        instructions: HashMap::new(),
        instructions_used: HashSet::new(),
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
    let bare = std::mem::take(&mut checker.bare_variants);
    checker.diags.list.retain(|d| !(d.code == Some("W1004") && d.span.is_some_and(|span| bare.contains(&span))));
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
    /// `std.simd`: vectors (standard library 14.46).
    Simd,
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
    /// `std.internal.cpu`: the processor, for `std.cpu`.
    InternalCpu,
    /// `std.internal.library`: shared libraries, for `std.ffi`.
    InternalLibrary,
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
    Trait(TraitId),
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
        _ => return elsewhere(module, member),
    };
    format!("; use `{module}.{function}`")
}

/// What another module offers in place of a function a module lacks, as
/// another language keeps it there: a program's arguments are `env.args()`.
fn elsewhere(module: &str, member: &str) -> String {
    let function = match (module, member) {
        ("process" | "os" | "sys", "args" | "argv" | "arguments") => "env.args",
        ("process" | "os", "env" | "getenv" | "getEnv") => "env.get",
        ("process" | "os", "pid" | "getpid" | "id" | "processId") => "env.processId",
        ("process" | "os", "cwd" | "getcwd" | "currentDirectory") => "env.currentDirectory",
        ("process" | "os", "chdir" | "setCurrentDirectory") => "env.setCurrentDirectory",
        ("process" | "os", "hostname") => "env.hostname",
        ("env", "exit") => "process.exit",
        _ => return String::new(),
    };
    format!("; use `{function}`, from `import std.{}`", function.split('.').next().unwrap_or_default())
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
    /// Whether every body is checked, the standard library's too, and not
    /// only those of it the program reaches (`reach.rs`).
    every_body: bool,
    /// How many expressions being checked take their expected type from an
    /// inference that their own type then feeds: a `some(...)` there is not
    /// one W1021 can remove.
    inferring: u32,
    /// The values the `Navigate`s being checked go on with, innermost last.
    holes: Vec<(LocalId, Ty)>,
    /// Where a pattern names a variant alone (14.3), which the linker took
    /// for a binding whose name it found wrong.
    bare_variants: Vec<Span>,
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
    /// The functions of `std.cpu.x64` and `std.cpu.arm64` (standard library
    /// 14.47), each the instruction its calls become.
    instructions: HashMap<FnId, crate::instructions::Instruction>,
    /// The instructions the checked code uses, for `features.rs`.
    instructions_used: HashSet<crate::instructions::Instruction>,
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
    /// Calls whose result nothing uses, of functions not all known yet.
    dropped_calls: Vec<(FnId, Span)>,
    impls: Vec<ImplDef>,
    /// The `impl`s of each trait by the head of the type they are for, a
    /// type parameter's as `None`, so that asking whether a type has a
    /// trait looks only at those that may match; and how many are there.
    impl_heads: HashMap<(TraitId, Option<Head>), Vec<usize>>,
    heads_indexed: usize,
    /// The records and enums whose parts are being asked for `Display`,
    /// which a part that holds one of them again takes for granted.
    showing: HashSet<Ty>,
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
    fractional: Option<TraitId>,
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
    /// The types whose methods `std.mcp` serves, said once (E1912).
    served: HashSet<Ty>,
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
    /// The `map[key, default: value]` read as values, which a `mut` or
    /// `var` loan of the expression around takes as places: what is left
    /// changes nothing, and is an error with the read to write instead.
    entry_values: Vec<(Span, String)>,
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
    /// For a `defer` block: the `ref` parameters of its function it uses,
    /// each by the capture that holds it and the binding of the body that
    /// lends it, as a `ref` parameter (13.7).
    loans_kept: Vec<(u32, LocalId)>,
    /// Whether the body changes a capture (`CallMut`).
    mutates: bool,
    /// A closure without a declared result infers it from its body.
    infer_result: bool,
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
    /// Its `pub` methods that reflection cannot call, and why (`derive`).
    unreflected: Vec<(String, Span, String)>,
}

/// A binding a closure captures: read from `source` in the enclosing
/// context when the closure is created.
struct Capture {
    name: String,
    ty: Ty,
    source: hir::Expr,
    mutable: bool,
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
        ast::PatternKind::Tuple(parts)
        | ast::PatternKind::Constructor { args: parts, .. }
        | ast::PatternKind::Or(parts) => parts.iter().for_each(|part| bound_idents(part, names)),
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

/// Why a type has no `Reflect`, which the compiler implements for every type
/// of the program, and how to reflect a value of it.
pub(crate) const REFLECT_WHY: &str = "; the compiler implements it for every record, enum and newtype of the program: a field of one holds this value, which reflection carries as it is";
