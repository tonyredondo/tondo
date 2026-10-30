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
    /// None in a library, which runs what C calls.
    pub main: Option<FnId>,
    pub traits: Vec<TraitInfo>,
    pub impls: Vec<ImplInfo>,
    /// After monomorphization: the user `Display.display` of each type whose
    /// values are displayed inside collections.
    pub display: HashMap<Ty, FnId>,
    /// The `extern fn` declarations, by `Callee::Extern` index.
    pub externs: Vec<Extern>,
    /// The `extern fn`s with a body, which C calls by their symbols.
    pub exports: Vec<Export>,
    /// What a library publishes, for its header.
    pub c_api: CApi,
    /// In a test build, the tree of suites and tests, the root first;
    /// `main` then runs the root's members.
    pub tests: Vec<TestNodeInfo>,
    /// The functions of each context (11.16), by the context's index.
    pub contexts: Vec<ContextFns>,
}

/// What the runtime calls of a context (11.16): the maker of its initial
/// value in a `Ref`, and those that show its fields marked `@log` and
/// `@label` that do not have their initial values, as names and texts
/// after each a zero character. A field whose initial value is computed
/// has zeros there, and a function of its own that computes it, once, in
/// a `Ref` of a record that holds only it.
#[derive(Clone)]
pub struct ContextFns {
    pub maker: FnId,
    pub log: Option<FnId>,
    pub label: Option<FnId>,
    /// Each computed field: its index, the function and its name,
    /// `Context.field`.
    pub computed: Vec<(u32, FnId, String)>,
}

/// The code of the panic that `testing.skip` ends a test or suite with,
/// which the runner reports as a skip.
pub const SKIP: u32 = 2000;
/// `testing.failNow`, a failed assertion (`P0007`) that no `recover` stops.
pub const FAIL_NOW: u32 = 2012;

/// A node of the test tree (testing 3.2).
#[derive(Clone)]
pub struct TestNodeInfo {
    /// The visible identity (`package::unit::module::suite::test`); empty
    /// for the root.
    pub id: String,
    pub parent: u32,
    pub suite: bool,
    /// A test whose body calls `testing.bench` (testing 10.4).
    pub bench: bool,
    /// A test whose body calls `testing.fuzz` (testing 10.6).
    pub fuzz: bool,
    pub span: Span,
}

/// What a library's header declares (toolchain 7): the functions it
/// publishes, and the C structs they reach, each after those it holds.
#[derive(Clone, Default)]
pub struct CApi {
    pub functions: Vec<CFunction>,
    pub structs: Vec<CStruct>,
}

#[derive(Clone)]
pub struct CFunction {
    pub symbol: String,
    pub params: Vec<(String, CType)>,
    pub result: CType,
    /// Its `///` documentation.
    pub docs: Option<String>,
}

/// An `extern type`, as C declares it.
#[derive(Clone)]
pub struct CStruct {
    pub name: String,
    pub fields: Vec<(String, CType)>,
    pub layout: crate::types::CLayout,
    pub docs: Option<String>,
}

/// A type that crosses to C, as a header spells it: `std.ffi`'s aliases by
/// their C names, Tondo's numbers by `stdint.h`'s.
#[derive(Clone, Debug, PartialEq)]
pub enum CType {
    Named(&'static str),
    /// An `extern type`, by name.
    Struct(String),
    Pointer(Box<CType>),
    /// The address of a function: its parameters and result.
    Function(Vec<CType>, Box<CType>),
    /// A `@count(n)` field.
    Array(Box<CType>, u32),
}

/// An `extern fn` with a body: a function C calls by its symbol (16.16).
#[derive(Clone)]
pub struct Export {
    pub function: FnId,
    pub symbol: String,
    /// The function as C calls it: an `extern fn(...)` type.
    pub ty: Ty,
    /// `pub`, in the package a library builds: the library publishes it.
    pub published: bool,
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
    /// Each method's name.
    pub methods: Vec<String>,
    /// `Additive`, `Number` or `Integer`, which the language's numbers and a
    /// duration implement as the compiler computes them (18.10).
    pub arithmetic: bool,
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
    /// Where it is declared: a function's name, a closure, a script's
    /// first statement; none for what the compiler makes.
    pub span: Span,
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
    /// `with Context { ... }`: the context `context` has `value`, a `Ref`
    /// of its record, for the rest of the block (11.16).
    ContextSet {
        context: u32,
        value: Expr,
        /// The fields it changes, a bit each, which the tools show it set.
        fields: u64,
        span: Span,
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
    /// The bytes of an embedded file (standard library 14.27), an
    /// `Array[Byte]` the program's data holds.
    Bytes(std::sync::Arc<[u8]>),
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
    /// `reference.value` of a `Ref[T]` (16.10): the value it identifies,
    /// which can be read but neither moved out nor changed.
    RefValue(Box<Expr>),
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
    /// The value of the context of this index where the code runs (11.16),
    /// which the task holds in a `Ref` of the type given.
    ContextRead(u32, Ty),
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
    /// `recover { ... }` (15.9): the block's value as `ok`, or the panic
    /// that stopped it as `err`, which the expression reads.
    Recover(Block, Box<Expr>),
    /// `within limit { ... }` (11.17): the limit, a binding's value; the
    /// block, whose value the result holds; and the result's error when the
    /// deadline passes first.
    Within(Box<Expr>, Block, Box<Expr>),
    /// The members of the suite (or of the root of the test tree) whose
    /// setup is done: each node with the closure, erased to a `fn()`, that
    /// runs it (testing 7.3).
    TestMembers(Vec<(u32, Expr)>),
    /// `assert(condition, parts...)`; `source` is the condition's text,
    /// and `detail` what a failure adds to the message, evaluated then.
    Assert {
        condition: Box<Expr>,
        parts: Vec<Expr>,
        source: String,
        detail: Option<Box<Expr>>,
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
    /// The elements as text with a separator between them: `joined` of
    /// `std.internal.text`, which `Array.join` is.
    Join,
    /// Whether the runtime puts the elements in order: those whose equal
    /// values cannot be told apart, as integers and strings, which it sorts
    /// without keeping equal ones as they were. `nativeOrder` of
    /// `std.internal.text`.
    NativeOrder,
    /// The elements in order, which the runtime sorts: `sortedNatively`.
    SortedNatively,
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
    /// `sender.isAbandoned()`: whether no receiver of its channel is left.
    Abandoned,
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
    /// `Ref(value)` (16.10): a new identity holding the value.
    RefNew,
    /// `WeakRef(reference)`
    WeakNew,
    /// `weak.get()`: `Ref[T]?`
    WeakGet,
    /// `std.internal.channel`'s `offer(sender, value)`, which waits for
    /// room, and `tryOffer`, which does not: 0 when sent, 1 when full, 2
    /// once closed; a value not sent is dropped. For `std.executor`.
    Offer,
    TryOffer,
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
            | TaskOp::RefNew
            | TaskOp::PermitRelease => index == 0,
            TaskOp::GroupWait | TaskOp::Channel => true,
            op => op.moves_in(index),
        }
    }

    /// Whether the operation takes over its operand at `index`, which the
    /// caller retains for it when it is not a temporary.
    pub fn moves_in(self, index: usize) -> bool {
        match self {
            TaskOp::GroupAdd | TaskOp::Send | TaskOp::Offer | TaskOp::TryOffer | TaskOp::Shared(SharedOp::Push) => {
                index == 1
            }
            TaskOp::SyncNew | TaskOp::RefNew => index == 0,
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
                | TaskOp::Offer
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
    /// An operation of `std.env` (14.3.6).
    Env(EnvFn),
    /// An operation of `std.process` (19.2).
    Process(ProcessFn),
    /// A function of an internal standard module, by its index in `NATIVES`.
    Native(u16),
    /// A named operation of a number (18.9).
    Number(crate::numbers::NumOp),
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
    /// A panic with the code, whose message is the argument, where the
    /// code unwinds when the call returns (15.9).
    PanicUnwind(u32),
    /// The task the next `spawn` starts is detached (11.11).
    SpawnDetached,
    /// A `within` block starts with its limit, ends, and asks whether what
    /// unwinds to it is its deadline (11.17).
    WithinEnter,
    WithinLeave,
    WithinCatch,
    /// The `Ref` of a context's value where the code runs; a new value of
    /// it, which gives what to restore; and that restored (11.16).
    ContextGet(u32),
    ContextSet(u32, u64),
    ContextRestore,
    /// The `Ref` of a context's value that holds a field whose initial
    /// value is computed: the task's, if a `with` set the field, or the
    /// one the first read computes (11.16).
    ContextField(u32, u32),
    /// A `recover` block starts, ends as code does (true when a task of it
    /// ended with a panic), ends another way, and stops the panic that
    /// unwinds to it (15.9).
    RecoverEnter,
    RecoverEnd,
    RecoverLeave,
    RecoverCatch,
    /// The cleanups of a frame a panic unwinds start and end (15.9).
    UnwindHold,
    UnwindResume,
    /// A `defer` ran; a function a cancellation unwinds returns (11.15).
    DeferEnd,
    CancelReturn,
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
    /// The loop of `testing.bench(name, body)` (testing 10.4): its start,
    /// how many runs come next, and its end; and `testing.keep(value)`.
    BenchStart,
    BenchNext,
    BenchEnd,
    Keep,
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
    /// A value of any type as text, as the report of an error that ends
    /// `main` shows it: its `Display`, a string, `Type.Variant`, or the
    /// type's name.
    Describe,
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
    /// `process.signals(...)` and `Process.signal(signal)` (19.2).
    Signals,
    Signal,
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
    Hostname,
    ProcessId,
    Executable,
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
    ReadBytes,
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
                | IoFn::ReadBytes
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
    /// Whether it may wait and ends at once when the task is cancelled,
    /// after which compiled code unwinds (11.15).
    pub fn waits(self) -> bool {
        matches!(
            self,
            StdFn::TimeSleep
                | StdFn::ContextField(..)
                | StdFn::ClockSettle
                | StdFn::ClockAdvance
                | StdFn::ConsoleReadLine
                | StdFn::Io(IoFn::ReadLine | IoFn::Read | IoFn::ReadText | IoFn::ReadBytes)
        )
    }

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
            StdFn::Process(ProcessFn::Signal) => vec![Mode::Ref, Mode::Value],
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
    /// A value of the runtime (`std.internal.http`).
    Object,
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
    const fn meta(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "meta", name, receiver: None, params, result, symbol }
    }
    const fn http(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "http", name, receiver: None, params, result, symbol }
    }
    const fn codec(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "codec", name, receiver: None, params, result, symbol }
    }
    const fn toml(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "toml", name, receiver: None, params, result, symbol }
    }
    const fn yaml(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "yaml", name, receiver: None, params, result, symbol }
    }
    const fn msgpack(
        name: &'static str,
        params: &'static [NativeTy],
        result: NativeTy,
        symbol: &'static str,
    ) -> Native {
        Native { module: "msgpack", name, receiver: None, params, result, symbol }
    }
    const fn cbor(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "cbor", name, receiver: None, params, result, symbol }
    }
    const fn time(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "time", name, receiver: None, params, result, symbol }
    }
    const fn fs(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "fs", name, receiver: None, params, result, symbol }
    }
    const fn net(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "net", name, receiver: None, params, result, symbol }
    }
    const fn sql(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "sql", name, receiver: None, params, result, symbol }
    }
    const fn image(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "image", name, receiver: None, params, result, symbol }
    }
    const fn env(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "env", name, receiver: None, params, result, symbol }
    }
    const fn fuzz(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "fuzz", name, receiver: None, params, result, symbol }
    }
    const fn panics(name: &'static str, params: &'static [NativeTy], result: NativeTy, symbol: &'static str) -> Native {
        Native { module: "panic", name, receiver: None, params, result, symbol }
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
        m(JsonReader, "nextEvent", &[], Int, "tondo_rt_json_next_event"),
        m(JsonReader, "integer", &[], Int, "tondo_rt_json_integer"),
        m(JsonReader, "text", &[], String, "tondo_rt_json_text"),
        m(JsonReader, "skip", &[Int], Int, "tondo_rt_json_skip"),
        m(JsonReader, "field", &[Strings, Bool], Int, "tondo_rt_json_field"),
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
        regex("escape", &[String], String, "tondo_rt_regex_escape"),
        regex("wholeMatch", &[String, String], Int, "tondo_rt_regex_whole_match"),
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
        meta("request", &[], String, "tondo_rt_meta_request"),
        meta("respond", &[String], Unit, "tondo_rt_meta_respond"),
        meta("bytes", &[String], Bytes, "tondo_rt_meta_bytes"),
        text("split", &[String, String], Strings, "tondo_rt_text_split"),
        text("lines", &[String], Strings, "tondo_rt_text_lines"),
        text("words", &[String], Strings, "tondo_rt_text_words"),
        text("repeat", &[String, Int], String, "tondo_rt_text_repeat"),
        text("trim", &[String, String, Bool, Int], String, "tondo_rt_text_trim"),
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
        text("float", &[Float, Int, Bool, Bool], String, "tondo_rt_text_float"),
        text("normalize", &[String, Int], String, "tondo_rt_text_normalize"),
        text("isNormalized", &[String, Int], Bool, "tondo_rt_text_is_normalized"),
        text("charIs", &[Char, Int], Bool, "tondo_rt_char_is"),
        text("charCase", &[Char, Bool], Char, "tondo_rt_char_case"),
        text("charValid", &[Int], Bool, "tondo_rt_char_valid"),
        text("charCode", &[Char], Int, "tondo_rt_char_code"),
        text("charFrom", &[Int], Char, "tondo_rt_char_from"),
        text("scalarAt", &[String, Int], Char, "tondo_rt_text_scalar_at"),
        text("between", &[String, Int, Int], String, "tondo_rt_text_between"),
        text("findIn", &[String, String, Int, Int], Int, "tondo_rt_text_find_in"),
        text("template", &[String, Bool], String, "tondo_rt_template_parse"),
        text("terminal", &[Int], Bool, "tondo_rt_console_terminal"),
        text("terminalWidth", &[], Int, "tondo_rt_console_width"),
        text("terminalEcho", &[Bool], Bool, "tondo_rt_console_echo"),
        text("numberSymbols", &[String], Strings, "tondo_rt_text_number_symbols"),
        text("currencySymbol", &[String, String], String, "tondo_rt_text_currency_symbol"),
        text("currencyDigits", &[String], Int, "tondo_rt_text_currency_digits"),
        text("pluralRules", &[String], Strings, "tondo_rt_text_plural_rules"),
        // `std.internal.env`: what `std.env` writes in Tondo asks of the host.
        env("processors", &[], Int, "tondo_rt_env_processors"),
        // `std.internal.sql`: SQLite, for `std.sql`.
        sql("open", &[String], Object, "tondo_rt_sqlite_open"),
        sql("failure", &[Object], String, "tondo_rt_sqlite_failure"),
        sql("run", &[Object, String, Bytes, Bool], Bytes, "tondo_rt_sqlite_run"),
        // `std.internal.image`: images, for `std.image`.
        image("read", &[Bytes], Object, "tondo_rt_image_read"),
        image("failure", &[Object], String, "tondo_rt_image_failure"),
        image("info", &[Bytes], Ints, "tondo_rt_image_info"),
        image("size", &[Object], Ints, "tondo_rt_image_size"),
        image("blank", &[Int, Int, Int], Object, "tondo_rt_image_blank"),
        image("change", &[Object, Int, Int, Int, Int, Int], Object, "tondo_rt_image_change"),
        image("blur", &[Object, Float], Object, "tondo_rt_image_blur"),
        image("overlay", &[Object, Object, Int, Int], Object, "tondo_rt_image_overlay"),
        image("pixel", &[Object, Int, Int], Int, "tondo_rt_image_pixel"),
        image("encode", &[Object, Int, Int], Bytes, "tondo_rt_image_encode"),
        env("target", &[], String, "tondo_rt_env_target"),
        codec("base64Encode", &[Bytes, Bool], String, "tondo_rt_base64_encode"),
        codec("base64Check", &[String, Bool], Int, "tondo_rt_base64_check"),
        codec("base64Decode", &[String, Bool], Bytes, "tondo_rt_base64_decode"),
        codec("hexEncode", &[Bytes], String, "tondo_rt_hex_encode"),
        codec("hexCheck", &[String], Int, "tondo_rt_hex_check"),
        codec("hexDecode", &[String], Bytes, "tondo_rt_hex_decode"),
        codec("urlEncode", &[String], String, "tondo_rt_url_encode"),
        codec("urlCheck", &[String], Int, "tondo_rt_url_check"),
        codec("urlDecode", &[String], String, "tondo_rt_url_decode"),
        codec("htmlEscape", &[String], String, "tondo_rt_html_escape"),
        codec("htmlUnescape", &[String], String, "tondo_rt_html_unescape"),
        codec("pipe", &[Int], Object, "tondo_rt_pipe"),
        codec("pipeWriter", &[Object], IoWriter, "tondo_rt_pipe_writer"),
        codec("pipeReader", &[Object], IoReader, "tondo_rt_pipe_reader"),
        codec("tee", &[IoWriter, IoWriter], IoWriter, "tondo_rt_tee"),
        codec("gzip", &[Bytes, Int], Bytes, "tondo_rt_gzip"),
        codec("zstd", &[Bytes], Bytes, "tondo_rt_zstd"),
        codec("unpack", &[Bytes, Bool], Object, "tondo_rt_unpack"),
        codec("deflate", &[Bytes, Int], Bytes, "tondo_rt_deflate"),
        codec("inflate", &[Bytes], Object, "tondo_rt_inflate"),
        codec("crc32", &[Bytes], Int, "tondo_rt_crc32"),
        codec("unpacked", &[Object], Bytes, "tondo_rt_unpacked"),
        codec("unpackFailed", &[Object], Bool, "tondo_rt_unpack_failed"),
        codec("packWriter", &[IoWriter, Bool, Int], IoWriter, "tondo_rt_pack_writer"),
        codec("unpackReader", &[IoReader, Bool], IoReader, "tondo_rt_unpack_reader"),
        fs("failure", &[Object], Int, "tondo_rt_files_failure"),
        fs("texts", &[Object], Strings, "tondo_rt_files_texts"),
        fs("numbers", &[Object], Ints, "tondo_rt_files_numbers"),
        fs("stat", &[String], Object, "tondo_rt_files_stat"),
        fs("walk", &[String, Int], Object, "tondo_rt_files_walk"),
        fs("temporary", &[Bool], Object, "tondo_rt_files_temporary"),
        fs("setPermissions", &[String, Int], Object, "tondo_rt_files_set_permissions"),
        fs("symlink", &[String, String], Object, "tondo_rt_files_symlink"),
        fs("readSymlink", &[String], Object, "tondo_rt_files_read_symlink"),
        fs("realPath", &[String], Object, "tondo_rt_files_real_path"),
        fs("watch", &[Strings, Int], Object, "tondo_rt_files_watch"),
        fs("watchNext", &[Object], Strings, "tondo_rt_files_watch_next"),
        fs("watchStop", &[Object], Unit, "tondo_rt_files_watch_stop"),
        codec("digest", &[Int, Bytes, Bytes, Bool], Bytes, "tondo_rt_hash_digest"),
        codec("seal", &[Int, Bytes, Bytes, Bytes, Bytes], Bytes, "tondo_rt_crypto_seal"),
        codec("open", &[Int, Bytes, Bytes, Bytes, Bytes], Bytes, "tondo_rt_crypto_open"),
        codec("hkdf", &[Bytes, Bytes, Bytes, Int], Bytes, "tondo_rt_crypto_hkdf"),
        codec("pbkdf2", &[Bytes, Bytes, Int, Int], Bytes, "tondo_rt_crypto_pbkdf2"),
        codec("generateKey", &[Int], Bytes, "tondo_rt_crypto_generate"),
        codec("publicKey", &[Int, Bytes], Bytes, "tondo_rt_crypto_public"),
        codec("sign", &[Int, Bytes, Bytes], Bytes, "tondo_rt_crypto_sign"),
        codec("verify", &[Int, Bytes, Bytes, Bytes], Bool, "tondo_rt_crypto_verify"),
        codec("hashPassword", &[String], String, "tondo_rt_crypto_hash_password"),
        codec("verifyPassword", &[String, String], Bool, "tondo_rt_crypto_verify_password"),
        codec("hasher", &[Int, Bytes, Bool], Object, "tondo_rt_hasher"),
        codec("hasherUpdate", &[Object, Bytes], Unit, "tondo_rt_hasher_update"),
        codec("hasherFinish", &[Object], Bytes, "tondo_rt_hasher_finish"),
        codec("entropy", &[Int], Bytes, "tondo_rt_entropy"),
        codec("unixMillis", &[], Int, "tondo_rt_unix_millis"),
        codec("csvCheck", &[String, Char], Int, "tondo_rt_csv_check"),
        codec("csvFields", &[String, Char], Strings, "tondo_rt_csv_fields"),
        codec("mix", &[Int, Int], Int, "tondo_rt_random_mix"),
        codec("randomBytes", &[Int, Int, Int], Bytes, "tondo_rt_random_bytes"),
        codec("metric", &[String, Int], Int, "tondo_rt_metric"),
        codec("metricAdd", &[Int, Int], Unit, "tondo_rt_metric_add"),
        codec("metricSet", &[Int, Int], Unit, "tondo_rt_metric_set"),
        codec("metricValue", &[Int], Int, "tondo_rt_metric_value"),
        codec("profileCpuStart", &[], Bool, "tondo_rt_profile_cpu_start"),
        codec("profileCpuStop", &[], Object, "tondo_rt_profile_cpu_stop"),
        codec("profileMemoryStart", &[], Unit, "tondo_rt_profile_memory_start"),
        codec("profileMemorySampled", &[], Bool, "tondo_rt_profile_memory_sampled"),
        codec("profileMemory", &[], Object, "tondo_rt_profile_memory"),
        codec("profileTasks", &[], Object, "tondo_rt_profile_tasks"),
        codec("profilePprof", &[Object], Bytes, "tondo_rt_profile_pprof"),
        codec("profileFolded", &[Object], String, "tondo_rt_profile_folded"),
        codec("labelsPush", &[Strings, Strings], Int, "tondo_rt_labels_push"),
        codec("metrics", &[], Strings, "tondo_rt_metrics"),
        codec("stack", &[], Strings, "tondo_rt_diagnostics_stack"),
        codec("waitsStart", &[Int], Unit, "tondo_rt_profile_waits_start"),
        codec("waitsSampled", &[Int], Bool, "tondo_rt_profile_waits_sampled"),
        codec("waits", &[Int], Object, "tondo_rt_profile_waits"),
        codec("labelsRestore", &[Int], Unit, "tondo_rt_labels_restore"),
        http("client", &[Int, Int, String, String, Bool, String, String, Int], Object, "tondo_rt_http_client"),
        http("listenUnix", &[String, Int, Int, Int], Object, "tondo_rt_http_listen_unix"),
        http("mediaType", &[String], String, "tondo_rt_http_media_type"),
        http("bodyEmpty", &[], Object, "tondo_rt_http_body_empty"),
        http("bodyBytes", &[Bytes, String], Object, "tondo_rt_http_body_bytes"),
        http("bodyStream", &[IoReader, Int, String], Object, "tondo_rt_http_body_stream"),
        http("form", &[], Object, "tondo_rt_http_form"),
        http("formText", &[Object, String, String], Unit, "tondo_rt_http_form_text"),
        http("formBytes", &[Object, String, Bytes, String, String], Unit, "tondo_rt_http_form_bytes"),
        http("formFile", &[Object, String, String, String], String, "tondo_rt_http_form_file"),
        http("formStream", &[Object, String, IoReader, String, String, Int], Unit, "tondo_rt_http_form_stream"),
        http("bodyForm", &[Object], Object, "tondo_rt_http_body_form"),
        http("send", &[Object, String, String, Strings, Object, Int], Object, "tondo_rt_http_send"),
        http("failure", &[Object], String, "tondo_rt_http_failure"),
        http("status", &[Object], Int, "tondo_rt_http_status"),
        http("head", &[Object], String, "tondo_rt_http_head"),
        http("headers", &[Object], Strings, "tondo_rt_http_headers"),
        http("readAll", &[Object], Bytes, "tondo_rt_http_read_all"),
        http("body", &[Object, Bool], IoReader, "tondo_rt_http_body"),
        http("windows1252", &[Bytes], String, "tondo_rt_http_windows_1252"),
        http("wsConnect", &[String, Strings, Int], Object, "tondo_rt_http_ws_connect"),
        http("wsSend", &[Object, Bool, String, Bytes], String, "tondo_rt_http_ws_send"),
        http("wsReceive", &[Object], Object, "tondo_rt_http_ws_receive"),
        http("wsKind", &[Object], Int, "tondo_rt_http_ws_kind"),
        http("wsText", &[Object], String, "tondo_rt_http_ws_text"),
        http("wsData", &[Object], Bytes, "tondo_rt_http_ws_data"),
        http("wsClose", &[Object], String, "tondo_rt_http_ws_close"),
        http("listen", &[String, Int, String, String, String, Int, Int, Int, Int], Object, "tondo_rt_http_listen"),
        http("requestAnswering", &[Object], Unit, "tondo_rt_http_request_answering"),
        http("requestLimit", &[Object, Int], Unit, "tondo_rt_http_request_limit"),
        http("requestExceeded", &[Object], Bool, "tondo_rt_http_request_exceeded"),
        http("serverAddress", &[Object], String, "tondo_rt_http_server_address"),
        http("next", &[Object], Object, "tondo_rt_http_next"),
        http("serverStop", &[Object], Unit, "tondo_rt_http_server_stop"),
        http("requestOpen", &[Object], Bool, "tondo_rt_http_request_open"),
        http("requestHead", &[Object], String, "tondo_rt_http_request_head"),
        http("requestHeaders", &[Object], Strings, "tondo_rt_http_request_headers"),
        http("requestCertificate", &[Object], Bytes, "tondo_rt_http_request_certificate"),
        http("requestBody", &[Object], IoReader, "tondo_rt_http_request_body"),
        http("requestRead", &[Object], Bytes, "tondo_rt_http_request_read"),
        http("respond", &[Object, Int, Strings, Bytes], Unit, "tondo_rt_http_respond"),
        http("respondStream", &[Object, Int, Strings, IoReader], Unit, "tondo_rt_http_respond_stream"),
        http("respondFile", &[Object, Strings, String, String], String, "tondo_rt_http_respond_file"),
        http("respondBytes", &[Object, Strings, String, Bytes, String], Unit, "tondo_rt_http_respond_bytes"),
        http("saveBody", &[Object, String, Int], Int, "tondo_rt_http_save_body"),
        http("requestForm", &[Object], Object, "tondo_rt_http_request_form"),
        http("formNext", &[Object], Object, "tondo_rt_http_form_next"),
        http("uploadHead", &[Object], String, "tondo_rt_http_upload_head"),
        http("uploadRead", &[Object], Bytes, "tondo_rt_http_upload_read"),
        http("uploadSave", &[Object, String], Int, "tondo_rt_http_upload_save"),
        http("acceptWebSocket", &[Object], Object, "tondo_rt_http_accept_websocket"),
        http("pipe", &[], Object, "tondo_rt_http_pipe"),
        http("pipeReader", &[Object], IoReader, "tondo_rt_http_pipe_reader"),
        http("pipeWriter", &[Object], IoWriter, "tondo_rt_http_pipe_writer"),
        toml("parse", &[String], Object, "tondo_rt_toml_parse"),
        toml("next", &[Object], Int, "tondo_rt_toml_next"),
        toml("text", &[Object], String, "tondo_rt_toml_text"),
        toml("int", &[Object], Int, "tondo_rt_toml_int"),
        toml("float", &[Object], Float, "tondo_rt_toml_float"),
        toml("skip", &[Object, Int], Int, "tondo_rt_toml_skip"),
        toml("fail", &[Object, String], Unit, "tondo_rt_toml_fail"),
        toml("where", &[Object, Int], String, "tondo_rt_toml_where"),
        toml("writer", &[], Object, "tondo_rt_toml_writer"),
        toml("writeNull", &[Object], Unit, "tondo_rt_toml_write_null"),
        toml("writeBool", &[Object, Bool], Unit, "tondo_rt_toml_write_bool"),
        toml("writeInt", &[Object, Int], Unit, "tondo_rt_toml_write_int"),
        toml("writeNumber", &[Object, String], Unit, "tondo_rt_toml_write_number"),
        toml("writeFloat", &[Object, Float], Unit, "tondo_rt_toml_write_float"),
        toml("writeText", &[Object, String], Unit, "tondo_rt_toml_write_text"),
        toml("writeDateTime", &[Object, String], Unit, "tondo_rt_toml_write_datetime"),
        toml("writeKey", &[Object, String], Unit, "tondo_rt_toml_write_key"),
        toml("writeOpen", &[Object, Bool], Unit, "tondo_rt_toml_write_open"),
        toml("writeClose", &[Object], Unit, "tondo_rt_toml_write_close"),
        toml("finish", &[Object], String, "tondo_rt_toml_finish"),
        toml("failure", &[Object, Int], String, "tondo_rt_toml_failure"),
        yaml("parse", &[String], Object, "tondo_rt_yaml_parse"),
        yaml("documents", &[Object], Int, "tondo_rt_yaml_documents"),
        yaml("nextDocument", &[Object], Bool, "tondo_rt_yaml_next_document"),
        yaml("next", &[Object], Int, "tondo_rt_yaml_next"),
        yaml("text", &[Object], String, "tondo_rt_yaml_text"),
        yaml("int", &[Object], Int, "tondo_rt_yaml_int"),
        yaml("float", &[Object], Float, "tondo_rt_yaml_float"),
        yaml("skip", &[Object, Int], Int, "tondo_rt_yaml_skip"),
        yaml("fail", &[Object, String], Unit, "tondo_rt_yaml_fail"),
        yaml("where", &[Object, Int], String, "tondo_rt_yaml_where"),
        yaml("writer", &[], Object, "tondo_rt_yaml_writer"),
        yaml("writeNull", &[Object], Unit, "tondo_rt_yaml_write_null"),
        yaml("writeBool", &[Object, Bool], Unit, "tondo_rt_yaml_write_bool"),
        yaml("writeInt", &[Object, Int], Unit, "tondo_rt_yaml_write_int"),
        yaml("writeNumber", &[Object, String], Unit, "tondo_rt_yaml_write_number"),
        yaml("writeFloat", &[Object, Float], Unit, "tondo_rt_yaml_write_float"),
        yaml("writeText", &[Object, String], Unit, "tondo_rt_yaml_write_text"),
        yaml("writeKey", &[Object, String], Unit, "tondo_rt_yaml_write_key"),
        yaml("writeOpen", &[Object, Bool], Unit, "tondo_rt_yaml_write_open"),
        yaml("writeClose", &[Object], Unit, "tondo_rt_yaml_write_close"),
        yaml("finish", &[Object], String, "tondo_rt_yaml_finish"),
        msgpack("parse", &[Bytes], Object, "tondo_rt_msgpack_parse"),
        msgpack("next", &[Object], Int, "tondo_rt_msgpack_next"),
        msgpack("text", &[Object], String, "tondo_rt_msgpack_text"),
        msgpack("int", &[Object], Int, "tondo_rt_msgpack_int"),
        msgpack("float", &[Object], Float, "tondo_rt_msgpack_float"),
        msgpack("bytes", &[Object], Bytes, "tondo_rt_msgpack_bytes"),
        msgpack("skip", &[Object, Int], Int, "tondo_rt_msgpack_skip"),
        msgpack("fail", &[Object, String], Unit, "tondo_rt_msgpack_fail"),
        msgpack("where", &[Object, Int], String, "tondo_rt_msgpack_where"),
        msgpack("writer", &[], Object, "tondo_rt_msgpack_writer"),
        msgpack("writeNull", &[Object], Unit, "tondo_rt_msgpack_write_null"),
        msgpack("writeBool", &[Object, Bool], Unit, "tondo_rt_msgpack_write_bool"),
        msgpack("writeInt", &[Object, Int], Unit, "tondo_rt_msgpack_write_int"),
        msgpack("writeNumber", &[Object, String], Unit, "tondo_rt_msgpack_write_number"),
        msgpack("writeFloat", &[Object, Float], Unit, "tondo_rt_msgpack_write_float"),
        msgpack("writeText", &[Object, String], Unit, "tondo_rt_msgpack_write_text"),
        msgpack("writeBytes", &[Object, Bytes], Unit, "tondo_rt_msgpack_write_bytes"),
        msgpack("writeDateTime", &[Object, String], Unit, "tondo_rt_msgpack_write_datetime"),
        msgpack("writeKey", &[Object, String], Unit, "tondo_rt_msgpack_write_key"),
        msgpack("writeOpen", &[Object, Bool, Int], Unit, "tondo_rt_msgpack_write_open"),
        msgpack("writeClose", &[Object], Unit, "tondo_rt_msgpack_write_close"),
        msgpack("finish", &[Object], Bytes, "tondo_rt_msgpack_finish"),
        msgpack("failure", &[Object], String, "tondo_rt_msgpack_failure"),
        msgpack("streamWriter", &[IoWriter], Object, "tondo_rt_msgpack_stream_writer"),
        msgpack("writeHeader", &[Object, Bool, Int], Unit, "tondo_rt_msgpack_write_header"),
        msgpack("flush", &[Object], String, "tondo_rt_msgpack_flush"),
        cbor("parse", &[Bytes], Object, "tondo_rt_cbor_parse"),
        cbor("next", &[Object], Int, "tondo_rt_cbor_next"),
        cbor("text", &[Object], String, "tondo_rt_cbor_text"),
        cbor("int", &[Object], Int, "tondo_rt_cbor_int"),
        cbor("float", &[Object], Float, "tondo_rt_cbor_float"),
        cbor("bytes", &[Object], Bytes, "tondo_rt_cbor_bytes"),
        cbor("skip", &[Object, Int], Int, "tondo_rt_cbor_skip"),
        cbor("fail", &[Object, String], Unit, "tondo_rt_cbor_fail"),
        cbor("where", &[Object, Int], String, "tondo_rt_cbor_where"),
        cbor("writer", &[Bool], Object, "tondo_rt_cbor_writer"),
        cbor("writeNull", &[Object], Unit, "tondo_rt_cbor_write_null"),
        cbor("writeBool", &[Object, Bool], Unit, "tondo_rt_cbor_write_bool"),
        cbor("writeInt", &[Object, Int], Unit, "tondo_rt_cbor_write_int"),
        cbor("writeNumber", &[Object, String], Unit, "tondo_rt_cbor_write_number"),
        cbor("writeFloat", &[Object, Float], Unit, "tondo_rt_cbor_write_float"),
        cbor("writeText", &[Object, String], Unit, "tondo_rt_cbor_write_text"),
        cbor("writeBytes", &[Object, Bytes], Unit, "tondo_rt_cbor_write_bytes"),
        cbor("writeDateTime", &[Object, String], Unit, "tondo_rt_cbor_write_datetime"),
        cbor("writeKey", &[Object, String], Unit, "tondo_rt_cbor_write_key"),
        cbor("writeOpen", &[Object, Bool, Int], Unit, "tondo_rt_cbor_write_open"),
        cbor("writeClose", &[Object], Unit, "tondo_rt_cbor_write_close"),
        cbor("finish", &[Object], Bytes, "tondo_rt_cbor_finish"),
        cbor("failure", &[Object], String, "tondo_rt_cbor_failure"),
        net("connect", &[String, Int, Int, Bool, String, String, String, Int], Object, "tondo_rt_net_connect"),
        net("startTls", &[Object, String, String, String, String, Int, Int], Object, "tondo_rt_net_start_tls"),
        net("read", &[Object, Int, Int], Bytes, "tondo_rt_net_read"),
        net("readLine", &[Object, Int], String, "tondo_rt_net_read_line"),
        net("readAll", &[Object, Int], Bytes, "tondo_rt_net_read_all"),
        net("write", &[Object, String, Int], Int, "tondo_rt_net_write"),
        net("writeBytes", &[Object, Bytes, Int], Int, "tondo_rt_net_write_bytes"),
        net("close", &[Object, Bool, Int], Int, "tondo_rt_net_close"),
        net("reader", &[Object], IoReader, "tondo_rt_net_reader"),
        net("writer", &[Object], IoWriter, "tondo_rt_net_writer"),
        net("listen", &[String, Int, String, String, String, Int], Object, "tondo_rt_net_listen"),
        net("certificates", &[Strings], String, "tondo_rt_tls_certificates"),
        net("certificatesSet", &[String, String, String, String, String], String, "tondo_rt_tls_certificates_set"),
        net("certificatesServing", &[String], Bool, "tondo_rt_tls_certificates_serving"),
        net("accept", &[Object, Int, Bool], Object, "tondo_rt_net_accept"),
        net("stop", &[Object], Unit, "tondo_rt_net_stop"),
        net("bind", &[String, Int], Object, "tondo_rt_net_bind"),
        net("sendTo", &[Object, Bytes, String, Int], Int, "tondo_rt_net_send_to"),
        net("receive", &[Object, Int], Object, "tondo_rt_net_receive"),
        net("datagram", &[Object], Bytes, "tondo_rt_net_datagram"),
        net("resolve", &[String, Int], Object, "tondo_rt_net_resolve"),
        net("names", &[Object], Strings, "tondo_rt_net_names"),
        net("ip", &[String], String, "tondo_rt_net_ip"),
        net("canonicalIp", &[String], String, "tondo_rt_net_canonical_ip"),
        net("ipKind", &[String], Int, "tondo_rt_net_ip_kind"),
        net("network", &[String], String, "tondo_rt_net_network"),
        net("contains", &[String, String], Bool, "tondo_rt_net_contains"),
        net("addressIp", &[Object, Int], String, "tondo_rt_net_address_ip"),
        net("addressPort", &[Object, Int], Int, "tondo_rt_net_address_port"),
        net("addressPath", &[Object, Int], String, "tondo_rt_net_address_path"),
        net("connectUnix", &[String, Int], Object, "tondo_rt_net_connect_unix"),
        net("listenUnix", &[String], Object, "tondo_rt_net_listen_unix"),
        net("bindUnix", &[String, Bool], Object, "tondo_rt_net_bind_unix"),
        net("sendToPath", &[Object, Bytes, String], Int, "tondo_rt_net_send_to_path"),
        net("failure", &[Object, Int], Int, "tondo_rt_net_failure"),
        net("detail", &[Object, Int], String, "tondo_rt_net_detail"),
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
        // `std.internal.fuzz`: the inputs of `testing.fuzz` (testing 10.6).
        fuzz("begin", &[String, String], Unit, "tondo_rt_fuzz_begin"),
        fuzz("seed", &[String], Unit, "tondo_rt_fuzz_seed"),
        fuzz("next", &[], Bool, "tondo_rt_fuzz_next"),
        fuzz("input", &[], String, "tondo_rt_fuzz_input"),
        fuzz("invalid", &[String], Unit, "tondo_rt_fuzz_invalid"),
        fuzz("failed", &[], Unit, "tondo_rt_fuzz_failed"),
        fuzz("end", &[], Unit, "tondo_rt_fuzz_end"),
        fuzz("replaying", &[], Bool, "tondo_rt_fuzz_replaying"),
        fuzz("encoded", &[String], Unit, "tondo_rt_fuzz_encoded"),
        // `std.internal.panic`: what a panic carries beyond its message (15.7).
        panics("code", &[String], Unit, "tondo_rt_panic_code"),
        panics("int", &[String, Int], Unit, "tondo_rt_panic_int"),
        panics("float", &[String, Float], Unit, "tondo_rt_panic_float"),
        panics("bool", &[String, Bool], Unit, "tondo_rt_panic_bool"),
        panics("text", &[String, String], Unit, "tondo_rt_panic_text"),
        panics("caughtCode", &[], String, "tondo_rt_caught_code"),
        panics("caughtMessage", &[], String, "tondo_rt_caught_message"),
        panics("caughtLocation", &[], String, "tondo_rt_caught_location"),
        panics("caughtStack", &[], String, "tondo_rt_caught_stack"),
        panics("caughtCount", &[], Int, "tondo_rt_caught_count"),
        panics("caughtKey", &[Int], String, "tondo_rt_caught_key"),
        panics("caughtKind", &[Int], Int, "tondo_rt_caught_kind"),
        panics("caughtInt", &[Int], Int, "tondo_rt_caught_int"),
        panics("caughtFloat", &[Int], Float, "tondo_rt_caught_float"),
        panics("caughtBool", &[Int], Bool, "tondo_rt_caught_bool"),
        panics("caughtText", &[Int], String, "tondo_rt_caught_text"),
        panics("name", &[String], String, "tondo_rt_panic_name"),
        panics("restage", &[String, String, String], Unit, "tondo_rt_panic_restage"),
        panics("report", &[String, String, String, String], Unit, "tondo_rt_panic_report"),
        time("zoneKnown", &[String], Bool, "tondo_rt_zone_known"),
        time("zoneLocal", &[], String, "tondo_rt_zone_local"),
        time("zoneOffset", &[String, Int], Int, "tondo_rt_zone_offset"),
        time("zoneResolve", &[String, Ints], Int, "tondo_rt_zone_resolve"),
        time("zoneAbbreviation", &[String, Int], String, "tondo_rt_zone_abbreviation"),
        time("names", &[String], Strings, "tondo_rt_time_names"),
        // `std.internal.time`: how long the task has until its deadline
        // (11.17), in nanoseconds, or -1 without one.
        Native {
            module: "time",
            name: "remaining",
            receiver: None,
            params: &[],
            result: Int,
            symbol: "tondo_rt_deadline_remaining",
        },
        // `std.internal.context`: the fields of the task's contexts marked
        // `@log` (11.16), each name and text followed by a zero character.
        Native {
            module: "context",
            name: "logged",
            receiver: None,
            params: &[],
            result: String,
            symbol: "tondo_rt_context_logged",
        },
    ]
};

/// The locals an expression may change: those it lends to a call or a
/// method, changes in place or assigns, in any block it holds. A closure
/// changes only its own copies of what it captures (11.8), so calling one
/// changes none.
pub fn written(e: &Expr, out: &mut Vec<LocalId>) {
    let many = |items: &[Expr], out: &mut Vec<LocalId>| items.iter().for_each(|item| written(item, out));
    match &e.kind {
        ExprKind::Loan(place) | ExprKind::CursorNext(place, _) => out.push(place.local),
        ExprKind::Region(place, bounds) => {
            out.push(place.local);
            bounds.iter().flatten().for_each(|bound| written(bound, out));
        }
        ExprKind::Mutate(_, place, arguments) => {
            out.push(place.local);
            many(arguments, out);
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::FnValue(..)
        | ExprKind::ExternFn(_)
        | ExprKind::Layout(..)
        | ExprKind::ContextRead(..)
        | ExprKind::Error => {}
        ExprKind::Aggregate(items)
        | ExprKind::Array(items)
        | ExprKind::Set(items)
        | ExprKind::ArrayOp(_, items)
        | ExprKind::StrOp(_, items)
        | ExprKind::Pointer(_, items)
        | ExprKind::MapOp(_, items)
        | ExprKind::Call(_, _, items)
        | ExprKind::Reflect(_, _, items)
        | ExprKind::TaskOp(_, items) => many(items, out),
        ExprKind::Record(values) | ExprKind::Variant(_, values) | ExprKind::TestMembers(values) => {
            values.iter().for_each(|(_, value)| written(value, out))
        }
        ExprKind::Field(inner, _)
        | ExprKind::Retype(inner)
        | ExprKind::RefValue(inner)
        | ExprKind::Length(inner)
        | ExprKind::Try(inner)
        | ExprKind::Convert(inner)
        | ExprKind::Fixed(inner)
        | ExprKind::Erase(inner, _)
        | ExprKind::Callback(inner, _)
        | ExprKind::Spawn(inner)
        | ExprKind::Unary(_, inner)
        | ExprKind::Panic(inner, _) => written(inner, out),
        ExprKind::With(base, values) => {
            written(base, out);
            values.iter().for_each(|(_, value)| written(value, out));
        }
        ExprKind::Match(scrutinee, arms) => {
            written(scrutinee, out);
            for arm in arms {
                arm.guard.iter().for_each(|guard| written(guard, out));
                written(&arm.body, out);
            }
        }
        ExprKind::Index(left, right) | ExprKind::Binary(_, left, right) => {
            written(left, out);
            written(right, out);
        }
        ExprKind::Slice(base, bounds) => {
            written(base, out);
            bounds.iter().flatten().for_each(|bound| written(bound, out));
        }
        ExprKind::Map(entries) => entries.iter().for_each(|(key, value)| {
            written(key, out);
            written(value, out);
        }),
        ExprKind::CallValue(function, arguments) => {
            written(function, out);
            many(arguments, out);
        }
        ExprKind::Select(arms, otherwise) => {
            for arm in arms {
                written(&arm.operation, out);
                written(&arm.body, out);
            }
            otherwise.iter().for_each(|otherwise| written(otherwise, out));
        }
        ExprKind::If(condition, then, otherwise) => {
            written(condition, out);
            written_block(then, out);
            otherwise.iter().for_each(|otherwise| written(otherwise, out));
        }
        ExprKind::Block(block) => written_block(block, out),
        ExprKind::Recover(block, caught) => {
            written_block(block, out);
            written(caught, out);
        }
        ExprKind::Within(limit, block, failure) => {
            written(limit, out);
            written_block(block, out);
            written(failure, out);
        }
        ExprKind::Interpolate(segments) => segments.iter().for_each(|segment| {
            if let Segment::Value(value) = segment {
                written(value, out);
            }
        }),
        ExprKind::Assert { condition, parts, detail, .. } => {
            written(condition, out);
            many(parts, out);
            detail.iter().for_each(|detail| written(detail, out));
        }
    }
}

fn written_block(block: &Block, out: &mut Vec<LocalId>) {
    for statement in &block.stmts {
        match statement {
            Stmt::Let(_, value) | Stmt::Destructure(_, value) | Stmt::Expr(value) | Stmt::Fail(value) => {
                written(value, out)
            }
            Stmt::Assign { local, value, .. } => {
                out.push(*local);
                written(value, out);
            }
            Stmt::AssignSlice { local, bounds, value, .. } => {
                out.push(*local);
                bounds.iter().flatten().for_each(|bound| written(bound, out));
                written(value, out);
            }
            Stmt::Defer { closure, .. } => written(closure, out),
            Stmt::Lend { place, .. } => out.push(place.local),
            Stmt::LockSync { handle, .. } => written(handle, out),
            Stmt::ContextSet { value, .. } => written(value, out),
            Stmt::Return(value) => value.iter().for_each(|value| written(value, out)),
            Stmt::AwaitChange(..) | Stmt::Break | Stmt::Continue => {}
            Stmt::Loop { condition, body } => {
                condition.iter().for_each(|condition| written(condition, out));
                written_block(body, out);
            }
            Stmt::ForEach { source, body, .. } => {
                written(source, out);
                written_block(body, out);
            }
            Stmt::ForRange { start, end, body, .. } => {
                written(start, out);
                written(end, out);
                written_block(body, out);
            }
        }
    }
    block.tail.iter().for_each(|tail| written(tail, out));
}
