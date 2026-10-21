//! Interned semantic types.

use std::collections::HashMap;

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

/// A nominal type provided by the standard library.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum StdType {
    ConsoleError,
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
    Result(Ty, Ty),
    Std(StdType),
}

pub struct Types {
    kinds: Vec<TyKind>,
    map: HashMap<TyKind, Ty>,
}

impl Ty {
    pub const ERROR: Ty = Ty(0);
    pub const UNIT: Ty = Ty(1);
    pub const NEVER: Ty = Ty(2);
    pub const BOOL: Ty = Ty(3);
    pub const STRING: Ty = Ty(4);
    pub const INT: Ty = Ty(5 + 3);

    pub fn int(int: IntTy) -> Ty {
        Ty(5 + IntTy::ALL.iter().position(|&i| i == int).unwrap() as u32)
    }
}

impl Default for Types {
    fn default() -> Types {
        let mut types = Types { kinds: Vec::new(), map: HashMap::new() };
        for kind in [TyKind::Error, TyKind::Unit, TyKind::Never, TyKind::Bool, TyKind::String] {
            types.intern(kind);
        }
        for int in IntTy::ALL {
            types.intern(TyKind::Int(int));
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
            TyKind::Result(ok, err) if *ok == Ty::UNIT => format!("!{}", self.display(*err)),
            TyKind::Result(ok, err) => format!("{} ! {}", self.display(*ok), self.display(*err)),
            TyKind::Std(StdType::ConsoleError) => "console.ConsoleError".into(),
        }
    }
}
