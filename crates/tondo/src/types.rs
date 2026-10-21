//! Interned semantic types.

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

/// A type parameter and the closed capabilities its bounds grant.
pub struct ParamDef {
    pub name: String,
    pub equatable: bool,
    pub key: bool,
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
    /// Set when an instantiation nests without bound, such as a type whose
    /// field is the same type applied to a larger argument.
    pub runaway: Option<String>,
    reaches: RefCell<HashMap<(Ty, Ty), bool>>,
    needs_drop: RefCell<HashMap<Ty, bool>>,
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
            runaway: None,
            reaches: RefCell::default(),
            needs_drop: RefCell::default(),
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
        self.params.push(ParamDef { name, equatable: false, key: false });
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

    /// The type parameters `ty` mentions.
    pub fn params_in(&self, ty: Ty, out: &mut Vec<Ty>) {
        match self.kind(ty) {
            TyKind::Param(_) => out.push(ty),
            TyKind::Tuple(items) | TyKind::Union(items) => items.iter().for_each(|&t| self.params_in(t, out)),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) => self.params_in(*t, out),
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
            TyKind::Tuple(items) | TyKind::Union(items) => items.iter().any(|&t| self.has_params(t)),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) => self.has_params(*t),
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
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) => vec![*t],
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
            TyKind::String | TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_)
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
            TyKind::Float(_) | TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_) => false,
            TyKind::Param(index) => self.params[*index as usize].key,
            _ => match (self.components(repr), self.variants(repr)) {
                (Some(items), _) => items.iter().all(|&t| self.key_in(t, visiting)),
                (None, Some(variants)) => variants.concat().iter().all(|&t| self.key_in(t, visiting)),
                (None, None) => false,
            },
        };
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
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => self.has_error(*element),
            TyKind::Map(key, value) => self.has_error(*key) || self.has_error(*value),
            TyKind::Union(members) => members.iter().any(|&t| self.has_error(t)),
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
            TyKind::Adt(id) => self.adt(*id).name.clone(),
            TyKind::Option(inner) => match self.kind(*inner) {
                TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) => format!("({})?", self.display(*inner)),
                _ => format!("{}?", self.display(*inner)),
            },
            TyKind::Array(element) => format!("Array[{}]", self.display(*element)),
            TyKind::Deque(element) => format!("Deque[{}]", self.display(*element)),
            TyKind::Map(key, value) => format!("Map[{}, {}]", self.display(*key), self.display(*value)),
            TyKind::Set(element) => format!("Set[{}]", self.display(*element)),
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
        }
    }
}
