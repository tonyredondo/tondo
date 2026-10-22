//! Typed, name-resolved program produced by the checker.

use crate::ast::{BinOp, UnaryOp};
use crate::source::Span;
use crate::types::{CursorKind, Ty, Types};
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnId(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

pub struct Program {
    pub types: Types,
    pub functions: Vec<Function>,
    pub main: FnId,
    pub traits: Vec<TraitInfo>,
    pub impls: Vec<ImplInfo>,
    /// After monomorphization: the user `Display.display` of each type whose
    /// values are displayed inside collections.
    pub display: HashMap<Ty, FnId>,
    /// The `extern fn` declarations, by `Callee::Extern` index.
    pub externs: Vec<Extern>,
    /// In a test build, the tree of suites and tests, the root first;
    /// `main` then runs the root's members.
    pub tests: Vec<TestNodeInfo>,
}

/// The code of the panic that `testing.skip` ends a test or suite with,
/// which the runner reports as a skip.
pub const SKIP: u32 = 2000;

/// A node of the test tree (testing 3.2).
#[derive(Clone)]
pub struct TestNodeInfo {
    /// The visible identity (`package::unit::module::suite::test`); empty
    /// for the root.
    pub id: String,
    pub parent: u32,
    pub suite: bool,
    pub span: Span,
}

/// An `extern fn`: a C function the program calls by its symbol.
#[derive(Clone)]
pub struct Extern {
    pub name: String,
    pub symbol: String,
    pub params: Vec<Ty>,
    pub result: Ty,
    /// Whether it ends with C's `...`; a call passes more arguments.
    pub variadic: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraitId(pub u32);

/// What monomorphization needs to know about a trait (12.3).
pub struct TraitInfo {
    pub name: String,
    /// The default body of each method, generic over `Self`, the trait's
    /// parameters and the method's own.
    pub defaults: Vec<Option<FnId>>,
    pub param_count: usize,
}

/// An implementation of a trait for a target (12.4). Its methods are
/// generic over the binders, then their own parameters.
pub struct ImplInfo {
    /// Implemented by the compiler: an intrinsic cursor's `Iterator`.
    pub intrinsic: bool,
    pub trait_id: TraitId,
    pub trait_args: Vec<Ty>,
    pub target: Ty,
    pub generics: Vec<Ty>,
    pub methods: Vec<Option<FnId>>,
}

#[derive(Clone)]
pub struct Function {
    pub name: String,
    /// Type parameters; monomorphization replaces a generic function by
    /// one copy per combination of arguments it is called with.
    pub generics: Vec<Ty>,
    pub params: Vec<LocalId>,
    /// Parameters that borrow the caller's place instead of a value: the
    /// `mut self` and `var self` receivers (11.6).
    pub loans: Vec<bool>,
    pub locals: Vec<Local>,
    pub result: Ty,
    /// Whether the result is `T ! E` with the body, `return` and `?`
    /// working on `T`, as in a function declared fallible (9.8). Otherwise
    /// the body's value is the result, even when that is a `T ! E`.
    pub fallible: bool,
    pub body: Block,
    /// The reads of a binding that move its value away, by binding and
    /// position; lowering releases a binding's references there instead of
    /// at the end of its scope.
    pub moves: HashSet<(LocalId, Span)>,
    /// Whether it runs a test or a suite's setup, which tells the runner
    /// where an error leaves it (testing 7.6).
    pub test: bool,
}

#[derive(Clone)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    /// Where it is declared, for diagnostics.
    pub span: Span,
}

#[derive(Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub ty: Ty,
}

#[derive(Clone)]
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
    /// `values[start:end:step] = value`, or a compound form such as `+=`.
    AssignSlice {
        local: LocalId,
        path: Vec<PlaceStep>,
        bounds: [Option<Expr>; 3],
        op: Option<BinOp>,
        value: Expr,
        span: Span,
    },
    /// `defer`: `closure` holds what the cleanup uses and is stored in
    /// `local` now; `call` invokes it when the scope is left (13.7).
    Defer {
        local: LocalId,
        closure: Expr,
        call: Expr,
    },
    /// Binds `local` to the place itself, not to a copy: a binding of a
    /// `for` over a collection that borrows its elements (13.3). Reads
    /// and writes of the binding go through it.
    Lend {
        local: LocalId,
        place: Place,
    },
    /// Takes the lock of a `std.sync` value for the rest of the block:
    /// `guard` holds it and is released when the block ends, and `inner`
    /// is a loan of the state it protects (14.4.4).
    LockSync {
        guard: LocalId,
        inner: LocalId,
        handle: Expr,
        mode: LockMode,
        span: Span,
    },
    /// In `updateWhen`: gives the lock `guard` holds up until the state
    /// changes, then takes it again.
    AwaitChange(LocalId, Span),
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
    /// `for pattern in source` over an array, map or set: iterates a
    /// snapshot by value, in order (10.12).
    ForEach {
        pattern: Pattern,
        source: Expr,
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

#[derive(Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone)]
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
    /// An intrinsic array or deque operation; the receiver comes first.
    ArrayOp(ArrayOp, Vec<Expr>),
    /// A `mut` or `var` loan of a place, passed to a loan parameter.
    Loan(Place),
    /// `mut values[start:end:step]` passed to a `mut Array[T]` parameter:
    /// the callee changes the selected elements of the array in place
    /// (16.4).
    Region(Place, [Option<Box<Expr>>; 3]),
    /// The number of elements of any collection, or of scalars of a string.
    Length(Box<Expr>),
    /// An intrinsic string operation; the string comes first.
    StrOp(StrOp, Vec<Expr>),
    /// `read`, `write` or `offset` of a `Pointer[T]`, the pointer first
    /// (16.12); casts and addresses are `Retype`s.
    Pointer(PointerOp, Vec<Expr>),
    /// A method that changes a collection on a `var` place; the arguments
    /// follow in order.
    Mutate(Mutation, Place, Vec<Expr>),
    /// `[key: value, ...]`
    Map(Vec<(Expr, Expr)>),
    /// `Set[a, b]`
    Set(Vec<Expr>),
    /// A map or set query; the collection comes first.
    MapOp(MapOp, Vec<Expr>),
    /// `value?` on an option or result; the type is the payload's.
    Try(Box<Expr>),
    /// A numeric conversion (18.6). Its type is the target, or
    /// `Target ! NumericConversionError` when it can fail.
    Convert(Box<Expr>),
    /// A call with the type arguments of a generic callee (empty otherwise).
    Call(Callee, Vec<Ty>, Vec<Expr>),
    /// A named function used as a `fn(...)` value, with its type arguments.
    FnValue(FnId, Vec<Ty>),
    /// An `extern fn` used as a value: its address, an `extern fn(...)`.
    ExternFn(u32),
    /// The size in bytes, or with the flag the alignment, of values of a
    /// type in memory (`std.ffi`).
    Layout(Ty, bool),
    /// Boxes a value of the type for a `reflect.Value`, takes one out of a
    /// box, or tells whether a box holds one (`std.internal.reflect`).
    Reflect(ReflectOp, Ty, Vec<Expr>),
    /// An `Array[T]` as a C array `T[n]` of its type, which panics unless
    /// the lengths agree, or a C array as an `Array[T]`.
    Fixed(Box<Expr>),
    /// A call through a value: a `fn(...)`, a closure, or a type parameter
    /// bound by `Call`, `CallMut` or `CallOnce`. Monomorphization turns a
    /// call of a closure into a direct call of its body.
    CallValue(Box<Expr>, Vec<Expr>),
    /// A closure made into a `fn(...)` value; the `FnId` is its body.
    Erase(Box<Expr>, FnId),
    /// A named function, or a closure without captures, as an
    /// `extern fn(...)` that C calls; the `FnId` is what it runs.
    Callback(Box<Expr>, FnId),
    /// `spawn call(...)`: starts a task running a `fn()` value that makes
    /// the call; its type is `Join[T]` (11.11).
    Spawn(Box<Expr>),
    /// An operation of a `Join` or a `Group`; the receiver comes first,
    /// as the loan of its place for `add` and `next`.
    TaskOp(TaskOp, Vec<Expr>),
    /// `select` (11.14): the arms' operations, all evaluated in order
    /// before waiting, and the `else` arm.
    Select(Vec<SelectArm>, Option<Box<Expr>>),
    /// `Iterator.next` of the intrinsic cursor at the place: the next entry,
    /// key or value of its map or set, or `none`.
    CursorNext(Place, CursorKind),
    Unary(UnaryOp, Box<Expr>),
    /// Arithmetic, bitwise and comparison operators; `and`/`or` short-circuit.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    Block(Block),
    Interpolate(Vec<Segment>),
    /// `panic(message)`, or a panic with another code: `testing.failNow`
    /// fails as an assertion (`P0007`) and `testing.skip` with `SKIP`.
    Panic(Box<Expr>, u32),
    /// The members of the suite (or of the root of the test tree) whose
    /// setup is done: each node with the closure, erased to a `fn()`, that
    /// runs it (testing 7.3).
    TestMembers(Vec<(u32, Expr)>),
    /// `assert(condition, parts...)`; `source` is the condition's text.
    Assert {
        condition: Box<Expr>,
        parts: Vec<Expr>,
        source: String,
    },
    Error,
}

/// A binding and a path of fields and indices inside it.
#[derive(Clone)]
pub struct Place {
    pub local: LocalId,
    pub path: Vec<PlaceStep>,
}

impl Place {
    /// The place a value expression reads, if it reads one.
    pub fn of(value: Expr) -> Option<Place> {
        let mut steps = Vec::new();
        let mut current = value;
        let local = loop {
            match current.kind {
                ExprKind::Local(local) => break local,
                ExprKind::Field(base, index) => {
                    steps.push(PlaceStep::Field(index));
                    current = *base;
                }
                ExprKind::Retype(base) => current = *base,
                ExprKind::Index(base, index) => {
                    steps.push(PlaceStep::Index(*index));
                    current = *base;
                }
                _ => return None,
            }
        };
        steps.reverse();
        Some(Place { local, path: steps })
    }
}

#[derive(Clone)]
pub enum PlaceStep {
    Field(u32),
    Index(Expr),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArrayOp {
    /// `Array.new[T]()` or `Deque.new[T]()`
    New,
    /// `Array.withCapacity[T](capacity)`
    WithCapacity,
    Get,
    /// The checked `slice(start, end)` method.
    SliceChecked,
    Concat,
    Repeat,
    /// `value in array`; the value comes first.
    Contains,
}

/// How `LockSync` takes a lock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
    /// Exclusive if it is free; otherwise the guard holds nothing.
    Try,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TaskOp {
    /// `job.wait()`
    Wait,
    /// `Group.new[T]()`
    GroupNew,
    /// `group.add(job)`
    GroupAdd,
    /// `group.next()`: `T?`
    GroupNext,
    /// `group.length()`
    GroupLength,
    /// `group.wait()`: `Array[T]`, or `Array[R] ! E` when `T` is `R ! E`.
    GroupWait,
    /// Used by `GroupWait`: waits for every task, or for the first that
    /// fails, and gives its index or -1.
    GroupSettle,
    /// Used by `GroupWait`: the result at an index, or the first one not
    /// taken when it is -1, as `T?`.
    GroupTake,
    /// `channel[T]()` or `channel[T](capacity)`: `(Sender[T], Receiver[T])`.
    Channel,
    /// `sender.send(value)`
    Send,
    /// `receiver.receive()`: `T?`
    Receive,
    /// `sender.close()`
    Close,
    /// `sender.copy()`
    CopySender,
    /// `isClosed()` of a sender or a receiver.
    IsClosed,
    /// `receiver.receive(timeout: delay)`: `T?`
    ReceiveTimeout,
    /// `job.wait(timeout: delay)`: `T?`, cancelling the task when it is late.
    WaitTimeout,
    /// `job.cancel()`
    Cancel,
    /// `job.detach()`
    Detach,
    /// `group.cancel()`
    GroupCancel,
    /// A new `std.sync` value holding its state; the state comes first.
    SyncNew,
    /// Used by `LockSync`: takes the lock and gives the guard.
    Lock(LockMode),
    /// Used by `AwaitChange`: gives the guard back once the state changed.
    SyncAwait,
    /// Whether a guard holds its lock, after `LockMode::Try`.
    GuardHeld,
    /// `sync.Semaphore(permits)`
    SemaphoreNew,
    /// `semaphore.acquire()`
    Acquire,
    /// `semaphore.tryAcquire()`: `Permit?`
    TryAcquire,
    /// `permit.release()`, which only consumes the permit.
    PermitRelease,
    /// `sync.Barrier(parties)`
    BarrierNew,
    /// `barrier.wait()`
    BarrierWait,
    /// An operation of a shared collection; the collection comes first.
    Shared(SharedOp),
    /// An operation of a `sync.Atomic` of an integer, `Bool`, `Char` or
    /// `Byte`, done with atomic instructions.
    Atomic(AtomicOp),
    /// A value of zeros, which releases nothing: the place an exchange
    /// writes what it saw.
    Zeroed,
}

/// The operations of the shared collections (standard library 14.4.4.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SharedOp {
    /// By key or index: `V?` or `T?`.
    Get,
    Contains,
    /// Map: `V?`, the replaced value. Set: whether the key was present.
    Insert,
    /// Map: `V?`. Set: whether the key was present.
    Remove,
    /// `compareExchange`: the collection, the key or index, the expected
    /// and desired values, and the loan of the place for the value seen;
    /// gives a code (see the runtime).
    Exchange,
    Push,
    /// `pop` of a stack, `popFront` of a queue.
    Pop,
    /// `set(index, value)` of a shared array: the replaced value, or `none`
    /// outside it.
    Set,
    Length,
    Snapshot,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AtomicOp {
    Load,
    Store,
    Swap,
    /// `compareExchange(expected, desired)`: the value seen.
    Exchange,
    Add,
}

impl TaskOp {
    /// Whether the operation takes its operand at `index` rather than
    /// reading it: a consumed handle or a value moved into it.
    pub fn takes(self, index: usize) -> bool {
        match self {
            TaskOp::Wait
            | TaskOp::WaitTimeout
            | TaskOp::Cancel
            | TaskOp::Detach
            | TaskOp::GroupCancel
            | TaskOp::SyncNew
            | TaskOp::PermitRelease => index == 0,
            TaskOp::GroupWait | TaskOp::Channel => true,
            op => op.moves_in(index),
        }
    }

    /// Whether the operation takes over its operand at `index`, which the
    /// caller retains for it when it is not a temporary.
    pub fn moves_in(self, index: usize) -> bool {
        match self {
            TaskOp::GroupAdd | TaskOp::Send | TaskOp::Shared(SharedOp::Push) => index == 1,
            TaskOp::SyncNew => index == 0,
            TaskOp::Shared(SharedOp::Insert) => index >= 1,
            TaskOp::Shared(SharedOp::Set) => index == 2,
            TaskOp::Shared(SharedOp::Exchange) => index == 3,
            _ => false,
        }
    }

    /// Whether the operation waits and ends at once when the task is
    /// cancelled, after which compiled code unwinds (11.15).
    pub fn waits(self) -> bool {
        matches!(
            self,
            TaskOp::Wait
                | TaskOp::WaitTimeout
                | TaskOp::GroupNext
                | TaskOp::GroupSettle
                | TaskOp::Send
                | TaskOp::Receive
                | TaskOp::ReceiveTimeout
                | TaskOp::Lock(LockMode::Shared | LockMode::Exclusive)
                | TaskOp::SyncAwait
                | TaskOp::Acquire
                | TaskOp::BarrierWait
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrOp {
    /// `text[index]`: a `Char`, panicking outside the string.
    At,
    /// `text.get(index)`: `Char?`
    Get,
    ByteLength,
    Concat,
    Contains,
    StartsWith,
    EndsWith,
    /// `find(needle)`: `Int?`
    Find,
    Replace,
    Trim,
    LowerAscii,
    UpperAscii,
    /// The checked `slice(start, end)`: `String ! text.TextError`
    SliceChecked,
    /// `String.fromChars(chars)`, wrapped in `ok` by the checker.
    FromChars,
    /// The scalar at a byte offset; used by iteration.
    Decode,
    /// The byte offset after the scalar at a byte offset; used by iteration.
    Next,
    /// `text.bytes()`: its UTF-8 bytes.
    Bytes,
    /// `String.fromBytes(bytes)` as `String?`: `none` when not UTF-8.
    FromBytes,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MapOp {
    /// `Map.new[K, V]()` or `Set.new[K]()`
    New,
    /// `map[key]` and `map.get(key)`: `V?`
    Get,
    /// `key in map`, `value in set` and `contains`; the collection comes first.
    Contains,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// `push(value)` on an array or deque.
    Push,
    /// `pushFront(value)` on a deque.
    PushFront,
    /// `pop()` on an array or deque: the last element, `T?`.
    Pop,
    /// `popFront()` on a deque: the first element, `T?`.
    PopFront,
    /// `insert(at, value)` on an array.
    InsertAt,
    /// `removeAt(at)` on an array: `T?`.
    RemoveAt,
    /// `clear()` on any collection.
    Clear,
    /// Map: `V?` with the replaced value. Set: `true` when newly added.
    Insert,
    /// Map: `V?` with the removed value. Set: `true` when it was present.
    Remove,
}

impl Mutation {
    /// Which arguments move into the collection.
    pub fn moves(self, count: usize) -> Vec<bool> {
        match self {
            Mutation::Push | Mutation::PushFront | Mutation::Insert => vec![true; count],
            Mutation::InsertAt => vec![false, true],
            _ => vec![false; count],
        }
    }
}

/// An arm of `select`: the body runs, with the pattern bound to the
/// operation's result, when the operation is the one that completes.
#[derive(Clone)]
pub struct SelectArm {
    pub operation: Expr,
    pub pattern: Option<Pattern>,
    pub body: Expr,
}

#[derive(Clone)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Clone)]
pub struct Pattern {
    pub kind: PatKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone)]
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
    /// `[a, b]` matches arrays of exactly that length; with `rest` it
    /// matches longer ones too, binding the remainder when named.
    Array {
        items: Vec<Pattern>,
        rest: Option<Option<LocalId>>,
    },
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

#[derive(Clone)]
pub enum Segment {
    Text(String),
    Value(Expr),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Callee {
    Fn(FnId),
    Std(StdFn),
    /// A trait method, by trait and index; its type arguments are `Self`,
    /// the trait's parameters and the method's own. Monomorphization
    /// replaces it with the implementation for the concrete `Self`.
    Trait(TraitId, u32),
    /// An `extern fn`, by its index in `Program::externs`. Arguments after
    /// its parameters go to C's `...`.
    Extern(u32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StdFn {
    ConsolePrint,
    ConsolePrintln,
    ConsoleFlush,
    ConsoleEprint,
    ConsoleEprintln,
    ConsoleReadLine,
    /// `console.stdout()`, `stderr()` or `stdin()`: 0, 1 or 2.
    ConsoleStream(u8),
    /// `console.setOutput`, `setError` or `setInput`: 0, 1 or 2.
    ConsoleSet(u8),
    /// An operation of `std.io` (standard library 10.3).
    Io(IoFn),
    /// An operation of `std.env` (14.3.5).
    Env(EnvFn),
    /// An operation of `std.process` (19.2).
    Process(ProcessFn),
    /// A function of an internal standard module, by its index in `NATIVES`.
    Native(u16),
    /// `time.sleep(delay)`: waits without blocking the thread (14.3.3).
    TimeSleep,
    /// `time.now()`, `unixTime()`, `utcNow()` and `utc(sinceEpoch)` (14.3).
    TimeNow,
    TimeUnix,
    TimeUtcNow,
    TimeUtc,
    /// An operation of `std.fs` (standard library 10.5).
    Fs(FsFn),
    /// An operation of `std.path` (10.4).
    Path(PathFn),
    /// `testing.log(message)` and `testing.tags(values)` (testing 11.2).
    TestLog,
    TestTags,
    /// Tells the runner where an error leaves a test or a setup.
    TestWhere,
    /// `testing.attach(name, mediaType, data)` and
    /// `testing.snapshot(name, actual)` (testing 9.7).
    TestAttach,
    TestSnapshot,
    /// The virtual clock of `testing.withVirtualTime` (testing 7.9): its
    /// start and end, and `VirtualTime.settle` and `advance`.
    ClockBegin,
    ClockEnd,
    ClockSettle,
    ClockAdvance,
}

/// What `std.internal.reflect` does with a value of a type it is given:
/// boxes it, or takes it out of a box, or tells whether a box holds one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReflectOp {
    Box,
    Unbox,
    Is,
    /// The type's name, as reflection shows it.
    Name,
    /// The type's identity, as an `Int`.
    Identity,
    /// The capabilities the compiler demonstrates of the type, as bits:
    /// 1 `Copy`, 2 `Equatable`, 4 `Key`.
    Capabilities,
}

/// The raw operations of a `Pointer[T]` that touch memory or move it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerOp {
    Read,
    Write,
    Offset,
    /// A read that takes the value, leaving the memory without it
    /// (`std.internal.memory`).
    Take,
}

/// The operations of `std.process`; those of a plan or a process take it
/// first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProcessFn {
    Command,
    Shell,
    Exit,
    /// `left | right`
    Pipe,
    /// A setting of a plan, numbered as the runtime's
    /// `tondo_rt_process_configure` takes it.
    Configure(u8),
    Run,
    Status,
    Output,
    Check,
    Start,
    Text,
    /// `stdin()`, `stdout()` or `stderr()` of a process: 0, 1 or 2.
    Stream(u8),
    Id,
    Wait,
    Cancel,
    /// `Process.output()` and `check()`.
    Collect(bool),
    Detach,
}

/// The operations of `std.env`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EnvFn {
    Args,
    Get,
    Vars,
    Set,
    SetAll,
    Remove,
    CurrentDirectory,
    SetCurrentDirectory,
    SyncNative,
}

/// The operations of `std.io`; those of a stream or buffer take it first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IoFn {
    Write,
    WriteLine,
    WriteBytes,
    Flush,
    Close,
    ReadLine,
    Read,
    ReadText,
    /// `io.reader(text)`
    Reader,
    /// `io.discard()`
    Discard,
    /// `io.Buffer()`
    BufferNew,
    BufferWriter,
    BufferReader,
    BufferText,
    BufferBytes,
    BufferLength,
    BufferClear,
}

impl IoFn {
    /// Whether it returns `T ! IoError` from a status (see the runtime's
    /// `io`), rather than its value.
    pub fn fails(self) -> bool {
        matches!(
            self,
            IoFn::Write
                | IoFn::WriteLine
                | IoFn::WriteBytes
                | IoFn::Flush
                | IoFn::Close
                | IoFn::ReadLine
                | IoFn::Read
                | IoFn::ReadText
                | IoFn::BufferText
        )
    }
}

/// The operations of `std.fs`; those of `File` take it first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FsFn {
    ReadText,
    ReadBytes,
    WriteText,
    WriteBytes,
    AppendText,
    Exists,
    Metadata,
    List,
    CreateDirectory,
    Rename,
    Copy,
    Remove,
    Open,
    ReadLine,
    Read,
    ReadRest,
    Write,
    WriteBytesTo,
    Flush,
    Close,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PathFn {
    Join,
    Parent,
    FileName,
    Extension,
    IsAbsolute,
}

impl StdFn {
    /// How each argument is passed: the methods of a `File` change it
    /// through a `var` loan, except `close`, which takes it.
    pub fn modes(self) -> Vec<crate::ast::Mode> {
        use crate::ast::Mode;
        match self {
            StdFn::Fs(
                FsFn::ReadLine | FsFn::Read | FsFn::ReadRest | FsFn::Write | FsFn::WriteBytesTo | FsFn::Flush,
            ) => vec![Mode::Var, Mode::Value],
            // They observe the process; the others take it.
            StdFn::Process(ProcessFn::Stream(_) | ProcessFn::Id) => vec![Mode::Ref],
            _ => Vec::new(),
        }
    }
}

/// A type a native function of an internal standard module takes or gives:
/// a scalar, a string or a handle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NativeTy {
    Int,
    Float,
    Bool,
    String,
    Unit,
    JsonReader,
    JsonWriter,
    /// A stream of `std.io`.
    IoReader,
    IoWriter,
    /// `Pointer[Byte]`
    BytePointer,
    /// `Array[Byte]`
    Bytes,
    /// `Array[Int]`
    Ints,
    /// The runtime's compiled regular expression (`std.internal.regex`).
    Regex,
    /// The box of a `reflect.Value` (`std.internal.reflect`).
    ReflectBox,
    /// `Array[String]`
    Strings,
    Char,
}

/// A function of an internal module: `std.internal.json`, the runtime's
/// JSON tokenizer and emitter that `std.json` is written on, or
/// `std.internal.memory`, raw memory for tests of `unsafe` code.
pub struct Native {
    /// The internal module: `json` or `memory`.
    pub module: &'static str,
    pub name: &'static str,
    /// The handle whose method it is, or `None` for a module function.
    pub receiver: Option<NativeTy>,
    pub params: &'static [NativeTy],
    pub result: NativeTy,
    pub symbol: &'static str,
}

pub const NATIVES: &[Native] = {
    use NativeTy::*;
    const fn f(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "json", name, receiver: None, params, result, symbol }
    }
    const fn memory(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "memory", name, receiver: None, params, result, symbol }
    }
    const fn regex(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "regex", name, receiver: None, params, result, symbol }
    }
    const fn reflect(
        name: &'static str,
        params: &'static [NativeTy],
        result: NativeTy,
        symbol: &'static str,
    ) -> Native {
        Native { module: "reflect", name, receiver: None, params, result, symbol }
    }
    const fn text(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "text", name, receiver: None, params, result, symbol }
    }
    const fn math(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "math", name, receiver: None, params, result, symbol }
    }
    const fn r(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "regex", name, receiver: Some(Regex), params, result, symbol }
    }
    const fn m(
        receiver: NativeTy,
        name: &'static str,
        params: &'static [NativeTy],
        result: NativeTy,
        symbol: &'static str,
    ) -> Native {
        Native { module: "json", name, receiver: Some(receiver), params, result, symbol }
    }
    &[
        f("reader", &[String, Bool], JsonReader, "tondo_rt_json_reader"),
        f("writer", &[Bool, Bool], JsonWriter, "tondo_rt_json_writer"),
        f("streamReader", &[IoReader, Bool], JsonReader, "tondo_rt_json_stream_reader"),
        f("streamWriter", &[IoWriter, Bool, Bool], JsonWriter, "tondo_rt_json_stream_writer"),
        f("isInt", &[String], Bool, "tondo_rt_json_is_int"),
        f("parseInt", &[String], Int, "tondo_rt_json_parse_int"),
        f("parseFloat", &[String], Float, "tondo_rt_json_parse_float"),
        m(JsonReader, "next", &[], Int, "tondo_rt_json_next"),
        m(JsonReader, "text", &[], String, "tondo_rt_json_text"),
        m(JsonReader, "skip", &[Int], Int, "tondo_rt_json_skip"),
        m(JsonReader, "fail", &[String], Unit, "tondo_rt_json_fail"),
        m(JsonReader, "where", &[Int], String, "tondo_rt_json_where"),
        m(JsonReader, "raw", &[Int], String, "tondo_rt_json_raw"),
        m(JsonWriter, "word", &[Int], Unit, "tondo_rt_json_write_word"),
        m(JsonWriter, "int", &[Int], Unit, "tondo_rt_json_write_int"),
        m(JsonWriter, "float", &[Float], Unit, "tondo_rt_json_write_float"),
        m(JsonWriter, "number", &[String], Unit, "tondo_rt_json_write_number"),
        m(JsonWriter, "string", &[String], Unit, "tondo_rt_json_write_string"),
        m(JsonWriter, "key", &[String], Unit, "tondo_rt_json_write_key"),
        m(JsonWriter, "bracket", &[Bool, Bool], Unit, "tondo_rt_json_write_bracket"),
        m(JsonWriter, "raw", &[String], Unit, "tondo_rt_json_write_raw"),
        m(JsonWriter, "finish", &[], String, "tondo_rt_json_finish"),
        m(JsonWriter, "error", &[], String, "tondo_rt_json_writer_error"),
        memory("allocate", &[Int], BytePointer, "tondo_rt_memory_allocate"),
        memory("release", &[BytePointer, Int], Unit, "tondo_rt_memory_release"),
        memory("cString", &[String], BytePointer, "tondo_rt_memory_c_string"),
        memory("fromCString", &[BytePointer], String, "tondo_rt_memory_from_c_string"),
        memory("bytes", &[BytePointer, Int], Bytes, "tondo_rt_memory_bytes"),
        memory("fromCChars", &[Bytes], String, "tondo_rt_memory_from_c_chars"),
        regex("compile", &[String, Int, Int, Int, Int, Int, Int, Int, Int, Int], Regex, "tondo_rt_regex_compile"),
        regex("errorKind", &[], Int, "tondo_rt_regex_error_kind"),
        regex("errorOffset", &[], Int, "tondo_rt_regex_error_offset"),
        regex("slice", &[String, Int, Int], String, "tondo_rt_regex_slice"),
        r("pattern", &[], String, "tondo_rt_regex_pattern"),
        r("captureCount", &[], Int, "tondo_rt_regex_capture_count"),
        r("captureName", &[Int], String, "tondo_rt_regex_capture_name"),
        r("captureIndex", &[String], Int, "tondo_rt_regex_capture_index"),
        r("isMatch", &[String, Bool], Bool, "tondo_rt_regex_is_match"),
        r("checkInput", &[String], Unit, "tondo_rt_regex_check_input"),
        r("find", &[String, Int, Int], Ints, "tondo_rt_regex_find"),
        r("replace", &[String, String, Bool], String, "tondo_rt_regex_replace"),
        reflect("boxName", &[ReflectBox], String, "tondo_rt_reflect_name"),
        reflect("index", &[String], Int, "tondo_rt_reflect_index"),
        reflect("quote", &[String], String, "tondo_rt_reflect_quote"),
        text("split", &[String, String], Strings, "tondo_rt_text_split"),
        text("lines", &[String], Strings, "tondo_rt_text_lines"),
        text("words", &[String], Strings, "tondo_rt_text_words"),
        text("join", &[Strings, String], String, "tondo_rt_text_join"),
        text("repeat", &[String, Int], String, "tondo_rt_text_repeat"),
        text("trimStart", &[String], String, "tondo_rt_text_trim_start"),
        text("trimEnd", &[String], String, "tondo_rt_text_trim_end"),
        text("findLast", &[String, String], Int, "tondo_rt_text_find_last"),
        text("count", &[String, String], Int, "tondo_rt_text_count"),
        text("lower", &[String], String, "tondo_rt_text_lower"),
        text("upper", &[String], String, "tondo_rt_text_upper"),
        text("isInt", &[String, Int], Bool, "tondo_rt_text_is_int"),
        text("parseInt", &[String, Int], Int, "tondo_rt_text_parse_int"),
        text("isFloat", &[String], Bool, "tondo_rt_text_is_float"),
        text("parseFloat", &[String], Float, "tondo_rt_text_parse_float"),
        text("int", &[Int, Int], String, "tondo_rt_text_int"),
        text("fixed", &[Float, Int], String, "tondo_rt_text_fixed"),
        text("charIs", &[Char, Int], Bool, "tondo_rt_char_is"),
        text("charCase", &[Char, Bool], Char, "tondo_rt_char_case"),
        text("charValid", &[Int], Bool, "tondo_rt_char_valid"),
        text("charCode", &[Char], Int, "tondo_rt_char_code"),
        text("charFrom", &[Int], Char, "tondo_rt_char_from"),
        math("sin", &[Float], Float, "tondo_rt_math_sin"),
        math("cos", &[Float], Float, "tondo_rt_math_cos"),
        math("tan", &[Float], Float, "tondo_rt_math_tan"),
        math("asin", &[Float], Float, "tondo_rt_math_asin"),
        math("acos", &[Float], Float, "tondo_rt_math_acos"),
        math("atan", &[Float], Float, "tondo_rt_math_atan"),
        math("exp", &[Float], Float, "tondo_rt_math_exp"),
        math("ln", &[Float], Float, "tondo_rt_math_ln"),
        math("log10", &[Float], Float, "tondo_rt_math_log10"),
        math("log2", &[Float], Float, "tondo_rt_math_log2"),
        math("sqrt", &[Float], Float, "tondo_rt_math_sqrt"),
        math("floor", &[Float], Float, "tondo_rt_math_floor"),
        math("ceil", &[Float], Float, "tondo_rt_math_ceil"),
        math("round", &[Float], Float, "tondo_rt_math_round"),
        math("trunc", &[Float], Float, "tondo_rt_math_trunc"),
        math("roundAway", &[Float], Float, "tondo_rt_math_round_away"),
        math("atan2", &[Float, Float], Float, "tondo_rt_math_atan2"),
        math("hypot", &[Float, Float], Float, "tondo_rt_math_hypot"),
        math("pow", &[Float, Float], Float, "tondo_rt_math_pow"),
        math("fma", &[Float, Float, Float], Float, "tondo_rt_math_fma"),
        math("classify", &[Float], Int, "tondo_rt_math_class"),
    ]
};
