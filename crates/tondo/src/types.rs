//! Interned semantic types.

pub use crate::ast::Mode;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ty(u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntTy {
    pub const ALL: [IntTy; 8] =
        [IntTy::I8, IntTy::I16, IntTy::I32, IntTy::I64, IntTy::U8, IntTy::U16, IntTy::U32, IntTy::U64];

    pub fn bits(self) -> u32 {
        match self {
            IntTy::I8 | IntTy::U8 => 8,
            IntTy::I16 | IntTy::U16 => 16,
            IntTy::I32 | IntTy::U32 => 32,
            IntTy::I64 | IntTy::U64 => 64,
        }
    }

    pub fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64)
    }

    pub fn min(self) -> i128 {
        if self.signed() { -(1i128 << (self.bits() - 1)) } else { 0 }
    }

    pub fn max(self) -> i128 {
        if self.signed() { (1i128 << (self.bits() - 1)) - 1 } else { (1i128 << self.bits()) - 1 }
    }

    pub fn name(self) -> &'static str {
        match self {
            IntTy::I8 => "Int8",
            IntTy::I16 => "Int16",
            IntTy::I32 => "Int32",
            IntTy::I64 => "Int",
            IntTy::U8 => "UInt8",
            IntTy::U16 => "UInt16",
            IntTy::U32 => "UInt32",
            IntTy::U64 => "UInt64",
        }
    }

    pub fn from_suffix(suffix: &str) -> IntTy {
        match suffix {
            "i8" => IntTy::I8,
            "i16" => IntTy::I16,
            "i32" => IntTy::I32,
            "i64" => IntTy::I64,
            "u8" => IntTy::U8,
            "u16" => IntTy::U16,
            "u32" => IntTy::U32,
            _ => IntTy::U64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyKind {
    /// Recovery type after an error; compatible with everything.
    Error,
    Unit,
    Never,
    Bool,
    String,
    Int(IntTy),
    Float(FloatTy),
    /// A Unicode scalar value.
    Char,
    /// A binary unit, distinct from `UInt8` (19.7).
    Byte,
    /// Two or more elements; the empty tuple is `Unit`.
    Tuple(Vec<Ty>),
    /// A nominal record, newtype or enum, declared in the program or by a
    /// standard module.
    Adt(AdtId),
    /// `T?`: variants `none` (0) and `some(T)` (1).
    Option(Ty),
    /// `T ! E`: variants `ok(T)` (0) and `err(E)` (1).
    Result(Ty, Ty),
    /// A reference-counted, copy-on-write sequence (10.2).
    Array(Ty),
    /// A copy-on-write sequence with cheap insertion at both ends (10.2.3).
    Deque(Ty),
    /// An insertion-ordered hash map (10.9).
    Map(Ty, Ty),
    /// An insertion-ordered set (10.15).
    Set(Ty),
    /// `A | B`: a closed anonymous union (9.5), represented as an enum with
    /// one variant per member. Built by `Types::union`, which normalizes it.
    Union(Vec<Ty>),
    /// A type parameter of a generic declaration (12.1). Generic code is
    /// checked once with parameters and monomorphized before lowering.
    Param(u32),
    /// `fn(A, mut B): R`: a uniform function value (11.7), with whether its
    /// last parameter is variadic and how each parameter is passed (16.3).
    /// Values are reference-counted callables.
    Fn(Vec<Ty>, Ty, bool, Vec<Mode>),
    /// A handle to a task or a channel (11.11 to 11.13): the task's result
    /// or the values the channel carries.
    Handle(Handle, Ty),
    /// The result `impl Bound` of a function (12.8) applied to the
    /// function's type arguments. Callers only know its bounds;
    /// monomorphization replaces it with the witness its body returns.
    Opaque(u32, Vec<Ty>),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Handle {
    Join,
    Group,
    Sender,
    Receiver,
    /// The `std.sync` values that hold shared state (14.4.4); the type is
    /// that of the state: `T`, `T?` for `Once`, or the value collection.
    Mutex,
    RwLock,
    Atomic,
    Once,
    SyncArray,
    SyncMap,
    SyncSet,
    SyncStack,
    SyncQueue,
    /// `std.sync` values without state; their type is `Unit`.
    Semaphore,
    Permit,
    Barrier,
    /// A lock taken on shared state, released when dropped; only lowering
    /// sees it.
    Guard,
    /// An open `fs.File`; its type is `Unit`.
    File,
    /// The shared streams of `std.io` and its memory buffer; their type is
    /// `Unit`.
    Writer,
    Reader,
    Buffer,
}

impl Handle {
    pub fn name(self) -> &'static str {
        match self {
            Handle::Join => "Join",
            Handle::Group => "Group",
            Handle::Sender => "Sender",
            Handle::Receiver => "Receiver",
            Handle::Mutex => "sync.Mutex",
            Handle::RwLock => "sync.RwLock",
            Handle::Atomic => "sync.Atomic",
            Handle::Once => "sync.Once",
            Handle::SyncArray => "sync.Array",
            Handle::SyncMap => "sync.Map",
            Handle::SyncSet => "sync.Set",
            Handle::SyncStack => "sync.Stack",
            Handle::SyncQueue => "sync.Queue",
            Handle::Semaphore => "sync.Semaphore",
            Handle::Permit => "sync.Permit",
            Handle::Barrier => "sync.Barrier",
            Handle::Guard => "lock",
            Handle::File => "fs.File",
            Handle::Writer => "io.Writer",
            Handle::Reader => "io.Reader",
            Handle::Buffer => "io.Buffer",
        }
    }

    /// A handle to state shared by all its copies: a `std.sync` value
    /// other than `Permit`, a `Receiver`, or a stream or buffer of `std.io`.
    pub fn shared(self) -> bool {
        !matches!(self, Handle::Join | Handle::Group | Handle::Sender | Handle::Permit | Handle::Guard | Handle::File)
    }

    /// One of the five shared collections of `std.sync`.
    pub fn collection(self) -> bool {
        matches!(self, Handle::SyncArray | Handle::SyncMap | Handle::SyncSet | Handle::SyncStack | Handle::SyncQueue)
    }

    /// Whether its state lives in a lockable `std.sync` cell.
    pub fn locked(self) -> bool {
        matches!(
            self,
            Handle::Mutex
                | Handle::RwLock
                | Handle::Atomic
                | Handle::Once
                | Handle::SyncArray
                | Handle::SyncMap
                | Handle::SyncSet
                | Handle::SyncStack
                | Handle::SyncQueue
        )
    }
}

/// An opaque result: its bounds are those of `carrier`, a type parameter
/// declared for it, and `witness` is the concrete type, written in terms of
/// the function's type parameters, once its body is checked.
pub struct OpaqueDef {
    pub name: String,
    pub generics: Vec<Ty>,
    pub carrier: Ty,
    pub witness: Option<Ty>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FloatTy {
    F32,
    F64,
}

/// Variant indices of the intrinsic enums.
pub const NONE: u32 = 0;
pub const SOME: u32 = 1;
pub const OK: u32 = 0;
pub const ERR: u32 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdtId(pub u32);

pub struct AdtDef {
    pub name: String,
    pub kind: AdtKind,
    /// For an instance of a generic type: its template and arguments.
    pub instance: Option<(TemplateId, Vec<Ty>)>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TemplateId(pub u32);

/// A generic record, enum or newtype. Each instantiation, such as
/// `Pair[Int, String]`, is an ordinary nominal type whose definition is the
/// template's with the arguments substituted.
pub struct Template {
    pub name: String,
    pub params: Vec<Ty>,
    /// Filled in once the declaration is resolved.
    pub kind: Option<AdtKind>,
}

/// The body and signature of a closure expression (11.8). Its type is an
/// anonymous record of captures, generic over the enclosing function's type
/// parameters; `params` and `result` may mention them.
#[derive(Clone)]
pub struct ClosureInfo {
    /// The `FnId` of the body, whose first parameter is the closure itself.
    pub body: u32,
    pub params: Vec<Ty>,
    pub result: Ty,
    pub variadic: bool,
    /// How each parameter is passed (16.3).
    pub modes: Vec<Mode>,
    /// Whether the body changes a capture, so calls need a `var` closure
    /// (`CallMut`) and it cannot become a `fn(...)` value.
    pub mutates: bool,
}

/// What an intrinsic cursor of a map or set yields (10.17): such cursors
/// implement `Iterator` without source.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CursorKind {
    Entries,
    Keys,
    Values,
}

/// A type parameter and the closed capabilities its bounds grant.
pub struct ParamDef {
    pub name: String,
    pub equatable: bool,
    pub key: bool,
    /// `Copy` (8.10).
    pub copy: bool,
}

#[derive(Clone)]
pub enum AdtKind {
    Record(Vec<Field>),
    /// `type Name = T`: a nominal wrapper with the same representation as `T`.
    Newtype(Ty),
    Enum(Vec<Variant>),
}

#[derive(Clone)]
pub struct Variant {
    pub name: String,
    pub payload: Payload,
}

#[derive(Clone)]
pub enum Payload {
    Unit,
    Tuple(Vec<Ty>),
    Record(Vec<Field>),
}

impl Payload {
    pub fn types(&self) -> Vec<Ty> {
        match self {
            Payload::Unit => Vec::new(),
            Payload::Tuple(items) => items.clone(),
            Payload::Record(fields) => fields.iter().map(|f| f.ty).collect(),
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
}

pub struct Types {
    kinds: Vec<TyKind>,
    map: HashMap<TyKind, Ty>,
    adts: Vec<AdtDef>,
    params: Vec<ParamDef>,
    templates: Vec<Template>,
    instances: HashMap<(TemplateId, Vec<Ty>), Ty>,
    /// Instances created before their template was defined.
    pending: Vec<AdtId>,
    closures: HashMap<TemplateId, ClosureInfo>,
    cursors: HashMap<TemplateId, CursorKind>,
    opaques: Vec<OpaqueDef>,
    /// Set when an instantiation nests without bound, such as a type whose
    /// field is the same type applied to a larger argument.
    pub runaway: Option<String>,
    reaches: RefCell<HashMap<(Ty, Ty), bool>>,
    needs_drop: RefCell<HashMap<Ty, bool>>,
    /// Standard library types with an intrinsic `Display`.
    pub shown: HashMap<Ty, Shown>,
}

/// How a standard library type displays.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// An error enum: its variant in words, such as `not found`.
    Words,
    /// `std.time.Duration`, as Go writes it.
    Duration,
    /// `std.time.DateTime`, in RFC 3339.
    DateTime,
}

/// Instantiation depth beyond which a generic type is considered to grow
/// without bound.
const MAX_TYPE_DEPTH: usize = 40;

impl Ty {
    pub const ERROR: Ty = Ty(0);
    pub const UNIT: Ty = Ty(1);
    pub const NEVER: Ty = Ty(2);
    pub const BOOL: Ty = Ty(3);
    pub const STRING: Ty = Ty(4);
    pub const INT: Ty = Ty(5 + 3);
    pub const CHAR: Ty = Ty(13);
    pub const BYTE: Ty = Ty(14);
    pub const F32: Ty = Ty(15);
    pub const F64: Ty = Ty(16);

    pub fn int(int: IntTy) -> Ty {
        Ty(5 + IntTy::ALL.iter().position(|&i| i == int).unwrap() as u32)
    }
}

impl Default for Types {
    fn default() -> Types {
        let mut types = Types {
            kinds: Vec::new(),
            map: HashMap::new(),
            adts: Vec::new(),
            params: Vec::new(),
            templates: Vec::new(),
            instances: HashMap::new(),
            pending: Vec::new(),
            closures: HashMap::new(),
            cursors: HashMap::new(),
            opaques: Vec::new(),
            runaway: None,
            reaches: RefCell::default(),
            needs_drop: RefCell::default(),
            shown: HashMap::new(),
        };
        for kind in [TyKind::Error, TyKind::Unit, TyKind::Never, TyKind::Bool, TyKind::String] {
            types.intern(kind);
        }
        for int in IntTy::ALL {
            types.intern(TyKind::Int(int));
        }
        for kind in [TyKind::Char, TyKind::Byte, TyKind::Float(FloatTy::F32), TyKind::Float(FloatTy::F64)] {
            types.intern(kind);
        }
        types
    }
}

impl Types {
    pub fn intern(&mut self, kind: TyKind) -> Ty {
        if let Some(&ty) = self.map.get(&kind) {
            return ty;
        }
        let ty = Ty(self.kinds.len() as u32);
        self.kinds.push(kind.clone());
        self.map.insert(kind, ty);
        ty
    }

    /// The union of `members` (9.5): nested unions are flattened, `Never`
    /// and duplicates are removed, and the order is canonical, so `A | B`
    /// and `B | A` are the same type. A single member is the member itself.
    pub fn union(&mut self, members: &[Ty]) -> Ty {
        let mut flat = Vec::new();
        for &member in members {
            match self.kind(member) {
                TyKind::Union(inner) => flat.extend(inner.iter().copied()),
                TyKind::Never => {}
                _ => flat.push(member),
            }
        }
        flat.sort_by_cached_key(|&t| (self.display(t), t.0));
        flat.dedup();
        match flat.as_slice() {
            [] => Ty::NEVER,
            [single] => *single,
            _ => self.intern(TyKind::Union(flat)),
        }
    }

    /// The members of a union, or `None` for other types.
    pub fn members(&self, ty: Ty) -> Option<&[Ty]> {
        match self.kind(ty) {
            TyKind::Union(members) => Some(members),
            _ => None,
        }
    }

    /// An already interned type.
    pub fn lookup(&self, kind: &TyKind) -> Option<Ty> {
        self.map.get(kind).copied()
    }

    pub fn kind(&self, ty: Ty) -> &TyKind {
        &self.kinds[ty.0 as usize]
    }

    /// Declares a nominal type; its definition is filled in by `define_adt`.
    pub fn declare_adt(&mut self, name: String) -> (AdtId, Ty) {
        let id = AdtId(self.adts.len() as u32);
        self.adts.push(AdtDef { name, kind: AdtKind::Record(Vec::new()), instance: None });
        (id, self.intern(TyKind::Adt(id)))
    }

    pub fn define_adt(&mut self, id: AdtId, kind: AdtKind) {
        self.adts[id.0 as usize].kind = kind;
        self.reaches.borrow_mut().clear();
        self.needs_drop.borrow_mut().clear();
    }

    pub fn adt(&self, id: AdtId) -> &AdtDef {
        &self.adts[id.0 as usize]
    }

    /// The name a pattern uses for a nominal type: a generic type's own
    /// name for its instances.
    pub fn pattern_name(&self, id: AdtId) -> &str {
        match &self.adts[id.0 as usize].instance {
            Some((template, _)) => &self.templates[template.0 as usize].name,
            None => &self.adts[id.0 as usize].name,
        }
    }

    pub fn declare_param(&mut self, name: String) -> Ty {
        self.params.push(ParamDef { name, equatable: false, key: false, copy: false });
        self.intern(TyKind::Param(self.params.len() as u32 - 1))
    }

    pub fn param(&self, ty: Ty) -> Option<&ParamDef> {
        match self.kind(ty) {
            TyKind::Param(index) => Some(&self.params[*index as usize]),
            _ => None,
        }
    }

    pub fn param_mut(&mut self, ty: Ty) -> &mut ParamDef {
        let TyKind::Param(index) = *self.kind(ty) else { panic!("not a type parameter") };
        &mut self.params[index as usize]
    }

    pub fn declare_template(&mut self, name: String, params: Vec<Ty>) -> TemplateId {
        self.templates.push(Template { name, params, kind: None });
        TemplateId(self.templates.len() as u32 - 1)
    }

    pub fn template(&self, id: TemplateId) -> &Template {
        &self.templates[id.0 as usize]
    }

    /// Defines a template and every instance already created from it.
    pub fn define_template(&mut self, id: TemplateId, kind: AdtKind) {
        self.templates[id.0 as usize].kind = Some(kind);
        let pending: Vec<AdtId> = std::mem::take(&mut self.pending);
        for adt in pending {
            match self.adts[adt.0 as usize].instance {
                Some((template, _)) if template == id => self.define_instance(adt),
                _ => self.pending.push(adt),
            }
        }
    }

    pub fn declare_opaque(&mut self, name: String, generics: Vec<Ty>, carrier: Ty) -> Ty {
        self.opaques.push(OpaqueDef { name, generics: generics.clone(), carrier, witness: None });
        self.intern(TyKind::Opaque(self.opaques.len() as u32 - 1, generics))
    }

    pub fn opaque(&self, ty: Ty) -> Option<(&OpaqueDef, &[Ty])> {
        match self.kind(ty) {
            TyKind::Opaque(id, args) => Some((&self.opaques[*id as usize], args)),
            _ => None,
        }
    }

    pub fn set_witness(&mut self, ty: Ty, witness: Ty) {
        let TyKind::Opaque(id, _) = *self.kind(ty) else { panic!("not an opaque type") };
        self.opaques[id as usize].witness = Some(witness);
    }

    /// `ty` with every opaque result replaced by its witness, for
    /// monomorphized code.
    pub fn reveal(&mut self, ty: Ty) -> Ty {
        let mut found = Vec::new();
        self.opaques_in(ty, &mut found);
        if found.is_empty() {
            return ty;
        }
        let mut map = HashMap::new();
        for opaque in found {
            let TyKind::Opaque(id, args) = self.kind(opaque).clone() else { unreachable!() };
            let def = &self.opaques[id as usize];
            let (generics, witness) = (def.generics.clone(), def.witness.expect("checked bodies have witnesses"));
            let inner: HashMap<Ty, Ty> = generics.into_iter().zip(args).collect();
            let witness = self.subst(witness, &inner);
            let witness = self.reveal(witness);
            map.insert(opaque, witness);
        }
        self.replace(ty, &map)
    }

    fn opaques_in(&self, ty: Ty, out: &mut Vec<Ty>) {
        match self.kind(ty) {
            TyKind::Opaque(..) => out.push(ty),
            TyKind::Tuple(items) | TyKind::Union(items) => items.iter().for_each(|&t| self.opaques_in(t, out)),
            TyKind::Fn(params, result, ..) => {
                params.iter().for_each(|&t| self.opaques_in(t, out));
                self.opaques_in(*result, out);
            }
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => {
                self.opaques_in(*t, out)
            }
            TyKind::Result(a, b) | TyKind::Map(a, b) => {
                self.opaques_in(*a, out);
                self.opaques_in(*b, out);
            }
            TyKind::Adt(id) => {
                if let Some((_, args)) = &self.adts[id.0 as usize].instance {
                    args.iter().for_each(|&t| self.opaques_in(t, out));
                }
            }
            _ => {}
        }
    }

    /// `ty` with whole types replaced according to `map`.
    fn replace(&mut self, ty: Ty, map: &HashMap<Ty, Ty>) -> Ty {
        if let Some(&to) = map.get(&ty) {
            return to;
        }
        let each =
            |types: &mut Self, items: &[Ty]| -> Vec<Ty> { items.iter().map(|&t| types.replace(t, map)).collect() };
        match self.kind(ty).clone() {
            TyKind::Tuple(items) => {
                let items = each(self, &items);
                self.intern(TyKind::Tuple(items))
            }
            TyKind::Union(items) => {
                let items = each(self, &items);
                self.union(&items)
            }
            TyKind::Fn(params, result, variadic, modes) => {
                let params = each(self, &params);
                let result = self.replace(result, map);
                self.intern(TyKind::Fn(params, result, variadic, modes))
            }
            TyKind::Option(t) => {
                let t = self.replace(t, map);
                self.intern(TyKind::Option(t))
            }
            TyKind::Array(t) => {
                let t = self.replace(t, map);
                self.intern(TyKind::Array(t))
            }
            TyKind::Deque(t) => {
                let t = self.replace(t, map);
                self.intern(TyKind::Deque(t))
            }
            TyKind::Set(t) => {
                let t = self.replace(t, map);
                self.intern(TyKind::Set(t))
            }
            TyKind::Handle(handle, t) => {
                let t = self.replace(t, map);
                self.handle(handle, t)
            }
            TyKind::Result(a, b) => {
                let (a, b) = (self.replace(a, map), self.replace(b, map));
                self.intern(TyKind::Result(a, b))
            }
            TyKind::Map(a, b) => {
                let (a, b) = (self.replace(a, map), self.replace(b, map));
                self.intern(TyKind::Tuple(vec![a, b]));
                self.intern(TyKind::Option(b));
                self.intern(TyKind::Map(a, b))
            }
            TyKind::Adt(id) => match self.adts[id.0 as usize].instance.clone() {
                Some((template, args)) => {
                    let args = each(self, &args);
                    self.instantiate(template, args)
                }
                None => ty,
            },
            _ => ty,
        }
    }

    pub fn declare_cursor(&mut self, template: TemplateId, kind: CursorKind) {
        self.cursors.insert(template, kind);
    }

    /// What the intrinsic cursor of type `ty` yields, if it is one.
    pub fn cursor(&self, ty: Ty) -> Option<CursorKind> {
        self.cursors.get(&self.instance_of(ty)?.0).copied()
    }

    pub fn declare_closure(&mut self, template: TemplateId, info: ClosureInfo) {
        self.closures.insert(template, info);
    }

    pub fn closure_info_mut(&mut self, template: TemplateId) -> &mut ClosureInfo {
        self.closures.get_mut(&template).expect("a closure template")
    }

    /// For the type of a closure: its body and its signature with the
    /// enclosing type parameters replaced by the instance's arguments.
    pub fn closure(&mut self, ty: Ty) -> Option<(u32, Vec<Ty>, ClosureInfo)> {
        let (template, args) = self.instance_of(ty)?;
        let mut info = self.closures.get(&template)?.clone();
        let args = args.to_vec();
        let map: HashMap<Ty, Ty> =
            self.templates[template.0 as usize].params.iter().copied().zip(args.iter().copied()).collect();
        info.params = info.params.iter().map(|&p| self.subst(p, &map)).collect();
        info.result = self.subst(info.result, &map);
        Some((info.body, args, info))
    }

    /// `Join[T]`, `Group[T]`, `Sender[T]` or `Receiver[T]`. Lowering a
    /// group's `wait` needs `T?` too.
    pub fn handle(&mut self, handle: Handle, ty: Ty) -> Ty {
        if handle == Handle::Group {
            self.intern(TyKind::Option(ty));
        }
        self.intern(TyKind::Handle(handle, ty))
    }

    /// The result of a task handle's type, `T` in `Join[T]`.
    pub fn handle_result(&self, ty: Ty) -> Ty {
        match self.kind(ty) {
            TyKind::Handle(_, result) => *result,
            _ => Ty::ERROR,
        }
    }

    /// The template and arguments of an instance of a generic type.
    pub fn instance_of(&self, ty: Ty) -> Option<(TemplateId, &[Ty])> {
        match self.kind(ty) {
            TyKind::Adt(id) => self.adts[id.0 as usize].instance.as_ref().map(|(t, args)| (*t, args.as_slice())),
            _ => None,
        }
    }

    /// `Template[args]`, created on first use.
    pub fn instantiate(&mut self, template: TemplateId, args: Vec<Ty>) -> Ty {
        if let Some(&ty) = self.instances.get(&(template, args.clone())) {
            return ty;
        }
        if args.iter().any(|&a| self.depth(a) > MAX_TYPE_DEPTH) {
            self.runaway.get_or_insert_with(|| self.templates[template.0 as usize].name.clone());
            return Ty::ERROR;
        }
        let shown: Vec<String> = args.iter().map(|&a| self.display(a)).collect();
        let name = format!("{}[{}]", self.templates[template.0 as usize].name, shown.join(", "));
        let (id, ty) = self.declare_adt(name);
        self.adts[id.0 as usize].instance = Some((template, args.clone()));
        self.instances.insert((template, args), ty);
        if self.templates[template.0 as usize].kind.is_some() {
            self.define_instance(id);
        } else {
            self.pending.push(id);
        }
        ty
    }

    fn define_instance(&mut self, id: AdtId) {
        let (template, args) = self.adts[id.0 as usize].instance.clone().unwrap();
        let template = &self.templates[template.0 as usize];
        let map: HashMap<Ty, Ty> = template.params.iter().copied().zip(args).collect();
        let kind = template.kind.clone().unwrap();
        let kind = match kind {
            AdtKind::Newtype(inner) => AdtKind::Newtype(self.subst(inner, &map)),
            AdtKind::Record(fields) => AdtKind::Record(self.subst_fields(fields, &map)),
            AdtKind::Enum(variants) => AdtKind::Enum(
                variants
                    .into_iter()
                    .map(|v| Variant {
                        name: v.name,
                        payload: match v.payload {
                            Payload::Unit => Payload::Unit,
                            Payload::Tuple(items) => {
                                Payload::Tuple(items.iter().map(|&t| self.subst(t, &map)).collect())
                            }
                            Payload::Record(fields) => Payload::Record(self.subst_fields(fields, &map)),
                        },
                    })
                    .collect(),
            ),
        };
        self.define_adt(id, kind);
    }

    fn subst_fields(&mut self, fields: Vec<Field>, map: &HashMap<Ty, Ty>) -> Vec<Field> {
        fields.into_iter().map(|f| Field { name: f.name, ty: self.subst(f.ty, map) }).collect()
    }

    /// `ty` with type parameters replaced according to `map`.
    pub fn subst(&mut self, ty: Ty, map: &HashMap<Ty, Ty>) -> Ty {
        if map.is_empty() {
            return ty;
        }
        let each = |types: &mut Self, items: &[Ty]| -> Vec<Ty> { items.iter().map(|&t| types.subst(t, map)).collect() };
        match self.kind(ty).clone() {
            TyKind::Param(_) => map.get(&ty).copied().unwrap_or(ty),
            TyKind::Tuple(items) => {
                let items = each(self, &items);
                self.intern(TyKind::Tuple(items))
            }
            TyKind::Option(inner) => {
                let inner = self.subst(inner, map);
                self.intern(TyKind::Option(inner))
            }
            TyKind::Result(ok, err) => {
                let (ok, err) = (self.subst(ok, map), self.subst(err, map));
                self.intern(TyKind::Result(ok, err))
            }
            TyKind::Array(element) => {
                let element = self.subst(element, map);
                self.intern(TyKind::Array(element))
            }
            TyKind::Deque(element) => {
                let element = self.subst(element, map);
                self.intern(TyKind::Deque(element))
            }
            TyKind::Set(element) => {
                let element = self.subst(element, map);
                self.intern(TyKind::Set(element))
            }
            TyKind::Handle(handle, element) => {
                let element = self.subst(element, map);
                self.handle(handle, element)
            }
            TyKind::Map(key, value) => {
                let (key, value) = (self.subst(key, map), self.subst(value, map));
                // Lowering needs the entry tuple and the `V?` of lookups.
                self.intern(TyKind::Tuple(vec![key, value]));
                self.intern(TyKind::Option(value));
                self.intern(TyKind::Map(key, value))
            }
            TyKind::Union(members) => {
                let members = each(self, &members);
                self.union(&members)
            }
            TyKind::Fn(params, result, variadic, modes) => {
                let params = each(self, &params);
                let result = self.subst(result, map);
                self.intern(TyKind::Fn(params, result, variadic, modes))
            }
            TyKind::Opaque(id, args) => {
                let args = each(self, &args);
                self.intern(TyKind::Opaque(id, args))
            }
            TyKind::Adt(id) => match self.adts[id.0 as usize].instance.clone() {
                Some((template, args)) => {
                    let args = each(self, &args);
                    self.instantiate(template, args)
                }
                None => ty,
            },
            _ => ty,
        }
    }

    /// Whether `pattern` matches `actual` when the parameters in `vars` are
    /// bound consistently in `map`; selects implementations (12.6).
    pub fn matches(&self, pattern: Ty, actual: Ty, vars: &[Ty], map: &mut HashMap<Ty, Ty>) -> bool {
        if vars.contains(&pattern) {
            return match map.get(&pattern) {
                Some(&bound) => bound == actual,
                None => {
                    map.insert(pattern, actual);
                    true
                }
            };
        }
        if pattern == actual {
            return true;
        }
        self.same_shape(pattern, actual, &mut |types, a, b| types.matches(a, b, vars, map))
    }

    /// Whether `a` and `b` become equal for some binding of the parameters
    /// in `vars` on either side; decides overlapping implementations (12.6).
    pub fn unify_both(&self, a: Ty, b: Ty, vars: &[Ty], map: &mut HashMap<Ty, Ty>) -> bool {
        let resolve = |mut ty: Ty, map: &HashMap<Ty, Ty>| {
            while let Some(&next) = map.get(&ty) {
                ty = next;
            }
            ty
        };
        let (a, b) = (resolve(a, map), resolve(b, map));
        if a == b {
            return true;
        }
        if vars.contains(&a) {
            map.insert(a, b);
            return true;
        }
        if vars.contains(&b) {
            map.insert(b, a);
            return true;
        }
        self.same_shape(a, b, &mut |types, x, y| types.unify_both(x, y, vars, map))
    }

    /// Whether `a` and `b` have the same outer constructor and `each` holds
    /// for every pair of corresponding components.
    fn same_shape(&self, a: Ty, b: Ty, each: &mut dyn FnMut(&Self, Ty, Ty) -> bool) -> bool {
        match (self.kind(a), self.kind(b)) {
            (TyKind::Tuple(x), TyKind::Tuple(y)) | (TyKind::Union(x), TyKind::Union(y)) if x.len() == y.len() => {
                x.iter().zip(y).all(|(&p, &q)| each(self, p, q))
            }
            (TyKind::Opaque(i, x), TyKind::Opaque(j, y)) if i == j => x.iter().zip(y).all(|(&p, &q)| each(self, p, q)),
            (TyKind::Fn(x, r, v, m), TyKind::Fn(y, s, w, n)) if x.len() == y.len() && v == w && m == n => {
                let (r, s) = (*r, *s);
                x.iter().zip(y).all(|(&p, &q)| each(self, p, q)) && each(self, r, s)
            }
            (TyKind::Option(x), TyKind::Option(y))
            | (TyKind::Array(x), TyKind::Array(y))
            | (TyKind::Deque(x), TyKind::Deque(y))
            | (TyKind::Set(x), TyKind::Set(y)) => each(self, *x, *y),
            (TyKind::Handle(h, x), TyKind::Handle(k, y)) if h == k => each(self, *x, *y),
            (TyKind::Result(p, q), TyKind::Result(r, t)) | (TyKind::Map(p, q), TyKind::Map(r, t)) => {
                let (p, q, r, t) = (*p, *q, *r, *t);
                each(self, p, r) && each(self, q, t)
            }
            (TyKind::Adt(_), TyKind::Adt(_)) => match (self.instance_of(a), self.instance_of(b)) {
                (Some((x, xs)), Some((y, ys))) if x == y => {
                    let pairs: Vec<(Ty, Ty)> = xs.iter().copied().zip(ys.iter().copied()).collect();
                    pairs.into_iter().all(|(p, q)| each(self, p, q))
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// The type parameters `ty` mentions.
    pub fn params_in(&self, ty: Ty, out: &mut Vec<Ty>) {
        match self.kind(ty) {
            TyKind::Param(_) => out.push(ty),
            TyKind::Tuple(items) | TyKind::Union(items) | TyKind::Opaque(_, items) => {
                items.iter().for_each(|&t| self.params_in(t, out))
            }
            TyKind::Fn(params, result, ..) => {
                params.iter().for_each(|&t| self.params_in(t, out));
                self.params_in(*result, out);
            }
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => {
                self.params_in(*t, out)
            }
            TyKind::Result(a, b) | TyKind::Map(a, b) => {
                self.params_in(*a, out);
                self.params_in(*b, out);
            }
            TyKind::Adt(id) => {
                if let Some((_, args)) = &self.adts[id.0 as usize].instance {
                    args.iter().for_each(|&t| self.params_in(t, out));
                }
            }
            _ => {}
        }
    }

    /// Whether `ty` mentions a type parameter.
    pub fn has_params(&self, ty: Ty) -> bool {
        match self.kind(ty) {
            TyKind::Param(_) => true,
            TyKind::Tuple(items) | TyKind::Union(items) | TyKind::Opaque(_, items) => {
                items.iter().any(|&t| self.has_params(t))
            }
            TyKind::Fn(params, result, ..) => params.iter().any(|&t| self.has_params(t)) || self.has_params(*result),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => {
                self.has_params(*t)
            }
            TyKind::Result(a, b) | TyKind::Map(a, b) => self.has_params(*a) || self.has_params(*b),
            TyKind::Adt(id) => self.adts[id.0 as usize]
                .instance
                .as_ref()
                .is_some_and(|(_, args)| args.iter().any(|&t| self.has_params(t))),
            _ => false,
        }
    }

    /// How deeply types nest inside `ty`, to bound instantiation.
    pub fn depth(&self, ty: Ty) -> usize {
        let children: Vec<Ty> = match self.kind(ty) {
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => vec![*t],
            TyKind::Result(a, b) | TyKind::Map(a, b) => vec![*a, *b],
            TyKind::Adt(id) => self.adts[id.0 as usize].instance.as_ref().map_or(Vec::new(), |(_, a)| a.clone()),
            _ => Vec::new(),
        };
        1 + children.iter().map(|&t| self.depth(t)).max().unwrap_or(0)
    }

    pub fn adt_count(&self) -> usize {
        self.adts.len()
    }

    /// Strips newtypes: they share the representation of their value.
    pub fn repr(&self, mut ty: Ty) -> Ty {
        while let TyKind::Adt(id) = self.kind(ty) {
            match &self.adt(*id).kind {
                AdtKind::Newtype(inner) => ty = *inner,
                AdtKind::Record(_) | AdtKind::Enum(_) => break,
            }
        }
        ty
    }

    /// Component types of a tuple or record, in declaration order.
    pub fn components(&self, ty: Ty) -> Option<Vec<Ty>> {
        match self.kind(self.repr(ty)) {
            TyKind::Tuple(items) => Some(items.clone()),
            TyKind::Adt(id) => match &self.adt(*id).kind {
                AdtKind::Record(fields) => Some(fields.iter().map(|f| f.ty).collect()),
                AdtKind::Newtype(_) => unreachable!("repr strips newtypes"),
                AdtKind::Enum(_) => None,
            },
            _ => None,
        }
    }

    /// Payload component types of each variant of an enum, including the
    /// intrinsic `Option` and `Result`.
    pub fn variants(&self, ty: Ty) -> Option<Vec<Vec<Ty>>> {
        match self.kind(self.repr(ty)) {
            TyKind::Option(inner) => Some(vec![Vec::new(), vec![*inner]]),
            TyKind::Result(ok, err) => Some(vec![vec![*ok], vec![*err]]),
            TyKind::Union(members) => Some(members.iter().map(|&m| vec![m]).collect()),
            TyKind::Adt(id) => match &self.adt(*id).kind {
                AdtKind::Enum(variants) => Some(variants.iter().map(|v| v.payload.types()).collect()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The type of an enum's discriminant.
    pub fn tag_ty(&self, ty: Ty) -> Ty {
        let count = self.variants(ty).map_or(0, |v| v.len());
        Ty::int(if count <= 256 { IntTy::U8 } else { IntTy::U32 })
    }

    /// Every type stored inline in a value of `ty`.
    pub fn inline_components(&self, ty: Ty) -> Vec<Ty> {
        match self.kind(ty) {
            TyKind::Tuple(items) => items.clone(),
            TyKind::Option(inner) => vec![*inner],
            TyKind::Result(ok, err) => vec![*ok, *err],
            TyKind::Union(members) => members.clone(),
            TyKind::Adt(id) => match &self.adt(*id).kind {
                AdtKind::Record(fields) => fields.iter().map(|f| f.ty).collect(),
                AdtKind::Newtype(inner) => vec![*inner],
                AdtKind::Enum(variants) => variants.iter().flat_map(|v| v.payload.types()).collect(),
            },
            _ => Vec::new(),
        }
    }

    /// Whether a value of `to` can be stored inline, transitively, in `from`.
    pub fn reaches(&self, from: Ty, to: Ty) -> bool {
        if let Some(&known) = self.reaches.borrow().get(&(from, to)) {
            return known;
        }
        let mut seen = HashSet::new();
        let mut stack = self.inline_components(from);
        let mut found = false;
        while let Some(ty) = stack.pop() {
            if ty == to {
                found = true;
                break;
            }
            if seen.insert(ty) {
                stack.extend(self.inline_components(ty));
            }
        }
        self.reaches.borrow_mut().insert((from, to), found);
        found
    }

    /// Whether `component`, stored in `container`, lives in a box. Every
    /// cycle of inline storage passes through a nominal type, so boxing the
    /// edges into nominal types of the same cycle makes every layout finite
    /// (9.10).
    pub fn is_boxed(&self, container: Ty, component: Ty) -> bool {
        matches!(self.kind(component), TyKind::Adt(_)) && self.reaches(component, container)
    }

    /// Whether values of this type own reference-counted storage.
    pub fn needs_drop(&self, ty: Ty) -> bool {
        if let Some(&known) = self.needs_drop.borrow().get(&ty) {
            return known;
        }
        let repr = self.repr(ty);
        let heap = matches!(
            self.kind(repr),
            TyKind::String
                | TyKind::Array(_)
                | TyKind::Deque(_)
                | TyKind::Map(..)
                | TyKind::Set(_)
                | TyKind::Fn(..)
                | TyKind::Handle(..)
        );
        let result = heap || {
            let parts = match (self.components(repr), self.variants(repr)) {
                (Some(items), _) => items,
                (None, Some(variants)) => variants.concat(),
                (None, None) => Vec::new(),
            };
            // Boxed components are counted; checking them first also stops
            // the recursion at every cycle.
            parts.iter().any(|&part| self.is_boxed(repr, part) || self.needs_drop(part))
        };
        self.needs_drop.borrow_mut().insert(ty, result);
        result
    }

    /// The closed `Key` capability (8.12): equality is total and hashing is
    /// stable, so floats and collections are excluded.
    pub fn is_key(&self, ty: Ty) -> bool {
        self.key_in(ty, &mut Vec::new())
    }

    fn key_in(&self, ty: Ty, visiting: &mut Vec<Ty>) -> bool {
        if visiting.contains(&ty) {
            return true;
        }
        visiting.push(ty);
        let repr = self.repr(ty);
        let result = match self.kind(repr) {
            TyKind::Int(_)
            | TyKind::Char
            | TyKind::Byte
            | TyKind::Bool
            | TyKind::String
            | TyKind::Unit
            | TyKind::Never
            | TyKind::Error => true,
            TyKind::Float(_)
            | TyKind::Array(_)
            | TyKind::Deque(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Fn(..)
            | TyKind::Handle(..) => false,
            TyKind::Param(index) => self.params[*index as usize].key,
            TyKind::Opaque(id, _) => self.key_in(self.opaques[*id as usize].carrier, visiting),
            _ => match (self.components(repr), self.variants(repr)) {
                (Some(items), _) => items.iter().all(|&t| self.key_in(t, visiting)),
                (None, Some(variants)) => variants.concat().iter().all(|&t| self.key_in(t, visiting)),
                (None, None) => false,
            },
        };
        visiting.pop();
        result
    }

    /// The closed `Copy` capability (8.10): values are duplicated implicitly
    /// and using them never moves them. In 0.1 only type parameters without
    /// it (`~Copy`, or a type's or trait's), and composites holding them,
    /// are affine.
    pub fn is_copy(&self, ty: Ty) -> bool {
        self.copy_in(ty, &mut Vec::new())
    }

    /// `Copy`, structural and computed coinductively: a type parameter has
    /// what it declares; a handle is `Copy` only when it is shared.
    fn copy_in(&self, ty: Ty, visiting: &mut Vec<Ty>) -> bool {
        if visiting.contains(&ty) {
            return true;
        }
        visiting.push(ty);
        let repr = self.repr(ty);
        let parts: Vec<Ty> = match self.kind(repr) {
            TyKind::Param(index) => {
                visiting.pop();
                return self.params[*index as usize].copy;
            }
            TyKind::Handle(handle, _) if handle.shared() => Vec::new(),
            TyKind::Handle(..) => {
                visiting.pop();
                return false;
            }
            TyKind::Opaque(id, _) => vec![self.opaques[*id as usize].carrier],
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) => vec![*t],
            TyKind::Map(a, b) | TyKind::Result(a, b) => vec![*a, *b],
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            _ => match (self.components(repr), self.variants(repr)) {
                (Some(items), _) => items,
                (None, Some(variants)) => variants.concat(),
                (None, None) => Vec::new(),
            },
        };
        let result = parts.iter().all(|&t| self.copy_in(t, visiting));
        visiting.pop();
        result
    }

    /// The closed `Equatable` capability (8.11), computed coinductively for
    /// recursive types.
    pub fn is_equatable(&self, ty: Ty) -> bool {
        self.equatable_in(ty, &mut Vec::new())
    }

    fn equatable_in(&self, ty: Ty, visiting: &mut Vec<Ty>) -> bool {
        if visiting.contains(&ty) {
            return true;
        }
        visiting.push(ty);
        let repr = self.repr(ty);
        let result = match self.kind(repr) {
            TyKind::Int(_)
            | TyKind::Float(_)
            | TyKind::Char
            | TyKind::Byte
            | TyKind::Bool
            | TyKind::String
            | TyKind::Unit
            | TyKind::Never
            | TyKind::Error => true,
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => {
                self.equatable_in(*element, visiting)
            }
            TyKind::Map(key, value) => self.equatable_in(*key, visiting) && self.equatable_in(*value, visiting),
            TyKind::Param(index) => self.params[*index as usize].equatable || self.params[*index as usize].key,
            TyKind::Fn(..) | TyKind::Handle(..) => false,
            TyKind::Opaque(id, _) => self.equatable_in(self.opaques[*id as usize].carrier, visiting),
            _ => match (self.components(repr), self.variants(repr)) {
                (Some(items), _) => items.iter().all(|&t| self.equatable_in(t, visiting)),
                (None, Some(variants)) => variants.concat().iter().all(|&t| self.equatable_in(t, visiting)),
                (None, None) => false,
            },
        };
        visiting.pop();
        result
    }

    /// Whether a type mentions the recovery type of an earlier error.
    pub fn has_error(&self, ty: Ty) -> bool {
        match self.kind(ty) {
            TyKind::Error => true,
            TyKind::Tuple(items) => items.iter().any(|&t| self.has_error(t)),
            TyKind::Option(inner) => self.has_error(*inner),
            TyKind::Result(ok, err) => self.has_error(*ok) || self.has_error(*err),
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::Handle(_, element) => {
                self.has_error(*element)
            }
            TyKind::Map(key, value) => self.has_error(*key) || self.has_error(*value),
            TyKind::Union(members) => members.iter().any(|&t| self.has_error(t)),
            TyKind::Fn(params, result, ..) => params.iter().any(|&t| self.has_error(t)) || self.has_error(*result),
            _ => false,
        }
    }

    pub fn as_int(&self, ty: Ty) -> Option<IntTy> {
        match self.kind(ty) {
            TyKind::Int(int) => Some(*int),
            _ => None,
        }
    }

    pub fn display(&self, ty: Ty) -> String {
        match self.kind(ty) {
            TyKind::Error => "{error}".into(),
            TyKind::Unit => "Unit".into(),
            TyKind::Never => "Never".into(),
            TyKind::Bool => "Bool".into(),
            TyKind::String => "String".into(),
            TyKind::Int(int) => int.name().into(),
            TyKind::Float(FloatTy::F32) => "Float32".into(),
            TyKind::Float(FloatTy::F64) => "Float".into(),
            TyKind::Char => "Char".into(),
            TyKind::Byte => "Byte".into(),
            TyKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|&t| self.display(t)).collect();
                format!("({})", items.join(", "))
            }
            TyKind::Adt(id) => match &self.adt(*id).instance {
                // A closure is shown by its signature.
                Some((template, _)) if self.closures.contains_key(template) => {
                    let info = &self.closures[template];
                    let signature = TyKind::Fn(info.params.clone(), info.result, info.variadic, info.modes.clone());
                    match self.map.get(&signature) {
                        Some(&ty) => format!("closure {}", self.display(ty)),
                        None => "closure".into(),
                    }
                }
                _ => self.adt(*id).name.clone(),
            },
            TyKind::Option(inner) => match self.kind(*inner) {
                TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) => format!("({})?", self.display(*inner)),
                _ => format!("{}?", self.display(*inner)),
            },
            TyKind::Array(element) => format!("Array[{}]", self.display(*element)),
            TyKind::Deque(element) => format!("Deque[{}]", self.display(*element)),
            TyKind::Map(key, value) => format!("Map[{}, {}]", self.display(*key), self.display(*value)),
            TyKind::Set(element) => format!("Set[{}]", self.display(*element)),
            TyKind::Handle(handle, state) => {
                let shown = match (handle, self.kind(*state)) {
                    (
                        Handle::Semaphore
                        | Handle::Permit
                        | Handle::Barrier
                        | Handle::Guard
                        | Handle::File
                        | Handle::Writer
                        | Handle::Reader
                        | Handle::Buffer,
                        _,
                    ) => {
                        return handle.name().to_string();
                    }
                    (Handle::SyncMap, TyKind::Map(key, value)) => {
                        format!("{}, {}", self.display(*key), self.display(*value))
                    }
                    (
                        Handle::Once | Handle::SyncArray | Handle::SyncSet | Handle::SyncStack | Handle::SyncQueue,
                        TyKind::Option(t) | TyKind::Array(t) | TyKind::Set(t) | TyKind::Deque(t),
                    ) => self.display(*t),
                    _ => self.display(*state),
                };
                format!("{}[{shown}]", handle.name())
            }
            TyKind::Result(ok, err) => {
                // A union in either position is grouped (15.1).
                let grouped = |t: Ty| match self.kind(t) {
                    TyKind::Union(_) => format!("({})", self.display(t)),
                    _ => self.display(t),
                };
                if *ok == Ty::UNIT {
                    format!("!{}", grouped(*err))
                } else {
                    format!("{} ! {}", grouped(*ok), grouped(*err))
                }
            }
            TyKind::Union(members) => members.iter().map(|&t| self.display(t)).collect::<Vec<_>>().join(" | "),
            TyKind::Param(index) => self.params[*index as usize].name.clone(),
            TyKind::Opaque(id, _) => self.opaques[*id as usize].name.clone(),
            TyKind::Fn(params, result, variadic, modes) => {
                let mut shown: Vec<String> = params
                    .iter()
                    .zip(modes)
                    .map(|(&t, mode)| match mode {
                        Mode::Value => self.display(t),
                        mode => format!("{} {}", mode.keyword(), self.display(t)),
                    })
                    .collect();
                if *variadic && let Some(last) = shown.last_mut() {
                    let element = match self.kind(*params.last().unwrap()) {
                        TyKind::Array(element) => self.display(*element),
                        _ => last.clone(),
                    };
                    *last = format!("...{element}");
                }
                match *result {
                    Ty::UNIT => format!("fn({})", shown.join(", ")),
                    _ => format!("fn({}): {}", shown.join(", "), self.display(*result)),
                }
            }
        }
    }
}
