//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

mod indexing;
mod routes;

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn, TraitId};
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
    let mut checker = Checker {
        index,
        index_exprs: Vec::new(),
        meta,
        types: Types::default(),
        diags,
        sources,
        names: HashMap::new(),
        type_names: HashMap::new(),
        aliases: Vec::new(),
        signatures: Vec::new(),
        methods: HashMap::new(),
        type_params: Vec::new(),
        self_type: None,
        implementing: None,
        traits: Vec::new(),
        trait_names: HashMap::new(),
        impls: Vec::new(),
        bounds: HashMap::new(),
        call_bounds: HashMap::new(),
        cursor_templates: HashMap::new(),
        closure_base: 0,
        closure_functions: Vec::new(),
        test_nodes: Vec::new(),
        fn_scopes: Vec::new(),
        duration: None,
        instant: None,
        datetime: None,
        time_error: None,
        compare_exchange: None,
        fs: None,
        module: 0,
        owners: HashMap::new(),
        method_visibility: HashMap::new(),
        prelude: None,
        contexts: Vec::new(),
        context_of: HashMap::new(),
        module_paths: Vec::new(),
        datetime_module: None,
        unrepeatable: HashMap::new(),
        interpolating: false,
        script_bindings: Vec::new(),
        scripting: false,
        naming: String::new(),
        comparing: None,
        panic_datum: None,
        closure_name: None,
        measuring: None,
        wires: HashMap::new(),
        intrinsic_owners: HashMap::new(),
        trait_modules: HashMap::new(),
        conversion_error: Ty::ERROR,
        collection_error: Ty::ERROR,
        text_error: Ty::ERROR,
        io_error: Ty::ERROR,
        env_error: None,
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
    (program, checker.index)
}

/// Prelude type names (26.3); they cannot be redeclared unqualified.
pub(crate) const PRELUDE_TYPES: [&str; 43] = [
    "NumericConversionError",
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
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
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
];

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
    InternalCbor,
    InternalNet,
    /// `std.internal.fuzz`: the runtime's side of `testing.fuzz`.
    InternalFuzz,
    /// `std.internal.panic`: what a panic carries beyond its message.
    InternalPanic,
    InternalTime,
    InternalContext,
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
}

/// The types `std.fs` declares when imported.
#[derive(Copy, Clone)]
struct FsTypes {
    error: Ty,
    kind: Ty,
    metadata: Ty,
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
}

struct TraitMethod {
    name: String,
    signature: Signature,
    default: Option<FnId>,
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
fn constant(value: &mut hir::Expr) -> bool {
    constant_parts(value).is_some_and(|parts| parts.into_iter().all(constant))
}

/// The components of a constant; `None` for a value that is not one.
fn constant_parts(value: &mut hir::Expr) -> Option<Vec<&mut hir::Expr>> {
    use hir::ExprKind as K;
    Some(match &mut value.kind {
        K::Int(_) | K::Float(_) | K::Char(_) | K::Bool(_) | K::Str(_) | K::Unit => Vec::new(),
        K::Unary(_, inner) | K::Retype(inner) | K::Convert(inner) => vec![inner],
        K::Binary(_, left, right) => vec![left, right],
        K::Aggregate(items) | K::Array(items) | K::Set(items) => items.iter_mut().collect(),
        K::Record(fields) | K::Variant(_, fields) => fields.iter_mut().map(|(_, v)| v).collect(),
        K::Map(entries) => entries.iter_mut().flat_map(|(k, v)| [k, v]).collect(),
        _ => return None,
    })
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
    let fits =
        |n: i128| if n < int.min() || n > int.max() { Err("the result does not fit in its type") } else { Ok(n) };
    let result = match &value.kind {
        hir::ExprKind::Int(n) => Ok(*n),
        hir::ExprKind::Convert(inner) | hir::ExprKind::Retype(inner) => fold(types, inner)?,
        hir::ExprKind::Unary(UnaryOp::Neg, inner) => fold(types, inner)?.and_then(|n| fits(-n)),
        hir::ExprKind::Unary(UnaryOp::BitNot, inner) => fold(types, inner)?.map(|n| {
            let bits = int.bits();
            let mask = if bits == 128 { -1 } else { (1i128 << bits) - 1 };
            let flipped = !n & mask;
            if int.signed() && flipped > int.max() { flipped - (1i128 << bits) } else { flipped }
        }),
        hir::ExprKind::Binary(op, left, right) => {
            let (a, b) = (fold(types, left)?, fold(types, right)?);
            let (a, b) = match (a, b) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
            };
            match op {
                BinOp::Add => fits(a + b),
                BinOp::Sub => fits(a - b),
                BinOp::Mul => a.checked_mul(b).map_or(Err("the result does not fit in its type"), fits),
                BinOp::Div | BinOp::Rem if b == 0 => Err("it divides by zero"),
                BinOp::Div => fits(a / b),
                BinOp::Rem => fits(a % b),
                BinOp::Shl | BinOp::Shr if b < 0 || b >= int.bits() as i128 => Err("the shift count is out of range"),
                BinOp::Shl => {
                    let bits = int.bits();
                    let shifted = (a << b) & ((1i128 << bits) - 1);
                    Ok(if int.signed() && shifted > int.max() { shifted - (1i128 << bits) } else { shifted })
                }
                BinOp::Shr => Ok(if int.signed() { a >> b } else { (a as u128 >> b) as i128 }),
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
    /// While the operands of a comparison meet, whose mismatch is one:
    /// whether the value coerced is the left one.
    comparing: Option<bool>,
    /// While a panic's `data` is checked, the type of its values, whose
    /// mismatch says to write a text instead.
    panic_datum: Option<Ty>,
    closure_name: Option<String>,
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
    types: Types,
    diags: &'a mut Diagnostics,
    sources: &'a SourceMap,
    names: HashMap<String, ModuleName>,
    type_names: HashMap<String, TypeName>,
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
    impls: Vec<ImplDef>,
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
    /// `time.Instant`: nanoseconds of the monotonic clock.
    instant: Option<Ty>,
    /// `time.DateTime`: seven `Int` fields, from the year down.
    datetime: Option<Ty>,
    /// `time.TimeError`: a text that is not a date and time.
    time_error: Option<Ty>,
    /// `sync.CompareExchange`, once `std.sync` is imported: the template
    /// and its instance with its own parameter.
    compare_exchange: Option<(TemplateId, Ty)>,
    /// The types of `std.fs`, once imported.
    fs: Option<FsTypes>,
    /// The module whose declaration or code is being checked (7.5).
    module: u32,
    /// Who declares each nominal type, by name.
    owners: HashMap<String, Owner>,
    /// The module of each inherent operation, and whether it is `pub`.
    method_visibility: HashMap<FnId, (u32, bool)>,
    /// The module `std.prelude`, which declares the methods of the intrinsic
    /// types, and their keys by name.
    prelude: Option<u32>,
    /// The contexts the program declares (11.16), by index: each one's
    /// record type and declaration; and the index of each such type.
    contexts: Vec<(Ty, &'a ast::TypeDecl)>,
    context_of: HashMap<Ty, u32>,
    /// The path of each module of the program, which an unknown name may
    /// be the last part of.
    module_paths: Vec<Vec<String>>,
    /// The module `std.datetime`, the part of `std.time` written in Tondo,
    /// which owns the types of `std.time` as their module would.
    datetime_module: Option<u32>,
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

/// A constant: its declaration, and its value once evaluated.
struct ConstEntry<'a> {
    decl: &'a ast::ConstDecl,
    value: Option<hir::Expr>,
    resolving: bool,
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
    scopes: Vec<Vec<(String, LocalId)>>,
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
    fn program(&mut self, module: &'a ast::Module) -> Option<hir::Program> {
        self.prelude = module.module_paths.iter().position(|p| p == &["std", "prelude"]).map(|i| i as u32);
        self.module_paths = module.module_paths.clone();
        self.datetime_module = module.module_paths.iter().position(|p| p == &["std", "datetime"]).map(|i| i as u32);
        for import in &module.imports {
            self.import(import);
        }
        // The network, through a module written in Tondo (27.7); `std.http`
        // brings `std.net`, so it is named first.
        let network = ["http", "net"].into_iter().find(|name| module.module_paths.iter().any(|p| p == &["std", name]));
        if let Some(name) = network.filter(|_| self.meta) {
            let message = format!(
                "`std.{name}` is not available to a provider or generator, which runs while the program compiles and works only on its request"
            );
            self.diags.error_without_span("E2108", message);
        }

        self.declare_types(&module.items);
        // The prelude's `trait Iterator[T]` adds methods to the intrinsic one.
        let (extensions, traits): (Vec<&ast::TraitDecl>, Vec<&ast::TraitDecl>) = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Trait(t) => Some(t),
                _ => None,
            })
            .partition(|t| Some(t.module) == self.prelude && t.name.name.rsplit('.').next() == Some("Iterator"));
        for extension in &extensions {
            // Its module names it by its own path.
            self.trait_names.insert(extension.name.name.clone(), ITERATOR);
        }
        self.declare_traits(&traits);
        for item in &module.items {
            if let ast::Item::Const(decl) = item {
                let key = Some(crate::index::Key::Value(decl.name.name.clone()));
                self.index_define(key, crate::index::DefKind::Const, &decl.name, decl.span, None);
                self.consts.push(ConstEntry { decl, value: None, resolving: false });
                self.declare_module_name(&decl.name, ModuleName::Const(self.consts.len() - 1));
            }
        }
        let (externs, mut fns): (Vec<&ast::FnDecl>, Vec<&ast::FnDecl>) = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Fn(f) => Some(f),
                _ => None,
            })
            .partition(|f| f.foreign.is_some() && f.body.is_none());
        // A fixture's operations come last, after the toolchain's own.
        fns.sort_by_key(|f| self.fixture(f.span));
        for f in externs {
            self.declare_extern(f);
        }
        // Every function with a body, in `FnId` order.
        let mut bodies: Vec<&ast::FnDecl> = Vec::new();
        for f in &fns {
            let id = FnId(bodies.len() as u32);
            // What reads the clock or the system's entropy (27.7).
            let bare = |name: &str| name.rsplit('.').next().unwrap_or_default().to_string();
            let qualified = (
                module.module_paths[f.module as usize].join("."),
                f.owner.as_ref().map(|o| bare(&o.name)),
                bare(&f.name.name),
            );
            let unrepeatable = match qualified {
                (m, Some(o), n) if m == "std.uuid" && o == "Uuid" && n == "v4" => Some("`Uuid.v4`"),
                (m, Some(o), n) if m == "std.uuid" && o == "Uuid" && n == "v7" => Some("`Uuid.v7`"),
                (m, Some(o), n) if m == "std.random" && o == "Random" && n == "new" => Some("`Random.new`"),
                (m, None, n) if m == "std.random" && n == "entropy" => Some("`random.entropy`"),
                _ => None,
            };
            if let Some(name) = unrepeatable {
                self.unrepeatable.insert(id, name);
            }
            let owner = f.owner.as_ref().map(|owner| self.method_owner(owner, f.module));
            let signature = self.signature(f, owner, FnScope::default());
            let receiver = signature.receiver;
            self.signatures.push(signature);
            if f.foreign.is_some() {
                self.declare_export(f, id, &module.module_paths);
            }
            bodies.push(f);
            match owner {
                Some(owner) => self.declare_method(f, owner, receiver.is_some(), id),
                None => self.declare_module_name(&f.name, ModuleName::Fn(id)),
            }
        }
        self.trait_methods(&traits, &mut bodies);
        for extension in extensions {
            self.iterator_defaults(extension, &mut bodies);
        }
        for item in &module.items {
            if let ast::Item::Impl(decl) = item {
                self.implementation(decl, &mut bodies);
            }
        }
        self.index_functions(&bodies);
        for (index, f) in fns.iter().enumerate() {
            self.defaults(f, FnId(index as u32));
        }
        self.check_termination();
        // Every constant is checked, used or not.
        for id in 0..self.consts.len() {
            let span = self.consts[id].decl.span;
            self.const_value(id, span);
        }

        for item in &module.items {
            if let ast::Item::Test(node) = item
                && Self::test_class(node) == ast::SourceClass::Production
            {
                let message = format!(
                    "a {} belongs in a test file: move it to a `_test.to` file next to this code, or to a file under `tests/`",
                    if matches!(node, ast::TestNode::Suite(_)) { "`suite`" } else { "`test`" }
                );
                self.diags.error("E2001", node.span(), message);
            }
        }

        // Closure bodies are numbered after every declared function, the
        // script's `main` and the root of a test build's tree.
        let testing = module.test_package.is_some();
        for statement in &module.statements {
            if let ast::StmtKind::Let { pattern, .. } = &statement.kind {
                bound_names(pattern, &mut self.script_bindings);
            }
        }
        self.closure_base = bodies.len() as u32 + u32::from(!module.statements.is_empty()) + u32::from(testing);
        let mut functions: Vec<hir::Function> =
            bodies.iter().zip(0..).map(|(f, i)| self.function(f, FnId(i))).collect();

        let explicit_main = match self.names.get("main") {
            Some(ModuleName::Fn(id)) => Some(*id),
            _ => None,
        };
        let main = if !module.statements.is_empty() {
            if let Some(id) = explicit_main {
                let span = fns[id.0 as usize].name.span;
                self.diags.error("E1802", span, "a script with top-level statements cannot also declare `main`");
            }
            functions.push(self.script_main(&module.statements));
            Some(FnId(functions.len() as u32 - 1))
        } else if let Some(id) = explicit_main {
            self.validate_main(fns[id.0 as usize], id);
            Some(id)
        } else if testing {
            Some(FnId(0))
        } else if module.library {
            // A library runs what C calls.
            None
        } else {
            let message = "nothing to run: the program has no `main` and no top-level statements; add `fn main() { ... }`, \
                or statements at the top of the file; a library for C builds with `--library`";
            self.diags.error_without_span("E1806", message);
            return None;
        };
        // A test build runs its tests instead of `main`.
        let main = match &module.test_package {
            Some(package) => {
                functions.push(self.tests(module, package));
                Some(FnId(functions.len() as u32 - 1))
            }
            None => main,
        };
        let makers = self.context_makers();
        functions.extend(std::mem::take(&mut self.closure_functions).into_iter().map(|f| f.expect("checked closure")));
        let mut contexts = Vec::new();
        for (maker, log, label) in makers {
            let mut add = |function: hir::Function| {
                functions.push(function);
                FnId(functions.len() as u32 - 1)
            };
            let maker = add(maker);
            let (log, label) = (log.map(&mut add), label.map(&mut add));
            contexts.push(hir::ContextFns { maker, log, label });
        }

        if module.c_library && self.c_functions.is_empty() {
            let message = "the library publishes no function: C calls a `pub extern fn name(...) { ... }` of the package by its name";
            self.diags.error_without_span("E1807", message);
        }
        if self.diags.has_errors() {
            return None;
        }
        let traits = self
            .traits
            .iter()
            .map(|t| hir::TraitInfo {
                name: t.name.clone(),
                defaults: t.methods.iter().map(|m| m.default).collect(),
                param_count: t.params.len(),
            })
            .collect();
        let impls = self
            .impls
            .iter()
            .map(|i| hir::ImplInfo {
                intrinsic: i.intrinsic,
                trait_id: i.trait_id,
                trait_args: i.trait_args.clone(),
                target: i.target,
                generics: i.generics.clone(),
                methods: i.methods.clone(),
            })
            .collect();
        // Before the program takes the types.
        self.index_finish();
        let c_api = hir::CApi { structs: self.c_structs(), functions: std::mem::take(&mut self.c_functions) };
        Some(hir::Program {
            types: std::mem::take(&mut self.types),
            functions,
            main,
            traits,
            impls,
            display: HashMap::new(),
            externs: std::mem::take(&mut self.externs),
            exports: std::mem::take(&mut self.exports),
            c_api,
            tests: std::mem::take(&mut self.test_nodes),
            contexts,
        })
    }

    fn test_class(node: &ast::TestNode) -> ast::SourceClass {
        match node {
            ast::TestNode::Test(t) => t.class,
            ast::TestNode::Suite(s) => s.class,
        }
    }

    /// The root of a test build's tree (testing 3.2): a function that runs
    /// the top-level suites and tests of the test files.
    fn tests(&mut self, module: &ast::Module, package: &str) -> hir::Function {
        let root = hir::TestNodeInfo {
            id: String::new(),
            parent: 0,
            suite: true,
            bench: false,
            fuzz: false,
            span: Span::new(0, 0),
        };
        self.test_nodes.push(root);
        let mut nodes = Vec::new();
        let mut prefixes = Vec::new();
        for item in &module.items {
            let ast::Item::Test(node) = item else { continue };
            let (module_index, class) = match node {
                ast::TestNode::Test(t) => (t.module, t.class),
                ast::TestNode::Suite(s) => (s.module, s.class),
            };
            // `package::unit::module.path` or `package::integration::file.path`.
            let path = &module.module_paths[module_index as usize];
            let (kind, logical) = match class {
                ast::SourceClass::Production => continue,
                ast::SourceClass::Unit if path.len() == 1 => ("unit", path[0].clone()),
                ast::SourceClass::Unit => ("unit", path[1..].join(".")),
                ast::SourceClass::Integration => ("integration", path[1..].join(".")),
            };
            nodes.push(node);
            prefixes.push(format!("{package}::{kind}::{logical}"));
        }
        let mut cx = self.new_cx(Ty::UNIT);
        let run = self.test_members(&mut cx, &nodes, 0, &prefixes, Span::new(0, 0));
        let body = hir::Block { stmts: vec![hir::Stmt::Expr(run)], tail: None, ty: Ty::UNIT };
        let mut function = hir::Function {
            name: "tests".into(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: Ty::UNIT,
            body,
            moves: HashSet::new(),
            test: false,
            span: Span::default(),
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// `testing.withVirtualTime((clock) { ... })` (testing 7.9): the closure
    /// runs on a new virtual clock, which ends as it returns; it infers its
    /// errors, which the call passes on.
    fn with_virtual_time(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let [arg] = args else {
            return self.arity_error(span, 1, args.len());
        };
        let ast::ExprKind::Closure(closure) = &arg.value.kind else {
            let message = "`withVirtualTime` takes a closure: `testing.withVirtualTime((clock) { ... })`";
            self.diags.error("E1102", arg.value.span, message);
            return Self::error_expr(span);
        };
        let clock = self.types.handle(Handle::Clock, Ty::UNIT);
        let expected = self.types.intern(TyKind::Fn(vec![clock], Ty::UNIT, false, vec![ast::Mode::Value], false));
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let body = &closure.body;
        let value =
            self.closure_with(cx, parts, Some(expected), kind, span, |this, cx, success| this.block(cx, body, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        // `let clock = begin(); defer end(); body(clock)`
        let local = |this: &mut Self, cx: &mut FnCx, ty: Ty| {
            let local = this.hidden_local(cx, ty);
            cx.mutable[local.0 as usize] = true;
            local
        };
        let clock_local = local(self, cx, clock);
        let begin = Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::ClockBegin), Vec::new(), Vec::new()), clock, span);
        let end = Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::ClockEnd), Vec::new(), Vec::new()), Ty::UNIT, span);
        let Some(cleanup) = self.deferred_call(end, span) else { return Self::error_expr(span) };
        let cleanup_local = local(self, cx, cleanup.ty);
        let read = Self::mk(hir::ExprKind::Local(cleanup_local), cleanup.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), Ty::UNIT, span);
        let body_ty = value.ty;
        let body_local = local(self, cx, body_ty);
        let stmts = vec![
            hir::Stmt::Let(clock_local, begin),
            hir::Stmt::Defer { local: cleanup_local, closure: cleanup, call },
            hir::Stmt::Let(body_local, value),
        ];
        let callee = Self::mk(hir::ExprKind::Local(body_local), body_ty, span);
        let argument = Self::mk(hir::ExprKind::Local(clock_local), clock, span);
        let run = Self::mk(hir::ExprKind::CallValue(Box::new(callee), vec![argument]), info.result, span);
        // Always a result, so that `?` follows the call as the canonical
        // form writes it; one that cannot fail fails with `Never`.
        let (run, ty) = match self.types.kind(info.result) {
            TyKind::Result(..) => (run, info.result),
            _ => {
                let ty = self.types.intern(TyKind::Result(info.result, Ty::NEVER));
                (Self::mk(hir::ExprKind::Variant(OK, vec![(0, run)]), ty, span), ty)
            }
        };
        let block = hir::Block { stmts, tail: Some(Box::new(run)), ty };
        Self::mk(hir::ExprKind::Block(block), ty, span)
    }

    /// `testing.fuzz(name, seeds, (input: T) { ... })` (testing 10.6), in a
    /// test's own body, which makes the test one that fuzzes: a call of the
    /// loop the standard library writes for it, `fuzz.run`, with the input's
    /// type, the body's error, `Never` if it does not fail, and the body's.
    fn fuzz(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        const EXAMPLE: &str = "testing.fuzz(\"parse\", [\"[1]\"], (text: String) { _ = json.parse(text) })";
        match self.measuring {
            Some(test) if cx.test_member => self.test_nodes[test as usize].fuzz = true,
            _ => {
                let message = format!(
                    "`testing.fuzz` tries inputs in the body of a `test`, which it makes a fuzz test: call it there, \
                     as `test parsesAnything {{ {EXAMPLE} }}`, not in a closure, a function or a suite's setup"
                );
                self.diags.error("E2007", span, message);
            }
        }
        let [name, seeds, body] = args else {
            return self.arity_error(span, 3, args.len());
        };
        let ast::ExprKind::Closure(closure) = &body.value.kind else {
            let message = format!("`fuzz` takes a name, the seeds and a closure that tries an input: `{EXAMPLE}`");
            self.diags.error("E1102", body.value.span, message);
            return Self::error_expr(span);
        };
        let name = self.expr(cx, &name.value, Some(Ty::STRING));
        let name = self.coerce(name, Ty::STRING);
        // The input's type: the closure's parameter's, or the seeds'.
        let written = closure.params.first().and_then(|p| p.ty.as_ref()).map(|t| self.resolve_type(t));
        let seeds_ty = written.map(|t| self.types.intern(TyKind::Array(t)));
        let seeds = self.expr(cx, &seeds.value, seeds_ty);
        let seeds = match seeds_ty {
            Some(ty) => self.coerce(seeds, ty),
            None => seeds,
        };
        let input = match *self.types.kind(seeds.ty) {
            TyKind::Array(input) if !self.types.has_params(input) => input,
            TyKind::Error => return Self::error_expr(span),
            _ => {
                let message = "the seeds of `testing.fuzz` are an array of inputs; with none, write the input's \
                               type on the closure's parameter: `testing.fuzz(\"parse\", [], (text: String) { ... })`";
                self.diags.error("E1101", seeds.span, message);
                return Self::error_expr(span);
            }
        };
        // Inputs are written and read as JSON.
        for path in ["serialization.Encode", "serialization.Decode"] {
            let Some(&id) = self.trait_names.get(path) else { continue };
            if input != Ty::ERROR && !self.satisfies(input, id, &[]) {
                let message = format!(
                    "{} does not implement `{path}`, which the input of `testing.fuzz` needs, as it is written and \
                     read as JSON; the compiler implements it for records, enums and newtypes whose parts all \
                     implement it",
                    self.display(input)
                );
                self.diags.error("E1105", seeds.span, message);
                return Self::error_expr(span);
            }
        }
        let expected = self.types.intern(TyKind::Fn(vec![input], Ty::UNIT, false, Vec::new(), false));
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let block = &closure.body;
        let value = self
            .closure_with(cx, parts, Some(expected), kind, span, |this, cx, success| this.block(cx, block, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        let error = match *self.types.kind(info.result) {
            TyKind::Result(_, error) => error,
            _ => Ty::NEVER,
        };
        let Some(&ModuleName::Fn(run)) = self.names.get("fuzz.run") else {
            unreachable!("std.testing brings the loop of `testing.fuzz`")
        };
        let ty = self.types.intern(TyKind::Result(Ty::UNIT, error));
        let mut defs = Vec::new();
        let root = self.fuzz_shape(input, &mut defs, &mut HashMap::new());
        let shape = Self::mk(
            hir::ExprKind::Str(format!("{{\"root\":{root},\"defs\":[{}]}}", defs.join(","))),
            Ty::STRING,
            span,
        );
        let types = vec![input, error, value.ty];
        Self::mk(hir::ExprKind::Call(Callee::Fn(run), types, vec![name, shape, seeds, value]), ty, span)
    }

    /// The shape of a record, or of a variant's fields, `key` naming their
    /// keys in `wires`: each written field's key and shape.
    fn fuzz_record(
        &mut self,
        key: &str,
        fields: &[Field],
        defs: &mut Vec<String>,
        named: &mut HashMap<Ty, usize>,
    ) -> String {
        let wires = self.wires.get(key).cloned().unwrap_or_default();
        let mut members = Vec::new();
        for (at, field) in fields.iter().enumerate() {
            let wire = match wires.get(at) {
                Some(None) => continue,
                Some(Some(wire)) => wire.clone(),
                None => field.name.clone(),
            };
            let shape = self.fuzz_shape(field.ty, defs, named);
            members.push(format!("[{},{shape}]", serde_json::Value::String(wire)));
        }
        format!("{{\"record\":[{}]}}", members.join(","))
    }

    /// What `json.encode` writes for a value of `ty`, as the fuzzer reads it
    /// (testing 10.6), in JSON: `"int"`, `"byte"`, `"float"`, `"bool"`,
    /// `"text"`, `"bytes"` and `"datetime"`; `{"option": s}`, `{"list": s}` and
    /// `{"map": s}`; and for a record, enum or newtype, `{"ref": n}`, its
    /// index in `defs`, whose entry is `{"record": [[key, s]...]}`,
    /// `{"enum": [[name, payload]...]}` or its value's shape, so that one
    /// that holds itself ends. What the fuzzer cannot tell is `"any"`.
    fn fuzz_shape(&mut self, ty: Ty, defs: &mut Vec<String>, named: &mut HashMap<Ty, usize>) -> String {
        let quoted = |text: &str| serde_json::Value::String(text.to_string()).to_string();
        match self.types.kind(ty).clone() {
            _ if ty == Ty::INT => "\"int\"".into(),
            _ if ty == Ty::BYTE => "\"byte\"".into(),
            _ if ty == Ty::F64 => "\"float\"".into(),
            _ if ty == Ty::BOOL => "\"bool\"".into(),
            _ if ty == Ty::STRING => "\"text\"".into(),
            _ if Some(ty) == self.datetime => "\"datetime\"".into(),
            TyKind::Option(inner) => format!("{{\"option\":{}}}", self.fuzz_shape(inner, defs, named)),
            TyKind::Array(inner) => format!("{{\"list\":{}}}", self.fuzz_shape(inner, defs, named)),
            TyKind::Map(key, value) if key == Ty::STRING => {
                format!("{{\"map\":{}}}", self.fuzz_shape(value, defs, named))
            }
            TyKind::Adt(id) => {
                if let Some(&index) = named.get(&ty) {
                    return format!("{{\"ref\":{index}}}");
                }
                let (kind, instance, own) = {
                    let adt = self.types.adt(id);
                    (adt.kind.clone(), adt.instance.clone(), adt.name.clone())
                };
                let name = match &instance {
                    Some((template, _)) => self.types.template(*template).name.clone(),
                    None => own,
                };
                if name == "serialization.Bytes" {
                    return "\"bytes\"".into();
                }
                let index = defs.len();
                named.insert(ty, index);
                defs.push(String::new());
                let def = match &kind {
                    AdtKind::Newtype(inner) => self.fuzz_shape(*inner, defs, named),
                    AdtKind::Record(fields) => self.fuzz_record(&name, fields, defs, named),
                    AdtKind::Enum(variants) => {
                        let mut shown = Vec::new();
                        for variant in variants {
                            let payload = match &variant.payload {
                                Payload::Unit => "null".to_string(),
                                Payload::Tuple(items) if items.len() == 1 => self.fuzz_shape(items[0], defs, named),
                                Payload::Tuple(items) => {
                                    let items: Vec<String> =
                                        items.iter().map(|&t| self.fuzz_shape(t, defs, named)).collect();
                                    format!("{{\"tuple\":[{}]}}", items.join(","))
                                }
                                Payload::Record(fields) => {
                                    self.fuzz_record(&format!("{name}.{}", variant.name), fields, defs, named)
                                }
                            };
                            shown.push(format!("[{},{payload}]", quoted(&variant.name)));
                        }
                        format!("{{\"enum\":[{}]}}", shown.join(","))
                    }
                };
                defs[index] = def;
                format!("{{\"ref\":{index}}}")
            }
            _ => "\"any\"".into(),
        }
    }

    /// `panic(message, code:, data:)` (15.7): the message, and the
    /// program's own code and data, which the standard library hands the
    /// runtime just before the panic.
    fn panic_call(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let mut given: [Option<&ast::Expr>; 3] = [None; 3];
        for (index, arg) in args.iter().enumerate() {
            let slot = match (arg.name.as_ref().map(|n| n.name.as_str()), index) {
                (Some("message"), _) | (None, 0) => 0,
                (Some("code"), _) | (None, 1) => 1,
                (Some("data"), _) | (None, 2) => 2,
                (Some(other), _) => {
                    let message = format!("`panic` takes `message`, `code` and `data`, not `{other}`");
                    self.diags.error("E1102", arg.value.span, message);
                    continue;
                }
                (None, _) => return self.arity_error(span, 3, args.len()),
            };
            given[slot] = Some(&arg.value);
        }
        let [Some(message), code, data] = given else {
            return self.arity_error(span, 1, 0);
        };
        let message = self.expr(cx, message, Some(Ty::STRING));
        let message = self.coerce(message, Ty::STRING);
        let panic = Self::mk(hir::ExprKind::Panic(Box::new(message), 8), Ty::NEVER, span);
        if code.is_none() && data.is_none() {
            return panic;
        }
        let mut stmts = Vec::new();
        if let Some(code) = code {
            if let ast::ExprKind::Str(segments) = &code.kind
                && let [ast::StrSegment::Text(text)] = segments.as_slice()
                && !own_panic_code(text)
            {
                let message = format!(
                    "`{text}` is not a code of the program's own: write segments of a lowercase letter followed by letters and digits, separated by `.`, as `cache.corrupt`; codes `P` with digits are the language's"
                );
                self.diags.error("E1305", code.span, message);
            }
            let code = self.expr(cx, code, Some(Ty::STRING));
            let code = self.coerce(code, Ty::STRING);
            stmts.push(hir::Stmt::Expr(self.panics_call("panics.stageCode", code, span)));
        }
        if let Some(data) = data {
            let datum = self.types.union(&[Ty::INT, Ty::F64, Ty::BOOL, Ty::STRING]);
            let map = self.map_type(Ty::STRING, datum);
            let outer = self.panic_datum.replace(datum);
            let data = self.expr(cx, data, Some(map));
            let data = self.coerce(data, map);
            self.panic_datum = outer;
            stmts.push(hir::Stmt::Expr(self.panics_call("panics.stageData", data, span)));
        }
        let block = hir::Block { stmts, tail: Some(Box::new(panic)), ty: Ty::NEVER };
        Self::mk(hir::ExprKind::Block(block), Ty::NEVER, span)
    }

    /// `recover { ... }` (15.9): the block's value, or the panic that
    /// stopped it, as `T ! Panic`.
    fn recover(&mut self, cx: &mut FnCx, block: &ast::Block, expected: Option<Ty>, span: Span) -> hir::Expr {
        let Some(panic) = self.type_named("Panic") else { unreachable!("the prelude names `Panic`") };
        let inner = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Result(ok, error)) if *error == panic => Some(*ok),
            _ => None,
        };
        let block = self.block(cx, block, inner);
        let ty = self.types.intern(TyKind::Result(block.ty, panic));
        let Some(&ModuleName::Fn(caught)) = self.names.get("panics.caught") else {
            unreachable!("the prelude brings `std.panics`")
        };
        let caught = Self::mk(hir::ExprKind::Call(Callee::Fn(caught), Vec::new(), Vec::new()), panic, span);
        Self::mk(hir::ExprKind::Recover(block, Box::new(caught)), ty, span)
    }

    /// `within limit { ... }` (11.17): the block's value, as a result whose
    /// error also takes `time.TimedOut`, which it is when the deadline
    /// passes first. The limit is evaluated first, into a binding that the
    /// error reads.
    fn within(&mut self, cx: &mut FnCx, limit: &ast::Expr, block: &ast::Block, span: Span) -> hir::Expr {
        let (Some(duration), Some(timed_out)) = (self.duration, self.timed_out()) else {
            let message =
                "`within` takes a `time.Duration`: `import std.time`, and write `within time.seconds(2) { ... }`";
            self.diags.error("E1001", limit.span, message);
            self.block(cx, block, None);
            return Self::error_expr(span);
        };
        let limit = self.expr(cx, limit, Some(duration));
        let limit = self.coerce(limit, duration);
        let held = self.hidden_local(cx, duration);
        let block = self.block(cx, block, None);
        let (ty, error) = match *self.types.kind(block.ty) {
            TyKind::Result(ok, error) => {
                let error = self.types.union(&[error, timed_out]);
                (self.types.intern(TyKind::Result(ok, error)), error)
            }
            _ => (self.types.intern(TyKind::Result(block.ty, timed_out)), timed_out),
        };
        let read = Self::mk(hir::ExprKind::Local(held), duration, span);
        let failure = Self::mk(hir::ExprKind::Record(vec![(0, read.clone())]), timed_out, span);
        let failure = self.coerce(failure, error);
        let within = Self::mk(hir::ExprKind::Within(Box::new(read), block, Box::new(failure)), ty, span);
        let stmts = vec![hir::Stmt::Let(held, limit)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(within)), ty }), ty, span)
    }

    /// `time.TimedOut`, which `std.time` writes in Tondo (11.17).
    fn timed_out(&self) -> Option<Ty> {
        match self.type_names.get("datetime.TimedOut") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A call of a function of the standard library's `std.panics`.
    fn panics_call(&mut self, name: &str, value: hir::Expr, span: Span) -> hir::Expr {
        let Some(&ModuleName::Fn(id)) = self.names.get(name) else { unreachable!("the prelude brings `std.panics`") };
        Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![value]), Ty::UNIT, span)
    }

    /// `testing.bench(name, () { ... })` (testing 10.4), in a test's own
    /// body, which makes the test a benchmark: runs the body as many times
    /// as the runtime asks, keeping what it gives, until it asks for none
    /// or the body fails; the call fails as the body does.
    fn bench(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let [name, body] = args else {
            return self.arity_error(span, 2, args.len());
        };
        let ast::ExprKind::Closure(closure) = &body.value.kind else {
            let message = "`bench` takes a name and a closure: `testing.bench(\"parse\", () { json.parse(text) })?`";
            self.diags.error("E1102", body.value.span, message);
            return Self::error_expr(span);
        };
        match self.measuring {
            Some(test) if cx.test_member => self.test_nodes[test as usize].bench = true,
            _ => {
                let message = "`testing.bench` measures in the body of a `test`, which it makes a benchmark: call it \
                               there, as `test parsing { testing.bench(\"parse\", () { json.parse(text) })? }`, not \
                               in a closure, a function or a suite's setup";
                self.diags.error("E2006", span, message);
            }
        }
        let name = self.expr(cx, &name.value, Some(Ty::STRING));
        let name = self.coerce(name, Ty::STRING);
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let block = &closure.body;
        let value = self.closure_with(cx, parts, None, kind, span, |this, cx, success| this.block(cx, block, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        let (success, error) = match *self.types.kind(info.result) {
            TyKind::Result(success, error) => (success, Some(error)),
            _ => (info.result, None),
        };
        let mk = |kind, ty| Self::mk(kind, ty, span);
        let std = |function, values, ty| mk(hir::ExprKind::Call(Callee::Std(function), Vec::new(), values), ty);
        let local = |this: &mut Self, cx: &mut FnCx, ty: Ty| {
            let local = this.hidden_local(cx, ty);
            cx.mutable[local.0 as usize] = true;
            local
        };
        let read = |local: LocalId, ty: Ty| mk(hir::ExprKind::Local(local), ty);
        let set = |local: LocalId, op: Option<BinOp>, value: hir::Expr| hir::Stmt::Assign {
            local,
            path: Vec::new(),
            op,
            value,
            span,
        };
        let int = |n: i128| mk(hir::ExprKind::Int(n), Ty::INT);
        let ty = self.types.intern(TyKind::Result(Ty::UNIT, error.unwrap_or(Ty::NEVER)));
        let (body, left, completed, outcome) =
            (local(self, cx, value.ty), local(self, cx, Ty::INT), local(self, cx, Ty::BOOL), local(self, cx, ty));
        let unit = || mk(hir::ExprKind::Unit, Ty::UNIT);
        let ok = mk(hir::ExprKind::Variant(OK, vec![(0, unit())]), ty);
        // What a run gives is kept, so that its work is not optimized away.
        let keep = |value: hir::Expr| match success {
            Ty::UNIT | Ty::NEVER => hir::Stmt::Expr(value),
            _ => {
                hir::Stmt::Expr(mk(hir::ExprKind::Call(Callee::Std(StdFn::Keep), vec![success], vec![value]), Ty::UNIT))
            }
        };
        let run = mk(hir::ExprKind::CallValue(Box::new(read(body, value.ty)), Vec::new()), info.result);
        let run = match error {
            None => keep(run),
            Some(error) => {
                // `match body() { ok(v) => keep(v), err(e) => { outcome = err(e); completed = false; break } }`
                let (kept, failure) = (local(self, cx, success), local(self, cx, error));
                let pattern = |variant, local, ty| hir::Pattern {
                    kind: hir::PatKind::Variant(
                        variant,
                        vec![hir::Pattern { kind: hir::PatKind::Binding(local), ty, span }],
                    ),
                    ty: info.result,
                    span,
                };
                let kept_body = hir::Block { stmts: vec![keep(read(kept, success))], tail: None, ty: Ty::UNIT };
                let failed = mk(hir::ExprKind::Variant(ERR, vec![(0, read(failure, error))]), ty);
                let stop = hir::Block {
                    stmts: vec![
                        set(outcome, None, failed),
                        set(completed, None, mk(hir::ExprKind::Bool(false), Ty::BOOL)),
                        hir::Stmt::Break,
                    ],
                    tail: None,
                    ty: Ty::NEVER,
                };
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(OK, kept, success),
                        guard: None,
                        body: mk(hir::ExprKind::Block(kept_body), Ty::UNIT),
                    },
                    hir::Arm {
                        pattern: pattern(ERR, failure, error),
                        guard: None,
                        body: mk(hir::ExprKind::Block(stop), Ty::NEVER),
                    },
                ];
                hir::Stmt::Expr(mk(hir::ExprKind::Match(Box::new(run), arms), Ty::UNIT))
            }
        };
        // `if left == 0 { left = next(); if left == 0 { break } }`
        let none_left =
            || mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(read(left, Ty::INT)), Box::new(int(0))), Ty::BOOL);
        let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
        let next = hir::Block {
            stmts: vec![
                set(left, None, std(StdFn::BenchNext, Vec::new(), Ty::INT)),
                hir::Stmt::Expr(mk(hir::ExprKind::If(Box::new(none_left()), stop, None), Ty::UNIT)),
            ],
            tail: None,
            ty: Ty::UNIT,
        };
        let refill = hir::Stmt::Expr(mk(hir::ExprKind::If(Box::new(none_left()), next, None), Ty::UNIT));
        let turn =
            hir::Block { stmts: vec![refill, set(left, Some(BinOp::Sub), int(1)), run], tail: None, ty: Ty::UNIT };
        let stmts = vec![
            hir::Stmt::Expr(std(StdFn::BenchStart, vec![name], Ty::UNIT)),
            hir::Stmt::Let(body, value),
            hir::Stmt::Let(left, int(0)),
            hir::Stmt::Let(completed, mk(hir::ExprKind::Bool(true), Ty::BOOL)),
            hir::Stmt::Let(outcome, ok),
            hir::Stmt::Loop { condition: None, body: turn },
            hir::Stmt::Expr(std(StdFn::BenchEnd, vec![read(completed, Ty::BOOL)], Ty::UNIT)),
        ];
        let block = hir::Block { stmts, tail: Some(Box::new(read(outcome, ty))), ty };
        Self::mk(hir::ExprKind::Block(block), ty, span)
    }

    /// `testing.snapshot(name, value, ignore:, matches:, replace:)` (testing
    /// 9.7): a `String` is compared as it is, any other value as the JSON
    /// `json.encode` writes for it.
    fn snapshot(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let Some(&ModuleName::Fn(encode)) = self.names.get("json.encode") else {
            unreachable!("std.json comes with std.testing")
        };
        let value = self.signatures[encode.0 as usize].generics[0];
        let strings = self.types.intern(TyKind::Array(Ty::STRING));
        let map = self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING));
        let mut signature = Signature::simple(vec![Ty::STRING, value, strings, map, map], Ty::UNIT);
        signature.names = ["name", "value", "ignore", "matches", "replace"].map(String::from).to_vec();
        signature.generics = vec![value];
        signature.name = "testing.snapshot".into();
        signature.defaults = vec![
            None,
            None,
            Some(Self::mk(hir::ExprKind::Array(Vec::new()), strings, span)),
            Some(Self::mk(hir::ExprKind::Map(Vec::new()), map, span)),
            Some(Self::mk(hir::ExprKind::Map(Vec::new()), map, span)),
        ];
        let target = CallTarget::new(Callee::Std(StdFn::TestSnapshot), signature, None);
        let mut call = self.call_signature(cx, target, None, args, span);
        // The call, or the block that evaluates its named arguments first.
        fn inner(expr: &mut hir::Expr) -> Option<(Ty, &mut Vec<hir::Expr>)> {
            match &mut expr.kind {
                hir::ExprKind::Call(_, types, values) => Some((types[0], values)),
                hir::ExprKind::Block(block) => block.tail.as_deref_mut().and_then(inner),
                _ => None,
            }
        }
        let Some((ty, values)) = inner(&mut call) else { return call };
        let json = ty != Ty::STRING;
        if json {
            let raw = values.remove(1);
            let no = || Self::mk(hir::ExprKind::Bool(false), Ty::BOOL, span);
            let text = hir::ExprKind::Call(Callee::Fn(encode), vec![ty], vec![raw, no(), no()]);
            values.insert(1, Self::mk(text, Ty::STRING, span));
        } else if let Some(arg) =
            args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == "ignore" || n.name == "matches"))
        {
            let message = "`ignore` and `matches` look into the JSON of a value; a `String` is compared as it is, so change its varying parts with `replace: [pattern: text]`";
            self.diags.error("E1102", arg.span, message);
        }
        values.insert(2, Self::mk(hir::ExprKind::Bool(json), Ty::BOOL, span));
        call
    }

    /// A path or method of `std.reflect` written as a literal, on a value of
    /// a type known here: one it cannot reach is `E2110` (27.10).
    fn check_reflection(&mut self, name: &str, type_args: &[Ty], args: &[ast::Arg]) {
        let target = if name == "reflect.set" { type_args[0] } else { type_args[1] };
        let Some(arg) = args.get(1).filter(|a| a.name.is_none()) else { return };
        let ast::ExprKind::Str(segments) = &arg.value.kind else { return };
        let [ast::StrSegment::Text(text)] = segments.as_slice() else { return };
        if self.types.has_params(target) || self.types.kind(target) == &TyKind::Error {
            return;
        }
        let result = if name.starts_with("reflect.call") {
            self.reflect_method(target, text, name == "reflect.callMut")
        } else {
            self.types.reflect_path(target, text)
        };
        if let Err(why) = result {
            self.diags.error("E2110", arg.value.span, format!("`{name}` cannot reach `{text}`: {why}"));
        }
    }

    /// Whether reflection can call a method of the type by this name.
    fn reflect_method(&mut self, ty: Ty, method: &str, mutating: bool) -> Result<(), String> {
        let key = self.owner_key(ty);
        let shown = self.types.display(ty);
        let public = |id: &FnId| self.method_visibility.get(id).is_some_and(|&(_, public)| public);
        let mut names: Vec<&str> = self
            .methods
            .iter()
            .filter(|((owner, _), id)| *owner == key && public(id) && self.signatures[id.0 as usize].receiver.is_some())
            .map(|((_, name), _)| name.as_str())
            .collect();
        names.sort();
        match self.methods.get(&(key, method.to_string())) {
            Some(id) if public(id) => match self.signatures[id.0 as usize].receiver {
                Some(ast::Mode::Mut) if !mutating => {
                    Err(format!("`{shown}.{method}` takes `mut self`: call it with `callMut`"))
                }
                Some(_) => Ok(()),
                None => Err(format!("`{shown}.{method}` has no receiver; call it as `{shown}.{method}(...)`")),
            },
            _ if names.is_empty() => Err(format!("`{shown}` has no `pub` methods, so it has no method `{method}`")),
            _ => {
                let listed: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                let listed = match listed.split_last() {
                    Some((last, [])) => format!("its method is {last}"),
                    Some((last, rest)) => format!("its methods are {} and {last}", rest.join(", ")),
                    None => String::new(),
                };
                Err(format!("`{shown}` has no `pub` method `{method}`; {listed}"))
            }
        }
    }

    /// A constant of an intrinsic numeric type: `max` and `min` of the
    /// integers, and `pi`, `e`, `infinity`, `nan` and `epsilon` of the floats.
    fn intrinsic_constant(&self, ty: Ty, name: &str) -> Option<hir::ExprKind> {
        match (self.types.kind(ty), name) {
            (TyKind::Int(int), "max") => Some(hir::ExprKind::Int(int.max())),
            (TyKind::Int(int), "min") => Some(hir::ExprKind::Int(int.min())),
            (TyKind::Float(_), "pi") => Some(hir::ExprKind::Float(std::f64::consts::PI)),
            (TyKind::Float(_), "e") => Some(hir::ExprKind::Float(std::f64::consts::E)),
            (TyKind::Float(_), "infinity") => Some(hir::ExprKind::Float(f64::INFINITY)),
            (TyKind::Float(_), "nan") => Some(hir::ExprKind::Float(f64::NAN)),
            (TyKind::Float(crate::types::FloatTy::F64), "epsilon") => Some(hir::ExprKind::Float(f64::EPSILON)),
            (TyKind::Float(_), "epsilon") => Some(hir::ExprKind::Float(f32::EPSILON as f64)),
            _ => None,
        }
    }

    /// In the `meta` profile, an operation of the host is `E2108`: a
    /// provider or generator runs while compiling and sees only its request
    /// (27.7). Whether it was.
    fn meta_denied(&mut self, span: Span, what: &str) -> bool {
        if self.meta {
            let message = format!(
                "{what} is not available to a provider or generator, which runs while the program compiles and works only on its request"
            );
            self.diags.error("E2108", span, message);
        }
        self.meta
    }

    /// A provider or generator gives the same output for the same request,
    /// so it cannot call what reads the clock or the system's entropy.
    fn repeatable(&mut self, id: FnId, span: Span) {
        if let Some(name) = self.unrepeatable.get(&id) {
            let what = format!("{name}, which gives another value each time,");
            self.meta_denied(span, &what);
        }
    }

    /// `reflect.Value`, once `std.reflect` is imported.
    fn reflect_value_ty(&self) -> Option<Ty> {
        match self.type_names.get("reflect.Value") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// `reflect.valueOf(value)`, which an argument for a `reflect.Value`
    /// parameter becomes.
    fn reflect_value(&mut self, value: hir::Expr) -> hir::Expr {
        let (Some(&ModuleName::Fn(id)), Some(&reflect), Some(target)) =
            (self.names.get("reflect.valueOf"), self.trait_names.get("reflect.Reflect"), self.reflect_value_ty())
        else {
            return value;
        };
        let (ty, span) = (value.ty, value.span);
        if ty != Ty::ERROR && !self.satisfies(ty, reflect, &[]) {
            let message = format!(
                "{} does not implement `reflect.Reflect`, so it cannot become a `reflect.Value`; the compiler implements it for records, enums and newtypes whose parts all implement it",
                self.display(ty)
            );
            self.diags.error("E1105", span, message);
            return Self::error_expr(span);
        }
        Self::mk(hir::ExprKind::Call(Callee::Fn(id), vec![ty], vec![value]), target, span)
    }

    /// `clock.settle()` and `clock.advance(duration)` (testing 7.9).
    fn clock_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let (function, params) = match method.name.as_str() {
            "settle" => (StdFn::ClockSettle, Vec::new()),
            "advance" => {
                let Some(duration) = self.duration else {
                    let message = "`advance` takes a `time.Duration`: import std.time";
                    self.diags.error("E1001", method.span, message);
                    return Self::error_expr(span);
                };
                (StdFn::ClockAdvance, vec![duration])
            }
            _ => return self.no_method(receiver.ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, &param) in args.iter().zip(&params) {
            let value = self.expr(cx, &arg.value, Some(param));
            values.push(self.coerce(value, param));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(function), Vec::new(), values), Ty::UNIT, span)
    }

    /// The members of a suite, or of the root, each as a closure that runs
    /// it; `prefixes` gives the identity each member's name extends.
    fn test_members(
        &mut self,
        cx: &mut FnCx,
        nodes: &[&ast::TestNode],
        parent: u32,
        prefixes: &[String],
        span: Span,
    ) -> hir::Expr {
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        for (&node, prefix) in nodes.iter().zip(prefixes) {
            let name = node.name();
            let id = format!("{prefix}::{}", name.name);
            if name.name == "_" {
                self.diags.error("E1115", name.span, "a test or a suite needs a name; `_` is not one");
                continue;
            }
            if !seen.insert(id.clone()) {
                let message =
                    format!("`{}` is declared twice at this level of the test tree; give one another name", name.name);
                self.diags.error("E2002", name.span, message);
                continue;
            }
            let index = self.test_nodes.len() as u32;
            let suite = matches!(node, ast::TestNode::Suite(_));
            let info = hir::TestNodeInfo { id: id.clone(), parent, suite, bench: false, fuzz: false, span: name.span };
            self.test_nodes.push(info);
            let unit = self.types.intern(TyKind::Fn(Vec::new(), Ty::UNIT, false, Vec::new(), false));
            let kind = ClosureKind { deferred: false, is_unsafe: false, test: true, infer_error: false };
            self.closure_name = Some(format!("{} {}", if suite { "suite" } else { "test" }, name.name));
            // A test's own body may measure (`testing.bench`).
            let outer = std::mem::replace(&mut self.measuring, (!suite).then_some(index));
            let closure =
                self.closure_with(cx, (&[], None), Some(unit), kind, node.span(), |this, inner, _| match node {
                    ast::TestNode::Test(t) => this.block(inner, &t.body, Some(Ty::UNIT)),
                    ast::TestNode::Suite(s) => this.suite_body(inner, s, index, &id),
                });
            self.measuring = outer;
            let Some((body, _, info)) = self.types.closure(closure.ty) else { continue };
            let erased = self.types.intern(TyKind::Fn(Vec::new(), info.result, false, Vec::new(), false));
            members.push((index, Self::mk(hir::ExprKind::Erase(Box::new(closure), FnId(body)), erased, name.span)));
        }
        Self::mk(hir::ExprKind::TestMembers(members), Ty::UNIT, span)
    }

    /// The body of a suite's closure: its setup, then its members, then,
    /// as it ends, its `defer`s.
    fn suite_body(&mut self, cx: &mut FnCx, suite: &ast::SuiteDecl, index: u32, id: &str) -> hir::Block {
        cx.setup = true;
        let mut stmts = self.statements(cx, &suite.setup);
        cx.setup = false;
        if suite.members.is_empty() {
            let message = format!("the suite `{}` has no tests: add a `test` inside it, or remove it", suite.name.name);
            self.diags.error("E2004", suite.name.span, message);
        }
        let members: Vec<&ast::TestNode> = suite.members.iter().collect();
        let prefixes = vec![id.to_string(); members.len()];
        stmts.push(hir::Stmt::Expr(self.test_members(cx, &members, index, &prefixes, suite.span)));
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        hir::Block { stmts, tail: None, ty }
    }

    /// The value of a constant, evaluated on first use (6.5): integer
    /// arithmetic is done now, so what would panic is an error (`E1903`).
    fn const_value(&mut self, id: usize, span: Span) -> hir::Expr {
        if let Some(value) = &self.consts[id].value {
            let mut value = value.clone();
            value.span = span;
            if let hir::ExprKind::Int(_) = value.kind {
                self.const_uses.insert(span);
            }
            return value;
        }
        let decl = self.consts[id].decl;
        if self.consts[id].resolving {
            let message = format!("the constant `{}` depends on itself", decl.name.name);
            self.diags.error("E1902", decl.name.span, message);
            return Self::error_expr(span);
        }
        self.consts[id].resolving = true;
        let module = std::mem::replace(&mut self.module, decl.module);
        let outer_types = std::mem::take(&mut self.type_params);
        let outer_self = self.self_type.take();
        let ty = decl.ty.as_ref().map(|t| self.resolve_type(t));
        let mut cx = self.new_cx(Ty::UNIT);
        let value = self.expr(&mut cx, &decl.value, ty);
        if decl.public && ty.is_none() {
            let written =
                if self.types.has_error(value.ty) { "Type".to_string() } else { self.types.display(value.ty) };
            let name = decl.name.name.rsplit('.').next().unwrap_or(&decl.name.name);
            let message = format!(
                "a public constant declares its type, which is part of the module's API: `pub const {name}: {written} = ...`"
            );
            self.diags.error("E1115", decl.name.span, message);
        }
        let mut value = match ty {
            Some(ty) => self.coerce(value, ty),
            None => value,
        };
        if !self.types.has_error(value.ty) {
            match fold(&self.types, &value) {
                Some(Ok(number)) => value.kind = hir::ExprKind::Int(number),
                Some(Err(problem)) => {
                    let message = format!("evaluating the constant would panic: {problem}");
                    self.diags.error("E1903", decl.value.span, message);
                }
                None if !constant(&mut value) => {
                    let message = "a constant is evaluated in compilation: build it from literals, other constants, their operators and constructors";
                    self.diags.error("E1901", decl.value.span, message);
                }
                None => {}
            }
        }
        self.module = module;
        self.type_params = outer_types;
        self.self_type = outer_self;
        self.consts[id].value = Some(value.clone());
        value.span = span;
        if let hir::ExprKind::Int(_) = value.kind {
            self.const_uses.insert(span);
        }
        value
    }

    /// An `extern fn` (FFI): a module function without body whose
    /// parameters and result cross to C, called inside `unsafe`.
    fn declare_extern(&mut self, f: &ast::FnDecl) {
        self.meta_denied(f.span, "`extern fn`");
        let module = std::mem::replace(&mut self.module, f.module);
        let foreign = f.foreign.clone().expect("an extern fn");
        let (params, result) = self.c_parts(f).unwrap_or_else(|| (vec![Ty::ERROR; f.params.len()], Ty::ERROR));
        let by_value = |this: &Self, ty: Ty| matches!(this.types.kind(this.types.repr(ty)), TyKind::Adt(_));
        if foreign.variadic && (params.iter().any(|&p| by_value(self, p)) || by_value(self, result)) {
            let message =
                "a C function with `...` takes and returns numbers and pointers; pass a `Pointer` to the struct";
            self.diags.error("E1704", f.name.span, message);
        }
        let id = self.externs.len() as u32;
        self.externs.push(hir::Extern {
            name: f.name.name.clone(),
            symbol: foreign.symbol,
            params,
            result,
            variadic: foreign.variadic,
        });
        self.extern_names.push(f.params.iter().map(|p| p.name.name.clone()).collect());
        self.declare_module_name(&f.name, ModuleName::Extern(id));
        self.module = module;
    }

    /// An `extern fn` with a body: a function of Tondo that C calls by its
    /// symbol (16.16), with the parameters and result of a C function.
    fn declare_export(&mut self, f: &ast::FnDecl, id: FnId, paths: &[Vec<String>]) {
        self.meta_denied(f.span, "`extern fn`");
        let module = std::mem::replace(&mut self.module, f.module);
        let foreign = f.foreign.clone().expect("an extern fn");
        let parts = self.c_parts(f);
        self.module = module;
        if foreign.variadic {
            let message = "Tondo cannot read C's `...`: give the function fixed parameters";
            self.diags.error("E1704", f.name.span, message);
        }
        let symbol = foreign.symbol;
        let taken = match self.export_symbols.get(&symbol) {
            Some(&at) => Some(format!(
                "C already calls another function `{symbol}`, at {}: give one of them another `@symbol(\"...\")`",
                self.sources.location(at)
            )),
            None if symbol == "main" || symbol.starts_with("tondo_") => Some(format!(
                "`{symbol}` is a symbol of the program itself: give the function another name, or `@symbol(\"...\")`"
            )),
            None => None,
        };
        if let Some(message) = taken {
            self.diags.error("E1706", f.name.span, message);
            return;
        }
        self.export_symbols.insert(symbol.clone(), f.name.span);
        let Some((params, result)) = parts else { return };
        // A library publishes the `pub` ones of the package it builds.
        let published = f.public && paths[f.module as usize].first() == paths[0].first();
        if published {
            let named = f.params.iter().zip(&params);
            self.c_functions.push(hir::CFunction {
                symbol: symbol.clone(),
                params: named.map(|(p, &ty)| (p.name.name.clone(), self.c_type(p.ty.as_ref(), ty))).collect(),
                result: self.c_type(f.result.as_ref(), result),
                docs: crate::derive::docs(self.sources, f.span),
            });
        }
        let ty = self.types.intern(TyKind::CFn(params, result, false));
        self.exports.push(hir::Export { function: id, symbol, ty, published });
    }

    /// How a header spells a type that crosses to C, from how it is
    /// written: `std.ffi`'s aliases keep their C names.
    fn c_type(&self, written: Option<&ast::TypeExpr>, ty: Ty) -> hir::CType {
        use crate::types::{FloatTy, IntTy};
        use hir::CType;
        if ty == self.c_string {
            return CType::Pointer(Box::new(CType::Named("char")));
        }
        let repr = self.types.repr(ty);
        let kind = self.types.kind(repr).clone();
        // The last name of the path: `CInt` of `ffi.CInt`.
        let (name, args) = match written.map(|w| &w.kind) {
            Some(ast::TypeKind::Path { segments, args }) => {
                (segments.last().and_then(|s| s.name.rsplit('.').next()), &args[..])
            }
            _ => (None, &[][..]),
        };
        let alias = match name {
            Some("CChar") => Some(("char", TyKind::Byte)),
            Some("CShort") => Some(("short", TyKind::Int(IntTy::I16))),
            Some("CUShort") => Some(("unsigned short", TyKind::Int(IntTy::U16))),
            Some("CInt") => Some(("int", TyKind::Int(IntTy::I32))),
            Some("CUInt") => Some(("unsigned int", TyKind::Int(IntTy::U32))),
            Some("CLong") => Some(("long", TyKind::Int(IntTy::I64))),
            Some("CULong") => Some(("unsigned long", TyKind::Int(IntTy::U64))),
            Some("CLongLong") => Some(("long long", TyKind::Int(IntTy::I64))),
            Some("CULongLong") => Some(("unsigned long long", TyKind::Int(IntTy::U64))),
            Some("CSize") => Some(("size_t", TyKind::Int(IntTy::U64))),
            Some("CSSize") => Some(("ssize_t", TyKind::Int(IntTy::I64))),
            Some("CFloat") => Some(("float", TyKind::Float(FloatTy::F32))),
            Some("CDouble") => Some(("double", TyKind::Float(FloatTy::F64))),
            Some("CBool") => Some(("bool", TyKind::Bool)),
            _ => None,
        };
        if let Some((c, aliased)) = alias
            && kind == aliased
        {
            return CType::Named(c);
        }
        match kind {
            TyKind::Int(int) => CType::Named(match int {
                IntTy::I8 => "int8_t",
                IntTy::I16 => "int16_t",
                IntTy::I32 => "int32_t",
                IntTy::I64 => "int64_t",
                IntTy::U8 => "uint8_t",
                IntTy::U16 => "uint16_t",
                IntTy::U32 => "uint32_t",
                IntTy::U64 => "uint64_t",
            }),
            TyKind::Byte => CType::Named("uint8_t"),
            TyKind::Float(FloatTy::F32) => CType::Named("float"),
            TyKind::Float(FloatTy::F64) => CType::Named("double"),
            TyKind::Bool => CType::Named("bool"),
            TyKind::Pointer(inner) => CType::Pointer(Box::new(self.c_type(args.first(), inner))),
            TyKind::Option(inner) => {
                let written = match written.map(|w| &w.kind) {
                    Some(ast::TypeKind::Optional(inner)) => Some(&**inner),
                    _ => None,
                };
                self.c_type(written, inner)
            }
            TyKind::CFn(params, result, _) => {
                let (written_params, written_result) = match written.map(|w| &w.kind) {
                    Some(ast::TypeKind::Fn { params, result, .. }) => (&params[..], result.as_deref()),
                    _ => (&[][..], None),
                };
                let params = params.iter().enumerate().map(|(i, &p)| self.c_type(written_params.get(i), p)).collect();
                CType::Function(params, Box::new(self.c_type(written_result, result)))
            }
            TyKind::CArray(inner, count) => CType::Array(Box::new(self.c_type(written, inner)), count),
            TyKind::Adt(_) if repr == self.void => CType::Named("void"),
            TyKind::Adt(id) => CType::Struct(self.types.adt(id).name.clone()),
            _ => CType::Named("void"),
        }
    }

    /// The `extern type`s the published functions reach, each after those
    /// it holds by value.
    fn c_structs(&self) -> Vec<hir::CStruct> {
        fn reach(this: &Checker, ty: &hir::CType, seen: &mut HashSet<String>, out: &mut Vec<hir::CStruct>) {
            match ty {
                hir::CType::Named(_) => {}
                hir::CType::Pointer(inner) | hir::CType::Array(inner, _) => reach(this, inner, seen, out),
                hir::CType::Function(params, result) => {
                    for ty in params.iter().chain([&**result]) {
                        reach(this, ty, seen, out);
                    }
                }
                hir::CType::Struct(name) => {
                    if !seen.insert(name.clone()) {
                        return;
                    }
                    let found = this.c_decls.iter().find(|(id, _)| this.types.adt(**id).name == *name);
                    let Some((&id, decl)) = found else { return };
                    let written: Vec<&ast::FieldDecl> = match &decl.body {
                        ast::TypeBody::Record(fields) => fields.iter().collect(),
                        _ => Vec::new(),
                    };
                    let fields: Vec<(String, hir::CType)> = match &this.types.adt(id).kind {
                        crate::types::AdtKind::Record(fields) => fields
                            .iter()
                            .zip(&written)
                            .map(|(field, decl)| (field.name.clone(), this.c_type(Some(&decl.ty), field.ty)))
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (_, field) in &fields {
                        reach(this, field, seen, out);
                    }
                    out.push(hir::CStruct {
                        name: name.clone(),
                        fields,
                        layout: this.types.foreign.get(&id).cloned().unwrap_or_default(),
                        docs: crate::derive::docs(this.sources, decl.span),
                    });
                }
            }
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for function in &self.c_functions {
            for ty in function.params.iter().map(|(_, ty)| ty).chain([&function.result]) {
                reach(self, ty, &mut seen, &mut out);
            }
        }
        out
    }

    /// The parameters and result of a C function's declaration, which
    /// cross to C; none for one with type parameters.
    fn c_parts(&mut self, f: &ast::FnDecl) -> Option<(Vec<Ty>, Ty)> {
        if !f.generics.is_empty() {
            self.diags.error("E1704", f.generics[0].name.span, "an `extern fn` takes no type parameters: C has none");
            return None;
        }
        if let Some(owner) = &f.owner {
            let message = "an `extern fn` is a module function, not a method: remove the type before its name";
            self.diags.error("E1704", owner.span, message);
        }
        for annotation in &f.annotations {
            match (annotation.name.name.as_str(), &annotation.argument) {
                ("symbol", Some(ast::AnnotationArg::Text(symbol))) if !symbol.is_empty() => {}
                ("symbol", _) => {
                    let message = "`@symbol` takes the C function's name: `@symbol(\"sqlite3_open\")`";
                    self.diags.error("E1704", annotation.span, message);
                }
                (name, _) => {
                    let message = format!(
                        "unknown annotation `@{name}`; an `extern fn` takes `@symbol(\"name\")`, the name C knows it by"
                    );
                    self.diags.error("E1001", annotation.span, message);
                }
            }
        }
        let mut params = Vec::new();
        for param in &f.params {
            let Some(written) = &param.ty else {
                self.diags.error("E1704", param.span, "an `extern fn` has no receiver: C has no methods");
                params.push(Ty::ERROR);
                continue;
            };
            let ty = self.resolve_type(written);
            if param.mode != ast::Mode::Value {
                let message = format!(
                    "C takes its arguments by value: remove `{}`, or pass a `Pointer[...]` to what it changes",
                    param.mode.keyword()
                );
                self.diags.error("E1704", param.span, message);
            } else if param.variadic {
                let message = "C's variable arguments are a last `...`, without name or type";
                self.diags.error("E1704", param.span, message);
            } else if let Some(default) = &param.default {
                self.diags.error("E1704", default.span, "a parameter of an `extern fn` has no default value");
            } else {
                self.require_c_type(ty, written.span);
            }
            params.push(ty);
        }
        let result = match &f.result {
            Some(written) => {
                let ty = self.resolve_type(written);
                if ty != Ty::UNIT && ty != Ty::NEVER {
                    self.require_c_type(ty, written.span);
                }
                ty
            }
            None => Ty::UNIT,
        };
        Some((params, result))
    }

    /// Reports a type that cannot cross to C (`E1703`): C's numbers,
    /// `Bool`, pointers, `extern fn(...)` and `extern type`s can.
    fn require_c_type(&mut self, ty: Ty, span: Span) -> bool {
        let Some(reason) = self.c_type_error(ty) else { return true };
        let message = format!("{} cannot cross to C: {reason}", self.display(ty));
        self.diags.error("E1703", span, message);
        false
    }

    /// Why a value of `ty` cannot cross to C, if it cannot.
    fn c_type_error(&self, ty: Ty) -> Option<String> {
        let repr = self.types.repr(ty);
        Some(match self.types.kind(repr) {
            TyKind::Int(_)
            | TyKind::Float(_)
            | TyKind::Bool
            | TyKind::Byte
            | TyKind::Pointer(_)
            | TyKind::CFn(..)
            | TyKind::Error => return None,
            TyKind::CArray(element, _) => return self.c_type_error(*element),
            TyKind::Option(inner) if self.types.is_address(*inner) => return None,
            TyKind::Adt(id) => match self.types.foreign.get(id) {
                Some(layout) if !layout.opaque => return None,
                Some(_) => "an opaque C type has no size; pass a `Pointer` to it".into(),
                None => "only an `extern type` is laid out as C lays out a struct".into(),
            },
            TyKind::String => "text crosses as `ffi.CString`".into(),
            TyKind::Array(element) => format!("pass a `Pointer[{}]` to its elements", self.types.display(*element)),
            TyKind::Char => "a character crosses as `UInt32`".into(),
            TyKind::Fn(..) => "a function crosses as `extern fn(...)`".into(),
            TyKind::Option(_) => {
                "only an option of a pointer or of an `extern fn(...)` crosses, as a null address for `none`".into()
            }
            _ => "only numbers, `Bool`, `Byte`, pointers, `extern fn(...)` and `extern type`s do".into(),
        })
    }

    fn declare_prelude(&mut self) {
        // `trait Display { fn display(self): String }` (19.6).
        let self_param = self.types.declare_param("Self".into());
        let signature = Signature {
            params: vec![self_param],
            names: vec!["self".into()],
            receiver: Some(ast::Mode::Ref),
            modes: vec![ast::Mode::Ref],
            variadic: false,
            result: Ty::STRING,
            generics: vec![self_param],
            name: "Display.display".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        let display = TraitMethod { name: "display".into(), signature, default: None };
        self.traits.push(TraitDef { name: "Display".into(), self_param, params: Vec::new(), methods: vec![display] });
        self.trait_names.insert("Display".into(), DISPLAY);
        self.bounds.insert(self_param, vec![(DISPLAY, Vec::new())]);
        // `trait Iterator[T] { fn next(mut self): T? }` (10.17).
        let self_param = self.types.declare_param("Self".into());
        let element = self.types.declare_param("T".into());
        let signature = Signature {
            params: vec![self_param],
            names: vec!["self".into()],
            receiver: Some(ast::Mode::Mut),
            modes: vec![ast::Mode::Mut],
            variadic: false,
            result: self.types.intern(TyKind::Option(element)),
            generics: vec![self_param, element],
            name: "Iterator.next".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        let next = TraitMethod { name: "next".into(), signature, default: None };
        self.traits.push(TraitDef { name: "Iterator".into(), self_param, params: vec![element], methods: vec![next] });
        self.trait_names.insert("Iterator".into(), ITERATOR);
        self.bounds.insert(self_param, vec![(ITERATOR, vec![element])]);
        // The cursors of maps and sets: a collection and a position.
        for (kind, set) in [
            (CursorKind::Entries, false),
            (CursorKind::Keys, false),
            (CursorKind::Values, false),
            (CursorKind::Values, true),
        ] {
            let key = self.types.declare_param("K".into());
            self.types.param_mut(key).key = true;
            let value = self.types.declare_param("V".into());
            let (params, collection) = if set {
                (vec![key], self.types.intern(TyKind::Set(key)))
            } else {
                (vec![key, value], self.map_type(key, value))
            };
            let name = match (kind, set) {
                (_, true) => "SetValues",
                (CursorKind::Entries, _) => "MapEntries",
                (CursorKind::Keys, _) => "MapKeys",
                (CursorKind::Values, _) => "MapValues",
            };
            let template = self.types.declare_template(name.into(), params.clone());
            let fields = vec![
                Field { name: "collection".into(), ty: collection, private: false },
                Field { name: "position".into(), ty: Ty::INT, private: false },
            ];
            self.types.define_template(template, AdtKind::Record(fields));
            self.types.declare_cursor(template, kind);
            let element = match (kind, set) {
                (CursorKind::Entries, false) => self.types.intern(TyKind::Tuple(vec![key, value])),
                (CursorKind::Values, false) => value,
                _ => key,
            };
            let target = self.types.instantiate(template, params.clone());
            self.impls.push(ImplDef {
                intrinsic: true,
                span: Span::default(),
                trait_id: ITERATOR,
                trait_args: vec![element],
                target,
                generics: params,
                methods: vec![None],
            });
            self.cursor_templates.insert((kind, set), template);
        }
        let (id, ty) = self.types.declare_adt("NumericConversionError".into());
        let variants = ["OutOfRange", "NotFinite", "NotIntegral"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.type_names.insert("NumericConversionError".into(), TypeName::Nominal(ty));
        self.conversion_error = ty;
        let (id, ty) = self.types.declare_adt("collections.CollectionError".into());
        let variants = ["InvalidCapacity", "InvalidIndex", "InvalidStep", "ResourceLimit"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.collection_error = ty;
        let (id, ty) = self.types.declare_adt("text.TextError".into());
        let variants = ["InvalidIndex", "InvalidBoundary", "ResourceLimit", "InvalidData"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.text_error = ty;
        let (id, ty) = self.types.declare_adt("io.IoError".into());
        let variants = ["Closed", "InvalidData", "Host"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.io_error = ty;
        for handle in [Handle::Writer, Handle::Reader, Handle::Buffer] {
            self.types.handle(handle, Ty::UNIT);
        }
        for ty in [Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))] {
            self.types.intern(TyKind::Option(ty));
        }
        // What `std.ffi` exposes through `std.internal.memory`: C's `void`,
        // which only a pointer refers to, and C strings.
        let (id, void) = self.types.declare_adt("ffi.Void".into());
        self.types.foreign.insert(id, crate::types::CLayout { opaque: true, ..Default::default() });
        self.void = void;
        let (id, c_string) = self.types.declare_adt("ffi.CString".into());
        let bytes = self.types.intern(TyKind::Pointer(Ty::BYTE));
        self.types.define_adt(id, AdtKind::Newtype(bytes));
        self.c_string = c_string;
        for ty in [c_string, bytes, self.types.intern(TyKind::Pointer(void))] {
            self.types.intern(TyKind::Option(ty));
        }
    }

    /// The type a numeric constructor such as `Int8(value)` names (18.6).
    fn numeric_type(name: &str) -> Option<Ty> {
        Some(match name {
            "Int" | "Int64" => Ty::INT,
            "Int8" => Ty::int(IntTy::I8),
            "Int16" => Ty::int(IntTy::I16),
            "Int32" => Ty::int(IntTy::I32),
            "UInt8" => Ty::int(IntTy::U8),
            "UInt16" => Ty::int(IntTy::U16),
            "UInt32" => Ty::int(IntTy::U32),
            "UInt64" => Ty::int(IntTy::U64),
            "Float" | "Float64" => Ty::F64,
            "Float32" => Ty::F32,
            "Byte" => Ty::BYTE,
            _ => return None,
        })
    }

    /// Whether converting `from` to `to` can fail, or `None` when the table
    /// of 18.6 has no such conversion.
    fn conversion_checked(&self, from: Ty, to: Ty) -> Option<bool> {
        self.types.numeric_conversion(from, to)
    }

    fn import(&mut self, import: &ast::Import) {
        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        let module = match path.as_slice() {
            ["std", "console"] => StdModule::Console,
            ["std", "collections"] => StdModule::Collections,
            ["std", "text"] => StdModule::Text,
            ["std", "time"] => StdModule::Time,
            ["std", "sync"] => StdModule::Sync,
            ["std", "fs"] => StdModule::Fs,
            ["std", "path"] => StdModule::Path,
            ["std", "io"] => StdModule::Io,
            ["std", "env"] => StdModule::Env,
            ["std", "process"] => StdModule::Process,
            ["std", "internal", "json"] => StdModule::InternalJson,
            ["std", "internal", "memory"] => StdModule::InternalMemory,
            ["std", "testing"] => StdModule::Testing,
            ["std", "internal", "regex"] => StdModule::InternalRegex,
            ["std", "internal", "reflect"] => StdModule::InternalReflect,
            ["std", "internal", "text"] => StdModule::InternalText,
            ["std", "internal", "math"] => StdModule::InternalMath,
            ["std", "internal", "meta"] => StdModule::InternalMeta,
            ["std", "internal", "codec"] => StdModule::InternalCodec,
            ["std", "internal", "http"] => StdModule::InternalHttp,
            ["std", "internal", "toml"] => StdModule::InternalToml,
            ["std", "internal", "yaml"] => StdModule::InternalYaml,
            ["std", "internal", "msgpack"] => StdModule::InternalMsgpack,
            ["std", "internal", "cbor"] => StdModule::InternalCbor,
            ["std", "internal", "net"] => StdModule::InternalNet,
            ["std", "internal", "fuzz"] => StdModule::InternalFuzz,
            ["std", "internal", "panic"] => StdModule::InternalPanic,
            ["std", "internal", "time"] => StdModule::InternalTime,
            ["std", "internal", "context"] => StdModule::InternalContext,
            ["std", "internal", "channel"] => StdModule::InternalChannel,
            ["std", rest @ ..] => {
                let name = rest.join(".");
                // Modules the standard library specifies and does not have yet.
                if name == "protobuf" {
                    self.diags.unsupported(import.span, &format!("module `std.{name}`"));
                    return;
                }
                let modules = crate::api::standard_modules();
                let names = modules.iter().filter_map(|m| m.strip_prefix("std."));
                let message = match closest(&name, names) {
                    Some(near) => format!("there is no module `std.{name}`; did you mean `std.{near}`?"),
                    None => format!("there is no module `std.{name}`; `tondo api --list` names the standard modules"),
                };
                self.diags.error("E1008", import.span, message);
                let near = closest(&name, modules.iter().filter_map(|m| m.strip_prefix("std.")));
                if let (Some(near), Some(first), Some(last)) = (near, import.path.first(), import.path.last()) {
                    let edit = (first.span.to(last.span), format!("std.{near}"));
                    self.diags.fix(format!("import `std.{near}`"), false, vec![edit]);
                }
                return;
            }
            _ => {
                let message = format!("module `{}` is not available", path.join("."));
                self.diags.error("E1008", import.span, message);
                return;
            }
        };
        if matches!(
            module,
            StdModule::Fs
                | StdModule::Process
                | StdModule::Env
                | StdModule::Time
                | StdModule::Sync
                | StdModule::Testing
                | StdModule::InternalMemory
        ) && self.meta_denied(import.span, &format!("`{}`", path.join(".")))
        {
            return;
        }
        let name = import.alias.as_ref().unwrap_or(import.path.last().unwrap());
        self.declare_module_name(name, ModuleName::Module(module));
        if module == StdModule::Sync && self.compare_exchange.is_none() {
            let param = self.types.declare_param("T".into());
            self.types.param_mut(param).copy = true;
            let template = self.types.declare_template(format!("{}.CompareExchange", name.name), vec![param]);
            let variant = |name: &str| Variant { name: name.into(), payload: Payload::Tuple(vec![param]) };
            self.types.define_template(template, AdtKind::Enum(vec![variant("Exchanged"), variant("Mismatch")]));
            let generic = self.types.instantiate(template, vec![param]);
            self.compare_exchange = Some((template, generic));
            for handle in [Handle::Semaphore, Handle::Permit, Handle::Barrier] {
                let ty = self.types.handle(handle, Ty::UNIT);
                self.types.intern(TyKind::Option(ty));
            }
        }
        if module == StdModule::Fs && self.fs.is_none() {
            let unit = |name: &str| Variant { name: name.into(), payload: Payload::Unit };
            let declare_enum = |this: &mut Self, ty: &str, variants: &[&str]| {
                let (id, ty) = this.types.declare_adt(format!("{}.{ty}", name.name));
                this.types.define_adt(id, AdtKind::Enum(variants.iter().map(|v| unit(v)).collect()));
                ty
            };
            let errors = [
                "NotFound",
                "PermissionDenied",
                "AlreadyExists",
                "NotADirectory",
                "IsADirectory",
                "InvalidData",
                "InvalidName",
                "Host",
            ];
            let error = declare_enum(self, "FsError", &errors);
            self.types.shown.insert(error, Shown::Words);
            let kind = declare_enum(self, "FileKind", &["File", "Directory", "Symlink", "Other"]);
            let mode = declare_enum(self, "OpenMode", &["Read", "Write", "Append", "CreateNew", "ReadWrite"]);
            let (id, metadata) = self.types.declare_adt(format!("{}.Metadata", name.name));
            let fields = vec![
                Field { name: "kind".into(), ty: kind, private: false },
                Field { name: "size".into(), ty: Ty::INT, private: false },
                Field { name: "readOnly".into(), ty: Ty::BOOL, private: false },
            ];
            self.types.define_adt(id, AdtKind::Record(fields));
            let file = self.types.handle(Handle::File, Ty::UNIT);
            for ty in [Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))] {
                self.types.intern(TyKind::Option(ty));
            }
            self.fs = Some(FsTypes { error, kind, metadata, mode, file });
        }
        if module == StdModule::InternalJson {
            self.types.handle(Handle::JsonReader, Ty::UNIT);
            self.types.handle(Handle::JsonWriter, Ty::UNIT);
        }
        if module == StdModule::InternalRegex {
            self.types.handle(Handle::Regex, Ty::UNIT);
            self.types.intern(TyKind::Array(Ty::INT));
        }
        if module == StdModule::InternalReflect {
            self.types.handle(Handle::ReflectBox, Ty::UNIT);
        }
        if matches!(module, StdModule::InternalText | StdModule::InternalCodec | StdModule::InternalHttp) {
            self.types.intern(TyKind::Array(Ty::STRING));
        }
        if matches!(
            module,
            StdModule::InternalHttp
                | StdModule::InternalCodec
                | StdModule::InternalToml
                | StdModule::InternalYaml
                | StdModule::InternalMsgpack
                | StdModule::InternalCbor
                | StdModule::InternalNet
        ) {
            self.types.handle(Handle::Native, Ty::UNIT);
        }
        if module == StdModule::Process && self.process.is_none() {
            let prefix = &name.name;
            let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
            let writer = self.types.handle(Handle::Writer, Ty::UNIT);
            let reader = self.types.handle(Handle::Reader, Ty::UNIT);
            let variant = |name: &str, payload: Option<Ty>| Variant {
                name: name.into(),
                payload: payload.map_or(Payload::Unit, |ty| Payload::Tuple(vec![ty])),
            };
            let s = Ty::STRING;
            let (id, input) = self.types.declare_adt(format!("{prefix}.Input"));
            let variants = vec![
                variant("Inherit", None),
                variant("Null", None),
                variant("Text", Some(s)),
                variant("Bytes", Some(bytes)),
                variant("File", Some(s)),
                variant("Reader", Some(reader)),
                variant("Pipe", None),
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            let (id, output) = self.types.declare_adt(format!("{prefix}.Output"));
            let variants = vec![
                variant("Inherit", None),
                variant("Null", None),
                variant("Capture", None),
                variant("File", Some(s)),
                variant("Append", Some(s)),
                variant("Writer", Some(writer)),
                variant("Stdout", None),
                variant("Pipe", None),
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            let (id, status) = self.types.declare_adt(format!("{prefix}.ExitStatus"));
            let fields = vec![
                Field { name: "code".into(), ty: Ty::INT, private: false },
                Field { name: "success".into(), ty: Ty::BOOL, private: false },
            ];
            self.types.define_adt(id, AdtKind::Record(fields));
            let statuses = self.types.intern(TyKind::Array(status));
            let (id, result) = self.types.declare_adt(format!("{prefix}.ProcessOutput"));
            let fields = vec![
                Field { name: "stdout".into(), ty: bytes, private: false },
                Field { name: "stderr".into(), ty: bytes, private: false },
                Field { name: "combined".into(), ty: bytes, private: false },
                Field { name: "statuses".into(), ty: statuses, private: false },
            ];
            self.types.define_adt(id, AdtKind::Record(fields));
            let (id, error) = self.types.declare_adt(format!("{prefix}.ProcessError"));
            let variants = vec![
                variant("NotFound", None),
                variant("PermissionDenied", None),
                variant("InvalidData", None),
                Variant { name: "Failed".into(), payload: Payload::Tuple(vec![status, result]) },
                variant("Host", None),
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(error, Shown::ProcessError);
            let process = self.types.handle(Handle::Process, Ty::UNIT);
            for handle in [Handle::Command, Handle::Pipeline] {
                self.types.handle(handle, Ty::UNIT);
            }
            self.process = Some(ProcessTypes { input, output, status, result, error, process });
        }
        if module == StdModule::Env && self.env_error.is_none() {
            let (id, ty) = self.types.declare_adt(format!("{}.EnvError", name.name));
            let variants = ["NotFound", "NotADirectory", "PermissionDenied", "Host"]
                .map(|name| Variant { name: name.into(), payload: Payload::Unit })
                .to_vec();
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(ty, Shown::Words);
            self.env_error = Some(ty);
            self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING));
            self.types.intern(TyKind::Array(Ty::STRING));
        }
        if module == StdModule::Time && self.duration.is_none() {
            let (id, ty) = self.types.declare_adt(format!("{}.Duration", name.name));
            self.types.define_adt(id, AdtKind::Newtype(Ty::INT));
            self.types.shown.insert(ty, Shown::Duration);
            self.duration = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.Instant", name.name));
            self.types.define_adt(id, AdtKind::Newtype(Ty::INT));
            self.instant = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.DateTime", name.name));
            let fields = ["year", "month", "day", "hour", "minute", "second", "nanosecond"]
                .map(|name| Field { name: name.into(), ty: Ty::INT, private: false })
                .to_vec();
            self.types.define_adt(id, AdtKind::Record(fields));
            self.types.shown.insert(ty, Shown::DateTime);
            self.datetime = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.TimeError", name.name));
            let variants = vec![Variant { name: "InvalidDateTime".into(), payload: Payload::Unit }];
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(ty, Shown::Words);
            self.time_error = Some(ty);
        }
    }

    /// A type of an imported standard module, such as `text.TextError`.
    fn module_type(&self, module: &str, name: &str) -> Option<Ty> {
        match (self.names.get(module), name) {
            (Some(ModuleName::Module(StdModule::Env)), "EnvError") => self.env_error,
            (Some(ModuleName::Module(StdModule::InternalJson)), "NativeReader") => {
                self.types.lookup(&TyKind::Handle(Handle::JsonReader, Ty::UNIT))
            }
            (Some(ModuleName::Module(StdModule::InternalJson)), "NativeWriter") => {
                self.types.lookup(&TyKind::Handle(Handle::JsonWriter, Ty::UNIT))
            }
            (Some(ModuleName::Module(StdModule::InternalRegex)), "NativeRegex") => {
                self.types.lookup(&TyKind::Handle(Handle::Regex, Ty::UNIT))
            }
            (
                Some(ModuleName::Module(
                    StdModule::InternalHttp
                    | StdModule::InternalCodec
                    | StdModule::InternalToml
                    | StdModule::InternalYaml
                    | StdModule::InternalMsgpack
                    | StdModule::InternalCbor
                    | StdModule::InternalNet,
                )),
                "Native",
            ) => self.types.lookup(&TyKind::Handle(Handle::Native, Ty::UNIT)),
            (Some(ModuleName::Module(StdModule::InternalReflect)), "Box") => {
                self.types.lookup(&TyKind::Handle(Handle::ReflectBox, Ty::UNIT))
            }
            (Some(ModuleName::Module(StdModule::InternalMemory)), "CString") => Some(self.c_string),
            (Some(ModuleName::Module(StdModule::InternalMemory)), "Void") => Some(self.void),
            (Some(ModuleName::Module(StdModule::Process)), name) => {
                let types = self.process?;
                match name {
                    "Input" => Some(types.input),
                    "Output" => Some(types.output),
                    "ExitStatus" => Some(types.status),
                    "ProcessOutput" => Some(types.result),
                    "ProcessError" => Some(types.error),
                    "Process" => Some(types.process),
                    _ => None,
                }
            }
            (Some(ModuleName::Module(StdModule::Io)), name) => match name {
                "IoError" => Some(self.io_error),
                "Writer" => self.types.lookup(&TyKind::Handle(Handle::Writer, Ty::UNIT)),
                "Reader" => self.types.lookup(&TyKind::Handle(Handle::Reader, Ty::UNIT)),
                "Buffer" => self.types.lookup(&TyKind::Handle(Handle::Buffer, Ty::UNIT)),
                _ => None,
            },
            (Some(ModuleName::Module(StdModule::Collections)), "CollectionError") => Some(self.collection_error),
            (Some(ModuleName::Module(StdModule::Text)), "TextError") => Some(self.text_error),
            (Some(ModuleName::Module(StdModule::Time)), "Duration") => self.duration,
            (Some(ModuleName::Module(StdModule::Time)), "TimeError") => self.time_error,
            (Some(ModuleName::Module(StdModule::Time)), "Instant") => self.instant,
            (Some(ModuleName::Module(StdModule::Time)), "DateTime") => self.datetime,
            (Some(ModuleName::Module(StdModule::Time)), "TimedOut") => self.timed_out(),
            (Some(ModuleName::Module(StdModule::Sync)), "CompareExchange") => self.compare_exchange.map(|(_, g)| g),
            (Some(ModuleName::Module(StdModule::Fs)), name) => {
                let fs = self.fs?;
                match name {
                    "FsError" => Some(fs.error),
                    "FileKind" => Some(fs.kind),
                    "Metadata" => Some(fs.metadata),
                    "OpenMode" => Some(fs.mode),
                    "File" => Some(fs.file),
                    _ => None,
                }
            }
            (Some(ModuleName::Module(StdModule::Sync)), name) => {
                let handle = match name {
                    "Semaphore" => Handle::Semaphore,
                    "Permit" => Handle::Permit,
                    "Barrier" => Handle::Barrier,
                    _ => return None,
                };
                self.types.lookup(&TyKind::Handle(handle, Ty::UNIT))
            }
            _ => None,
        }
    }

    fn declare_module_name(&mut self, name: &ast::Ident, value: ModuleName) {
        if matches!(value, ModuleName::Fn(_)) && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str()) {
            let message = format!(
                "a function cannot be named `{}`: `{}(...)` always constructs a `Result` or `Option`",
                name.name, name.name
            );
            self.diags.error("E1005", name.span, message);
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        } else if self.names.insert(name.name.clone(), value).is_some() {
            let message = format!("`{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
        }
    }

    fn declare_type_name(&mut self, name: &ast::Ident, value: TypeName) {
        if PRELUDE_TYPES.contains(&name.name.as_str()) {
            let message = format!("`{}` is a prelude type and cannot be redeclared", name.name);
            self.diags.error("E1005", name.span, message);
        } else if self.type_names.insert(name.name.clone(), value).is_some() {
            let message = format!("type `{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
        }
    }

    /// Declares every nominal type and alias, then resolves their definitions,
    /// so declarations may refer to each other in any order (7.11).
    /// The name a type shows: its own for an intrinsic type the prelude
    /// declares, such as `Range`.
    fn shown_type_name(&self, name: &str, module: u32) -> String {
        match name.strip_prefix("prelude.") {
            Some(bare) if PRELUDE_TYPES.contains(&bare) && Some(module) == self.prelude => bare.to_string(),
            // `Panic`, which the prelude names from `std.panics`, and
            // `time.TimedOut`, which `std.time` writes in Tondo.
            _ if self.module_paths.get(module as usize).is_some_and(|path| path == &["std", "panics"]) => {
                name.strip_prefix("panics.").unwrap_or(name).to_string()
            }
            _ if self.module_paths.get(module as usize).is_some_and(|path| path == &["std", "datetime"]) => {
                format!("time.{}", name.strip_prefix("datetime.").unwrap_or(name))
            }
            _ => name.to_string(),
        }
    }

    fn declare_types(&mut self, items: &'a [ast::Item]) {
        let mut declared = Vec::new();
        let mut templates = Vec::new();
        for item in items {
            self.index_type(item);
            let owner = match item {
                ast::Item::Type(decl) => Some((
                    decl.module,
                    decl.public,
                    match &decl.body {
                        ast::TypeBody::Record(fields) => fields.iter().map(|f| f.private).collect(),
                        ast::TypeBody::Newtype(_) | ast::TypeBody::Opaque => Vec::new(),
                    },
                )),
                ast::Item::Enum(decl) => Some((decl.module, decl.public, Vec::new())),
                _ => None,
            };
            if let Some((module, public, private_fields)) = owner {
                let name = match item {
                    ast::Item::Type(decl) => &decl.name.name,
                    ast::Item::Enum(decl) => &decl.name.name,
                    _ => unreachable!(),
                };
                self.owners.insert(self.shown_type_name(name, module), Owner { module, public, private_fields });
            }
            let name = match item {
                ast::Item::Type(decl) => &decl.name,
                ast::Item::Enum(decl) => &decl.name,
                ast::Item::Alias(decl) => {
                    let entry = AliasEntry {
                        target: decl.target.clone(),
                        span: decl.name.span,
                        resolved: None,
                        resolving: false,
                    };
                    self.aliases.push(entry);
                    self.declare_type_name(&decl.name, TypeName::Alias(self.aliases.len() - 1));
                    // The prelude's aliases are everywhere, by their own name,
                    // as `Panic`.
                    if let Some(bare) = decl.name.name.strip_prefix("prelude.")
                        && PRELUDE_TYPES.contains(&bare)
                    {
                        self.type_names.insert(bare.to_string(), TypeName::Alias(self.aliases.len() - 1));
                    }
                    continue;
                }
                ast::Item::Fn(_)
                | ast::Item::Trait(_)
                | ast::Item::Impl(_)
                | ast::Item::Const(_)
                | ast::Item::Test(_)
                | ast::Item::Derive(_) => continue,
            };
            let generics = match item {
                ast::Item::Type(decl) => &decl.generics,
                ast::Item::Enum(decl) => &decl.generics,
                _ => unreachable!(),
            };
            if let ast::Item::Type(decl) = item
                && !self.c_type_declaration(decl)
            {
                continue;
            }
            if !generics.is_empty() {
                if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                    self.diags.unsupported(name.span, "generic newtypes");
                    continue;
                }
                let module = match item {
                    ast::Item::Type(decl) => decl.module,
                    ast::Item::Enum(decl) => decl.module,
                    _ => unreachable!(),
                };
                let params = self.declare_generics(generics, false, module);
                let template = self.types.declare_template(self.shown_type_name(&name.name, module), params.clone());
                self.declare_type_name(name, TypeName::Template(template));
                templates.push((item, name, template, generics, params));
                continue;
            }
            let module = match item {
                ast::Item::Type(decl) => decl.module,
                ast::Item::Enum(decl) => decl.module,
                _ => unreachable!(),
            };
            let renamed = |path: &Vec<String>| path == &["std", "panics"] || path == &["std", "datetime"];
            let shown = match self.module_paths.get(module as usize).is_some_and(renamed) {
                true => self.shown_type_name(&name.name, module),
                false => name.name.clone(),
            };
            let (id, ty) = self.types.declare_adt(shown);
            if let ast::Item::Type(decl) = item
                && decl.context
            {
                self.context_of.insert(ty, self.contexts.len() as u32);
                self.contexts.push((ty, decl));
            }
            if let ast::Item::Type(decl) = item
                && decl.foreign
            {
                let layout = self.c_layout(decl);
                self.types.foreign.insert(id, layout);
                self.c_decls.insert(id, decl);
            }
            self.declare_type_name(name, TypeName::Nominal(ty));
            if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                self.declare_module_name(name, ModuleName::Newtype(ty));
            }
            declared.push((item, name, id));
        }
        for &(item, _, id) in &declared {
            let kind = self.adt_kind(item);
            self.types.define_adt(id, kind);
        }
        for &(item, name, template, generics, ref params) in &templates {
            self.enter_generics(generics, params);
            let kind = self.adt_kind(item);
            self.leave_generics(generics);
            self.types.define_template(template, kind);
            // Productivity is checked on the instance with its own parameters.
            let ty = self.types.instantiate(template, params.clone());
            if let TyKind::Adt(id) = *self.types.kind(ty) {
                declared.push((item, name, id));
            }
        }
        for index in 0..self.aliases.len() {
            self.resolve_alias(index);
        }
        if let Some(name) = self.types.runaway.take() {
            let message = format!("the generic type `{name}` contains itself with ever larger type arguments");
            self.diags.error_without_span("E1107", message);
        }
        self.check_productivity(&declared);
    }

    /// Declares the parameters of a generic declaration with the
    /// capabilities their bounds grant.
    /// Declares type parameters. Those of functions, methods and impls are
    /// `Copy` unless they say `~Copy`; a type's parameters assume nothing
    /// (12.1).
    fn declare_generics(&mut self, generics: &[ast::GenericParam], code: bool, module: u32) -> Vec<Ty> {
        let mut params = Vec::new();
        for generic in generics {
            // Outside the root, the root's types are out of sight: other
            // modules' are renamed, and only the prelude's keep their names.
            let name = generic.name.name.as_str();
            let visible = module == 0 || PRELUDE_TYPES.contains(&name);
            if (visible && self.type_named(name).is_some()) || self.type_params.iter().any(|(n, _)| n == name) {
                let message = format!("type parameter `{}` would shadow a visible type", generic.name.name);
                self.diags.error("E1003", generic.name.span, message);
            }
            let param = self.types.declare_param(generic.name.name.clone());
            self.types.param_mut(param).copy = code;
            params.push(param);
        }
        // A bound may mention any parameter of the list.
        self.enter_generics(generics, &params);
        for (generic, &param) in generics.iter().zip(&params) {
            for bound in &generic.bounds {
                self.apply_bound(param, bound);
            }
        }
        self.leave_generics(generics);
        params
    }

    /// Records what a bound lets generic code do with `param` (12.5).
    fn apply_bound(&mut self, param: Ty, bound: &ast::TypeExpr) {
        if let ast::TypeKind::NotCopy = bound.kind {
            let def = self.types.param_mut(param);
            if !def.copy {
                let message =
                    "`~Copy` drops the `Copy` that parameters of functions, methods and impls assume; remove it here";
                self.diags.error("E1115", bound.span, message);
            }
            def.copy = false;
            return;
        }
        let ast::TypeKind::Path { segments, args } = &bound.kind else {
            self.diags.error("E1105", bound.span, "a bound names a trait or capability");
            return;
        };
        match (segments.as_slice(), args.is_empty()) {
            ([name], true) if name.name == "Equatable" => self.types.param_mut(param).equatable = true,
            ([name], true) if name.name == "Key" => self.types.param_mut(param).key = true,
            ([name], true) if name.name == "Copy" => self.types.param_mut(param).copy = true,
            ([name], true) if name.name == "Send" => self.types.param_mut(param).send = true,
            ([name], true) if name.name == "Share" => self.types.param_mut(param).share = true,
            ([name], false) if matches!(name.name.as_str(), "Call" | "CallMut" | "CallOnce") => {
                let [signature] = args.as_slice() else {
                    self.diags.error("E1104", bound.span, "a call bound takes one function type: `Call[fn(A): B]`");
                    return;
                };
                let signature = self.resolve_type(signature);
                if !matches!(self.types.kind(signature), TyKind::Fn(..)) {
                    self.diags.error("E1104", bound.span, "a call bound takes a function type: `Call[fn(A): B]`");
                    return;
                }
                let protocol = match name.name.as_str() {
                    "Call" => Protocol::Call,
                    "CallMut" => Protocol::CallMut,
                    _ => Protocol::CallOnce,
                };
                if self.call_bounds.insert(param, (protocol, signature)).is_some() {
                    self.diags.error("E1115", bound.span, "a type parameter has one call bound");
                }
            }
            _ => {
                if let Some((trait_id, args)) = self.trait_ref(bound) {
                    let bounds = self.bounds.entry(param).or_default();
                    if bounds.contains(&(trait_id, args.clone())) {
                        self.diags.error("E1115", bound.span, "this bound is repeated");
                    } else {
                        bounds.push((trait_id, args));
                    }
                }
            }
        }
    }

    /// Brings the parameters of a generic declaration into scope.
    fn enter_generics(&mut self, generics: &[ast::GenericParam], params: &[Ty]) {
        for (generic, &param) in generics.iter().zip(params) {
            self.type_params.push((generic.name.name.clone(), param));
        }
    }

    fn leave_generics(&mut self, generics: &[ast::GenericParam]) {
        self.type_params.truncate(self.type_params.len() - generics.len());
    }

    fn adt_kind(&mut self, item: &ast::Item) -> AdtKind {
        {
            match item {
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(inner), .. }) => {
                    AdtKind::Newtype(self.resolve_type(inner))
                }
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Record(fields), foreign: true, .. }) => {
                    AdtKind::Record(self.c_fields(fields))
                }
                ast::Item::Type(ast::TypeDecl { name, body: ast::TypeBody::Record(fields), context, .. }) => {
                    self.wires.insert(name.name.clone(), fields.iter().map(wire).collect());
                    AdtKind::Record(self.field_decls(fields, *context))
                }
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Opaque, .. }) => AdtKind::Record(Vec::new()),
                ast::Item::Enum(decl) => {
                    let mut seen = HashSet::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if !seen.insert(variant.name.name.as_str()) {
                            let message = format!("variant `{}` is declared more than once", variant.name.name);
                            self.diags.error("E1505", variant.name.span, message);
                        }
                        let payload = match &variant.payload {
                            ast::VariantPayload::Unit => Payload::Unit,
                            ast::VariantPayload::Tuple(items) => {
                                Payload::Tuple(items.iter().map(|t| self.resolve_type(t)).collect())
                            }
                            ast::VariantPayload::Record(fields) => {
                                let key = format!("{}.{}", decl.name.name, variant.name.name);
                                self.wires.insert(key, fields.iter().map(wire).collect());
                                Payload::Record(self.field_decls(fields, false))
                            }
                        };
                        variants.push(Variant { name: variant.name.name.clone(), payload });
                    }
                    AdtKind::Enum(variants)
                }
                _ => unreachable!(),
            }
        }
    }

    /// Whether a type declaration is well formed as to C: an `extern type`
    /// is a struct or opaque, without type parameters, and only it takes
    /// `@packed` or `@align(n)`.
    fn c_type_declaration(&mut self, decl: &ast::TypeDecl) -> bool {
        if !decl.foreign {
            if let Some(annotation) = decl.annotations.first() {
                let message = format!(
                    "`@{}` lays out a C struct: declare it as `extern type {} = {{ ... }}`",
                    annotation.name.name, decl.name.name
                );
                self.diags.error("E1704", annotation.span, message);
            }
            return true;
        }
        if let Some(generic) = decl.generics.first() {
            self.diags.error("E1704", generic.name.span, "an `extern type` takes no type parameters: C has none");
            return false;
        }
        if let ast::TypeBody::Newtype(inner) = &decl.body {
            let message = "an `extern type` is a C struct, `= { field: Type, ... }`, or opaque, without `=`; C's other types are aliases in `std.ffi`";
            self.diags.error("E1704", inner.span, message);
            return false;
        }
        true
    }

    /// The layout of an `extern type` from `@packed` and `@align(n)`.
    fn c_layout(&mut self, decl: &ast::TypeDecl) -> crate::types::CLayout {
        let mut layout =
            crate::types::CLayout { opaque: matches!(decl.body, ast::TypeBody::Opaque), ..Default::default() };
        for annotation in &decl.annotations {
            match (annotation.name.name.as_str(), &annotation.argument) {
                _ if layout.opaque => {
                    let message = "an opaque C type has no layout: C alone knows it";
                    self.diags.error("E1704", annotation.span, message);
                }
                ("packed", None) => layout.packed = true,
                ("union", None) => layout.union = true,
                ("align", Some(ast::AnnotationArg::Int(align))) if align.is_power_of_two() && *align <= 4096 => {
                    layout.align = *align as u32
                }
                ("align", _) => {
                    let message = "`@align` takes a power of two up to 4096: `@align(16)`";
                    self.diags.error("E1704", annotation.span, message);
                }
                (name, _) => {
                    let message = format!(
                        "unknown annotation `@{name}`; an `extern type` takes `@packed`, `@align(n)` and `@union`"
                    );
                    self.diags.error("E1001", annotation.span, message);
                }
            }
        }
        layout
    }

    /// The fields of a C struct: types that cross to C, and `@count(n)`
    /// arrays of them stored in place.
    fn c_fields(&mut self, fields: &[ast::FieldDecl]) -> Vec<Field> {
        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        for field in fields {
            if !seen.insert(field.name.name.as_str()) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1505", field.name.span, message);
            }
            let mut ty = self.resolve_type(&field.ty);
            if !self.require_c_type(ty, field.ty.span) {
                ty = Ty::ERROR;
            }
            for annotation in &field.annotations {
                match (annotation.name.name.as_str(), &annotation.argument) {
                    ("count", Some(ast::AnnotationArg::Int(count))) if *count > 0 && *count <= u32::MAX as u64 => {
                        ty = self.types.intern(TyKind::CArray(ty, *count as u32));
                    }
                    ("count", _) => {
                        let message = "`@count` takes how many elements the C array has: `@count(16)`";
                        self.diags.error("E1704", annotation.span, message);
                    }
                    (name, _) => {
                        let message = format!(
                            "unknown annotation `@{name}`; a field of an `extern type` takes `@count(n)`, for a C array"
                        );
                        self.diags.error("E1001", annotation.span, message);
                    }
                }
            }
            resolved.push(Field { name: field.name.name.clone(), ty, private: false });
        }
        resolved
    }

    /// The fields of a record or a variant; of a context's record with
    /// `context`, whose fields take `@log` and `@label` (11.16).
    fn field_decls(&mut self, fields: &[ast::FieldDecl], context: bool) -> Vec<Field> {
        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        for field in fields {
            if !seen.insert(field.name.name.as_str()) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1505", field.name.span, message);
            }
            let ty = self.resolve_type(&field.ty);
            for annotation in &field.annotations {
                match (annotation.name.name.as_str(), &annotation.argument) {
                    ("log" | "label", None | Some(ast::AnnotationArg::Text(_))) if context => {}
                    (name, _) if context => {
                        let message = format!(
                            "unknown annotation `@{name}`; a field of a context takes `@log`, which puts it in every log event and panic report, or `@label`, which makes it a label of profiles, each with an optional name: `@log(\"request_id\")`"
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                    ("name", Some(_)) => {}
                    ("ignore", None) if matches!(self.types.kind(ty), TyKind::Option(_) | TyKind::Error) => {}
                    ("ignore", None) => {
                        let message = format!(
                            "an ignored field is optional, since decoding leaves it `none`: write `{}: {}?`",
                            field.name.name,
                            self.types.display(ty)
                        );
                        self.diags.error("E1102", annotation.span, message);
                    }
                    ("name", None) => {
                        let message = "`@name` takes the name the field has outside: `@name(\"user_id\")`";
                        self.diags.error("E1102", annotation.span, message);
                    }
                    _ => {
                        let message = format!(
                            "unknown annotation `@{}`; a field takes `@name(\"text\")` or `@ignore`",
                            annotation.name.name
                        );
                        self.diags.error("E1001", annotation.span, message);
                    }
                }
            }
            resolved.push(Field { name: field.name.name.clone(), ty, private: field.private });
        }
        resolved
    }

    /// Productivity (9.10) as a least fixed point: a nominal type is
    /// productive once some finite value of it can be built from productive
    /// components. A recursive type that never becomes productive is an error.
    fn check_productivity(&mut self, declared: &[(&ast::Item, &ast::Ident, AdtId)]) {
        let mut productive: HashSet<AdtId> = HashSet::new();
        loop {
            let before = productive.len();
            for &(_, _, id) in declared {
                if !productive.contains(&id) && self.adt_productive(id, &productive) {
                    productive.insert(id);
                }
            }
            if productive.len() == before {
                break;
            }
        }
        for &(_, name, id) in declared {
            let ty = self.types.intern(TyKind::Adt(id));
            if !productive.contains(&id) && self.types.reaches(ty, ty) {
                let message = format!("type `{}` contains itself and has no finite value", name.name);
                self.diags.error("E1107", name.span, message);
                self.types.define_adt(id, AdtKind::Newtype(Ty::ERROR));
            }
        }
    }

    fn adt_productive(&self, id: AdtId, productive: &HashSet<AdtId>) -> bool {
        let all = |types: Vec<Ty>| types.iter().all(|&t| self.ty_productive(t, productive));
        match &self.types.adt(id).kind {
            AdtKind::Newtype(inner) => self.ty_productive(*inner, productive),
            AdtKind::Record(fields) => all(fields.iter().map(|f| f.ty).collect()),
            AdtKind::Enum(variants) => variants.iter().any(|v| all(v.payload.types())),
        }
    }

    fn ty_productive(&self, ty: Ty, productive: &HashSet<AdtId>) -> bool {
        match self.types.kind(ty) {
            TyKind::Adt(id) => productive.contains(id),
            TyKind::Tuple(items) => items.iter().all(|&t| self.ty_productive(t, productive)),
            TyKind::Never => false,
            _ => true,
        }
    }

    fn resolve_alias(&mut self, index: usize) -> Ty {
        let entry = &self.aliases[index];
        if let Some(ty) = entry.resolved {
            return ty;
        }
        if entry.resolving {
            let span = entry.span;
            self.diags.error("E1106", span, "transparent aliases form a cycle");
            self.aliases[index].resolved = Some(Ty::ERROR);
            return Ty::ERROR;
        }
        self.aliases[index].resolving = true;
        let target = self.aliases[index].target.clone();
        let ty = self.resolve_type(&target);
        self.aliases[index].resolved.get_or_insert(ty);
        self.aliases[index].resolved.unwrap()
    }

    /// The signature of `f` declared in `outer` (an impl or trait scope, or
    /// the module), recording the scope its body is checked in.
    /// Checks the default values of a function's parameters against their
    /// types, once every name of the module is known.
    fn defaults(&mut self, f: &ast::FnDecl, id: FnId) {
        if f.params.iter().all(|p| p.default.is_none()) {
            return;
        }
        let module = std::mem::replace(&mut self.module, f.module);
        let mut defaults = Vec::new();
        for (index, param) in f.params.iter().enumerate() {
            let Some(default) = &param.default else {
                defaults.push(None);
                continue;
            };
            if param.mode != ast::Mode::Value || param.variadic {
                let message = if param.variadic {
                    "a variadic parameter has no default value: omitted, it is empty"
                } else {
                    "a loan parameter has no default value: it lends the caller's place"
                };
                self.diags.error("E1102", default.span, message);
                defaults.push(None);
                continue;
            }
            let ty = self.signatures[id.0 as usize].params[index];
            let mut cx = self.new_cx(Ty::UNIT);
            let value = self.expr(&mut cx, default, Some(ty));
            let mut value = self.coerce(value, ty);
            if !constant(&mut value) && !self.types.has_error(value.ty) {
                let message = "a default value is a constant: a literal, `none`, `[]`, or a variant, tuple or record of constants";
                self.diags.error("E1102", default.span, message);
            }
            defaults.push(Some(value));
        }
        self.module = module;
        self.signatures[id.0 as usize].defaults = defaults;
    }

    /// A trait method's parameters have no default values: implementations
    /// could not agree on them.
    fn no_defaults(&mut self, f: &ast::FnDecl) {
        if let Some(default) = f.params.iter().find_map(|p| p.default.as_ref()) {
            let message = "a trait method's parameters have no default values; declare an inherent method `fn Type.name(...)` with them";
            self.diags.error("E1102", default.span, message);
        }
    }

    /// A default value for a call whose type arguments are `map`.
    fn instantiate(&mut self, value: &mut hir::Expr, map: &HashMap<Ty, Ty>) {
        value.ty = self.types.subst(value.ty, map);
        for part in constant_parts(value).unwrap_or_default() {
            self.instantiate(part, map);
        }
    }

    fn signature(&mut self, f: &ast::FnDecl, owner: Option<Ty>, outer: FnScope) -> Signature {
        let outer_count = outer.types.len();
        self.type_params.extend(outer.types.iter().cloned());
        let saved_self = std::mem::replace(&mut self.self_type, outer.self_type);
        let binders = self.declare_generics(&f.owner_generics, true, f.module);
        self.enter_generics(&f.owner_generics, &binders);
        let owner = match owner {
            Some(key) => Some(self.receiver_type(key, &binders, f)),
            None => self.self_type,
        };
        if f.owner.is_some() {
            // `Self` names the owner in an inherent method (12.3).
            self.self_type = owner;
        }
        let own = self.declare_generics(&f.generics, true, f.module);
        self.enter_generics(&f.generics, &own);
        let (mut params, mut names, mut receiver, mut variadic) = (Vec::new(), Vec::new(), None, false);
        for (index, param) in f.params.iter().enumerate() {
            let ty = match &param.ty {
                None => {
                    receiver = Some(param.mode);
                    owner.unwrap_or_else(|| {
                        let message = format!(
                            "only a method has a receiver: declare it on its type, as in `fn Type.{}(self, ...)`",
                            f.name.name
                        );
                        self.diags.error("E1002", param.span, message);
                        Ty::ERROR
                    })
                }
                Some(ty) => {
                    let ty = self.resolve_type(ty);
                    if param.variadic {
                        if index + 1 != f.params.len() {
                            self.diags.error("E1102", param.span, "only the last parameter can be variadic");
                        }
                        variadic = true;
                        self.types.intern(TyKind::Array(ty))
                    } else {
                        ty
                    }
                }
            };
            params.push(ty);
            names.push(param.name.name.clone());
        }
        let all_generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let result = match &f.result {
            Some(result) => self.result_type(result, f, all_generics),
            None => Ty::UNIT,
        };
        if f.public {
            self.check_public_api(&params, result, f);
        }
        let scope = FnScope {
            module: f.module,
            types: self.type_params.clone(),
            self_type: self.self_type,
            implementing: outer.implementing.clone(),
        };
        self.leave_generics(&f.generics);
        self.leave_generics(&f.owner_generics);
        self.type_params.truncate(self.type_params.len() - outer_count);
        self.self_type = saved_self;
        let outer_generics: Vec<Ty> = outer.types.iter().map(|&(_, t)| t).collect();
        let generics = outer_generics.into_iter().chain(binders).chain(own).collect();
        let name = match &f.owner {
            Some(owner) => format!("{}.{}", owner.name, f.name.name),
            None => f.name.name.clone(),
        };
        self.fn_scopes.push(scope);
        let modes = f.params.iter().map(|p| p.mode).collect();
        Signature {
            params,
            names,
            receiver,
            modes,
            variadic,
            result,
            generics,
            name,
            defaults: Vec::new(),
            is_unsafe: f.is_unsafe,
            foreign: false,
        }
    }

    /// Declares the traits of the module: names and parameters first, so
    /// that bounds can mention any of them, then their bounds (12.3).
    fn declare_traits(&mut self, traits: &[&ast::TraitDecl]) {
        for decl in traits {
            let name = &decl.name;
            if self.trait_names.contains_key(&name.name) || self.type_names.contains_key(&name.name) {
                let message = format!("`{}` is already declared", name.name);
                self.diags.error("E1002", name.span, message);
            }
            let self_param = self.types.declare_param("Self".into());
            let params = decl.generics.iter().map(|g| self.types.declare_param(g.name.name.clone())).collect();
            let id = TraitId(self.traits.len() as u32);
            self.trait_modules.insert(id, decl.module);
            // The prelude's traits are everywhere, by their own name.
            let shown = match name.name.strip_prefix("prelude.") {
                Some(bare) if Some(decl.module) == self.prelude => {
                    self.trait_names.insert(bare.to_string(), id);
                    bare.to_string()
                }
                _ => name.name.clone(),
            };
            self.traits.push(TraitDef { name: shown, self_param, params, methods: Vec::new() });
            self.trait_names.insert(name.name.clone(), id);
        }
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
            self.index_trait(id.0, decl);
            let (self_param, params) =
                (self.traits[id.0 as usize].self_param, self.traits[id.0 as usize].params.clone());
            self.enter_generics(&decl.generics, &params);
            for (generic, &param) in decl.generics.iter().zip(&params) {
                for bound in &generic.bounds {
                    self.apply_bound(param, bound);
                }
            }
            self.leave_generics(&decl.generics);
            // Inside the trait, `Self` implements it.
            self.bounds.entry(self_param).or_default().push((id, params));
        }
    }

    /// The signatures of each trait's methods, and the bodies of defaults.
    fn trait_methods<'d>(&mut self, traits: &[&'d ast::TraitDecl], bodies: &mut Vec<&'d ast::FnDecl>) {
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
            let def = &self.traits[id.0 as usize];
            let mut types = vec![("Self".to_string(), def.self_param)];
            types.extend(decl.generics.iter().map(|g| g.name.name.clone()).zip(def.params.iter().copied()));
            let scope = FnScope { module: decl.module, types, self_type: Some(def.self_param), implementing: None };
            let mut seen = HashSet::new();
            for f in &decl.methods {
                if !seen.insert(f.name.name.as_str()) {
                    let message = format!("`{}` declares `{}` more than once", decl.name.name, f.name.name);
                    self.diags.error("E1505", f.name.span, message);
                    continue;
                }
                let mut signature = self.signature(f, None, scope.clone());
                signature.name = format!("{}.{}", decl.name.name, f.name.name);
                self.no_defaults(f);
                let default = match &f.body {
                    Some(_) => {
                        let fn_id = FnId(bodies.len() as u32);
                        self.signatures.push(signature.clone());
                        bodies.push(f);
                        Some(fn_id)
                    }
                    None => {
                        self.fn_scopes.pop();
                        None
                    }
                };
                self.traits[id.0 as usize].methods.push(TraitMethod { name: f.name.name.clone(), signature, default });
            }
        }
    }

    /// The methods the prelude gives every iterator (10.17): defaults of the
    /// intrinsic `Iterator`, whose `self` they copy, so their receivers are
    /// `Copy`.
    fn iterator_defaults<'d>(&mut self, decl: &'d ast::TraitDecl, bodies: &mut Vec<&'d ast::FnDecl>) {
        let def = &self.traits[ITERATOR.0 as usize];
        let (self_param, element) = (def.self_param, def.params[0]);
        self.types.param_mut(self_param).copy = true;
        self.types.param_mut(element).copy = true;
        let Some(generic) = decl.generics.first() else { return };
        let types = vec![("Self".to_string(), self_param), (generic.name.name.clone(), element)];
        let scope = FnScope { module: decl.module, types, self_type: Some(self_param), implementing: None };
        for f in &decl.methods {
            if f.name.name == "next" || f.body.is_none() {
                continue;
            }
            let mut signature = self.signature(f, None, scope.clone());
            signature.name = format!("Iterator.{}", f.name.name);
            let id = FnId(bodies.len() as u32);
            self.signatures.push(signature.clone());
            bodies.push(f);
            let method = TraitMethod { name: f.name.name.clone(), signature, default: Some(id) };
            self.traits[ITERATOR.0 as usize].methods.push(method);
        }
    }

    /// The trait a bound or implementation names, with its arguments.
    fn trait_ref(&mut self, t: &ast::TypeExpr) -> Option<(TraitId, Vec<Ty>)> {
        let ast::TypeKind::Path { segments, args } = &t.kind else {
            self.diags.error("E1105", t.span, "expected a trait");
            return None;
        };
        let [name] = segments.as_slice() else {
            self.diags.error("E1105", t.span, "expected a trait");
            return None;
        };
        let Some(&id) = self.trait_names.get(&name.name) else {
            let message = format!("unknown trait `{}`", name.name);
            self.diags.error("E1001", name.span, message);
            return None;
        };
        self.index_refer(name, crate::index::Key::Trait(id.0));
        let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
        let expected = self.traits[id.0 as usize].params.len();
        if args.len() != expected {
            let message = format!("`{}` takes {expected} type argument(s), found {}", name.name, args.len());
            self.diags.error("E1104", t.span, message);
            return None;
        }
        Some((id, args))
    }

    /// Checks an implementation's header and registers its methods (12.4).
    fn implementation<'d>(&mut self, decl: &'d ast::ImplDecl, bodies: &mut Vec<&'d ast::FnDecl>) {
        let binders = self.declare_generics(&decl.generics, true, decl.module);
        self.enter_generics(&decl.generics, &binders);
        let resolved = self.trait_ref(&decl.trait_ref);
        let target = self.resolve_type(&decl.target);
        self.leave_generics(&decl.generics);
        let Some((trait_id, trait_args)) = resolved else { return };
        if target == Ty::ERROR {
            return;
        }
        let trait_name = self.traits[trait_id.0 as usize].name.clone();
        let target_name = self.types.display(target);
        // Coherence (12.6): the module owns the trait or the target's nominal type.
        let local_trait = self.trait_modules.get(&trait_id) == Some(&decl.module);
        let local_target = self.owner(target).is_some_and(|o| o.module == decl.module)
            || (self.datetime_module == Some(decl.module)
                && [self.datetime, self.duration, self.instant].contains(&Some(target)));
        if !local_trait && !local_target {
            let message = format!(
                "`impl {trait_name} for {target_name}` implements a trait for a type, neither of which this module declares; implement it in the module of one of them, or wrap the type in a newtype, such as `type Name = {target_name}`"
            );
            self.diags.error("E1114", decl.target.span, message);
            return;
        }
        let mut mentioned = Vec::new();
        self.types.params_in(target, &mut mentioned);
        trait_args.iter().for_each(|&a| self.types.params_in(a, &mut mentioned));
        for (generic, binder) in decl.generics.iter().zip(&binders) {
            if !mentioned.contains(binder) {
                let message =
                    format!("`{}` must appear in the trait or the target of the implementation", generic.name.name);
                self.diags.error("E1114", generic.name.span, message);
            }
        }
        // A type has at most one `Iterator`, whatever it yields (10.17).
        if trait_id == ITERATOR {
            let earlier: Vec<(Ty, Vec<Ty>)> =
                self.impls.iter().filter(|i| i.trait_id == ITERATOR).map(|i| (i.target, i.generics.clone())).collect();
            for (other, generics) in earlier {
                let vars: Vec<Ty> = binders.iter().chain(&generics).copied().collect();
                if self.types.unify_both(target, other, &vars, &mut HashMap::new()) {
                    let message =
                        format!("`{target_name}` already implements `Iterator`; a type yields one element type");
                    self.diags.error("E1113", decl.span, message);
                    return;
                }
            }
        }
        let earlier: Vec<(Vec<Ty>, Ty, Vec<Ty>)> = self
            .impls
            .iter()
            .filter(|i| i.trait_id == trait_id)
            .map(|i| (i.trait_args.clone(), i.target, i.generics.clone()))
            .collect();
        for (other_args, other_target, other_generics) in earlier {
            if self.headers_overlap((&trait_args, target, &binders), (&other_args, other_target, &other_generics)) {
                let message = format!(
                    "`impl {trait_name} for {target_name}` overlaps an earlier implementation for `{}`",
                    self.types.display(other_target)
                );
                self.diags.error("E1111", decl.span, message);
                return;
            }
        }
        // Each method must match the trait's signature with `Self` and the
        // trait's parameters substituted.
        let def = &self.traits[trait_id.0 as usize];
        let mut map: HashMap<Ty, Ty> = HashMap::from([(def.self_param, target)]);
        map.extend(def.params.iter().copied().zip(trait_args.iter().copied()));
        let declared: Vec<(String, Signature, bool)> =
            def.methods.iter().map(|m| (m.name.clone(), m.signature.clone(), m.default.is_some())).collect();
        let mut types: Vec<(String, Ty)> =
            decl.generics.iter().map(|g| g.name.name.clone()).zip(binders.iter().copied()).collect();
        types.dedup();
        let scope = FnScope {
            module: decl.module,
            types,
            self_type: Some(target),
            implementing: Some((trait_id, trait_args.clone())),
        };
        let mut methods = vec![None; declared.len()];
        for f in &decl.methods {
            let Some(index) = declared.iter().position(|(name, ..)| *name == f.name.name) else {
                let message = format!(
                    "`{trait_name}` has no method `{}`; an operation of `{target_name}` alone is an inherent method `fn {target_name}.{}(...)`",
                    f.name.name, f.name.name
                );
                self.diags.error("E1114", f.name.span, message);
                continue;
            };
            if methods[index].is_some() {
                let message = format!("`{}` is implemented more than once", f.name.name);
                self.diags.error("E1114", f.name.span, message);
                continue;
            }
            let id = FnId(bodies.len() as u32);
            let mut signature = self.signature(f, None, scope.clone());
            signature.name = format!("{trait_name}.{}", f.name.name);
            self.no_defaults(f);
            let expected = &declared[index].1;
            if let Some(problem) = self.signature_mismatch(expected, &signature, &map, binders.len()) {
                // An `unsafe` contract that differs is its own error (16.12).
                let code = if expected.is_unsafe != signature.is_unsafe { "E1702" } else { "E1114" };
                let message = format!("`{}` does not match `{trait_name}`: {problem}", f.name.name);
                self.diags.error(code, f.name.span, message);
            }
            self.signatures.push(signature);
            self.index_implemented(trait_id.0, id.0, &f.name);
            bodies.push(f);
            methods[index] = Some(id);
        }
        for ((name, _, has_default), provided) in declared.iter().zip(&methods) {
            if provided.is_none() && !has_default {
                let message = format!("`impl {trait_name} for {target_name}` is missing `{name}`");
                self.diags.error("E1114", decl.span, message);
            }
        }
        let name =
            self.index.is_some().then(|| self.index_type_name(target).unwrap_or_else(|| self.types.display(target)));
        if let (Some(index), Some(name)) = (self.index.as_mut(), name) {
            let declared = index.of(&crate::index::Key::Trait(trait_id.0));
            index.impls.push((declared, decl.span, name));
        }
        self.impls.push(ImplDef {
            intrinsic: false,
            span: decl.span,
            trait_id,
            trait_args,
            target,
            generics: binders,
            methods,
        });
    }

    /// The size-change termination check of trait resolution (12.6). Each
    /// generic implementation leads from the query of its header to each
    /// trait bound of its binders, with a matrix relating their arguments
    /// (`<` strictly smaller, `=` identical, `?` unknown). Every idempotent
    /// composition that returns to the same trait must shrink some argument.
    fn check_termination(&mut self) {
        let mut edges: Vec<(TraitId, TraitId, Matrix, Span)> = Vec::new();
        for implementation in self.impls.iter().filter(|i| !i.intrinsic) {
            let mut source = implementation.trait_args.clone();
            source.push(implementation.target);
            for &binder in &implementation.generics {
                for (trait_id, args) in self.bounds.get(&binder).cloned().unwrap_or_default() {
                    let mut target = args;
                    target.push(binder);
                    let matrix = target
                        .iter()
                        .map(|&to| source.iter().map(|&from| self.size_relation(to, from)).collect())
                        .collect();
                    edges.push((implementation.trait_id, trait_id, matrix, implementation.span));
                }
            }
        }
        // Saturate the compositions of paths, remembering the traits passed.
        let mut known: HashMap<(TraitId, TraitId, Matrix), (Vec<TraitId>, Span)> = HashMap::new();
        let mut work: Vec<(TraitId, TraitId, Matrix, Vec<TraitId>, Span)> =
            edges.iter().map(|(from, to, m, span)| (*from, *to, m.clone(), vec![*from, *to], *span)).collect();
        while let Some((from, to, matrix, path, span)) = work.pop() {
            if known.contains_key(&(from, to, matrix.clone())) {
                continue;
            }
            for (next_from, next_to, next, _) in &edges {
                if *next_from == to {
                    let mut longer = path.clone();
                    longer.push(*next_to);
                    work.push((from, *next_to, compose(next, &matrix), longer, span));
                }
            }
            known.insert((from, to, matrix), (path, span));
        }
        // Report each cycle once, in a stable order.
        let mut cycles: Vec<(&Vec<TraitId>, &Matrix, Span)> = known
            .iter()
            .filter(|((from, to, matrix), _)| {
                let shrinks = (0..matrix.len()).any(|i| matrix[i][i] == Size::Less);
                from == to && compose(matrix, matrix) == *matrix && !shrinks
            })
            .map(|((_, _, matrix), (path, span))| (path, matrix, *span))
            .collect();
        cycles.sort_by_key(|(path, _, span)| (path.len(), path.iter().map(|t| t.0).collect::<Vec<_>>(), span.start));
        let mut reported = HashSet::new();
        for (path, matrix, span) in cycles {
            if path.iter().any(|t| reported.contains(t)) {
                continue;
            }
            reported.extend(path.iter().copied());
            let names: Vec<String> = path.iter().map(|t| format!("`{}`", self.traits[t.0 as usize].name)).collect();
            let shown: Vec<String> = matrix
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|c| match c {
                            Size::Less => "<",
                            Size::Equal => "=",
                            Size::Unknown => "?",
                        })
                        .collect()
                })
                .collect();
            let message = format!(
                "resolving {} may never end: no type argument gets smaller along the way (matrix [{}]); bound a part of the target instead, as in `impl[T: Trait] Trait for Box[T]`",
                names.join(" -> "),
                shown.join("; ")
            );
            self.diags.error("E1112", span, message);
        }
    }

    /// How the term `to` relates in size to `from` (12.6).
    fn size_relation(&self, to: Ty, from: Ty) -> Size {
        if to == from {
            Size::Equal
        } else if self.strict_subterm(to, from) {
            Size::Less
        } else {
            Size::Unknown
        }
    }

    /// Whether `part` is reached by descending at least once through the
    /// arguments of `whole`'s type constructors.
    fn strict_subterm(&self, part: Ty, whole: Ty) -> bool {
        let children: Vec<Ty> = match self.types.kind(whole) {
            TyKind::Tuple(items) | TyKind::Union(items) | TyKind::Opaque(_, items) => items.clone(),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => vec![*t],
            TyKind::Result(a, b) | TyKind::Map(a, b) => vec![*a, *b],
            TyKind::Fn(params, result, ..) => params.iter().copied().chain([*result]).collect(),
            TyKind::Adt(_) => self.types.instance_of(whole).map_or(Vec::new(), |(_, args)| args.to_vec()),
            _ => Vec::new(),
        };
        children.into_iter().any(|child| child == part || self.strict_subterm(part, child))
    }

    /// How an implementation's method differs from the trait's, if at all.
    fn signature_mismatch(
        &mut self,
        expected: &Signature,
        found: &Signature,
        map: &HashMap<Ty, Ty>,
        binders: usize,
    ) -> Option<String> {
        // `map` binds `Self` and the trait's parameters, which come first.
        let own_expected = &expected.generics[map.len()..];
        let own_found = &found.generics[binders..];
        if own_expected.len() != own_found.len() {
            return Some(format!("{} type parameter(s) are expected", own_expected.len()));
        }
        let mut map = map.clone();
        map.extend(own_expected.iter().copied().zip(own_found.iter().copied()));
        if expected.is_unsafe != found.is_unsafe {
            return Some(if expected.is_unsafe {
                "the trait declares it `unsafe fn`".into()
            } else {
                "the trait declares it without `unsafe`".into()
            });
        }
        if expected.receiver != found.receiver {
            return Some("a different receiver".into());
        }
        if let Some(index) =
            (0..expected.modes.len().min(found.modes.len())).find(|&i| expected.modes[i] != found.modes[i])
        {
            let mode = match expected.modes[index] {
                ast::Mode::Value => "by value".to_string(),
                mode => format!("as a `{}` loan", mode.keyword()),
            };
            return Some(format!("parameter `{}` is passed {mode}", expected.names[index]));
        }
        if expected.params.len() != found.params.len() || expected.variadic != found.variadic {
            return Some(format!("{} parameter(s) are expected", expected.params.len()));
        }
        for (index, (&want, &got)) in expected.params.iter().zip(&found.params).enumerate() {
            let want = self.types.subst(want, &map);
            if want != got && !self.types.has_error(got) {
                let name = &expected.names[index];
                return Some(format!("parameter `{name}` must be {}", self.display(want)));
            }
        }
        let result = self.types.subst(expected.result, &map);
        if result != found.result && !self.types.has_error(found.result) {
            return Some(format!("the result must be {}", self.display(result)));
        }
        None
    }

    /// Whether two implementation headers can describe the same pair of
    /// trait arguments and target (12.6).
    fn headers_overlap(&mut self, a: (&[Ty], Ty, &[Ty]), b: (&[Ty], Ty, &[Ty])) -> bool {
        let vars: Vec<Ty> = a.2.iter().chain(b.2).copied().collect();
        let mut map = HashMap::new();
        let left: Vec<Ty> = a.0.iter().copied().chain([a.1]).collect();
        let right: Vec<Ty> = b.0.iter().copied().chain([b.1]).collect();
        left.iter().zip(&right).all(|(&x, &y)| self.types.unify_both(x, y, &vars, &mut map))
    }

    /// Whether `ty` implements `trait_id` with `args` (12.5): by a bound of a
    /// type parameter, intrinsically (`Display` of built-in values), or by an
    /// implementation whose bounds hold.
    fn satisfies(&mut self, ty: Ty, trait_id: TraitId, args: &[Ty]) -> bool {
        self.satisfies_depth(ty, trait_id, args, 0)
    }

    fn satisfies_depth(&mut self, ty: Ty, trait_id: TraitId, args: &[Ty], depth: usize) -> bool {
        if depth > 32 || self.types.has_error(ty) || ty == Ty::NEVER {
            return true;
        }
        if self.is_abstract(ty) {
            return self.bounds_of(ty).iter().any(|(t, a)| *t == trait_id && a == args);
        }
        if trait_id == DISPLAY {
            let parts: Option<Vec<Ty>> = match self.types.kind(ty) {
                TyKind::Int(_) | TyKind::Float(_) | TyKind::Char | TyKind::Byte | TyKind::Bool | TyKind::String => {
                    Some(Vec::new())
                }
                TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::Option(element) => {
                    Some(vec![*element])
                }
                TyKind::Map(key, value) | TyKind::Result(key, value) => Some(vec![*key, *value]),
                // A union shows the member it holds.
                TyKind::Tuple(items) | TyKind::Union(items) => Some(items.clone()),
                _ => None,
            };
            if let Some(parts) = parts {
                return parts.into_iter().all(|part| self.satisfies_depth(part, DISPLAY, &[], depth + 1));
            }
            // Standard library errors display a human description (7.4);
            // durations and dates their usual forms.
            if self.types.shown.contains_key(&ty) {
                return true;
            }
        }
        let candidates: Vec<usize> = (0..self.impls.len()).filter(|&i| self.impls[i].trait_id == trait_id).collect();
        for index in candidates {
            let (target, trait_args, generics) =
                (self.impls[index].target, self.impls[index].trait_args.clone(), self.impls[index].generics.clone());
            let mut map = HashMap::new();
            let matched = self.types.matches(target, ty, &generics, &mut map)
                && trait_args.iter().zip(args).all(|(&p, &a)| self.types.matches(p, a, &generics, &mut map));
            if !matched {
                continue;
            }
            return generics.iter().all(|g| {
                let actual = map.get(g).copied().unwrap_or(Ty::ERROR);
                self.bound_holds(*g, actual, &map, depth + 1).is_none()
            });
        }
        false
    }

    /// The first bound of the type parameter `param` that `actual` does not
    /// meet, as text for a diagnostic.
    fn bound_holds(&mut self, param: Ty, actual: Ty, map: &HashMap<Ty, Ty>, depth: usize) -> Option<String> {
        let def = self.types.param(param)?;
        let (equatable, key, copy, send, share) = (def.equatable, def.key, def.copy, def.send, def.share);
        if copy && !self.types.is_copy(actual) {
            return Some("`Copy` (type parameters are `Copy` unless declared `~Copy`)".into());
        }
        if send && !self.types.is_send(actual) {
            return Some("`Send`".into());
        }
        if share && !self.types.is_share(actual) {
            return Some("`Share`".into());
        }
        if key && !self.types.is_key(actual) {
            return Some("`Key`".into());
        }
        if equatable && !self.types.is_equatable(actual) {
            return Some("`Equatable`".into());
        }
        if let Some((protocol, signature)) = self.call_bounds.get(&param).copied() {
            let signature = self.types.subst(signature, map);
            if let Some(problem) = self.callable_as(actual, protocol, signature) {
                return Some(problem);
            }
        }
        let bounds = self.bounds.get(&param).cloned().unwrap_or_default();
        for (trait_id, args) in bounds {
            let args: Vec<Ty> = args.iter().map(|&a| self.types.subst(a, map)).collect();
            if !self.satisfies_depth(actual, trait_id, &args, depth) {
                return Some(format!("`{}`", self.traits[trait_id.0 as usize].name));
            }
        }
        None
    }

    /// Infers parameters that only appear in trait bounds of others, such
    /// as `T` in `I: Iterator[T]`, from the implementation of the type
    /// inferred for the bounded parameter when only one can apply.
    fn infer_from_bounds(&mut self, inference: &mut Inference) {
        for var in inference.vars.clone() {
            let Some(&actual) = inference.map.get(&var) else { continue };
            for (trait_id, args) in self.bounds.get(&var).cloned().unwrap_or_default() {
                if args.iter().all(|&a| self.inferred(inference, a).is_some()) {
                    continue;
                }
                let found: Vec<Vec<Ty>> = if self.is_abstract(actual) {
                    self.bounds_of(actual).into_iter().filter(|(t, _)| *t == trait_id).map(|(_, a)| a).collect()
                } else {
                    let candidates: Vec<(Ty, Vec<Ty>, Vec<Ty>)> = self
                        .impls
                        .iter()
                        .filter(|i| i.trait_id == trait_id)
                        .map(|i| (i.target, i.trait_args.clone(), i.generics.clone()))
                        .collect();
                    candidates
                        .into_iter()
                        .filter_map(|(target, trait_args, generics)| {
                            let mut map = HashMap::new();
                            self.types
                                .matches(target, actual, &generics, &mut map)
                                .then(|| trait_args.iter().map(|&a| self.types.subst(a, &map)).collect())
                        })
                        .collect()
                };
                if let [values] = found.as_slice() {
                    for (&arg, &value) in args.iter().zip(values) {
                        self.unify(inference, arg, value);
                    }
                }
            }
        }
    }

    /// The result a closure is checked against: what inference knows, or
    /// `T ! {error}` when only `T` is known, so that the closure infers its
    /// errors, none when it cannot fail; `{error}` when nothing is known.
    fn expected_result(&mut self, inference: &Inference, result: Ty) -> Ty {
        if let Some(result) = self.inferred(inference, result) {
            return result;
        }
        match self.types.kind(result).clone() {
            TyKind::Result(success, _) => match self.inferred(inference, success) {
                Some(success) => self.types.intern(TyKind::Result(success, Ty::ERROR)),
                None => Ty::ERROR,
            },
            _ => Ty::ERROR,
        }
    }

    /// What a closure passed for the type parameter `ty` with a call bound
    /// should look like: the bound's signature with the arguments inferred
    /// so far. An unknown result is left to the closure (`{error}`).
    fn call_expectation(&mut self, inference: &Inference, ty: Ty) -> Option<Ty> {
        let &(_, bound) = self.call_bounds.get(&ty)?;
        let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(bound).clone() else {
            return None;
        };
        let params = params.into_iter().map(|p| self.inferred(inference, p)).collect::<Option<Vec<Ty>>>()?;
        let result = self.expected_result(inference, result);
        Some(self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe)))
    }

    /// What a closure for a `fn(A): B` parameter is checked against when
    /// its parameters are known and its result is not yet: `B` comes from
    /// the closure's body.
    fn fn_expectation(&mut self, inference: &Inference, ty: Ty, annotated: bool) -> Option<Ty> {
        let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(ty).clone() else {
            return None;
        };
        // A closure that writes its parameters' types needs only the result.
        let params = params
            .into_iter()
            .map(|p| self.inferred(inference, p).or(annotated.then_some(Ty::ERROR)))
            .collect::<Option<Vec<Ty>>>()?;
        let result = self.expected_result(inference, result);
        Some(self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe)))
    }

    /// Why `ty` cannot be called as `signature` with the protocol a call
    /// bound demands, if it cannot (11.8).
    fn callable_as(&mut self, ty: Ty, protocol: Protocol, signature: Ty) -> Option<String> {
        let wanted = format!("`{protocol:?}[{}]`", self.types.display(signature));
        match self.callable(ty) {
            Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe }) => {
                let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                if found != signature {
                    Some(format!("{wanted} (its signature is `{}`)", self.types.display(found)))
                } else if once && protocol != Protocol::CallOnce {
                    Some(format!("{wanted} (it gives a capture away, so it is only `CallOnce`)"))
                } else if mutates && protocol == Protocol::Call {
                    Some(format!("{wanted} (it changes its captures, so it is only `CallMut`)"))
                } else {
                    None
                }
            }
            None if self.types.has_error(ty) => None,
            None => Some(wanted),
        }
    }

    /// How a value of `ty` is called, if it can be.
    fn callable(&mut self, ty: Ty) -> Option<Callable> {
        if let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(ty).clone() {
            return Some(Callable { params, result, variadic, mutates: false, once: false, modes, is_unsafe });
        }
        if let Some((protocol, signature)) = self.call_bound_of(ty) {
            let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(signature).clone() else {
                unreachable!()
            };
            let (mutates, once) = (protocol != Protocol::Call, protocol == Protocol::CallOnce);
            return Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe });
        }
        let (_, _, info) = self.types.closure(ty)?;
        Some(Callable {
            params: info.params,
            result: info.result,
            variadic: info.variadic,
            mutates: info.mutates,
            once: info.once,
            modes: info.modes,
            is_unsafe: info.is_unsafe,
        })
    }

    /// A function's result type, where `impl A + B` (12.8) declares an
    /// opaque type with those bounds over the function's type parameters.
    fn result_type(&mut self, result: &ast::TypeExpr, f: &ast::FnDecl, generics: Vec<Ty>) -> Ty {
        let (bounds, error) = match &result.kind {
            ast::TypeKind::Opaque(bounds) => (bounds, None),
            ast::TypeKind::Result { ok: Some(ok), err } if matches!(ok.kind, ast::TypeKind::Opaque(_)) => {
                let ast::TypeKind::Opaque(bounds) = &ok.kind else { unreachable!() };
                (bounds, Some(err))
            }
            _ => return self.resolve_type(result),
        };
        if f.owner.is_none() && self.self_type.is_some() {
            self.diags.error(
                "E0004",
                result.span,
                "trait methods and their implementations cannot return `impl Bound`",
            );
            return Ty::ERROR;
        }
        let shown: Vec<String> = bounds.iter().map(|b| self.bound_text(b)).collect();
        let name = format!("impl {}", shown.join(" + "));
        // Like a type parameter, an opaque result is `Copy` unless it says
        // `~Copy`.
        let carrier = self.types.declare_param(name.clone());
        self.types.param_mut(carrier).copy = true;
        for bound in bounds {
            self.apply_bound(carrier, bound);
        }
        let opaque = self.types.declare_opaque(name, generics, carrier);
        match error {
            Some(error) => {
                let error = self.resolve_type(error);
                self.types.intern(TyKind::Result(opaque, error))
            }
            None => opaque,
        }
    }

    /// Records the concrete type an opaque result hides: every normal path
    /// returns it and it meets the published bounds (12.8).
    fn record_witness(&mut self, cx: &FnCx, opaque: Ty, body: &hir::Block, f: &ast::FnDecl) {
        let span = f.result.as_ref().map_or(f.name.span, |r| r.span);
        let returned = cx.result;
        let witness = match (returned, body.ty) {
            (Ty::ERROR, tail) => tail,
            (returned, Ty::NEVER) => returned,
            (returned, tail) if returned == tail || self.types.has_error(tail) => returned,
            (returned, tail) => {
                let message = format!(
                    "every path of an `impl` result returns one type, but this returns {} and {}",
                    self.display(returned),
                    self.display(tail)
                );
                self.diags.error("E1117", span, message);
                returned
            }
        };
        if witness == Ty::NEVER {
            self.diags.error("E1117", span, "an `impl` result needs a path that returns a value; use `Never` instead");
        }
        let carrier = self.types.opaque(opaque).unwrap().0.carrier;
        if !self.types.has_error(witness)
            && witness != Ty::NEVER
            && let Some(missing) = self.bound_holds(carrier, witness, &HashMap::new(), 0)
        {
            let message =
                format!("{} does not implement {missing}, which the `impl` result publishes", self.display(witness));
            self.diags.error("E1117", span, message);
        }
        self.types.set_witness(opaque, witness);
    }

    /// The source text of a bound, for the name of an opaque type.
    fn bound_text(&self, bound: &ast::TypeExpr) -> String {
        self.sources.file_of(bound.span.start).slice(bound.span).to_string()
    }

    /// The traits bounding a type parameter or an opaque result, with
    /// arguments for the opaque result's type arguments.
    fn bounds_of(&mut self, ty: Ty) -> Vec<(TraitId, Vec<Ty>)> {
        if let Some((def, args)) = self.types.opaque(ty) {
            let map: HashMap<Ty, Ty> = def.generics.iter().copied().zip(args.iter().copied()).collect();
            let bounds = self.bounds.get(&def.carrier).cloned().unwrap_or_default();
            return bounds
                .into_iter()
                .map(|(t, a)| (t, a.into_iter().map(|x| self.types.subst(x, &map)).collect()))
                .collect();
        }
        self.bounds.get(&ty).cloned().unwrap_or_default()
    }

    /// The call bound of a type parameter or an opaque result.
    fn call_bound_of(&mut self, ty: Ty) -> Option<(Protocol, Ty)> {
        if let Some((def, args)) = self.types.opaque(ty) {
            let map: HashMap<Ty, Ty> = def.generics.iter().copied().zip(args.iter().copied()).collect();
            let (protocol, signature) = self.call_bounds.get(&def.carrier).copied()?;
            return Some((protocol, self.types.subst(signature, &map)));
        }
        self.call_bounds.get(&ty).copied()
    }

    /// Whether only bounds describe `ty`: a type parameter or an opaque result.
    fn is_abstract(&self, ty: Ty) -> bool {
        self.types.param(ty).is_some() || self.types.opaque(ty).is_some()
    }

    /// The type of `self` in a method of the owner keyed `key`: a generic
    /// owner applied to the method's binders (11.5).
    fn receiver_type(&mut self, key: Ty, binders: &[Ty], f: &ast::FnDecl) -> Ty {
        if key == Ty::ERROR {
            return key;
        }
        let owner = f.owner.as_ref().unwrap();
        // An intrinsic collection of the prelude, over the method's binders.
        let rebuilt = match (self.types.kind(key).clone(), binders) {
            (TyKind::Array(_), [t]) => Some(TyKind::Array(*t)),
            (TyKind::Deque(_), [t]) => Some(TyKind::Deque(*t)),
            (TyKind::Set(_), [t]) => Some(TyKind::Set(*t)),
            (TyKind::Map(..), [k, v]) => Some(TyKind::Map(*k, *v)),
            (TyKind::Option(_), [t]) => Some(TyKind::Option(*t)),
            (TyKind::Result(..), [t, e]) => Some(TyKind::Result(*t, *e)),
            (TyKind::Array(_) | TyKind::Deque(_) | TyKind::Set(_) | TyKind::Option(_), _) => {
                let message = format!(
                    "an operation of `{0}` declares its type parameter: `fn {0}[T].{1}(...)`",
                    owner.name, f.name.name
                );
                self.diags.error("E1104", owner.span, message);
                return Ty::ERROR;
            }
            (TyKind::Map(..) | TyKind::Result(..), _) => {
                let message = format!("an operation of `{0}` declares its two type parameters", owner.name);
                self.diags.error("E1104", owner.span, message);
                return Ty::ERROR;
            }
            _ => None,
        };
        if let Some(kind) = rebuilt {
            return self.types.intern(kind);
        }
        match self.types.instance_of(key) {
            Some((template, _)) => {
                let params: Vec<String> =
                    self.types.template(template).params.iter().map(|&p| self.types.display(p)).collect();
                if binders.len() != params.len() {
                    let message = format!(
                        "an operation of `{0}` declares its type parameters: `fn {0}[{1}].{2}(...)`",
                        owner.name,
                        params.join(", "),
                        f.name.name
                    );
                    self.diags.error("E1104", owner.span, message);
                    return Ty::ERROR;
                }
                self.types.instantiate(template, binders.to_vec())
            }
            None if !binders.is_empty() => {
                let message = format!("`{}` is not generic", owner.name);
                self.diags.error("E1104", owner.span, message);
                Ty::ERROR
            }
            None => key,
        }
    }

    /// The key under which the prelude declares the methods of an intrinsic
    /// type: a collection, option or result over parameters of its own, or
    /// the type itself.
    fn intrinsic_owner(&mut self, name: &str) -> Option<Ty> {
        if let Some(&ty) = self.intrinsic_owners.get(name) {
            return Some(ty);
        }
        let param = |this: &mut Self, name: &str, key: bool| {
            let param = this.types.declare_param(name.into());
            this.types.param_mut(param).key = key;
            param
        };
        let ty = match name {
            "Array" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Array(t))
            }
            "Deque" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Deque(t))
            }
            "Set" => {
                let t = param(self, "T", true);
                self.types.intern(TyKind::Set(t))
            }
            "Map" => {
                let (k, v) = (param(self, "K", true), param(self, "V", false));
                self.types.intern(TyKind::Map(k, v))
            }
            "Option" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Option(t))
            }
            "Result" => {
                let (t, e) = (param(self, "T", false), param(self, "E", false));
                self.types.intern(TyKind::Result(t, e))
            }
            "String" => Ty::STRING,
            "Char" => Ty::CHAR,
            "Bool" => Ty::BOOL,
            "Byte" => Ty::BYTE,
            "Int" | "Int64" => Ty::INT,
            "Int8" => Ty::int(IntTy::I8),
            "Int16" => Ty::int(IntTy::I16),
            "Int32" => Ty::int(IntTy::I32),
            "UInt8" => Ty::int(IntTy::U8),
            "UInt16" => Ty::int(IntTy::U16),
            "UInt32" => Ty::int(IntTy::U32),
            "UInt64" => Ty::int(IntTy::U64),
            "Float" | "Float64" => self.types.intern(TyKind::Float(crate::types::FloatTy::F64)),
            "Float32" => self.types.intern(TyKind::Float(crate::types::FloatTy::F32)),
            _ => return None,
        };
        self.intrinsic_owners.insert(name.to_string(), ty);
        Some(ty)
    }

    /// The key of the operations of `ty`: a generic type's instance with its
    /// own parameters stands for all of its instances.
    fn owner_key(&mut self, ty: Ty) -> Ty {
        let intrinsic = match self.types.kind(ty) {
            TyKind::Array(_) => Some("Array"),
            TyKind::Deque(_) => Some("Deque"),
            TyKind::Set(_) => Some("Set"),
            TyKind::Map(..) => Some("Map"),
            TyKind::Option(_) => Some("Option"),
            TyKind::Result(..) => Some("Result"),
            _ => None,
        };
        if let Some(name) = intrinsic {
            return self.intrinsic_owner(name).expect("an intrinsic owner");
        }
        match self.types.instance_of(ty) {
            Some((template, _)) => {
                let params = self.types.template(template).params.clone();
                self.types.instantiate(template, params)
            }
            None => ty,
        }
    }

    /// The type an inherent method is declared on: a nominal type of this
    /// module (11.5).
    fn method_owner(&mut self, owner: &ast::Ident, module: u32) -> Ty {
        if self.type_names.contains_key(&owner.name) {
            self.index_refer(owner, crate::index::Key::Type(owner.name.clone()));
        }
        // The prelude declares the methods of the intrinsic types, and so
        // may a fixture of documentation (Appendix C).
        if (Some(module) == self.prelude || self.fixture(owner.span))
            && let Some(key) = self.intrinsic_owner(&owner.name)
        {
            return key;
        }
        let nominal = match self.type_names.get(&owner.name) {
            Some(TypeName::Nominal(ty)) => Some(Some(*ty)),
            Some(TypeName::Template(template)) => {
                let template = *template;
                let params = self.types.template(template).params.clone();
                Some(Some(self.types.instantiate(template, params)))
            }
            Some(TypeName::Alias(_)) => Some(None),
            None => None,
        };
        match nominal {
            Some(Some(ty)) if self.owner(ty).is_some_and(|o| o.module != module) => {
                let message = format!(
                    "`{0}` is declared in another module, which alone declares its methods: move this method there, or write a function that takes a `{0}`",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            Some(Some(ty)) if !PRELUDE_TYPES.contains(&owner.name.as_str()) => ty,
            // An operation the toolchain has natively.
            _ if self.fixture(owner.span) => Ty::ERROR,
            Some(_) => {
                let message = format!(
                    "methods can only be declared on records, enums and newtypes of this module, not `{}`",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            None if PRELUDE_TYPES.contains(&owner.name.as_str()) || self.type_named(&owner.name).is_some() => {
                let message = format!(
                    "methods cannot be added to the intrinsic type `{}`: write a function that takes one",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            None => {
                self.diags.error("E1001", owner.span, unknown_type(&owner.name));
                Ty::ERROR
            }
        }
    }

    fn declare_method(&mut self, f: &ast::FnDecl, owner: Ty, has_receiver: bool, id: FnId) {
        if owner == Ty::ERROR {
            return;
        }
        let name = &f.name.name;
        let type_name = self.types.display(owner);
        let clash = if self.methods.contains_key(&(owner, name.clone())) {
            Some(format!("`{type_name}` already has an operation named `{name}`"))
        } else if self.types.variants(owner).is_some()
            && let TyKind::Adt(adt) = self.types.kind(owner)
            && let AdtKind::Enum(variants) = &self.types.adt(*adt).kind
            && variants.iter().any(|v| &v.name == name)
        {
            Some(format!("`{type_name}.{name}` is a variant; an operation of `{type_name}` needs another name"))
        } else if has_receiver && self.types.components(owner).is_some_and(|_| self.record_has_field(owner, name)) {
            Some(format!("`{name}` is a field of `{type_name}`; a method with a receiver needs another name"))
        } else {
            None
        };
        // What a fixture of documentation declares that the toolchain has
        // is the toolchain's.
        let native = || f.owner.as_ref().is_some_and(|o| crate::api::builtin_methods(&o.name).contains(&name.as_str()));
        match clash {
            _ if self.fixture(f.span) && (clash.is_some() || native()) => {}
            Some(message) => self.diags.error("E1505", f.name.span, message),
            None => {
                self.methods.insert((owner, name.clone()), id);
                self.method_visibility.insert(id, (f.module, f.public));
            }
        }
    }

    /// Whether `span` is in a fixture of documentation (Appendix C).
    fn fixture(&self, span: Span) -> bool {
        self.sources.file_of(span.start).logical.starts_with("<fixture>/")
    }

    fn record_has_field(&self, ty: Ty, name: &str) -> bool {
        matches!(self.types.kind(ty), TyKind::Adt(id)
            if matches!(&self.types.adt(*id).kind, AdtKind::Record(fields) if fields.iter().any(|f| f.name == name)))
    }

    fn validate_main(&mut self, f: &ast::FnDecl, id: FnId) {
        let signature = &self.signatures[id.0 as usize];
        let problem = if f.public {
            Some("`main` cannot be `pub`")
        } else if f.is_unsafe {
            Some("`main` cannot be `unsafe`")
        } else if !f.params.is_empty() {
            Some("`main` takes no parameters")
        } else if signature.result != Ty::UNIT
            && !matches!(self.types.kind(signature.result), TyKind::Result(ok, _) if *ok == Ty::UNIT)
        {
            Some("`main` returns `Unit` or `!E`")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.diags.error("E1803", f.name.span, problem);
        }
    }

    fn resolve_type(&mut self, t: &ast::TypeExpr) -> Ty {
        if let ast::TypeKind::Path { segments, .. } = &t.kind
            && let [name] = segments.as_slice()
            && self.index.is_some()
            && self.type_names.contains_key(&name.name)
            && !self.type_params.iter().any(|(n, _)| *n == name.name)
        {
            self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        }
        match &t.kind {
            ast::TypeKind::Path { segments, args } => {
                if !args.is_empty() {
                    let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
                    if let [module, name] = segments.as_slice()
                        && let Some(ModuleName::Module(StdModule::Sync)) = self.names.get(&module.name)
                    {
                        return self.sync_type(name, &args, t.span);
                    }
                    if let [name] = segments.as_slice()
                        && let Some(&TypeName::Template(template)) = self.type_names.get(&name.name)
                    {
                        let expected = self.types.template(template).params.len();
                        if args.len() != expected {
                            let message =
                                format!("`{}` takes {expected} type argument(s), found {}", name.name, args.len());
                            self.diags.error("E1104", t.span, message);
                            return Ty::ERROR;
                        }
                        return self.types.instantiate(template, args);
                    }
                    return match (segments[0].name.as_str(), args.as_slice()) {
                        ("Option", [inner]) if segments.len() == 1 => self.types.intern(TyKind::Option(*inner)),
                        ("Result", [ok, err]) if segments.len() == 1 => self.types.intern(TyKind::Result(*ok, *err)),
                        ("Array", [element]) if segments.len() == 1 => self.types.intern(TyKind::Array(*element)),
                        ("Deque", [element]) if segments.len() == 1 => self.types.intern(TyKind::Deque(*element)),
                        ("Join", [result]) if segments.len() == 1 => self.types.handle(Handle::Join, *result),
                        ("Group", [result]) if segments.len() == 1 => self.types.handle(Handle::Group, *result),
                        ("Sender", [value]) if segments.len() == 1 => self.types.handle(Handle::Sender, *value),
                        ("Receiver", [value]) if segments.len() == 1 => self.types.handle(Handle::Receiver, *value),
                        ("Ref", [value]) if segments.len() == 1 => self.types.handle(Handle::Ref, *value),
                        ("WeakRef", [value]) if segments.len() == 1 => self.types.handle(Handle::Weak, *value),
                        ("Pointer", [target]) if segments.len() == 1 => self.types.intern(TyKind::Pointer(*target)),
                        ("Range", [element]) if segments.len() == 1 => self.range_type(*element, t.span),
                        ("Map", [key, value]) if segments.len() == 1 => {
                            self.require_key(*key, t.span);
                            self.map_type(*key, *value)
                        }
                        ("Set", [element]) if segments.len() == 1 => {
                            self.require_key(*element, t.span);
                            self.types.intern(TyKind::Set(*element))
                        }
                        (
                            "Option" | "Result" | "Array" | "Deque" | "Map" | "Set" | "Join" | "Group" | "Sender"
                            | "Receiver" | "Ref" | "WeakRef" | "Pointer" | "Range",
                            _,
                        ) if segments.len() == 1 => {
                            self.diags.error("E1104", t.span, "wrong number of type arguments");
                            Ty::ERROR
                        }
                        _ => self.unsupported_type(t.span, "generic types"),
                    };
                }
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                if let [name] = names.as_slice()
                    && let Some(&(_, param)) = self.type_params.iter().rev().find(|(n, _)| n == name)
                {
                    return param;
                }
                if let ["Self"] = names.as_slice() {
                    return self.self_type.unwrap_or_else(|| {
                        self.diags.error(
                            "E1001",
                            t.span,
                            "`Self` only exists in a trait, an implementation or a method",
                        );
                        Ty::ERROR
                    });
                }
                if let [name] = names.as_slice()
                    && self.trait_names.contains_key(*name)
                {
                    let message = format!(
                        "a trait is not a type; take a type parameter bound by it: `fn name[T: {name}](value: T)`"
                    );
                    self.diags.error("E1110", t.span, message);
                    return Ty::ERROR;
                }
                match names.as_slice() {
                    ["Int" | "Int64"] => Ty::INT,
                    ["Int8"] => Ty::int(IntTy::I8),
                    ["Int16"] => Ty::int(IntTy::I16),
                    ["Int32"] => Ty::int(IntTy::I32),
                    ["UInt8"] => Ty::int(IntTy::U8),
                    ["UInt16"] => Ty::int(IntTy::U16),
                    ["UInt32"] => Ty::int(IntTy::U32),
                    ["UInt64"] => Ty::int(IntTy::U64),
                    ["Bool"] => Ty::BOOL,
                    ["String"] => Ty::STRING,
                    ["Unit"] => Ty::UNIT,
                    ["Command"] => self.types.handle(Handle::Command, Ty::UNIT),
                    ["Pipeline"] => self.types.handle(Handle::Pipeline, Ty::UNIT),
                    ["Never"] => Ty::NEVER,
                    [module, name] if self.module_type(module, name).is_some() => {
                        self.module_type(module, name).unwrap()
                    }
                    ["Float" | "Float64"] => Ty::F64,
                    ["Float32"] => Ty::F32,
                    ["Char"] => Ty::CHAR,
                    ["Byte"] => Ty::BYTE,
                    [single] if self.type_names.contains_key(*single) => match self.type_names[*single] {
                        TypeName::Nominal(ty) => ty,
                        TypeName::Alias(index) => self.resolve_alias(index),
                        TypeName::Template(template) => {
                            let params: Vec<String> =
                                self.types.template(template).params.iter().map(|&p| self.types.display(p)).collect();
                            let message = format!("`{single}` takes type arguments: `{single}[{}]`", params.join(", "));
                            self.diags.error("E1104", t.span, message);
                            Ty::ERROR
                        }
                    },
                    _ => {
                        self.diags.error("E1001", t.span, unknown_type(&names.join(".")));
                        Ty::ERROR
                    }
                }
            }
            ast::TypeKind::Result { ok, err } => {
                let ok = ok.as_ref().map_or(Ty::UNIT, |ok| self.resolve_type(ok));
                let err = self.resolve_type(err);
                self.types.intern(TyKind::Result(ok, err))
            }
            ast::TypeKind::Optional(inner) => {
                let inner = self.resolve_type(inner);
                self.types.intern(TyKind::Option(inner))
            }
            ast::TypeKind::Union(members) => {
                let mut types = Vec::new();
                for member in members {
                    let ty = self.resolve_type(member);
                    self.check_union_member(ty, member.span);
                    types.push(ty);
                }
                self.types.union(&types)
            }
            ast::TypeKind::Tuple(items) => {
                let items = items.iter().map(|item| self.resolve_type(item)).collect();
                self.types.intern(TyKind::Tuple(items))
            }
            ast::TypeKind::Opaque(_) => {
                self.diags.error("E0004", t.span, "`impl Bound` only describes the result of a function");
                Ty::ERROR
            }
            ast::TypeKind::NotCopy => {
                self.diags.error("E0004", t.span, "`~Copy` only appears in a list of bounds");
                Ty::ERROR
            }
            ast::TypeKind::Fn { params, modes, variadic, result, is_unsafe: _, is_extern: true } => {
                // The address of a C function: what crosses to C, by value.
                let mut types = Vec::new();
                for param in params {
                    let ty = self.resolve_type(param);
                    self.require_c_type(ty, param.span);
                    types.push(ty);
                }
                if let Some(mode) = modes.iter().find(|&&m| m != ast::Mode::Value) {
                    let message = format!("C takes its arguments by value: remove `{}`", mode.keyword());
                    self.diags.error("E1704", t.span, message);
                }
                // C's `...` has a mode and no type.
                if *variadic && modes.len() == params.len() {
                    let message = "C's variable arguments are a last `...`, without a type";
                    self.diags.error("E1704", t.span, message);
                }
                let result = match result {
                    Some(written) => {
                        let ty = self.resolve_type(written);
                        if ty != Ty::UNIT && ty != Ty::NEVER {
                            self.require_c_type(ty, written.span);
                        }
                        ty
                    }
                    None => Ty::UNIT,
                };
                self.types.intern(TyKind::CFn(types, result, *variadic))
            }
            ast::TypeKind::Fn { params, modes, variadic, result, is_unsafe, is_extern: false } => {
                if modes.len() > params.len() {
                    let message = "C's `...` ends only an `extern fn(...)`; a Tondo variadic parameter is `...T`";
                    self.diags.error("E1102", t.span, message);
                    return Ty::ERROR;
                }
                let mut types: Vec<Ty> = params.iter().map(|p| self.resolve_type(p)).collect();
                if *variadic && let Some(last) = types.last_mut() {
                    *last = self.types.intern(TyKind::Array(*last));
                }
                let result = result.as_ref().map_or(Ty::UNIT, |r| self.resolve_type(r));
                self.types.intern(TyKind::Fn(types, result, *variadic, modes.clone(), *is_unsafe))
            }
        }
    }

    /// Union members are nominal or intrinsic types written as a path (9.5);
    /// anonymous structural types first get a name with a newtype.
    fn check_union_member(&mut self, ty: Ty, span: Span) {
        let anonymous = match self.types.kind(ty) {
            TyKind::Tuple(_) => Some("a tuple"),
            TyKind::Unit => Some("`Unit`"),
            _ => None,
        };
        if let Some(what) = anonymous {
            let message =
                format!("{what} cannot be a union member: wrap it in a newtype, such as `type Pair = (Int, Int)`");
            self.diags.error("E1102", span, message);
        }
    }

    fn unsupported_type(&mut self, span: Span, what: &str) -> Ty {
        self.diags.unsupported(span, what);
        Ty::ERROR
    }

    fn display(&self, ty: Ty) -> String {
        format!("`{}`", self.types.display(ty))
    }

    // ---- functions -------------------------------------------------------

    /// A context for a body whose declared result is `result`. A `T ! E`
    /// result makes the body fallible: it is written in terms of `T` (9.8).
    fn new_cx(&self, result: Ty) -> FnCx {
        let (success, error) = match self.types.kind(result) {
            TyKind::Result(ok, err) => (*ok, Some(*err)),
            _ => (result, None),
        };
        FnCx {
            withs: Vec::new(),
            locals: Vec::new(),
            local_defs: HashMap::new(),
            result_span: None,
            mutable: Vec::new(),
            scopes: vec![Vec::new()],
            result: success,
            error,
            infer_error: false,
            undeclared: None,
            gathers: false,
            loops: Vec::new(),
            diverged: false,
            parent: None,
            env: None,
            captures: Vec::new(),
            mutates: false,
            infer_result: false,
            params: Vec::new(),
            deferred: false,
            body: false,
            unsafe_region: false,
            unsafe_body: false,
            lent: Vec::new(),
            loan_bindings: HashSet::new(),
            test_member: false,
            setup: false,
            named: Vec::new(),
            reads: HashSet::new(),
        }
    }

    fn function(&mut self, f: &ast::FnDecl, id: FnId) -> hir::Function {
        let signature = self.signatures[id.0 as usize].clone();
        let scope = self.fn_scopes[id.0 as usize].clone();
        // A trace names a trait's method by the type: `Cell.display`, and
        // `Box.display[Int]` once monomorphized.
        self.naming = match (&scope.implementing, scope.self_type) {
            (Some(_), Some(target)) => {
                let target = self.types.display(target);
                format!("{}.{}", target.split('[').next().unwrap_or(&target), f.name.name)
            }
            _ => signature.name.clone(),
        };
        let outer_types = std::mem::replace(&mut self.type_params, scope.types);
        let outer_self = std::mem::replace(&mut self.self_type, scope.self_type);
        let outer_implementing = std::mem::replace(&mut self.implementing, scope.implementing);
        self.module = scope.module;
        // An opaque result's witness is inferred from the body (12.8).
        let opaque = match self.types.kind(signature.result) {
            TyKind::Opaque(..) => Some(signature.result),
            TyKind::Result(ok, _) if matches!(self.types.kind(*ok), TyKind::Opaque(..)) => Some(*ok),
            _ => None,
        };
        let mut cx = match opaque {
            Some(opaque) => {
                let open = match self.types.kind(signature.result) {
                    TyKind::Result(_, error) => self.types.intern(TyKind::Result(Ty::ERROR, *error)),
                    _ => Ty::ERROR,
                };
                let mut cx = self.new_cx(open);
                cx.infer_result = true;
                let _ = opaque;
                cx
            }
            None => self.new_cx(signature.result),
        };
        let mut params = Vec::new();
        let mut loans = Vec::new();
        for (param, ty) in f.params.iter().zip(signature.params) {
            // `mut` and `var` parameters, receivers included, borrow the
            // caller's place (16.3); `ref` and values are read-only values.
            let loan = matches!(param.mode, ast::Mode::Mut | ast::Mode::Var);
            let key = crate::index::Key::Param(id.0, param.name.name.clone());
            let local = self.declare_with(&mut cx, &param.name, ty, loan, Some(key));
            cx.params[local.0 as usize] = Some(param.mode);
            params.push(local);
            loans.push(loan);
        }
        let success = (!cx.infer_result).then_some(cx.result);
        cx.result_span = f.result.as_ref().map(|t| (t.span, f.public));
        // `main` without a declared error infers it, as a script does (20.2).
        let main = f.result.is_none() && Some(id) == self.main_fn();
        cx.infer_error = main;
        cx.gathers = true;
        cx.body = true;
        cx.unsafe_region = signature.is_unsafe;
        cx.unsafe_body = signature.is_unsafe;
        let body = self.block(&mut cx, f.body.as_ref().expect("functions with an id have a body"), success);
        if let Some(opaque) = opaque {
            self.record_witness(&cx, opaque, &body, f);
        }
        self.unused(&cx, &params);
        if let Some((error, at, code)) = cx.undeclared {
            self.undeclared_error(f, signature.result, error, at, code);
        }
        self.type_params = outer_types;
        self.self_type = outer_self;
        self.implementing = outer_implementing;
        let result = match cx.error {
            Some(error) if main => {
                let result = self.types.intern(TyKind::Result(Ty::UNIT, error));
                self.signatures[id.0 as usize].result = result;
                result
            }
            _ => signature.result,
        };
        let mut function = hir::Function {
            name: self.naming.clone(),
            generics: signature.generics,
            params,
            loans,
            fallible: cx.error.is_some(),
            locals: cx.locals,
            result,
            body,
            moves: HashSet::new(),
            test: false,
            span: f.name.span,
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// Checks moves and obligations (8.10) once the body is typed; whether
    /// a closure body gives a capture away.
    fn ownership(&mut self, function: &mut hir::Function, params: &[Option<ast::Mode>]) -> bool {
        let (signatures, traits, externs) = (&self.signatures, &self.traits, &self.externs);
        let modes = |callee: Callee| match callee {
            Callee::Fn(id) => signatures.get(id.0 as usize).map(|s| s.modes.clone()).unwrap_or_default(),
            Callee::Trait(id, index) => traits[id.0 as usize].methods[index as usize].signature.modes.clone(),
            Callee::Std(function) => function.modes(),
            // C borrows what it receives.
            Callee::Extern(id) => vec![ast::Mode::Ref; externs[id as usize].params.len()],
        };
        // The parameter modes of the type parameters bound by `Call...`.
        let bound_modes: HashMap<Ty, Vec<ast::Mode>> = self
            .call_bounds
            .iter()
            .filter_map(|(&param, &(_, signature))| match self.types.kind(signature) {
                TyKind::Fn(_, _, _, modes, _) => Some((param, modes.clone())),
                _ => None,
            })
            .collect();
        // The type parameters a call consumes.
        let once: HashSet<Ty> =
            self.call_bounds.iter().filter(|(_, (p, _))| *p == Protocol::CallOnce).map(|(&t, _)| t).collect();
        let context = crate::ownership::Context { params, modes: &modes, bound_modes: &bound_modes, once: &once };
        let gives;
        (function.moves, gives) = crate::ownership::check(&mut self.types, function, context, self.diags);
        gives
    }

    /// The implicit `main` of a root script (spec 20.3). Its error type is
    /// inferred from the `?` and `fail` in its statements.
    fn script_main(&mut self, statements: &[ast::Stmt]) -> hir::Function {
        self.naming = "main".into();
        let mut cx = self.new_cx(Ty::UNIT);
        cx.infer_error = true;
        self.scripting = true;
        let stmts = self.statements(&mut cx, statements);
        self.scripting = false;
        self.unused(&cx, &[]);
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        let body = hir::Block { stmts, tail: None, ty };
        let result = match cx.error {
            Some(error) => self.types.intern(TyKind::Result(Ty::UNIT, error)),
            None => Ty::UNIT,
        };
        let mut function = hir::Function {
            name: "main".into(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: cx.error.is_some(),
            locals: cx.locals,
            result,
            body,
            moves: HashSet::new(),
            test: false,
            span: statements.first().map_or(Span::default(), |s| s.span),
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// The error type a `fail` or `?` may produce here, inferring it in a
    /// root script; `None` in an infallible function.
    fn error_channel(&mut self, cx: &mut FnCx, produced: Ty, span: Span, code: &'static str) -> Option<Ty> {
        match cx.error {
            // What infers its errors fails with the union of what it
            // propagates (20.3).
            _ if cx.infer_error => {
                let channel = match cx.error {
                    Some(error) => self.types.union(&[error, produced]),
                    None => produced,
                };
                cx.error = Some(channel);
                Some(channel)
            }
            Some(error) => Some(error),
            None if cx.gathers => {
                cx.undeclared = Some(match cx.undeclared {
                    Some((before, at, code)) => (self.types.union(&[before, produced]), at, code),
                    None => (produced, span, code),
                });
                None
            }
            None => {
                self.diags.error(code, span, "this function cannot fail; declare its result as `T ! E`");
                None
            }
        }
    }

    /// `?` or `fail` in a function that declares no error: what it
    /// propagates, with the result that declares it and the imports that
    /// name it.
    fn undeclared_error(&mut self, f: &ast::FnDecl, success: Ty, error: Ty, at: Span, code: &'static str) {
        if self.types.has_error(error) {
            return;
        }
        let result = self.types.intern(TyKind::Result(success, error));
        let declared = self.types.display(result);
        let message = format!(
            "`{}` cannot fail, but this propagates {}: declare its result as `{declared}`",
            f.name.name,
            self.display(error)
        );
        self.diags.error(code, at, message);
        let mut edits = match &f.result {
            Some(written) => vec![(written.span, declared.clone())],
            None => {
                // After the parameters' `)`.
                let body = f.body.as_ref().map_or(f.span.end, |b| b.span.start);
                let between = self.sources.slice(Span::new(f.name.span.end, body));
                let Some(close) = between.rfind(')') else { return };
                let place = f.name.span.end + close as u32 + 1;
                vec![(Span::new(place, place), format!(": {declared}"))]
            }
        };
        edits.extend(self.imports_naming(error, at));
        // Callers of a function that now fails must handle it.
        self.diags.fix(format!("declare the result as `{declared}`"), false, edits);
    }

    /// The imports a file lacks to name a type as it is shown, as
    /// `text.TextError` needs `import std.text`.
    fn imports_naming(&self, ty: Ty, at: Span) -> Vec<(Span, String)> {
        let file = self.sources.file_of(at.start);
        let imported: HashSet<&str> = file
            .text
            .lines()
            .filter_map(|line| {
                let path = line.strip_prefix("import ")?.trim();
                Some(match path.split_once(" as ") {
                    Some((_, alias)) => alias.trim(),
                    None => path.rsplit('.').next().unwrap_or(path),
                })
            })
            .collect();
        let shown = self.types.display(ty);
        let mut modules: Vec<&str> = shown
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .filter_map(|name| name.rsplit_once('.').map(|(module, _)| module))
            .filter(|module| !module.contains('.') && !imported.contains(module))
            .collect();
        modules.dedup();
        let standard = crate::api::standard_modules();
        modules
            .into_iter()
            .filter_map(|module| {
                let path = format!("std.{module}");
                let path = match standard.contains(&path) {
                    true => path,
                    false => self.module_paths.iter().find(|p| p.last().is_some_and(|l| l == module))?.join("."),
                };
                Some((self.import_place(at, &path), format!("import {path}\n")))
            })
            .collect()
    }

    /// Where an import of `path` goes in the file of `at`: before the first
    /// import that sorts after it, as the formatter orders them, or after
    /// the last one, or at the file's start.
    fn import_place(&self, at: Span, path: &str) -> Span {
        let file = self.sources.file_of(at.start);
        let mut place = None;
        let mut offset = 0;
        for line in file.text.split_inclusive('\n') {
            if let Some(written) = line.strip_prefix("import ") {
                if written.trim() > path {
                    place = Some(offset);
                    break;
                }
                place = Some(offset + line.len());
            }
            offset += line.len();
        }
        let place = file.base + place.unwrap_or(0) as u32;
        Span::new(place, place)
    }

    /// `main` used as a value or called: where the program starts, whose
    /// error its body infers (20.2), is not.
    fn main_called(&mut self, span: Span) -> hir::Expr {
        let message = "`main` is where the program starts, and nothing calls it: move what it does to a function, \
                       and call that";
        self.diags.error("E1803", span, message);
        Self::error_expr(span)
    }

    /// The root module's `main`.
    fn main_fn(&self) -> Option<FnId> {
        match self.names.get("main") {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        }
    }

    fn lookup(&self, cx: &FnCx, name: &str) -> Option<LocalId> {
        cx.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name).map(|&(_, id)| id)
    }

    /// Whether `name` is a binding of an enclosing function, which a
    /// closure can capture.
    fn visible_capture(&self, cx: &FnCx, name: &str) -> bool {
        cx.parent
            .as_deref()
            .is_some_and(|parent| self.lookup(parent, name).is_some() || self.visible_capture(parent, name))
    }

    /// Reads the binding `name` of an enclosing function from inside a
    /// closure, capturing it on first use (11.8).
    fn capture(&mut self, cx: &mut FnCx, name: &str, span: Span) -> Option<hir::Expr> {
        let env = cx.env?;
        let test_member = cx.test_member;
        let index = match cx.captures.iter().position(|c| c.name == name) {
            Some(index) => index,
            None => {
                let parent = cx.parent.as_deref_mut()?;
                if let Some(local) = self.lookup(parent, name) {
                    parent.reads.insert(local);
                }
                if test_member && let Some(local) = self.lookup(parent, name) {
                    let index = local.0 as usize;
                    let ty = parent.locals[index].ty;
                    let why = if parent.params[index].is_some_and(|m| m != ast::Mode::Value) {
                        Some("a loan".to_string())
                    } else if parent.mutable[index] {
                        Some("a `var`".to_string())
                    } else if !self.types.is_copy(ty) {
                        Some(format!("of type {}, which is not `Copy`", self.display(ty)))
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        let message = format!(
                            "a test cannot use `{name}` of its suite, {why}: a suite shares only `let` bindings of `Copy` types; make what the test needs inside it"
                        );
                        self.diags.error("E2005", span, message);
                        return Some(Self::error_expr(span));
                    }
                }
                let (source, mutable) = match self.lookup(parent, name) {
                    Some(local)
                        if matches!(
                            parent.params[local.0 as usize],
                            Some(ast::Mode::Ref | ast::Mode::Mut | ast::Mode::Var)
                        ) =>
                    {
                        // A loan never escapes into a closure (16.5).
                        let message = if name == "self" {
                            "a closure cannot capture the receiver `self`, a loan; copy what it needs first, as in `let offset = self.offset`".to_string()
                        } else {
                            format!(
                                "a closure cannot capture `{name}`, a loan; copy what it needs into a binding first, or pass it as a closure parameter"
                            )
                        };
                        self.diags.error("E1402", span, message);
                        return Some(Self::error_expr(span));
                    }
                    Some(local) => {
                        if let Some((lent, _)) = parent.lent.iter().find(|(place, e)| place.local == local && *e) {
                            let lent = self.place_name(parent, lent);
                            self.lent_error(&lent, true, span);
                        }
                        let ty = parent.locals[local.0 as usize].ty;
                        let mutable = parent.mutable[local.0 as usize] && Some(local) != parent.env;
                        (Self::mk(hir::ExprKind::Local(local), ty, span), mutable)
                    }
                    None => {
                        let source = self.capture(parent, name, span)?;
                        let mutable = parent.captures.iter().any(|c| c.name == name && c.mutable);
                        (source, mutable)
                    }
                };
                if !cx.unsafe_body && self.holds_pointer(source.ty) {
                    let message = format!(
                        "a closure that captures `{name}`, which holds a raw pointer, is `unsafe`: write `unsafe (...) {{ ... }}`"
                    );
                    self.diags.error("E1702", span, message);
                }
                cx.captures.push(Capture { name: name.into(), ty: source.ty, source, mutable });
                cx.captures.len() - 1
            }
        };
        let closure = Self::mk(hir::ExprKind::Local(env), cx.locals[env.0 as usize].ty, span);
        Some(Self::mk(hir::ExprKind::Field(Box::new(closure), index as u32), cx.captures[index].ty, span))
    }

    /// A closure expression (11.8). Its type is an anonymous record of the
    /// bindings it captures; its body is a function whose first parameter
    /// is the closure, generic over the enclosing type parameters.
    fn closure(&mut self, cx: &mut FnCx, closure: &ast::Closure, expected: Option<Ty>, span: Span) -> hir::Expr {
        let parts = (&closure.params[..], closure.result.as_ref(), &closure.body);
        self.closure_parts(cx, parts, expected, false, closure.is_unsafe, span)
    }

    /// A closure from its parts; a `defer` block is a closure without
    /// parameters whose body cannot leave its scope (13.7).
    fn closure_parts(
        &mut self,
        cx: &mut FnCx,
        (closure_params, closure_result, closure_body): (&[ast::ClosureParam], Option<&ast::TypeExpr>, &ast::Block),
        expected: Option<Ty>,
        deferred: bool,
        is_unsafe: bool,
        span: Span,
    ) -> hir::Expr {
        let kind = ClosureKind { deferred, is_unsafe, test: false, infer_error: false };
        let body = |this: &mut Self, cx: &mut FnCx, success| this.block(cx, closure_body, success);
        self.closure_with(cx, (closure_params, closure_result), expected, kind, span, body)
    }

    /// A closure whose body `check_body` checks in the closure's context,
    /// given the success type it must produce, if known. The closure of a
    /// test or a suite infers its errors, as a script does.
    fn closure_with(
        &mut self,
        cx: &mut FnCx,
        (closure_params, closure_result): (&[ast::ClosureParam], Option<&ast::TypeExpr>),
        expected: Option<Ty>,
        ClosureKind { deferred, is_unsafe, test, infer_error }: ClosureKind,
        span: Span,
        check_body: impl FnOnce(&mut Self, &mut FnCx, Option<Ty>) -> hir::Block,
    ) -> hir::Expr {
        // A closure that C calls takes its parameter types from the
        // `extern fn(...)` expected.
        let expected = expected.and_then(|t| match self.types.kind(t).clone() {
            TyKind::CFn(params, result, _) => Some(Callable {
                modes: vec![ast::Mode::Value; params.len()],
                params,
                result,
                variadic: false,
                mutates: false,
                once: false,
                is_unsafe: false,
            }),
            _ => self.callable(t),
        });
        let mut params = Vec::new();
        let mut modes = Vec::new();
        let mut variadic = false;
        for (index, param) in closure_params.iter().enumerate() {
            // A parameter without a type takes its mode from the expected
            // signature too.
            let mode = match (&param.ty, &expected) {
                (None, Some(callable)) => callable.modes.get(index).copied().unwrap_or(ast::Mode::Value),
                _ => param.mode,
            };
            modes.push(mode);
            let ty = match (&param.ty, &expected) {
                (Some(ty), _) => {
                    let ty = self.resolve_type(ty);
                    if param.variadic {
                        variadic = true;
                        self.types.intern(TyKind::Array(ty))
                    } else {
                        ty
                    }
                }
                (None, Some(callable)) if index < callable.params.len() => {
                    variadic = callable.variadic && index + 1 == callable.params.len();
                    callable.params[index]
                }
                (None, _) => {
                    let message =
                        format!("closure parameter `{0}` needs a type: `({0}: Int) {{ ... }}`", param.name.name);
                    self.diags.error("E1101", param.name.span, message);
                    Ty::ERROR
                }
            };
            params.push(ty);
        }
        let declared = closure_result
            .map(|t| self.resolve_type(t))
            .or(expected.as_ref().map(|e| e.result).filter(|&r| r != Ty::ERROR));
        // Expected to give `T ! E` with `E` not known yet, it infers its
        // errors, none when it cannot fail.
        let infer_failure =
            declared.is_some_and(|d| matches!(self.types.kind(d), TyKind::Result(_, e) if *e == Ty::ERROR));
        let generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let template = self.types.declare_template("closure".into(), generics.clone());
        let env_ty = self.types.instantiate(template, generics.clone());
        let index = self.closure_functions.len();
        self.closure_functions.push(None);
        let body_id = FnId(self.closure_base + index as u32);
        let info = ClosureInfo {
            body: body_id.0,
            params: params.clone(),
            result: declared.unwrap_or(Ty::ERROR),
            variadic,
            modes: modes.clone(),
            mutates: false,
            once: false,
            is_unsafe,
        };
        self.types.declare_closure(template, info);

        let name = match self.closure_name.take() {
            Some(name) => name,
            None if deferred => format!("defer in {}", self.naming),
            None => format!("closure in {}", self.naming),
        };
        // The closures of a test are in the test.
        let outer_naming = test.then(|| std::mem::replace(&mut self.naming, name.clone()));
        // The body is checked in its own context, nested in the current one.
        let mut inner = self.new_cx(declared.unwrap_or(Ty::ERROR));
        inner.infer_result = declared.is_none();
        inner.infer_error = test || infer_error || infer_failure;
        if infer_failure {
            inner.error = None;
        }
        inner.test_member = test;
        inner.deferred = deferred;
        inner.unsafe_region = is_unsafe;
        inner.unsafe_body = is_unsafe;
        let outer = std::mem::replace(cx, inner);
        cx.parent = Some(Box::new(outer));
        let env = self.hidden_local(cx, env_ty);
        cx.mutable[env.0 as usize] = true;
        cx.env = Some(env);
        let mut locals = vec![env];
        for ((param, &ty), &mode) in closure_params.iter().zip(&params).zip(&modes) {
            let loan = matches!(mode, ast::Mode::Mut | ast::Mode::Var);
            let local = self.declare(cx, &param.name, ty, loan);
            cx.params[local.0 as usize] = Some(mode);
            locals.push(local);
        }
        let success = (!cx.infer_result).then_some(cx.result);
        let body = check_body(self, cx, success);
        if let Some(outer) = outer_naming {
            self.naming = outer;
        }
        let result = match declared {
            Some(declared) => declared,
            None if cx.result != Ty::ERROR => cx.result,
            None => body.ty,
        };
        let result = match cx.error {
            _ if infer_failure => {
                let success = cx.result;
                self.types.intern(TyKind::Result(success, cx.error.unwrap_or(Ty::NEVER)))
            }
            Some(error) if test || infer_error => self.types.intern(TyKind::Result(result, error)),
            _ => result,
        };
        let parent = cx.parent.take().expect("a closure context has a parent");
        let inner = std::mem::replace(cx, *parent);
        self.unused(&inner, &locals);

        let fields = inner.captures.iter().map(|c| Field { name: c.name.clone(), ty: c.ty, private: false }).collect();
        self.types.define_template(template, AdtKind::Record(fields));
        let info = self.types.closure_info_mut(template);
        info.result = result;
        info.mutates = inner.mutates;
        // The closure itself and its `mut` and `var` parameters are loans.
        let loans =
            [true].into_iter().chain(modes.iter().map(|m| matches!(m, ast::Mode::Mut | ast::Mode::Var))).collect();
        let mut params = inner.params;
        params[locals[0].0 as usize] = Some(ast::Mode::Mut);
        let mut function = hir::Function {
            name,
            generics,
            params: locals,
            loans,
            fallible: inner.error.is_some() || infer_failure,
            locals: inner.locals,
            result,
            body,
            moves: HashSet::new(),
            test,
            span,
        };
        // A cleanup runs once, so it may give its captures away; another
        // closure that does is called once (`CallOnce`).
        if !deferred && self.ownership(&mut function, &params) {
            self.types.closure_info_mut(template).once = true;
        }
        self.closure_functions[index] = Some(function);
        // The closure value holds the captured values, read now.
        let values = inner.captures.into_iter().enumerate().map(|(i, c)| (i as u32, c.source)).collect();
        Self::mk(hir::ExprKind::Record(values), env_ty, span)
    }

    /// `defer call(args)` or `defer { block }` (13.7): a closure created
    /// now, holding the values the cleanup uses, and called when the scope
    /// is left in any way, in LIFO order, or while a panic unwinds.
    fn defer(&mut self, cx: &mut FnCx, deferred: &ast::Defer, span: Span) -> Option<hir::Stmt> {
        let closure = match deferred {
            ast::Defer::Block(block) => {
                // A cleanup gives nothing back: its final value is dropped (11.9).
                let unit = self.types.intern(TyKind::Fn(Vec::new(), Ty::UNIT, false, Vec::new(), false));
                self.closure_parts(cx, (&[], None, block), Some(unit), true, false, span)
            }
            ast::Defer::Call(call) => {
                if !matches!(call.kind, ast::ExprKind::Call { .. }) {
                    self.diags.error(
                        "E1410",
                        call.span,
                        "`defer` takes a call, such as `defer close(file)`, or a block",
                    );
                    return None;
                }
                let value = self.expr(cx, call, Some(Ty::UNIT));
                if value.ty != Ty::UNIT && !self.types.has_error(value.ty) {
                    let message = format!(
                        "a deferred call must return `Unit` and cannot fail, but this one returns {}; handle it in `defer {{ ... }}`",
                        self.display(value.ty)
                    );
                    self.diags.error("E1410", call.span, message);
                    return None;
                }
                self.deferred_call(value, span)?
            }
        };
        let local = self.hidden_local(cx, closure.ty);
        cx.mutable[local.0 as usize] = true;
        let read = Self::mk(hir::ExprKind::Local(local), closure.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), Ty::UNIT, span);
        Some(hir::Stmt::Defer { local, closure, call })
    }

    /// The closure of `defer call(args)`: its operands are evaluated now
    /// and the call is made at the end of the scope.
    fn deferred_call(&mut self, call: hir::Expr, span: Span) -> Option<hir::Expr> {
        let args = match &call.kind {
            hir::ExprKind::Call(_, _, args) | hir::ExprKind::CallValue(_, args) => args,
            hir::ExprKind::Error => return None,
            _ => {
                self.diags.error("E1410", span, "this call cannot be deferred; write it in a `defer { ... }` block");
                return None;
            }
        };
        if args.iter().any(|a| matches!(a.kind, hir::ExprKind::Loan(_))) {
            self.diags.error("E1410", span, "a deferred call cannot borrow: loans end before the scope does");
            return None;
        }
        let name = format!("defer in {}", self.naming);
        Some(self.call_closure(call, &name, span))
    }

    /// A closure that makes `call` later with its callee and arguments,
    /// which are evaluated now and become its captures: the cleanup of
    /// `defer call(...)` and the body of `spawn call(...)`.
    fn call_closure(&mut self, call: hir::Expr, name: &str, span: Span) -> hir::Expr {
        let result = call.ty;
        let (callee, values) = match call.kind {
            hir::ExprKind::Call(callee, type_args, args) => (Err((callee, type_args)), args),
            hir::ExprKind::CallValue(callee, args) => {
                let mut values = vec![*callee];
                values.extend(args);
                (Ok(()), values)
            }
            _ => unreachable!("only calls become closures"),
        };
        let generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let template = self.types.declare_template(name.into(), generics.clone());
        let fields = values
            .iter()
            .enumerate()
            .map(|(i, a)| Field { name: format!("argument{i}"), ty: a.ty, private: false })
            .collect();
        self.types.define_template(template, AdtKind::Record(fields));
        let env_ty = self.types.instantiate(template, generics.clone());
        let index = self.closure_functions.len();
        let body_id = FnId(self.closure_base + index as u32);
        let info = ClosureInfo {
            body: body_id.0,
            params: Vec::new(),
            result,
            variadic: false,
            modes: Vec::new(),
            mutates: false,
            once: false,
            is_unsafe: false,
        };
        self.types.declare_closure(template, info);
        let env = Self::mk(hir::ExprKind::Local(LocalId(0)), env_ty, span);
        let mut fields: Vec<hir::Expr> = values
            .iter()
            .enumerate()
            .map(|(i, a)| Self::mk(hir::ExprKind::Field(Box::new(env.clone()), i as u32), a.ty, a.span))
            .collect();
        let invoke = match callee {
            Err((callee, type_args)) => hir::ExprKind::Call(callee, type_args, fields),
            Ok(()) => {
                let callee = fields.remove(0);
                hir::ExprKind::CallValue(Box::new(callee), fields)
            }
        };
        let invoke = Self::mk(invoke, result, span);
        let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(invoke)), ty: result };
        self.closure_functions.push(Some(hir::Function {
            name: name.into(),
            generics,
            params: vec![LocalId(0)],
            loans: vec![true],
            fallible: false,
            locals: vec![hir::Local { name: format!("<{name}>"), ty: env_ty, span }],
            result,
            body,
            moves: HashSet::new(),
            test: false,
            span,
        }));
        let values = values.into_iter().enumerate().map(|(i, a)| (i as u32, a)).collect();
        Self::mk(hir::ExprKind::Record(values), env_ty, span)
    }

    /// `spawn call(...)` as a statement: the task belongs to the block,
    /// which waits for it at its end.
    fn spawn_statement(&mut self, cx: &mut FnCx, call: &ast::Expr) -> hir::Stmt {
        // In a loop the block is the iteration, so the tasks would run one
        // at a time: almost never what a loop that spawns wants.
        if !cx.loops.is_empty() {
            let message = "each iteration waits for this task at its end, so the tasks run one at a time; `spawn detached` lets them run at once, or add them to a `Group` and wait for all of them";
            self.diags.warning("W1013", call.span, message);
            let start = Span::new(call.span.start, call.span.start);
            self.diags.fix("launch it with `spawn detached`", false, vec![(start, "detached ".into())]);
        }
        let join = self.expr(cx, call, None);
        let local = self.hidden_local(cx, join.ty);
        hir::Stmt::Let(local, join)
    }

    /// `spawn call(arguments)` (11.11): the callee and arguments are
    /// evaluated now, by value, and a new task makes the call.
    fn spawn(&mut self, cx: &mut FnCx, call: &ast::Expr, span: Span) -> hir::Expr {
        self.meta_denied(span, "`spawn`");
        let ast::ExprKind::Call { args, .. } = &call.kind else {
            let message =
                "`spawn` takes a call, such as `spawn fetch(url)`; put other work in a function and spawn its call";
            self.diags.error("E1611", call.span, message);
            return Self::error_expr(span);
        };
        if let Some(arg) = args.iter().find(|a| a.mode != ast::Mode::Value) {
            let message = format!(
                "a task cannot borrow: remove `{}` to move or copy the value into the task",
                arg.mode.keyword()
            );
            self.diags.error("E1609", arg.span, message);
            return Self::error_expr(span);
        }
        let call = self.expr(cx, call, None);
        self.spawn_call(call, span)
    }

    fn spawn_call(&mut self, call: hir::Expr, span: Span) -> hir::Expr {
        match call.kind {
            // Named arguments are evaluated in order before the call.
            hir::ExprKind::Block(mut block) if block.tail.is_some() => {
                let tail = self.spawn_call(*block.tail.take().unwrap(), span);
                block.ty = tail.ty;
                let ty = tail.ty;
                block.tail = Some(Box::new(tail));
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            hir::ExprKind::Call(_, _, ref args) | hir::ExprKind::CallValue(_, ref args) => {
                if args.iter().any(|a| matches!(a.kind, hir::ExprKind::Loan(_) | hir::ExprKind::Region(..))) {
                    let message = "a task cannot borrow: this call lends a place with `mut` or `var`; make the task return the new value instead";
                    self.diags.error("E1609", span, message);
                    return Self::error_expr(span);
                }
                // What enters the task, and its result that comes back, cross
                // between tasks (16.13).
                let crossing = match &call.kind {
                    hir::ExprKind::CallValue(callee, args) => std::iter::once(&**callee).chain(args).collect(),
                    _ => args.iter().collect::<Vec<_>>(),
                };
                for value in crossing {
                    if let Some(part) = self.not_send(value.ty) {
                        let message = format!(
                            "`spawn` moves this value into another task, but it holds {}, which is not `Send`: it stays in the task that has it",
                            self.display(part)
                        );
                        self.diags.error("E1605", value.span, message);
                        return Self::error_expr(span);
                    }
                }
                if let Some(part) = self.not_send(call.ty) {
                    let message = format!(
                        "the task's result goes back to its `Join`, but it holds {}, which is not `Send`",
                        self.display(part)
                    );
                    self.diags.error("E1605", span, message);
                    return Self::error_expr(span);
                }
                let result = call.ty;
                // A trace names the task by the function it runs.
                let name = match &call.kind {
                    hir::ExprKind::Call(hir::Callee::Fn(id), ..) if (id.0 as usize) < self.signatures.len() => {
                        format!("spawn {}", self.signatures[id.0 as usize].name)
                    }
                    _ => "spawn".to_string(),
                };
                let closure = self.call_closure(call, &name, span);
                let (body, ..) = self.types.closure(closure.ty).expect("a call closure");
                let erased = self.types.intern(TyKind::Fn(Vec::new(), result, false, Vec::new(), false));
                let erased = Self::mk(hir::ExprKind::Erase(Box::new(closure), FnId(body)), erased, span);
                let join = self.types.handle(Handle::Join, result);
                Self::mk(hir::ExprKind::Spawn(Box::new(erased)), join, span)
            }
            hir::ExprKind::Error => Self::error_expr(span),
            _ => {
                let message = "`spawn` takes a call of a function, method or closure; put this work in a function and spawn its call";
                self.diags.error("E1611", span, message);
                Self::error_expr(span)
            }
        }
    }

    /// A `std.sync` type (14.4.4) with its type arguments.
    fn sync_type(&mut self, name: &ast::Ident, args: &[Ty], span: Span) -> Ty {
        let handle = |this: &mut Self, handle: Handle, state: TyKind| {
            let state = this.types.intern(state);
            this.types.handle(handle, state)
        };
        match (name.name.as_str(), args) {
            ("Mutex", [t]) => self.types.handle(Handle::Mutex, *t),
            ("RwLock", [t]) => self.types.handle(Handle::RwLock, *t),
            ("Atomic", [t]) => self.types.handle(Handle::Atomic, *t),
            ("Once", [t]) => handle(self, Handle::Once, TyKind::Option(*t)),
            ("Array", [t]) => handle(self, Handle::SyncArray, TyKind::Array(*t)),
            ("Stack", [t]) => handle(self, Handle::SyncStack, TyKind::Array(*t)),
            ("Queue", [t]) => handle(self, Handle::SyncQueue, TyKind::Deque(*t)),
            ("Set", [k]) => {
                self.require_key(*k, span);
                handle(self, Handle::SyncSet, TyKind::Set(*k))
            }
            ("Map", [k, v]) => {
                self.require_key(*k, span);
                let map = self.map_type(*k, *v);
                self.types.handle(Handle::SyncMap, map)
            }
            ("CompareExchange", [t]) => {
                let (template, _) = self.compare_exchange.expect("declared with the module");
                self.types.instantiate(template, vec![*t])
            }
            (
                "Mutex" | "RwLock" | "Atomic" | "Once" | "Array" | "Stack" | "Queue" | "Set" | "Map"
                | "CompareExchange",
                _,
            ) => {
                self.diags.error("E1104", span, "wrong number of type arguments");
                Ty::ERROR
            }
            _ => {
                let message = format!("module `sync` has no generic type `{}`", name.name);
                self.diags.error("E1001", name.span, message);
                Ty::ERROR
            }
        }
    }

    /// The methods of `fs.File` (standard library 10.5): a `var` loan of the
    /// file, except `close`, which takes it.
    /// A value a function gives back as its result `expected`: a fallible
    /// function may give the whole `T ! E` it declares, which propagates its
    /// error like `?` (11.2).
    fn returned(&mut self, cx: &FnCx, value: hir::Expr, expected: Ty) -> hir::Expr {
        if let Some(error) = cx.error
            && expected == cx.result
            && let TyKind::Result(ok, produced) = *self.types.kind(value.ty)
            && ok == expected
            && produced == error
        {
            let span = value.span;
            return Self::mk(hir::ExprKind::Try(Box::new(value)), expected, span);
        }
        self.coerce(value, expected)
    }

    /// Whether `Type.Variant` names a variant without payload of an enum.
    fn unit_variant(&self, path: &[ast::Ident]) -> bool {
        let Some(TypeName::Nominal(ty)) = self.type_names.get(&path[0].name) else { return false };
        let TyKind::Adt(id) = *self.types.kind(*ty) else { return false };
        match &self.types.adt(id).kind {
            AdtKind::Enum(variants) => {
                variants.iter().any(|v| v.name == path[1].name && matches!(v.payload, Payload::Unit))
            }
            _ => false,
        }
    }

    /// A raw operation of a `Pointer[T]`, or `toPointer` of a `UInt64`
    /// (16.12): only inside an `unsafe` region.
    fn raw(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        types: Vec<Ty>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let address = Ty::int(IntTy::U64);
        let target = match *self.types.kind(receiver.ty) {
            TyKind::Pointer(target) => Some(target),
            _ => None,
        };
        let known = matches!(
            (target.is_some(), method.name.as_str()),
            (true, "read" | "write" | "offset" | "cast" | "address") | (false, "toPointer" | "toFunction")
        );
        if !known {
            return self.no_method(receiver.ty, method, span);
        }
        if !cx.unsafe_region {
            let message = format!(
                "`{}` is a raw operation: write it inside `unsafe {{ ... }}`, once its preconditions hold",
                method.name
            );
            self.diags.error("E1701", method.span, message);
        }
        if let Some(target) = target
            && matches!(method.name.as_str(), "read" | "write" | "offset")
            && self.types.c_layout(target).is_some_and(|l| l.opaque)
        {
            let message = format!(
                "{} is opaque: C alone knows its size, so only C reads, writes or steps over it",
                self.display(target)
            );
            self.diags.error("E1703", method.span, message);
            return Self::error_expr(span);
        }
        let generic = matches!(method.name.as_str(), "cast" | "toPointer" | "toFunction");
        let ty_arg = match (generic, types.as_slice()) {
            (true, [ty]) => Some(*ty),
            (false, []) => None,
            (true, _) => {
                let example = match method.name.as_str() {
                    "cast" => "pointer.cast[Byte]()",
                    "toPointer" => "address.toPointer[Byte]()",
                    _ => "address.toFunction[extern fn(Int): Int]()",
                };
                let message = format!("`{}` takes the type it points to: `{example}`", method.name);
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            (false, _) => {
                let message = format!("`{}` takes no type arguments", method.name);
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
        };
        let params = match method.name.as_str() {
            "write" => vec![target.unwrap()],
            "offset" => vec![Ty::INT],
            _ => Vec::new(),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let retype = |value: hir::Expr, ty: Ty| Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span);
        let receiver_ty = values[0].ty;
        match method.name.as_str() {
            "read" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Read, values), target.unwrap(), span),
            "write" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Write, values), Ty::UNIT, span),
            "offset" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Offset, values), receiver_ty, span),
            "address" => retype(values.remove(0), address),
            // The C function at an address, such as a sentinel `(fn)-1` of C.
            "toFunction" => {
                let function = ty_arg.unwrap();
                if !matches!(self.types.kind(function), TyKind::CFn(..)) && !self.types.has_error(function) {
                    let message = format!(
                        "`toFunction` makes a C function: its type argument is an `extern fn(...)`, not {}",
                        self.display(function)
                    );
                    self.diags.error("E1104", span, message);
                    return Self::error_expr(span);
                }
                retype(values.remove(0), function)
            }
            _ => {
                let pointer = self.types.intern(TyKind::Pointer(ty_arg.unwrap()));
                retype(values.remove(0), pointer)
            }
        }
    }

    /// Whether a value of `ty` may hold a raw pointer.
    fn holds_pointer(&self, ty: Ty) -> bool {
        fn walk(types: &Types, ty: Ty, seen: &mut Vec<Ty>) -> bool {
            if seen.contains(&ty) {
                return false;
            }
            seen.push(ty);
            let repr = types.repr(ty);
            match types.kind(repr) {
                TyKind::Pointer(_) => true,
                TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) => walk(types, *t, seen),
                TyKind::Map(a, b) | TyKind::Result(a, b) => walk(types, *a, seen) || walk(types, *b, seen),
                TyKind::Tuple(items) | TyKind::Union(items) => items.iter().any(|&t| walk(types, t, seen)),
                _ => match (types.components(repr), types.variants(repr)) {
                    (Some(items), _) => items.iter().any(|&t| walk(types, t, seen)),
                    (None, Some(variants)) => variants.concat().iter().any(|&t| walk(types, t, seen)),
                    _ => false,
                },
            }
        }
        walk(&self.types, ty, &mut Vec::new())
    }

    fn native_ty(&mut self, ty: hir::NativeTy) -> Ty {
        match ty {
            hir::NativeTy::Int => Ty::INT,
            hir::NativeTy::Float => Ty::F64,
            hir::NativeTy::Bool => Ty::BOOL,
            hir::NativeTy::String => Ty::STRING,
            hir::NativeTy::Unit => Ty::UNIT,
            hir::NativeTy::JsonReader => self.types.handle(Handle::JsonReader, Ty::UNIT),
            hir::NativeTy::JsonWriter => self.types.handle(Handle::JsonWriter, Ty::UNIT),
            hir::NativeTy::IoReader => self.types.handle(Handle::Reader, Ty::UNIT),
            hir::NativeTy::IoWriter => self.types.handle(Handle::Writer, Ty::UNIT),
            hir::NativeTy::BytePointer => self.types.intern(TyKind::Pointer(Ty::BYTE)),
            hir::NativeTy::Bytes => self.types.intern(TyKind::Array(Ty::BYTE)),
            hir::NativeTy::Ints => self.types.intern(TyKind::Array(Ty::INT)),
            hir::NativeTy::Regex => self.types.handle(Handle::Regex, Ty::UNIT),
            hir::NativeTy::ReflectBox => self.types.handle(Handle::ReflectBox, Ty::UNIT),
            hir::NativeTy::Strings => self.types.intern(TyKind::Array(Ty::STRING)),
            hir::NativeTy::Char => Ty::CHAR,
            hir::NativeTy::Object => self.types.handle(Handle::Native, Ty::UNIT),
        }
    }

    /// Nanoseconds in a unit of `std.time`, by the name of its constructor
    /// and reader.
    fn time_unit(name: &str) -> Option<i128> {
        Some(match name {
            "nanoseconds" => 1,
            "microseconds" => 1_000,
            "milliseconds" => 1_000_000,
            "seconds" => 1_000_000_000,
            "minutes" => 60_000_000_000,
            "hours" => 3_600_000_000_000,
            _ => return None,
        })
    }

    /// A method of a `time.Duration` or `time.Instant` (14.3): arithmetic
    /// on their nanoseconds, which panics with `P0005` when it overflows.
    fn time_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let (duration, instant) = (self.duration.unwrap(), self.instant.unwrap());
        let ty = receiver.ty;
        let nanos = |value: hir::Expr| Self::mk(hir::ExprKind::Retype(Box::new(value)), Ty::INT, span);
        let binary = |op: BinOp, left: hir::Expr, right: hir::Expr, ty: Ty| {
            Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span)
        };
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, span);
        let retype = |value: hir::Expr, ty: Ty| Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span);
        let name = method.name.as_str();
        let param = match (ty == duration, name) {
            (true, "add" | "subtract" | "isLessThan") | (false, "add") => Some(duration),
            (true, "multiply") => Some(Ty::INT),
            (false, "since") => Some(instant),
            _ => None,
        };
        let known = param.is_some()
            || (ty == duration && (Self::time_unit(name).is_some() || matches!(name, "isZero" | "isNegative")))
            || (ty == instant && name == "elapsed");
        if !known {
            return self.no_method(ty, method, span);
        }
        let expected = usize::from(param.is_some());
        if args.len() != expected {
            return self.arity_error(span, expected, args.len());
        }
        let argument = param.map(|param| {
            let value = self.expr(cx, &args[0].value, Some(param));
            self.coerce(value, param)
        });
        let own = nanos(receiver);
        match (ty == duration, name, argument) {
            (true, "add", Some(other)) => retype(binary(BinOp::Add, own, nanos(other), Ty::INT), duration),
            (true, "subtract", Some(other)) => retype(binary(BinOp::Sub, own, nanos(other), Ty::INT), duration),
            (true, "multiply", Some(factor)) => retype(binary(BinOp::Mul, own, factor, Ty::INT), duration),
            (true, "isLessThan", Some(other)) => binary(BinOp::Lt, own, nanos(other), Ty::BOOL),
            (true, "isZero", None) => binary(BinOp::Eq, own, int(0), Ty::BOOL),
            (true, "isNegative", None) => binary(BinOp::Lt, own, int(0), Ty::BOOL),
            (true, unit, None) => match Self::time_unit(unit).unwrap() {
                1 => own,
                factor => binary(BinOp::Div, own, int(factor), Ty::INT),
            },
            (false, "add", Some(delta)) => retype(binary(BinOp::Add, own, nanos(delta), Ty::INT), instant),
            (false, "since", Some(earlier)) => retype(binary(BinOp::Sub, own, nanos(earlier), Ty::INT), duration),
            (false, "elapsed", None) => {
                let now =
                    Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::TimeNow), Vec::new(), Vec::new()), instant, span);
                retype(binary(BinOp::Sub, nanos(now), own, Ty::INT), duration)
            }
            _ => unreachable!("checked above"),
        }
    }

    /// A method of a `Command`, a `Pipeline` or a `process.Process` (19.2).
    fn process_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::ProcessFn as P;
        let Some(types) = self.process else {
            let message = format!("running a {} takes `import std.process`", self.display(receiver.ty));
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        };
        let ty = receiver.ty;
        let failing = |this: &mut Self, ok: Ty| this.types.intern(TyKind::Result(ok, types.error));
        let (s, plan) = (Ty::STRING, handle != Handle::Process);
        let (function, params, result) = match (handle, method.name.as_str()) {
            (Handle::Command, "directory") => (P::Configure(0), vec![s], ty),
            (Handle::Command, "env") => (P::Configure(1), vec![s, s], ty),
            (Handle::Command, "removeEnv") => (P::Configure(2), vec![s], ty),
            (Handle::Command, "clearEnv") => (P::Configure(3), vec![], ty),
            (_, "stdin") if plan => (P::Configure(4), vec![types.input], ty),
            (_, "stdout") if plan => (P::Configure(5), vec![types.output], ty),
            (Handle::Command, "stderr") => (P::Configure(6), vec![types.output], ty),
            (_, "mergeStderr") if plan => (P::Configure(7), vec![], ty),
            (_, "run") if plan => (P::Run, vec![], failing(self, types.status)),
            (_, "status") if plan => (P::Status, vec![], failing(self, types.status)),
            (_, "output") if plan => (P::Output, vec![], failing(self, types.result)),
            (_, "check") if plan => (P::Check, vec![], failing(self, types.result)),
            (_, "start") if plan => (P::Start, vec![], failing(self, types.process)),
            (Handle::Process, "stdin") => (P::Stream(0), vec![], self.types.handle(Handle::Writer, Ty::UNIT)),
            (Handle::Process, "stdout") => (P::Stream(1), vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Process, "stderr") => (P::Stream(2), vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Process, "id") => (P::Id, vec![], Ty::INT),
            (Handle::Process, "wait") => (P::Wait, vec![], failing(self, types.status)),
            (Handle::Process, "cancel") => (P::Cancel, vec![], failing(self, types.status)),
            (Handle::Process, "output") => (P::Collect(false), vec![], failing(self, types.result)),
            (Handle::Process, "check") => (P::Collect(true), vec![], failing(self, types.result)),
            (Handle::Process, "detach") => (P::Detach, vec![], Ty::UNIT),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            // A path where a redirection goes: name the variant that takes it.
            if value.ty == Ty::STRING && (ty == types.input || ty == types.output) {
                let message = format!(
                    "expected {}, found `String`: write `{}.File(path)`",
                    self.display(ty),
                    self.types.display(ty)
                );
                self.diags.error("E1102", value.span, message);
                return Self::error_expr(span);
            }
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Process(function)), Vec::new(), values), result, span)
    }

    /// A method of an `io.Writer`, `io.Reader` or `io.Buffer` (10.3): shared
    /// handles, which their methods only read.
    fn io_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, this.io_error));
        let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
        use hir::IoFn as I;
        let (function, params, result) = match (handle, method.name.as_str()) {
            (Handle::Writer, "write") => (I::Write, vec![Ty::STRING], failing(self, Ty::UNIT)),
            (Handle::Writer, "writeLine") => (I::WriteLine, vec![Ty::STRING], failing(self, Ty::UNIT)),
            (Handle::Writer, "writeBytes") => (I::WriteBytes, vec![bytes], failing(self, Ty::UNIT)),
            (Handle::Writer, "flush") => (I::Flush, vec![], failing(self, Ty::UNIT)),
            (Handle::Writer | Handle::Reader, "close") => (I::Close, vec![], failing(self, Ty::UNIT)),
            (Handle::Reader, "readLine") => {
                let line = self.types.intern(TyKind::Option(Ty::STRING));
                (I::ReadLine, vec![], failing(self, line))
            }
            (Handle::Reader, "read") => {
                let chunk = self.types.intern(TyKind::Option(bytes));
                (I::Read, vec![Ty::INT], failing(self, chunk))
            }
            (Handle::Reader, "readText") => (I::ReadText, vec![], failing(self, Ty::STRING)),
            (Handle::Buffer, "writer") => (I::BufferWriter, vec![], self.types.handle(Handle::Writer, Ty::UNIT)),
            (Handle::Buffer, "reader") => (I::BufferReader, vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Buffer, "text") => (I::BufferText, vec![], failing(self, Ty::STRING)),
            (Handle::Buffer, "bytes") => (I::BufferBytes, vec![], bytes),
            (Handle::Buffer, "length") => (I::BufferLength, vec![], Ty::INT),
            (Handle::Buffer, "clear") => (I::BufferClear, vec![], Ty::UNIT),
            (Handle::Buffer | Handle::Reader, "write" | "writeLine" | "writeBytes" | "flush") => {
                let fix =
                    if handle == Handle::Buffer { "write through `buffer.writer()`" } else { "a reader only reads" };
                let message = format!("{} has no method `{}`: {fix}", self.display(receiver.ty), method.name);
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            (Handle::Buffer | Handle::Writer, "readLine" | "read" | "readText") => {
                let fix =
                    if handle == Handle::Buffer { "read through `buffer.reader()`" } else { "a writer only writes" };
                let message = format!("{} has no method `{}`: {fix}", self.display(receiver.ty), method.name);
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            _ => return self.no_method(receiver.ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Io(function)), Vec::new(), values), result, span)
    }

    fn file_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let fs = self.fs.expect("declared with the module");
        let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, fs.error));
        let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
        use hir::FsFn as F;
        let (function, params, result) = match method.name.as_str() {
            // The file becomes a stream of `std.io`, the same object.
            "writer" | "reader" if args.is_empty() => {
                let handle = if method.name == "writer" { Handle::Writer } else { Handle::Reader };
                let ty = self.types.handle(handle, Ty::UNIT);
                return Self::mk(hir::ExprKind::Retype(Box::new(receiver)), ty, span);
            }
            "readLine" => {
                let line = self.types.intern(TyKind::Option(Ty::STRING));
                (F::ReadLine, vec![], failing(self, line))
            }
            "read" => {
                let chunk = self.types.intern(TyKind::Option(bytes));
                (F::Read, vec![Ty::INT], failing(self, chunk))
            }
            "readText" => (F::ReadRest, vec![], failing(self, Ty::STRING)),
            "write" => (F::Write, vec![Ty::STRING], failing(self, Ty::UNIT)),
            "writeBytes" => (F::WriteBytesTo, vec![bytes], failing(self, Ty::UNIT)),
            "flush" => (F::Flush, vec![], failing(self, Ty::UNIT)),
            "close" => (F::Close, vec![], failing(self, Ty::UNIT)),
            _ => return self.no_method(receiver.ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let receiver = if function == F::Close {
            receiver
        } else {
            let (ty, receiver_span) = (receiver.ty, receiver.span);
            let operation = format!("`{}`", method.name);
            let Some(place) = self.loan_place(cx, receiver, &operation, true) else {
                return Self::error_expr(span);
            };
            Self::mk(hir::ExprKind::Loan(place), ty, receiver_span)
        };
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Fs(function)), Vec::new(), values), result, span)
    }

    /// `Ref(value)` (16.10): a new identity holding the value, copied when
    /// it is `Copy` and moved otherwise.
    fn ref_new(
        &mut self,
        cx: &mut FnCx,
        args: &[ast::Arg],
        explicit: Option<Ty>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let wanted = explicit.or_else(|| match expected.map(|e| self.types.kind(e)) {
            Some(&TyKind::Handle(Handle::Ref, value)) => Some(value),
            _ => None,
        });
        let value = self.expr(cx, &arg.value, wanted);
        let value = match wanted {
            Some(wanted) => self.coerce(value, wanted),
            None => value,
        };
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let ty = self.types.handle(Handle::Ref, value.ty);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![value]), ty, span)
    }

    /// `WeakRef(reference)`: a reference to the same object that does not
    /// keep it alive.
    fn weak_new(&mut self, cx: &mut FnCx, args: &[ast::Arg], expected: Option<Ty>, span: Span) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let wanted = match expected.map(|e| self.types.kind(e)) {
            Some(&TyKind::Handle(Handle::Weak, value)) => Some(self.types.handle(Handle::Ref, value)),
            _ => None,
        };
        let reference = self.expr(cx, &arg.value, wanted);
        let value = match *self.types.kind(reference.ty) {
            TyKind::Handle(Handle::Ref, value) => value,
            TyKind::Error => return Self::error_expr(span),
            _ => {
                let message = format!(
                    "`WeakRef` refers to the object of a `Ref`, found {}; make one first: `WeakRef(Ref(value))`",
                    self.display(reference.ty)
                );
                self.diags.error("E1102", reference.span, message);
                return Self::error_expr(span);
            }
        };
        let ty = self.types.handle(Handle::Weak, value);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::WeakNew, vec![reference]), ty, span)
    }

    /// `sync.Mutex(value)` and the other `std.sync` constructors: a new
    /// shared cell holding the value, or a semaphore or barrier.
    fn sync_new(
        &mut self,
        cx: &mut FnCx,
        name: &str,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let expected_state = expected.and_then(|e| match self.types.kind(e) {
            TyKind::Handle(_, state) => Some(*state),
            _ => None,
        });
        let (op, handle) = match name {
            "Semaphore" => (hir::TaskOp::SemaphoreNew, Handle::Semaphore),
            "Barrier" => (hir::TaskOp::BarrierNew, Handle::Barrier),
            _ => (hir::TaskOp::SyncNew, Handle::Mutex),
        };
        if op != hir::TaskOp::SyncNew {
            let count = self.expr(cx, &arg.value, Some(Ty::INT));
            let count = self.coerce(count, Ty::INT);
            let ty = self.types.handle(handle, Ty::UNIT);
            return Self::mk(hir::ExprKind::TaskOp(op, vec![count]), ty, span);
        }
        let value = self.expr(cx, &arg.value, expected_state);
        let ty = value.ty;
        let (handle, value) = match (name, self.types.kind(ty).clone()) {
            (_, TyKind::Error) => return Self::error_expr(span),
            ("Mutex", _) => (Handle::Mutex, value),
            ("RwLock", _) => (Handle::RwLock, value),
            ("Atomic", _) if self.types.is_equatable(ty) => (Handle::Atomic, value),
            ("Array", TyKind::Array(_)) => (Handle::SyncArray, value),
            ("Stack", TyKind::Array(_)) => (Handle::SyncStack, value),
            ("Queue", TyKind::Deque(_)) => (Handle::SyncQueue, value),
            // An array is a deque that starts at its first element.
            ("Queue", TyKind::Array(element)) => {
                let deque = self.types.intern(TyKind::Deque(element));
                (Handle::SyncQueue, Self::mk(hir::ExprKind::Retype(Box::new(value)), deque, span))
            }
            ("Map", TyKind::Map(..)) => (Handle::SyncMap, value),
            ("Set", TyKind::Set(_)) => (Handle::SyncSet, value),
            ("Atomic", _) => {
                let message = format!("`sync.Atomic` compares its value, but {} is not `Equatable`", self.display(ty));
                self.diags.error("E1105", value.span, message);
                return Self::error_expr(span);
            }
            ("Once", _) => {
                self.diags.error("E1102", span, "`sync.Once` starts empty: `sync.Once.new[T]()`");
                return Self::error_expr(span);
            }
            ("Array" | "Stack" | "Queue" | "Map" | "Set", _) => {
                let wanted = match name {
                    "Map" => "a map",
                    "Set" => "a set",
                    _ => "an array",
                };
                let message = format!("`sync.{name}` takes {wanted}, found {}", self.display(ty));
                self.diags.error("E1102", value.span, message);
                return Self::error_expr(span);
            }
            _ => {
                let message = format!("module `sync` has no function `{name}`");
                self.diags.error("E1001", span, message);
                return Self::error_expr(span);
            }
        };
        let state = value.ty;
        let ty = self.types.handle(handle, state);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![value]), ty, span)
    }

    /// `sync.Map["one": 1]` and the other literals of shared collections
    /// (5.11): the literal of the value collection, in a new cell.
    fn sync_literal(
        &mut self,
        cx: &mut FnCx,
        path: &[ast::Ident],
        literal: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if !matches!(self.names.get(&path[0].name), Some(ModuleName::Module(StdModule::Sync)))
            || self.lookup(cx, &path[0].name).is_some()
        {
            let message =
                format!("`{}` is not `std.sync`; import it to write `sync.{}[...]`", path[0].name, path[1].name);
            self.diags.error("E1001", path[0].span, message);
            return Self::error_expr(span);
        }
        let expected = expected.and_then(|e| match self.types.kind(e) {
            TyKind::Handle(_, state) => Some(*state),
            _ => None,
        });
        let kind = path[1].name.as_str();
        // The literal a set, stack or queue holds is written as an array.
        let wanted = match (kind, expected.map(|e| self.types.kind(e).clone())) {
            ("Set" | "Queue", Some(TyKind::Set(t) | TyKind::Deque(t))) => Some(self.types.intern(TyKind::Array(t))),
            (_, Some(_)) => expected,
            _ => None,
        };
        let value = match (kind, &literal.kind) {
            ("Set", ast::ExprKind::Array(items)) => self.set_literal(cx, items, expected, span),
            ("Set", _) => {
                self.diags.error("E1102", literal.span, "a set literal lists values: `sync.Set[a, b]`");
                return Self::error_expr(span);
            }
            _ => self.expr(cx, literal, wanted),
        };
        let value = match (kind, self.types.kind(value.ty).clone()) {
            (_, TyKind::Error) => return Self::error_expr(span),
            ("Map", TyKind::Map(..)) | ("Array" | "Stack", TyKind::Array(_)) | ("Set", TyKind::Set(_)) => value,
            ("Queue", TyKind::Array(element)) => {
                let deque = self.types.intern(TyKind::Deque(element));
                Self::mk(hir::ExprKind::Retype(Box::new(value)), deque, span)
            }
            _ => {
                let message = format!(
                    "`sync.{kind}[...]` lists {}",
                    if kind == "Map" { "entries: `[key: value]`" } else { "values" }
                );
                self.diags.error("E1102", literal.span, message);
                return Self::error_expr(span);
            }
        };
        let handle = match kind {
            "Array" => Handle::SyncArray,
            "Map" => Handle::SyncMap,
            "Set" => Handle::SyncSet,
            "Stack" => Handle::SyncStack,
            _ => Handle::SyncQueue,
        };
        let ty = self.types.handle(handle, value.ty);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![value]), ty, span)
    }

    /// A block that evaluates `args`, takes the lock of the state of
    /// `handle`, and ends with what `body` builds; the lock is released
    /// when the block ends, in any way.
    fn locked(
        &mut self,
        cx: &mut FnCx,
        handle: hir::Expr,
        mode: hir::LockMode,
        args: Vec<hir::Expr>,
        span: Span,
        body: impl FnOnce(&mut Self, &mut FnCx, &Lock) -> (Vec<hir::Stmt>, hir::Expr),
    ) -> hir::Expr {
        let state = self.types.handle_result(handle.ty);
        let mut stmts = Vec::new();
        let receiver = self.hidden_local(cx, handle.ty);
        let read = Self::mk(hir::ExprKind::Local(receiver), handle.ty, span);
        stmts.push(hir::Stmt::Let(receiver, handle));
        let mut locals = Vec::new();
        for arg in args {
            let local = self.hidden_local(cx, arg.ty);
            locals.push((local, arg.ty));
            stmts.push(hir::Stmt::Let(local, arg));
        }
        let guard_ty = self.types.handle(Handle::Guard, state);
        let guard = self.hidden_local(cx, guard_ty);
        let inner = self.hidden_local(cx, state);
        let exclusive = mode != hir::LockMode::Shared;
        cx.mutable[inner.0 as usize] = exclusive;
        cx.params[inner.0 as usize] = Some(if exclusive { ast::Mode::Var } else { ast::Mode::Ref });
        stmts.push(hir::Stmt::LockSync { guard, inner, handle: read, mode, span });
        let lock = Lock { inner, guard, ty: state, args: locals, span };
        let (more, tail) = body(self, cx, &lock);
        stmts.extend(more);
        let ty = tail.ty;
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span)
    }

    /// The function argument of a `std.sync` operation, which receives
    /// `params` passed as `mode`: the argument and its result type, held
    /// in a local that may change when the function changes its captures.
    fn sync_callback(
        &mut self,
        cx: &mut FnCx,
        arg: &ast::Arg,
        params: Vec<Ty>,
        mode: ast::Mode,
    ) -> Option<(hir::Expr, Ty)> {
        let modes = vec![mode; params.len()];
        let expected = self.types.intern(TyKind::Fn(params.clone(), Ty::ERROR, false, modes.clone(), false));
        let value = self.expr(cx, &arg.value, Some(expected));
        if value.ty == Ty::ERROR {
            return None;
        }
        match self.callable(value.ty) {
            Some(callable) if callable.params == params && callable.modes == modes => Some((value, callable.result)),
            _ => {
                let message = match (params.as_slice(), mode) {
                    ([], _) => "expected a function without parameters, such as `() { ... }`".to_string(),
                    ([state], ast::Mode::Var) => format!(
                        "this function receives the state as a `var` loan: write `(value) {{ ... }}`, or `(value: var {}) {{ ... }}`",
                        self.types.display(*state)
                    ),
                    ([state], _) => format!(
                        "this function receives a copy of the state: write `(value) {{ ... }}`, or `(value: {}) {{ ... }}`",
                        self.types.display(*state)
                    ),
                    _ => unreachable!("one parameter at most"),
                };
                self.diags.error("E1102", value.span, message);
                None
            }
        }
    }

    /// The operations of the `std.sync` values (14.4.4). Each locks the
    /// shared state for one step, after evaluating its arguments, and works
    /// on it with the operations of the value it holds.
    #[allow(clippy::too_many_arguments)]
    fn sync_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::LockMode::{Exclusive, Shared, Try};
        let ty = receiver.ty;
        let name = method.name.as_str();
        let mk = Self::mk;
        let arity = |this: &mut Self, count: usize| -> bool {
            if args.len() != count {
                this.arity_error(span, count, args.len());
                false
            } else {
                true
            }
        };
        let value_arg = |this: &mut Self, cx: &mut FnCx, index: usize, ty: Ty| {
            let value = this.expr(cx, &args[index].value, Some(ty));
            this.coerce(value, ty)
        };
        let element = match self.types.kind(state) {
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Option(t) | TyKind::Set(t) => *t,
            _ => state,
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        if handle.collection() {
            return self.shared_method(cx, receiver, handle, state, method, args, span);
        }
        if handle == Handle::Atomic && atomic_scalar(&self.types, state) {
            return self.atomic_method(cx, receiver, state, method, args, span);
        }
        match (handle, name) {
            (Handle::Semaphore | Handle::Permit | Handle::Barrier, _) => {
                let (op, result) = match (handle, name) {
                    (Handle::Semaphore, "acquire") => {
                        (hir::TaskOp::Acquire, self.types.handle(Handle::Permit, Ty::UNIT))
                    }
                    (Handle::Semaphore, "tryAcquire") => {
                        let permit = self.types.handle(Handle::Permit, Ty::UNIT);
                        (hir::TaskOp::TryAcquire, option(self, permit))
                    }
                    (Handle::Permit, "release") => (hir::TaskOp::PermitRelease, Ty::UNIT),
                    (Handle::Barrier, "wait") => (hir::TaskOp::BarrierWait, Ty::BOOL),
                    _ => return self.no_method(ty, method, span),
                };
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                mk(hir::ExprKind::TaskOp(op, vec![receiver]), result, span)
            }
            (Handle::Mutex | Handle::RwLock, "update") | (Handle::Mutex, "tryUpdate") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((change, result)) = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Var) else {
                    return Self::error_expr(span);
                };
                let mode = if name == "tryUpdate" { Try } else { Exclusive };
                let some_result = option(self, result);
                self.locked(cx, receiver, mode, vec![change], span, |_, _, lock| {
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.loan()]), result);
                    if mode == Exclusive {
                        return (Vec::new(), call);
                    }
                    let held = lock.mk(
                        hir::ExprKind::TaskOp(
                            hir::TaskOp::GuardHeld,
                            vec![lock.mk(hir::ExprKind::Local(lock.guard), Ty::UNIT)],
                        ),
                        Ty::BOOL,
                    );
                    let some = lock.mk(hir::ExprKind::Variant(SOME, vec![(0, call)]), some_result);
                    let then = hir::Block { stmts: Vec::new(), tail: Some(Box::new(some)), ty: some_result };
                    let none = lock.mk(hir::ExprKind::Variant(NONE, Vec::new()), some_result);
                    (Vec::new(), lock.mk(hir::ExprKind::If(Box::new(held), then, Some(Box::new(none))), some_result))
                })
            }
            (Handle::Mutex, "updateWhen") => {
                if !arity(self, 2) {
                    return Self::error_expr(span);
                }
                let ready = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Value);
                let change = self.sync_callback(cx, &args[1], vec![state], ast::Mode::Var);
                let (Some((ready, ready_ty)), Some((change, result))) = (ready, change) else {
                    return Self::error_expr(span);
                };
                if ready_ty != Ty::BOOL {
                    let message = format!(
                        "`updateWhen` waits until its first function gives `true`, but it gives {}",
                        self.display(ready_ty)
                    );
                    self.diags.error("E1102", args[0].value.span, message);
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Exclusive, vec![ready, change], span, |_, _, lock| {
                    let check = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.state()]), Ty::BOOL);
                    let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
                    let test = lock.mk(hir::ExprKind::If(Box::new(check), stop, None), Ty::UNIT);
                    let body = hir::Block {
                        stmts: vec![hir::Stmt::Expr(test), hir::Stmt::AwaitChange(lock.guard, lock.span)],
                        tail: None,
                        ty: Ty::UNIT,
                    };
                    let wait = hir::Stmt::Loop { condition: None, body };
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(1)), vec![lock.loan()]), result);
                    (vec![wait], call)
                })
            }
            (Handle::RwLock, "read") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((observe, result)) = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Value) else {
                    return Self::error_expr(span);
                };
                self.locked(cx, receiver, Shared, vec![observe], span, |_, _, lock| {
                    (Vec::new(), lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.state()]), result))
                })
            }
            (Handle::Mutex | Handle::RwLock, "get") | (Handle::Atomic, "load") | (Handle::Once, "get") => {
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Shared, Vec::new(), span, |_, _, lock| (Vec::new(), lock.state()))
            }
            (Handle::Mutex | Handle::RwLock, "set") | (Handle::Atomic, "store") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let value = value_arg(self, cx, 0, state);
                self.locked(cx, receiver, Exclusive, vec![value], span, |_, _, lock| {
                    (vec![lock.assign(Vec::new(), None, lock.arg(0))], lock.mk(hir::ExprKind::Unit, Ty::UNIT))
                })
            }
            (Handle::Atomic, "swap") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let value = value_arg(self, cx, 0, state);
                let old = self.hidden_local(cx, state);
                self.locked(cx, receiver, Exclusive, vec![value], span, |_, _, lock| {
                    let stmts = vec![hir::Stmt::Let(old, lock.state()), lock.assign(Vec::new(), None, lock.arg(0))];
                    (stmts, lock.mk(hir::ExprKind::Local(old), lock.ty))
                })
            }
            (Handle::Atomic, "add") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                if self.types.as_int(state).is_none() {
                    let message = format!("`add` counts in integers, but this atomic holds {}", self.display(state));
                    self.diags.error("E1102", method.span, message);
                    return Self::error_expr(span);
                }
                let delta = value_arg(self, cx, 0, state);
                self.locked(cx, receiver, Exclusive, vec![delta], span, |_, _, lock| {
                    (vec![lock.assign(Vec::new(), Some(BinOp::Add), lock.arg(0))], lock.state())
                })
            }
            (Handle::Atomic, "compareExchange") => {
                if !arity(self, 2) {
                    return Self::error_expr(span);
                }
                let expected = value_arg(self, cx, 0, state);
                let desired = value_arg(self, cx, 1, state);
                let (template, _) = self.compare_exchange.expect("declared with the module");
                let outcome = self.types.instantiate(template, vec![state]);
                self.locked(cx, receiver, Exclusive, vec![expected, desired], span, |_, _, lock| {
                    let same = lock
                        .mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(lock.state()), Box::new(lock.arg(0))), Ty::BOOL);
                    let exchanged = lock.mk(hir::ExprKind::Variant(0, vec![(0, lock.arg(0))]), outcome);
                    let then = hir::Block {
                        stmts: vec![lock.assign(Vec::new(), None, lock.arg(1))],
                        tail: Some(Box::new(exchanged)),
                        ty: outcome,
                    };
                    let mismatch = lock.mk(hir::ExprKind::Variant(1, vec![(0, lock.state())]), outcome);
                    (Vec::new(), lock.mk(hir::ExprKind::If(Box::new(same), then, Some(Box::new(mismatch))), outcome))
                })
            }
            (Handle::Once, "getOrInit") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((init, made)) = self.sync_callback(cx, &args[0], Vec::new(), ast::Mode::Value) else {
                    return Self::error_expr(span);
                };
                let error = match *self.types.kind(made) {
                    _ if made == element => None,
                    TyKind::Result(value, error) if value == element => Some(error),
                    _ => {
                        let message = format!(
                            "`getOrInit` makes the value of this `sync.Once`, of type {}, but this function gives {}",
                            self.display(element),
                            self.display(made)
                        );
                        self.diags.error("E1102", args[0].value.span, message);
                        return Self::error_expr(span);
                    }
                };
                let result = match error {
                    Some(error) => self.types.intern(TyKind::Result(element, error)),
                    None => element,
                };
                let ready = self.hidden_local(cx, element);
                let fresh = self.hidden_local(cx, element);
                let failure = error.map(|e| self.hidden_local(cx, e));
                self.locked(cx, receiver, Exclusive, vec![init], span, |_, _, lock| {
                    let read = |local: LocalId, ty: Ty| lock.mk(hir::ExprKind::Local(local), ty);
                    let wrap = |value: hir::Expr| match error {
                        Some(_) => lock.mk(hir::ExprKind::Variant(OK, vec![(0, value)]), result),
                        None => value,
                    };
                    let bind = |local: LocalId, ty: Ty| hir::Pattern { kind: hir::PatKind::Binding(local), ty, span };
                    let variant = |index: u32, items: Vec<hir::Pattern>, ty: Ty| hir::Pattern {
                        kind: hir::PatKind::Variant(index, items),
                        ty,
                        span,
                    };
                    // Store the value made, then give it.
                    let store = |value: LocalId| {
                        let some = lock.mk(hir::ExprKind::Variant(SOME, vec![(0, read(value, element))]), lock.ty);
                        let block = hir::Block {
                            stmts: vec![lock.assign(Vec::new(), None, some)],
                            tail: Some(Box::new(wrap(read(value, element)))),
                            ty: result,
                        };
                        lock.mk(hir::ExprKind::Block(block), result)
                    };
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), Vec::new()), made);
                    let make = match failure {
                        None => {
                            let stmts = vec![hir::Stmt::Let(fresh, call)];
                            let tail = store(fresh);
                            lock.mk(
                                hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty: result }),
                                result,
                            )
                        }
                        Some(failed) => {
                            let error_ty = error.unwrap();
                            let arms = vec![
                                hir::Arm {
                                    pattern: variant(OK, vec![bind(fresh, element)], made),
                                    guard: None,
                                    body: store(fresh),
                                },
                                hir::Arm {
                                    pattern: variant(ERR, vec![bind(failed, error_ty)], made),
                                    guard: None,
                                    body: lock
                                        .mk(hir::ExprKind::Variant(ERR, vec![(0, read(failed, error_ty))]), result),
                                },
                            ];
                            lock.mk(hir::ExprKind::Match(Box::new(call), arms), result)
                        }
                    };
                    let arms = vec![
                        hir::Arm {
                            pattern: variant(SOME, vec![bind(ready, element)], lock.ty),
                            guard: None,
                            body: wrap(read(ready, element)),
                        },
                        hir::Arm { pattern: variant(NONE, Vec::new(), lock.ty), guard: None, body: make },
                    ];
                    (Vec::new(), lock.mk(hir::ExprKind::Match(Box::new(lock.state()), arms), result))
                })
            }
            (Handle::Once, "isReady") => {
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Shared, Vec::new(), span, |_, _, lock| {
                    let none = lock.mk(hir::ExprKind::Variant(NONE, Vec::new()), lock.ty);
                    (
                        Vec::new(),
                        lock.mk(hir::ExprKind::Binary(BinOp::Ne, Box::new(lock.state()), Box::new(none)), Ty::BOOL),
                    )
                })
            }
            _ => self.no_method(ty, method, span),
        }
    }

    /// The operations of the shared collections (14.4.4.2), each one call
    /// of the runtime.
    #[allow(clippy::too_many_arguments)]
    fn shared_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::SharedOp as S;
        let ty = receiver.ty;
        let name = method.name.as_str();
        let (key, entry, element) = match *self.types.kind(state) {
            TyKind::Map(key, value) => (key, value, key),
            TyKind::Set(key) => (key, Ty::UNIT, key),
            TyKind::Array(t) | TyKind::Deque(t) => (Ty::INT, t, t),
            _ => return Self::error_expr(span),
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        let (op, params, result) = match (handle, name) {
            (_, "length" | "isEmpty") => (S::Length, Vec::new(), Ty::INT),
            (_, "snapshot") => (S::Snapshot, Vec::new(), state),
            (Handle::SyncMap, "get") => (S::Get, vec![key], option(self, entry)),
            (Handle::SyncMap | Handle::SyncSet, "contains") => (S::Contains, vec![key], Ty::BOOL),
            (Handle::SyncMap, "insert") => (S::Insert, vec![key, entry], option(self, entry)),
            (Handle::SyncSet, "insert") => (S::Insert, vec![key], Ty::BOOL),
            (Handle::SyncMap, "remove") => (S::Remove, vec![key], option(self, entry)),
            (Handle::SyncSet, "remove") => (S::Remove, vec![key], Ty::BOOL),
            (Handle::SyncArray | Handle::SyncStack | Handle::SyncQueue, "get") => {
                (S::Get, vec![Ty::INT], option(self, element))
            }
            (Handle::SyncStack | Handle::SyncQueue, "push") => (S::Push, vec![element], Ty::UNIT),
            (Handle::SyncStack, "pop") | (Handle::SyncQueue, "popFront") => (S::Pop, Vec::new(), option(self, element)),
            (Handle::SyncArray, "set") => (S::Set, vec![Ty::INT, element], option(self, element)),
            (Handle::SyncArray, "compareExchange") => (S::Exchange, vec![Ty::INT, element, element], Ty::INT),
            (Handle::SyncMap, "compareExchange") => {
                let maybe = option(self, entry);
                (S::Exchange, vec![key, maybe, maybe], Ty::INT)
            }
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        if op == S::Exchange && !self.types.is_equatable(entry) {
            let message = format!("`compareExchange` compares values, but {} is not `Equatable`", self.display(entry));
            self.diags.error("E1105", span, message);
            return Self::error_expr(span);
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, span);
        match op {
            S::Length if name == "isEmpty" => {
                let length = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), Ty::INT, span);
                Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(int(0))), Ty::BOOL, span)
            }
            S::Set => {
                // The replaced value, or `InvalidIndex` outside the array.
                let replaced = option(self, element);
                let result = self.types.intern(TyKind::Result(element, self.collection_error));
                let old = self.hidden_local(cx, element);
                let set = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), replaced, span);
                let pattern = |kind: hir::PatKind, ty: Ty| hir::Pattern { kind, ty, span };
                let read = Self::mk(hir::ExprKind::Local(old), element, span);
                let invalid = Self::mk(hir::ExprKind::Variant(1, Vec::new()), self.collection_error, span);
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(
                            hir::PatKind::Variant(SOME, vec![pattern(hir::PatKind::Binding(old), element)]),
                            replaced,
                        ),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(OK, vec![(0, read)]), result, span),
                    },
                    hir::Arm {
                        pattern: pattern(hir::PatKind::Variant(NONE, Vec::new()), replaced),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), result, span),
                    },
                ];
                Self::mk(hir::ExprKind::Match(Box::new(set), arms), result, span)
            }
            S::Exchange => {
                // The runtime writes the value it saw into `seen` and gives a
                // code; see `tondo_rt_shared_map_exchange` and the array's.
                let seen = self.hidden_local(cx, entry);
                let zero = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), entry, span);
                values.push(Self::mk(hir::ExprKind::Loan(hir::Place { local: seen, path: Vec::new() }), entry, span));
                let code = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), Ty::INT, span);
                let (template, _) = self.compare_exchange.expect("declared with the module");
                let read = Self::mk(hir::ExprKind::Local(seen), entry, span);
                let pattern = |value: Option<i128>| hir::Pattern {
                    kind: value.map_or(hir::PatKind::Wildcard, |v| hir::PatKind::Literal(hir::Literal::Int(v))),
                    ty: Ty::INT,
                    span,
                };
                let (arms, result) = if handle == Handle::SyncMap {
                    let maybe = option(self, entry);
                    let outcome = self.types.instantiate(template, vec![maybe]);
                    let value = |present: bool| match present {
                        true => Self::mk(hir::ExprKind::Variant(SOME, vec![(0, read.clone())]), maybe, span),
                        false => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), maybe, span),
                    };
                    let arm = |code: Option<i128>, variant: u32, present: bool| hir::Arm {
                        pattern: pattern(code),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(variant, vec![(0, value(present))]), outcome, span),
                    };
                    (
                        vec![arm(Some(0), 1, false), arm(Some(1), 0, false), arm(Some(2), 1, true), arm(None, 0, true)],
                        outcome,
                    )
                } else {
                    let outcome = self.types.instantiate(template, vec![element]);
                    let result = self.types.intern(TyKind::Result(outcome, self.collection_error));
                    let ok = |variant: u32| {
                        let outcome = Self::mk(hir::ExprKind::Variant(variant, vec![(0, read.clone())]), outcome, span);
                        Self::mk(hir::ExprKind::Variant(OK, vec![(0, outcome)]), result, span)
                    };
                    let invalid = Self::mk(hir::ExprKind::Variant(1, Vec::new()), self.collection_error, span);
                    let arms = vec![
                        hir::Arm {
                            pattern: pattern(Some(0)),
                            guard: None,
                            body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), result, span),
                        },
                        hir::Arm { pattern: pattern(Some(1)), guard: None, body: ok(0) },
                        hir::Arm { pattern: pattern(None), guard: None, body: ok(1) },
                    ];
                    (arms, result)
                };
                let decide = Self::mk(hir::ExprKind::Match(Box::new(code), arms), result, span);
                let block =
                    hir::Block { stmts: vec![hir::Stmt::Let(seen, zero)], tail: Some(Box::new(decide)), ty: result };
                Self::mk(hir::ExprKind::Block(block), result, span)
            }
            _ => Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), result, span),
        }
    }

    /// The operations of a `sync.Atomic` of an integer, `Bool`, `Char` or
    /// `Byte`, done with atomic instructions.
    fn atomic_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::AtomicOp as A;
        let ty = receiver.ty;
        let (op, count, result) = match method.name.as_str() {
            "load" => (A::Load, 0, state),
            "store" => (A::Store, 1, Ty::UNIT),
            "swap" => (A::Swap, 1, state),
            "add" => (A::Add, 1, state),
            "compareExchange" => (A::Exchange, 2, state),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != count {
            return self.arity_error(span, count, args.len());
        }
        if op == A::Add && self.types.as_int(state).is_none() {
            let message = format!("`add` counts in integers, but this atomic holds {}", self.display(state));
            self.diags.error("E1102", method.span, message);
            return Self::error_expr(span);
        }
        let mut values = vec![receiver];
        for arg in args {
            let value = self.expr(cx, &arg.value, Some(state));
            values.push(self.coerce(value, state));
        }
        if op != A::Exchange {
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Atomic(op), values), result, span);
        }
        // `Exchanged(seen)` when the value seen is the one expected.
        let expected = self.hidden_local(cx, state);
        let seen = self.hidden_local(cx, state);
        let read = |local: LocalId| Self::mk(hir::ExprKind::Local(local), state, span);
        let given = values.remove(1);
        values.insert(1, read(expected));
        let exchange = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Atomic(op), values), state, span);
        let (template, _) = self.compare_exchange.expect("declared with the module");
        let outcome = self.types.instantiate(template, vec![state]);
        let same =
            Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(read(seen)), Box::new(read(expected))), Ty::BOOL, span);
        let variant = |index: u32| Self::mk(hir::ExprKind::Variant(index, vec![(0, read(seen))]), outcome, span);
        let then = hir::Block { stmts: Vec::new(), tail: Some(Box::new(variant(0))), ty: outcome };
        let decide = Self::mk(hir::ExprKind::If(Box::new(same), then, Some(Box::new(variant(1)))), outcome, span);
        let stmts = vec![hir::Stmt::Let(expected, given), hir::Stmt::Let(seen, exchange)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(decide)), ty: outcome }), outcome, span)
    }

    /// `channel[T]()` or `channel[T](capacity)` (11.13): the two ends of a
    /// new channel, unbounded without a capacity.
    fn channel(&mut self, cx: &mut FnCx, type_args: &[ast::TypeExpr], args: &[ast::Arg], span: Span) -> hir::Expr {
        let [value] = type_args else {
            self.diags.error("E1104", span, "`channel` takes one type argument, the type of its values");
            return Self::error_expr(span);
        };
        let value = self.resolve_type(value);
        if let Some(part) = self.not_send(value) {
            let what = match part == value {
                true => format!("{} is not `Send`", self.display(value)),
                false => format!("{} holds {}, which is not `Send`", self.display(value), self.display(part)),
            };
            let message = format!("a channel carries its values between tasks, but {what}");
            self.diags.error("E1605", span, message);
        }
        let capacity = match args {
            [] => None,
            [arg] if arg.name.as_ref().is_none_or(|n| n.name == "capacity") => {
                let capacity = self.expr(cx, &arg.value, Some(Ty::INT));
                Some(self.coerce(capacity, Ty::INT))
            }
            [arg] => {
                let message = "`channel` takes its capacity, as in `channel[T](16)` or `channel[T](capacity: 16)`";
                self.diags.error("E1102", arg.span, message);
                return Self::error_expr(span);
            }
            _ => return self.arity_error(span, 1, args.len()),
        };
        let sender = self.types.handle(Handle::Sender, value);
        let receiver = self.types.handle(Handle::Receiver, value);
        let ty = self.types.intern(TyKind::Tuple(vec![sender, receiver]));
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Channel, capacity.into_iter().collect()), ty, span)
    }

    /// The methods of `Join`, `Group`, `Sender` and `Receiver` (11.11 to
    /// 11.13); `result` is the type of their results or values.
    #[allow(clippy::too_many_arguments)]
    fn handle_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        result: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::TaskOp as T;
        let ty = receiver.ty;
        let op = match (handle, method.name.as_str()) {
            (Handle::Join, "wait") => T::Wait,
            (Handle::Group, "add") => T::GroupAdd,
            (Handle::Group, "next") => T::GroupNext,
            (Handle::Group, "length" | "isEmpty") => T::GroupLength,
            (Handle::Group, "wait") => T::GroupWait,
            (Handle::Join, "cancel") => T::Cancel,
            (Handle::Join, "detach") => T::Detach,
            (Handle::Group, "cancel") => T::GroupCancel,
            (Handle::Sender, "send") => T::Send,
            (Handle::Sender, "close") => T::Close,
            (Handle::Sender, "copy") => T::CopySender,
            (Handle::Sender | Handle::Receiver, "isClosed") => T::IsClosed,
            (Handle::Receiver, "receive") => T::Receive,
            (Handle::Weak, "get") => T::WeakGet,
            _ => return self.no_method(ty, method, span),
        };
        let timeout = matches!(args, [arg] if arg.name.as_ref().is_some_and(|n| n.name == "timeout"));
        let op = match op {
            T::Wait if timeout => T::WaitTimeout,
            T::Receive if timeout => T::ReceiveTimeout,
            op => op,
        };
        let params = match op {
            T::GroupAdd => vec![self.types.handle(Handle::Join, result)],
            T::Send => vec![result],
            T::ReceiveTimeout | T::WaitTimeout => match self.duration {
                Some(duration) => vec![duration],
                None => {
                    let message =
                        "a timeout is a `time.Duration`: add `import std.time` and write `time.milliseconds(n)`";
                    self.diags.error("E1102", args[0].span, message);
                    return Self::error_expr(span);
                }
            },
            _ => Vec::new(),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let receiver = match op {
            T::GroupAdd | T::GroupNext => {
                let operation = format!("`{}`", method.name);
                let receiver_span = receiver.span;
                let Some(place) = self.loan_place(cx, receiver, &operation, true) else {
                    return Self::error_expr(span);
                };
                Self::mk(hir::ExprKind::Loan(place), ty, receiver_span)
            }
            _ => receiver,
        };
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let result = match op {
            T::Wait => result,
            T::GroupAdd | T::Send | T::Close | T::Cancel | T::Detach | T::GroupCancel => Ty::UNIT,
            T::GroupNext | T::Receive | T::ReceiveTimeout | T::WaitTimeout => self.types.intern(TyKind::Option(result)),
            T::CopySender => ty,
            T::IsClosed => Ty::BOOL,
            T::WeakGet => {
                let reference = self.types.handle(Handle::Ref, result);
                self.types.intern(TyKind::Option(reference))
            }
            T::GroupWait => match *self.types.kind(result) {
                TyKind::Result(value, error) => {
                    let values = self.types.intern(TyKind::Array(value));
                    self.types.intern(TyKind::Result(values, error))
                }
                _ => self.types.intern(TyKind::Array(result)),
            },
            _ => Ty::INT,
        };
        let value = Self::mk(hir::ExprKind::TaskOp(op, values), result, span);
        if method.name == "isEmpty" {
            let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
            return Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(value), Box::new(zero)), Ty::BOOL, span);
        }
        value
    }

    /// A local that no name refers to, such as the value of `for _ in ...`.
    fn hidden_local(&mut self, cx: &mut FnCx, ty: Ty) -> LocalId {
        cx.locals.push(hir::Local { name: "_".into(), ty, span: Span::default() });
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
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        }
        let id = LocalId(cx.locals.len() as u32);
        let kind = if key.is_some() { crate::index::DefKind::Parameter } else { crate::index::DefKind::Local };
        if let Some(def) = self.index_define(key, kind, name, name.span, Some(ty)) {
            cx.local_defs.insert(id.0, def);
        }
        cx.locals.push(hir::Local { name: name.name.clone(), ty, span: name.span });
        cx.mutable.push(mutable);
        cx.params.push(None);
        if !discard {
            cx.scopes.last_mut().unwrap().push((name.name.clone(), id));
            cx.named.push(id);
        }
        id
    }

    /// Warns of the bindings and parameters a body never reads (`W1002`,
    /// `W1003`); `self` and names that start with `_` are exempt.
    fn unused(&mut self, cx: &FnCx, params: &[LocalId]) {
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
            } else {
                let message = format!(
                    "`{name}` is never used: write `_` instead, or `_{name}` to hold its value until the block ends"
                );
                self.diags.warning("W1002", *span, message);
            }
        }
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self, cx: &mut FnCx, b: &ast::Block, expected: Option<Ty>) -> hir::Block {
        let body = std::mem::take(&mut cx.body);
        cx.scopes.push(Vec::new());
        let withs = cx.withs.len();
        let outer = std::mem::replace(&mut cx.diverged, false);
        let stmts = self.statements(cx, &b.stmts);
        let tail = b.tail.as_ref().map(|t| {
            if cx.diverged {
                self.diags.warning("W1006", t.span, "unreachable expression");
            }
            let value = self.expr(cx, t, expected);
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
        cx.scopes.pop();
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
                // The value of a block, `if` or `match` statement is dropped.
                let expected = matches!(
                    value.kind,
                    ast::ExprKind::Block(_) | ast::ExprKind::If { .. } | ast::ExprKind::Match { .. }
                )
                .then_some(Ty::UNIT);
                let value = self.expr(cx, value, expected);
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
            ast::StmtKind::Return(_) if cx.setup => {
                let message = "the setup of a suite cannot `return`, or its tests would never run; use `fail` or `testing.skip(reason)`";
                self.diags.error("E1205", stmt.span, message);
                None
            }
            ast::StmtKind::Return(value) => {
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
                let error = self.error_channel(cx, value.ty, stmt.span, "E1302")?;
                let value = self.coerce(value, error);
                cx.diverged = true;
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
                if self.types.as_int(ty).is_none() && ty != Ty::ERROR {
                    let message =
                        format!("`{}=` requires an integer target, found {}", op.unwrap().as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, None);
                self.require_int(&value);
                value
            }
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem)) => {
                self.compound_value(cx, op, ty, value, span)
            }
            Some(op) => {
                let bitwise = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error);
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
                    cx.scopes.push(Vec::new());
                    let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
                    let pattern = self.pattern(cx, pattern, element, context);
                    if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
                        let message = format!("this pattern can fail, for example on `{missing}`");
                        self.diags.error("E1201", pattern.span, message);
                    }
                    cx.loops.push(false);
                    let body = self.block(cx, body, Some(Ty::UNIT));
                    cx.loops.pop();
                    cx.scopes.pop();
                    return Some(hir::Stmt::ForEach { pattern, source, body });
                };
                let (start, end) = self.operands(cx, start, end, None, false);
                // Characters count through the prelude's `Range`.
                if start.ty == Ty::CHAR {
                    let range = self.range_value(start, end, *op == BinOp::RangeInclusive, source.span);
                    return Some(self.for_cursor(cx, pattern, range, Ty::CHAR, body));
                }
                self.require_int(&start);
                cx.scopes.push(Vec::new());
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
                cx.scopes.pop();
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

        cx.scopes.push(Vec::new());
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
        cx.scopes.pop();
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
            if number >= int.min() && number <= int.max() {
                return Self::mk(hir::ExprKind::Int(number), expected, value.span);
            }
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
                    && matches!(self.types.kind(value.ty), TyKind::Fn(..) | TyKind::Adt(_)))
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
        if self.types.as_int(value.ty).is_none() && ![Ty::NEVER, Ty::ERROR].contains(&value.ty) {
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
            ast::ExprKind::Int { value, suffix } => self.int_literal(*value as i128, *suffix, expected, span),
            ast::ExprKind::Bool(b) => Self::mk(hir::ExprKind::Bool(*b), Ty::BOOL, span),
            ast::ExprKind::Unit => Self::mk(hir::ExprKind::Unit, Ty::UNIT, span),
            ast::ExprKind::Str(segments) => self.string(cx, segments, span),
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
            ast::ExprKind::Within(limit, block) => self.within(cx, limit, block, span),
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
            self.report_missing(&fields, &seen, span);
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        let [name] = path else {
            return self.unsupported_expr(span, "qualified record literals");
        };
        self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        let ty = match self.type_names.get(&name.name) {
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
        if self.types.c_layout(ty).is_some_and(|l| l.opaque) {
            let message =
                format!("{} is opaque: only C makes one, and Tondo holds a `Pointer` to it", self.display(ty));
            self.diags.error("E1703", name.span, message);
            return Self::error_expr(span);
        }
        let Some(fields) = self.record_fields(ty, name.span) else {
            return Self::error_expr(span);
        };
        if self.hidden_fields(ty).is_some() {
            let message = format!(
                "{} has private fields, so only its module can build it with a literal; call a function of that module",
                self.display(ty)
            );
            self.diags.error("E1502", name.span, message);
        }
        self.index_fields(Some(name.name.clone()), inits.iter().map(|i| (&i.name, i.value.is_none())));
        let (ty, values, seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
        if self.types.c_layout(ty).is_some_and(|l| l.union) {
            return self.union_literal(cx, ty, values, span);
        }
        self.report_missing(&fields, &seen, span);
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

    fn report_missing(&mut self, fields: &[(String, Ty)], seen: &[bool], span: Span) {
        let missing: Vec<&str> = fields.iter().zip(seen).filter(|(_, s)| !**s).map(|((n, _), _)| n.as_str()).collect();
        if !missing.is_empty() {
            let typed: Vec<String> = fields
                .iter()
                .zip(seen)
                .filter(|(_, s)| !**s)
                .map(|((n, t), _)| format!("{n}: {}", self.types.display(*t)))
                .collect();
            let what = if missing.len() == 1 { "the field" } else { "the fields" };
            let message = format!("this value is missing {what} {}", listed(&typed));
            self.diags.error("E1102", span, message);
        }
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
    /// context. The initial values are constants; the fields are `Send` and
    /// `Copy`, since tasks share them and reading one copies it, and those
    /// shown as text have `Display`.
    fn context_makers(&mut self) -> Vec<(hir::Function, Option<hir::Function>, Option<hir::Function>)> {
        let mut makers = Vec::new();
        for (ty, decl) in self.contexts.clone() {
            let module = std::mem::replace(&mut self.module, decl.module);
            let ast::TypeBody::Record(declared) = &decl.body else { unreachable!("a context is a record") };
            let fields = self.record_fields(ty, decl.span).unwrap_or_default();
            let reference = self.types.handle(Handle::Ref, ty);
            let mut cx = self.new_cx(reference);
            let mut values = Vec::new();
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
                for annotation in &field.annotations {
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
                let value = self.expr(&mut cx, default, Some(field_ty));
                let mut value = self.coerce(value, field_ty);
                if !self.types.has_error(value.ty) && !constant(&mut value) {
                    let message = format!(
                        "the initial value of {what} is a constant: build it from literals, constants, their operators and constructors; a value made as the program runs is a `{name}: T? = none`, set with `with` in `main`"
                    );
                    self.diags.error("E1616", default.span, message);
                }
                values.push((index as u32, value));
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
                locals: cx.locals,
                result: reference,
                body,
                moves: HashSet::new(),
                test: false,
                span: Span::default(),
            };
            self.ownership(&mut maker, &cx.params);
            let log = self.context_shown(decl, "log");
            let label = self.context_shown(decl, "label");
            self.module = module;
            makers.push((maker, log, label));
        }
        makers
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
                    ast::StrSegment::Expr(value),
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
            parts.push(ast::StrSegment::Expr(part));
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
            None if ty == Ty::ERROR => Some(None),
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
        for arm in arms {
            cx.scopes.push(Vec::new());
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
            let body = self.arm_body(cx, &arm.body, &mut result);
            cx.diverged = outer;
            cx.scopes.pop();
            out.push(hir::Arm { pattern, guard, body });
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
            cx.scopes.push(Vec::new());
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
            cx.scopes.pop();
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
    /// in the current scope.
    fn pattern(&mut self, cx: &mut FnCx, p: &ast::Pattern, ty: Ty, context: PatternContext) -> hir::Pattern {
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
            ast::PatternKind::Constructor { path, args } if path.len() == 1 => {
                let name = &path[0].name;
                match self.type_named(name) {
                    Some(member) => {
                        let [inner] = args.as_slice() else {
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
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
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
        self.update(cx, base, updates, false, span)
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
        match self.types.kind(value.ty).clone() {
            TyKind::Option(payload) => {
                if !matches!(self.types.kind(cx.result), TyKind::Option(_)) {
                    let message = format!(
                        "`?` on an option needs the function to return an option, but it returns {}",
                        self.display(cx.result)
                    );
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
                    if !missing.is_empty() {
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

    /// `Array.new[T]()` and `Array.withCapacity[T](capacity)`.
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
                "Pointer" | "UInt64" if self.lookup(cx, first).is_none() => None,
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
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
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
        if let [module, kind, "new"] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::Sync)) = self.names.get(*module)
        {
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let ty = self.sync_type(&segments[1], &types, span);
            let state = match self.types.kind(ty) {
                TyKind::Handle(_, state) => *state,
                _ => return Self::error_expr(span),
            };
            let empty = match *self.types.kind(state) {
                TyKind::Array(_) | TyKind::Deque(_) => hir::ExprKind::ArrayOp(hir::ArrayOp::New, Vec::new()),
                TyKind::Map(..) | TyKind::Set(_) => hir::ExprKind::MapOp(hir::MapOp::New, Vec::new()),
                TyKind::Option(_) => hir::ExprKind::Variant(NONE, Vec::new()),
                _ => {
                    let message = format!("`sync.{kind}` takes its initial value: `sync.{kind}(value)`");
                    self.diags.error("E1102", span, message);
                    return Self::error_expr(span);
                }
            };
            let empty = Self::mk(empty, state, span);
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![empty]), ty, span);
        }
        if let ["Group", "new"] = names.as_slice() {
            let [result] = type_args else {
                self.diags.error("E1104", span, "`Group.new` takes one type argument, the tasks' result");
                return Self::error_expr(span);
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let result = self.resolve_type(result);
            let ty = self.types.handle(Handle::Group, result);
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::GroupNew, Vec::new()), ty, span);
        }
        if let ["Map" | "Set", "new"] = names.as_slice() {
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
            ["Array" | "Deque", "new"] => (hir::ArrayOp::New, Vec::new()),
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
        let ty = value.ty;
        if self.is_http_route(ty, method, args) {
            return self.http_route(cx, value, method, args, expected, span);
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
        // `DateTime.sinceEpoch()`, written in Tondo (14.3.4).
        if Some(ty) == self.datetime
            && method.name == "sinceEpoch"
            && let Some(&ModuleName::Fn(id)) = self.names.get("datetime.sinceEpoch")
        {
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let call = hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![value]);
            return Self::mk(call, self.duration.unwrap(), span);
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
        if matches!(
            method.name.as_str(),
            "push" | "pushFront" | "pop" | "popFront" | "insert" | "removeAt" | "remove" | "clear"
        ) && matches!(self.types.kind(ty), TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_))
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
            if handle.locked() || matches!(handle, Handle::Semaphore | Handle::Permit | Handle::Barrier) {
                return self.sync_method(cx, value, handle, state, method, args, span);
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
            "trim" => (S::Trim, vec![], Ty::STRING),
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
        let Some(Callable { params, result, variadic, mutates, modes, is_unsafe, .. }) = self.callable(callee.ty)
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

    /// A call of a C function (FFI), inside `unsafe`: its arguments as an
    /// ordinary call takes them, and after them those of C's `...`.
    fn extern_call(
        &mut self,
        cx: &mut FnCx,
        id: u32,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let declared = self.externs[id as usize].clone();
        let fixed = declared.params.len();
        let (args, extra) =
            if declared.variadic && args.len() > fixed { args.split_at(fixed) } else { (args, &[][..]) };
        if declared.variadic
            && let Some(arg) = args.iter().chain(extra).find(|a| a.name.is_some())
        {
            let message = "a C function with `...` takes its arguments by position: remove the name";
            self.diags.error("E1102", arg.span, message);
            return Self::error_expr(span);
        }
        let signature = Signature {
            modes: vec![ast::Mode::Value; fixed],
            names: self.extern_names[id as usize].clone(),
            is_unsafe: true,
            foreign: true,
            name: declared.name.clone(),
            ..Signature::simple(declared.params, declared.result)
        };
        let target = CallTarget::new(Callee::Extern(id), signature, expected);
        let mut call = self.call_signature(cx, target, None, args, span);
        let values: Vec<hir::Expr> = extra.iter().map(|arg| self.c_variadic_argument(cx, arg)).collect();
        if let hir::ExprKind::Call(_, _, args) = &mut call.kind {
            args.extend(values);
        }
        call
    }

    /// An argument of C's `...`: a number, `Bool`, `Byte`, pointer or
    /// text, which passes as a `ffi.CString`.
    fn c_variadic_argument(&mut self, cx: &mut FnCx, arg: &ast::Arg) -> hir::Expr {
        let value = self.expr(cx, &arg.value, None);
        if arg.spread || arg.mode != ast::Mode::Value {
            self.diags.error("E1102", arg.span, "C's `...` takes values: remove the keyword or the spread");
            return value;
        }
        let repr = self.types.repr(value.ty);
        if repr == Ty::STRING {
            return value;
        }
        let problem = match self.types.kind(repr) {
            TyKind::Adt(id) if self.types.foreign.contains_key(id) => {
                Some("C's `...` takes no struct by value; pass a `Pointer` to it".to_string())
            }
            _ => self.c_type_error(value.ty),
        };
        if let Some(reason) = problem {
            let message = format!("{} cannot cross to C: {reason}", self.display(value.ty));
            self.diags.error("E1703", value.span, message);
        }
        value
    }

    /// A call through an `extern fn(...)` value, inside `unsafe`.
    fn c_value_call(
        &mut self,
        cx: &mut FnCx,
        callee: hir::Expr,
        (params, result, variadic): (Vec<Ty>, Ty, bool),
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if variadic {
            let message =
                "call a C function with `...` by its `extern fn` declaration, which passes its extra arguments";
            self.diags.error("E1704", span, message);
            return Self::error_expr(span);
        }
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            self.diags.error("E1102", arg.span, "a call through a function value takes positional arguments");
            return Self::error_expr(span);
        }
        let signature = Signature {
            names: (0..params.len()).map(|i| format!("argument{}", i + 1)).collect(),
            is_unsafe: true,
            foreign: true,
            name: "this C function".into(),
            ..Signature::simple(params, result)
        };
        let target = CallTarget::new(Callee::Fn(FnId(u32::MAX)), signature, expected);
        let call = self.call_signature(cx, target, None, args, span);
        let hir::ExprKind::Call(_, _, values) = call.kind else { return call };
        Self::mk(hir::ExprKind::CallValue(Box::new(callee), values), call.ty, span)
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
        cx.scopes.push(Vec::new());
        let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
        let pattern = self.pattern(cx, pattern, element, context);
        if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
            let message = format!("this pattern can fail, for example on `{missing}`");
            self.diags.error("E1201", pattern.span, message);
        }
        cx.loops.push(false);
        let body = self.block(cx, body, Some(Ty::UNIT));
        cx.loops.pop();
        cx.scopes.pop();
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
        }
        self.diags.error("E1001", method.span, message);
        Self::error_expr(span)
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
            declared.chain(crate::api::builtin_methods(&owner).into_iter().map(String::from)).collect();
        closest(name, names.iter().map(String::as_str)).map(String::from)
    }

    /// A trait with a method named `name` that `ty` implements, if any.
    fn implemented_trait_with(&mut self, ty: Ty, name: &str) -> Option<String> {
        let candidates: Vec<TraitId> = (0..self.traits.len() as u32)
            .map(TraitId)
            .filter(|t| self.traits[t.0 as usize].params.is_empty())
            .filter(|t| self.traits[t.0 as usize].methods.iter().any(|m| m.name == name))
            .collect();
        candidates.into_iter().find(|&t| self.satisfies(ty, t, &[])).map(|t| self.traits[t.0 as usize].name.clone())
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
            (TyKind::String, "strip" | "trimmed") => "use `trim()`, `trimStart()` or `trimEnd()`",
            (TyKind::String, "indexOf" | "index" | "firstIndex") => "use `find(text)`, which gives `Int?`",
            (TyKind::String, "includes" | "has") => "use `contains(text)`",
            (TyKind::String, "starts_with" | "hasPrefix") => "use `startsWith(prefix)`",
            (TyKind::String, "ends_with" | "hasSuffix") => "use `endsWith(suffix)`",
            (TyKind::String, "format") => "interpolate the values: `\"{first} and {second}\"`",
            (TyKind::String, "equalsIgnoreCase") => "compare `a.toLower() == b.toLower()`",
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
                "use `removeAt(0)`, or a `Deque` for a queue: `Deque.new[T]()` has `popFront()`"
            }
            (TyKind::Array(_), "unshift" | "prepend" | "pushFront") => {
                "use `insert(0, value)`, or a `Deque` for a queue: `Deque.new[T]()` has `pushFront(value)`"
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

    fn int_literal(&mut self, value: i128, suffix: Option<&str>, expected: Option<Ty>, span: Span) -> hir::Expr {
        let int = match suffix {
            Some(suffix) => IntTy::from_suffix(suffix),
            None => self.expected_number(expected, false).and_then(|t| self.types.as_int(t)).unwrap_or(IntTy::I64),
        };
        if value < int.min() || value > int.max() {
            let message = format!("literal `{value}` does not fit in `{}`", int.name());
            self.diags.error("E1102", span, message);
        }
        Self::mk(hir::ExprKind::Int(value), Ty::int(int), span)
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
        let shown = |this: &mut Self, ty: Ty| !this.types.has_error(ty) && this.satisfies(ty, DISPLAY, &[]);
        if !comparison || !shown(self, left.ty) || !shown(self, right.ty) {
            return (Vec::new(), condition, None);
        }
        let hir::ExprKind::Binary(op, left, right) = condition.kind else { unreachable!() };
        let (span, ty) = (condition.span, condition.ty);
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
        let condition = Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span);
        let detail = Self::mk(hir::ExprKind::Interpolate(segments), Ty::STRING, span);
        (sides, condition, Some(Box::new(detail)))
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

    fn string(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], span: Span) -> hir::Expr {
        if let [ast::StrSegment::Text(text)] = segments {
            return Self::mk(hir::ExprKind::Str(text.clone()), Ty::STRING, span);
        }
        let mut out = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(text) => out.push(hir::Segment::Text(text.clone())),
                ast::StrSegment::Expr(e) => {
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
                        } else if matches!(self.types.kind(value.ty), TyKind::Adt(_)) {
                            let name = self.types.display(value.ty);
                            message.push_str(&format!(
                                ": add `impl Display for {name} {{ fn display(self): String {{ ... }} }}`"
                            ));
                        }
                        self.diags.error("E1105", value.span, message);
                        out.push(hir::Segment::Value(value));
                        continue;
                    }
                    out.push(hir::Segment::Value(self.displayed(value)));
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
        // The constants of the intrinsic numeric types: `Int.max`, `Float.pi`.
        if let [type_name, constant] = segments
            && !self.names.contains_key(&type_name.name)
            && let Some(ty) = self.intrinsic_owner(&type_name.name)
            && let Some(kind) = self.intrinsic_constant(ty, &constant.name)
        {
            return Self::mk(kind, ty, span);
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
            && !self.names.contains_key(&type_name.name)
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
            _ => {
                // The bindings in sight and the module's names.
                let mut names: Vec<&str> = self.names.keys().map(String::as_str).collect();
                let mut scope = Some(cx);
                while let Some(found) = scope {
                    names.extend(found.scopes.iter().flatten().map(|(n, _)| n.as_str()));
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
        if let Some((safe, replacement)) = replacement {
            self.diags.fix(format!("write `{replacement}`"), safe, vec![(span, replacement)]);
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
        if let ["Array", "new" | "withCapacity"] | ["Deque" | "Group", "new"] = names.as_slice() {
            let message = format!("`{0}.{1}` needs its element type: `{0}.{1}[T](...)`", names[0], names[1]);
            self.diags.error("E1101", span, message);
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
        {
            self.index_refer(&segments[0], crate::index::Key::Trait(trait_id.0));
            return self.trait_call(cx, trait_id, &segments[1], Vec::new(), args, expected, span);
        }
        // `Type.operation(...)`: an associated operation, or a method with
        // its receiver written first (11.5). Operations and variants never
        // share a name.
        if let [type_name, operation] = names.as_slice()
            && !self.names.contains_key(*type_name)
            && let Some(owner) = self.type_named(type_name)
        {
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
                        let code = if *member == "failNow" { hir::FAIL_NOW } else { hir::SKIP };
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
                    | StdModule::InternalCbor
                    | StdModule::InternalNet
                    | StdModule::InternalFuzz
                    | StdModule::InternalPanic
                    | StdModule::InternalTime
                    | StdModule::InternalContext),
                )),
            ) => {
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
                    StdModule::InternalCbor => "cbor",
                    StdModule::InternalNet => "net",
                    StdModule::InternalFuzz => "fuzz",
                    StdModule::InternalPanic => "panic",
                    StdModule::InternalTime => "time",
                    StdModule::InternalContext => "context",
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
                    "metadata" => (F::Metadata, vec![("path", s)], failing(self, fs.metadata)),
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
                    // ISO 8601, written in Tondo (14.3.4).
                    // The deadline of the task (11.17), written in Tondo.
                    "deadline" | "remaining"
                        if let Some(&ModuleName::Fn(id)) = self.names.get(&format!("datetime.{member}")) =>
                    {
                        let result = if *member == "deadline" { instant } else { duration };
                        let result = self.types.intern(TyKind::Option(result));
                        let signature = Signature::simple(Vec::new(), result);
                        let target = CallTarget::new(Callee::Fn(id), signature, expected);
                        return self.call_signature(cx, target, None, args, span);
                    }
                    "parseDateTime" if let Some(&ModuleName::Fn(id)) = self.names.get("datetime.parse") => {
                        let result = self.types.intern(TyKind::Result(datetime, self.time_error.unwrap()));
                        let signature = Signature::simple(vec![Ty::STRING], result);
                        let target = CallTarget::new(Callee::Fn(id), signature, expected);
                        return self.call_signature(cx, target, None, args, span);
                    }
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
    fn call_signature(
        &mut self,
        cx: &mut FnCx,
        target: CallTarget,
        receiver: Option<hir::Expr>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let CallTarget { callee, signature, explicit, prebound, expected } = target;
        if let Callee::Fn(id) = callee {
            self.repeatable(id, span);
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
                let value = self.argument_value(
                    cx,
                    arg,
                    expected,
                    mode,
                    parameter_of(index) < first + usize::from(signature.receiver.is_some()) && first == 0,
                );
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
                let message = format!(
                    "{} does not implement {missing}, which `{}` requires of `{}`{}",
                    self.display(actual),
                    signature.name,
                    self.types.display(param),
                    if automatic {
                        "; the compiler implements it for records, enums and newtypes whose parts all implement it"
                    } else {
                        ""
                    }
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
                let message = format!(
                    "{} does not implement `{trait_name}`: add `impl {trait_name} for {}`",
                    self.display(self_ty),
                    self.types.display(self_ty)
                );
                self.diags.error("E1105", span, message);
                return Self::error_expr(span);
            }
        }
        if matches!(signature.name.as_str(), "reflect.get" | "reflect.set" | "reflect.call" | "reflect.callMut") {
            self.check_reflection(&signature.name, &type_args, args);
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
                    let literal = self.int_literal(-(value as i128), suffix, expected, span);
                    if self.types.as_int(literal.ty).is_some_and(|i| !i.signed()) {
                        let message = format!("negation is not defined for {}", self.display(literal.ty));
                        self.diags.error("E1102", span, message);
                    }
                    return literal;
                }
                if let ast::ExprKind::Float { text, suffix } = &operand.kind {
                    return self.float_literal(text, *suffix, true, expected, span);
                }
                let value = self.expr(cx, operand, expected);
                let negatable = match self.types.kind(value.ty) {
                    TyKind::Int(int) => int.signed(),
                    TyKind::Float(_) | TyKind::Error | TyKind::Never => true,
                    _ => false,
                };
                if !negatable {
                    let message = format!("negation is not defined for {}", self.display(value.ty));
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
                if value.ty != Ty::BYTE {
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
        // so, without another for the right operand.
        if !matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Float(_) | TyKind::Error) {
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
        let numeric = match self.types.kind(number) {
            TyKind::Int(_) => true,
            TyKind::Float(_) => op != BinOp::Rem,
            _ => false,
        };
        if !numeric {
            let mut message = format!("operator `{}` cannot be applied to {}", op.as_str(), self.display(left));
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
                if left.ty != Ty::BYTE {
                    self.require_int(&left);
                }
                let right = self.expr(cx, right, None);
                self.require_int(&right);
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let (left, right) = self.arithmetic_operands(cx, left, right, expected);
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
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected, false);
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never);
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
                );
                if !ordered {
                    operator_error(self, left.ty);
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

/// Whether a `for` pattern borrows: `Some(true)` with any `mut` or `var`
/// binding, `Some(false)` with only `ref` ones (13.3).
/// Whether a `sync.Atomic` of `ty` uses atomic instructions rather than a
/// lock: integers, `Bool`, `Char` and `Byte`.
fn atomic_scalar(types: &Types, ty: Ty) -> bool {
    types.as_int(ty).is_some() || [Ty::BOOL, Ty::CHAR, Ty::BYTE].contains(&ty)
}

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
        "u32" | "uint" => "Tondo writes `UInt32`",
        "u64" | "usize" | "ulong" => "Tondo writes `UInt64`",
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
fn closest<'n>(name: &str, candidates: impl IntoIterator<Item = &'n str>) -> Option<&'n str> {
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
fn bound_names(pattern: &ast::Pattern, names: &mut Vec<String>) {
    match &pattern.kind {
        ast::PatternKind::Binding(name) | ast::PatternKind::Loan(_, name) => names.push(name.name.clone()),
        ast::PatternKind::Tuple(parts) => parts.iter().for_each(|part| bound_names(part, names)),
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
