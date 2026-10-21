//! Mid-level IR: a control-flow graph per function.
//!
//! Reference counting is explicit: `Retain` and `Release` statements are
//! inserted during lowering, so code generation is a direct translation.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::Callee;
use crate::source::Span;
use crate::types::Ty;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

pub struct Body {
    pub name: String,
    pub locals: Vec<Ty>,
    pub params: Vec<Local>,
    pub result: Ty,
    pub blocks: Vec<BlockData>,
}

pub struct BlockData {
    pub stmts: Vec<Statement>,
    pub terminator: Terminator,
}

pub enum Statement {
    Assign(Local, Rvalue, Span),
    Retain(Local),
    Release(Local),
}

#[derive(Clone, Debug)]
pub enum Operand {
    Copy(Local),
    Const(Const),
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
    /// Checked arithmetic, shifts, bitwise operators and comparisons.
    /// `and`/`or` are lowered to control flow.
    Binary(BinOp, Operand, Operand),
    /// Unchecked `+ 1`, used by range iteration after its bound check.
    Increment(Operand),
    Call(Callee, Vec<Operand>),
    Concat(Vec<Piece>),
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
