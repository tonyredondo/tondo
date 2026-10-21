//! Mid-level IR: a control-flow graph per function.
//!
//! Reference counting is explicit: `Retain` and `Release` statements are
//! inserted during lowering, so code generation is a direct translation.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{ArrayOp, Callee, MapMutation, MapOp};
use crate::source::Span;
use crate::types::{Ty, TyKind, Types};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// One step of a place: a tuple element or record field, or a payload
/// component of an enum value known to hold that variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Proj {
    Field(u32),
    Variant(u32, u32),
    /// An array element at the index held by the local; out-of-bounds
    /// accesses panic at the span.
    Index(Local, Span),
}

/// A local or a path inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub local: Local,
    pub path: Vec<Proj>,
}

impl Place {
    pub fn local(local: Local) -> Place {
        Place { local, path: Vec::new() }
    }

    fn with(&self, proj: Proj) -> Place {
        let mut path = self.path.clone();
        path.push(proj);
        Place { local: self.local, path }
    }

    pub fn field(&self, index: u32) -> Place {
        self.with(Proj::Field(index))
    }

    pub fn variant_field(&self, variant: u32, index: u32) -> Place {
        self.with(Proj::Variant(variant, index))
    }

    pub fn index(&self, index: Local, span: Span) -> Place {
        self.with(Proj::Index(index, span))
    }
}

/// The type of the component a projection selects inside `ty`.
pub fn project(types: &Types, ty: Ty, proj: Proj) -> Ty {
    match proj {
        Proj::Field(index) => types.components(ty).expect("field of an aggregate")[index as usize],
        Proj::Variant(variant, index) => {
            types.variants(ty).expect("payload of an enum")[variant as usize][index as usize]
        }
        // Iteration also indexes maps (entries as `(K, V)`) and sets.
        Proj::Index(..) => match types.kind(types.repr(ty)) {
            TyKind::Array(element) | TyKind::Set(element) => *element,
            TyKind::Map(key, value) => {
                types.lookup(&TyKind::Tuple(vec![*key, *value])).expect("map entry type is interned")
            }
            _ => unreachable!("index of a non-collection"),
        },
    }
}

pub struct Body {
    pub name: String,
    pub locals: Vec<Ty>,
    pub params: Vec<Local>,
    /// The local that holds the result, unless it is `Unit` or `Never`.
    pub ret: Option<Local>,
    pub result: Ty,
    pub blocks: Vec<BlockData>,
}

impl Body {
    pub fn place_ty(&self, types: &Types, place: &Place) -> Ty {
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &proj| project(types, ty, proj))
    }
}

pub struct BlockData {
    pub stmts: Vec<Statement>,
    pub terminator: Terminator,
}

pub enum Statement {
    Assign(Place, Box<Rvalue>, Span),
    /// Takes one more reference to every counted value inside the place.
    Retain(Place),
    /// Releases every counted value inside the place.
    Release(Place),
}

#[derive(Clone, Debug)]
pub enum Operand {
    Copy(Place),
    Const(Const),
}

impl Operand {
    pub fn local(local: Local) -> Operand {
        Operand::Copy(Place::local(local))
    }
}

#[derive(Clone, Debug)]
pub enum Const {
    Int(i128, Ty),
    /// The bits of an `f64`; `Float32` constants are exactly representable.
    Float(u64, Ty),
    Char(char),
    Bool(bool),
    Str(String),
    Unit,
}

pub enum Rvalue {
    Use(Operand),
    Unary(UnaryOp, Operand),
    /// Checked arithmetic, shifts, bitwise operators and comparisons
    /// (including structural equality). `and`/`or` are lowered to control flow.
    Binary(BinOp, Operand, Operand),
    /// Unchecked `+ 1`, used by range iteration after its bound check.
    Increment(Operand),
    Call(Callee, Vec<Operand>),
    Concat(Vec<Piece>),
    /// A tuple or record from its components in declaration order. The
    /// destination is always a fresh local, so components never alias it.
    Aggregate(Vec<Operand>),
    /// An enum value of the given variant, from its payload components.
    Variant(u32, Vec<Operand>),
    /// The variant index of an enum value, as the enum's tag type.
    Discriminant(Place),
    /// A numeric conversion (18.6) to the destination's type, or to
    /// `T ! NumericConversionError` when it can fail.
    Convert(Operand),
    /// `[a, b, c]`; a flag marks an element that was copied, not moved, and
    /// must be retained once stored.
    Array(Vec<Operand>, Vec<bool>),
    /// An intrinsic array operation, receiver first.
    ArrayOp(ArrayOp, Vec<Operand>, Span),
    /// `array[start:end:step]`.
    Slice(Operand, [Option<Operand>; 3], Span),
    /// Appends to the array at the place (made unique first); the flag
    /// retains a copied value.
    Push(Place, Operand, bool),
    /// Removes the last element of the array at the place.
    Pop(Place),
    /// The number of elements of an array, map or set.
    Length(Operand),
    /// A map or set query, collection first.
    MapOp(MapOp, Vec<Operand>, Span),
    /// `insert` or `remove` on the map or set at the place (made unique
    /// first); flags mark copied operands to retain as they move in.
    MapMutate(MapMutation, Place, Vec<Operand>, Vec<bool>),
}

pub enum Piece {
    Text(String),
    Value(Operand, Ty),
}

pub enum Terminator {
    Goto(BlockId),
    Branch(Operand, BlockId, BlockId),
    Return(Operand),
    Panic { code: u32, message: Option<Operand>, span: Span },
    Unreachable,
}
