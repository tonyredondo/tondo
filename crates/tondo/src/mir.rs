//! Mid-level IR: a control-flow graph per function.
//!
//! Reference counting is explicit: `Retain` and `Release` statements are
//! inserted during lowering, so code generation is a direct translation.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{self, ArrayOp, Callee, MapOp, Mutation, StrOp, TaskOp};
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
    /// An array element at an index that the loop it is read in keeps
    /// within bounds: `xs[i]` in `for i in 0..xs.length()`.
    InBounds(Local),
    /// The value a `Ref[T]` identifies, which is only read.
    Deref,
    /// The captures of a closure in a `fn` value, of the type given, which
    /// its body borrows (`inline`).
    Captures(Ty),
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

    /// An element at an index known to be within bounds.
    pub fn in_bounds(&self, index: Local) -> Place {
        self.with(Proj::InBounds(index))
    }

    pub fn deref(&self) -> Place {
        self.with(Proj::Deref)
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
        Proj::Index(..) | Proj::InBounds(_) => match types.kind(types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::CArray(element, _) => {
                *element
            }
            TyKind::Map(key, value) => {
                types.lookup(&TyKind::Tuple(vec![*key, *value])).expect("map entry type is interned")
            }
            _ => unreachable!("index of a non-collection"),
        },
        Proj::Deref => types.handle_result(ty),
        Proj::Captures(captures) => captures,
    }
}

#[derive(Clone)]
pub struct Body {
    pub name: String,
    pub locals: Vec<Ty>,
    /// The names of the program's own bindings, for a debugger; the
    /// compiler's temporaries have none.
    pub names: Vec<Option<String>>,
    pub params: Vec<Local>,
    /// Parameters passed as the address of the caller's place (a `mut self`
    /// or `var self` loan); reads and writes go through it.
    pub loans: Vec<bool>,
    /// The local that holds the result, unless it is `Unit` or `Never`.
    pub ret: Option<Local>,
    pub result: Ty,
    pub blocks: Vec<BlockData>,
    /// In a live build (toolchain 7.10): the hidden parameter that brings
    /// a frame's state from the old code, null in an ordinary call; the
    /// points where a frame can restart or move to new code; and where
    /// each local comes from, which names it across versions.
    pub osr: Option<Local>,
    pub points: Vec<Point>,
    pub origins: Vec<Origin>,
    /// In a live build, each wait, by where it is, and whether the
    /// statement it waits in can start again when a reload ends the wait.
    pub pauses: Vec<(Span, bool)>,
    /// Where the function starts, where each statement the program wrote
    /// starts to run (the statement of a block, and where the statement
    /// is), and each way its branches can go, which coverage counts
    /// (testing 10.3).
    pub span: Span,
    pub starts: Vec<(BlockId, usize, Span)>,
    pub ways: Vec<Way>,
    /// The calls inlined into the function, and the one each block's code
    /// comes from, when it comes from one (`inline`).
    pub inlined: Vec<Inlined>,
    pub scopes: Vec<Option<u32>>,
}

/// A call inlined: the function it called and where that function is,
/// where the call was, and the inlined call that holds it, when one does.
#[derive(Clone, Debug)]
pub struct Inlined {
    pub name: String,
    pub span: Span,
    pub call: Span,
    pub parent: Option<u32>,
}

/// A way a branch of the program can go: the statement of the block it
/// starts at, the branch it belongs to (an `if`, a `match`, `&&` or `||`),
/// where the way is, and what it is called: `then`, `else`, `evaluated`,
/// `short-circuited`, or `arm`, which is called by its pattern.
#[derive(Clone, Debug)]
pub struct Way {
    pub block: BlockId,
    pub at: usize,
    pub branch: Span,
    pub span: Span,
    pub label: &'static str,
}

/// Where a statement starts in a live build: a frame stopped at its probe
/// can restart there, or move to the same point of the new code with the
/// locals live there.
#[derive(Clone, Debug)]
pub struct Point {
    /// The statement: its place among the statements and blocks of the
    /// function, and where it is.
    pub path: Vec<u32>,
    pub span: Span,
    /// The block its code starts at, after the check.
    pub continues: BlockId,
    pub state: Vec<Local>,
    /// What the statement is, for matching it with one of the new code,
    /// and how many `defer`s are registered there.
    pub kind: &'static str,
    pub defers: u32,
}

/// Where a local comes from: a binding's name, a parameter, or the
/// statement whose lowering made it and its place among those it made.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    Named(String),
    Param(u32),
    Made(Vec<u32>, u32),
}

impl Body {
    pub fn place_ty(&self, types: &Types, place: &Place) -> Ty {
        place.path.iter().fold(self.locals[place.local.0 as usize], |ty, &proj| project(types, ty, proj))
    }
}

#[derive(Clone)]
pub struct BlockData {
    pub stmts: Vec<Statement>,
    pub terminator: Terminator,
}

#[derive(Clone)]
pub enum Statement {
    Assign(Place, Box<Rvalue>, Span),
    /// Takes one more reference to every counted value inside the place.
    Retain(Place),
    /// Releases every counted value inside the place.
    Release(Place),
    /// Registers a `defer` cleanup with the runtime: the body to call and
    /// the closure it receives, run if a panic unwinds the scope (15.7).
    DeferPush(hir::FnId, Place),
    /// Unregisters the most recent cleanup before running it normally.
    DeferPop,
    /// Makes the local the address of the place, made unique first: a
    /// binding that borrows an element of a collection (13.3).
    Lend(Local, Place),
    /// Zeroes a binding whose value moved away, so that releasing it at
    /// the end of its scope releases nothing.
    Clear(Local),
    /// Zeroes a component of a temporary whose value moved away, out of a
    /// `match` or a `?`, so that releasing the temporary leaves it.
    Moved(Place),
    /// Makes the local the address of the state a `std.sync` guard locks.
    LendAt(Local, Operand),
    /// Loads the state of a point from the hidden parameter (toolchain 7.10).
    OsrLoad(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Copy(Place),
    Const(Const),
}

impl Operand {
    pub fn local(local: Local) -> Operand {
        Operand::Copy(Place::local(local))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    Int(i128, Ty),
    /// The bits of an `f64`; `Float32` constants are exactly representable.
    Float(u64, Ty),
    Char(char),
    Bool(bool),
    Str(String),
    Unit,
}

/// What else a call lends of an array a region of it is lent from.
#[derive(Clone, Debug)]
pub enum Overlap {
    Region([Option<Operand>; 3]),
    Index(Operand),
}

#[derive(Clone)]
pub enum Rvalue {
    Use(Operand),
    Unary(UnaryOp, Operand),
    /// Checked arithmetic, shifts, bitwise operators and comparisons
    /// (including structural equality). `and`/`or` are lowered to control flow.
    Binary(BinOp, Operand, Operand),
    /// Unchecked `+ 1`, used by range iteration after its bound check.
    Increment(Operand),
    /// A sum, difference or product of `Int`s that `prove` showed cannot
    /// overflow, without the check; or a quotient or remainder of one it
    /// showed is not negative by a power of two, without signs.
    Exact(BinOp, Operand, Operand),
    Call(Callee, Vec<Operand>),
    /// A named function as a `fn(...)` value: a static object.
    FnValue(hir::FnId),
    /// The address of an `extern fn`.
    ExternFn(u32),
    /// The address of a C function that runs the function, the body of a
    /// closure without captures when the flag is set (FFI).
    Callback(hir::FnId, bool),
    /// The size, or with the flag the alignment, of a type in memory.
    Layout(Ty, bool),
    /// A value of the type boxed for a `reflect.Value`, which retains one
    /// that was copied when the flag is set; or the value out of a box; or
    /// whether a box holds one.
    Reflect(hir::ReflectOp, Ty, Vec<Operand>, bool),
    /// An array as a C array of the destination's type, or the reverse.
    Fixed(Operand, Span),
    /// A call through a `fn(...)` value.
    CallIndirect(Operand, Vec<Operand>),
    /// A closure boxed into a `fn(...)` value whose code is the body; the
    /// flag retains captures copied rather than moved.
    Erase(Operand, hir::FnId, bool),
    /// Starts a task that calls a `fn()` value, which the task takes; the
    /// flag retains one that was copied rather than moved.
    Spawn(Operand, bool),
    /// Gives the runner the members of a suite: each node of the test tree
    /// with the `fn()` value that runs it, taken as `Spawn` takes its body.
    TestMembers(Vec<(u32, Operand, bool)>),
    /// An operation of a `Join` or a `Group`, receiver first; flags mark
    /// copied operands to retain as they move in, as a join into a group.
    TaskOp(TaskOp, Vec<Operand>, Vec<bool>, Span),
    /// `select` (11.14): completes one of the operations and gives its
    /// index, -1 for `else` when the flag (blocking) is false, or -2 when
    /// the task is cancelled.
    Select(Vec<SelectCase>, bool, Span),
    /// Whether the current task is cancelled (11.15).
    Cancelled,
    /// In a live build, what the probe of the line that starts at the
    /// point said: go on (0), restart the frame (1), or move it to the new
    /// code (2) (toolchain 7.10).
    Restart(u32),
    /// The state of the point, handed to the runtime, which gives it back
    /// as the new code's point expects it; null when it does not move.
    Transfer(u32),
    /// A call of the function's new code with the state the runtime gave.
    CallOsr(hir::FnId, Vec<Operand>, Operand),
    /// The point the state of the hidden parameter is for.
    OsrPoint,
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
    /// An intrinsic array or deque operation, receiver first.
    ArrayOp(ArrayOp, Vec<Operand>, Span),
    /// `array[start:end:step]`.
    Slice(Operand, [Option<Operand>; 3], Span),
    /// Replaces the elements a slice of the array at the place selects with
    /// those of an array of the same length.
    AssignSlice(Place, [Option<Operand>; 3], Operand, Span),
    /// Panics with `P0004` when a region of the array, lent by a call,
    /// shares an element with another region or with the element at an
    /// index lent by the same call (16.4).
    CheckRegion(Operand, [Option<Operand>; 3], Overlap, Span),
    /// The number of elements of any collection, or of scalars of a string.
    Length(Operand),
    /// An intrinsic string operation, string first.
    StrOp(StrOp, Vec<Operand>, Span),
    /// A raw operation of a `Pointer[T]`, the pointer first; the flag
    /// retains a written value that was copied rather than moved.
    Pointer(hir::PointerOp, Vec<Operand>, bool),
    /// A map or set query, collection first.
    MapOp(MapOp, Vec<Operand>, Span),
    /// A method that changes the collection at the place (made unique
    /// first); flags mark copied operands to retain as they move in.
    Mutate(Mutation, Place, Vec<Operand>, Vec<bool>, Span),
}

/// One operation of a `select`.
#[derive(Clone)]
pub struct SelectCase {
    /// `receive`, `send`, `wait` or `next`; `None` for `time.sleep`.
    pub op: Option<TaskOp>,
    pub operands: Vec<Operand>,
    /// Receives the result of `receive`, `next` or `wait` when it completes.
    pub out: Local,
    /// A value to send that was copied, not moved: retained before waiting.
    pub retain: bool,
}

#[derive(Clone)]
pub enum Piece {
    Text(String),
    Value(Operand, Ty),
    /// First, the string at the place, which the rest is appended to in its
    /// own block when nothing else holds it, or which is released once the
    /// text is built: `x = "{x}..."`, assigned to the place.
    Taken(Place),
}

#[derive(Clone)]
pub enum Terminator {
    Goto(BlockId),
    Branch(Operand, BlockId, BlockId),
    Return(Operand),
    Panic { code: u32, message: Option<Operand>, span: Span },
    Unreachable,
}

/// The locals each block reads before writing them, on some path from its
/// start (toolchain 7.10): what a frame must bring to start there.
pub fn live_in(blocks: &[BlockData], locals: usize) -> Vec<Vec<Local>> {
    let words = locals.div_ceil(64).max(1);
    let set = |bits: &mut [u64], local: Local| bits[local.0 as usize / 64] |= 1 << (local.0 % 64);
    let unset = |bits: &mut [u64], local: Local| bits[local.0 as usize / 64] &= !(1 << (local.0 % 64));
    // What each block reads first and what it writes, as one pass back
    // through it gives them.
    let mut reads = vec![vec![0u64; words]; blocks.len()];
    let mut writes = vec![vec![0u64; words]; blocks.len()];
    for (index, block) in blocks.iter().enumerate() {
        let (read, written) = (&mut reads[index], &mut writes[index]);
        let mut step = |used: &[Local], defined: &[Local]| {
            for &local in defined {
                unset(read, local);
                set(written, local);
            }
            for &local in used {
                set(read, local);
            }
        };
        let (mut used, defined) = (Vec::new(), Vec::new());
        terminator_uses(&block.terminator, &mut used);
        step(&used, &defined);
        for statement in block.stmts.iter().rev() {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            step(&used, &defined);
        }
    }
    let successors = |terminator: &Terminator| -> Vec<usize> {
        match terminator {
            Terminator::Goto(target) => vec![target.0 as usize],
            Terminator::Branch(_, then, otherwise) => vec![then.0 as usize, otherwise.0 as usize],
            _ => Vec::new(),
        }
    };
    let mut live = vec![vec![0u64; words]; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for index in (0..blocks.len()).rev() {
            let mut out = vec![0u64; words];
            for successor in successors(&blocks[index].terminator) {
                for (word, bits) in out.iter_mut().zip(&live[successor]) {
                    *word |= bits;
                }
            }
            let inward: Vec<u64> = (0..words).map(|w| reads[index][w] | (out[w] & !writes[index][w])).collect();
            if inward != live[index] {
                live[index] = inward;
                changed = true;
            }
        }
    }
    live.iter()
        .map(|bits| {
            (0..locals as u32).map(Local).filter(|l| bits[l.0 as usize / 64] & (1 << (l.0 % 64)) != 0).collect()
        })
        .collect()
}

fn place_uses(place: &Place, used: &mut Vec<Local>) {
    used.push(place.local);
    for projection in &place.path {
        if let Proj::Index(index, _) | Proj::InBounds(index) = projection {
            used.push(*index);
        }
    }
}

pub(crate) fn operand_uses(operand: &Operand, used: &mut Vec<Local>) {
    if let Operand::Copy(place) = operand {
        place_uses(place, used);
    }
}

fn operands_uses(operands: &[Operand], used: &mut Vec<Local>) {
    operands.iter().for_each(|operand| operand_uses(operand, used));
}

fn bounds_uses(bounds: &[Option<Operand>; 3], used: &mut Vec<Local>) {
    bounds.iter().flatten().for_each(|operand| operand_uses(operand, used));
}

fn rvalue_uses(rvalue: &Rvalue, used: &mut Vec<Local>) {
    match rvalue {
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::Increment(o)
        | Rvalue::Convert(o)
        | Rvalue::Length(o)
        | Rvalue::Fixed(o, _)
        | Rvalue::Erase(o, ..)
        | Rvalue::Spawn(o, _) => operand_uses(o, used),
        Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b) => {
            operand_uses(a, used);
            operand_uses(b, used);
        }
        Rvalue::Call(_, operands)
        | Rvalue::Array(operands, _)
        | Rvalue::Aggregate(operands)
        | Rvalue::Variant(_, operands)
        | Rvalue::Reflect(_, _, operands, _)
        | Rvalue::TaskOp(_, operands, ..)
        | Rvalue::ArrayOp(_, operands, _)
        | Rvalue::StrOp(_, operands, _)
        | Rvalue::Pointer(_, operands, _)
        | Rvalue::MapOp(_, operands, _) => operands_uses(operands, used),
        Rvalue::CallIndirect(callee, operands) => {
            operand_uses(callee, used);
            operands_uses(operands, used);
        }
        Rvalue::CallOsr(_, operands, state) => {
            operands_uses(operands, used);
            operand_uses(state, used);
        }
        Rvalue::TestMembers(members) => members.iter().for_each(|(_, o, _)| operand_uses(o, used)),
        Rvalue::Select(cases, ..) => cases.iter().for_each(|case| operands_uses(&case.operands, used)),
        Rvalue::Concat(pieces) => pieces.iter().for_each(|piece| match piece {
            Piece::Value(o, _) => operand_uses(o, used),
            Piece::Taken(place) => place_uses(place, used),
            Piece::Text(_) => {}
        }),
        Rvalue::Discriminant(place) => place_uses(place, used),
        Rvalue::Slice(o, bounds, _) => {
            operand_uses(o, used);
            bounds_uses(bounds, used);
        }
        Rvalue::AssignSlice(place, bounds, o, _) => {
            place_uses(place, used);
            bounds_uses(bounds, used);
            operand_uses(o, used);
        }
        Rvalue::CheckRegion(o, bounds, overlap, _) => {
            operand_uses(o, used);
            bounds_uses(bounds, used);
            match overlap {
                Overlap::Region(bounds) => bounds_uses(bounds, used),
                Overlap::Index(o) => operand_uses(o, used),
            }
        }
        Rvalue::Mutate(_, place, operands, ..) => {
            place_uses(place, used);
            operands_uses(operands, used);
        }
        Rvalue::FnValue(_)
        | Rvalue::ExternFn(_)
        | Rvalue::Callback(..)
        | Rvalue::Layout(..)
        | Rvalue::Cancelled
        | Rvalue::Restart(_)
        | Rvalue::Transfer(_)
        | Rvalue::OsrPoint => {}
    }
}

/// The locals a statement reads, and those it writes whole.
pub(crate) fn statement_locals(statement: &Statement, used: &mut Vec<Local>, defined: &mut Vec<Local>) {
    match statement {
        Statement::Assign(place, rvalue, _) => {
            rvalue_uses(rvalue, used);
            if place.path.is_empty() {
                defined.push(place.local);
            } else {
                place_uses(place, used);
            }
            // A `select` writes what it receives only when it completes.
            if let Rvalue::Select(cases, ..) = &**rvalue {
                cases.iter().for_each(|case| used.push(case.out));
            }
        }
        Statement::Retain(place) | Statement::Release(place) | Statement::DeferPush(_, place) => {
            place_uses(place, used)
        }
        Statement::Lend(local, place) => {
            place_uses(place, used);
            defined.push(*local);
        }
        Statement::Clear(local) => defined.push(*local),
        Statement::Moved(place) => place_uses(place, used),
        Statement::LendAt(local, operand) => {
            operand_uses(operand, used);
            defined.push(*local);
        }
        Statement::DeferPop | Statement::OsrLoad(_) => {}
    }
}

pub(crate) fn terminator_uses(terminator: &Terminator, used: &mut Vec<Local>) {
    match terminator {
        Terminator::Branch(o, ..) | Terminator::Return(o) => operand_uses(o, used),
        Terminator::Panic { message: Some(o), .. } => operand_uses(o, used),
        _ => {}
    }
}
