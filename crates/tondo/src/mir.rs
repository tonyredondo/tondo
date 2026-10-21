//! Mid-level IR: a control-flow graph per function.
//!
//! Reference counting is explicit: `Retain` and `Release` statements are
//! inserted during lowering, so code generation is a direct translation.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::Callee;
use crate::source::Span;
use crate::types::{Ty, Types};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// A local or a field path inside it (tuple elements and record fields).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub local: Local,
    pub path: Vec<u32>,
}

impl Place {
    pub fn local(local: Local) -> Place {
        Place { local, path: Vec::new() }
    }

    pub fn field(&self, index: u32) -> Place {
        let mut path = self.path.clone();
        path.push(index);
        Place { local: self.local, path }
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
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &index| {
            types.components(ty).expect("field path through an aggregate")[index as usize]
        })
    }
}

pub struct BlockData {
    pub stmts: Vec<Statement>,
    pub terminator: Terminator,
}

pub enum Statement {
    Assign(Place, Rvalue, Span),
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
