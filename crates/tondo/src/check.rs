//! Name resolution and type checking: AST to HIR.
//!
//! Checking is bidirectional: every expression receives the type its context
//! expects, if any, which is how unsuffixed literals pick their type (5.7).

use crate::ast::{self, BinOp, UnaryOp};
use crate::diag::Diagnostics;
use crate::hir::{self, Callee, FnId, LocalId, StdFn, TraitId};
use crate::parser::CONTEXTUAL_CONSTRUCTORS;
use crate::source::{SourceMap, Span};
use crate::types::{
    AdtId, AdtKind, ClosureInfo, CursorKind, ERR, Field, IntTy, NONE, OK, Payload, SOME, TemplateId, Ty, TyKind, Types,
    Variant,
};
use std::collections::{HashMap, HashSet};

pub fn check(module: &ast::Module, sources: &SourceMap, diags: &mut Diagnostics) -> Option<hir::Program> {
    let mut checker = Checker {
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
        traits: Vec::new(),
        trait_names: HashMap::new(),
        impls: Vec::new(),
        bounds: HashMap::new(),
        call_bounds: HashMap::new(),
        cursor_templates: HashMap::new(),
        closure_base: 0,
        closure_functions: Vec::new(),
        fn_scopes: Vec::new(),
        console_error: None,
        conversion_error: Ty::ERROR,
        collection_error: Ty::ERROR,
        text_error: Ty::ERROR,
    };
    checker.declare_prelude();
    checker.program(module)
}

/// Prelude type names (26.3); they cannot be redeclared unqualified.
const PRELUDE_TYPES: [&str; 39] = [
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
    "Pointer",
    "Join",
    "Command",
    "Pipeline",
    "Copy",
    "Discard",
    "Equatable",
    "Key",
    "Send",
    "Share",
    "Display",
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
}

#[derive(Copy, Clone)]
enum ModuleName {
    Fn(FnId),
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
}

#[derive(Copy, Clone)]
enum FieldRef<'a> {
    Name(&'a str),
    Slot(u32),
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
    types: Vec<(String, Ty)>,
    self_type: Option<Ty>,
}

#[derive(Clone)]
struct Signature {
    params: Vec<Ty>,
    /// Parameter names, which named arguments use (11.3).
    names: Vec<String>,
    /// The mode of the receiver when the first parameter is `self`.
    receiver: Option<ast::Mode>,
    /// Whether the last parameter is variadic; its type is `Array[T]`.
    variadic: bool,
    result: Ty,
    /// Type parameters: a generic owner's binders, then the function's own.
    generics: Vec<Ty>,
    /// The name diagnostics use for the callee.
    name: String,
}

impl Signature {
    /// The signature of a standard function taking `params` by value.
    fn simple(params: Vec<Ty>, result: Ty) -> Signature {
        let names = (0..params.len()).map(|i| format!("argument{i}")).collect();
        Signature { params, names, receiver: None, variadic: false, result, generics: Vec::new(), name: String::new() }
    }
}

const INTRINSICS: [&str; 2] = ["panic", "assert"];

struct Checker<'a> {
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
    traits: Vec<TraitDef>,
    trait_names: HashMap<String, TraitId>,
    impls: Vec<ImplDef>,
    /// The traits each type parameter is bound by, with their arguments.
    bounds: HashMap<Ty, Vec<(TraitId, Vec<Ty>)>>,
    /// The call protocol bounding a type parameter (11.8): whether it may
    /// change its captures (`CallMut`/`CallOnce`), and its `fn(...)` type.
    call_bounds: HashMap<Ty, (bool, Ty)>,
    /// The cursor types of maps and sets, by what they yield and whether
    /// they belong to a set.
    cursor_templates: HashMap<(CursorKind, bool), TemplateId>,
    /// The `FnId` of the first closure body; closures are numbered after
    /// every declared function.
    closure_base: u32,
    closure_functions: Vec<Option<hir::Function>>,
    /// The scope of each function's signature and body.
    fn_scopes: Vec<FnScope>,
    /// `console.ConsoleError`, once `std.console` is imported.
    console_error: Option<Ty>,
    /// The prelude's `NumericConversionError`.
    conversion_error: Ty,
    /// `std.collections.CollectionError`.
    collection_error: Ty,
    /// `std.text.TextError`.
    text_error: Ty,
}

/// Per-function checking state.
struct FnCx {
    locals: Vec<hir::Local>,
    mutable: Vec<bool>,
    scopes: Vec<Vec<(String, LocalId)>>,
    /// Success type that `return` produces.
    result: Ty,
    /// Error type of a fallible function (`T ! E`), or of a script whose
    /// errors are still being inferred.
    error: Option<Ty>,
    /// A root script infers its error type from `?` and `fail` (20.3).
    infer_error: bool,
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
}

/// A binding a closure captures: read from `source` in the enclosing
/// context when the closure is created.
struct Capture {
    name: String,
    ty: Ty,
    source: hir::Expr,
    mutable: bool,
}

impl Checker<'_> {
    fn program(&mut self, module: &ast::Module) -> Option<hir::Program> {
        for import in &module.imports {
            self.import(import);
        }

        self.declare_types(&module.items);
        let traits: Vec<&ast::TraitDecl> = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Trait(t) => Some(t),
                _ => None,
            })
            .collect();
        self.declare_traits(&traits);
        let fns: Vec<&ast::FnDecl> = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Fn(f) => Some(f),
                _ => None,
            })
            .collect();
        // Every function with a body, in `FnId` order.
        let mut bodies: Vec<&ast::FnDecl> = Vec::new();
        for f in &fns {
            let id = FnId(bodies.len() as u32);
            let owner = f.owner.as_ref().map(|owner| self.method_owner(owner));
            let signature = self.signature(f, owner, FnScope::default());
            let receiver = signature.receiver;
            self.signatures.push(signature);
            bodies.push(f);
            match owner {
                Some(owner) => self.declare_method(f, owner, receiver.is_some(), id),
                None => self.declare_module_name(&f.name, ModuleName::Fn(id)),
            }
        }
        self.trait_methods(&traits, &mut bodies);
        for item in &module.items {
            if let ast::Item::Impl(decl) = item {
                self.implementation(decl, &mut bodies);
            }
        }

        // Closure bodies are numbered after every declared function and the
        // script's `main`.
        self.closure_base = bodies.len() as u32 + u32::from(!module.statements.is_empty());
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
            FnId(functions.len() as u32 - 1)
        } else if let Some(id) = explicit_main {
            self.validate_main(fns[id.0 as usize], id);
            id
        } else {
            self.diags.error_without_span("E1806", "the program has no `main` function and no top-level statements");
            return None;
        };
        functions.extend(std::mem::take(&mut self.closure_functions).into_iter().map(|f| f.expect("checked closure")));

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
        Some(hir::Program {
            types: std::mem::take(&mut self.types),
            functions,
            main,
            traits,
            impls,
            display: HashMap::new(),
        })
    }

    fn declare_prelude(&mut self) {
        // `trait Display { fn display(self): String }` (19.6).
        let self_param = self.types.declare_param("Self".into());
        let signature = Signature {
            params: vec![self_param],
            names: vec!["self".into()],
            receiver: Some(ast::Mode::Ref),
            variadic: false,
            result: Ty::STRING,
            generics: vec![self_param],
            name: "Display.display".into(),
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
            variadic: false,
            result: self.types.intern(TyKind::Option(element)),
            generics: vec![self_param, element],
            name: "Iterator.next".into(),
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
                Field { name: "collection".into(), ty: collection },
                Field { name: "position".into(), ty: Ty::INT },
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
        self.collection_error = ty;
        let (id, ty) = self.types.declare_adt("text.TextError".into());
        let variants = ["InvalidIndex", "InvalidBoundary", "ResourceLimit"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.text_error = ty;
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
        let range = |ty: Ty| match self.types.kind(ty) {
            TyKind::Int(int) => Some((int.min(), int.max())),
            TyKind::Byte => Some((0, 255)),
            _ => None,
        };
        let float = |ty: Ty| matches!(self.types.kind(ty), TyKind::Float(_));
        match (range(from), range(to)) {
            (Some((from_min, from_max)), Some((to_min, to_max))) => Some(from_min < to_min || from_max > to_max),
            (Some(_), None) if float(to) => Some(false),
            (None, Some(_)) if float(from) => Some(true),
            (None, None) if float(from) && float(to) => Some(from == Ty::F64 && to == Ty::F32),
            _ => None,
        }
    }

    fn import(&mut self, import: &ast::Import) {
        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        let module = match path.as_slice() {
            ["std", "console"] => StdModule::Console,
            ["std", "collections"] => StdModule::Collections,
            ["std", "text"] => StdModule::Text,
            ["std", ..] => {
                let what = format!("module `{}`", path.join("."));
                self.diags.unsupported(import.span, &what);
                return;
            }
            _ => {
                let message = format!("module `{}` is not available", path.join("."));
                self.diags.error("E1008", import.span, message);
                return;
            }
        };
        let name = import.alias.as_ref().unwrap_or(import.path.last().unwrap());
        self.declare_module_name(name, ModuleName::Module(module));
        if module == StdModule::Console && self.console_error.is_none() {
            let unit = |name: &str| Variant { name: name.into(), payload: Payload::Unit };
            let (io_id, io) = self.types.declare_adt("io.IoError".into());
            let io_variants = ["Closed", "Cancelled", "InvalidData", "ResourceLimit", "Host"].map(unit).to_vec();
            self.types.define_adt(io_id, AdtKind::Enum(io_variants));
            let (id, ty) = self.types.declare_adt(format!("{}.ConsoleError", name.name));
            let mut variants = ["Unavailable", "Closed", "Cancelled"].map(unit).to_vec();
            variants.push(Variant { name: "Io".into(), payload: Payload::Tuple(vec![io]) });
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.console_error = Some(ty);
        }
    }

    /// A type of an imported standard module, such as `text.TextError`.
    fn module_type(&self, module: &str, name: &str) -> Option<Ty> {
        match (self.names.get(module), name) {
            (Some(ModuleName::Module(StdModule::Console)), "ConsoleError") => self.console_error,
            (Some(ModuleName::Module(StdModule::Collections)), "CollectionError") => Some(self.collection_error),
            (Some(ModuleName::Module(StdModule::Text)), "TextError") => Some(self.text_error),
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
    fn declare_types(&mut self, items: &[ast::Item]) {
        let mut declared = Vec::new();
        let mut templates = Vec::new();
        for item in items {
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
                    continue;
                }
                ast::Item::Fn(_) | ast::Item::Trait(_) | ast::Item::Impl(_) => continue,
            };
            let generics = match item {
                ast::Item::Type(decl) => &decl.generics,
                ast::Item::Enum(decl) => &decl.generics,
                _ => unreachable!(),
            };
            if !generics.is_empty() {
                if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                    self.diags.unsupported(name.span, "generic newtypes");
                    continue;
                }
                let params = self.declare_generics(generics);
                let template = self.types.declare_template(name.name.clone(), params.clone());
                self.declare_type_name(name, TypeName::Template(template));
                templates.push((item, name, template, generics, params));
                continue;
            }
            let (id, ty) = self.types.declare_adt(name.name.clone());
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
    fn declare_generics(&mut self, generics: &[ast::GenericParam]) -> Vec<Ty> {
        let mut params = Vec::new();
        for generic in generics {
            if self.type_named(&generic.name.name).is_some()
                || self.type_params.iter().any(|(n, _)| *n == generic.name.name)
            {
                let message = format!("type parameter `{}` would shadow a visible type", generic.name.name);
                self.diags.error("E1003", generic.name.span, message);
            }
            params.push(self.types.declare_param(generic.name.name.clone()));
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
        let ast::TypeKind::Path { segments, args } = &bound.kind else {
            self.diags.error("E1105", bound.span, "a bound names a trait or capability");
            return;
        };
        match (segments.as_slice(), args.is_empty()) {
            ([name], true) if name.name == "Equatable" => self.types.param_mut(param).equatable = true,
            ([name], true) if name.name == "Key" => self.types.param_mut(param).key = true,
            // Affine values arrive with ownership (H3); until then every
            // value can be copied, discarded and shared.
            ([name], true) if matches!(name.name.as_str(), "Copy" | "Discard" | "Send" | "Share") => {}
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
                if self.call_bounds.insert(param, (name.name != "Call", signature)).is_some() {
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
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Record(fields), .. }) => {
                    AdtKind::Record(self.field_decls(fields))
                }
                ast::Item::Enum(decl) => {
                    let mut seen = HashSet::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if !seen.insert(variant.name.name.as_str()) {
                            let message = format!("variant `{}` is declared more than once", variant.name.name);
                            self.diags.error("E1002", variant.name.span, message);
                        }
                        let payload = match &variant.payload {
                            ast::VariantPayload::Unit => Payload::Unit,
                            ast::VariantPayload::Tuple(items) => {
                                Payload::Tuple(items.iter().map(|t| self.resolve_type(t)).collect())
                            }
                            ast::VariantPayload::Record(fields) => Payload::Record(self.field_decls(fields)),
                        };
                        variants.push(Variant { name: variant.name.name.clone(), payload });
                    }
                    AdtKind::Enum(variants)
                }
                _ => unreachable!(),
            }
        }
    }

    fn field_decls(&mut self, fields: &[ast::FieldDecl]) -> Vec<Field> {
        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        for field in fields {
            if !seen.insert(field.name.name.as_str()) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1002", field.name.span, message);
            }
            resolved.push(Field { name: field.name.name.clone(), ty: self.resolve_type(&field.ty) });
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
    fn signature(&mut self, f: &ast::FnDecl, owner: Option<Ty>, outer: FnScope) -> Signature {
        if f.suspends {
            self.diags.unsupported(f.span, "`suspends` functions");
        }
        let outer_count = outer.types.len();
        self.type_params.extend(outer.types.iter().cloned());
        let saved_self = std::mem::replace(&mut self.self_type, outer.self_type);
        let binders = self.declare_generics(&f.owner_generics);
        self.enter_generics(&f.owner_generics, &binders);
        let owner = match owner {
            Some(key) => Some(self.receiver_type(key, &binders, f)),
            None => self.self_type,
        };
        if f.owner.is_some() {
            // `Self` names the owner in an inherent method (12.3).
            self.self_type = owner;
        }
        let own = self.declare_generics(&f.generics);
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
                    if param.mode != ast::Mode::Value {
                        self.diags.unsupported(param.span, &format!("`{}` parameters", param.mode.keyword()));
                    }
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
        let result = f.result.as_ref().map_or(Ty::UNIT, |t| self.resolve_type(t));
        let scope = FnScope { types: self.type_params.clone(), self_type: self.self_type };
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
        Signature { params, names, receiver, variadic, result, generics, name }
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
            self.traits.push(TraitDef { name: name.name.clone(), self_param, params, methods: Vec::new() });
            self.trait_names.insert(name.name.clone(), id);
        }
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
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
            let scope = FnScope { types, self_type: Some(def.self_param) };
            let mut seen = HashSet::new();
            for f in &decl.methods {
                if !seen.insert(f.name.name.as_str()) {
                    let message = format!("`{}` declares `{}` more than once", decl.name.name, f.name.name);
                    self.diags.error("E1002", f.name.span, message);
                    continue;
                }
                let mut signature = self.signature(f, None, scope.clone());
                signature.name = format!("{}.{}", decl.name.name, f.name.name);
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
        let binders = self.declare_generics(&decl.generics);
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
        let local_trait = trait_id != DISPLAY;
        let local_target = matches!(self.types.kind(target), TyKind::Adt(_)) && self.types.repr(target) != Ty::ERROR;
        if !local_trait && !local_target {
            let message = format!(
                "`impl {trait_name} for {target_name}` implements a prelude trait for a type this module does not own; wrap it in a newtype, such as `type Name = {target_name}`"
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
        let scope = FnScope { types, self_type: Some(target) };
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
            let expected = &declared[index].1;
            if let Some(problem) = self.signature_mismatch(expected, &signature, &map, binders.len()) {
                let message = format!("`{}` does not match `{trait_name}`: {problem}", f.name.name);
                self.diags.error("E1114", f.name.span, message);
            }
            self.signatures.push(signature);
            bodies.push(f);
            methods[index] = Some(id);
        }
        for ((name, _, has_default), provided) in declared.iter().zip(&methods) {
            if provided.is_none() && !has_default {
                let message = format!("`impl {trait_name} for {target_name}` is missing `{name}`");
                self.diags.error("E1114", decl.span, message);
            }
        }
        self.impls.push(ImplDef { intrinsic: false, trait_id, trait_args, target, generics: binders, methods });
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
        if expected.receiver != found.receiver {
            return Some("a different receiver".into());
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
        if self.types.param(ty).is_some() {
            return self.bounds.get(&ty).is_some_and(|b| b.iter().any(|(t, a)| *t == trait_id && a == args));
        }
        if trait_id == DISPLAY {
            let parts: Option<Vec<Ty>> = match self.types.kind(ty) {
                TyKind::Int(_) | TyKind::Float(_) | TyKind::Char | TyKind::Byte | TyKind::Bool | TyKind::String => {
                    Some(Vec::new())
                }
                TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => Some(vec![*element]),
                TyKind::Map(key, value) => Some(vec![*key, *value]),
                _ => None,
            };
            if let Some(parts) = parts {
                return parts.into_iter().all(|part| self.satisfies_depth(part, DISPLAY, &[], depth + 1));
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
        let (equatable, key) = (def.equatable, def.key);
        if key && !self.types.is_key(actual) {
            return Some("`Key`".into());
        }
        if equatable && !self.types.is_equatable(actual) {
            return Some("`Equatable`".into());
        }
        if let Some((mutable, signature)) = self.call_bounds.get(&param).copied() {
            let signature = self.types.subst(signature, map);
            if let Some(problem) = self.callable_as(actual, mutable, signature) {
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
                let found: Vec<Vec<Ty>> = if self.types.param(actual).is_some() {
                    self.bounds
                        .get(&actual)
                        .into_iter()
                        .flatten()
                        .filter(|(t, _)| *t == trait_id)
                        .map(|(_, a)| a.clone())
                        .collect()
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

    /// What a closure passed for the type parameter `ty` with a call bound
    /// should look like: the bound's signature with the arguments inferred
    /// so far. An unknown result is left to the closure (`{error}`).
    fn call_expectation(&mut self, inference: &Inference, ty: Ty) -> Option<Ty> {
        let &(_, bound) = self.call_bounds.get(&ty)?;
        let TyKind::Fn(params, result, variadic) = self.types.kind(bound).clone() else { return None };
        let params = params.into_iter().map(|p| self.inferred(inference, p)).collect::<Option<Vec<Ty>>>()?;
        let result = self.inferred(inference, result).unwrap_or(Ty::ERROR);
        Some(self.types.intern(TyKind::Fn(params, result, variadic)))
    }

    /// Why `ty` cannot be called as `signature` with the protocol a call
    /// bound demands (`Call` unless `mutable`), if it cannot (11.8).
    fn callable_as(&mut self, ty: Ty, mutable: bool, signature: Ty) -> Option<String> {
        let wanted = format!("`{}[{}]`", if mutable { "CallMut" } else { "Call" }, self.types.display(signature));
        match self.callable(ty) {
            Some((params, result, variadic, mutates)) => {
                let found = self.types.intern(TyKind::Fn(params, result, variadic));
                if found != signature {
                    Some(format!("{wanted} (its signature is `{}`)", self.types.display(found)))
                } else if mutates && !mutable {
                    Some(format!("{wanted} (it changes its captures, so it is only `CallMut`)"))
                } else {
                    None
                }
            }
            None if self.types.has_error(ty) => None,
            None => Some(wanted),
        }
    }

    /// How a value of `ty` is called: parameters, result, whether the last
    /// parameter is variadic and whether calling may change it.
    fn callable(&mut self, ty: Ty) -> Option<(Vec<Ty>, Ty, bool, bool)> {
        if let TyKind::Fn(params, result, variadic) = self.types.kind(ty).clone() {
            return Some((params, result, variadic, false));
        }
        if let Some(&(mutable, signature)) = self.call_bounds.get(&ty) {
            let TyKind::Fn(params, result, variadic) = self.types.kind(signature).clone() else { unreachable!() };
            return Some((params, result, variadic, mutable));
        }
        let (_, _, info) = self.types.closure(ty)?;
        Some((info.params, info.result, info.variadic, info.mutates))
    }

    /// The type of `self` in a method of the owner keyed `key`: a generic
    /// owner applied to the method's binders (11.5).
    fn receiver_type(&mut self, key: Ty, binders: &[Ty], f: &ast::FnDecl) -> Ty {
        if key == Ty::ERROR {
            return key;
        }
        let owner = f.owner.as_ref().unwrap();
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

    /// The key of the operations of `ty`: a generic type's instance with its
    /// own parameters stands for all of its instances.
    fn owner_key(&mut self, ty: Ty) -> Ty {
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
    fn method_owner(&mut self, owner: &ast::Ident) -> Ty {
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
            Some(Some(ty)) if !PRELUDE_TYPES.contains(&owner.name.as_str()) => ty,
            Some(_) => {
                let message = format!(
                    "methods can only be declared on records, enums and newtypes of this module, not `{}`",
                    owner.name
                );
                self.diags.error("E1002", owner.span, message);
                Ty::ERROR
            }
            None if PRELUDE_TYPES.contains(&owner.name.as_str()) || self.type_named(&owner.name).is_some() => {
                let message = format!("methods cannot be added to the intrinsic type `{}`", owner.name);
                self.diags.error("E1002", owner.span, message);
                Ty::ERROR
            }
            None => {
                let message = format!("unknown type `{}`", owner.name);
                self.diags.error("E1001", owner.span, message);
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
        match clash {
            Some(message) => self.diags.error("E1002", f.name.span, message),
            None => {
                self.methods.insert((owner, name.clone()), id);
            }
        }
    }

    fn record_has_field(&self, ty: Ty, name: &str) -> bool {
        matches!(self.types.kind(ty), TyKind::Adt(id)
            if matches!(&self.types.adt(*id).kind, AdtKind::Record(fields) if fields.iter().any(|f| f.name == name)))
    }

    fn validate_main(&mut self, f: &ast::FnDecl, id: FnId) {
        let signature = &self.signatures[id.0 as usize];
        let problem = if f.public {
            Some("`main` cannot be `pub`")
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
        match &t.kind {
            ast::TypeKind::Path { segments, args } => {
                if !args.is_empty() {
                    let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
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
                        ("Map", [key, value]) if segments.len() == 1 => {
                            self.require_key(*key, t.span);
                            self.map_type(*key, *value)
                        }
                        ("Set", [element]) if segments.len() == 1 => {
                            self.require_key(*element, t.span);
                            self.types.intern(TyKind::Set(*element))
                        }
                        ("Option" | "Result" | "Array" | "Deque" | "Map" | "Set", _) if segments.len() == 1 => {
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
                        let message = format!("unknown type `{}`", names.join("."));
                        self.diags.error("E1001", t.span, message);
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
            ast::TypeKind::Fn { params, variadic, result } => {
                let mut types: Vec<Ty> = params.iter().map(|p| self.resolve_type(p)).collect();
                if *variadic && let Some(last) = types.last_mut() {
                    *last = self.types.intern(TyKind::Array(*last));
                }
                let result = result.as_ref().map_or(Ty::UNIT, |r| self.resolve_type(r));
                self.types.intern(TyKind::Fn(types, result, *variadic))
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
            locals: Vec::new(),
            mutable: Vec::new(),
            scopes: vec![Vec::new()],
            result: success,
            error,
            infer_error: false,
            loops: Vec::new(),
            diverged: false,
            parent: None,
            env: None,
            captures: Vec::new(),
            mutates: false,
            infer_result: false,
        }
    }

    fn function(&mut self, f: &ast::FnDecl, id: FnId) -> hir::Function {
        let signature = self.signatures[id.0 as usize].clone();
        let scope = self.fn_scopes[id.0 as usize].clone();
        let outer_types = std::mem::replace(&mut self.type_params, scope.types);
        let outer_self = std::mem::replace(&mut self.self_type, scope.self_type);
        let mut cx = self.new_cx(signature.result);
        let mut params = Vec::new();
        let mut loans = Vec::new();
        for (param, ty) in f.params.iter().zip(signature.params) {
            // `mut self` and `var self` borrow the caller's place (11.6).
            let loan = param.ty.is_none() && matches!(param.mode, ast::Mode::Mut | ast::Mode::Var);
            params.push(self.declare(&mut cx, &param.name, ty, loan));
            loans.push(loan);
        }
        let success = cx.result;
        let body = self.block(&mut cx, f.body.as_ref().expect("functions with an id have a body"), Some(success));
        self.type_params = outer_types;
        self.self_type = outer_self;
        hir::Function {
            name: signature.name,
            generics: signature.generics,
            params,
            loans,
            locals: cx.locals,
            result: signature.result,
            body,
        }
    }

    /// The implicit `main` of a root script (spec 20.3). Its error type is
    /// inferred from the `?` and `fail` in its statements.
    fn script_main(&mut self, statements: &[ast::Stmt]) -> hir::Function {
        let mut cx = self.new_cx(Ty::UNIT);
        cx.infer_error = true;
        let stmts = self.statements(&mut cx, statements);
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        let body = hir::Block { stmts, tail: None, ty };
        let result = match cx.error {
            Some(error) => self.types.intern(TyKind::Result(Ty::UNIT, error)),
            None => Ty::UNIT,
        };
        hir::Function {
            name: "main".into(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            locals: cx.locals,
            result,
            body,
        }
    }

    /// The error type a `fail` or `?` may produce here, inferring it in a
    /// root script; `None` in an infallible function.
    fn error_channel(&mut self, cx: &mut FnCx, produced: Ty, span: Span, code: &'static str) -> Option<Ty> {
        match cx.error {
            Some(error) => Some(error),
            None if cx.infer_error => {
                cx.error = Some(produced);
                Some(produced)
            }
            None => {
                self.diags.error(code, span, "this function cannot fail; declare its result as `T ! E`");
                None
            }
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
        let index = match cx.captures.iter().position(|c| c.name == name) {
            Some(index) => index,
            None => {
                let parent = cx.parent.as_deref_mut()?;
                let (source, mutable) = match self.lookup(parent, name) {
                    Some(local) => {
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
        let expected =
            expected.and_then(|t| self.callable(t)).map(|(params, result, variadic, _)| (params, result, variadic));
        let mut params = Vec::new();
        let mut variadic = false;
        for (index, param) in closure.params.iter().enumerate() {
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
                (None, Some((types, _, expected_variadic))) if index < types.len() => {
                    variadic = *expected_variadic && index + 1 == types.len();
                    types[index]
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
        let declared = closure
            .result
            .as_ref()
            .map(|t| self.resolve_type(t))
            .or(expected.as_ref().map(|e| e.1).filter(|&r| r != Ty::ERROR));
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
            mutates: false,
        };
        self.types.declare_closure(template, info);

        // The body is checked in its own context, nested in the current one.
        let mut inner = self.new_cx(declared.unwrap_or(Ty::ERROR));
        inner.infer_result = declared.is_none();
        let outer = std::mem::replace(cx, inner);
        cx.parent = Some(Box::new(outer));
        let env = self.hidden_local(cx, env_ty);
        cx.mutable[env.0 as usize] = true;
        cx.env = Some(env);
        let mut locals = vec![env];
        for (param, &ty) in closure.params.iter().zip(&params) {
            locals.push(self.declare(cx, &param.name, ty, false));
        }
        let success = (!cx.infer_result).then_some(cx.result);
        let body = self.block(cx, &closure.body, success);
        let result = match declared {
            Some(declared) => declared,
            None if cx.result != Ty::ERROR => cx.result,
            None => body.ty,
        };
        let parent = cx.parent.take().expect("a closure context has a parent");
        let inner = std::mem::replace(cx, *parent);

        let fields = inner.captures.iter().map(|c| Field { name: c.name.clone(), ty: c.ty }).collect();
        self.types.define_template(template, AdtKind::Record(fields));
        let info = self.types.closure_info_mut(template);
        info.result = result;
        info.mutates = inner.mutates;
        let mut loans = vec![false; locals.len()];
        loans[0] = true;
        self.closure_functions[index] = Some(hir::Function {
            name: "closure".into(),
            generics,
            params: locals,
            loans,
            locals: inner.locals,
            result,
            body,
        });
        // The closure value holds the captured values, read now.
        let values = inner.captures.into_iter().enumerate().map(|(i, c)| (i as u32, c.source)).collect();
        Self::mk(hir::ExprKind::Record(values), env_ty, span)
    }

    /// A local that no name refers to, such as the value of `for _ in ...`.
    fn hidden_local(&mut self, cx: &mut FnCx, ty: Ty) -> LocalId {
        cx.locals.push(hir::Local { name: "_".into(), ty });
        cx.mutable.push(false);
        LocalId(cx.locals.len() as u32 - 1)
    }

    fn declare(&mut self, cx: &mut FnCx, name: &ast::Ident, ty: Ty, mutable: bool) -> LocalId {
        if self.lookup(cx, &name.name).is_some()
            || self.visible_capture(cx, &name.name)
            || self.names.contains_key(&name.name)
        {
            let message = format!("`{}` would shadow a visible binding with the same name", name.name);
            self.diags.error("E1003", name.span, message);
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        }
        let id = LocalId(cx.locals.len() as u32);
        cx.locals.push(hir::Local { name: name.name.clone(), ty });
        cx.mutable.push(mutable);
        cx.scopes.last_mut().unwrap().push((name.name.clone(), id));
        id
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self, cx: &mut FnCx, b: &ast::Block, expected: Option<Ty>) -> hir::Block {
        cx.scopes.push(Vec::new());
        let outer = std::mem::replace(&mut cx.diverged, false);
        let stmts = self.statements(cx, &b.stmts);
        let tail = b.tail.as_ref().map(|t| {
            if cx.diverged {
                self.diags.warning("W1006", t.span, "unreachable expression");
            }
            let value = self.expr(cx, t, expected);
            Box::new(match expected {
                Some(expected) => self.coerce(value, expected),
                None => value,
            })
        });
        let ty = match &tail {
            _ if cx.diverged => Ty::NEVER,
            Some(tail) => tail.ty,
            None => {
                if let Some(expected) = expected.filter(|&t| t != Ty::UNIT && t != Ty::ERROR) {
                    let message = format!("expected {}, found `Unit`", self.display(expected));
                    let end = Span::new(b.span.end - 1, b.span.end);
                    self.diags.error("E1102", end, message);
                }
                Ty::UNIT
            }
        };
        cx.diverged = outer;
        cx.scopes.pop();
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
                        let pattern =
                            self.pattern(cx, pattern, ty, PatternContext { in_match: false, mutable: *mutable });
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
            ast::StmtKind::Expr(value) => {
                let value = self.expr(cx, value, None);
                if ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&value.ty) {
                    let message =
                        format!("a value of type {} is not used; discard it with `_ = ...`", self.display(value.ty));
                    self.diags.error("E1303", value.span, message);
                }
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
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
                    Some(value) => {
                        let value = self.expr(cx, value, Some(result));
                        Some(self.coerce(value, result))
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
        if let ast::ExprKind::Slice { base, start, end, step } = &target.kind {
            return self.assign_slice(cx, base, [start, end, step], op, value, span);
        }
        if let ast::ExprKind::Index { base, index } = &target.kind {
            let (local, path, ty) = self.place(cx, base)?;
            if let TyKind::Map(key, entry) = *self.types.kind(ty) {
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
                let place = hir::Place { local, path };
                let insert = hir::ExprKind::Mutate(hir::Mutation::Insert, place, vec![key_value, new]);
                return Some(hir::Stmt::Expr(Self::mk(insert, result, span)));
            }
        }
        let (local, path, ty) = self.place(cx, target)?;
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
                        let message = format!("cannot assign to `{}`: it is not declared with `var`", root.name);
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
                if !cx.mutable[local.0 as usize] {
                    let message = format!("cannot assign to `{}`: it is not declared with `var`", root.name);
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
                let (local, mut path, ty) = self.place(cx, base)?;
                let (TyKind::Array(element) | TyKind::Deque(element)) = *self.types.kind(ty) else {
                    let message = if ty == Ty::STRING {
                        "a `String` is immutable: build a new one, for example with interpolation".to_string()
                    } else {
                        format!("{} cannot be indexed", self.display(ty))
                    };
                    self.diags.error("E1102", base.span, message);
                    return None;
                };
                let index = self.expr(cx, index, Some(Ty::INT));
                path.push(hir::PlaceStep::Index(self.coerce(index, Ty::INT)));
                return Some((local, path, element));
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
            let message = format!("{} has no {what}", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        found
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
                    let element = match *self.types.kind(source.ty) {
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
                    cx.scopes.push(Vec::new());
                    let pattern =
                        self.pattern(cx, pattern, element, PatternContext { in_match: false, mutable: false });
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
                let (start, end) = self.operands(cx, start, end, None);
                self.require_int(&start);
                cx.scopes.push(Vec::new());
                let local = match &pattern.kind {
                    ast::PatternKind::Binding(binding) => self.declare(cx, binding, start.ty, false),
                    ast::PatternKind::Wildcard => self.hidden_local(cx, start.ty),
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

    // ---- expressions -----------------------------------------------------

    fn mk(kind: hir::ExprKind, ty: Ty, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
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
        // A closure becomes a `fn(...)` value when it never changes its
        // captures (11.8).
        if let TyKind::Fn(..) = self.types.kind(expected)
            && let Some((body, _, info)) = self.types.closure(value.ty)
        {
            let found = self.types.intern(TyKind::Fn(info.params, info.result, info.variadic));
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
        // A value of a member's exact type is injected into a union (9.5).
        if let Some(member) = self.types.members(expected).and_then(|m| m.iter().position(|&t| t == value.ty)) {
            let span = value.span;
            return Self::mk(hir::ExprKind::Variant(member as u32, vec![(0, value)]), expected, span);
        }
        if let TyKind::Option(inner) = *self.types.kind(expected)
            && (value.ty == inner || self.types.members(inner).is_some_and(|m| m.contains(&value.ty)))
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
            let message = format!("expected {}, found {}", self.display(expected), self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
        value
    }

    fn require_int(&mut self, value: &hir::Expr) {
        if self.types.as_int(value.ty).is_none() && ![Ty::NEVER, Ty::ERROR].contains(&value.ty) {
            let message = format!("expected an integer, found {}", self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
    }

    fn expr(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
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
                let (TyKind::Array(element) | TyKind::Deque(element)) = *self.types.kind(base.ty) else {
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
            ast::ExprKind::Record { path, type_args, fields } => {
                self.record_literal(cx, path, type_args, fields, expected, span)
            }
            ast::ExprKind::With { base, updates } => self.with_expr(cx, base, updates, span),
            ast::ExprKind::Try(inner) => self.try_expr(cx, inner, span),
            ast::ExprKind::Closure(closure) => self.closure(cx, closure, expected, span),
            ast::ExprKind::None => match self.union_member(expected, "none").map(|t| (t, self.types.kind(t))) {
                Some((ty, TyKind::Option(_))) => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), ty, span),
                _ => {
                    self.diags.error("E1304", span, "`none` needs an expected `T?` type");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Construct(constructor, value) => self.construct(cx, constructor, value, expected, span),
            ast::ExprKind::Error => Self::error_expr(span),
        }
    }

    fn field_access(&mut self, base: hir::Expr, field: FieldRef, field_span: Span, span: Span) -> hir::Expr {
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
                let message = format!("{} has no field `{}`", self.display(record), init.name.name);
                self.diags.error("E1102", init.name.span, message);
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
            | (TyKind::Deque(a), TyKind::Deque(b))
            | (TyKind::Set(a), TyKind::Set(b)) => self.unify(inference, a, b),
            // A value lifts into an option.
            (TyKind::Option(a), _) => self.unify(inference, a, actual),
            (TyKind::Result(a, b), TyKind::Result(c, d)) | (TyKind::Map(a, b), TyKind::Map(c, d)) => {
                self.unify(inference, a, c);
                self.unify(inference, b, d);
            }
            (TyKind::Fn(params, result, _), TyKind::Fn(values, value, _)) if params.len() == values.len() => {
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
                        self.unify(inference, arg, value);
                    }
                }
            }
            _ => {}
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
            let (ty, values, seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
            self.report_missing(&fields, &seen, span);
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        let [name] = path else {
            return self.unsupported_expr(span, "qualified record literals");
        };
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
                let message = format!("unknown type `{}`", name.name);
                self.diags.error("E1001", name.span, message);
                return Self::error_expr(span);
            }
        };
        let Some(fields) = self.record_fields(ty, name.span) else {
            return Self::error_expr(span);
        };
        let (ty, values, seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
        self.report_missing(&fields, &seen, span);
        Self::mk(hir::ExprKind::Record(values), ty, span)
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
            let message = format!("construction is missing fields: {}", missing.join(", "));
            self.diags.error("E1102", span, message);
        }
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
            Some(Some(index)) => Some(Some((ty, index as u32))),
            Some(None) => {
                let message = format!("enum `{}` has no variant `{}`", type_name.name, variant.name);
                self.diags.error("E1001", variant.span, message);
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
            let pattern =
                self.pattern(cx, &arm.pattern, scrutinee.ty, PatternContext { in_match: true, mutable: false });
            let guard = arm.guard.as_ref().map(|guard| {
                let guard = self.expr(cx, guard, Some(Ty::BOOL));
                self.coerce(guard, Ty::BOOL)
            });
            let body = match &arm.body {
                ast::ArmBody::Expr(body) => {
                    let value = self.expr(cx, body, result);
                    match result {
                        Some(expected) => self.coerce(value, expected),
                        None => value,
                    }
                }
                ast::ArmBody::Transfer(stmt) => {
                    let stmts = self.statement(cx, stmt).into_iter().collect();
                    let block = hir::Block { stmts, tail: None, ty: Ty::NEVER };
                    Self::mk(hir::ExprKind::Block(block), Ty::NEVER, stmt.span)
                }
            };
            if result.is_none() && body.ty != Ty::NEVER && body.ty != Ty::ERROR {
                result = Some(body.ty);
            }
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
                    (record, fields, None)
                } else {
                    let message = format!("`{}` is not a record type or variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                if record_ty != ty {
                    let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
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
                let rest =
                    rest.as_ref().map(|name| name.as_ref().map(|name| self.declare(cx, name, ty, context.mutable)));
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
        let ty = base.ty;
        let Some(fields) = self.record_fields(ty, base.span) else {
            return Self::error_expr(span);
        };
        if updates.is_empty() {
            self.diags.error("E1102", span, "`with` must update at least one field");
        }
        let (values, _) = self.field_values(cx, ty, &fields, updates, false, None);
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
    fn try_expr(&mut self, cx: &mut FnCx, inner: &ast::Expr, span: Span) -> hir::Expr {
        let value = self.expr(cx, inner, None);
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
            let constant = match &k.kind {
                hir::ExprKind::Int(v) => Some(hir::Literal::Int(*v)),
                hir::ExprKind::Str(s) => Some(hir::Literal::Str(s.clone())),
                hir::ExprKind::Bool(b) => Some(hir::Literal::Bool(*b)),
                hir::ExprKind::Char(c) => Some(hir::Literal::Char(*c)),
                _ => None,
            };
            if let Some(constant) = constant {
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
        let ast::ExprKind::Path(segments) = &base.kind else {
            return self.unsupported_expr(span, "generic calls");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
        {
            let explicit: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
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
            _ => return self.unsupported_expr(span, "generic functions"),
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
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, receiver, None);
        let ty = value.ty;
        let key = self.owner_key(ty);
        if let Some(&id) = self.methods.get(&(key, method.name.clone())) {
            let signature = self.signatures[id.0 as usize].clone();
            return self.dot_call(cx, Callee::Fn(id), signature, HashMap::new(), value, method, args, span);
        }
        if self.types.param(ty).is_some() {
            // A type parameter has the methods of the traits it is bound by (12.7).
            let bounds = self.bounds.get(&ty).cloned().unwrap_or_default();
            let found: Vec<(TraitId, Vec<Ty>, usize)> = bounds
                .iter()
                .filter_map(|(t, a)| {
                    let index = self.traits[t.0 as usize].methods.iter().position(|m| m.name == method.name)?;
                    Some((*t, a.clone(), index))
                })
                .collect();
            return match found.as_slice() {
                [(trait_id, trait_args, index)] => {
                    let def = &self.traits[trait_id.0 as usize];
                    let signature = def.methods[*index].signature.clone();
                    let mut prebound = HashMap::from([(def.self_param, ty)]);
                    prebound.extend(def.params.iter().copied().zip(trait_args.iter().copied()));
                    let callee = Callee::Trait(*trait_id, *index as u32);
                    self.dot_call(cx, callee, signature, prebound, value, method, args, span)
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
            "length" | "byteLength" | "isEmpty" | "chars" => (S::ByteLength, vec![], Ty::INT),
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
            // A string is its own iterator of `Char` (10.2.2).
            "chars" => return values.remove(0),
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
        let generic = self.types.intern(TyKind::Fn(signature.params.clone(), signature.result, signature.variadic));
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
        let Some((params, result, variadic, mutates)) = self.callable(callee.ty) else {
            let message = format!("{} cannot be called", self.display(callee.ty));
            self.diags.error("E1102", callee.span, message);
            return Self::error_expr(span);
        };
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            self.diags.error("E1102", arg.span, "a call through a function value takes positional arguments");
            return Self::error_expr(span);
        }
        let names = (0..params.len()).map(|i| format!("argument{}", i + 1)).collect();
        let signature = Signature {
            params,
            names,
            receiver: None,
            variadic,
            result,
            generics: Vec::new(),
            name: "this function".into(),
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
        cx.scopes.push(Vec::new());
        let pattern = self.pattern(cx, pattern, element, PatternContext { in_match: false, mutable: false });
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
        if self.types.param(ty).is_some() {
            return self.bounds.get(&ty)?.iter().find(|(t, _)| *t == ITERATOR).map(|(_, args)| args[0]);
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
        let Some(hir::Place { local, path }) = self.loan_place(cx, receiver, &operation) else {
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
            Some(ast::Mode::Mut | ast::Mode::Var) => {
                let receiver_span = receiver.span;
                let operation = format!("`{}`", method.name);
                match self.loan_place(cx, receiver, &operation) {
                    Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, receiver_span),
                    None => return Self::error_expr(span),
                }
            }
            Some(_) => receiver,
        };
        let target = CallTarget { prebound, ..CallTarget::new(callee, signature, None) };
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
        }
        self.diags.error("E1001", method.span, message);
        Self::error_expr(span)
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

    fn string(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], span: Span) -> hir::Expr {
        if let [ast::StrSegment::Text(text)] = segments {
            return Self::mk(hir::ExprKind::Str(text.clone()), Ty::STRING, span);
        }
        let mut out = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(text) => out.push(hir::Segment::Text(text.clone())),
                ast::StrSegment::Expr(e) => {
                    let value = self.expr(cx, e, None);
                    if !self.satisfies(value.ty, DISPLAY, &[]) {
                        let mut message = format!("{} does not implement `Display`", self.display(value.ty));
                        if self.types.members(value.ty).is_some() {
                            message.push_str(": `match` on its members to format each one");
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
                    // A user type or a type parameter is displayed by `Display.display`.
                    let value = if matches!(self.types.kind(value.ty), TyKind::Adt(_) | TyKind::Param(_)) {
                        let (ty, span) = (value.ty, value.span);
                        let call = hir::ExprKind::Call(Callee::Trait(DISPLAY, 0), vec![ty], vec![value]);
                        Self::mk(call, Ty::STRING, span)
                    } else {
                        value
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
            for segment in &segments[1..] {
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        if let Some(local) = self.lookup(cx, &first.name) {
            let ty = cx.locals[local.0 as usize].ty;
            let mut value = Self::mk(hir::ExprKind::Local(local), ty, first.span);
            for segment in &segments[1..] {
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
            Some(ModuleName::Fn(id)) if segments.len() == 1 => self.fn_value(*id, Vec::new(), expected, span),
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
                let message = format!(
                    "`{}` is a module, not a value",
                    segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
                );
                self.diags.error("E1102", span, message);
                Self::error_expr(span)
            }
            None => {
                self.unknown_name(&first.name, first.span);
                Self::error_expr(span)
            }
        }
    }

    /// Reports `E1001`, pointing habits from other languages at the canonical form.
    fn unknown_name(&mut self, name: &str, span: Span) {
        let message = match name {
            "Some" | "Ok" | "Err" => format!("unknown name `{name}`; Tondo writes `{}(...)`", name.to_lowercase()),
            "None" => "unknown name `None`; Tondo writes `none`".to_string(),
            _ => format!("unknown name `{name}`"),
        };
        self.diags.error("E1001", span, message);
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
            ast::ExprKind::Field(receiver, method) => return self.method_call(cx, receiver, method, args, span),
            ast::ExprKind::Path(segments)
                if segments.len() > 1
                    && (self.lookup(cx, &segments[0].name).is_some()
                        || self.visible_capture(cx, &segments[0].name)) =>
            {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, span);
            }
            ast::ExprKind::Generic { base, args: type_args } => {
                return self.generic_call(cx, base, type_args, args, expected, span);
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
            ["String", "empty"] => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                return Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
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
        if let ["Array", "new" | "withCapacity"] | ["Deque", "new"] = names.as_slice() {
            let message = format!("`{0}.{1}` needs its element type: `{0}.{1}[T](...)`", names[0], names[1]);
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        // `Trait.method(value, ...)` calls an implementation explicitly (12.7).
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
        {
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
            (["panic"], _) => {
                let [message] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let message = self.expr(cx, &message.value, Some(Ty::STRING));
                let message = self.coerce(message, Ty::STRING);
                Self::mk(hir::ExprKind::Panic(Box::new(message)), Ty::NEVER, span)
            }
            (["assert"], _) => {
                let Some((condition, parts)) = args.split_first() else {
                    return self.arity_error(span, 1, 0);
                };
                let source = self.sources.slice(condition.value.span).to_string();
                let condition = self.expr(cx, &condition.value, Some(Ty::BOOL));
                let condition = Box::new(self.coerce(condition, Ty::BOOL));
                let parts = parts
                    .iter()
                    .map(|p| {
                        let part = self.expr(cx, &p.value, Some(Ty::STRING));
                        self.coerce(part, Ty::STRING)
                    })
                    .collect();
                Self::mk(hir::ExprKind::Assert { condition, parts, source }, Ty::UNIT, span)
            }
            ([_], Some(ModuleName::Newtype(ty))) => {
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                let value = self.expr(cx, &arg.value, Some(inner));
                let value = self.coerce(value, inner);
                Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span)
            }
            ([_], Some(ModuleName::Fn(id))) => {
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Console))) => {
                // `print` and `println` only append to the output buffer and
                // cannot fail; delivery errors surface through `flush` (19.1).
                let (function, params, result) = match *member {
                    "print" => (StdFn::ConsolePrint, vec![Ty::STRING], Ty::UNIT),
                    "println" => (StdFn::ConsolePrintln, vec![Ty::STRING], Ty::UNIT),
                    "flush" => {
                        let error = self.console_error.unwrap();
                        (StdFn::ConsoleFlush, Vec::new(), self.types.intern(TyKind::Result(Ty::UNIT, error)))
                    }
                    "stdin" | "stdout" | "stderr" | "readLine" => {
                        let what = format!("`console.{member}`");
                        return self.unsupported_expr(span, &what);
                    }
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
            ([name], None) if Self::numeric_type(name).is_some() => {
                let to = Self::numeric_type(name).unwrap();
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let value = self.expr(cx, &arg.value, None);
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
                self.unknown_name(names[0], segments[0].span);
                Self::error_expr(span)
            }
            _ => self.unsupported_expr(callee.span, "calling this expression"),
        }
    }

    fn arity_error(&mut self, span: Span, expected: usize, found: usize) -> hir::Expr {
        let message = format!("expected {expected} argument(s), found {found}");
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
        let signature = &signature;
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
                        Some(position) => position,
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
                if spread.is_some() || (arg.spread && !pack.is_empty()) {
                    self.diags.error("E1102", arg.span, "a spread `...values` must be the only variadic argument");
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
        if let Some(missing) = slots.iter().position(Option::is_none) {
            let message = format!("missing argument `{}`", signature.names[first + missing]);
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
        // Check the arguments in the order they are written; an unsuffixed
        // literal waits until the others have fixed its type.
        let mut values: Vec<Option<hir::Expr>> = (0..args.len()).map(|_| None).collect();
        for pass in 0..2 {
            for (index, arg) in args.iter().enumerate() {
                if values[index].is_some() {
                    continue;
                }
                let ty = type_of(index);
                let expected = self.inferred(&inference, ty).or_else(|| self.call_expectation(&inference, ty));
                if pass == 0 && expected.is_none() && self.needs_context(&arg.value) {
                    continue;
                }
                let receiver_mode = (parameter_of(index) == 0).then_some(signature.receiver).flatten();
                let value = self.argument_value(cx, arg, expected, receiver_mode);
                self.unify(&mut inference, ty, value.ty);
                // A callable bound to `F: Call[fn(A): B]` also fixes `A` and `B`.
                if let Some(&(_, bound)) = self.call_bounds.get(&ty)
                    && let Some((params, result, variadic, _)) = self.callable(value.ty)
                {
                    let found = self.types.intern(TyKind::Fn(params, result, variadic));
                    self.unify(&mut inference, bound, found);
                }
                values[index] = Some(value);
            }
        }
        if let Some(expected) = expected {
            self.unify(&mut inference, signature.result, expected);
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
        for (index, value) in values.iter_mut().enumerate() {
            let ty = self.types.subst(type_of(index), &inference.map);
            let receiver_mode = (parameter_of(index) == 0).then_some(signature.receiver).flatten();
            let raw = value.take().unwrap();
            *value = Some(self.finish_argument(cx, raw, ty, receiver_mode));
        }
        let type_args: Vec<Ty> = signature.generics.iter().map(|g| inference.map[g]).collect();
        // The arguments must meet the bounds of the parameters they replace.
        for (&param, &actual) in signature.generics.iter().zip(&type_args) {
            if let Some(missing) = self.bound_holds(param, actual, &inference.map, 0) {
                let message = format!(
                    "{} does not implement {missing}, which `{}` requires of `{}`",
                    self.display(actual),
                    signature.name,
                    self.types.display(param)
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
        let result = self.types.subst(signature.result, &inference.map);
        let mut ordered: Vec<(Option<usize>, hir::Expr)> = Vec::new();
        if let Some(receiver) = receiver {
            ordered.push((None, receiver));
        }
        for slot in slots {
            let index = slot.unwrap();
            ordered.push((Some(index), values[index].take().unwrap()));
        }
        if let Some(array) = signature.variadic.then(|| signature.params[signature.params.len() - 1]) {
            let array = self.types.subst(array, &inference.map);
            let value = match spread {
                Some(index) => values[index].take().unwrap(),
                None => {
                    let items: Vec<hir::Expr> = pack.iter().map(|&i| values[i].take().unwrap()).collect();
                    Self::mk(hir::ExprKind::Array(items), array, span)
                }
            };
            ordered.push((spread.or(pack.first().copied()), value));
        }
        // Named arguments out of declaration order are evaluated into
        // temporaries first, in the order they are written.
        let in_order = ordered.iter().filter_map(|(i, _)| *i).collect::<Vec<_>>().is_sorted();
        if in_order {
            let args = ordered.into_iter().map(|(_, value)| value).collect();
            return Self::mk(hir::ExprKind::Call(callee, type_args, args), result, span);
        }
        let mut written: Vec<(usize, LocalId)> = Vec::new();
        let mut stmts = Vec::new();
        let mut args = Vec::new();
        for (index, value) in &mut ordered {
            if let Some(index) = index
                && !matches!(value.kind, hir::ExprKind::Loan(_))
            {
                let local = self.hidden_local(cx, value.ty);
                written.push((*index, local));
                let value = std::mem::replace(value, Self::error_expr(span));
                args.push(Self::mk(hir::ExprKind::Local(local), value.ty, value.span));
                stmts.push((*index, hir::Stmt::Let(local, value)));
            } else {
                args.push(std::mem::replace(value, Self::error_expr(span)));
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
    fn argument_value(
        &mut self,
        cx: &mut FnCx,
        arg: &ast::Arg,
        expected: Option<Ty>,
        receiver: Option<ast::Mode>,
    ) -> hir::Expr {
        let loan = matches!(receiver, Some(ast::Mode::Mut | ast::Mode::Var));
        let expected_mode = if loan { receiver.unwrap() } else { ast::Mode::Value };
        if arg.mode != expected_mode {
            let message = match (expected_mode, arg.mode) {
                (ast::Mode::Value, _) => format!("this parameter takes a value: remove `{}`", arg.mode.keyword()),
                (mode, _) => format!("the receiver is a `{0}` loan: write `{0} value`", mode.keyword()),
            };
            self.diags.error("E1102", arg.span, message);
        }
        self.expr(cx, &arg.value, expected)
    }

    /// An argument coerced to its final parameter type, or the loan of its
    /// place for a `mut self` or `var self` receiver.
    fn finish_argument(&mut self, cx: &mut FnCx, value: hir::Expr, ty: Ty, receiver: Option<ast::Mode>) -> hir::Expr {
        let value = self.coerce(value, ty);
        if !matches!(receiver, Some(ast::Mode::Mut | ast::Mode::Var)) || matches!(value.kind, hir::ExprKind::Loan(_)) {
            return value;
        }
        let span = value.span;
        match self.loan_place(cx, value, "this operation") {
            Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, span),
            None => Self::error_expr(span),
        }
    }

    /// The `var` place a value expression denotes, for an operation that
    /// changes it (11.6); reports values that are not such a place.
    fn loan_place(&mut self, cx: &mut FnCx, value: hir::Expr, operation: &str) -> Option<hir::Place> {
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
                _ => {
                    let message =
                        format!("{operation} changes its receiver, which must be a `var` binding or part of one");
                    self.diags.error("E1411", span, message);
                    return None;
                }
            }
        };
        steps.reverse();
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
        if !cx.mutable[local.0 as usize] {
            let name = &cx.locals[local.0 as usize].name;
            let message =
                format!("{operation} changes `{name}`, which is not declared with `var`: declare it as `var {name}`");
            self.diags.error("E1411", span, message);
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
        let right = self.coerce(right, left.ty);
        (left, right)
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
    fn operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
    ) -> (hir::Expr, hir::Expr) {
        if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let left = self.expr(cx, left, Some(right.ty));
            let left = self.coerce(left, right.ty);
            (left, right)
        } else {
            let left = self.expr(cx, left, expected);
            let right = self.expr(cx, right, Some(left.ty));
            let right = if left.ty == Ty::NEVER { right } else { self.coerce(right, left.ty) };
            (left, right)
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
            let message = format!("operator `{}` cannot be applied to {}", op.as_str(), this.display(ty));
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
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected);
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.operands(cx, left, right, None);
                if !self.types.is_equatable(left.ty) {
                    operator_error(self, left.ty);
                }
                (left, right, Ty::BOOL)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.operands(cx, left, right, None);
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
                (left, right, Ty::BOOL)
            }
            BinOp::In => {
                let container = self.expr(cx, right, None);
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
                return self.unsupported_expr(span, &format!("`{}` outside a `for` header", op.as_str()));
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
            let then = self.block(cx, then, Some(Ty::UNIT));
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
