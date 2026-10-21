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
}

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
    reaches: RefCell<HashMap<(Ty, Ty), bool>>,
    needs_drop: RefCell<HashMap<Ty, bool>>,
}

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

    pub fn kind(&self, ty: Ty) -> &TyKind {
        &self.kinds[ty.0 as usize]
    }

    /// Declares a nominal type; its definition is filled in by `define_adt`.
    pub fn declare_adt(&mut self, name: String) -> (AdtId, Ty) {
        let id = AdtId(self.adts.len() as u32);
        self.adts.push(AdtDef { name, kind: AdtKind::Record(Vec::new()) });
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
        let result = repr == Ty::STRING || {
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
                TyKind::Option(_) | TyKind::Result(..) => format!("({})?", self.display(*inner)),
                _ => format!("{}?", self.display(*inner)),
            },
            TyKind::Result(ok, err) if *ok == Ty::UNIT => format!("!{}", self.display(*err)),
            TyKind::Result(ok, err) => format!("{} ! {}", self.display(*ok), self.display(*err)),
        }
    }
}
