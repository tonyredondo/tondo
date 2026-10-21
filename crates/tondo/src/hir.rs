//! Typed, name-resolved program produced by the checker.

use crate::ast::{BinOp, UnaryOp};
use crate::source::Span;
use crate::types::{Ty, Types};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnId(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

pub struct Program {
    pub types: Types,
    pub functions: Vec<Function>,
    pub main: FnId,
}

pub struct Function {
    pub name: String,
    pub params: Vec<LocalId>,
    pub locals: Vec<Local>,
    pub result: Ty,
    pub body: Block,
}

pub struct Local {
    pub name: String,
    pub ty: Ty,
}

pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub ty: Ty,
}

pub enum Stmt {
    Let(LocalId, Expr),
    /// `let pattern = value` with an irrefutable pattern.
    Destructure(Pattern, Expr),
    /// Assignment to a binding or to a path inside it.
    Assign {
        local: LocalId,
        path: Vec<PlaceStep>,
        op: Option<BinOp>,
        value: Expr,
        span: Span,
    },
    /// An evaluated expression whose value is dropped (`_ =` or a `Unit` statement).
    Expr(Expr),
    Return(Option<Expr>),
    /// `fail error` in a fallible function.
    Fail(Expr),
    Break,
    Continue,
    /// `for {}` when `condition` is `None`, otherwise `for condition {}`.
    Loop {
        condition: Option<Expr>,
        body: Block,
    },
    /// `for local in array`: iterates a snapshot of the array by value.
    ForArray {
        local: LocalId,
        array: Expr,
        body: Block,
    },
    /// `for local in start..end` over an integer range.
    ForRange {
        local: LocalId,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Block,
    },
}

pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

pub enum ExprKind {
    Int(i128),
    Float(f64),
    Char(char),
    Bool(bool),
    Str(String),
    Unit,
    Local(LocalId),
    /// Tuple elements or record fields in declaration order.
    Aggregate(Vec<Expr>),
    /// A record literal: `(field index, value)` in source order, which is
    /// also the evaluation order (5.12).
    Record(Vec<(u32, Expr)>),
    /// A tuple element or record field.
    Field(Box<Expr>, u32),
    /// `base with { ... }`: updates in source order.
    With(Box<Expr>, Vec<(u32, Expr)>),
    /// Newtype construction or `.value`: same representation, new type.
    Retype(Box<Expr>),
    /// An enum value: the variant and its payload as `(component, value)`
    /// in source order.
    Variant(u32, Vec<(u32, Expr)>),
    Match(Box<Expr>, Vec<Arm>),
    /// `[a, b, c]`
    Array(Vec<Expr>),
    /// `array[index]`
    Index(Box<Expr>, Box<Expr>),
    /// `array[start:end:step]`
    Slice(Box<Expr>, [Option<Box<Expr>>; 3]),
    /// An intrinsic array operation; the receiver comes first.
    ArrayOp(ArrayOp, Vec<Expr>),
    /// `array.push(value)` on a `var` place.
    Push(Place, Box<Expr>),
    /// `array.pop()` on a `var` place.
    Pop(Place),
    /// `value?` on an option or result; the type is the payload's.
    Try(Box<Expr>),
    /// A numeric conversion (18.6). Its type is the target, or
    /// `Target ! NumericConversionError` when it can fail.
    Convert(Box<Expr>),
    Call(Callee, Vec<Expr>),
    Unary(UnaryOp, Box<Expr>),
    /// Arithmetic, bitwise and comparison operators; `and`/`or` short-circuit.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    Block(Block),
    Interpolate(Vec<Segment>),
    /// `panic(message)`
    Panic(Box<Expr>),
    /// `assert(condition, parts...)`; `source` is the condition's text.
    Assert {
        condition: Box<Expr>,
        parts: Vec<Expr>,
        source: String,
    },
    Error,
}

/// A binding and a path of fields and indices inside it.
pub struct Place {
    pub local: LocalId,
    pub path: Vec<PlaceStep>,
}

pub enum PlaceStep {
    Field(u32),
    Index(Expr),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArrayOp {
    /// `Array.new[T]()`
    New,
    /// `Array.withCapacity[T](capacity)`
    WithCapacity,
    Length,
    Get,
    /// The checked `slice(start, end)` method.
    SliceChecked,
    Concat,
    Repeat,
    /// `value in array`; the value comes first.
    Contains,
}

pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

pub struct Pattern {
    pub kind: PatKind,
    pub ty: Ty,
    pub span: Span,
}

pub enum PatKind {
    Wildcard,
    Binding(LocalId),
    Literal(Literal),
    /// Tuple elements or record fields, one pattern per component.
    Fields(Vec<Pattern>),
    /// An enum variant with one pattern per payload component.
    Variant(u32, Vec<Pattern>),
    /// `Newtype(pattern)`: the same value seen as the newtype's value.
    Newtype(Box<Pattern>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Int(i128),
    /// The bits of an `f64` (an `f32` literal is stored exactly as `f64`).
    Float(u64),
    Char(char),
    Bool(bool),
    Str(String),
    Unit,
}

pub enum Segment {
    Text(String),
    Value(Expr),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Callee {
    Fn(FnId),
    Std(StdFn),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StdFn {
    ConsolePrint,
    ConsolePrintln,
}
