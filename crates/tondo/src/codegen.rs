//! MIR to native object code with Cranelift.
//!
//! Values have one of three classes. Zero-sized values have no storage.
//! Scalars (integers, `Bool`, string pointers, payload-free enums) live in SSA
//! variables. Aggregates (tuples, records, enums with payloads) live in memory
//! with a C-like layout: locals get stack slots, parameters arrive as pointers
//! to the caller's memory, and results are written through a pointer the
//! caller passes first.
//!
//! Where a recursive type contains itself, the component lives in a box: a
//! reference-counted heap cell (see `Types::is_boxed`). Boxes are shared on
//! copy and made unique before a write through them, which keeps value
//! semantics. Per-type "glue" functions retain, release and compare values
//! that contain boxes or enums; the runtime calls the release glue when the
//! last reference to a box goes away.

use crate::ast::{BinOp, UnaryOp};
use crate::hir::{
    ArrayOp, AtomicOp, Callee, EnvFn, FnId, FsFn, IoFn, LockMode, MapOp, Mutation, PathFn, PointerOp, ProcessFn,
    Program, ReflectOp, SharedOp, StdFn, StrOp, TaskOp,
};
use crate::mir::{
    Body, Const, Local, Operand, Origin, Overlap, Piece, Place, Proj, Rvalue, SelectCase, Statement, Terminator,
    project,
};
use crate::numbers::NumOp;
use crate::source::{SourceMap, Span};
use crate::types::{AdtKind, ERR, FloatTy, Handle, IntTy, Mode, OK, SOME, Shown, Ty, TyKind, Types};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    self, AbiParam, ArgumentPurpose, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, TrapCode, UserFuncName,
    types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use std::collections::HashSet;

mod cabi;
use cabi::{CTarget, Callback, define_adapter, define_inner};
mod worker;

const POINTER: ir::Type = types::I64;
const IMMORTAL: u64 = u64::MAX;
/// Size of a function value's header in the runtime (`FnObject`): the code
/// is at 16 and the captures follow the header.
const FN_HEADER: i64 = 40;
/// Size of a box header in the runtime (`BoxHeader`).
const BOX_HEADER: i64 = 16;
/// The size of the header of a `Ref[T]`'s object: its strong and weak
/// counts, the size of the value that follows, and whether it is garbage.
const REF_HEADER: i64 = 32;
/// Where the value starts in the box of a `reflect.Value` (`BoxObject`).
const REFLECT_VALUE: i64 = 40;
/// Size of an array header in the runtime (`ArrayHeader`): `len` is at 8 and
/// a deque's `head` at 24. An array's `head` is always 0.
const ARRAY_HEADER: i64 = 32;
/// Where a string's length is in its header (`Str`), after its count, its
/// room after that, and where its bytes start.
const STRING_LENGTH: i32 = 8;
const STRING_ROOM: i32 = 16;
const STRING_BYTES: i64 = 24;
/// The count of a static array: so many references that releases never
/// bring it to one, so that a change copies it, as one of a shared array.
const SHARED_FOREVER: u64 = 1 << 62;

/// Runtime entry points (see `tondo-runtime`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Rt {
    /// A live build's tables, before anything runs (toolchain 7.10).
    Live,
    /// A frame's state for the new code, and its release once taken.
    LiveTransfer,
    LiveFree,
    LiveArgsDone,
    LiveObject,
    StrRetain,
    StrRelease,
    StrEq,
    /// What the counted code of a build for `--fuzz` compares.
    FuzzCompare,
    FuzzCompareText,
    StrCmp,
    StrLength,
    StrGet,
    StrAt,
    StrSlice,
    StrConcat,
    StrDecode,
    StrNext,
    StrTest,
    StrFind,
    StrReplace,
    StrTransform,
    StrSliceChecked,
    StrFromChars,
    SbStart,
    SbJoin,
    SbAdopt,
    StrAppend,
    StrAppendInt,
    SbStr,
    SbI64,
    SbU64,
    SbInt128,
    IntPow,
    IntParse,
    IntRadix,
    IntToBytes,
    IntFromBytes,
    FloatParse,
    MathPow,
    MathRoundAway,
    TextFixed,
    Int128Multiply,
    Int128Divide,
    Int128ToFloat,
    FloatToInt128,
    SbBool,
    SbF64,
    SbF32,
    SbChar,
    SbFinish,
    ConsolePrint,
    ConsolePrintln,
    ConsoleFlush,
    ConsoleEprint,
    ConsoleEprintln,
    ConsoleReadLine,
    ConsoleStream,
    ConsoleSet,
    Panic,
    PanicValues,
    Trace,
    /// A debug build's table of probes, and each probe (toolchain 7.6).
    DebugTable,
    MetricsWatch,
    DebugLine,
    SbQuoted,
    BoxNew,
    BoxRetain,
    BoxRelease,
    BoxUnique,
    RefNew,
    RefRelease,
    WeakNew,
    WeakRetain,
    WeakRelease,
    WeakGet,
    GcEdge,
    ReportError,
    TestStart,
    CoverageStart,
    TestMembers,
    TestError,
    TestLog,
    TestTags,
    TestWhere,
    TestAttach,
    TestSnapshot,
    ClockBegin,
    ClockEnd,
    ClockSettle,
    ClockAdvance,
    ClockRelease,
    BenchStart,
    BenchNext,
    BenchEnd,
    Keep,
    RegexRelease,
    NativeRelease,
    ReflectBox,
    ReflectRelease,
    ReflectIs,
    MetaStep,
    ArrayNew,
    ArrayRetain,
    ArrayRelease,
    ArrayUnique,
    ArrayPush,
    ArrayPop,
    ArraySlice,
    ArraySliceChecked,
    ArrayConcat,
    ArraySortedInts,
    IntText,
    ArraySortedStrings,
    ArrayRepeat,
    ArrayEq,
    BytesEq,
    ArrayContains,
    ArrayAssignSlice,
    ArrayCheckRegion,
    ArrayArith,
    ArrayInsert,
    ArrayRemoveAt,
    ArrayClear,
    DequePush,
    DequePopFront,
    SbArray,
    MapNew,
    MapRetain,
    UnwindPass,
    MapRelease,
    MapFind,
    MapInsert,
    MapRemove,
    MapLen,
    MapEntry,
    MapUnique,
    MapEq,
    MapClear,
    FnNew,
    FnRetain,
    FnRelease,
    DeferPush,
    DeferPop,
    SbMap,
    StrHash,
    Spawn,
    JoinWait,
    JoinRelease,
    HandleRetain,
    GroupNew,
    GroupAdd,
    GroupLength,
    GroupNext,
    GroupSettle,
    GroupTake,
    GroupRelease,
    ChannelNew,
    SenderNew,
    SenderCopy,
    ChannelSend,
    ChannelOffer,
    ChannelReceive,
    ChannelClose,
    ChannelClosed,
    SenderRelease,
    ReceiverRelease,
    Sleep,
    TimeNow,
    TimeUnix,
    TimeUtc,
    TimeUtcNow,
    SbDuration,
    SbDateTime,
    ReceiveTimeout,
    Select,
    Cancelled,
    PanicUnwind,
    PanicValuesUnwind,
    Unwinding,
    RecoverEnter,
    RecoverEnd,
    RecoverLeave,
    RecoverCatch,
    UnwindHold,
    UnwindResume,
    UnwindReturn,
    DeferEnd,
    CancelReturn,
    WithinEnter,
    WithinLeave,
    WithinCatch,
    SpawnDetached,
    ContextRegister,
    ContextGet,
    ContextField,
    ContextComputed,
    ContextShows,
    ContextSet,
    ContextRestore,
    JoinWaitTimeout,
    JoinCancel,
    JoinDetach,
    GroupCancel,
    SyncNew,
    SyncValue,
    SyncLock,
    SyncUnlock,
    SyncAwait,
    SyncRelease,
    SemaphoreNew,
    SemaphoreAcquire,
    SemaphoreRelease,
    PermitRelease,
    BarrierNew,
    BarrierWait,
    BarrierRelease,
    SharedMapFrom,
    SharedMapGet,
    SharedMapInsert,
    SharedMapRemove,
    SharedMapExchange,
    SharedMapLength,
    SharedMapSnapshot,
    SharedMapRelease,
    SharedQueueFrom,
    SharedQueuePush,
    SharedQueuePop,
    SharedQueueGet,
    SharedQueueLength,
    SharedQueueSnapshot,
    SharedQueueRelease,
    SharedStackFrom,
    SharedStackPush,
    SharedStackPop,
    SharedStackGet,
    SharedStackLength,
    SharedStackSnapshot,
    SharedStackRelease,
    SharedArrayFrom,
    SharedArrayGet,
    SharedArraySet,
    SharedArrayExchange,
    SharedArrayLength,
    SharedArraySnapshot,
    SharedArrayRelease,
    AtomicLoad,
    AtomicStore,
    AtomicSwap,
    AtomicExchange,
    AtomicAdd,
    Fs(FsFn),
    Io(IoFn),
    Env(EnvFn),
    Process(ProcessFn),
    Native(u16),
    JsonReaderRelease,
    JsonWriterRelease,
    PlanRelease,
    ProcessRelease,
    SbProcessError,
    StreamRelease,
    BufferRelease,
    PathJoin,
    PathPart,
    PathIsAbsolute,
    StrToBytes,
    StrFromBytes,
    /// Around a call of C: the worker is in C until it returns.
    ForeignEnter,
    ForeignLeave,
    /// A `String` passed to C: a copy that ends with a zero byte, freed
    /// after the call.
    CString,
    CStringFree,
    /// Calls a C function with `...` from a frame of registers.
    CVariadic,
    /// Runs the body of a callback C made.
    Callback,
    /// Runs the program, from the process's `main`.
    Main,
}

impl Rt {
    fn signature(self) -> (&'static str, &'static [ir::Type], &'static [ir::Type]) {
        use types::{I8, I32, I64};
        match self {
            Rt::StrRetain => ("tondo_rt_str_retain", &[I64], &[]),
            Rt::StrRelease => ("tondo_rt_str_release", &[I64], &[]),
            Rt::StrEq => ("tondo_rt_str_eq", &[I64, I64], &[I8]),
            Rt::FuzzCompare => ("tondo_rt_fuzz_compare", &[I64, I64], &[]),
            Rt::FuzzCompareText => ("tondo_rt_fuzz_compare_text", &[I64, I64], &[]),
            Rt::StrCmp => ("tondo_rt_str_cmp", &[I64, I64], &[I32]),
            Rt::StrLength => ("tondo_rt_str_length", &[I64], &[I64]),
            Rt::StrGet => ("tondo_rt_str_get", &[I64, I64], &[I64]),
            Rt::StrAt => ("tondo_rt_str_at", &[I64, I64, I64], &[I32]),
            Rt::StrSlice => ("tondo_rt_str_slice", &[I64, I64, I8, I64, I8, I64, I64], &[I64]),
            Rt::StrConcat => ("tondo_rt_str_concat", &[I64, I64], &[I64]),
            Rt::StrDecode => ("tondo_rt_str_decode", &[I64, I64], &[I32]),
            Rt::StrNext => ("tondo_rt_str_next", &[I64, I64], &[I64]),
            Rt::StrTest => ("tondo_rt_str_test", &[I64, I64, I32], &[I8]),
            Rt::StrFind => ("tondo_rt_str_find", &[I64, I64], &[I64]),
            Rt::StrReplace => ("tondo_rt_str_replace", &[I64, I64, I64], &[I64]),
            Rt::StrTransform => ("tondo_rt_str_transform", &[I64, I32], &[I64]),
            Rt::StrSliceChecked => ("tondo_rt_str_slice_checked", &[I64, I64, I64, I64], &[I64]),
            Rt::StrFromChars => ("tondo_rt_str_from_chars", &[I64], &[I64]),
            Rt::SbStart => ("tondo_rt_sb_start", &[I64, I64], &[]),
            Rt::SbJoin => ("tondo_rt_sb_join", &[I64, I64, I64, I64, I64], &[]),
            Rt::SbAdopt => ("tondo_rt_sb_adopt", &[I64, I64, I64], &[I8]),
            Rt::StrAppend => ("tondo_rt_str_append", &[I64, I64], &[I64]),
            Rt::StrAppendInt => ("tondo_rt_str_append_int", &[I64, I64], &[I64]),
            Rt::SbStr => ("tondo_rt_sb_str", &[I64, I64], &[]),
            Rt::SbI64 => ("tondo_rt_sb_i64", &[I64, I64], &[]),
            Rt::SbU64 => ("tondo_rt_sb_u64", &[I64, I64], &[]),
            Rt::SbInt128 => ("tondo_rt_sb_int128", &[I64, I64, I64, I8], &[]),
            Rt::IntPow => ("tondo_rt_int_pow", &[I8, I8, I32, I64, I64], &[I8]),
            Rt::IntParse => ("tondo_rt_int_parse", &[I64, I64, I32, I8, I64], &[I8]),
            Rt::IntRadix => ("tondo_rt_int_radix", &[I64, I64, I8, I64], &[I64]),
            Rt::IntToBytes => ("tondo_rt_int_to_bytes", &[I64, I64, I64, I8], &[I64]),
            Rt::IntFromBytes => ("tondo_rt_int_from_bytes", &[I64, I64, I64, I8, I64, I64], &[]),
            Rt::FloatParse => ("tondo_rt_float_parse", &[I64, I8, I64], &[I8]),
            Rt::MathPow => ("tondo_rt_math_pow", &[types::F64, types::F64], &[types::F64]),
            Rt::MathRoundAway => ("tondo_rt_math_round_away", &[types::F64], &[types::F64]),
            Rt::TextFixed => ("tondo_rt_text_fixed", &[types::F64, I64], &[I64]),
            Rt::Int128Multiply => ("tondo_rt_int128_multiply", &[I8, I64, I64], &[I8]),
            Rt::Int128Divide => ("tondo_rt_int128_divide", &[I8, I8, I64, I64], &[]),
            Rt::Int128ToFloat => ("tondo_rt_int128_to_float", &[I64, I64, I8, I8], &[types::F64]),
            Rt::FloatToInt128 => ("tondo_rt_float_to_int128", &[types::F64, I8, I64], &[]),
            Rt::SbBool => ("tondo_rt_sb_bool", &[I64, I8], &[]),
            Rt::SbF64 => ("tondo_rt_sb_f64", &[I64, types::F64], &[]),
            Rt::SbF32 => ("tondo_rt_sb_f32", &[I64, types::F32], &[]),
            Rt::SbChar => ("tondo_rt_sb_char", &[I64, I32], &[]),
            Rt::SbFinish => ("tondo_rt_sb_finish", &[I64], &[I64]),
            Rt::ConsolePrint => ("tondo_rt_console_print", &[I64], &[]),
            Rt::ConsolePrintln => ("tondo_rt_console_println", &[I64], &[]),
            Rt::ConsoleFlush => ("tondo_rt_console_flush", &[], &[I64]),
            Rt::ConsoleEprint => ("tondo_rt_console_eprint", &[I64], &[]),
            Rt::ConsoleEprintln => ("tondo_rt_console_eprintln", &[I64], &[]),
            Rt::ConsoleReadLine => ("tondo_rt_console_read_line", &[], &[I64]),
            Rt::ConsoleStream => ("tondo_rt_console_stream", &[I64], &[I64]),
            Rt::ConsoleSet => ("tondo_rt_console_set", &[I64, I64], &[]),
            Rt::Panic => ("tondo_rt_panic", &[I32, I64, I64], &[]),
            Rt::PanicValues => ("tondo_rt_panic_values", &[I32, I32, I64, I64, I64], &[]),
            Rt::Trace => ("tondo_rt_trace", &[I64], &[]),
            Rt::DebugTable => ("tondo_rt_debug_table", &[I64], &[]),
            Rt::MetricsWatch => ("tondo_rt_metrics_watch", &[], &[]),
            Rt::DebugLine => ("tondo_rt_debug_line", &[I64, I64, I64], &[I8]),
            Rt::SbQuoted => ("tondo_rt_sb_quoted", &[I64, I64], &[]),
            Rt::BoxNew => ("tondo_rt_box_new", &[I64], &[I64]),
            Rt::BoxRetain => ("tondo_rt_box_retain", &[I64], &[]),
            Rt::BoxRelease => ("tondo_rt_box_release", &[I64, I64], &[]),
            Rt::RefNew => ("tondo_rt_ref_new", &[I64], &[I64]),
            Rt::RefRelease => ("tondo_rt_ref_release", &[I64, I64], &[]),
            Rt::WeakNew => ("tondo_rt_weak_new", &[I64], &[I64]),
            Rt::WeakRetain => ("tondo_rt_weak_retain", &[I64], &[]),
            Rt::WeakRelease => ("tondo_rt_weak_release", &[I64], &[]),
            Rt::WeakGet => ("tondo_rt_weak_get", &[I64], &[I64]),
            Rt::GcEdge => ("tondo_rt_gc_edge", &[I64, I64, I8, I64, I64], &[]),
            Rt::BoxUnique => ("tondo_rt_box_unique", &[I64, I64, I64], &[]),
            Rt::ReportError => ("tondo_rt_report_error", &[I64, I64], &[]),
            Rt::TestStart => ("tondo_rt_test_start", &[I64, I64], &[]),
            Rt::CoverageStart => ("tondo_rt_coverage_start", &[I64, I64], &[]),
            Rt::TestMembers => ("tondo_rt_test_members", &[I64, I64], &[]),
            Rt::TestError => ("tondo_rt_test_error", &[I64, I64], &[]),
            Rt::TestLog => ("tondo_rt_test_log", &[I64], &[]),
            Rt::TestTags => ("tondo_rt_test_tags", &[I64], &[]),
            Rt::TestWhere => ("tondo_rt_test_where", &[I64], &[]),
            Rt::TestAttach => ("tondo_rt_test_attach", &[I64, I64, I64, I64], &[]),
            Rt::TestSnapshot => ("tondo_rt_test_snapshot", &[I64, I64, I8, I64, I64, I64, I64], &[]),
            Rt::ClockBegin => ("tondo_rt_clock_begin", &[I64], &[I64]),
            Rt::ClockEnd => ("tondo_rt_clock_end", &[], &[]),
            Rt::ClockSettle => ("tondo_rt_clock_settle", &[I64], &[]),
            Rt::ClockAdvance => ("tondo_rt_clock_advance", &[I64, I64, I64], &[]),
            Rt::ClockRelease => ("tondo_rt_clock_release", &[I64], &[]),
            Rt::BenchStart => ("tondo_rt_bench_start", &[I64, I64], &[]),
            Rt::BenchNext => ("tondo_rt_bench_next", &[], &[I64]),
            Rt::BenchEnd => ("tondo_rt_bench_end", &[I8], &[]),
            Rt::Keep => ("tondo_rt_keep", &[I64], &[]),
            Rt::RegexRelease => ("tondo_rt_regex_release", &[I64], &[]),
            Rt::NativeRelease => ("tondo_rt_native_release", &[I64], &[]),
            Rt::ReflectBox => ("tondo_rt_reflect_box", &[I64, I64, I64, I64], &[I64]),
            Rt::ReflectRelease => ("tondo_rt_reflect_release", &[I64], &[]),
            Rt::ReflectIs => ("tondo_rt_reflect_is", &[I64, I64], &[I8]),
            Rt::MetaStep => ("tondo_rt_meta_step", &[], &[]),
            Rt::ArrayNew => ("tondo_rt_array_new", &[I64, I64], &[I64]),
            Rt::ArrayRetain => ("tondo_rt_array_retain", &[I64], &[]),
            Rt::ArrayRelease => ("tondo_rt_array_release", &[I64, I64, I64], &[]),
            Rt::ArrayUnique => ("tondo_rt_array_unique", &[I64, I64, I64, I64, I64], &[]),
            Rt::ArrayPush => ("tondo_rt_array_push", &[I64, I64, I64, I64, I64], &[]),
            Rt::ArrayPop => ("tondo_rt_array_pop", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::ArraySlice => ("tondo_rt_array_slice", &[I64, I64, I64, I64, I8, I64, I8, I64, I64], &[I64]),
            Rt::ArraySliceChecked => ("tondo_rt_array_slice_checked", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayConcat => ("tondo_rt_array_concat", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArraySortedInts => ("tondo_rt_array_sorted_ints", &[I64], &[I64]),
            Rt::IntText => ("tondo_rt_int_text", &[I64], &[I64]),
            Rt::ArraySortedStrings => ("tondo_rt_array_sorted_strings", &[I64], &[I64]),
            Rt::ArrayRepeat => ("tondo_rt_array_repeat", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayEq => ("tondo_rt_array_eq", &[I64, I64, I64, I64], &[I8]),
            Rt::BytesEq => ("tondo_rt_bytes_eq", &[I64, I64], &[I8]),
            Rt::ArrayContains => ("tondo_rt_array_contains", &[I64, I64, I64, I64], &[I8]),
            Rt::ArrayAssignSlice => {
                ("tondo_rt_array_assign_slice", &[I64, I64, I8, I64, I8, I64, I64, I64, I64, I64, I64], &[])
            }
            Rt::ArrayCheckRegion => {
                ("tondo_rt_array_check_region", &[I64, I64, I8, I64, I8, I64, I8, I64, I8, I64, I8, I64, I64], &[])
            }
            Rt::ArrayArith => ("tondo_rt_array_arith", &[I64, I32, I64, I32, I32, I32, I64], &[I64]),
            Rt::ArrayInsert => ("tondo_rt_array_insert", &[I64, I64, I64, I64, I64, I64, I64], &[]),
            Rt::ArrayRemoveAt => ("tondo_rt_array_remove_at", &[I64, I64, I64, I64, I64, I64], &[I8]),
            Rt::ArrayClear => ("tondo_rt_array_clear", &[I64, I64, I64], &[]),
            Rt::DequePush => ("tondo_rt_deque_push", &[I64, I64, I64, I8, I64, I64], &[]),
            Rt::DequePopFront => ("tondo_rt_deque_pop_front", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::SbArray => ("tondo_rt_sb_array", &[I64, I64, I64, I64], &[]),
            Rt::MapNew => ("tondo_rt_map_new", &[I64, I64], &[I64]),
            Rt::MapRetain => ("tondo_rt_map_retain", &[I64], &[]),
            Rt::MapRelease => ("tondo_rt_map_release", &[I64, I64], &[]),
            Rt::MapFind => ("tondo_rt_map_find", &[I64, I64, I64], &[I64]),
            Rt::MapInsert => ("tondo_rt_map_insert", &[I64, I64, I64, I64, I64], &[I8]),
            Rt::MapRemove => ("tondo_rt_map_remove", &[I64, I64, I64, I64], &[I8]),
            Rt::MapLen => ("tondo_rt_map_len", &[I64], &[I64]),
            Rt::MapEntry => ("tondo_rt_map_entry", &[I64, I64], &[I64]),
            Rt::MapUnique => ("tondo_rt_map_unique", &[I64, I64], &[]),
            Rt::MapEq => ("tondo_rt_map_eq", &[I64, I64, I64, I64], &[I8]),
            Rt::MapClear => ("tondo_rt_map_clear", &[I64, I64], &[]),
            Rt::FnNew => ("tondo_rt_fn_new", &[I64, I64, I64, I64], &[I64]),
            Rt::FnRetain => ("tondo_rt_fn_retain", &[I64], &[]),
            Rt::FnRelease => ("tondo_rt_fn_release", &[I64], &[]),
            Rt::DeferPush => ("tondo_rt_defer_push", &[I64, I64], &[]),
            Rt::DeferPop => ("tondo_rt_defer_pop", &[], &[]),
            Rt::SbMap => ("tondo_rt_sb_map", &[I64, I64, I64, I64], &[]),
            Rt::StrHash => ("tondo_rt_str_hash", &[I64], &[I64]),
            Rt::Spawn => ("tondo_rt_spawn", &[I64, I8, I64, I64, I64, I64], &[I64]),
            Rt::JoinWait => ("tondo_rt_join_wait", &[I64, I64], &[]),
            Rt::JoinRelease => ("tondo_rt_join_release", &[I64], &[]),
            Rt::HandleRetain => ("tondo_rt_handle_retain", &[I64], &[]),
            Rt::GroupNew => ("tondo_rt_group_new", &[I8], &[I64]),
            Rt::GroupAdd => ("tondo_rt_group_add", &[I64, I64], &[]),
            Rt::GroupLength => ("tondo_rt_group_length", &[I64], &[I64]),
            Rt::GroupNext => ("tondo_rt_group_next", &[I64, I64], &[I8]),
            Rt::GroupSettle => ("tondo_rt_group_settle", &[I64], &[I64]),
            Rt::GroupTake => ("tondo_rt_group_take", &[I64, I64, I64], &[I8]),
            Rt::GroupRelease => ("tondo_rt_group_release", &[I64], &[]),
            Rt::ChannelNew => ("tondo_rt_channel_new", &[I64, I8, I64, I64, I64, I64, I64, I8], &[I64]),
            Rt::SenderNew => ("tondo_rt_sender_new", &[I64], &[I64]),
            Rt::SenderCopy => ("tondo_rt_sender_copy", &[I64], &[I64]),
            Rt::ChannelSend => ("tondo_rt_channel_send", &[I64, I64, I64], &[]),
            Rt::ChannelOffer => ("tondo_rt_channel_offer", &[I64, I64, I8], &[I64]),
            Rt::ChannelReceive => ("tondo_rt_channel_receive", &[I64, I64], &[I8]),
            Rt::ChannelClose => ("tondo_rt_channel_close", &[I64], &[]),
            Rt::ChannelClosed => ("tondo_rt_channel_closed", &[I64, I8], &[I8]),
            Rt::SenderRelease => ("tondo_rt_sender_release", &[I64], &[]),
            Rt::ReceiverRelease => ("tondo_rt_receiver_release", &[I64], &[]),
            Rt::Sleep => ("tondo_rt_sleep", &[I64], &[]),
            Rt::TimeNow => ("tondo_rt_time_now", &[], &[I64]),
            Rt::TimeUnix => ("tondo_rt_time_unix", &[], &[I64]),
            Rt::TimeUtc => ("tondo_rt_time_utc", &[I64, I64], &[]),
            Rt::TimeUtcNow => ("tondo_rt_time_utc_now", &[I64], &[]),
            Rt::SbDuration => ("tondo_rt_sb_duration", &[I64, I64], &[]),
            Rt::SbDateTime => ("tondo_rt_sb_datetime", &[I64, I64], &[]),
            Rt::ReceiveTimeout => ("tondo_rt_channel_receive_timeout", &[I64, I64, I64], &[I8]),
            Rt::Select => ("tondo_rt_select", &[I64, I64, I8, I64], &[I64]),
            Rt::Cancelled => ("tondo_rt_cancelled", &[], &[I8]),
            Rt::PanicUnwind => ("tondo_rt_panic_unwind", &[I32, I64, I64], &[]),
            Rt::PanicValuesUnwind => ("tondo_rt_panic_values_unwind", &[I32, I32, I64, I64, I64], &[]),
            Rt::Unwinding => ("tondo_rt_unwinding", &[], &[I8]),
            Rt::RecoverEnter => ("tondo_rt_recover_enter", &[], &[]),
            Rt::RecoverEnd => ("tondo_rt_recover_end", &[], &[I8]),
            Rt::RecoverLeave => ("tondo_rt_recover_leave", &[], &[]),
            Rt::RecoverCatch => ("tondo_rt_recover_catch", &[], &[I8]),
            Rt::WithinEnter => ("tondo_rt_within_enter", &[I64], &[]),
            Rt::SpawnDetached => ("tondo_rt_spawn_detached", &[], &[]),
            Rt::WithinLeave => ("tondo_rt_within_leave", &[], &[]),
            Rt::WithinCatch => ("tondo_rt_within_catch", &[], &[I8]),
            Rt::UnwindHold => ("tondo_rt_unwind_hold", &[], &[]),
            Rt::UnwindResume => ("tondo_rt_unwind_resume", &[], &[]),
            Rt::UnwindReturn => ("tondo_rt_unwind_return", &[], &[]),
            Rt::DeferEnd => ("tondo_rt_defer_end", &[], &[]),
            Rt::CancelReturn => ("tondo_rt_cancel_return", &[], &[]),
            Rt::UnwindPass => ("tondo_rt_unwind_pass", &[], &[]),
            Rt::ContextRegister => ("tondo_rt_context_register", &[I64, I64, I64, I64, I64, I64, I64], &[]),
            Rt::ContextGet => ("tondo_rt_context_get", &[I64], &[I64]),
            Rt::ContextField => ("tondo_rt_context_field", &[I64, I64, I64], &[I64]),
            Rt::ContextComputed => ("tondo_rt_context_computed", &[I64, I64, I64, I64], &[]),
            Rt::ContextShows => ("tondo_rt_context_shows", &[I64, I64, I64, I64], &[]),
            Rt::ContextSet => ("tondo_rt_context_set", &[I64, I64, I64, I64], &[I64]),
            Rt::ContextRestore => ("tondo_rt_context_restore", &[I64], &[]),
            Rt::JoinWaitTimeout => ("tondo_rt_join_wait_timeout", &[I64, I64, I64], &[I8]),
            Rt::JoinCancel => ("tondo_rt_join_cancel", &[I64], &[]),
            Rt::JoinDetach => ("tondo_rt_join_detach", &[I64], &[]),
            Rt::GroupCancel => ("tondo_rt_group_cancel", &[I64], &[]),
            Rt::SyncNew => ("tondo_rt_sync_new", &[I64, I64, I64, I64], &[I64]),
            Rt::SyncValue => ("tondo_rt_sync_value", &[I64], &[I64]),
            Rt::SyncLock => ("tondo_rt_sync_lock", &[I64, I8, I64], &[I64]),
            Rt::SyncUnlock => ("tondo_rt_sync_unlock", &[I64], &[]),
            Rt::SyncAwait => ("tondo_rt_sync_await", &[I64], &[I64]),
            Rt::SyncRelease => ("tondo_rt_sync_release", &[I64], &[]),
            Rt::SemaphoreNew => ("tondo_rt_semaphore_new", &[I64, I64], &[I64]),
            Rt::SemaphoreAcquire => ("tondo_rt_semaphore_acquire", &[I64, I8], &[I64]),
            Rt::SemaphoreRelease => ("tondo_rt_semaphore_release", &[I64], &[]),
            Rt::PermitRelease => ("tondo_rt_permit_release", &[I64], &[]),
            Rt::BarrierNew => ("tondo_rt_barrier_new", &[I64, I64], &[I64]),
            Rt::BarrierWait => ("tondo_rt_barrier_wait", &[I64], &[I8]),
            Rt::BarrierRelease => ("tondo_rt_barrier_release", &[I64], &[]),
            Rt::SharedMapFrom => ("tondo_rt_shared_map_from", &[I64, I64, I64, I64], &[I64]),
            Rt::SharedMapGet => ("tondo_rt_shared_map_get", &[I64, I64, I64], &[I8]),
            Rt::SharedMapInsert => ("tondo_rt_shared_map_insert", &[I64, I64, I64, I64, I8], &[I8]),
            Rt::SharedMapRemove => ("tondo_rt_shared_map_remove", &[I64, I64, I64], &[I8]),
            Rt::SharedMapExchange => ("tondo_rt_shared_map_exchange", &[I64, I64, I64, I64, I64, I64], &[I8]),
            Rt::SharedMapLength => ("tondo_rt_shared_map_length", &[I64], &[I64]),
            Rt::SharedMapSnapshot => ("tondo_rt_shared_map_snapshot", &[I64], &[I64]),
            Rt::SharedMapRelease => ("tondo_rt_shared_map_release", &[I64], &[]),
            Rt::SharedQueueFrom => ("tondo_rt_shared_queue_from", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::SharedQueuePush => ("tondo_rt_shared_queue_push", &[I64, I64], &[]),
            Rt::SharedQueuePop => ("tondo_rt_shared_queue_pop", &[I64, I64], &[I8]),
            Rt::SharedQueueGet => ("tondo_rt_shared_queue_get", &[I64, I64, I64], &[I8]),
            Rt::SharedQueueLength => ("tondo_rt_shared_queue_length", &[I64], &[I64]),
            Rt::SharedQueueSnapshot => ("tondo_rt_shared_queue_snapshot", &[I64], &[I64]),
            Rt::SharedQueueRelease => ("tondo_rt_shared_queue_release", &[I64], &[]),
            Rt::SharedStackFrom => ("tondo_rt_shared_stack_from", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::SharedStackPush => ("tondo_rt_shared_stack_push", &[I64, I64], &[]),
            Rt::SharedStackPop => ("tondo_rt_shared_stack_pop", &[I64, I64], &[I8]),
            Rt::SharedStackGet => ("tondo_rt_shared_stack_get", &[I64, I64, I64], &[I8]),
            Rt::SharedStackLength => ("tondo_rt_shared_stack_length", &[I64], &[I64]),
            Rt::SharedStackSnapshot => ("tondo_rt_shared_stack_snapshot", &[I64], &[I64]),
            Rt::SharedStackRelease => ("tondo_rt_shared_stack_release", &[I64], &[]),
            Rt::SharedArrayFrom => ("tondo_rt_shared_array_from", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::SharedArrayGet => ("tondo_rt_shared_array_get", &[I64, I64, I64], &[I8]),
            Rt::SharedArraySet => ("tondo_rt_shared_array_set", &[I64, I64, I64, I64], &[I8]),
            Rt::SharedArrayExchange => ("tondo_rt_shared_array_exchange", &[I64, I64, I64, I64, I64, I64], &[I8]),
            Rt::SharedArrayLength => ("tondo_rt_shared_array_length", &[I64], &[I64]),
            Rt::SharedArraySnapshot => ("tondo_rt_shared_array_snapshot", &[I64], &[I64]),
            Rt::SharedArrayRelease => ("tondo_rt_shared_array_release", &[I64], &[]),
            Rt::AtomicLoad => ("tondo_rt_atomic_load", &[I64, I8], &[I64]),
            Rt::AtomicStore => ("tondo_rt_atomic_store", &[I64, I8, I64], &[]),
            Rt::AtomicSwap => ("tondo_rt_atomic_swap", &[I64, I8, I64], &[I64]),
            Rt::AtomicExchange => ("tondo_rt_atomic_exchange", &[I64, I8, I64, I64], &[I64]),
            Rt::AtomicAdd => ("tondo_rt_atomic_add", &[I64, I8, I8, I64, I64], &[I64]),
            Rt::Fs(function) => match function {
                FsFn::ReadText => ("tondo_rt_fs_read_text", &[I64, I64], &[I64]),
                FsFn::ReadBytes => ("tondo_rt_fs_read_bytes", &[I64, I64], &[I64]),
                FsFn::WriteText => ("tondo_rt_fs_write_text", &[I64, I64], &[I64]),
                FsFn::WriteBytes => ("tondo_rt_fs_write_bytes", &[I64, I64], &[I64]),
                FsFn::AppendText => ("tondo_rt_fs_append_text", &[I64, I64], &[I64]),
                FsFn::Exists => ("tondo_rt_fs_exists", &[I64], &[I8]),
                FsFn::List => ("tondo_rt_fs_list", &[I64, I64], &[I64]),
                FsFn::CreateDirectory => ("tondo_rt_fs_create_directory", &[I64], &[I64]),
                FsFn::Rename => ("tondo_rt_fs_rename", &[I64, I64], &[I64]),
                FsFn::Copy => ("tondo_rt_fs_copy", &[I64, I64], &[I64]),
                FsFn::Remove => ("tondo_rt_fs_remove", &[I64, I64], &[I64]),
                FsFn::Open => ("tondo_rt_fs_open", &[I64, I64, I64], &[I64]),
                FsFn::ReadLine => ("tondo_rt_fs_read_line", &[I64, I64], &[I64]),
                FsFn::Read => ("tondo_rt_fs_read", &[I64, I64, I64], &[I64]),
                FsFn::ReadRest => ("tondo_rt_fs_read_rest", &[I64, I64], &[I64]),
                FsFn::Write => ("tondo_rt_fs_write", &[I64, I64], &[I64]),
                FsFn::WriteBytesTo => ("tondo_rt_fs_write_bytes_to", &[I64, I64], &[I64]),
                FsFn::Flush => ("tondo_rt_fs_flush", &[I64], &[I64]),
                FsFn::Close => ("tondo_rt_fs_close", &[I64], &[I64]),
                FsFn::Lock => ("tondo_rt_fs_lock", &[I64, I64], &[I64]),
                FsFn::TryLock => ("tondo_rt_fs_try_lock", &[I64, I64, I64], &[I64]),
            },
            Rt::Io(function) => match function {
                IoFn::Write => ("tondo_rt_io_write", &[I64, I64], &[I64]),
                IoFn::WriteLine => ("tondo_rt_io_write_line", &[I64, I64], &[I64]),
                IoFn::WriteBytes => ("tondo_rt_io_write_bytes", &[I64, I64], &[I64]),
                IoFn::Flush => ("tondo_rt_io_flush", &[I64], &[I64]),
                IoFn::Close => ("tondo_rt_io_close", &[I64], &[I64]),
                IoFn::ReadLine => ("tondo_rt_io_read_line", &[I64, I64], &[I64]),
                IoFn::Read => ("tondo_rt_io_read", &[I64, I64, I64], &[I64]),
                IoFn::ReadText => ("tondo_rt_io_read_text", &[I64, I64], &[I64]),
                IoFn::ReadBytes => ("tondo_rt_io_read_bytes", &[I64, I64], &[I64]),
                IoFn::Reader => ("tondo_rt_io_reader", &[I64], &[I64]),
                IoFn::Discard => ("tondo_rt_io_discard", &[], &[I64]),
                IoFn::BufferNew => ("tondo_rt_io_buffer_new", &[], &[I64]),
                IoFn::BufferWriter => ("tondo_rt_io_buffer_writer", &[I64], &[I64]),
                IoFn::BufferReader => ("tondo_rt_io_buffer_reader", &[I64], &[I64]),
                IoFn::BufferText => ("tondo_rt_io_buffer_text", &[I64, I64], &[I64]),
                IoFn::BufferBytes => ("tondo_rt_io_buffer_bytes", &[I64], &[I64]),
                IoFn::BufferLength => ("tondo_rt_io_buffer_length", &[I64], &[I64]),
                IoFn::BufferClear => ("tondo_rt_io_buffer_clear", &[I64], &[]),
            },
            Rt::Env(function) => match function {
                EnvFn::Args => ("tondo_rt_env_args", &[], &[I64]),
                EnvFn::Get => ("tondo_rt_env_get", &[I64], &[I64]),
                EnvFn::Vars => ("tondo_rt_env_vars", &[I64], &[I64]),
                EnvFn::Set => ("tondo_rt_env_set", &[I64, I64, I64], &[]),
                EnvFn::SetAll => ("tondo_rt_env_set_all", &[I64, I64], &[]),
                EnvFn::Remove => ("tondo_rt_env_remove", &[I64, I64], &[]),
                EnvFn::CurrentDirectory => ("tondo_rt_env_current_directory", &[], &[I64]),
                EnvFn::SetCurrentDirectory => ("tondo_rt_env_set_current_directory", &[I64], &[I64]),
                EnvFn::SyncNative => ("tondo_rt_env_sync_native", &[], &[]),
                EnvFn::Hostname => ("tondo_rt_env_hostname", &[], &[I64]),
                EnvFn::ProcessId => ("tondo_rt_env_process_id", &[], &[I64]),
                EnvFn::Executable => ("tondo_rt_env_executable", &[], &[I64]),
            },
            Rt::Process(function) => match function {
                ProcessFn::Command => ("tondo_rt_process_command", &[I64, I64], &[I64]),
                ProcessFn::Shell => ("tondo_rt_process_shell", &[I64], &[I64]),
                ProcessFn::Exit => ("tondo_rt_process_exit", &[I64], &[]),
                ProcessFn::Pipe => ("tondo_rt_process_pipe", &[I64, I64], &[I64]),
                ProcessFn::Configure(_) => ("tondo_rt_process_configure", &[I64, I64, I64, I64], &[I64]),
                ProcessFn::Run | ProcessFn::Status => ("tondo_rt_process_run", &[I64, I8, I64], &[I64]),
                ProcessFn::Output | ProcessFn::Check => ("tondo_rt_process_output", &[I64, I8, I64, I64, I64], &[I64]),
                ProcessFn::Start => ("tondo_rt_process_start", &[I64, I64], &[I64]),
                ProcessFn::Text => ("tondo_rt_process_text", &[I64, I64], &[I64]),
                ProcessFn::Stream(_) => ("tondo_rt_process_stream", &[I64, I64], &[I64]),
                ProcessFn::Id => ("tondo_rt_process_id", &[I64], &[I64]),
                ProcessFn::Wait | ProcessFn::Cancel => ("tondo_rt_process_wait", &[I64, I8, I64], &[I64]),
                ProcessFn::Collect(_) => ("tondo_rt_process_collect", &[I64, I8, I64, I64, I64], &[I64]),
                ProcessFn::Detach => ("tondo_rt_process_detach", &[I64], &[]),
                ProcessFn::Signals => ("tondo_rt_process_signals", &[I64, I64, I64, I64], &[I64]),
                ProcessFn::Signal => ("tondo_rt_process_signal", &[I64, I64], &[]),
            },
            Rt::Native(_) => unreachable!("declared from `NATIVES`"),
            Rt::ForeignEnter => ("tondo_rt_foreign_enter", &[], &[]),
            Rt::ForeignLeave => ("tondo_rt_foreign_leave", &[], &[]),
            Rt::CString => ("tondo_rt_c_string", &[I64, I64], &[I64]),
            Rt::CStringFree => ("tondo_rt_c_string_free", &[I64], &[]),
            Rt::CVariadic => ("tondo_rt_c_variadic", &[I64, I64], &[]),
            Rt::Callback => ("tondo_rt_callback", &[I64, I64, I64], &[]),
            Rt::Main => ("tondo_rt_main", &[I64], &[I32]),
            Rt::Live => ("tondo_rt_live", &[I64], &[]),
            Rt::LiveTransfer => ("tondo_rt_live_transfer", &[I64, I64, I64], &[I64]),
            Rt::LiveFree => ("tondo_rt_live_free", &[I64], &[]),
            Rt::LiveArgsDone => ("tondo_rt_live_args_done", &[I64], &[]),
            Rt::LiveObject => ("tondo_rt_live_object", &[I64, I64, I8, I64], &[]),
            Rt::JsonReaderRelease => ("tondo_rt_json_reader_release", &[I64], &[]),
            Rt::JsonWriterRelease => ("tondo_rt_json_writer_release", &[I64], &[]),
            Rt::PlanRelease => ("tondo_rt_process_plan_release", &[I64], &[]),
            Rt::ProcessRelease => ("tondo_rt_process_release", &[I64], &[]),
            Rt::SbProcessError => ("tondo_rt_sb_process_error", &[I64, I64, I64], &[]),
            Rt::StreamRelease => ("tondo_rt_stream_release", &[I64], &[]),
            Rt::BufferRelease => ("tondo_rt_io_buffer_release", &[I64], &[]),
            Rt::PathJoin => ("tondo_rt_path_join", &[I64, I64], &[I64]),
            Rt::PathPart => ("tondo_rt_path_part", &[I64, I8], &[I64]),
            Rt::PathIsAbsolute => ("tondo_rt_path_is_absolute", &[I64], &[I8]),
            Rt::StrToBytes => ("tondo_rt_str_to_bytes", &[I64], &[I64]),
            Rt::StrFromBytes => ("tondo_rt_str_from_bytes", &[I64], &[I64]),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Class {
    Zero,
    Scalar(ir::Type),
    Memory,
}

/// Placement of the components of a struct-like value.
#[derive(Clone, Default)]
struct Fields {
    offsets: Vec<u32>,
    boxed: Vec<bool>,
}

#[derive(Clone)]
enum Shape {
    Scalar,
    /// `T?` of an address that is never zero: the address, or zero for
    /// `none`, as C writes a null pointer.
    Niche,
    Struct(Fields),
    /// The tag is at offset 0; each variant's payload starts at `payload`.
    Enum {
        tag: ir::Type,
        payload: u32,
        variants: Vec<Fields>,
    },
}

#[derive(Clone)]
struct Layout {
    size: u32,
    align: u32,
    shape: Shape,
}

/// Where a component lives inside its container.
struct Slot {
    offset: u32,
    ty: Ty,
    boxed: bool,
}

/// A value during code generation.
#[derive(Copy, Clone)]
enum Val {
    Zero,
    Scalar(ir::Value),
    /// The address of an aggregate.
    Memory(ir::Value),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Glue {
    Retain,
    Release,
    Equal,
    /// Appends the `Display` text of a value to a string builder.
    Display,
    /// Hashes a `Key` value.
    Hash,
    /// Runs a member of the test tree, a `fn()` value, and reports the
    /// error it returns.
    Test,
    /// Reports the references a value holds to the cycle collector.
    Visit,
    /// Appends a value as a debugger shows it to a string builder.
    Show,
}

fn int_type(int: IntTy) -> ir::Type {
    match int.bits() {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        64 => types::I64,
        _ => types::I128,
    }
}

/// `value` in LEB128: seven bits a byte, the lowest first.
fn leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn symbol_name(index: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("tondo_f{index}_{clean}")
}

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

/// How an object is built: `meta` counts a step at each call and each turn
/// of a loop, for a provider or generator (27.7); a debug build for a
/// debugger; for another machine; live; or counting what runs, with what
/// each counter counts.
pub struct Options<'o> {
    pub meta: bool,
    pub debug: bool,
    pub target: Option<&'o str>,
    pub live: Option<&'o mut crate::live::Build>,
    pub coverage: Option<&'o mut Vec<Counted>>,
    /// The build `tondo test --fuzz` searches with (testing 10.6): it counts
    /// the dependencies' code too, and tells the runtime what the counted
    /// code compares.
    pub fuzz: bool,
}

/// Compiles the program to an object file.
pub fn compile(program: &Program, bodies: &[Body], sources: &SourceMap, options: Options) -> Result<Vec<u8>, String> {
    let Options { meta, debug, target, mut live, coverage, fuzz } = options;
    let mut flags = settings::builder();
    // A debug build keeps the code as written, statement after statement.
    flags.set("opt_level", if debug { "none" } else { "speed" }).map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    // Integers of 128 bits pass as C passes them, in two registers.
    flags.set("enable_llvm_abi_extensions", "true").map_err(|e| e.to_string())?;
    // Code for the machine, with what its processor has, or for a target.
    let builder = match target {
        Some(triple) => cranelift_codegen::isa::lookup_by_name(triple).map_err(|e| format!("{triple}: {e}"))?,
        None => cranelift_native::builder().map_err(|e| format!("unsupported host: {e}"))?,
    };
    let isa = builder.finish(settings::Flags::new(flags)).map_err(|e| e.to_string())?;
    let shapes = Shapes::new(&program.types, &*isa);
    let builder =
        ObjectBuilder::new(isa, "tondo", cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    let mut cg = Codegen {
        module: ObjectModule::new(builder),
        types: &program.types,
        sources,
        meta,
        functions: Vec::new(),
        runtime: HashMap::new(),
        strings: HashMap::new(),
        arrays: 0,
        byte_arrays: HashMap::new(),
        shapes,
        glue: HashMap::new(),
        pending_glue: Vec::new(),
        map_glue: HashMap::new(),
        identities: HashMap::new(),
        loans: bodies.iter().map(|body| body.loans.clone()).collect(),
        display: &program.display,
        fn_values: HashMap::new(),
        pending_thunks: Vec::new(),
        externs: &program.externs,
        c_functions: HashMap::new(),
        callbacks: HashMap::new(),
        pending_callbacks: Vec::new(),
        library: None,
        traced: Vec::new(),
        debug: None,
        debug_functions: Vec::new(),
        debug_sites: Vec::new(),
        live: None,
        current: 0,
        takes_osr: bodies.iter().map(|body| body.osr.is_some()).collect(),
        forwarders: HashMap::new(),
        pending_forwarders: Vec::new(),
        coverage: None,
        fuzz,
        contexts: program
            .contexts
            .iter()
            .map(|fns| (fns.clone(), program.types.handle_result(bodies[fns.maker.0 as usize].result)))
            .collect(),
    };
    if coverage.is_some() {
        let counters =
            cg.module.declare_data("tondo_coverage", Linkage::Local, true, false).map_err(|e| e.to_string())?;
        cg.coverage = Some((counters, Vec::new()));
    }

    if debug {
        let table = cg.module.declare_data("tondo_debug", Linkage::Local, false, false).map_err(|e| e.to_string())?;
        cg.debug = Some(table);
    }
    for (index, body) in bodies.iter().enumerate() {
        let signature = cg.signature(body);
        let id = cg
            .module
            .declare_function(&symbol_name(index, &body.name), Linkage::Local, &signature)
            .map_err(|e| e.to_string())?;
        cg.functions.push(id);
    }
    if let Some(build) = live.as_deref_mut() {
        cg.live = Some(LiveCodegen::new(&mut cg.module, build, bodies, &program.types)?);
    }
    for (index, export) in program.exports.iter().enumerate() {
        cg.export(index, export)?;
    }
    let mut context = cg.module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    // The IR of every function is built on every core, and what each asks
    // to declare is declared in the functions' order (see `worker`);
    // `compile_all` compiles them on every core.
    let mut jobs = Vec::new();
    let mut job = |id: FuncId, context: &mut cranelift_codegen::Context, traced: Option<&str>, sites, inlined| {
        let func = std::mem::replace(&mut context.func, ir::Function::new());
        jobs.push(Job { id, func, traced: traced.map(str::to_string), sites, inlined });
    };
    let built = {
        let shared = worker::Shared::new(&cg);
        let indices: Vec<usize> = (0..bodies.len()).collect();
        crate::parallel::map(&indices, 1, |&index| worker::build(&shared, bodies, index))
    };
    for (index, (body, mut built)) in bodies.iter().zip(built).enumerate() {
        cg.current = index;
        // A test's trace ends at the test, without the runner's tree that
        // calls it.
        let runner = !program.tests.is_empty() && program.main.is_some_and(|main| index == main.0 as usize);
        let traced = (!runner && !body.name.starts_with("suite ")).then_some(body.name.as_str());
        if built.serial {
            context.func.signature = cg.signature(body);
            context.func.name = UserFuncName::user(0, index as u32);
            let sites = FnCodegen::new(&mut cg, body, bodies, &mut context.func, &mut builder_context).run();
            job(cg.functions[index], &mut context, traced, sites, body.inlined.clone());
        } else {
            worker::replay(&mut cg, bodies, &mut built);
            context.func = built.func;
            job(cg.functions[index], &mut context, traced, built.sites, body.inlined.clone());
        }
        cg.module.clear_context(&mut context);
    }
    let trace = cg.module.declare_data("tondo_trace", Linkage::Local, false, false).map_err(|e| e.to_string())?;
    let patch = cg.live.as_ref().is_some_and(|live| live.patch.is_some());
    match program.main {
        // A patch starts nothing: the running program calls into it.
        _ if patch => {}
        Some(main) => {
            let main = main.0 as usize;
            let entry =
                cg.entry_point(main, bodies[main].result, &program.tests, trace, &mut context, &mut builder_context)?;
            job(entry, &mut context, None, Vec::new(), Vec::new());
            cg.module.clear_context(&mut context);
            let start = cg.process_main(entry, &mut context, &mut builder_context)?;
            job(start, &mut context, None, Vec::new(), Vec::new());
        }
        None => {
            let init = cg.library_start(trace, &mut context, &mut builder_context)?;
            job(init, &mut context, None, Vec::new(), Vec::new());
            cg.library = Some(init);
        }
    }
    cg.module.clear_context(&mut context);
    while let Some((target, thunk)) = cg.pending_thunks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(thunk).signature.clone();
        context.func.name = UserFuncName::user(3, thunk.as_u32());
        define_thunk(&mut cg, &bodies[target.0 as usize], target, &mut context.func, &mut builder_context);
        // In the table of the trace, without lines, for its handler.
        job(thunk, &mut context, Some(""), Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
    }
    while let Some((target, forward)) = cg.pending_forwarders.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(forward).signature.clone();
        context.func.name = UserFuncName::user(6, forward.as_u32());
        define_forwarder(&mut cg, target, &mut context.func, &mut builder_context);
        job(forward, &mut context, Some(""), Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
    }
    while let Some(callback) = cg.pending_callbacks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(callback.adapter).signature.clone();
        context.func.name = UserFuncName::user(4, callback.adapter.as_u32());
        define_adapter(&mut cg, &callback, &mut context.func, &mut builder_context);
        job(callback.adapter, &mut context, None, Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
        context.func.signature = cg.module.declarations().get_function_decl(callback.inner).signature.clone();
        context.func.name = UserFuncName::user(5, callback.inner.as_u32());
        define_inner(&mut cg, &bodies[callback.target.0 as usize], &callback, &mut context.func, &mut builder_context);
        job(callback.inner, &mut context, Some(&bodies[callback.target.0 as usize].name), Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
    }
    // A patch's conversions of the running program's values, which the
    // runtime calls as frames move, with the glue that releases what they
    // convert from and to.
    for function in live.as_ref().map(|build| build.migrations.clone()).unwrap_or_default() {
        let body = &bodies[function.0 as usize];
        let mut signature = cg.module.make_signature();
        signature.params.extend([AbiParam::new(POINTER), AbiParam::new(POINTER)]);
        signature.returns.push(AbiParam::new(types::I8));
        let name = format!("tondo_convert{}", function.0);
        let id = cg.module.declare_function(&name, Linkage::Local, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(7, id.as_u32());
        define_conversion(&mut cg, body, function, &mut context.func, &mut builder_context);
        job(id, &mut context, None, Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
        let TyKind::Option(new) = *cg.types.kind(body.result) else { unreachable!("a conversion gives `T?`") };
        let old = body.locals[body.params[0].0 as usize];
        let mut release = |ty: Ty| cg.types.needs_drop(ty).then(|| cg.glue(Glue::Release, ty));
        let entry = (id, release(old), release(new));
        let index = cg.live.as_ref().expect("a live build").conversions.len();
        cg.live.as_mut().expect("a live build").conversions.push(entry);
        if live.as_ref().is_some_and(|build| build.objects.contains(&index)) {
            let (from, to) = (cg.identity(old), cg.identity(new));
            let layout = cg.layout(new);
            let visit = cg.types.can_cycle(new).then(|| cg.glue(Glue::Visit, new));
            let object = (index, from, to, layout.size, layout.align, entry.2, visit);
            cg.live.as_mut().expect("a live build").objects.push(object);
        }
    }
    while let Some((kind, ty, id)) = cg.pending_glue.pop() {
        context.func.signature = cg.glue_signature(kind);
        context.func.name = UserFuncName::user(2, id.as_u32());
        define_glue(&mut cg, kind, ty, &mut context.func, &mut builder_context);
        job(id, &mut context, None, Vec::new(), Vec::new());
        cg.module.clear_context(&mut context);
    }
    cg.compile_all(jobs)?;

    if let (Some(out), Some((counters, counted))) = (coverage, cg.coverage.take()) {
        let mut data = cranelift_module::DataDescription::new();
        data.define_zeroinit(counted.len().max(1) * 8);
        cg.module.define_data(counters, &data).map_err(|e| e.to_string())?;
        *out = counted;
    }
    cg.trace_table(trace);
    if let Some(debug) = cg.debug {
        cg.debug_table(debug);
    }
    if let Some(build) = live {
        cg.live_points(build, bodies);
        cg.live_tables(build, trace)?;
    }
    let product = cg.module.finish();
    product.emit().map_err(|e| e.to_string())
}

struct Codegen<'a> {
    module: ObjectModule,
    types: &'a Types,
    /// Each context's functions and the type of its record (11.16).
    contexts: Vec<(crate::hir::ContextFns, Ty)>,
    sources: &'a SourceMap,
    /// Counts steps, for the `meta` profile.
    meta: bool,
    functions: Vec<FuncId>,
    runtime: HashMap<Rt, FuncId>,
    strings: HashMap<String, DataId>,
    /// How many static arrays there are, which names the next.
    arrays: usize,
    /// The static arrays of embedded files, by their bytes.
    byte_arrays: HashMap<std::sync::Arc<[u8]>, DataId>,
    shapes: Shapes<'a>,
    glue: HashMap<(Glue, Ty), FuncId>,
    pending_glue: Vec<(Glue, Ty, FuncId)>,
    map_glue: HashMap<Ty, DataId>,
    /// The identity of each type a `reflect.Value` holds: a static with its
    /// name.
    identities: HashMap<Ty, DataId>,
    /// Which parameters of each function are loans (`Body::loans`).
    loans: Vec<Vec<bool>>,
    /// The user `Display.display` of types displayed inside collections.
    display: &'a HashMap<Ty, FnId>,
    /// The static `fn(...)` object of each named function used as a value,
    /// and its thunk with the uniform calling convention.
    fn_values: HashMap<FnId, (DataId, FuncId)>,
    pending_thunks: Vec<(FnId, FuncId)>,
    /// The `extern fn` declarations, and the C functions imported so far by
    /// symbol, with the signature of their import.
    externs: &'a [crate::hir::Extern],
    c_functions: HashMap<String, (FuncId, ir::Signature)>,
    /// The C function that runs a function or closure body for C, by the
    /// `extern fn(...)` type it has, and those still to define with the
    /// function that makes the call from a frame.
    callbacks: HashMap<(FnId, Ty), FuncId>,
    pending_callbacks: Vec<Callback>,
    /// In a library, what starts it: the first call from C runs it.
    library: Option<FuncId>,
    /// The functions of the stack trace table, with their calls.
    traced: Vec<Traced>,
    /// A debug build's probes (toolchain 7.6): each function's name and
    /// the bindings it shows, and each probe's source and function.
    debug: Option<DataId>,
    debug_functions: Vec<(String, Vec<DebugBinding>)>,
    debug_sites: Vec<(u32, u32)>,
    live: Option<LiveCodegen>,
    /// The body whose IR is being built.
    current: usize,
    /// Which bodies take a live build's hidden parameter.
    takes_osr: Vec<bool>,
    /// In a live build, what closures call their bodies through.
    forwarders: HashMap<FnId, FuncId>,
    pending_forwarders: Vec<(FnId, FuncId)>,
    /// A test build that counts what runs (testing 10.3): its counters,
    /// and what each counts.
    coverage: Option<(DataId, Vec<Counted>)>,
    /// A build for the search of `--fuzz`.
    fuzz: bool,
}

/// What a counter of a test build with coverage counts (testing 10.3):
/// the runs of a line of the project's code, the calls of a function, or
/// the times a branch went one way, with where its branch is and what the
/// way is called.
#[derive(Clone, Debug)]
pub enum Counted {
    Line { file: String, line: usize },
    Function { file: String, line: usize, name: String },
    Way { file: String, line: usize, column: usize, branch: (usize, usize), label: String },
}

/// A live build's slots (toolchain 7.10): the table every call of the
/// program's functions goes through, the slot of each function, and its
/// identity; and what the image registers with the runtime.
struct LiveCodegen {
    slots: DataId,
    info: DataId,
    slot_of: Vec<u32>,
    identities: Vec<String>,
    /// Each compiled function's code, hashed, by its function.
    code: HashMap<FuncId, u64>,
    /// For a patch: the types whose identity the running program holds,
    /// and the number its debugger's sites start from; and the code the
    /// program runs, hashed, by identity, which a patch leaves where it is
    /// unless it changes.
    patch: Option<(Vec<String>, u32)>,
    known: HashMap<String, u64>,
    /// A patch's conversions, and the release glue of what each converts
    /// from and to; and of those that convert what values with identity
    /// hold, the identity of the old type, and the new one's size and
    /// alignment, release and visit glue.
    conversions: Vec<(FuncId, Option<FuncId>, Option<FuncId>)>,
    objects: Vec<LiveObject>,
    /// The word of each function's first site, the function, and the
    /// site's number in the build.
    sites: Vec<(DataId, usize, usize)>,
    /// The site of each point that has a probe, by function and point.
    point_sites: HashMap<(usize, u32), u32>,
    /// What each point's state is in its function, as `point_layout` lays
    /// it out, by function and point.
    layouts: HashMap<(usize, u32), StateLayout>,
}

impl LiveCodegen {
    fn new(
        module: &mut ObjectModule,
        build: &mut crate::live::Build,
        bodies: &[Body],
        types: &Types,
    ) -> Result<LiveCodegen, String> {
        if build.capacity == 0 {
            build.capacity = crate::live::capacity(bodies.len());
        }
        let mut next = build.slots.values().max().map_or(0, |&slot| slot + 1);
        let mut seen: HashMap<String, u32> = HashMap::new();
        let (mut slot_of, mut identities) = (Vec::new(), Vec::new());
        // A closure is its function's n-th of so many: an edit that adds
        // or removes one makes them all others.
        let mut closures: HashMap<&str, (u32, u32)> = HashMap::new();
        for body in bodies.iter().filter(|b| b.name.starts_with("closure in ")) {
            closures.entry(&body.name).or_default().1 += 1;
        }
        for body in bodies {
            let params: Vec<String> = body
                .params
                .iter()
                .zip(&body.loans)
                .map(|(param, &loan)| {
                    let ty = types.display(body.locals[param.0 as usize]);
                    if loan { format!("mut {ty}") } else { ty }
                })
                .collect();
            let name = match closures.get_mut(body.name.as_str()) {
                Some((ordinal, total)) => {
                    *ordinal += 1;
                    format!("{} {ordinal}/{total}", body.name)
                }
                None => body.name.clone(),
            };
            let mut signature: Vec<Ty> = body.params.iter().map(|p| body.locals[p.0 as usize]).collect();
            signature.push(body.result);
            let digest = crate::live::shape_digest(types, &signature);
            let base = format!("{name}({}): {}", params.join(", "), types.display(body.result));
            let base = if digest.is_empty() { base } else { format!("{base} @{digest}") };
            build.holds.entry(body.name.clone()).or_default().extend(crate::live::held(types, &body.locals, false));
            // Closures share a name: the first of a function keeps it.
            let count = seen.entry(base.clone()).or_insert(0);
            let identity = if *count == 0 { base } else { format!("{base} #{count}") };
            *count += 1;
            let slot = *build.slots.entry(identity.clone()).or_insert_with(|| {
                next += 1;
                next - 1
            });
            if slot >= build.capacity {
                return Err("this live build has no room for more functions: start it again".into());
            }
            slot_of.push(slot);
            identities.push(identity);
        }
        // A patch uses the running program's slots, and says how to set them.
        let (linkage, info) = match build.patch.is_some() {
            true => (Linkage::Import, "tondo_patch"),
            false => (Linkage::Local, "tondo_live"),
        };
        let slots = module.declare_data("tondo_live_slots", linkage, true, false).map_err(|e| e.to_string())?;
        let info = module.declare_data(info, Linkage::Local, false, false).map_err(|e| e.to_string())?;
        let patch = build.patch.as_ref().map(|base| (base.data.clone(), base.sites));
        let known = build.patch.as_ref().map(|base| base.code.clone()).unwrap_or_default();
        Ok(LiveCodegen {
            slots,
            info,
            slot_of,
            identities,
            code: HashMap::new(),
            patch,
            known,
            conversions: Vec::new(),
            objects: Vec::new(),
            sites: Vec::new(),
            point_sites: HashMap::new(),
            layouts: HashMap::new(),
        })
    }
}

/// What a live build's code hashes see of the names it refers to.
struct Labels {
    functions: HashMap<FuncId, String>,
    data: HashMap<DataId, String>,
}

/// A hash of a function's code and of what it refers to.
fn code_hash(code: &Compiled, labels: &Labels) -> u64 {
    use cranelift_module::ModuleRelocTarget;
    let mut parts: Vec<Vec<u8>> = vec![code.bytes.clone()];
    for reloc in &code.relocs {
        let target = match &reloc.name {
            ModuleRelocTarget::User { namespace: 0, index } => {
                labels.functions.get(&FuncId::from_u32(*index)).cloned().unwrap_or_default()
            }
            ModuleRelocTarget::User { index, .. } => {
                labels.data.get(&DataId::from_u32(*index)).cloned().unwrap_or_default()
            }
            other => format!("{other:?}"),
        };
        parts.push(format!("{} {:?} {} {target}", reloc.offset, reloc.kind, reloc.addend).into_bytes());
    }
    let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    u64::from_str_radix(&crate::driver::fingerprint(&parts), 16).unwrap_or(0)
}

/// Calls one of the program's functions: directly, or in a live build
/// through its slot, which a patch may point at new code.
fn call_body<'t, G: Gen<'t> + ?Sized>(cg: &mut G, fb: &mut FunctionBuilder, id: FnId, args: &[ir::Value]) -> ir::Inst {
    call_body_with(cg, fb, id, args, None)
}

/// A call with the state a frame brings to the new code, or none.
fn call_body_with<'t, G: Gen<'t> + ?Sized>(
    cg: &mut G,
    fb: &mut FunctionBuilder,
    id: FnId,
    args: &[ir::Value],
    state: Option<ir::Value>,
) -> ir::Inst {
    let mut args = args.to_vec();
    if cg.takes_osr(id) {
        args.push(state.unwrap_or_else(|| fb.ins().iconst(types::I64, 0)));
    }
    let args = &args[..];
    let func = cg.function(id);
    let Some((table, slot)) = cg.slot_of(id) else {
        let callee = cg.declare_func_in_func(func, fb.func);
        return fb.ins().call(callee, args);
    };
    // A frame that moves enters the code its state is for, which a patch
    // that comes meanwhile does not change.
    let code = match state {
        Some(state) => fb.ins().load(POINTER, flags(), state, 8),
        None => {
            let table = cg.declare_data_in_func(table, fb.func);
            let base = fb.ins().symbol_value(POINTER, table);
            fb.ins().load(POINTER, flags(), base, (slot * 8) as i32)
        }
    };
    let signature = cg.function_signature(func);
    let signature = fb.import_signature(signature);
    fb.ins().call_indirect(signature, code, args)
}

/// A binding a debug build's probe shows: its name, its type and the glue
/// that shows it.
type DebugBinding = (String, Ty, FuncId);

/// A compiled function for stack traces: its name, size and, for each
/// call, the offset it returns to and where its source starts; and where
/// the code of each statement starts, for profiles.
struct Traced {
    id: FuncId,
    name: String,
    size: u32,
    calls: Vec<(u32, u32)>,
    handlers: Vec<(u32, u32)>,
    lines: Vec<(u32, u32)>,
    sites: Vec<Site>,
    inlined: Vec<crate::mir::Inlined>,
}

/// Where a statement's code is in the source, the type of the local it
/// assigns, and the inlined call it comes from (`Body::inlined`).
type Site = (u32, Option<Ty>, Option<u32>);

/// A function whose IR is built, to compile; `traced` names it in stack
/// traces.
struct Job {
    id: FuncId,
    func: ir::Function,
    traced: Option<String>,
    /// Where each statement's code is in the source, and the type of the
    /// local it assigns, so that a sample of the memory it allocates names
    /// its type: the source location of an instruction indexes this, from 1.
    sites: Vec<Site>,
    inlined: Vec<crate::mir::Inlined>,
}

/// A function's machine code, its relocations, and its size and calls for
/// stack traces.
struct Compiled {
    alignment: u64,
    bytes: Vec<u8>,
    relocs: Vec<cranelift_module::ModuleReloc>,
    size: u32,
    calls: Vec<(u32, u32)>,
    /// Each `try_call`'s return address and handler.
    handlers: Vec<(u32, u32)>,
    /// Where each run of code of one statement starts, and the statement's
    /// site, `0` for code of none.
    lines: Vec<(u32, u32)>,
}

/// Compiles a function's IR to machine code.
fn compile_one(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    context: &mut cranelift_codegen::Context,
    id: FuncId,
    function: ir::Function,
) -> Result<Compiled, String> {
    context.clear();
    context.func = function;
    context.compile(isa, &mut Default::default()).map_err(|e| format!("{:?}", e.inner))?;
    let code = context.compiled_code().expect("compiled code");
    let sources = code.buffer.get_srclocs_sorted();
    let calls = code
        .buffer
        .call_sites()
        .filter_map(|call| {
            let at = call.ret_addr - 1;
            let source = sources.get(sources.partition_point(|s| s.end <= at)).filter(|s| s.start <= at)?;
            let loc = source.loc.as_abs();
            (!loc.is_default()).then(|| (call.ret_addr, loc.bits()))
        })
        .collect();
    let handlers = code
        .buffer
        .call_sites()
        .filter_map(|call| match call.exception_handlers {
            [cranelift_codegen::MachExceptionHandler::Default(at)] if at.is_offset() => {
                Some((call.ret_addr, at.as_offset()))
            }
            _ => None,
        })
        .collect();
    let relocs = code
        .buffer
        .relocs()
        .iter()
        .map(|reloc| cranelift_module::ModuleReloc::from_mach_reloc(reloc, &context.func, id))
        .collect();
    Ok(Compiled {
        alignment: code.buffer.min_alignment as u64,
        bytes: code.buffer.data().to_vec(),
        relocs,
        size: code.code_info().total_size,
        calls,
        handlers,
        lines: sources
            .iter()
            .map(|s| (s.start, if s.loc.as_abs().is_default() { 0 } else { s.loc.as_abs().bits() }))
            .collect(),
    })
}

/// What the code generator knows of the program's types without declaring
/// anything: their classes and layouts, and the signatures that pass them,
/// which any core can work out.
struct Shapes<'t> {
    types: &'t Types,
    layouts: HashMap<Ty, Layout>,
    call_conv: cranelift_codegen::isa::CallConv,
    abi: cabi::Abi,
}

impl<'t> Shapes<'t> {
    fn new(types: &'t Types, isa: &dyn cranelift_codegen::isa::TargetIsa) -> Shapes<'t> {
        let abi = match (isa.name(), isa.default_call_conv()) {
            ("aarch64", call_conv) => {
                cabi::Abi::Arm64 { apple: call_conv == cranelift_codegen::isa::CallConv::AppleAarch64 }
            }
            _ => cabi::Abi::X64,
        };
        Shapes { types, layouts: HashMap::new(), call_conv: isa.default_call_conv(), abi }
    }

    fn make_signature(&self) -> ir::Signature {
        ir::Signature::new(self.call_conv)
    }

    fn class(&mut self, ty: Ty) -> Class {
        let repr = self.types.repr(ty);
        match self.types.kind(repr) {
            TyKind::Unit | TyKind::Never | TyKind::Error => Class::Zero,
            TyKind::Param(_) | TyKind::Opaque(..) => {
                unreachable!("monomorphization leaves only concrete types")
            }
            TyKind::Bool => Class::Scalar(types::I8),
            TyKind::Int(int) => Class::Scalar(int_type(*int)),
            TyKind::Float(FloatTy::F32) => Class::Scalar(types::F32),
            TyKind::Float(FloatTy::F64) => Class::Scalar(types::F64),
            TyKind::Char => Class::Scalar(types::I32),
            TyKind::Byte => Class::Scalar(types::I8),
            TyKind::String
            | TyKind::Array(_)
            | TyKind::Deque(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Fn(..)
            | TyKind::CFn(..)
            | TyKind::Handle(..)
            | TyKind::Pointer(_) => Class::Scalar(POINTER),
            TyKind::CArray(..) => Class::Memory,
            TyKind::Tuple(_) | TyKind::Adt(_) | TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) => {
                let layout = self.layout(repr);
                match layout.shape {
                    Shape::Niche => Class::Scalar(POINTER),
                    Shape::Enum { tag, .. } if layout.size == tag.bytes() => Class::Scalar(tag),
                    _ if layout.size == 0 => Class::Zero,
                    _ => Class::Memory,
                }
            }
        }
    }

    fn fields(&mut self, container: Ty, components: &[Ty]) -> (Fields, u32, u32) {
        let (mut size, mut align, mut fields) = (0u32, 1u32, Fields::default());
        for &component in components {
            let boxed = self.types.is_boxed(container, component);
            let (component_size, component_align) = if boxed {
                (8, 8)
            } else {
                let layout = self.layout(component);
                (layout.size, layout.align)
            };
            let offset = size.next_multiple_of(component_align);
            fields.offsets.push(offset);
            fields.boxed.push(boxed);
            size = offset + component_size;
            align = align.max(component_align);
        }
        (fields, size, align)
    }

    fn layout(&mut self, ty: Ty) -> Layout {
        let ty = self.types.repr(ty);
        if let Some(layout) = self.layouts.get(&ty) {
            return layout.clone();
        }
        let layout = if let TyKind::Option(inner) = *self.types.kind(ty)
            && self.types.is_address(inner)
        {
            Layout { size: 8, align: 8, shape: Shape::Niche }
        } else if let TyKind::CArray(element, count) = *self.types.kind(ty) {
            let element = self.layout(element);
            Layout { size: element.size * count, align: element.align, shape: Shape::Scalar }
        } else if let Some(c) = self.types.c_layout(ty) {
            // An `extern type`: C's layout, without padding when packed.
            let (mut size, mut align, mut fields) = (0u32, 1u32, Fields::default());
            for component in self.types.components(ty).expect("a record") {
                let layout = self.layout(component);
                // C aligns an `__int128` to 16 bytes.
                let natural = if self.class(component) == Class::Scalar(types::I128) { 16 } else { layout.align };
                let component_align = if c.packed { 1 } else { natural };
                // A union's fields share its memory.
                let offset = if c.union { 0 } else { size.next_multiple_of(component_align) };
                fields.offsets.push(offset);
                fields.boxed.push(false);
                size = size.max(offset + layout.size);
                align = align.max(component_align);
            }
            let align = align.max(c.align);
            Layout { size: size.next_multiple_of(align), align, shape: Shape::Struct(fields) }
        } else if let Some(components) = self.types.components(ty) {
            let (fields, size, align) = self.fields(ty, &components);
            Layout { size: size.next_multiple_of(align), align, shape: Shape::Struct(fields) }
        } else if let Some(variants) = self.types.variants(ty) {
            let tag = self.scalar_type(self.types.tag_ty(ty));
            let (mut payload_size, mut payload_align, mut placed) = (0, 1, Vec::new());
            for components in &variants {
                let (fields, size, align) = self.fields(ty, components);
                payload_size = payload_size.max(size);
                payload_align = payload_align.max(align);
                placed.push(fields);
            }
            let payload = tag.bytes().next_multiple_of(payload_align);
            let align = payload_align.max(tag.bytes());
            let size = if payload_size == 0 { tag.bytes() } else { (payload + payload_size).next_multiple_of(align) };
            Layout { size, align, shape: Shape::Enum { tag, payload, variants: placed } }
        } else {
            let size = match self.class(ty) {
                Class::Scalar(t) => t.bytes(),
                _ => 0,
            };
            // An integer of 128 bits is two words, aligned as one.
            Layout { size, align: size.clamp(1, 8), shape: Shape::Scalar }
        };
        self.layouts.insert(ty, layout.clone());
        layout
    }

    fn scalar_type(&mut self, ty: Ty) -> ir::Type {
        match self.class(ty) {
            Class::Scalar(t) => t,
            other => unreachable!("not a scalar: {other:?}"),
        }
    }

    /// The component that `proj` selects inside a value of type `ty`.
    fn slot(&mut self, ty: Ty, proj: Proj) -> Slot {
        let component = project(self.types, ty, proj);
        match (self.layout(ty).shape, proj) {
            (Shape::Struct(fields), Proj::Field(index)) => {
                Slot { offset: fields.offsets[index as usize], ty: component, boxed: fields.boxed[index as usize] }
            }
            (Shape::Niche, Proj::Variant(SOME, 0)) => Slot { offset: 0, ty: component, boxed: false },
            (Shape::Enum { payload, variants, .. }, Proj::Variant(variant, index)) => {
                let fields = &variants[variant as usize];
                Slot {
                    offset: payload + fields.offsets[index as usize],
                    ty: component,
                    boxed: fields.boxed[index as usize],
                }
            }
            _ => unreachable!("projection does not match the layout"),
        }
    }

    fn signature(&mut self, body: &Body) -> ir::Signature {
        let mut signature = self.make_signature();
        if self.class(body.result) == Class::Memory {
            signature.params.push(AbiParam::new(POINTER));
        }
        for (index, &param) in body.params.iter().enumerate() {
            match self.class(body.locals[param.0 as usize]) {
                // A loan is the address of the caller's place.
                _ if body.loans[index] => signature.params.push(AbiParam::new(POINTER)),
                Class::Zero => {}
                Class::Scalar(ty) => signature.params.push(AbiParam::new(ty)),
                Class::Memory => signature.params.push(AbiParam::new(POINTER)),
            }
        }
        // A live build's hidden parameter: the state a frame brings from
        // the old code.
        if body.osr.is_some() {
            signature.params.push(AbiParam::new(types::I64));
        }
        if let Class::Scalar(ty) = self.class(body.result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    /// The calling convention of a `fn(...)` value: the result slot when
    /// the result lives in memory, the captures' address, then the
    /// parameters. Closure bodies and thunks follow it.
    fn uniform_signature(&mut self, params: &[Ty], loans: &[bool], result: Ty) -> ir::Signature {
        let mut signature = self.make_signature();
        if self.class(result) == Class::Memory {
            signature.params.push(AbiParam::new(POINTER));
        }
        signature.params.push(AbiParam::new(POINTER));
        for (&param, &loan) in params.iter().zip(loans) {
            match self.class(param) {
                _ if loan => signature.params.push(AbiParam::new(POINTER)),
                Class::Zero => {}
                Class::Scalar(ty) => signature.params.push(AbiParam::new(ty)),
                Class::Memory => signature.params.push(AbiParam::new(POINTER)),
            }
        }
        if let Class::Scalar(ty) = self.class(result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        let mut signature = self.make_signature();
        signature.params.push(AbiParam::new(POINTER));
        if matches!(kind, Glue::Display | Glue::Visit | Glue::Show) {
            signature.params.push(AbiParam::new(POINTER));
        }
        if kind == Glue::Equal {
            signature.params.push(AbiParam::new(POINTER));
            signature.returns.push(AbiParam::new(types::I8));
        }
        if kind == Glue::Hash {
            signature.returns.push(AbiParam::new(types::I64));
        }
        signature
    }

    /// The element type of an array or deque, and whether it is a deque.
    fn sequence_parts(&self, ty: Ty) -> (Ty, bool) {
        match *self.types.kind(self.types.repr(ty)) {
            TyKind::Array(element) => (element, false),
            TyKind::Deque(element) => (element, true),
            _ => unreachable!("not an array or deque"),
        }
    }

    /// The key and value of a map or set: `(K, V)`, with `Unit` for sets.
    fn map_parts(&self, ty: Ty) -> (Ty, Ty) {
        match *self.types.kind(self.types.repr(ty)) {
            TyKind::Map(key, value) => (key, value),
            TyKind::Set(element) => (element, Ty::UNIT),
            _ => unreachable!("not a map or set"),
        }
    }

    /// Entry size and value offset, laid out like the tuple `(K, V)`.
    fn entry_layout(&mut self, ty: Ty) -> (u32, u32) {
        let (key, value) = self.map_parts(ty);
        let (key, value) = (self.layout(key), self.layout(value));
        let offset = key.size.next_multiple_of(value.align);
        ((offset + value.size).next_multiple_of(key.align.max(value.align)), offset)
    }
}

/// The symbol of a function of the runtime, and its signature, which
/// `signature` starts as.
fn runtime_signature(rt: Rt, mut signature: ir::Signature) -> (&'static str, ir::Signature) {
    let native;
    let (name, params, returns) = match rt {
        Rt::Native(index) => {
            let function = &crate::hir::NATIVES[index as usize];
            let clif = |ty: crate::hir::NativeTy| match ty {
                crate::hir::NativeTy::Float => types::F64,
                crate::hir::NativeTy::Bool => types::I8,
                crate::hir::NativeTy::Char => types::I32,
                _ => types::I64,
            };
            let params: Vec<ir::Type> = function.receiver.iter().chain(function.params).map(|&t| clif(t)).collect();
            let returns: Vec<ir::Type> =
                (function.result != crate::hir::NativeTy::Unit).then(|| clif(function.result)).into_iter().collect();
            native = (params, returns);
            (function.symbol, native.0.as_slice(), native.1.as_slice())
        }
        _ => rt.signature(),
    };
    // The C ABI as LLVM implements it expects narrow integer arguments
    // extended to 32 bits by the caller.
    signature.params.extend(
        params.iter().map(|&t| if t.is_int() && t.bits() < 32 { AbiParam::new(t).uext() } else { AbiParam::new(t) }),
    );
    signature.returns.extend(returns.iter().map(|&t| AbiParam::new(t)));
    (name, signature)
}

/// What building a function's IR asks of the code generator: what the
/// program is, the shapes of its types, the functions and data the code
/// refers to, and what debug and live builds record of it. `Codegen`
/// declares those as asked; a `Worker`, which builds a function on another
/// core, names them so that `replay` declares them in the same order.
trait Gen<'t> {
    fn shapes(&mut self) -> &mut Shapes<'t>;
    fn types(&self) -> &'t Types;
    fn sources(&self) -> &'t SourceMap;
    fn display(&self) -> &'t HashMap<Ty, FnId>;
    fn externs(&self) -> &'t [crate::hir::Extern];
    fn meta(&self) -> bool;
    fn isa(&self) -> &dyn cranelift_codegen::isa::TargetIsa;
    /// Which parameters of a function are loans.
    fn loans(&self, id: FnId) -> Vec<bool>;
    /// A function of the program, and whether it takes a live build's
    /// hidden parameter.
    fn function(&self, id: FnId) -> FuncId;
    fn takes_osr(&self, id: FnId) -> bool;
    fn function_signature(&self, id: FuncId) -> ir::Signature;
    /// In a live build, the table of slots and a function's slot.
    fn live(&self) -> bool;
    fn slot_of(&self, id: FnId) -> Option<(DataId, u32)>;
    /// The number a patch's debugger sites start from.
    fn site_base(&self) -> u32;
    fn debug(&self) -> bool;
    /// In a test build with coverage, its counters.
    fn coverage(&self) -> Option<DataId>;
    /// A build for the search of `--fuzz`.
    fn fuzzing(&self) -> bool;
    /// A counter of what `counted` says: its number.
    fn counter(&mut self, counted: Counted) -> usize;
    /// Where the counter `index` is among the counters, as an
    /// instruction's constant.
    fn counter_offset(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value;

    fn string(&mut self, text: &str) -> DataId;
    /// A static array of constants, each `size` bytes.
    fn static_array(&mut self, size: usize, elements: Vec<Const>) -> DataId;
    fn bytes(&mut self, bytes: &std::sync::Arc<[u8]>) -> DataId;
    fn identity(&mut self, ty: Ty) -> DataId;
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId;
    fn runtime(&mut self, rt: Rt) -> FuncId;
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId;
    fn map_glue(&mut self, ty: Ty) -> DataId;
    fn closure_code(&mut self, body: FnId) -> FuncId;
    fn c_function(&mut self, symbol: &str, signature: &ir::Signature) -> (FuncId, bool);
    fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId;
    fn declare_func_in_func(&mut self, id: FuncId, func: &mut ir::Function) -> ir::FuncRef;
    fn declare_data_in_func(&mut self, id: DataId, func: &mut ir::Function) -> ir::GlobalValue;
    /// The runtime's count of reasons to check for cancellation.
    fn attention(&mut self) -> DataId;

    /// A debug build's function, with the bindings it shows: its number.
    fn debug_function(&mut self, name: String, described: Vec<DebugBinding>) -> u32;
    /// In a live build, the word that holds the number of the function's
    /// first site, and that number.
    fn function_sites(&mut self) -> Option<(DataId, usize)>;
    /// A probe of the function `function` at `start`: its site's number.
    fn debug_site(&mut self, start: u32, function: u32) -> usize;
    /// The number of the site `index`, as an instruction's constant.
    fn site_constant(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value;
    /// In a live build, the site of the probe of a point of the function.
    fn point_site(&mut self, point: u32, number: u32);
    /// In a live build, how a point's state is laid out, once worked out.
    fn point_layout(&self, point: u32) -> Option<StateLayout>;
    fn keep_point_layout(&mut self, point: u32, layout: StateLayout);

    fn class(&mut self, ty: Ty) -> Class {
        self.shapes().class(ty)
    }
    fn layout(&mut self, ty: Ty) -> Layout {
        self.shapes().layout(ty)
    }
    fn scalar_type(&mut self, ty: Ty) -> ir::Type {
        self.shapes().scalar_type(ty)
    }
    fn slot(&mut self, ty: Ty, proj: Proj) -> Slot {
        self.shapes().slot(ty, proj)
    }
    fn signature(&mut self, body: &Body) -> ir::Signature {
        self.shapes().signature(body)
    }
    fn uniform_signature(&mut self, params: &[Ty], loans: &[bool], result: Ty) -> ir::Signature {
        self.shapes().uniform_signature(params, loans, result)
    }
    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        self.shapes().glue_signature(kind)
    }
    fn sequence_parts(&mut self, ty: Ty) -> (Ty, bool) {
        self.shapes().sequence_parts(ty)
    }
    fn map_parts(&mut self, ty: Ty) -> (Ty, Ty) {
        self.shapes().map_parts(ty)
    }
    fn entry_layout(&mut self, ty: Ty) -> (u32, u32) {
        self.shapes().entry_layout(ty)
    }
    fn abi(&mut self) -> cabi::Abi {
        self.shapes().abi
    }
    fn c_plan(&mut self, args: &[Ty], fixed: usize, result: Ty) -> cabi::Plan {
        self.shapes().c_plan(args, fixed, result)
    }
    fn c_signature(&mut self, params: &[Ty], result: Ty) -> ir::Signature {
        self.shapes().c_signature(params, result)
    }
    fn callback_frame(&mut self, ty: Ty) -> (Vec<u32>, u32) {
        self.shapes().callback_frame(ty)
    }
}

impl<'t> Gen<'t> for Codegen<'t> {
    fn shapes(&mut self) -> &mut Shapes<'t> {
        &mut self.shapes
    }
    fn types(&self) -> &'t Types {
        self.types
    }
    fn sources(&self) -> &'t SourceMap {
        self.sources
    }
    fn display(&self) -> &'t HashMap<Ty, FnId> {
        self.display
    }
    fn externs(&self) -> &'t [crate::hir::Extern] {
        self.externs
    }
    fn meta(&self) -> bool {
        self.meta
    }
    fn isa(&self) -> &dyn cranelift_codegen::isa::TargetIsa {
        self.module.isa()
    }
    fn loans(&self, id: FnId) -> Vec<bool> {
        self.loans[id.0 as usize].clone()
    }
    fn function(&self, id: FnId) -> FuncId {
        self.functions[id.0 as usize]
    }
    fn takes_osr(&self, id: FnId) -> bool {
        self.takes_osr[id.0 as usize]
    }
    fn function_signature(&self, id: FuncId) -> ir::Signature {
        self.module.declarations().get_function_decl(id).signature.clone()
    }
    fn live(&self) -> bool {
        self.live.is_some()
    }
    fn slot_of(&self, id: FnId) -> Option<(DataId, u32)> {
        self.live.as_ref().map(|live| (live.slots, live.slot_of[id.0 as usize]))
    }
    fn site_base(&self) -> u32 {
        self.live.as_ref().and_then(|live| live.patch.as_ref()).map_or(0, |(_, sites)| *sites)
    }
    fn debug(&self) -> bool {
        self.debug.is_some()
    }
    fn coverage(&self) -> Option<DataId> {
        self.coverage.as_ref().map(|(counters, _)| *counters)
    }
    fn fuzzing(&self) -> bool {
        self.fuzz
    }
    fn counter(&mut self, counted: Counted) -> usize {
        let (_, all) = self.coverage.as_mut().expect("a build with coverage");
        all.push(counted);
        all.len() - 1
    }
    fn counter_offset(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        fb.ins().iconst(types::I64, index as i64 * 8)
    }
    fn string(&mut self, text: &str) -> DataId {
        Codegen::string(self, text)
    }
    fn static_array(&mut self, size: usize, elements: Vec<Const>) -> DataId {
        Codegen::static_array(self, size, &elements)
    }
    fn bytes(&mut self, bytes: &std::sync::Arc<[u8]>) -> DataId {
        Codegen::bytes(self, bytes)
    }
    fn identity(&mut self, ty: Ty) -> DataId {
        Codegen::identity(self, ty)
    }
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        Codegen::glue(self, kind, ty)
    }
    fn runtime(&mut self, rt: Rt) -> FuncId {
        Codegen::runtime(self, rt)
    }
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId {
        Codegen::fn_value(self, id, body)
    }
    fn map_glue(&mut self, ty: Ty) -> DataId {
        Codegen::map_glue(self, ty)
    }
    fn closure_code(&mut self, body: FnId) -> FuncId {
        Codegen::closure_code(self, body)
    }
    fn c_function(&mut self, symbol: &str, signature: &ir::Signature) -> (FuncId, bool) {
        Codegen::c_function(self, symbol, signature)
    }
    fn callback(&mut self, target: FnId, ty: Ty, closure: bool) -> FuncId {
        Codegen::callback(self, target, ty, closure)
    }
    fn declare_func_in_func(&mut self, id: FuncId, func: &mut ir::Function) -> ir::FuncRef {
        self.module.declare_func_in_func(id, func)
    }
    fn declare_data_in_func(&mut self, id: DataId, func: &mut ir::Function) -> ir::GlobalValue {
        self.module.declare_data_in_func(id, func)
    }
    fn attention(&mut self) -> DataId {
        self.module.declare_data("tondo_rt_attention", Linkage::Import, true, false).expect("the runtime's attention")
    }
    fn debug_function(&mut self, name: String, described: Vec<DebugBinding>) -> u32 {
        self.debug_functions.push((name, described));
        self.debug_functions.len() as u32 - 1
    }
    fn function_sites(&mut self) -> Option<(DataId, usize)> {
        self.live.as_ref()?;
        let name = format!("tondo_sites{}", self.debug_functions.len() - 1);
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("a function's sites");
        let first = self.debug_sites.len();
        self.live.as_mut().expect("a live build").sites.push((id, self.current, first));
        Some((id, first))
    }
    fn debug_site(&mut self, start: u32, function: u32) -> usize {
        self.debug_sites.push((start, function));
        self.debug_sites.len() - 1
    }
    fn site_constant(&mut self, fb: &mut FunctionBuilder, index: usize) -> ir::Value {
        fb.ins().iconst(types::I64, index as i64)
    }
    fn point_site(&mut self, point: u32, number: u32) {
        let function = self.current;
        if let Some(live) = self.live.as_mut() {
            live.point_sites.insert((function, point), number);
        }
    }
    fn point_layout(&self, point: u32) -> Option<StateLayout> {
        self.live.as_ref().and_then(|live| live.layouts.get(&(self.current, point)).cloned())
    }
    fn keep_point_layout(&mut self, point: u32, layout: StateLayout) {
        let function = self.current;
        if let Some(live) = self.live.as_mut() {
            live.layouts.insert((function, point), layout);
        }
    }
}

impl Codegen<'_> {
    /// The static `fn(...)` object of a named function, created on first use.
    fn fn_value(&mut self, id: FnId, body: &Body) -> DataId {
        if let Some(&(data, _)) = self.fn_values.get(&id) {
            return data;
        }
        let params: Vec<Ty> = body.params.iter().map(|p| body.locals[p.0 as usize]).collect();
        let signature = self.uniform_signature(&params, &body.loans, body.result);
        let name = format!("tondo_thunk{}", id.0);
        let thunk = self.module.declare_function(&name, Linkage::Local, &signature).expect("thunk declaration");
        let data =
            self.module.declare_data(&format!("tondo_fn{}", id.0), Linkage::Local, false, false).expect("fn value");
        let mut description = DataDescription::new();
        let mut bytes = vec![0u8; FN_HEADER as usize];
        bytes[..8].copy_from_slice(&IMMORTAL.to_le_bytes());
        description.define(bytes.into_boxed_slice());
        description.set_align(8);
        let reference = self.module.declare_func_in_data(thunk, &mut description);
        description.write_function_addr(16, reference);
        self.module.define_data(data, &description).expect("fn value definition");
        self.fn_values.insert(id, (data, thunk));
        self.pending_thunks.push((id, thunk));
        data
    }

    /// The static table of glue functions the runtime uses for a map or set.
    fn map_glue(&mut self, ty: Ty) -> DataId {
        let ty = self.types.repr(ty);
        if let Some(&id) = self.map_glue.get(&ty) {
            return id;
        }
        let (key, value) = self.map_parts(ty);
        let name = format!("tondo_map_glue{}", self.map_glue.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("glue table");
        let mut data = DataDescription::new();
        data.define(vec![0u8; 64].into_boxed_slice());
        data.set_align(8);
        let entries = [
            (Glue::Hash, key, true),
            (Glue::Equal, key, true),
            (Glue::Retain, key, self.types.needs_drop(key)),
            (Glue::Release, key, self.types.needs_drop(key)),
            (Glue::Retain, value, self.types.needs_drop(value)),
            (Glue::Release, value, self.types.needs_drop(value)),
            (Glue::Visit, key, self.types.can_cycle(key)),
            (Glue::Visit, value, self.types.can_cycle(value)),
        ];
        for (index, (kind, ty, present)) in entries.into_iter().enumerate() {
            if present {
                let function = self.glue(kind, ty);
                let reference = self.module.declare_func_in_data(function, &mut data);
                data.write_function_addr(index as u32 * 8, reference);
            }
        }
        self.module.define_data(id, &data).expect("glue table definition");
        self.map_glue.insert(ty, id);
        id
    }

    /// The glue function of `kind` for values of type `ty`, defined later.
    fn glue(&mut self, kind: Glue, ty: Ty) -> FuncId {
        let ty = self.types.repr(ty);
        if let Some(&id) = self.glue.get(&(kind, ty)) {
            return id;
        }
        let signature = self.glue_signature(kind);
        let name = format!("tondo_glue_{kind:?}_{}", self.glue.len()).to_lowercase();
        let id = self.module.declare_function(&name, Linkage::Local, &signature).expect("glue declaration");
        self.glue.insert((kind, ty), id);
        self.pending_glue.push((kind, ty, id));
        id
    }

    /// The code a closure or a `defer` keeps: its body, or in a live build
    /// a function that calls the body through its slot, so that a closure
    /// made before a patch runs the patch's body.
    fn closure_code(&mut self, body: FnId) -> FuncId {
        if self.live.is_none() {
            return self.functions[body.0 as usize];
        }
        if let Some(&forward) = self.forwarders.get(&body) {
            return forward;
        }
        // The convention of `fn(...)` values: no hidden parameter.
        let mut signature =
            self.module.declarations().get_function_decl(self.functions[body.0 as usize]).signature.clone();
        if self.takes_osr[body.0 as usize] {
            signature.params.pop();
        }
        let name = format!("tondo_forward{}", body.0);
        let forward = self.module.declare_function(&name, Linkage::Local, &signature).expect("a closure's forwarder");
        self.forwarders.insert(body, forward);
        self.pending_forwarders.push((body, forward));
        forward
    }

    fn runtime(&mut self, rt: Rt) -> FuncId {
        if let Some(&id) = self.runtime.get(&rt) {
            return id;
        }
        let (name, signature) = runtime_signature(rt, self.module.make_signature());
        let id = self.module.declare_function(name, Linkage::Import, &signature).expect("runtime declaration");
        self.runtime.insert(rt, id);
        id
    }

    /// The tree of a test build for the runner: for each node, its parent,
    /// whether it is a suite, and its identity.
    /// Records, for stack traces, where each call of the function just
    /// compiled returns to and the source of the statement that makes it.
    /// Compiles the functions on every core, and defines them in order, so
    /// the object is the same each time.
    fn compile_all(&mut self, mut jobs: Vec<Job>) -> Result<(), String> {
        let isa = self.module.isa();
        // Each function goes to the thread that compiles it.
        let functions: Vec<(FuncId, std::sync::Mutex<Option<ir::Function>>)> = jobs
            .iter_mut()
            .map(|job| (job.id, std::sync::Mutex::new(Some(std::mem::replace(&mut job.func, ir::Function::new())))))
            .collect();
        let compiled = crate::parallel::map(&functions, 1, |(id, function)| {
            thread_local! {
                static CONTEXT: std::cell::RefCell<cranelift_codegen::Context> =
                    std::cell::RefCell::new(cranelift_codegen::Context::new());
            }
            let function = function.lock().unwrap_or_else(|e| e.into_inner()).take().expect("a function");
            CONTEXT.with(|context| compile_one(isa, &mut context.borrow_mut(), *id, function))
        });
        let labels = self.live.is_some().then(|| self.labels());
        for (job, code) in jobs.into_iter().zip(compiled) {
            let code = code?;
            if let Some(labels) = &labels {
                let hash = code_hash(&code, labels);
                self.live.as_mut().expect("a live build").code.insert(job.id, hash);
            }
            self.module
                .define_function_bytes(job.id, code.alignment, &code.bytes, &code.relocs)
                .map_err(|e| format!("{e:?}"))?;
            if let Some(name) = job.traced {
                let (size, calls, lines, sites, inlined) = (code.size, code.calls, code.lines, job.sites, job.inlined);
                let handlers = code.handlers;
                self.traced.push(Traced { id: job.id, name, size, calls, handlers, lines, sites, inlined });
            }
        }
        Ok(())
    }

    /// What each function and data a function's code refers to is, the
    /// same from one build to the next: a function by its identity, glue by
    /// its type, a string by its text; the runtime's by their names.
    fn labels(&self) -> Labels {
        let live = self.live.as_ref().expect("a live build");
        let mut functions: HashMap<FuncId, String> = HashMap::new();
        let mut data: HashMap<DataId, String> = HashMap::new();
        for (index, &id) in self.functions.iter().enumerate() {
            functions.insert(id, live.identities[index].clone());
        }
        for (&body, &(value, thunk)) in &self.fn_values {
            functions.insert(thunk, format!("thunk {}", live.identities[body.0 as usize]));
            data.insert(value, format!("fn {}", live.identities[body.0 as usize]));
        }
        for (&(kind, ty), &id) in &self.glue {
            functions.insert(id, format!("glue {kind:?} {}", self.types.display(ty)));
        }
        for (&body, &id) in &self.forwarders {
            functions.insert(id, format!("forward {}", live.identities[body.0 as usize]));
        }
        for (&ty, &id) in &self.map_glue {
            data.insert(id, format!("map glue {}", self.types.display(ty)));
        }
        for (text, &id) in &self.strings {
            data.insert(id, format!("string {text}"));
        }
        for (&ty, &id) in &self.identities {
            data.insert(id, format!("identity {}", self.types.display(ty)));
        }
        for &(id, body, _) in &live.sites {
            data.insert(id, format!("sites {}", live.identities[body]));
        }
        let declarations = self.module.declarations();
        for (id, decl) in declarations.get_functions() {
            functions.entry(id).or_insert_with(|| decl.name.clone().unwrap_or_default());
        }
        for (id, decl) in declarations.get_data_objects() {
            data.entry(id).or_insert_with(|| decl.name.clone().unwrap_or_default());
        }
        Labels { functions, data }
    }

    /// What a live build's manifest keeps of each function's points, for a
    /// patch to move frames from one version's to another's: a hash of each
    /// of the function's lines, which says what lines stay the same, and of
    /// each point its line among them, what it is, its site and what its
    /// state holds, with each local's name across versions.
    fn live_points(&mut self, build: &mut crate::live::Build, bodies: &[Body]) {
        let live = self.live.take().expect("a live build");
        let mut functions = serde_json::Map::new();
        for (index, body) in bodies.iter().enumerate() {
            let placed: Vec<(u32, &crate::mir::Point)> = (0u32..)
                .zip(&body.points)
                .filter(|(number, point)| point.span != Span::default() && live.layouts.contains_key(&(index, *number)))
                .collect();
            let Some(&(_, first)) = placed.first() else { continue };
            let lines: Vec<usize> = placed.iter().map(|(_, p)| self.sources.position(p.span.start).1).collect();
            let (top, bottom) = (*lines.iter().min().expect("a point"), *lines.iter().max().expect("a point"));
            let text = &self.sources.file_of(first.span.start).text;
            let hashes: Vec<u64> =
                text.lines().skip(top - 1).take(bottom - top + 1).map(crate::live::line_hash).collect();
            // A closure's text, which tells which closure it is when the
            // number of its function's changes.
            let shown: Vec<&str> = text.lines().skip(top - 1).take(bottom - top + 1).map(str::trim).collect();
            let shown = body.name.starts_with("closure in ").then(|| shown.join("\n"));
            let mut points = Vec::new();
            for (&(number, point), line) in placed.iter().zip(&lines) {
                let (items, size) = &live.layouts[&(index, number)];
                let mut items: Vec<_> = items.iter().collect();
                items.sort_by_key(|(local, ..)| local.0);
                // Bindings of one name live at once are told apart by order.
                let mut named: HashMap<&str, u32> = HashMap::new();
                let state: Vec<serde_json::Value> = items
                    .into_iter()
                    .filter_map(|(local, kept, offset, bytes)| {
                        let key = match &body.origins[local.0 as usize] {
                            Origin::Named(name) => {
                                let rank = named.entry(name).or_insert(0);
                                *rank += 1;
                                format!("{name}/{}", *rank - 1)
                            }
                            Origin::Made(path, made) => format!("#{}/{made}", crate::live::path_text(path)),
                            Origin::Param(_) => return None,
                        };
                        let ty = body.locals[local.0 as usize];
                        Some(serde_json::json!({
                            "key": key,
                            "type": crate::live::describe(self.types, ty, &mut build.structures),
                            "lent": matches!(kept, StateKept::Pointer(_)),
                            "owned": self.types.needs_drop(ty),
                            "offset": offset,
                            "size": bytes,
                        }))
                    })
                    .collect();
                points.push(serde_json::json!({
                    "point": number,
                    "line": line - top,
                    "kind": point.kind,
                    "path": crate::live::path_text(&point.path),
                    "defers": point.defers,
                    "site": live.point_sites.get(&(index, number)),
                    "size": size,
                    "state": state,
                }));
            }
            let file = self.sources.position(first.span.start).0;
            // How its frames get their parameters, which a frame that moves
            // to a function of another identity passes converted; and its
            // result, which the old caller takes.
            let params: Vec<serde_json::Value> = body
                .params
                .iter()
                .zip(&body.loans)
                .map(|(param, &loan)| {
                    let ty = body.locals[param.0 as usize];
                    let class = match self.class(ty) {
                        Class::Zero => "zero",
                        Class::Scalar(_) => "scalar",
                        Class::Memory => "memory",
                    };
                    let key = crate::live::describe(self.types, ty, &mut build.structures);
                    let size = self.layout(ty).size;
                    serde_json::json!({ "type": key, "loan": loan, "class": class, "size": size })
                })
                .collect();
            let result = crate::live::describe(self.types, body.result, &mut build.structures);
            // A closure's frame works on its captures, through its first
            // parameter; one that changes them or gives them away works on
            // the closure's own, which a copy cannot stand for.
            let (mutates, consumes) = body
                .params
                .first()
                .and_then(|env| self.types.closure_calls(body.locals[env.0 as usize]))
                .unwrap_or_default();
            let returns = match self.class(body.result) {
                Class::Zero => "zero",
                Class::Scalar(_) => "scalar",
                Class::Memory => "memory",
            };
            // Each call a frame may be in: what the rest of its statement
            // reaches through values with identity, and for a wait, whether
            // its statement can start again.
            let mut calls: Vec<serde_json::Value> = crate::live::touches(self.types, body)
                .into_iter()
                .map(|(span, touches)| {
                    let (_, line, column) = self.sources.position(span.start);
                    let again = body.pauses.iter().find(|(at, _)| *at == span).map(|&(_, again)| again);
                    serde_json::json!({ "line": line, "column": column, "touches": touches, "again": again })
                })
                .collect();
            calls.dedup();
            let entry = serde_json::json!({
                "calls": calls,
                "file": file,
                "top": top,
                "lines": hashes,
                "points": points,
                "params": params,
                "result": result,
                "mutates": mutates,
                "consumes": consumes,
                "returns": returns,
                "result_size": self.layout(body.result).size,
                "text": shown,
            });
            functions.insert(live.identities[index].clone(), entry);
        }
        build.points = functions;
        self.live = Some(live);
    }

    /// A live build's tables (toolchain 7.10): the slots, each pointing at
    /// its function; the runtime's functions, the functions of C the
    /// program calls and the identities of its types, which a patch reaches
    /// by their place; and what the image registers with the runtime.
    fn live_tables(&mut self, build: &mut crate::live::Build, trace: DataId) -> Result<(), String> {
        let live = self.live.take().expect("a live build");
        build.code = live
            .identities
            .iter()
            .enumerate()
            .filter_map(|(index, identity)| live.code.get(&self.functions[index]).map(|&h| (identity.clone(), h)))
            .collect();
        let base = live.patch.as_ref().map_or(0, |(_, sites)| *sites as u64);
        for &(id, _, first) in &live.sites {
            let mut word = DataDescription::new();
            word.define((base + first as u64).to_le_bytes().to_vec().into_boxed_slice());
            word.set_align(8);
            self.module.define_data(id, &word).map_err(|e| e.to_string())?;
        }
        if let Some((known, sites)) = &live.patch {
            return self.patch_table(build, &live, known.len(), *sites, trace);
        }
        let mut slots = DataDescription::new();
        slots.define(vec![0u8; build.capacity as usize * 8].into_boxed_slice());
        slots.set_align(8);
        for (index, &slot) in live.slot_of.iter().enumerate() {
            let function = self.module.declare_func_in_data(self.functions[index], &mut slots);
            slots.write_function_addr(slot * 8, function);
        }
        self.module.define_data(live.slots, &slots).map_err(|e| e.to_string())?;
        let runtime = self.address_table("tondo_live_runtime", &build.runtime)?;
        // What Cranelift calls itself, besides what the program calls.
        let mut c: Vec<String> = self.c_functions.keys().cloned().collect();
        c.extend(["memcpy", "memmove", "memset", "memcmp"].map(String::from));
        c.sort();
        c.dedup();
        build.c = c;
        let c = self.address_table("tondo_live_c", &build.c)?;
        let mut identities: Vec<(String, DataId)> =
            self.identities.iter().map(|(&ty, &id)| (self.identity_key(ty), id)).collect();
        identities.sort();
        build.data = identities.iter().map(|(name, _)| name.clone()).collect();
        // With room for the types patches add.
        let room = identities.len() + 1024;
        let data =
            self.module.declare_data("tondo_live_data", Linkage::Local, true, false).map_err(|e| e.to_string())?;
        let mut table = DataDescription::new();
        table.define(vec![0u8; room * 8].into_boxed_slice());
        table.set_align(8);
        for (index, (_, id)) in identities.iter().enumerate() {
            let identity = self.module.declare_data_in_data(*id, &mut table);
            table.write_data_addr(index as u32 * 8, identity, 0);
        }
        self.module.define_data(data, &table).map_err(|e| e.to_string())?;
        build.sites = self.debug_sites.len() as u32;
        let id = self.string(&build.id);
        let used = live.slot_of.iter().max().map_or(0, |&slot| slot as u64 + 1);
        let words: [(u64, Option<DataId>); 12] = [
            (0, Some(live.slots)),
            (build.capacity as u64, None),
            (used.max(build.slots.values().max().map_or(0, |&s| s as u64 + 1)), None),
            (0, Some(runtime)),
            (build.runtime.len() as u64, None),
            (0, Some(c)),
            (build.c.len() as u64, None),
            (0, Some(data)),
            (build.data.len() as u64, None),
            (room as u64, None),
            (0, Some(id)),
            (build.sites as u64, None),
        ];
        let mut info = DataDescription::new();
        info.define(words.iter().flat_map(|(value, _)| value.to_le_bytes()).collect::<Vec<u8>>().into_boxed_slice());
        info.set_align(8);
        for (index, (_, target)) in words.iter().enumerate() {
            if let Some(target) = target {
                let target = self.module.declare_data_in_data(*target, &mut info);
                info.write_data_addr(index as u32 * 8, target, 0);
            }
        }
        self.module.define_data(live.info, &info).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// A patch's table, which the runtime reads once it maps the patch:
    /// its tables of stack traces and of the debugger's probes, the site
    /// its probes number from, then the slots it sets and the identities
    /// of the types it adds.
    fn patch_table(
        &mut self,
        build: &mut crate::live::Build,
        live: &LiveCodegen,
        known: usize,
        sites: u32,
        trace: DataId,
    ) -> Result<(), String> {
        let mut added: Vec<(String, DataId)> = self
            .identities
            .iter()
            .filter(|(_, id)| self.module.declarations().get_data_decl(**id).linkage != Linkage::Import)
            .map(|(&ty, &id)| (self.identity_key(ty), id))
            .collect();
        added.sort();
        // The functions whose code changes or is new: the others go on
        // running the code the program has, whose frames a later patch
        // moves as it knows their points.
        let set: Vec<usize> = (0..live.slot_of.len())
            .filter(|&index| live.known.get(&live.identities[index]) != live.code.get(&self.functions[index]))
            .collect();
        let start = 40 + set.len() * 16;
        // Then the conversions: their count, and each one's function and
        // the release glue of what it converts from and to.
        let conversions = start + added.len() * 16;
        // Then what values with identity hold that converts: the count, and
        // each one's conversion, old identity, new size and alignment, and
        // release and visit glue.
        let objects = conversions + 8 + live.conversions.len() * 24;
        let mut bytes = vec![0u8; objects + 8 + live.objects.len() * 56];
        bytes[conversions..conversions + 8].copy_from_slice(&(live.conversions.len() as u64).to_le_bytes());
        bytes[objects..objects + 8].copy_from_slice(&(live.objects.len() as u64).to_le_bytes());
        for (index, &(conversion, _, _, size, align, _, _)) in live.objects.iter().enumerate() {
            let at = objects + 8 + index * 56;
            bytes[at..at + 8].copy_from_slice(&(conversion as u64).to_le_bytes());
            bytes[at + 24..at + 32].copy_from_slice(&(size as u64).to_le_bytes());
            bytes[at + 32..at + 40].copy_from_slice(&(align as u64).to_le_bytes());
        }
        bytes[16..24].copy_from_slice(&(sites as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(set.len() as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&(added.len() as u64).to_le_bytes());
        for (at, &index) in set.iter().enumerate() {
            bytes[40 + at * 16..48 + at * 16].copy_from_slice(&(live.slot_of[index] as u64).to_le_bytes());
        }
        for index in 0..added.len() {
            let at = start + index * 16;
            bytes[at..at + 8].copy_from_slice(&((known + index) as u64).to_le_bytes());
        }
        let mut table = DataDescription::new();
        table.define(bytes.into_boxed_slice());
        table.set_align(8);
        let trace = self.module.declare_data_in_data(trace, &mut table);
        table.write_data_addr(0, trace, 0);
        if let Some(debug) = self.debug {
            let debug = self.module.declare_data_in_data(debug, &mut table);
            table.write_data_addr(8, debug, 0);
        }
        for (at, &index) in set.iter().enumerate() {
            let function = self.module.declare_func_in_data(self.functions[index], &mut table);
            table.write_function_addr((48 + at * 16) as u32, function);
        }
        for (index, (name, id)) in added.iter().enumerate() {
            let identity = self.module.declare_data_in_data(*id, &mut table);
            table.write_data_addr((start + index * 16 + 8) as u32, identity, 0);
            build.data.push(name.clone());
        }
        for (index, &(convert, from, to)) in live.conversions.iter().enumerate() {
            let at = conversions + 8 + index * 24;
            for (offset, function) in [(0, Some(convert)), (8, from), (16, to)] {
                if let Some(function) = function {
                    let function = self.module.declare_func_in_data(function, &mut table);
                    table.write_function_addr((at + offset) as u32, function);
                }
            }
        }
        for (index, &(_, from, to, _, _, release, visit)) in live.objects.iter().enumerate() {
            let at = objects + 8 + index * 56;
            for (offset, identity) in [(8, from), (16, to)] {
                let identity = self.module.declare_data_in_data(identity, &mut table);
                table.write_data_addr((at + offset) as u32, identity, 0);
            }
            for (offset, function) in [(40, release), (48, visit)] {
                if let Some(function) = function {
                    let function = self.module.declare_func_in_data(function, &mut table);
                    table.write_function_addr((at + offset) as u32, function);
                }
            }
        }
        self.module.define_data(live.info, &table).map_err(|e| e.to_string())?;
        build.sites = sites + self.debug_sites.len() as u32;
        Ok(())
    }

    /// A table of the addresses of functions by name: those the program
    /// declares, or others as imports.
    fn address_table(&mut self, name: &str, functions: &[String]) -> Result<DataId, String> {
        use cranelift_module::FuncOrDataId;
        let id = self.module.declare_data(name, Linkage::Local, false, false).map_err(|e| e.to_string())?;
        let mut table = DataDescription::new();
        table.define(vec![0u8; functions.len().max(1) * 8].into_boxed_slice());
        table.set_align(8);
        for (index, function) in functions.iter().enumerate() {
            let offset = index as u32 * 8;
            match self.module.get_name(function) {
                Some(FuncOrDataId::Data(data)) => {
                    let data = self.module.declare_data_in_data(data, &mut table);
                    table.write_data_addr(offset, data, 0);
                }
                found => {
                    let func = match found {
                        Some(FuncOrDataId::Func(func)) => func,
                        _ => {
                            let signature = self.module.make_signature();
                            self.module
                                .declare_function(function, Linkage::Import, &signature)
                                .map_err(|e| e.to_string())?
                        }
                    };
                    let func = self.module.declare_func_in_data(func, &mut table);
                    table.write_function_addr(offset, func);
                }
            }
        }
        self.module.define_data(id, &table).map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// The stack trace table (the runtime's `trace::Table`): each function's
    /// code and size, and in one blob its name, its calls, its lines and
    /// the calls inlined into it, then the names they refer to: their
    /// files, the types the calls assign and the functions inlined. A function's lines are its file and, in as many bytes as
    /// they take, where each run of code of one line starts and its line,
    /// each as the difference from the run before, in LEB128, the line's
    /// zigzag; line `0` is code of no line.
    fn trace_table(&mut self, id: DataId) {
        let pad = |blob: &mut Vec<u8>| blob.resize(blob.len().next_multiple_of(4), 0);
        let mut blob = Vec::new();
        let mut files: Vec<String> = Vec::new();
        let name = |files: &mut Vec<String>, text: String| match files.iter().position(|f| *f == text) {
            Some(found) => found as u32,
            None => {
                files.push(text);
                files.len() as u32 - 1
            }
        };
        let mut entries = vec![0u8; self.traced.len() * 16];
        for (index, function) in self.traced.iter().enumerate() {
            let meta = blob.len() as u32;
            entries[index * 16 + 8..index * 16 + 12].copy_from_slice(&function.size.to_le_bytes());
            entries[index * 16 + 12..index * 16 + 16].copy_from_slice(&meta.to_le_bytes());
            blob.extend_from_slice(&(function.name.len() as u32).to_le_bytes());
            blob.extend_from_slice(&(function.calls.len() as u32).to_le_bytes());
            blob.extend_from_slice(function.name.as_bytes());
            pad(&mut blob);
            for &(offset, site) in &function.calls {
                let (source, ty, scope) = function.sites[site as usize - 1];
                let (file, line, column) = self.sources.position(source);
                let file = name(&mut files, file);
                let ty = match ty {
                    Some(ty) => name(&mut files, self.types.display(ty)),
                    None => u32::MAX,
                };
                for value in [offset, line as u32, column as u32, file, ty, scope.unwrap_or(u32::MAX)] {
                    blob.extend_from_slice(&value.to_le_bytes());
                }
            }
            // Each run of code of one line, and of one inlined call: its
            // line is in the inlined function's file. Code of no statement,
            // as the moves and jumps between two, is the one before's.
            let mut file = None;
            let placed: Vec<(u32, Option<(i64, u32)>)> = function
                .lines
                .iter()
                .map(|&(start, site)| {
                    let placed = (site != 0).then(|| {
                        let (source, _, scope) = function.sites[site as usize - 1];
                        let (path, line, _) = self.sources.position(source);
                        if scope.is_none() {
                            file.get_or_insert(path);
                        }
                        (line as i64, scope.map_or(0, |scope| scope + 1))
                    });
                    (start, placed)
                })
                .collect();
            // Code before the first statement is the first statement's;
            // a debugger's builds keep it of none, which it does not stop in.
            let mut known = placed.iter().find_map(|&(_, placed)| placed).unwrap_or((0, 0));
            let mut runs: Vec<(u32, i64, u32)> = Vec::new();
            for (start, placed) in placed {
                let (line, scope) = placed.unwrap_or(if self.debug.is_some() { (0, 0) } else { known });
                known = (line, scope);
                if runs.last().is_none_or(|&(_, last, inlined)| (last, inlined) != (line, scope)) {
                    runs.push((start, line, scope));
                }
            }
            let mut encoded = Vec::new();
            let (mut offset, mut line) = (0u32, 0i64);
            for (start, now, scope) in runs {
                leb128(&mut encoded, (start - offset) as u64);
                leb128(&mut encoded, ((now - line) << 1 ^ (now - line) >> 63) as u64);
                leb128(&mut encoded, scope as u64);
                (offset, line) = (start, now);
            }
            let file = file.map_or(u32::MAX, |path| name(&mut files, path));
            blob.extend_from_slice(&file.to_le_bytes());
            blob.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            blob.extend_from_slice(&encoded);
            pad(&mut blob);
            // Each inlined call: the function inlined, where it was called,
            // the inlined call that holds it, and the inlined function's file.
            for inlined in &function.inlined {
                let (path, line, column) = self.sources.position(inlined.call.start);
                let (called, path) = (name(&mut files, inlined.name.clone()), name(&mut files, path));
                let own = name(&mut files, self.sources.position(inlined.span.start).0);
                let parent = inlined.parent.unwrap_or(u32::MAX);
                for value in [called, line as u32, column as u32, path, parent, own] {
                    blob.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let files_at = blob.len() as u64;
        blob.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for file in &files {
            blob.extend_from_slice(&(file.len() as u32).to_le_bytes());
            blob.extend_from_slice(file.as_bytes());
            pad(&mut blob);
        }
        let define = |module: &mut ObjectModule, name: &str, bytes: Vec<u8>| {
            let id = module.declare_data(name, Linkage::Local, false, false).expect("trace data");
            let mut data = DataDescription::new();
            data.define(bytes.into_boxed_slice());
            data.set_align(8);
            module.define_data(id, &data).expect("trace data definition");
            id
        };
        let blob = define(&mut self.module, "tondo_trace_blob", blob);
        let functions = self.module.declare_data("tondo_trace_functions", Linkage::Local, false, false).unwrap();
        let mut data = DataDescription::new();
        data.define(entries.into_boxed_slice());
        data.set_align(8);
        for (index, function) in self.traced.iter().enumerate() {
            let code = self.module.declare_func_in_data(function.id, &mut data);
            data.write_function_addr(index as u32 * 16, code);
        }
        self.module.define_data(functions, &data).expect("trace functions");
        // Each `try_call`'s handler: the function, its return address and
        // the handler, in the order of the functions and their code.
        let mut handlers = Vec::new();
        for (index, function) in self.traced.iter().enumerate() {
            for &(ret, at) in &function.handlers {
                for value in [index as u32, ret, at] {
                    handlers.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let handled = handlers.len() as u64 / 12;
        let handlers = define(&mut self.module, "tondo_trace_handlers", handlers);
        let mut header = vec![0u8; 48];
        header[8..16].copy_from_slice(&(self.traced.len() as u64).to_le_bytes());
        header[24..32].copy_from_slice(&files_at.to_le_bytes());
        header[40..48].copy_from_slice(&handled.to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let functions = self.module.declare_data_in_data(functions, &mut data);
        let blob = self.module.declare_data_in_data(blob, &mut data);
        let handlers = self.module.declare_data_in_data(handlers, &mut data);
        data.write_data_addr(0, functions, 0);
        data.write_data_addr(16, blob, 0);
        data.write_data_addr(32, handlers, 0);
        self.module.define_data(id, &data).expect("trace table");
    }

    /// A debug build's table (the runtime's `debugger::Table`): a text of
    /// lines, `F` and a function's name, `L` a binding's name and type for
    /// each it shows, and `S` a probe's file, line and function; and the
    /// glue that shows each binding, in the order of the `L` lines.
    fn debug_table(&mut self, id: DataId) {
        let mut text = String::new();
        let mut glues = Vec::new();
        for (name, bindings) in &self.debug_functions {
            text.push_str(&format!("F\t{name}\n"));
            for &(ref binding, ty, glue) in bindings {
                text.push_str(&format!("L\t{binding}\t{}\n", self.types.display(ty)));
                glues.push(glue);
            }
        }
        for &(start, function) in &self.debug_sites {
            let (file, line, _) = self.sources.position(start);
            text.push_str(&format!("S\t{file}\t{line}\t{function}\n"));
        }
        let text = self.string(&text);
        let array = self.module.declare_data("tondo_debug_glue", Linkage::Local, false, false).expect("debug glue");
        let mut data = DataDescription::new();
        data.define(vec![0u8; glues.len().max(1) * 8].into_boxed_slice());
        data.set_align(8);
        for (index, &glue) in glues.iter().enumerate() {
            let code = self.module.declare_func_in_data(glue, &mut data);
            data.write_function_addr(index as u32 * 8, code);
        }
        self.module.define_data(array, &data).expect("debug glue definition");
        let mut header = vec![0u8; 24];
        header[16..24].copy_from_slice(&(glues.len() as u64).to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let text = self.module.declare_data_in_data(text, &mut data);
        let array = self.module.declare_data_in_data(array, &mut data);
        data.write_data_addr(0, text, 0);
        data.write_data_addr(8, array, 0);
        self.module.define_data(id, &data).expect("debug table");
    }

    fn test_table(&mut self, tests: &[crate::hir::TestNodeInfo]) -> DataId {
        let names: Vec<DataId> = tests.iter().map(|t| self.string(&t.id)).collect();
        let id = self.module.declare_data("tondo_tests", Linkage::Local, false, false).expect("test table");
        let mut bytes = vec![0u8; tests.len() * 24];
        for (index, test) in tests.iter().enumerate() {
            bytes[index * 24..index * 24 + 8].copy_from_slice(&(test.parent as u64).to_le_bytes());
            bytes[index * 24 + 8..index * 24 + 16].copy_from_slice(&(test.suite as u64).to_le_bytes());
        }
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        for (index, name) in names.into_iter().enumerate() {
            let name = self.module.declare_data_in_data(name, &mut data);
            data.write_data_addr(index as u32 * 24 + 16, name, 0);
        }
        self.module.define_data(id, &data).expect("test table definition");
        id
    }

    /// The identity of a type for `std.reflect`: a static holding its name,
    /// whose address tells the type apart.
    fn identity(&mut self, ty: Ty) -> DataId {
        if let Some(&id) = self.identities.get(&ty) {
            return id;
        }
        let shown = self.types.display(ty);
        // A patch shares the running program's identity of a type it has,
        // of the same shape.
        let key = self.identity_key(ty);
        if let Some(index) = self.live.as_ref().and_then(|live| live.patch.as_ref()?.0.iter().position(|t| *t == key)) {
            let symbol = format!("tondo_live_data{index}");
            let id = self.module.declare_data(&symbol, Linkage::Import, false, false).expect("type identity");
            self.identities.insert(ty, id);
            return id;
        }
        let name = self.string(&shown);
        let symbol = format!("tondo_type{}", self.identities.len());
        let id = self.module.declare_data(&symbol, Linkage::Local, false, false).expect("type identity");
        let mut data = DataDescription::new();
        data.define(vec![0u8; 8].into_boxed_slice());
        data.set_align(8);
        let name = self.module.declare_data_in_data(name, &mut data);
        data.write_data_addr(0, name, 0);
        self.module.define_data(id, &data).expect("type identity definition");
        self.identities.insert(ty, id);
        id
    }

    /// How a live build's tables name a type's identity: by its name and
    /// the digest of its shape.
    fn identity_key(&self, ty: Ty) -> String {
        crate::live::type_key(self.types, ty)
    }

    /// A static array of constants: `[count][length][room][head]`, then
    /// each element's bytes, a string's the address of its literal. Its
    /// count is written to as it is shared, never freed.
    fn static_array(&mut self, size: usize, elements: &[Const]) -> DataId {
        let mut bytes = Vec::with_capacity(ARRAY_HEADER as usize + size * elements.len());
        let length = elements.len() as u64;
        for word in [SHARED_FOREVER, length, length, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut texts = Vec::new();
        for element in elements {
            let at = bytes.len();
            match element {
                Const::Int(value, _) => bytes.extend_from_slice(&value.to_le_bytes()[..size]),
                Const::Float(bits, _) if size == 4 => {
                    bytes.extend_from_slice(&(f64::from_bits(*bits) as f32).to_bits().to_le_bytes())
                }
                Const::Float(bits, _) => bytes.extend_from_slice(&bits.to_le_bytes()),
                Const::Bool(value) => bytes.push(*value as u8),
                Const::Char(c) => bytes.extend_from_slice(&(*c as u32).to_le_bytes()),
                Const::Str(text) => {
                    texts.push((at, self.string(text)));
                    bytes.extend_from_slice(&[0; 8]);
                }
                Const::Bytes(contents, _) => {
                    texts.push((at, self.bytes(contents)));
                    bytes.extend_from_slice(&[0; 8]);
                }
                Const::Unit => {}
            }
            bytes.resize(at + size, 0);
        }
        let name = format!("tondo_array{}", self.arrays);
        self.arrays += 1;
        let id = self.module.declare_data(&name, Linkage::Local, true, false).expect("array declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        for (at, text) in texts {
            let text = self.module.declare_data_in_data(text, &mut data);
            data.write_data_addr(at as u32, text, 0);
        }
        self.module.define_data(id, &data).expect("array definition");
        id
    }

    /// The static `Array[Byte]` of an embedded file's bytes, shared as any
    /// static array is.
    fn bytes(&mut self, contents: &std::sync::Arc<[u8]>) -> DataId {
        if let Some(&id) = self.byte_arrays.get(contents) {
            return id;
        }
        let length = contents.len() as u64;
        let mut bytes = Vec::with_capacity(ARRAY_HEADER as usize + contents.len());
        for word in [SHARED_FOREVER, length, length, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(contents);
        let name = format!("tondo_array{}", self.arrays);
        self.arrays += 1;
        let id = self.module.declare_data(&name, Linkage::Local, true, false).expect("array declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        self.module.define_data(id, &data).expect("array definition");
        self.byte_arrays.insert(contents.clone(), id);
        id
    }

    /// A static, immortal string object: `[refcount][length][room][bytes]`.
    fn string(&mut self, text: &str) -> DataId {
        if let Some(&id) = self.strings.get(text) {
            return id;
        }
        let mut bytes = Vec::with_capacity(24 + text.len());
        bytes.extend_from_slice(&IMMORTAL.to_le_bytes());
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        let name = format!("tondo_str{}", self.strings.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).expect("string declaration");
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        self.module.define_data(id, &data).expect("string definition");
        self.strings.insert(text.to_string(), id);
        id
    }

    /// `tondo_main`, called by the runtime's C `main`.
    /// `tondo_main`, called by the runtime's C `main`. It returns the process
    /// status: 0, or 1 after reporting an error that a fallible `main`
    /// returned (20.10).
    /// What a library runs before the first call from C: it hands the
    /// runtime its tables.
    fn library_start(
        &mut self,
        trace: DataId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let signature = self.module.make_signature();
        let id =
            self.module.declare_function("tondo_library", Linkage::Local, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 2);
        let fb = FunctionBuilder::new(&mut context.func, fbc);
        let mut e = Emit { cg: self, fb };
        let block = e.fb.create_block();
        e.fb.switch_to_block(block);
        let trace = e.cg.module.declare_data_in_func(trace, e.fb.func);
        let trace = e.fb.ins().symbol_value(POINTER, trace);
        e.call_rt(Rt::Trace, &[trace]);
        e.register_contexts();
        if let Some(debug) = e.cg.debug {
            let debug = e.cg.module.declare_data_in_func(debug, e.fb.func);
            let debug = e.fb.ins().symbol_value(POINTER, debug);
            e.call_rt(Rt::DebugTable, &[debug]);
        }
        e.fb.ins().return_(&[]);
        e.fb.seal_all_blocks();
        let config = e.cg.module.isa().frontend_config();
        e.fb.finalize(config);
        Ok(id)
    }

    /// The process's `main`, which C's start calls: the runtime runs
    /// `tondo_main` and gives its status.
    fn process_main(
        &mut self,
        entry: FuncId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let mut signature = self.module.make_signature();
        signature.params = vec![AbiParam::new(types::I32), AbiParam::new(POINTER)];
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 1);
        let fb = FunctionBuilder::new(&mut context.func, fbc);
        let mut e = Emit { cg: self, fb };
        let block = e.fb.create_block();
        e.fb.append_block_params_for_function_params(block);
        e.fb.switch_to_block(block);
        // A live build's tables, and then a debug build's, before a
        // debugger can ask for them.
        if let Some(info) = e.cg.live.as_ref().map(|live| live.info) {
            let info = e.cg.module.declare_data_in_func(info, e.fb.func);
            let info = e.fb.ins().symbol_value(POINTER, info);
            e.call_rt(Rt::Live, &[info]);
        }
        if let Some(debug) = e.cg.debug {
            let debug = e.cg.module.declare_data_in_func(debug, e.fb.func);
            let debug = e.fb.ins().symbol_value(POINTER, debug);
            e.call_rt(Rt::DebugTable, &[debug]);
        }
        // A program that reads its metrics counts them from the start
        // (standard library 14.25).
        if e.cg.module.get_name("tondo_rt_metrics").is_some() {
            e.call_rt(Rt::MetricsWatch, &[]);
        }
        let entry = e.cg.module.declare_func_in_func(entry, e.fb.func);
        let entry = e.fb.ins().func_addr(POINTER, entry);
        let status = e.call_rt(Rt::Main, &[entry]).expect("an exit status");
        e.fb.ins().return_(&[status]);
        e.fb.seal_all_blocks();
        let config = e.cg.module.isa().frontend_config();
        e.fb.finalize(config);
        Ok(id)
    }

    fn entry_point(
        &mut self,
        main: usize,
        result: Ty,
        tests: &[crate::hir::TestNodeInfo],
        trace: DataId,
        context: &mut cranelift_codegen::Context,
        fbc: &mut FunctionBuilderContext,
    ) -> Result<FuncId, String> {
        let table = (!tests.is_empty()).then(|| self.test_table(tests));
        let mut signature = self.module.make_signature();
        signature.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("tondo_main", Linkage::Export, &signature).map_err(|e| e.to_string())?;
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, 0);
        {
            let fb = FunctionBuilder::new(&mut context.func, fbc);
            let mut e = Emit { cg: self, fb };
            let block = e.fb.create_block();
            e.fb.switch_to_block(block);
            let trace = e.cg.module.declare_data_in_func(trace, e.fb.func);
            let trace = e.fb.ins().symbol_value(POINTER, trace);
            e.call_rt(Rt::Trace, &[trace]);
            e.register_contexts();
            // A test build hands the runner its tree before `main` runs it.
            if let Some(table) = table {
                let table = e.cg.module.declare_data_in_func(table, e.fb.func);
                let address = e.fb.ins().symbol_value(POINTER, table);
                let count = e.fb.ins().iconst(types::I64, tests.len() as i64);
                e.call_rt(Rt::TestStart, &[address, count]);
            }
            // With coverage, the counters, which the runtime writes out.
            if let Some((counters, counted)) = &e.cg.coverage {
                let (counters, count) = (*counters, counted.len() as i64);
                let counters = e.cg.module.declare_data_in_func(counters, e.fb.func);
                let address = e.fb.ins().symbol_value(POINTER, counters);
                let count = e.fb.ins().iconst(types::I64, count);
                e.call_rt(Rt::CoverageStart, &[address, count]);
            }
            let main = FnId(main as u32);
            let value = match e.cg.class(result) {
                Class::Memory => {
                    let layout = e.cg.layout(result);
                    let data = StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        layout.size,
                        layout.align.trailing_zeros() as u8,
                    );
                    let slot = e.fb.create_sized_stack_slot(data);
                    let address = e.fb.ins().stack_addr(POINTER, slot, 0);
                    call_body(e.cg, &mut e.fb, main, &[address]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = call_body(e.cg, &mut e.fb, main, &[]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    call_body(e.cg, &mut e.fb, main, &[]);
                    Val::Zero
                }
            };
            if let TyKind::Result(_, error) = *e.cg.types.kind(result) {
                let tag = e.tag(result, value);
                let failed = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, ERR as i64);
                let (report, done) = (e.fb.create_block(), e.fb.create_block());
                e.fb.ins().brif(failed, report, &[], done, &[]);
                e.fb.switch_to_block(report);
                let error_value = match value {
                    Val::Memory(address) => {
                        let slot = e.cg.slot(result, Proj::Variant(ERR, 0));
                        let address = e.component_address(address, &slot);
                        e.value_at(address, error)
                    }
                    _ => Val::Zero,
                };
                let description = e.describe(error, error_value, true);
                let ty = e.type_name(error, error_value);
                e.call_rt(Rt::ReportError, &[description, ty]);
                e.call_rt(Rt::StrRelease, &[description]);
                e.refcount(result, value, true);
                let one = e.fb.ins().iconst(types::I32, 1);
                e.fb.ins().return_(&[one]);
                e.fb.switch_to_block(done);
            }
            // `main` returns an owned value; release it before exiting.
            e.refcount(result, value, true);
            let status = e.fb.ins().iconst(types::I32, 0);
            e.fb.ins().return_(&[status]);
            e.fb.seal_all_blocks();
            let config = e.cg.module.isa().frontend_config();
            e.fb.finalize(config);
        }
        Ok(id)
    }
}

/// What a `try_call` calls: a function, or code at an address.
#[derive(Clone, Copy)]
enum Called {
    Direct(ir::FuncRef),
    Indirect(ir::SigRef, ir::Value),
}

impl Emit<'_, Codegen<'_>> {
    /// Hands the runtime each context's functions, the maker of its
    /// initial value and those that show its fields, and the release of its
    /// record (11.16).
    fn register_contexts(&mut self) {
        for (index, (fns, record)) in self.cg.contexts.clone().into_iter().enumerate() {
            let address = |e: &mut Self, id: Option<FnId>| match id {
                Some(id) => {
                    let function = e.cg.function(id);
                    let callee = e.cg.declare_func_in_func(function, e.fb.func);
                    e.fb.ins().func_addr(POINTER, callee)
                }
                None => e.fb.ins().iconst(POINTER, 0),
            };
            let maker = address(self, Some(fns.maker));
            let (log, label) = (address(self, fns.log), address(self, fns.label));
            let release = self.glue_or_null(Glue::Release, record);
            let show = self.glue_address(Glue::Show, record);
            let name = self.cg.types().display(record);
            let name = self.string_ptr(&name);
            let index = self.fb.ins().iconst(types::I64, index as i64);
            self.call_rt(Rt::ContextRegister, &[index, maker, release, log, label, name, show]);
            for (field, computes, named) in &fns.computed {
                let computes = address(self, Some(*computes));
                let field = self.fb.ins().iconst(types::I64, i64::from(*field));
                let named = self.string_ptr(named);
                self.call_rt(Rt::ContextComputed, &[index, field, computes, named]);
            }
            // The tools show its fields one by one, the computed ones from
            // where they are, since the record holds zeros for them.
            let fields = match self.cg.types().kind(record) {
                TyKind::Adt(id) => match &self.cg.types().adt(*id).kind {
                    AdtKind::Record(fields) => fields.iter().map(|f| f.name.clone()).collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let slots: Vec<Slot> = (0..fields.len()).map(|i| self.cg.slot(record, Proj::Field(i as u32))).collect();
            if fns.computed.is_empty() || slots.iter().any(|slot| slot.boxed) {
                continue;
            }
            for (name, slot) in fields.iter().zip(&slots) {
                let offset = self.fb.ins().iconst(types::I64, i64::from(slot.offset));
                let show = match self.cg.class(slot.ty) {
                    Class::Zero => self.fb.ins().iconst(POINTER, 0),
                    _ => self.glue_address(Glue::Show, slot.ty),
                };
                let name = self.string_ptr(name);
                self.call_rt(Rt::ContextShows, &[index, offset, show, name]);
            }
        }
    }
}

/// Code emission shared by function bodies and glue functions.
struct Emit<'a, G: ?Sized> {
    cg: &'a mut G,
    fb: FunctionBuilder<'a>,
}

impl<'t, G: Gen<'t> + ?Sized> Emit<'_, G> {
    fn call_rt(&mut self, rt: Rt, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.runtime(rt);
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn call_glue(&mut self, kind: Glue, ty: Ty, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn glue_address(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.declare_func_in_func(id, self.fb.func);
        self.fb.ins().func_addr(POINTER, callee)
    }

    /// A glue function's address, or null when a retain or release of `ty`
    /// has nothing to do.
    fn glue_or_null(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        if matches!(kind, Glue::Retain | Glue::Release) && !self.cg.types().needs_drop(ty)
            || kind == Glue::Visit && !self.cg.types().can_cycle(ty)
        {
            return self.fb.ins().iconst(POINTER, 0);
        }
        self.glue_address(kind, ty)
    }

    fn map_glue_address(&mut self, ty: Ty) -> ir::Value {
        let id = self.cg.map_glue(ty);
        let global = self.cg.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
    }

    /// An integer constant of type `ty`, from its bits.
    fn int_const(&mut self, ty: ir::Type, value: i128) -> ir::Value {
        // Narrow immediates are zero-extended bit patterns; one of 128 bits
        // is made of its two halves.
        match ty.bits() {
            128 => {
                let low = self.fb.ins().iconst(types::I64, value as i64);
                let high = self.fb.ins().iconst(types::I64, (value >> 64) as i64);
                self.fb.ins().iconcat(low, high)
            }
            64 => self.fb.ins().iconst(ty, value as i64),
            bits => self.fb.ins().iconst(ty, (value as i64) & ((1i64 << bits) - 1)),
        }
    }

    /// Mixes `value` into a running hash.
    fn mix(&mut self, hash: ir::Value, value: ir::Value) -> ir::Value {
        let combined = self.fb.ins().bxor(hash, value);
        self.fb.ins().imul_imm_s(combined, 0x100_0000_01b3)
    }

    /// A 64-bit hash of a `Key` value (8.12).
    fn hash(&mut self, ty: Ty, value: Val) -> ir::Value {
        let seed = self.fb.ins().iconst(types::I64, 0xcbf2_9ce4_8422_2325u64 as i64);
        match value {
            Val::Zero => seed,
            Val::Scalar(value) => {
                let word = match *self.cg.types().kind(self.cg.types().repr(ty)) {
                    TyKind::String => self.call_rt(Rt::StrHash, &[value]).unwrap(),
                    TyKind::Int(int) if int.bits() == 128 => {
                        let (low, high) = self.fb.ins().isplit(value);
                        let hash = self.mix(seed, low);
                        return self.mix(hash, high);
                    }
                    TyKind::Int(int) if int.signed() && int.bits() < 64 => self.fb.ins().sextend(types::I64, value),
                    _ => match self.cg.scalar_type(ty).bits() {
                        64 => value,
                        _ => self.fb.ins().uextend(types::I64, value),
                    },
                };
                self.mix(seed, word)
            }
            Val::Memory(address) => match self.cg.layout(ty).shape {
                Shape::Struct(_) => {
                    let count = self.cg.types().components(ty).unwrap().len() as u32;
                    let mut hash = seed;
                    for index in 0..count {
                        let slot = self.cg.slot(ty, Proj::Field(index));
                        let part = self.component_address(address, &slot);
                        let part = if slot.boxed {
                            self.call_glue(Glue::Hash, slot.ty, &[part]).unwrap()
                        } else {
                            let value = self.value_at(part, slot.ty);
                            self.hash(slot.ty, value)
                        };
                        hash = self.mix(hash, part);
                    }
                    hash
                }
                _ => self.call_glue(Glue::Hash, ty, &[address]).unwrap(),
            },
        }
    }

    fn element_size(&mut self, element: Ty) -> ir::Value {
        let size = self.cg.layout(element).size;
        self.fb.ins().iconst(POINTER, size as i64)
    }

    /// Appends the `Display` text of `value` to `builder`.
    fn append_display(&mut self, builder: ir::Value, ty: Ty, value: Val) {
        if let Some(&id) = self.cg.display().get(&ty) {
            // A user type: its `Display.display` returns an owned string.
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let call = call_body(self.cg, &mut self.fb, id, &args);
            let text = self.fb.inst_results(call)[0];
            self.call_rt(Rt::SbStr, &[builder, text]);
            self.call_rt(Rt::StrRelease, &[text]);
            return;
        }
        let types = self.cg.types();
        if let TyKind::Result(success, failure) = *types.kind(types.repr(ty)) {
            let tag = self.tag(ty, value);
            let (good, bad, done) = (self.fb.create_block(), self.fb.create_block(), self.fb.create_block());
            let is_ok = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, OK as i64);
            self.fb.ins().brif(is_ok, good, &[], bad, &[]);
            for (block, variant, inner, name) in [(good, OK, success, "ok("), (bad, ERR, failure, "err(")] {
                self.fb.switch_to_block(block);
                let open = self.string_ptr(name);
                self.call_rt(Rt::SbStr, &[builder, open]);
                let payload = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Variant(variant, 0));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, inner)
                    }
                    _ => Val::Zero,
                };
                self.append_display(builder, inner, payload);
                let close = self.string_ptr(")");
                self.call_rt(Rt::SbStr, &[builder, close]);
                self.fb.ins().jump(done, &[]);
            }
            self.fb.switch_to_block(done);
            return;
        }
        // A union: the member it holds, with its own `Display`.
        if let Some(members) = types.members(types.repr(ty)).map(<[Ty]>::to_vec) {
            let tag = self.tag(ty, value);
            let done = self.fb.create_block();
            for (variant, &member) in members.iter().enumerate() {
                let (this, next) = (self.fb.create_block(), self.fb.create_block());
                let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                self.fb.ins().brif(is_this, this, &[], next, &[]);
                self.fb.switch_to_block(this);
                let payload = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Variant(variant as u32, 0));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, member)
                    }
                    _ => Val::Zero,
                };
                self.append_display(builder, member, payload);
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(next);
            }
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(done);
            return;
        }
        // A tuple: `(a, b)`, each part with its own `Display`.
        if let TyKind::Tuple(items) = types.kind(types.repr(ty)).clone() {
            let open = self.string_ptr("(");
            self.call_rt(Rt::SbStr, &[builder, open]);
            for (index, &item) in items.iter().enumerate() {
                if index > 0 {
                    let comma = self.string_ptr(", ");
                    self.call_rt(Rt::SbStr, &[builder, comma]);
                }
                let part = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Field(index as u32));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, item)
                    }
                    _ => Val::Zero,
                };
                self.append_display(builder, item, part);
            }
            let close = self.string_ptr(")");
            self.call_rt(Rt::SbStr, &[builder, close]);
            return;
        }
        if let TyKind::Option(inner) = *types.kind(types.repr(ty)) {
            let tag = self.tag(ty, value);
            let (some, none, done) = (self.fb.create_block(), self.fb.create_block(), self.fb.create_block());
            let is_some = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, SOME as i64);
            self.fb.ins().brif(is_some, some, &[], none, &[]);
            self.fb.switch_to_block(some);
            let open = self.string_ptr("some(");
            self.call_rt(Rt::SbStr, &[builder, open]);
            let payload = match value {
                Val::Memory(address) => {
                    let slot = self.cg.slot(ty, Proj::Variant(SOME, 0));
                    let address = self.component_address(address, &slot);
                    self.value_at(address, inner)
                }
                _ => Val::Zero,
            };
            self.append_display(builder, inner, payload);
            let close = self.string_ptr(")");
            self.call_rt(Rt::SbStr, &[builder, close]);
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(none);
            let text = self.string_ptr("none");
            self.call_rt(Rt::SbStr, &[builder, text]);
            self.fb.ins().jump(done, &[]);
            self.fb.switch_to_block(done);
            return;
        }
        match types.shown.get(&ty) {
            Some(Shown::Duration) => {
                let Val::Scalar(nanos) = value else { unreachable!("a duration is a scalar") };
                self.call_rt(Rt::SbDuration, &[builder, nanos]);
                return;
            }
            Some(Shown::ProcessError) => {
                // The code of `Failed`, the variant with a payload; other
                // variants ignore it.
                let tag = self.tag(ty, value);
                let tag = self.fb.ins().uextend(types::I64, tag);
                let Val::Memory(address) = value else { unreachable!("an error with payloads is in memory") };
                let payload = self.cg.slot(ty, Proj::Variant(3, 0));
                let status = self.component_address(address, &payload);
                let code_slot = self.cg.slot(payload.ty, Proj::Field(0));
                let code = self.fb.ins().load(types::I64, flags(), status, code_slot.offset as i32);
                self.call_rt(Rt::SbProcessError, &[builder, tag, code]);
                return;
            }
            Some(Shown::DateTime) => {
                let Val::Memory(address) = value else { unreachable!("a date is in memory") };
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 56, 3);
                let slot = self.fb.create_sized_stack_slot(data);
                let fields = self.fb.ins().stack_addr(POINTER, slot, 0);
                for index in 0..7 {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    let word = self.fb.ins().load(types::I64, flags(), address, slot.offset as i32);
                    self.fb.ins().store(flags(), word, fields, index as i32 * 8);
                }
                self.call_rt(Rt::SbDateTime, &[builder, fields]);
                return;
            }
            _ => {}
        }
        if let TyKind::Adt(id) = *types.kind(types.repr(ty))
            && let AdtKind::Enum(variants) = &types.adt(id).kind
        {
            // A standard library error: its variant name in words.
            let tag = self.tag(ty, value);
            let mut text = self.string_ptr(&words(&variants[0].name));
            for (index, variant) in variants.iter().enumerate().skip(1) {
                let this = self.string_ptr(&words(&variant.name));
                let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
                text = self.fb.ins().select(is_this, this, text);
            }
            self.call_rt(Rt::SbStr, &[builder, text]);
            return;
        }
        let Val::Scalar(value) = value else { unreachable!("no Display for aggregates") };
        match *types.kind(types.repr(ty)) {
            TyKind::String => {
                self.call_rt(Rt::SbStr, &[builder, value]);
            }
            TyKind::Bool => {
                self.call_rt(Rt::SbBool, &[builder, value]);
            }
            TyKind::Float(FloatTy::F64) => {
                self.call_rt(Rt::SbF64, &[builder, value]);
            }
            TyKind::Float(FloatTy::F32) => {
                self.call_rt(Rt::SbF32, &[builder, value]);
            }
            TyKind::Char => {
                self.call_rt(Rt::SbChar, &[builder, value]);
            }
            TyKind::Byte => {
                let wide = self.fb.ins().uextend(types::I64, value);
                self.call_rt(Rt::SbU64, &[builder, wide]);
            }
            TyKind::Int(int) if int.bits() == 128 => {
                let (low, high) = self.fb.ins().isplit(value);
                let signed = self.fb.ins().iconst(types::I8, int.signed() as i64);
                self.call_rt(Rt::SbInt128, &[builder, low, high, signed]);
            }
            TyKind::Int(int) => {
                let wide = match (int.bits(), int.signed()) {
                    (64, _) => value,
                    (_, true) => self.fb.ins().sextend(types::I64, value),
                    (_, false) => self.fb.ins().uextend(types::I64, value),
                };
                let rt = if int.signed() { Rt::SbI64 } else { Rt::SbU64 };
                self.call_rt(rt, &[builder, wide]);
            }
            TyKind::Array(element) | TyKind::Deque(element) => {
                let size = self.element_size(element);
                let display = self.glue_address(Glue::Display, element);
                self.call_rt(Rt::SbArray, &[builder, value, size, display]);
            }
            TyKind::Map(key, entry) => {
                let key = self.glue_address(Glue::Display, key);
                let entry = self.glue_address(Glue::Display, entry);
                self.call_rt(Rt::SbMap, &[builder, value, key, entry]);
            }
            TyKind::Set(element) => {
                let element = self.glue_address(Glue::Display, element);
                let none = self.fb.ins().iconst(POINTER, 0);
                self.call_rt(Rt::SbMap, &[builder, value, element, none]);
            }
            ref other => unreachable!("no Display for {other:?}"),
        }
    }

    /// Appends a value as a debugger shows it: its structure, whatever
    /// `Display` it has, with texts in quotes. `address` is where it is.
    /// Components that are not scalars go through their own glue, so a
    /// recursive type does not expand forever.
    fn append_show(&mut self, builder: ir::Value, ty: Ty, value: Val, address: ir::Value) {
        let types = self.cg.types();
        let repr = types.repr(ty);
        let text = |e: &mut Self, text: &str| {
            let text = e.string_ptr(text);
            e.call_rt(Rt::SbStr, &[builder, text]);
        };
        // A component: scalars here, the rest by its glue.
        let part = |e: &mut Self, ty: Ty, slot: &Slot| {
            let at = e.component_address(address, slot);
            match e.cg.class(slot.ty) {
                Class::Zero => text(e, "()"),
                _ => {
                    e.call_glue(Glue::Show, slot.ty, &[builder, at]);
                }
            }
            let _ = ty;
        };
        match types.kind(repr).clone() {
            TyKind::String => {
                let Val::Scalar(value) = value else { unreachable!("a string is a pointer") };
                self.call_rt(Rt::SbQuoted, &[builder, value]);
            }
            TyKind::Char => {
                text(self, "'");
                self.append_display(builder, ty, value);
                text(self, "'");
            }
            TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Byte => self.append_display(builder, ty, value),
            TyKind::Unit => text(self, "()"),
            TyKind::Tuple(items) => {
                text(self, "(");
                for index in 0..items.len() {
                    if index > 0 {
                        text(self, ", ");
                    }
                    let slot = self.cg.slot(ty, Proj::Field(index as u32));
                    part(self, ty, &slot);
                }
                text(self, ")");
            }
            TyKind::Array(element) | TyKind::Deque(element) => {
                let Val::Scalar(value) = value else { unreachable!("a sequence is a pointer") };
                let size = self.element_size(element);
                let show = self.glue_address(Glue::Show, element);
                self.call_rt(Rt::SbArray, &[builder, value, size, show]);
            }
            TyKind::Map(key, entry) => {
                let Val::Scalar(value) = value else { unreachable!("a map is a pointer") };
                let (key, entry) = (self.glue_address(Glue::Show, key), self.glue_address(Glue::Show, entry));
                self.call_rt(Rt::SbMap, &[builder, value, key, entry]);
            }
            TyKind::Set(element) => {
                let Val::Scalar(value) = value else { unreachable!("a set is a pointer") };
                let element = self.glue_address(Glue::Show, element);
                let none = self.fb.ins().iconst(POINTER, 0);
                self.call_rt(Rt::SbMap, &[builder, value, element, none]);
            }
            TyKind::Option(_) | TyKind::Result(..) | TyKind::Union(_) | TyKind::Adt(_)
                if !matches!(value, Val::Zero) || types.variants(ty).is_some() =>
            {
                // Each variant by its name, then its payload.
                let names: Vec<String> = match types.kind(repr) {
                    TyKind::Option(_) => vec!["none".into(), "some".into()],
                    TyKind::Result(..) => vec!["ok".into(), "err".into()],
                    TyKind::Union(members) => members.iter().map(|&m| format!("{}: ", types.display(m))).collect(),
                    TyKind::Adt(id) => match &types.adt(*id).kind {
                        AdtKind::Enum(variants) => {
                            variants.iter().map(|v| format!("{}.{}", types.adt(*id).name, v.name)).collect()
                        }
                        AdtKind::Record(fields) => {
                            text(self, &format!("{} {{ ", types.adt(*id).name));
                            for (index, field) in fields.iter().enumerate() {
                                text(self, &format!("{}{}: ", if index > 0 { ", " } else { "" }, field.name));
                                let slot = self.cg.slot(ty, Proj::Field(index as u32));
                                part(self, ty, &slot);
                            }
                            text(self, " }");
                            return;
                        }
                        AdtKind::Newtype(_) => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                let Some(variants) = types.variants(ty) else {
                    text(self, &types.display(ty));
                    return;
                };
                let tag = self.tag(ty, value);
                let done = self.fb.create_block();
                let union = matches!(types.kind(repr), TyKind::Union(_));
                for (variant, components) in variants.iter().enumerate() {
                    let (this, next) = (self.fb.create_block(), self.fb.create_block());
                    let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                    self.fb.ins().brif(is_this, this, &[], next, &[]);
                    self.fb.switch_to_block(this);
                    text(self, &names[variant]);
                    if !components.is_empty() {
                        if !union {
                            text(self, "(");
                        }
                        for index in 0..components.len() {
                            if index > 0 {
                                text(self, ", ");
                            }
                            let slot = self.cg.slot(ty, Proj::Variant(variant as u32, index as u32));
                            part(self, ty, &slot);
                        }
                        if !union {
                            text(self, ")");
                        }
                    }
                    self.fb.ins().jump(done, &[]);
                    self.fb.switch_to_block(next);
                }
                self.fb.ins().jump(done, &[]);
                self.fb.switch_to_block(done);
            }
            _ => text(self, &types.display(ty)),
        }
    }

    fn string_ptr(&mut self, text: &str) -> ir::Value {
        let id = self.cg.string(text);
        let global = self.cg.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
    }

    fn offset(&mut self, base: ir::Value, offset: u32) -> ir::Value {
        if offset == 0 { base } else { self.fb.ins().iadd_imm_s(base, offset as i64) }
    }

    /// The value of type `ty` stored at `address`.
    fn value_at(&mut self, address: ir::Value, ty: Ty) -> Val {
        match self.cg.class(ty) {
            Class::Zero => Val::Zero,
            Class::Scalar(t) => Val::Scalar(self.fb.ins().load(t, flags(), address, 0)),
            Class::Memory => Val::Memory(address),
        }
    }

    /// Where the component lives: inside the container, or inside its box.
    fn component_address(&mut self, base: ir::Value, slot: &Slot) -> ir::Value {
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            self.fb.ins().iadd_imm_s(cell, BOX_HEADER)
        } else {
            self.offset(base, slot.offset)
        }
    }

    fn store_val(&mut self, address: ir::Value, ty: Ty, value: Val) {
        match value {
            Val::Zero => {}
            Val::Scalar(value) => {
                self.fb.ins().store(flags(), value, address, 0);
            }
            Val::Memory(source) => {
                let size = self.cg.layout(ty).size;
                self.copy_memory(address, source, size);
            }
        }
    }

    /// Writes a component into a container under construction, allocating a
    /// box for boxed components.
    fn init_component(&mut self, base: ir::Value, slot: &Slot, value: Val) {
        if slot.boxed {
            let size = self.fb.ins().iconst(POINTER, self.cg.layout(slot.ty).size as i64);
            let cell = self.call_rt(Rt::BoxNew, &[size]).unwrap();
            let inner = self.fb.ins().iadd_imm_s(cell, BOX_HEADER);
            self.store_val(inner, slot.ty, value);
            self.fb.ins().store(flags(), cell, base, slot.offset as i32);
        } else {
            let address = self.offset(base, slot.offset);
            self.store_val(address, slot.ty, value);
        }
    }

    /// Where a reference's value is: after its header, or in a live build,
    /// which keeps it apart, where the header says.
    fn ref_value(&mut self, reference: ir::Value) -> ir::Value {
        match self.cg.live() {
            true => self.fb.ins().load(POINTER, flags(), reference, REF_HEADER as i32),
            false => self.fb.ins().iadd_imm_s(reference, REF_HEADER),
        }
    }

    fn copy_memory(&mut self, destination: ir::Value, source: ir::Value, size: u32) {
        if size > 64 {
            let config = self.cg.isa().frontend_config();
            let size = self.fb.ins().iconst(POINTER, size as i64);
            self.fb.call_memcpy(config, destination, source, size);
            return;
        }
        let mut offset = 0;
        for (bytes, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while size - offset >= bytes {
                let value = self.fb.ins().load(ty, flags(), source, offset as i32);
                self.fb.ins().store(flags(), value, destination, offset as i32);
                offset += bytes;
            }
        }
    }

    fn tag(&mut self, ty: Ty, value: Val) -> ir::Value {
        if let Shape::Niche = self.cg.layout(ty).shape {
            let address = match value {
                Val::Scalar(address) => address,
                Val::Memory(at) => self.fb.ins().load(POINTER, flags(), at, 0),
                Val::Zero => unreachable!("an address is not zero-sized"),
            };
            return self.fb.ins().icmp_imm_u(IntCC::NotEqual, address, 0);
        }
        match value {
            Val::Scalar(tag) => tag,
            Val::Memory(address) => {
                let tag = self.cg.scalar_type(self.cg.types().tag_ty(ty));
                self.fb.ins().load(tag, flags(), address, 0)
            }
            Val::Zero => self.fb.ins().iconst(types::I8, 0),
        }
    }

    /// The name of the type of a value: of the member a union holds.
    fn type_name(&mut self, ty: Ty, value: Val) -> ir::Value {
        let types = self.cg.types();
        let Some(members) = types.members(types.repr(ty)).map(<[Ty]>::to_vec) else {
            let name = types.display(ty);
            return self.string_ptr(&name);
        };
        let tag = self.tag(ty, value);
        let done = self.fb.create_block();
        self.fb.append_block_param(done, POINTER);
        for (variant, &member) in members.iter().enumerate() {
            let (this, next) = (self.fb.create_block(), self.fb.create_block());
            let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
            self.fb.ins().brif(is_this, this, &[], next, &[]);
            self.fb.switch_to_block(this);
            let name = self.cg.types().display(member);
            let name = self.string_ptr(&name);
            self.fb.ins().jump(done, &[name.into()]);
            self.fb.switch_to_block(next);
        }
        let name = self.cg.types().display(ty);
        let fallback = self.string_ptr(&name);
        self.fb.ins().jump(done, &[fallback.into()]);
        self.fb.switch_to_block(done);
        self.fb.block_params(done)[0]
    }

    /// A string describing an error that reached the end of `main`: its text
    /// for a `String`, `Type.Variant` for an enum, otherwise the type name.
    /// With `display`, a type's own `Display` comes first, and the string
    /// is owned.
    fn describe(&mut self, ty: Ty, value: Val, display: bool) -> ir::Value {
        if display && let Some(&id) = self.cg.display().get(&ty) {
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let call = call_body(self.cg, &mut self.fb, id, &args);
            return self.fb.inst_results(call)[0];
        }
        let types = self.cg.types();
        let repr = types.repr(ty);
        if repr == Ty::STRING
            && let Val::Scalar(text) = value
        {
            if display {
                self.call_rt(Rt::StrRetain, &[text]);
            }
            return text;
        }
        if let Some(members) = types.members(repr) {
            // Describe the member the union holds.
            let tag = self.tag(ty, value);
            let done = self.fb.create_block();
            self.fb.append_block_param(done, POINTER);
            for (variant, &member) in members.iter().enumerate() {
                let (this, next) = (self.fb.create_block(), self.fb.create_block());
                let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                self.fb.ins().brif(is_this, this, &[], next, &[]);
                self.fb.switch_to_block(this);
                let payload = match value {
                    Val::Memory(address) => {
                        let slot = self.cg.slot(ty, Proj::Variant(variant as u32, 0));
                        let address = self.component_address(address, &slot);
                        self.value_at(address, member)
                    }
                    _ => Val::Zero,
                };
                let description = self.describe(member, payload, display);
                self.fb.ins().jump(done, &[description.into()]);
                self.fb.switch_to_block(next);
            }
            let fallback = self.string_ptr(&types.display(ty));
            self.fb.ins().jump(done, &[fallback.into()]);
            self.fb.switch_to_block(done);
            return self.fb.block_params(done)[0];
        }
        let names: Vec<String> = match types.kind(repr) {
            TyKind::Adt(id) => match &types.adt(*id).kind {
                AdtKind::Enum(variants) => {
                    variants.iter().map(|v| format!("{}.{}", types.adt(*id).name, v.name)).collect()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        if names.is_empty() || matches!(value, Val::Zero) {
            return self.string_ptr(&types.display(ty));
        }
        let tag = self.tag(ty, value);
        let mut description = self.string_ptr(&names[0]);
        for (index, name) in names.iter().enumerate().skip(1) {
            let text = self.string_ptr(name);
            let is_this = self.fb.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
            description = self.fb.ins().select(is_this, text, description);
        }
        description
    }

    /// Retains or releases every counted value inside `value`.
    fn refcount(&mut self, ty: Ty, value: Val, release: bool) {
        if !self.cg.types().needs_drop(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => match *self.cg.types().kind(self.cg.types().repr(ty)) {
                TyKind::Array(element) | TyKind::Deque(element) if release => {
                    let size = self.element_size(element);
                    let glue = self.glue_or_null(Glue::Release, element);
                    self.call_rt(Rt::ArrayRelease, &[pointer, size, glue]);
                }
                TyKind::Array(_) | TyKind::Deque(_) => {
                    self.call_rt(Rt::ArrayRetain, &[pointer]);
                }
                TyKind::Map(..) | TyKind::Set(_) if release => {
                    let glue = self.map_glue_address(ty);
                    self.call_rt(Rt::MapRelease, &[pointer, glue]);
                }
                TyKind::Map(..) | TyKind::Set(_) => {
                    self.call_rt(Rt::MapRetain, &[pointer]);
                }
                TyKind::Fn(..) => {
                    self.call_rt(if release { Rt::FnRelease } else { Rt::FnRetain }, &[pointer]);
                }
                TyKind::Handle(Handle::Ref, value) if release => {
                    let glue = self.glue_or_null(Glue::Release, value);
                    self.call_rt(Rt::RefRelease, &[pointer, glue]);
                }
                TyKind::Handle(Handle::Weak, _) => {
                    self.call_rt(if release { Rt::WeakRelease } else { Rt::WeakRetain }, &[pointer]);
                }
                TyKind::Handle(handle, _) if release => {
                    let rt = match handle {
                        Handle::Join => Rt::JoinRelease,
                        Handle::Group => Rt::GroupRelease,
                        Handle::Sender => Rt::SenderRelease,
                        Handle::Receiver => Rt::ReceiverRelease,
                        Handle::Semaphore => Rt::SemaphoreRelease,
                        Handle::Permit => Rt::PermitRelease,
                        Handle::Barrier => Rt::BarrierRelease,
                        Handle::Guard => Rt::SyncUnlock,
                        Handle::SyncMap | Handle::SyncSet => Rt::SharedMapRelease,
                        Handle::SyncQueue => Rt::SharedQueueRelease,
                        Handle::SyncStack => Rt::SharedStackRelease,
                        Handle::SyncArray => Rt::SharedArrayRelease,
                        Handle::File | Handle::Writer | Handle::Reader => Rt::StreamRelease,
                        Handle::Buffer => Rt::BufferRelease,
                        Handle::Command | Handle::Pipeline => Rt::PlanRelease,
                        Handle::Process => Rt::ProcessRelease,
                        Handle::JsonReader => Rt::JsonReaderRelease,
                        Handle::JsonWriter => Rt::JsonWriterRelease,
                        Handle::Clock => Rt::ClockRelease,
                        Handle::Regex => Rt::RegexRelease,
                        Handle::Native => Rt::NativeRelease,
                        Handle::ReflectBox => Rt::ReflectRelease,
                        _ => Rt::SyncRelease,
                    };
                    self.call_rt(rt, &[pointer]);
                }
                // A guard is held once and never copied.
                TyKind::Handle(Handle::Guard, _) => {}
                TyKind::Handle(..) => {
                    self.call_rt(Rt::HandleRetain, &[pointer]);
                }
                _ => {
                    self.call_rt(if release { Rt::StrRelease } else { Rt::StrRetain }, &[pointer]);
                }
            },
            (Val::Memory(base), Shape::Struct(_)) => {
                let count = self.cg.types().components(ty).unwrap().len() as u32;
                for index in 0..count {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    self.refcount_slot(base, &slot, release);
                }
            }
            (Val::Memory(address), Shape::Enum { .. }) => {
                let kind = if release { Glue::Release } else { Glue::Retain };
                self.call_glue(kind, ty, &[address]);
            }
            _ => {}
        }
    }

    fn refcount_slot(&mut self, base: ir::Value, slot: &Slot, release: bool) {
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            if release {
                let glue = self.glue_address(Glue::Release, slot.ty);
                self.call_rt(Rt::BoxRelease, &[cell, glue]);
            } else {
                self.call_rt(Rt::BoxRetain, &[cell]);
            }
        } else if self.cg.types().needs_drop(slot.ty) {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.refcount(slot.ty, value, release);
        }
    }

    /// Reports to the cycle collector's `visitor` the references inside
    /// `value` that may lead to a cycle (see `Types::can_cycle`).
    fn visit(&mut self, ty: Ty, value: Val, visitor: ir::Value) {
        if !self.cg.types().can_cycle(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => {
                let zero = self.fb.ins().iconst(POINTER, 0);
                // The kinds of `cycles::Kind` in the runtime.
                let (kind, a, b) = match *self.cg.types().kind(self.cg.types().repr(ty)) {
                    TyKind::Array(element) | TyKind::Deque(element) => {
                        (2, self.element_size(element), self.glue_or_null(Glue::Visit, element))
                    }
                    TyKind::Map(..) | TyKind::Set(_) => {
                        let (key, value) = self.cg.map_parts(ty);
                        (3, self.glue_or_null(Glue::Visit, key), self.glue_or_null(Glue::Visit, value))
                    }
                    TyKind::Fn(..) => (4, zero, zero),
                    TyKind::Handle(Handle::Ref, inner) => (1, self.glue_or_null(Glue::Visit, inner), zero),
                    TyKind::Handle(handle, _) => {
                        let kind = match handle {
                            Handle::Mutex | Handle::RwLock | Handle::Atomic | Handle::Once => 5,
                            Handle::Receiver => 6,
                            Handle::Sender => 7,
                            Handle::Join => 8,
                            Handle::Group => 9,
                            Handle::SyncMap | Handle::SyncSet => 10,
                            Handle::SyncQueue => 11,
                            Handle::SyncStack => 12,
                            Handle::SyncArray => 13,
                            Handle::ReflectBox => 14,
                            _ => return,
                        };
                        (kind, zero, zero)
                    }
                    _ => return,
                };
                let kind = self.fb.ins().iconst(types::I8, kind);
                self.call_rt(Rt::GcEdge, &[visitor, pointer, kind, a, b]);
            }
            (Val::Memory(base), Shape::Struct(_)) => {
                let count = self.cg.types().components(ty).unwrap().len() as u32;
                for index in 0..count {
                    let slot = self.cg.slot(ty, Proj::Field(index));
                    self.visit_slot(base, &slot, visitor);
                }
            }
            (Val::Memory(address), Shape::Enum { .. }) => {
                self.call_glue(Glue::Visit, ty, &[address, visitor]);
            }
            _ => {}
        }
    }

    fn visit_slot(&mut self, base: ir::Value, slot: &Slot, visitor: ir::Value) {
        if !self.cg.types().can_cycle(slot.ty) {
            return;
        }
        if slot.boxed {
            let cell = self.fb.ins().load(POINTER, flags(), base, slot.offset as i32);
            let glue = self.glue_address(Glue::Visit, slot.ty);
            let kind = self.fb.ins().iconst(types::I8, 0);
            let zero = self.fb.ins().iconst(POINTER, 0);
            self.call_rt(Rt::GcEdge, &[visitor, cell, kind, glue, zero]);
        } else {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.visit(slot.ty, value, visitor);
        }
    }

    /// Structural equality (8.11): 1 when equal, 0 otherwise.
    fn equal(&mut self, ty: Ty, a: Val, b: Val) -> ir::Value {
        match (a, b) {
            (Val::Scalar(a), Val::Scalar(b)) => match *self.cg.types().kind(self.cg.types().repr(ty)) {
                TyKind::String => self.call_rt(Rt::StrEq, &[a, b]).unwrap(),
                TyKind::Array(Ty::BYTE) => self.call_rt(Rt::BytesEq, &[a, b]).unwrap(),
                TyKind::Array(element) | TyKind::Deque(element) => {
                    let size = self.element_size(element);
                    let eq = self.glue_address(Glue::Equal, element);
                    self.call_rt(Rt::ArrayEq, &[a, b, size, eq]).unwrap()
                }
                TyKind::Map(_, value) => {
                    let glue = self.map_glue_address(ty);
                    let value_eq = self.glue_address(Glue::Equal, value);
                    self.call_rt(Rt::MapEq, &[a, b, glue, value_eq]).unwrap()
                }
                TyKind::Set(_) => {
                    let glue = self.map_glue_address(ty);
                    let none = self.fb.ins().iconst(POINTER, 0);
                    self.call_rt(Rt::MapEq, &[a, b, glue, none]).unwrap()
                }
                // IEEE equality: NaN equals nothing (8.11).
                TyKind::Float(_) => self.fb.ins().fcmp(FloatCC::Equal, a, b),
                _ => self.fb.ins().icmp(IntCC::Equal, a, b),
            },
            (Val::Memory(a), Val::Memory(b)) => match self.cg.layout(ty).shape {
                Shape::Struct(_) => {
                    let count = self.cg.types().components(ty).unwrap().len() as u32;
                    let mut result = self.fb.ins().iconst(types::I8, 1);
                    for index in 0..count {
                        let slot = self.cg.slot(ty, Proj::Field(index));
                        let same = self.equal_slot(a, b, &slot);
                        result = self.fb.ins().band(result, same);
                    }
                    result
                }
                _ => self.call_glue(Glue::Equal, ty, &[a, b]).unwrap(),
            },
            _ => self.fb.ins().iconst(types::I8, 1),
        }
    }

    fn equal_slot(&mut self, a: ir::Value, b: ir::Value, slot: &Slot) -> ir::Value {
        let left = self.component_address(a, slot);
        let right = self.component_address(b, slot);
        if slot.boxed {
            return self.call_glue(Glue::Equal, slot.ty, &[left, right]).unwrap();
        }
        let left = self.value_at(left, slot.ty);
        let right = self.value_at(right, slot.ty);
        self.equal(slot.ty, left, right)
    }
}

/// Defines a glue function: retain, release or compare values of `ty` given
/// their addresses.
fn define_glue(cg: &mut Codegen, kind: Glue, ty: Ty, func: &mut ir::Function, fbc: &mut FunctionBuilderContext) {
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit { cg, fb };
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let params = e.fb.block_params(entry).to_vec();
    let enum_shape = match e.cg.layout(ty).shape {
        Shape::Enum { payload, variants, .. } => Some((payload, variants.len() as u32)),
        _ => None,
    };
    match (kind, enum_shape) {
        (Glue::Test, _) => {
            let TyKind::Fn(_, result, ..) = *e.cg.types.kind(ty) else { unreachable!("a test member is a `fn()`") };
            let code = e.fb.ins().load(POINTER, flags(), params[0], 16);
            let captures = e.fb.ins().iadd_imm_s(params[0], FN_HEADER);
            let signature = e.cg.uniform_signature(&[], &[], result);
            let signature = e.fb.import_signature(signature);
            let value = match e.cg.class(result) {
                Class::Memory => {
                    let layout = e.cg.layout(result);
                    let data = StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        layout.size,
                        layout.align.trailing_zeros() as u8,
                    );
                    let slot = e.fb.create_sized_stack_slot(data);
                    let address = e.fb.ins().stack_addr(POINTER, slot, 0);
                    e.fb.ins().call_indirect(signature, code, &[address, captures]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = e.fb.ins().call_indirect(signature, code, &[captures]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    e.fb.ins().call_indirect(signature, code, &[captures]);
                    Val::Zero
                }
            };
            if let TyKind::Result(_, error) = *e.cg.types.kind(result) {
                let tag = e.tag(result, value);
                let failed = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, ERR as i64);
                let (report, done) = (e.fb.create_block(), e.fb.create_block());
                e.fb.ins().brif(failed, report, &[], done, &[]);
                e.fb.switch_to_block(report);
                let error_value = match value {
                    Val::Memory(address) => {
                        let slot = e.cg.slot(result, Proj::Variant(ERR, 0));
                        let address = e.component_address(address, &slot);
                        e.value_at(address, error)
                    }
                    _ => Val::Zero,
                };
                let description = e.describe(error, error_value, false);
                let error_type = e.cg.types.display(error);
                let error_type = e.string_ptr(&error_type);
                e.call_rt(Rt::TestError, &[description, error_type]);
                e.fb.ins().jump(done, &[]);
                e.fb.switch_to_block(done);
            }
            e.refcount(result, value, true);
            e.fb.ins().return_(&[]);
        }
        (Glue::Display, _) => {
            let value = e.value_at(params[1], ty);
            e.append_display(params[0], ty, value);
            e.fb.ins().return_(&[]);
        }
        (Glue::Show, _) => {
            let value = e.value_at(params[1], ty);
            e.append_show(params[0], ty, value, params[1]);
            e.fb.ins().return_(&[]);
        }
        (Glue::Hash, None) => {
            let value = e.value_at(params[0], ty);
            let hash = e.hash(ty, value);
            e.fb.ins().return_(&[hash]);
        }
        (Glue::Hash, Some((_, count))) => {
            // The tag, then the active variant's payload.
            let tag = e.tag(ty, Val::Memory(params[0]));
            let tag_word = e.fb.ins().uextend(types::I64, tag);
            let seed = e.fb.ins().iconst(types::I64, 0xcbf2_9ce4_8422_2325u64 as i64);
            let start = e.mix(seed, tag_word);
            for variant in 0..count {
                let components = e.cg.types.variants(ty).unwrap()[variant as usize].len() as u32;
                let (this, next) = (e.fb.create_block(), e.fb.create_block());
                let is_this = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                e.fb.ins().brif(is_this, this, &[], next, &[]);
                e.fb.switch_to_block(this);
                let mut hash = start;
                for index in 0..components {
                    let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                    let part = e.component_address(params[0], &slot);
                    let part = if slot.boxed {
                        e.call_glue(Glue::Hash, slot.ty, &[part]).unwrap()
                    } else {
                        let value = e.value_at(part, slot.ty);
                        e.hash(slot.ty, value)
                    };
                    hash = e.mix(hash, part);
                }
                e.fb.ins().return_(&[hash]);
                e.fb.switch_to_block(next);
            }
            e.fb.ins().return_(&[start]);
        }
        (Glue::Retain | Glue::Release, None) => {
            let value = e.value_at(params[0], ty);
            e.refcount(ty, value, kind == Glue::Release);
            e.fb.ins().return_(&[]);
        }
        (Glue::Visit, None) => {
            let value = e.value_at(params[0], ty);
            e.visit(ty, value, params[1]);
            e.fb.ins().return_(&[]);
        }
        (Glue::Equal, None) => {
            let (a, b) = (e.value_at(params[0], ty), e.value_at(params[1], ty));
            let result = e.equal(ty, a, b);
            e.fb.ins().return_(&[result]);
        }
        (_, Some((_, count))) => {
            // Dispatch on the tag; only the active variant's payload is valid.
            let tag = e.tag(ty, Val::Memory(params[0]));
            if kind == Glue::Equal {
                let other = e.tag(ty, Val::Memory(params[1]));
                let differs = e.fb.ins().icmp(IntCC::NotEqual, tag, other);
                let (different, same) = (e.fb.create_block(), e.fb.create_block());
                e.fb.ins().brif(differs, different, &[], same, &[]);
                e.fb.switch_to_block(different);
                let zero = e.fb.ins().iconst(types::I8, 0);
                e.fb.ins().return_(&[zero]);
                e.fb.switch_to_block(same);
            }
            for variant in 0..count {
                let components = e.cg.types.variants(ty).unwrap()[variant as usize].len() as u32;
                let (this, next) = (e.fb.create_block(), e.fb.create_block());
                let is_this = e.fb.ins().icmp_imm_s(IntCC::Equal, tag, variant as i64);
                e.fb.ins().brif(is_this, this, &[], next, &[]);
                e.fb.switch_to_block(this);
                if kind == Glue::Equal {
                    let mut result = e.fb.ins().iconst(types::I8, 1);
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        let same = e.equal_slot(params[0], params[1], &slot);
                        result = e.fb.ins().band(result, same);
                    }
                    e.fb.ins().return_(&[result]);
                } else if kind == Glue::Visit {
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        e.visit_slot(params[0], &slot, params[1]);
                    }
                    e.fb.ins().return_(&[]);
                } else {
                    for index in 0..components {
                        let slot = e.cg.slot(ty, Proj::Variant(variant, index));
                        e.refcount_slot(params[0], &slot, kind == Glue::Release);
                    }
                    e.fb.ins().return_(&[]);
                }
                e.fb.switch_to_block(next);
            }
            if kind == Glue::Equal {
                let one = e.fb.ins().iconst(types::I8, 1);
                e.fb.ins().return_(&[one]);
            } else {
                e.fb.ins().return_(&[]);
            }
        }
    }
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}

/// What a live build's closure calls: its body, through the body's slot,
/// with the same arguments.
fn define_forwarder(cg: &mut Codegen, target: FnId, func: &mut ir::Function, fbc: &mut FunctionBuilderContext) {
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let params = fb.block_params(entry).to_vec();
    let results = call_passing(cg, &mut fb, target, &params);
    fb.ins().return_(&results);
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
}

/// A call that code with no cleanups of its own makes for another, as a
/// thunk does: a `try_call`, whose handler passes a panic the callee unwinds
/// on to the caller's way (15.9), with zeros for the results.
fn call_passing(cg: &mut Codegen, fb: &mut FunctionBuilder, target: FnId, args: &[ir::Value]) -> Vec<ir::Value> {
    use cranelift_codegen::ir::{BlockArg, BlockCall, ExceptionTableData, ExceptionTableItem};
    if cg.live() || cg.takes_osr(target) || cg.slot_of(target).is_some() {
        let call = call_body(cg, fb, target, args);
        return fb.inst_results(call).to_vec();
    }
    let func = cg.function(target);
    let callee = cg.declare_func_in_func(func, fb.func);
    let signature = fb.func.dfg.ext_funcs[callee].signature;
    let returns: Vec<ir::Type> = fb.func.dfg.signatures[signature].returns.iter().map(|r| r.value_type).collect();
    let (next, handler) = (fb.create_block(), fb.create_block());
    for &ty in &returns {
        fb.append_block_param(next, ty);
    }
    let pool = &mut fb.func.dfg.value_lists;
    let normal = BlockCall::new(next, (0..returns.len() as u32).map(BlockArg::TryCallRet), pool);
    let caught = BlockCall::new(handler, [], pool);
    let table = ExceptionTableData::new(signature, normal, [ExceptionTableItem::Default(caught)]);
    let table = fb.func.dfg.exception_tables.push(table);
    fb.ins().try_call(callee, args, table);
    fb.switch_to_block(handler);
    fb.set_cold_block(handler);
    let pass = cg.runtime(Rt::UnwindPass);
    let pass = cg.declare_func_in_func(pass, fb.func);
    fb.ins().call(pass, &[]);
    let zeros: Vec<ir::Value> = returns
        .iter()
        .map(|&ty| match ty {
            types::F32 => fb.ins().f32const(0.0),
            types::F64 => fb.ins().f64const(0.0),
            types::I128 => {
                let half = fb.ins().iconst(types::I64, 0);
                fb.ins().iconcat(half, half)
            }
            ty => fb.ins().iconst(ty, 0),
        })
        .collect();
    fb.ins().return_(&zeros);
    fb.switch_to_block(next);
    fb.block_params(next).to_vec()
}

/// What the runtime calls to convert a value a frame brings to the new code
/// (toolchain 7.10): the conversion `function`, given the value at the
/// first address, writes what it gives at the second when that is `some`;
/// the answer is whether it was.
fn define_conversion(
    cg: &mut Codegen,
    body: &Body,
    function: FnId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let fb = FunctionBuilder::new(func, fbc);
    let mut e = Emit { cg, fb };
    let entry = e.fb.create_block();
    e.fb.append_block_params_for_function_params(entry);
    e.fb.switch_to_block(entry);
    let (from, to) = (e.fb.block_params(entry)[0], e.fb.block_params(entry)[1]);
    let option = body.result;
    let TyKind::Option(new) = *e.cg.types.kind(option) else { unreachable!("a conversion gives `T?`") };
    let old = body.locals[body.params[0].0 as usize];
    let mut values = Vec::new();
    let result = (e.cg.class(option) == Class::Memory).then(|| {
        let layout = e.cg.layout(option);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size, layout.align.trailing_zeros() as u8);
        let slot = e.fb.create_sized_stack_slot(data);
        e.fb.ins().stack_addr(POINTER, slot, 0)
    });
    values.extend(result);
    match e.value_at(from, old) {
        Val::Scalar(value) | Val::Memory(value) => values.push(value),
        Val::Zero => {}
    }
    let call = call_body(e.cg, &mut e.fb, function, &values);
    let value = match result {
        Some(address) => Val::Memory(address),
        None => Val::Scalar(e.fb.inst_results(call)[0]),
    };
    let tag = e.tag(option, value);
    let (some, none) = (e.fb.create_block(), e.fb.create_block());
    e.fb.ins().brif(tag, some, &[], none, &[]);
    e.fb.switch_to_block(some);
    match value {
        // `T?` of an address is the address.
        Val::Scalar(address) if e.cg.class(new) != Class::Zero => {
            e.fb.ins().store(flags(), address, to, 0);
        }
        Val::Memory(base) => {
            let slot = e.cg.slot(option, Proj::Variant(SOME, 0));
            let at = e.component_address(base, &slot);
            let converted = e.value_at(at, new);
            e.store_val(to, new, converted);
            // A boxed value is shared with its box, which goes.
            if slot.boxed {
                let copy = e.value_at(to, new);
                e.refcount(new, copy, false);
                e.refcount(option, value, true);
            }
        }
        _ => {}
    }
    let yes = e.fb.ins().iconst(types::I8, 1);
    e.fb.ins().return_(&[yes]);
    e.fb.switch_to_block(none);
    let no = e.fb.ins().iconst(types::I8, 0);
    e.fb.ins().return_(&[no]);
    e.fb.seal_all_blocks();
    let config = e.cg.module.isa().frontend_config();
    e.fb.finalize(config);
}

/// A thunk with the uniform convention of `fn(...)` values that ignores the
/// captures' address and calls the named function `target`.
fn define_thunk(
    cg: &mut Codegen,
    body: &Body,
    target: FnId,
    func: &mut ir::Function,
    fbc: &mut FunctionBuilderContext,
) {
    let mut fb = FunctionBuilder::new(func, fbc);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    let mut params = fb.block_params(entry).to_vec();
    // Drop the captures' address, which follows the result slot if any.
    let env = usize::from(cg.class(body.result) == Class::Memory);
    params.remove(env);
    let results = call_passing(cg, &mut fb, target, &params);
    fb.ins().return_(&results);
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
}

/// A point's state: each local live there, how it is kept, its offset and
/// size; and the state's size.
type StateLayout = (Vec<(Local, StateKept, u32, u32)>, u32);

/// What a patch converts in place in values with identity that hold a
/// type of another shape: its conversion, by its place among the patch's,
/// the identities of the old type and of the new, the new one's size and
/// alignment, and its release and visit glue.
type LiveObject = (usize, DataId, DataId, u32, u32, Option<FuncId>, Option<FuncId>);

/// How a live build keeps a local of a point's state.
#[derive(Clone, Copy)]
enum StateKept {
    Var(Variable, ir::Type),
    Pointer(Variable),
    Slot(ir::StackSlot),
}

/// Where a MIR local lives.
#[derive(Copy, Clone)]
enum Storage {
    Zero,
    Var(Variable),
    Slot(ir::StackSlot),
    /// An aggregate owned by someone else: a parameter or the result slot.
    Pointer(Variable),
}

struct FnCodegen<'a, 'b> {
    e: Emit<'a, dyn Gen<'b> + 'a>,
    body: &'a Body,
    /// Every function, for named functions used as values.
    bodies: &'a [Body],
    blocks: Vec<ir::Block>,
    storage: Vec<Storage>,
    /// Where the statement being compiled is, for panics of the runtime.
    span: Span,
    sites: Vec<Site>,
    /// The inlined call the block being compiled comes from.
    scope: Option<u32>,
    /// In a debug build, what its probes show.
    debug: Option<DebugFrame>,
    /// What the probe just placed answered: whether to restart the frame;
    /// the local that holds it, and the line each block a restart point
    /// leads to is on.
    probed: Option<ir::Value>,
    carrying: bool,
    carried: HashMap<usize, (String, usize)>,
    /// The site of the probe just placed, as a value and its number; that
    /// of each point with a probe; and the state of the call being made.
    site_now: Option<(ir::Value, u32)>,
    point_sites: HashMap<u32, ir::Value>,
    osr_arg: Option<ir::Value>,
    state_slot: Option<ir::StackSlot>,
    /// Where the transfer leaves the frame's parameters, a word each, which
    /// the runtime converts for a function of another identity; and where
    /// the call to the new code takes them from.
    params_slot: Option<ir::StackSlot>,
    osr_params: Option<ir::Value>,
    /// With coverage: the counters, the line counted last in the block,
    /// and the ways of branches by the block and statement they start at.
    counters: Option<ir::GlobalValue>,
    counted: Option<(u32, usize)>,
    starts: HashMap<(usize, usize), Vec<Span>>,
    ways: HashMap<(usize, usize), Vec<usize>>,
    /// Locals that only ever hold a string literal, which counts no
    /// references: retaining and releasing them does nothing; and the
    /// literal, when it is always the same.
    literal: HashMap<Local, Option<String>>,
    /// While a statement that a check for a panic follows is built: where
    /// its panics unwind to (15.9). And whether that check follows one
    /// whose panics all jumped there, so that it asks nothing.
    unwind_to: Option<ir::Block>,
    quiet_check: bool,
    /// Whether the statement being built called compiled code with a
    /// `try_call`, and whether the block it is in returns: where a panic
    /// that unwinds the function goes to its caller's way.
    tried: bool,
    returns: bool,
}

/// What a function of a debug build keeps for its probes (toolchain 7.6):
/// its index among the debug table's functions; each binding it shows, and
/// for one in a variable the slot its value is copied to when a probe
/// runs; a bit for each that holds a value now; their addresses, for the
/// probe; and the line the last probe was for, in the block.
struct DebugFrame {
    function: u32,
    /// In a live build, the word that holds the number of the function's
    /// first site, and that number: its probes count from it, so that the
    /// code of a function that did not change stays the same.
    first: Option<(DataId, usize)>,
    shown: Vec<(Local, Option<ir::StackSlot>)>,
    alive: ir::StackSlot,
    addresses: ir::StackSlot,
    line: Option<(String, usize)>,
}

impl<'a, 'b> FnCodegen<'a, 'b> {
    fn new(
        cg: &'a mut (dyn Gen<'b> + 'a),
        body: &'a Body,
        bodies: &'a [Body],
        func: &'a mut ir::Function,
        fbc: &'a mut FunctionBuilderContext,
    ) -> Self {
        let mut fb = FunctionBuilder::new(func, fbc);
        let blocks = body.blocks.iter().map(|_| fb.create_block()).collect();
        let mut storage = Vec::with_capacity(body.locals.len());
        // Locals that hold the address of an element they borrow.
        let lent: HashSet<Local> = body
            .blocks
            .iter()
            .flat_map(|b| &b.stmts)
            .filter_map(|s| match s {
                Statement::Lend(local, _) | Statement::LendAt(local, _) => Some(*local),
                _ => None,
            })
            .collect();
        for (index, &ty) in body.locals.iter().enumerate() {
            let local = Local(index as u32);
            let borrowed = body.params.contains(&local) || body.ret == Some(local);
            let loan =
                body.params.iter().position(|&p| p == local).is_some_and(|i| body.loans[i]) || lent.contains(&local);
            storage.push(match cg.class(ty) {
                Class::Zero => Storage::Zero,
                _ if loan => Storage::Pointer(fb.declare_var(POINTER)),
                Class::Scalar(t) => Storage::Var(fb.declare_var(t)),
                Class::Memory if borrowed => Storage::Pointer(fb.declare_var(POINTER)),
                Class::Memory => {
                    let layout = cg.layout(ty);
                    let align = layout.align.trailing_zeros() as u8;
                    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size, align);
                    Storage::Slot(fb.create_sized_stack_slot(data))
                }
            });
        }
        let debug = cg.debug().then(|| {
            let mut shown = Vec::new();
            let mut described = Vec::new();
            for (index, name) in body.names.iter().enumerate() {
                let (Some(name), local) = (name, Local(index as u32)) else { continue };
                let copy = match storage[index] {
                    Storage::Zero => continue,
                    // A copy of a register's value: a word, or two.
                    Storage::Var(_) => {
                        let size = cg.layout(body.locals[index]).size.max(8);
                        Some(fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3)))
                    }
                    Storage::Slot(_) | Storage::Pointer(_) => None,
                };
                shown.push((local, copy));
                let ty = body.locals[index];
                described.push((name.clone(), ty, cg.glue(Glue::Show, ty)));
            }
            let words = shown.len().div_ceil(64).max(1) as u32;
            let alive = fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, words * 8, 3));
            let size = (shown.len().max(1) * 8) as u32;
            let addresses = fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
            let function = cg.debug_function(body.name.clone(), described);
            let first = cg.function_sites();
            DebugFrame { function, first, shown, alive, addresses, line: None }
        });
        FnCodegen {
            e: Emit { cg, fb },
            body,
            bodies,
            blocks,
            storage,
            span: Span::default(),
            sites: Vec::new(),
            scope: None,
            debug,
            probed: None,
            carrying: false,
            carried: HashMap::new(),
            site_now: None,
            point_sites: HashMap::new(),
            osr_arg: None,
            state_slot: None,
            params_slot: None,
            osr_params: None,
            counters: None,
            counted: None,
            starts: HashMap::new(),
            ways: HashMap::new(),
            literal: literals(body),
            unwind_to: None,
            quiet_check: false,
            tried: false,
            returns: false,
        }
    }

    /// Builds the function's IR; gives where its statements are, which the
    /// source locations of its instructions index.
    fn run(mut self) -> Vec<Site> {
        let entry = self.blocks[0];
        self.e.fb.append_block_params_for_function_params(entry);
        self.e.fb.switch_to_block(entry);
        let mut incoming = self.e.fb.block_params(entry).to_vec().into_iter();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            self.e.fb.def_var(var, incoming.next().unwrap());
        }
        for (index, &param) in self.body.params.iter().enumerate() {
            match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.e.fb.def_var(var, incoming.next().unwrap()),
                // A loan of a zero-sized place still has its address parameter.
                Storage::Zero if self.body.loans[index] => {
                    incoming.next();
                }
                Storage::Zero | Storage::Slot(_) => {}
            }
        }
        if let Some(osr) = self.body.osr {
            let state = incoming.next().expect("the hidden parameter");
            self.write(&Place::local(osr), Val::Scalar(state));
        }
        if let Some(debug) = &self.debug {
            let zero = self.e.fb.ins().iconst(types::I64, 0);
            for word in 0..debug.shown.len().div_ceil(64) {
                self.e.fb.ins().stack_store(POINTER, zero, debug.alive, word as i32 * 8);
            }
            for &param in &self.body.params {
                self.mark(param, true);
            }
        }
        if self.e.cg.coverage().is_some() {
            for &(block, at, span) in &self.body.starts {
                self.starts.entry((block.0 as usize, at)).or_default().push(span);
            }
            for (n, way) in self.body.ways.iter().enumerate() {
                self.ways.entry((way.block.0 as usize, way.at)).or_default().push(n);
            }
        }
        for (index, block) in self.body.blocks.iter().enumerate() {
            self.scope = self.body.scopes.get(index).copied().flatten();
            if index > 0 {
                self.e.fb.switch_to_block(self.blocks[index]);
            } else if self.e.cg.meta() {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            self.counted = None;
            if index == 0 {
                self.count_call();
            }
            if let Some(debug) = &mut self.debug {
                debug.line = self.carried.remove(&index);
                // The blocks of a restart point carry its line on; the one
                // the statement goes on in only takes it.
                self.carrying =
                    debug.line.is_some() && !self.body.points.iter().any(|p| p.continues.0 as usize == index);
            }
            let returns = matches!(block.terminator, Terminator::Return(_));
            let last_resume = block.stmts.iter().rposition(resumes);
            let mut quiet = false;
            for (at, stmt) in block.stmts.iter().enumerate() {
                self.count_ways(index, at);
                // The panic goes to the caller from the last `UnwindResume`
                // of a block that returns.
                self.returns = returns && last_resume.is_none_or(|last| last <= at);
                // A statement a check for a panic follows: its panics
                // unwind to where the check goes (15.9). The check asks
                // nothing after one whose panics all went there.
                self.unwind_to = unwind_target(block, at).map(|target| self.blocks[target.0 as usize]);
                self.quiet_check = quiet;
                self.tried = false;
                quiet = self.inline_only(stmt);
                self.statement(stmt);
                quiet |= self.tried;
            }
            self.unwind_to = None;
            self.count_ways(index, block.stmts.len());
            // A jump back to an earlier block closes a loop: a step.
            let back = match &block.terminator {
                Terminator::Goto(target) => target.0 as usize <= index,
                Terminator::Branch(_, then, otherwise) => then.0 as usize <= index || otherwise.0 as usize <= index,
                _ => false,
            };
            if back && self.e.cg.meta() {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            match self.loop_test(index, &block.terminator) {
                // The loop's test again at its end, so that a turn takes one
                // jump instead of two.
                Some(head) => {
                    for stmt in &head.stmts {
                        self.statement(stmt);
                    }
                    self.terminator(&head.terminator);
                }
                None => self.terminator(&block.terminator),
            }
        }
        self.e.fb.seal_all_blocks();
        let config = self.e.cg.isa().frontend_config();
        self.e.fb.finalize(config);
        self.sites
    }

    // ---- places and values ---------------------------------------------------

    fn place_ty(&self, place: &Place) -> Ty {
        self.body.place_ty(self.e.cg.types(), place)
    }

    fn operand_ty(&self, operand: &Operand) -> Ty {
        match operand {
            Operand::Copy(place) => self.place_ty(place),
            Operand::Const(Const::Int(_, ty) | Const::Float(_, ty)) => *ty,
            Operand::Const(Const::Char(_)) => Ty::CHAR,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
            Operand::Const(Const::Bytes(_, ty)) => *ty,
            Operand::Const(Const::Unit) => Ty::UNIT,
        }
    }

    /// The address of a place inside an aggregate local. When `write` is
    /// set, every box on the way is made unique first (copy-on-write).
    fn pointer_to(&mut self, place: &Place, write: bool) -> ir::Value {
        // Where the value reached so far lives: in memory, or (only for an
        // array held by the root local) in a variable.
        let mut base = match self.storage[place.local.0 as usize] {
            Storage::Slot(slot) => Some(self.e.fb.ins().stack_addr(POINTER, slot, 0)),
            Storage::Pointer(var) => Some(self.e.fb.use_var(var)),
            Storage::Var(_) => None,
            Storage::Zero => unreachable!("address of a zero-sized value"),
        };
        let mut ty = self.body.locals[place.local.0 as usize];
        for &proj in &place.path {
            if let Proj::Index(index, _) = proj
                && matches!(self.e.cg.types().kind(self.e.cg.types().repr(ty)), TyKind::Map(..) | TyKind::Set(_))
            {
                // Iteration over a map or set: the entry at a valid position,
                // in storage of its own when it changes (13.3).
                let map = match base {
                    Some(address) => {
                        if write {
                            let glue = self.e.map_glue_address(ty);
                            self.e.call_rt(Rt::MapUnique, &[address, glue]);
                        }
                        self.e.fb.ins().load(POINTER, flags(), address, 0)
                    }
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        if write {
                            let slot = self.spill(var);
                            let glue = self.e.map_glue_address(ty);
                            self.e.call_rt(Rt::MapUnique, &[slot, glue]);
                            self.unspill(var, slot);
                        }
                        self.e.fb.use_var(var)
                    }
                };
                let position = self.scalar(&Operand::local(index));
                base = Some(self.e.call_rt(Rt::MapEntry, &[map, position]).unwrap());
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            if let Proj::Index(index, span) = proj
                && let TyKind::CArray(element, count) = *self.e.cg.types().kind(ty)
            {
                // A C array is stored in place.
                let address = base.expect("a C array lives in memory");
                let index = self.scalar(&Operand::local(index));
                let outside = self.e.fb.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, index, count as i64);
                self.panic_values_if(outside, 1, span, |this| {
                    let count = this.e.fb.ins().iconst(types::I64, count as i64);
                    (1 << 16 | 0x140, index, count, ty)
                });
                let size = self.e.cg.layout(element).size;
                let offset = self.e.fb.ins().imul_imm_u(index, size as i64);
                base = Some(self.e.fb.ins().iadd(address, offset));
                ty = element;
                continue;
            }
            if let Proj::Captures(captures) = proj {
                let object = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                base = Some(self.e.fb.ins().iadd_imm_s(object, FN_HEADER));
                ty = captures;
                continue;
            }
            if let Proj::Deref = proj {
                assert!(!write, "the value of a `Ref` is never written");
                let reference = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                base = Some(self.e.ref_value(reference));
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            // The value inside an option of a reference is the option.
            if base.is_none()
                && let Proj::Variant(SOME, 0) = proj
                && let Shape::Niche = self.e.cg.layout(ty).shape
            {
                ty = project(self.e.cg.types(), ty, proj);
                continue;
            }
            if let Proj::Index(index, _) | Proj::InBounds(index) = proj {
                let element = project(self.e.cg.types(), ty, proj);
                let sequence = match base {
                    Some(address) => {
                        if write {
                            self.make_unique(address, element);
                        }
                        self.e.fb.ins().load(POINTER, flags(), address, 0)
                    }
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        if write {
                            let slot = self.spill(var);
                            self.make_unique(slot, element);
                            self.unspill(var, slot);
                        }
                        self.e.fb.use_var(var)
                    }
                };
                let index = self.scalar(&Operand::local(index));
                base = Some(match proj {
                    Proj::Index(_, span) => self.element_address(sequence, ty, index, span),
                    _ => self.element_at(sequence, ty, index),
                });
                ty = element;
                continue;
            }
            let address = base.expect("fields live in memory");
            let slot = self.e.cg.slot(ty, proj);
            if slot.boxed && write {
                let cell = self.e.offset(address, slot.offset);
                let retain = self.e.glue_address(Glue::Retain, slot.ty);
                let release = self.e.glue_address(Glue::Release, slot.ty);
                self.e.call_rt(Rt::BoxUnique, &[cell, retain, release]);
            }
            base = Some(self.e.component_address(address, &slot));
            ty = slot.ty;
        }
        base.expect("address of a scalar local")
    }

    /// The address of `sequence[index]`, panicking with `P0001` out of
    /// bounds.
    fn element_address(&mut self, sequence: ir::Value, ty: Ty, index: ir::Value, span: Span) -> ir::Value {
        // An index from the start, in bounds, takes one comparison; one
        // from the end, or out of bounds, takes the cold path.
        let length = self.e.fb.ins().load(types::I64, flags(), sequence, 8);
        let (found, from_end, next) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        let address = self.e.fb.append_block_param(next, POINTER);
        let inside = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, index, length);
        self.e.fb.ins().brif(inside, found, &[], from_end, &[]);
        self.e.fb.switch_to_block(found);
        let element = self.element_at(sequence, ty, index);
        self.e.fb.ins().jump(next, &[element.into()]);
        self.e.fb.set_cold_block(from_end);
        self.e.fb.switch_to_block(from_end);
        let (wrapped, inside) = self.position(sequence, index);
        let outside = self.e.fb.ins().bxor_imm_u(inside, 1);
        let deque = self.e.cg.sequence_parts(ty).1;
        self.panic_values_if(outside, 1, span, |_| ((deque as u32) << 17 | 0x140, index, length, ty));
        let rest = self.e.fb.current_block().unwrap();
        self.e.fb.set_cold_block(rest);
        let element = self.element_at(sequence, ty, wrapped);
        self.e.fb.ins().jump(next, &[element.into()]);
        self.e.fb.switch_to_block(next);
        address
    }

    /// An index as a position, counting from the end when negative (10.3),
    /// and whether that position holds an element.
    fn position(&mut self, sequence: ir::Value, index: ir::Value) -> (ir::Value, ir::Value) {
        let length = self.e.fb.ins().load(types::I64, flags(), sequence, 8);
        let wrapped = self.e.fb.ins().iadd(index, length);
        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
        let position = self.e.fb.ins().select(negative, wrapped, index);
        let inside = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, position, length);
        (position, inside)
    }

    /// The address of the element at a valid position of an array or deque.
    fn element_at(&mut self, sequence: ir::Value, ty: Ty, position: ir::Value) -> ir::Value {
        let (element, deque) = self.e.cg.sequence_parts(ty);
        let position = if deque {
            let head = self.e.fb.ins().load(types::I64, flags(), sequence, 24);
            self.e.fb.ins().iadd(position, head)
        } else {
            position
        };
        let size = self.e.cg.layout(element).size as i64;
        let offset = self.e.fb.ins().imul_imm_s(position, size);
        let address = self.e.fb.ins().iadd(sequence, offset);
        self.e.fb.ins().iadd_imm_s(address, ARRAY_HEADER)
    }

    /// The runtime arguments of `[start:end:step]`: each bound with a flag
    /// for its presence, the step, and the location for panics.
    fn slice_range(&mut self, parts: &[Option<Operand>; 3], span: Span) -> [ir::Value; 6] {
        let bound = |this: &mut Self, part: &Option<Operand>, default: i64| match part {
            Some(part) => (this.scalar(part), this.e.fb.ins().iconst(types::I8, 1)),
            None => (this.e.fb.ins().iconst(types::I64, default), this.e.fb.ins().iconst(types::I8, 0)),
        };
        let (start, has_start) = bound(self, &parts[0], 0);
        let (end, has_end) = bound(self, &parts[1], 0);
        let (step, _) = bound(self, &parts[2], 1);
        let location = self.location(span);
        [start, has_start, end, has_end, step, location]
    }

    /// `values[start:end:step] = value` (10.5).
    fn assign_slice(&mut self, target: &Place, parts: &[Option<Operand>; 3], value: &Operand, span: Span) {
        let (element, _) = self.e.cg.sequence_parts(self.place_ty(target));
        let range = self.slice_range(parts, span);
        let source = self.scalar(value);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let (slot, spilled) = self.array_slot(target);
        let [start, has_start, end, has_end, step, location] = range;
        let args = [slot, start, has_start, end, has_end, step, source, size, retain, release, location];
        self.e.call_rt(Rt::ArrayAssignSlice, &args);
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
    }

    /// Two loans of one call that select elements of the same array (16.4).
    fn check_region(&mut self, array: &Operand, region: &[Option<Operand>; 3], other: &Overlap, span: Span) {
        let array = self.scalar(array);
        let len = self.e.fb.ins().load(types::I64, flags(), array, 8);
        let [start, has_start, end, has_end, step, location] = self.slice_range(region, span);
        let (index, second) = match other {
            Overlap::Region(parts) => (0, self.slice_range(parts, span)),
            Overlap::Index(at) => {
                let at = self.scalar(at);
                let (zero, none) = (self.e.fb.ins().iconst(types::I64, 0), self.e.fb.ins().iconst(types::I8, 0));
                (1, [at, none, zero, none, zero, location])
            }
        };
        let index = self.e.fb.ins().iconst(types::I8, index);
        let [start2, has_start2, end2, has_end2, step2, _] = second;
        let args =
            [len, start, has_start, end, has_end, step, index, start2, has_start2, end2, has_end2, step2, location];
        self.e.call_rt(Rt::ArrayCheckRegion, &args);
    }

    /// Lifted arithmetic over arrays (10.6), done by the runtime.
    fn array_arithmetic(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let mut args = Vec::new();
        let mut number = Ty::ERROR;
        for operand in [left, right] {
            let mut ty = self.operand_ty(operand);
            let value = self.operand(operand);
            args.push(self.in_memory(ty, value));
            let mut depth = 0;
            while let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) {
                ty = element;
                depth += 1;
            }
            args.push(self.e.fb.ins().iconst(types::I32, depth));
            number = ty;
        }
        let op = match op {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            _ => 4,
        };
        // The runtime's codes: the integers of 8 to 64 bits, `Float32`,
        // `Float`, `Int128` and `UInt128`.
        let code = match *self.e.cg.types().kind(self.e.cg.types().repr(number)) {
            TyKind::Int(IntTy::I128) => 10,
            TyKind::Int(IntTy::U128) => 11,
            TyKind::Int(int) => IntTy::ALL.iter().position(|&i| i == int).unwrap() as i64,
            TyKind::Float(FloatTy::F32) => 8,
            _ => 9,
        };
        args.push(self.e.fb.ins().iconst(types::I32, op));
        args.push(self.e.fb.ins().iconst(types::I32, code));
        args.push(self.location(span));
        self.e.call_rt(Rt::ArrayArith, &args).unwrap()
    }

    /// Stack space for a value of `ty` that the runtime writes.
    fn scratch(&mut self, ty: Ty) -> ir::Value {
        let size = self.e.cg.layout(ty).size.max(1);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_addr(POINTER, slot, 0)
    }

    /// Makes the array stored at `slot` unique before a write (copy-on-write).
    fn make_unique(&mut self, slot: ir::Value, element: Ty) {
        // Nearly always the only reference already: no call.
        let array = self.e.fb.ins().load(POINTER, flags(), slot, 0);
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), array);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let (copy, done) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(unique, done, &[], copy, &[]);
        self.e.fb.set_cold_block(copy);
        self.e.fb.switch_to_block(copy);
        let size = self.e.element_size(element);
        let extra = self.e.fb.ins().iconst(POINTER, 0);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        self.e.call_rt(Rt::ArrayUnique, &[slot, size, extra, retain, release]);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// Stores a variable into a fresh stack slot so the runtime can update it.
    fn spill(&mut self, var: Variable) -> ir::Value {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let value = self.e.fb.use_var(var);
        self.e.fb.ins().store(flags(), value, address, 0);
        address
    }

    fn unspill(&mut self, var: Variable, address: ir::Value) {
        let current = self.e.fb.use_var(var);
        let ty = self.e.fb.func.dfg.value_type(current);
        let value = self.e.fb.ins().load(ty, flags(), address, 0);
        self.e.fb.def_var(var, value);
    }

    /// The address of the storage holding the array at `place`, spilled to
    /// the stack when it lives in a variable; `unspill` it after the call.
    fn array_slot(&mut self, place: &Place) -> (ir::Value, Option<Variable>) {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            return (self.spill(var), Some(var));
        }
        (self.pointer_to(place, true), None)
    }

    /// The address of a value, spilling a scalar to the stack.
    fn in_memory(&mut self, ty: Ty, value: Val) -> ir::Value {
        match value {
            Val::Memory(address) => address,
            Val::Scalar(value) => {
                let size = self.e.cg.layout(ty).size.max(1);
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                self.e.fb.ins().store(flags(), value, address, 0);
                address
            }
            // The runtime copies nothing, but needs a valid address.
            Val::Zero => self.scratch(ty),
        }
    }

    fn read(&mut self, place: &Place) -> Val {
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            return Val::Scalar(self.e.fb.use_var(var));
        }
        // The address inside an option of one is the option itself.
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && let [Proj::Variant(SOME, 0)] = place.path[..]
            && let Shape::Niche = self.e.cg.layout(self.body.locals[place.local.0 as usize]).shape
        {
            return Val::Scalar(self.e.fb.use_var(var));
        }
        match self.e.cg.class(self.place_ty(place)) {
            Class::Zero => Val::Zero,
            _ => {
                let address = self.pointer_to(place, false);
                self.e.value_at(address, self.place_ty(place))
            }
        }
    }

    fn write(&mut self, place: &Place, value: Val) {
        if let (Val::Scalar(value), Storage::Var(var)) = (value, self.storage[place.local.0 as usize])
            && place.path.is_empty()
        {
            self.e.fb.def_var(var, value);
            return;
        }
        if let Val::Zero = value {
            return;
        }
        let address = self.pointer_to(place, true);
        let ty = self.place_ty(place);
        self.e.store_val(address, ty, value);
    }

    /// The elements of an array literal when all are constants of its
    /// element's type that the program's data can hold; never in a live
    /// build, whose reloads replace the arrays they convert and free them.
    fn constants(&mut self, values: &[Operand], ty: Ty) -> Option<Vec<Const>> {
        if self.e.cg.live() {
            return None;
        }
        let types = self.e.cg.types();
        let TyKind::Array(element) = *types.kind(types.repr(ty)) else { return None };
        let element = types.repr(element);
        let fits = |constant: &Const| match constant {
            Const::Int(_, of) | Const::Float(_, of) => types.repr(*of) == element,
            Const::Bool(_) => element == Ty::BOOL,
            Const::Char(_) => element == Ty::CHAR,
            Const::Str(_) => element == Ty::STRING,
            Const::Bytes(_, of) => types.repr(*of) == element,
            Const::Unit => false,
        };
        if values.is_empty() {
            return None;
        }
        values
            .iter()
            .map(|value| match value {
                Operand::Const(constant) if fits(constant) => Some(constant.clone()),
                _ => None,
            })
            .collect()
    }

    fn operand(&mut self, operand: &Operand) -> Val {
        match operand {
            Operand::Copy(place) => self.read(place),
            Operand::Const(Const::Int(value, ty)) => {
                let clif = self.e.cg.scalar_type(*ty);
                Val::Scalar(self.e.int_const(clif, *value))
            }
            Operand::Const(Const::Float(bits, ty)) => {
                let value = f64::from_bits(*bits);
                Val::Scalar(if *ty == Ty::F32 {
                    self.e.fb.ins().f32const(value as f32)
                } else {
                    self.e.fb.ins().f64const(value)
                })
            }
            Operand::Const(Const::Char(c)) => Val::Scalar(self.e.fb.ins().iconst(types::I32, *c as i64)),
            Operand::Const(Const::Bool(value)) => Val::Scalar(self.e.fb.ins().iconst(types::I8, *value as i64)),
            Operand::Const(Const::Str(text)) => Val::Scalar(self.e.string_ptr(text)),
            Operand::Const(Const::Bytes(bytes, _)) => {
                let id = self.e.cg.bytes(bytes);
                let global = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Operand::Const(Const::Unit) => Val::Zero,
        }
    }

    fn scalar(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar operand"),
        }
    }

    /// Compiles what follows as the source at `span`: runtime panics report
    /// it, and stack traces map the calls to it.
    fn at(&mut self, span: Span) {
        self.at_typed(span, None);
    }

    /// The same for a statement that assigns a local of type `ty`.
    fn at_typed(&mut self, span: Span, ty: Option<Ty>) {
        self.span = span;
        // A statement the compiler made up stays at the last one's.
        if span != Span::default() {
            let site = self.site(span.start, ty);
            self.e.fb.set_srcloc(site);
        }
    }

    /// The source location of code at `start` that assigns `ty`.
    fn site(&mut self, start: u32, ty: Option<Ty>) -> ir::SourceLoc {
        self.sites.push((start, ty, self.scope));
        ir::SourceLoc::new(self.sites.len() as u32)
    }

    /// In a debug build, notes whether a binding holds a value: a probe
    /// shows only those that do, and none whose value went away.
    fn mark(&mut self, local: Local, holds: bool) {
        let Some(debug) = &self.debug else { return };
        let Some(index) = debug.shown.iter().position(|(l, _)| *l == local) else { return };
        let (alive, offset, bit) = (debug.alive, (index / 64) as i32 * 8, 1i64 << (index % 64));
        let word = self.e.fb.ins().stack_load(POINTER, types::I64, alive, offset);
        let word = if holds { self.e.fb.ins().bor_imm_s(word, bit) } else { self.e.fb.ins().band_imm_s(word, !bit) };
        self.e.fb.ins().stack_store(POINTER, word, alive, offset);
    }

    /// With coverage, one more run of what a counter counts.
    fn count(&mut self, counted: Counted) {
        let Some(counters) = self.e.cg.coverage() else { return };
        let index = self.e.cg.counter(counted);
        let counters = match self.counters {
            Some(counters) => counters,
            None => *self.counters.insert(self.e.cg.declare_data_in_func(counters, self.e.fb.func)),
        };
        let base = self.e.fb.ins().symbol_value(POINTER, counters);
        let offset = self.e.cg.counter_offset(&mut self.e.fb, index);
        let address = self.e.fb.ins().iadd(base, offset);
        let count = self.e.fb.ins().load(types::I64, flags(), address, 0);
        let count = self.e.fb.ins().iadd_imm_s(count, 1);
        self.e.fb.ins().store(flags(), count, address, 0);
    }

    /// In a build for `--fuzz`, tells the runtime the operands of a
    /// comparison of integers or texts of the counted code, which the
    /// search takes values from (testing 10.6).
    fn compared(&mut self, op: BinOp, left: &Operand, right: &Operand, ty: Ty) {
        if !self.e.cg.fuzzing() || !matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
        {
            return;
        }
        let span = self.body.span;
        if span == Span::default() || !self.counts(self.e.cg.sources().file_of(span.start)) {
            return;
        }
        let types = self.e.cg.types();
        let rt = match types.kind(types.repr(ty)) {
            TyKind::String => Rt::FuzzCompareText,
            TyKind::Int(_) if types.as_int(ty).map(IntTy::bits) == Some(64) => Rt::FuzzCompare,
            _ => return,
        };
        if let (Val::Scalar(a), Val::Scalar(b)) = (self.operand(left), self.operand(right)) {
            self.e.call_rt(rt, &[a, b]);
        }
    }

    /// Whether coverage counts a file: the project's own code, and for the
    /// search of `--fuzz` its dependencies' too.
    fn counts(&self, file: &crate::source::SourceFile) -> bool {
        file.own || (file.dependency && self.e.cg.fuzzing())
    }

    /// With coverage, a run of the line a statement of the project's code
    /// starts on, once for statements one after another on a line.
    fn count_line(&mut self, span: Span) {
        let file = self.e.cg.sources().file_of(span.start);
        let (line, _) = file.line_col(span.start);
        if !self.counts(file) || self.counted == Some((file.base, line)) {
            return;
        }
        self.counted = Some((file.base, line));
        self.count(Counted::Line { file: file.logical.clone(), line });
    }

    /// With coverage, a call of a function of the project's code.
    fn count_call(&mut self) {
        let span = self.body.span;
        if self.e.cg.coverage().is_none() || span == Span::default() {
            return;
        }
        let file = self.e.cg.sources().file_of(span.start);
        if self.counts(file) {
            let (line, _) = file.line_col(span.start);
            let name = self.body.name.clone();
            self.count(Counted::Function { file: file.logical.clone(), line, name });
        }
    }

    /// With coverage, the statements and the ways of branches that start
    /// at this statement of the block.
    fn count_ways(&mut self, block: usize, at: usize) {
        for span in self.starts.remove(&(block, at)).unwrap_or_default() {
            self.count_line(span);
        }
        let Some(ways) = self.ways.remove(&(block, at)) else { return };
        for n in ways {
            let way = &self.body.ways[n];
            let file = self.e.cg.sources().file_of(way.span.start);
            if !self.counts(file) || way.span == Span::default() {
                continue;
            }
            let (line, column) = file.line_col(way.span.start);
            let branch = file.line_col(way.branch.start);
            let label = match way.label {
                // An arm is called by its pattern.
                "arm" => file.slice(way.span).lines().next().unwrap_or_default().to_string(),
                label => label.to_string(),
            };
            self.count(Counted::Way { file: file.logical.clone(), line, column, branch, label });
        }
    }

    /// In a debug build, a probe where a line starts: the debugger stops
    /// there when it is asked to, and reads the bindings.
    fn probe(&mut self, span: Span) {
        let Some(mut debug) = self.debug.take() else { return };
        let (file, line, _) = self.e.cg.sources().position(span.start);
        if span != Span::default() && debug.line.as_ref() != Some(&(file.clone(), line)) {
            debug.line = Some((file, line));
            for (index, &(local, copy)) in debug.shown.iter().enumerate() {
                let address = match (self.storage[local.0 as usize], copy) {
                    (Storage::Var(var), Some(slot)) => {
                        let value = self.e.fb.use_var(var);
                        self.e.fb.ins().stack_store(POINTER, value, slot, 0);
                        self.e.fb.ins().stack_addr(POINTER, slot, 0)
                    }
                    (Storage::Slot(slot), _) => self.e.fb.ins().stack_addr(POINTER, slot, 0),
                    (Storage::Pointer(var), _) => self.e.fb.use_var(var),
                    _ => self.e.fb.ins().iconst(POINTER, 0),
                };
                self.e.fb.ins().stack_store(POINTER, address, debug.addresses, index as i32 * 8);
            }
            let index = self.e.cg.debug_site(span.start, debug.function);
            let site = match debug.first {
                Some((first, number)) => {
                    let first = self.e.cg.declare_data_in_func(first, self.e.fb.func);
                    let first = self.e.fb.ins().symbol_value(POINTER, first);
                    let first = self.e.fb.ins().load(types::I64, flags(), first, 0);
                    self.e.fb.ins().iadd_imm_s(first, (index - number) as i64)
                }
                None => self.e.cg.site_constant(&mut self.e.fb, index),
            };
            let addresses = self.e.fb.ins().stack_addr(POINTER, debug.addresses, 0);
            let alive = self.e.fb.ins().stack_addr(POINTER, debug.alive, 0);
            self.probed = self.e.call_rt(Rt::DebugLine, &[site, addresses, alive]);
            // A binding the debugger changed comes back from its copy.
            for &(local, copy) in &debug.shown {
                if let (Storage::Var(var), Some(slot)) = (self.storage[local.0 as usize], copy)
                    && let Class::Scalar(ty) = self.e.cg.class(self.body.locals[local.0 as usize])
                {
                    let value = self.e.fb.ins().stack_load(POINTER, ty, slot, 0);
                    self.e.fb.def_var(var, value);
                }
            }
            self.site_now = Some((site, self.e.cg.site_base() + index as u32));
        }
        self.debug = Some(debug);
    }

    /// A live build's state at a point: each local live there, how it is
    /// kept, and where in the state a frame brings it goes, with the
    /// state's size; the same in the old code that writes it and the new
    /// that reads it, for the runtime to match by the locals' names.
    fn point_layout(&mut self, point: u32) -> StateLayout {
        if let Some(layout) = self.e.cg.point_layout(point) {
            return layout;
        }
        let mut items = Vec::new();
        let mut size = 0u32;
        for &local in &self.body.points[point as usize].state {
            let ty = self.body.locals[local.0 as usize];
            let (kept, bytes, align) = match self.storage[local.0 as usize] {
                Storage::Zero => continue,
                Storage::Var(var) => match self.e.cg.class(ty) {
                    Class::Scalar(clif) => (StateKept::Var(var, clif), clif.bytes(), clif.bytes()),
                    _ => (StateKept::Var(var, POINTER), 8, 8),
                },
                Storage::Pointer(var) => (StateKept::Pointer(var), 8, 8),
                Storage::Slot(slot) => {
                    let layout = self.e.cg.layout(ty);
                    (StateKept::Slot(slot), layout.size, layout.align.max(1))
                }
            };
            size = size.next_multiple_of(align);
            items.push((local, kept, size, bytes));
            size += bytes;
        }
        let layout = (items, size);
        self.e.cg.keep_point_layout(point, layout.clone());
        layout
    }

    /// The old code's side of a move: its state at the point, which the
    /// runtime gives back as the new code expects it, or null.
    fn transfer(&mut self, point: u32) -> ir::Value {
        let (items, _) = self.point_layout(point);
        // One place for the state of every point: frames stay small, and a
        // deep recursion still meets the stack's guard.
        let slot = match self.state_slot {
            Some(slot) => slot,
            None => {
                let largest = (0..self.body.points.len() as u32).map(|p| self.point_layout(p).1).max().unwrap_or(0);
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, largest.max(8), 4);
                let slot = self.e.fb.create_sized_stack_slot(data);
                self.state_slot = Some(slot);
                slot
            }
        };
        for (_, kept, offset, bytes) in items {
            let address = self.e.fb.ins().stack_addr(POINTER, slot, offset as i32);
            match kept {
                StateKept::Var(var, _) | StateKept::Pointer(var) => {
                    let value = self.e.fb.use_var(var);
                    self.e.fb.ins().store(flags(), value, address, 0);
                }
                StateKept::Slot(kept) => {
                    let source = self.e.fb.ins().stack_addr(POINTER, kept, 0);
                    self.e.copy_memory(address, source, bytes);
                }
            }
        }
        let site = match self.point_sites.get(&point) {
            Some(&site) => site,
            None => self.e.fb.ins().iconst(types::I64, -1),
        };
        let buffer = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        // Where the result goes, when it goes in memory, then each
        // parameter.
        let params = self.params_word_slot();
        if let Some(ret) = self.body.ret
            && let Storage::Pointer(var) = self.storage[ret.0 as usize]
        {
            let address = self.e.fb.use_var(var);
            self.e.fb.ins().stack_store(POINTER, address, params, 0);
        }
        for (index, &param) in self.body.params.iter().enumerate() {
            let value = match self.storage[param.0 as usize] {
                Storage::Var(var) | Storage::Pointer(var) => self.e.fb.use_var(var),
                Storage::Slot(slot) => self.e.fb.ins().stack_addr(POINTER, slot, 0),
                Storage::Zero => continue,
            };
            self.e.fb.ins().stack_store(POINTER, value, params, (index as i32 + 1) * 8);
        }
        let params = self.e.fb.ins().stack_addr(POINTER, params, 0);
        self.e.call_rt(Rt::LiveTransfer, &[site, buffer, params]).expect("the new state")
    }

    /// A word for where the result goes, and one for each parameter, as
    /// the call passes them.
    fn params_word_slot(&mut self) -> ir::StackSlot {
        if let Some(slot) = self.params_slot {
            return slot;
        }
        let size = (self.body.params.len() as u32 + 1) * 8;
        let slot = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
        self.params_slot = Some(slot);
        slot
    }

    /// The call of the new code a frame moves to, with its parameters as
    /// the runtime gives them, converted, or as they are; what the runtime
    /// converted goes once the call returns.
    fn call_osr(
        &mut self,
        id: FnId,
        args: &[Operand],
        state: &Operand,
        result: Option<ir::Value>,
    ) -> Option<ir::Value> {
        let state = self.scalar(state);
        let block = self.e.fb.ins().load(types::I64, flags(), state, 16);
        let slot = self.params_word_slot();
        let params = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let given = self.e.fb.ins().icmp_imm_u(IntCC::NotEqual, block, 0);
        self.osr_params = Some(self.e.fb.ins().select(given, block, params));
        self.osr_arg = Some(state);
        // Stacks show the frame once, in its new code.
        self.e.fb.set_srcloc(ir::SourceLoc::default());
        let value = self.call(Callee::Fn(id), args, result);
        self.e.call_rt(Rt::LiveArgsDone, &[block]);
        value
    }

    /// In a live build, a value with identity the runtime knows of, with
    /// the identity of the type of what it holds: a reload that changes
    /// that type converts it (toolchain 7.10).
    fn live_object(&mut self, object: ir::Value, holds: Ty, kind: i64) {
        if !self.e.cg.live() {
            return;
        }
        let identity = self.e.cg.identity(holds);
        let identity = self.e.cg.declare_data_in_func(identity, self.e.fb.func);
        let identity = self.e.fb.ins().symbol_value(POINTER, identity);
        // A `Ref`'s value is released with the glue of its type then, which
        // a reload changes.
        let release = match kind {
            0 => self.e.glue_or_null(Glue::Release, holds),
            _ => self.e.fb.ins().iconst(POINTER, 0),
        };
        let kind = self.e.fb.ins().iconst(types::I8, kind);
        self.e.call_rt(Rt::LiveObject, &[object, identity, kind, release]);
    }

    /// The hidden parameter's value: the state a frame brings.
    fn osr_state(&mut self) -> ir::Value {
        let osr = self.body.osr.expect("a live build's function");
        match self.read(&Place::local(osr)) {
            Val::Scalar(value) => value,
            _ => unreachable!("the state is a word"),
        }
    }

    /// The new code's side of a move: the state of the point into the
    /// locals live there, which the frame now owns.
    fn osr_load(&mut self, point: u32) {
        let state = self.osr_state();
        let (items, _) = self.point_layout(point);
        for (local, kept, offset, bytes) in items {
            let address = self.e.fb.ins().iadd_imm_s(state, 24 + offset as i64);
            match kept {
                StateKept::Var(var, clif) => {
                    let value = self.e.fb.ins().load(clif, flags(), address, 0);
                    self.e.fb.def_var(var, value);
                }
                StateKept::Pointer(var) => {
                    let value = self.e.fb.ins().load(POINTER, flags(), address, 0);
                    self.e.fb.def_var(var, value);
                }
                StateKept::Slot(kept) => {
                    let target = self.e.fb.ins().stack_addr(POINTER, kept, 0);
                    self.e.copy_memory(target, address, bytes);
                }
            }
            self.mark(local, true);
        }
        self.e.call_rt(Rt::LiveFree, &[state]);
    }

    fn location(&mut self, span: Span) -> ir::Value {
        let location = self.e.cg.sources().location(span);
        self.e.string_ptr(&location)
    }

    // ---- statements and terminators ----------------------------------------

    fn statement(&mut self, stmt: &Statement) {
        // A probe's answer is for the statement right after it.
        self.probed = None;
        match stmt {
            Statement::OsrLoad(point) => self.osr_load(*point),
            Statement::Assign(place, rvalue, span) => {
                // What it allocates: the collection a method changes, or the
                // value it makes.
                let made = match &**rvalue {
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => target,
                    _ => place,
                };
                self.at_typed(*span, Some(self.body.place_ty(self.e.cg.types(), made)));
                self.probe(*span);
                self.assign(place, rvalue, *span);
                if place.path.is_empty() {
                    self.mark(place.local, true);
                }
            }
            Statement::DeferPush(body, place) => {
                let code = self.e.cg.closure_code(*body);
                let code = self.e.cg.declare_func_in_func(code, self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, code);
                let mut spilled = Vec::new();
                let closure = self.loan_address(place, &mut spilled);
                self.e.call_rt(Rt::DeferPush, &[code, closure]);
            }
            Statement::DeferPop => {
                self.e.call_rt(Rt::DeferPop, &[]);
            }
            Statement::Clear(local) => {
                self.mark(*local, false);
                self.zero(&Place::local(*local), false);
            }
            Statement::Moved(place) => self.zero(place, true),
            Statement::Lend(local, place) => {
                if let Storage::Pointer(var) = self.storage[local.0 as usize] {
                    let address = self.pointer_to(place, true);
                    self.e.fb.def_var(var, address);
                }
            }
            Statement::LendAt(local, guard) => {
                if let Storage::Pointer(var) = self.storage[local.0 as usize] {
                    let guard = self.scalar(guard);
                    let address = self.e.call_rt(Rt::SyncValue, &[guard]).unwrap();
                    self.e.fb.def_var(var, address);
                }
            }
            Statement::Retain(place) | Statement::Release(place) => {
                if matches!(stmt, Statement::Release(_)) && place.path.is_empty() {
                    self.mark(place.local, false);
                }
                let ty = self.place_ty(place);
                if !self.e.cg.types().needs_drop(ty) || place.path.is_empty() && self.literal.contains_key(&place.local)
                {
                    return;
                }
                let value = self.read(place);
                self.e.refcount(ty, value, matches!(stmt, Statement::Release(_)));
            }
        }
    }

    /// Zeroes a place whose value moved away: releasing it releases nothing.
    fn zero(&mut self, place: &Place, write: bool) {
        let ty = self.place_ty(place);
        match self.e.cg.class(ty) {
            Class::Zero => {}
            Class::Scalar(clif) if clif.is_float() => {
                let zero = self.e.fb.ins().f64const(0.0);
                let zero = if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, zero) } else { zero };
                self.write(place, Val::Scalar(zero));
            }
            Class::Scalar(clif) => {
                let zero = self.e.int_const(clif, 0);
                self.write(place, Val::Scalar(zero));
            }
            Class::Memory => {
                let address = self.pointer_to(place, write);
                let size = self.e.cg.layout(ty).size;
                let mut offset = 0;
                for width in [types::I64, types::I32, types::I16, types::I8] {
                    let zero = self.e.fb.ins().iconst(width, 0);
                    while offset + width.bytes() <= size {
                        self.e.fb.ins().store(flags(), zero, address, offset as i32);
                        offset += width.bytes();
                    }
                }
            }
        }
    }

    fn assign(&mut self, place: &Place, rvalue: &Rvalue, span: Span) {
        let ty = self.place_ty(place);
        let class = self.e.cg.class(ty);
        if class == Class::Zero {
            // Zero-sized values carry no data; only calls and mutations have
            // effects.
            match rvalue {
                Rvalue::Call(Callee::Std(StdFn::Env(function)), args) => {
                    let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                    // An invalid name panics where the call is.
                    if matches!(function, EnvFn::Set | EnvFn::SetAll | EnvFn::Remove) {
                        values.push(self.location(span));
                    }
                    self.e.call_rt(Rt::Env(*function), &values);
                }
                Rvalue::Call(callee, args) => {
                    self.call(*callee, args, None);
                }
                Rvalue::CallOsr(id, args, state) => {
                    self.call_osr(*id, args, state, None);
                }
                Rvalue::CallIndirect(callee, args) => {
                    self.call_indirect(callee, args, None);
                }
                Rvalue::Pointer(PointerOp::Write, operands, retain) => {
                    // The memory takes the value, and one reference to a
                    // value that was copied.
                    let address = self.scalar(&operands[0]);
                    let value_ty = self.operand_ty(&operands[1]);
                    let value = self.operand(&operands[1]);
                    if *retain {
                        self.e.refcount(value_ty, value, false);
                    }
                    self.e.store_val(address, value_ty, value);
                }
                Rvalue::Mutate(mutation, target, operands, retain, span) => {
                    self.mutate(*mutation, target, operands, retain, place, *span);
                }
                Rvalue::TaskOp(op, operands, retain, span) => {
                    self.task_op(*op, operands, retain, place, *span);
                }
                Rvalue::AssignSlice(target, bounds, value, span) => self.assign_slice(target, bounds, value, *span),
                Rvalue::CheckRegion(array, region, other, span) => self.check_region(array, region, other, *span),
                Rvalue::TestMembers(members) => self.test_members(members),
                _ => {}
            }
            return;
        }
        let value = match rvalue {
            Rvalue::Call(Callee::Std(StdFn::Number(op)), args) => match self.number(*op, args, place, ty, span) {
                Some(value) => value,
                None => return,
            },
            Rvalue::Call(Callee::Std(StdFn::Fs(FsFn::Exists)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                Val::Scalar(self.e.call_rt(Rt::Fs(FsFn::Exists), &values).unwrap())
            }
            Rvalue::Call(Callee::Std(StdFn::Fs(function)), args) => {
                self.status_call(Rt::Fs(*function), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Io(function)), args) if function.fails() => {
                self.status_call(Rt::Io(*function), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleFlush), args) => {
                self.status_call(Rt::ConsoleFlush, args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Io(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                Val::Scalar(self.e.call_rt(Rt::Io(*function), &values).unwrap())
            }
            Rvalue::Call(Callee::Std(StdFn::Env(EnvFn::SetCurrentDirectory)), args) => {
                self.status_call(Rt::Env(EnvFn::SetCurrentDirectory), args, place);
                return;
            }
            Rvalue::Call(Callee::Std(StdFn::Env(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                let rt = Rt::Env(*function);
                match function {
                    EnvFn::Get => {
                        let value = self.e.call_rt(rt, &values).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, value) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                    EnvFn::Vars => {
                        let glue = self.e.map_glue_address(ty);
                        Val::Scalar(self.e.call_rt(rt, &[glue]).unwrap())
                    }
                    _ => Val::Scalar(self.e.call_rt(rt, &values).unwrap()),
                }
            }
            Rvalue::Call(Callee::Std(StdFn::TimeNow), _) => Val::Scalar(self.e.call_rt(Rt::TimeNow, &[]).unwrap()),
            Rvalue::Call(Callee::Std(StdFn::TimeUnix), _) => Val::Scalar(self.e.call_rt(Rt::TimeUnix, &[]).unwrap()),
            Rvalue::Call(Callee::Std(function @ (StdFn::TimeUtc | StdFn::TimeUtcNow)), args) => {
                // Seven words, from the year down to the nanosecond.
                let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 56, 3));
                let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
                let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                values.push(out);
                let rt = if *function == StdFn::TimeUtc { Rt::TimeUtc } else { Rt::TimeUtcNow };
                self.e.call_rt(rt, &values);
                let address = self.scratch(ty);
                for index in 0..7i32 {
                    let word = self.e.fb.ins().load(types::I64, flags(), out, index * 8);
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    self.e.fb.ins().store(flags(), word, address, slot.offset as i32);
                }
                Val::Memory(address)
            }
            Rvalue::Call(Callee::Std(StdFn::Process(function)), args) => {
                match self.process_call(*function, args, place) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleReadLine), _) => {
                let line = self.e.call_rt(Rt::ConsoleReadLine, &[]).unwrap();
                match self.pointer_option(place, ty, Ty::STRING, line) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::Call(Callee::Std(StdFn::ConsoleStream(index)), _) => {
                let index = self.e.fb.ins().iconst(types::I64, *index as i64);
                Val::Scalar(self.e.call_rt(Rt::ConsoleStream, &[index]).unwrap())
            }

            Rvalue::Call(Callee::Std(StdFn::Path(function)), args) => {
                let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
                match function {
                    PathFn::Join => Val::Scalar(self.e.call_rt(Rt::PathJoin, &values).unwrap()),
                    PathFn::IsAbsolute => Val::Scalar(self.e.call_rt(Rt::PathIsAbsolute, &values).unwrap()),
                    _ => {
                        let which = match function {
                            PathFn::Parent => 0,
                            PathFn::FileName => 1,
                            _ => 2,
                        };
                        let which = self.e.fb.ins().iconst(types::I8, which);
                        let part = self.e.call_rt(Rt::PathPart, &[values[0], which]).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, part) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                }
            }
            Rvalue::Call(callee, args) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call(*callee, args, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::Aggregate(components) => {
                let base = self.pointer_to(place, true);
                for (index, component) in components.iter().enumerate() {
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    let value = self.operand(component);
                    self.e.init_component(base, &slot, value);
                }
                return;
            }
            Rvalue::Variant(_, components) if matches!(self.e.cg.layout(ty).shape, Shape::Niche) => {
                match components.first() {
                    Some(address) => self.operand(address),
                    None => Val::Scalar(self.e.fb.ins().iconst(POINTER, 0)),
                }
            }
            Rvalue::Variant(variant, components) => {
                let tag_ty = self.e.cg.types().tag_ty(ty);
                let tag = self.operand(&Operand::Const(Const::Int(*variant as i128, tag_ty)));
                if let Class::Scalar(_) = class {
                    tag
                } else {
                    let base = self.pointer_to(place, true);
                    // A whole word when the payloads start past it, so that
                    // a copy of the value reads what was just stored as
                    // stored, without waiting for the store to land.
                    let payloads = self
                        .e
                        .cg
                        .types()
                        .variants(ty)
                        .expect("an enum")
                        .iter()
                        .enumerate()
                        .filter(|(_, components)| !components.is_empty())
                        .map(|(variant, _)| self.e.cg.slot(ty, Proj::Variant(variant as u32, 0)).offset)
                        .min();
                    match tag {
                        Val::Scalar(tag)
                            if payloads.is_none_or(|offset| offset >= 8) && self.e.cg.layout(ty).size >= 8 =>
                        {
                            let word = self.wide(tag, false);
                            self.e.fb.ins().store(flags(), word, base, 0);
                        }
                        _ => self.e.store_val(base, tag_ty, tag),
                    }
                    for (index, component) in components.iter().enumerate() {
                        let slot = self.e.cg.slot(ty, Proj::Variant(*variant, index as u32));
                        let value = self.operand(component);
                        self.e.init_component(base, &slot, value);
                    }
                    return;
                }
            }
            Rvalue::Convert(operand) => {
                let from = self.operand_ty(operand);
                let value = self.scalar(operand);
                match self.e.cg.types().kind(ty) {
                    TyKind::Result(target, _) => {
                        let target = *target;
                        let (converted, error) = self.convert_checked(value, from, target);
                        self.write_checked(place, ty, converted, error);
                        return;
                    }
                    _ => Val::Scalar(self.convert_total(value, from, ty)),
                }
            }
            Rvalue::Array(values, _) if let Some(elements) = self.constants(values, ty) => {
                let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                    unreachable!()
                };
                // Made once, in the program's data, rather than each time.
                let size = self.e.cg.layout(element).size as usize;
                let id = self.e.cg.static_array(size, elements);
                let global = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Rvalue::Array(values, retain) => {
                let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                    unreachable!()
                };
                let size = self.e.cg.layout(element).size as i64;
                let size_value = self.e.fb.ins().iconst(POINTER, size);
                let count = self.e.fb.ins().iconst(POINTER, values.len() as i64);
                let array = self.e.call_rt(Rt::ArrayNew, &[size_value, count]).unwrap();
                for (index, (value, retain)) in values.iter().zip(retain).enumerate() {
                    let address = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER + index as i64 * size);
                    let value = self.operand(value);
                    self.e.store_val(address, element, value);
                    if *retain {
                        let stored = self.e.value_at(address, element);
                        self.e.refcount(element, stored, false);
                    }
                }
                Val::Scalar(array)
            }
            Rvalue::ArrayOp(op, operands, span) => {
                if let Some(value) = self.array_op(*op, operands, place, ty, *span) {
                    value
                } else {
                    return;
                }
            }
            Rvalue::Slice(base, parts, span) => {
                let sequence = self.scalar(base);
                let range = self.slice_range(parts, *span);
                Val::Scalar(if ty == Ty::STRING {
                    self.e.call_rt(Rt::StrSlice, &[&[sequence], &range[..]].concat()).unwrap()
                } else {
                    let TyKind::Array(element) = *self.e.cg.types().kind(self.e.cg.types().repr(ty)) else {
                        unreachable!()
                    };
                    let size = self.e.element_size(element);
                    let retain = self.e.glue_or_null(Glue::Retain, element);
                    self.e.call_rt(Rt::ArraySlice, &[&[sequence, size, retain], &range[..]].concat()).unwrap()
                })
            }
            Rvalue::Pointer(PointerOp::Read, operands, _) => {
                // A copy of the value, which the memory keeps too.
                let address = self.scalar(&operands[0]);
                let value = self.e.value_at(address, ty);
                self.e.refcount(ty, value, false);
                value
            }
            Rvalue::Pointer(PointerOp::Take, operands, _) => {
                let address = self.scalar(&operands[0]);
                self.e.value_at(address, ty)
            }
            Rvalue::Pointer(PointerOp::Offset, operands, _) => {
                let TyKind::Pointer(element) = *self.e.cg.types().kind(ty) else { unreachable!("a pointer") };
                let (address, count) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let size = self.e.element_size(element);
                let bytes = self.e.fb.ins().imul(count, size);
                Val::Scalar(self.e.fb.ins().iadd(address, bytes))
            }
            Rvalue::Pointer(PointerOp::Write, ..) => unreachable!("a write gives `Unit`"),
            Rvalue::Length(collection) => {
                let collection_ty = self.operand_ty(collection);
                let value = self.scalar(collection);
                Val::Scalar(match self.e.cg.types().kind(self.e.cg.types().repr(collection_ty)) {
                    TyKind::Array(_) | TyKind::Deque(_) => self.e.fb.ins().load(types::I64, flags(), value, 8),
                    TyKind::String => self.e.call_rt(Rt::StrLength, &[value]).unwrap(),
                    _ => self.e.call_rt(Rt::MapLen, &[value]).unwrap(),
                })
            }
            Rvalue::StrOp(op, operands, span) => {
                let values: Vec<ir::Value> = operands.iter().map(|o| self.scalar(o)).collect();
                match op {
                    StrOp::At => {
                        let location = self.location(*span);
                        Val::Scalar(self.e.call_rt(Rt::StrAt, &[values[0], values[1], location]).unwrap())
                    }
                    StrOp::Bytes => Val::Scalar(self.e.call_rt(Rt::StrToBytes, &values).unwrap()),
                    StrOp::FromBytes => {
                        let text = self.e.call_rt(Rt::StrFromBytes, &values).unwrap();
                        match self.pointer_option(place, ty, Ty::STRING, text) {
                            Some(value) => value,
                            None => return,
                        }
                    }
                    StrOp::Get | StrOp::Find => {
                        // A position, or -1 for `none`.
                        let rt = if *op == StrOp::Get { Rt::StrGet } else { Rt::StrFind };
                        let found = self.e.call_rt(rt, &values).unwrap();
                        let present = self.e.fb.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, found, 0);
                        let base = self.pointer_to(place, true);
                        self.e.fb.ins().store(flags(), present, base, 0);
                        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                        let value = if *op == StrOp::Get { self.e.fb.ins().ireduce(types::I32, found) } else { found };
                        self.e.fb.ins().store(flags(), value, base, payload.offset as i32);
                        return;
                    }
                    StrOp::Contains | StrOp::StartsWith | StrOp::EndsWith => {
                        let test = match op {
                            StrOp::Contains => 0,
                            StrOp::StartsWith => 1,
                            _ => 2,
                        };
                        let test = self.e.fb.ins().iconst(types::I32, test);
                        Val::Scalar(self.e.call_rt(Rt::StrTest, &[values[0], values[1], test]).unwrap())
                    }
                    StrOp::LowerAscii | StrOp::UpperAscii => {
                        let transform = match op {
                            StrOp::LowerAscii => 1,
                            _ => 2,
                        };
                        let transform = self.e.fb.ins().iconst(types::I32, transform);
                        Val::Scalar(self.e.call_rt(Rt::StrTransform, &[values[0], transform]).unwrap())
                    }
                    StrOp::Replace => Val::Scalar(self.e.call_rt(Rt::StrReplace, &values).unwrap()),
                    StrOp::FromChars => Val::Scalar(self.e.call_rt(Rt::StrFromChars, &values).unwrap()),
                    StrOp::SliceChecked => {
                        let error = self.scratch(Ty::BYTE);
                        let slice = self.e.call_rt(Rt::StrSliceChecked, &[values[0], values[1], values[2], error]);
                        let code = self.e.fb.ins().load(types::I8, flags(), error, 0);
                        self.write_checked(place, ty, slice.unwrap(), code);
                        return;
                    }
                    StrOp::ByteLength => Val::Scalar(self.e.fb.ins().load(types::I64, flags(), values[0], 8)),
                    StrOp::Concat => Val::Scalar(self.e.call_rt(Rt::StrConcat, &values).unwrap()),
                    StrOp::Decode => Val::Scalar(self.e.call_rt(Rt::StrDecode, &values).unwrap()),
                    StrOp::Next => Val::Scalar(self.e.call_rt(Rt::StrNext, &values).unwrap()),
                }
            }
            Rvalue::MapOp(op, operands, _) => match op {
                MapOp::New => {
                    let (size, offset) = self.e.cg.entry_layout(ty);
                    let size = self.e.fb.ins().iconst(POINTER, size as i64);
                    let offset = self.e.fb.ins().iconst(POINTER, offset as i64);
                    Val::Scalar(self.e.call_rt(Rt::MapNew, &[size, offset]).unwrap())
                }
                MapOp::Contains | MapOp::Get => {
                    let map_ty = self.operand_ty(&operands[0]);
                    let (key, value) = self.e.cg.map_parts(map_ty);
                    let map = self.scalar(&operands[0]);
                    let key_value = self.operand(&operands[1]);
                    let key_address = self.in_memory(key, key_value);
                    let glue = self.e.map_glue_address(map_ty);
                    let found = self.e.call_rt(Rt::MapFind, &[map, key_address, glue]).unwrap();
                    let present = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, found, 0);
                    if *op == MapOp::Contains {
                        Val::Scalar(present)
                    } else {
                        // `some(value)` owns a copy of the stored value.
                        let base = self.pointer_to(place, true);
                        self.e.fb.ins().store(flags(), present, base, 0);
                        let (copy, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                        self.e.fb.ins().brif(present, copy, &[], done, &[]);
                        self.e.fb.switch_to_block(copy);
                        let stored = self.e.value_at(found, value);
                        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                        self.e.init_component(base, &payload, stored);
                        self.e.refcount(value, stored, false);
                        self.e.fb.ins().jump(done, &[]);
                        self.e.fb.switch_to_block(done);
                        return;
                    }
                }
            },
            Rvalue::Mutate(mutation, target, operands, retain, span) => {
                match self.mutate(*mutation, target, operands, retain, place, *span) {
                    Some(value) => value,
                    None => return,
                }
            }
            Rvalue::AssignSlice(..) => unreachable!("a slice assignment produces `Unit`"),
            Rvalue::CheckRegion(..) => unreachable!("a region check produces `Unit`"),
            Rvalue::CallIndirect(callee, args) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call_indirect(callee, args, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::Callback(target, closure) => {
                let adapter = self.e.cg.callback(*target, ty, *closure);
                let func = self.e.cg.declare_func_in_func(adapter, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::ExternFn(id) => {
                let declared = &self.e.cg.externs()[*id as usize];
                let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
                let signature = self.e.cg.c_signature(&params, result_ty);
                let (function, _) = self.e.cg.c_function(&symbol, &signature);
                let func = self.e.cg.declare_func_in_func(function, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::Fixed(value, span) => {
                let from = self.operand_ty(value);
                let types = self.e.cg.types();
                match (types.kind(types.repr(from)), types.kind(types.repr(ty))) {
                    (TyKind::Array(element), TyKind::CArray(_, count)) => {
                        // An array of another length has no place there (`P0006`).
                        let (element, count) = (*element, *count);
                        let array = self.scalar(value);
                        let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
                        let differs = self.e.fb.ins().icmp_imm_u(IntCC::NotEqual, length, count as i64);
                        self.panic_values_if(differs, 6, *span, |this| {
                            let count = this.e.fb.ins().iconst(types::I64, count as i64);
                            (0x140, length, count, ty)
                        });
                        let destination = self.pointer_to(place, true);
                        let source = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER);
                        let size = self.e.cg.layout(element).size * count;
                        self.e.copy_memory(destination, source, size);
                        return;
                    }
                    (TyKind::CArray(element, count), _) => {
                        let (element, count) = (*element, *count);
                        let Val::Memory(source) = self.operand(value) else { unreachable!("a C array is in memory") };
                        let size = self.e.cg.layout(element).size;
                        let size_value = self.e.fb.ins().iconst(POINTER, size as i64);
                        let count_value = self.e.fb.ins().iconst(POINTER, count as i64);
                        let array = self.e.call_rt(Rt::ArrayNew, &[size_value, count_value]).unwrap();
                        let destination = self.e.fb.ins().iadd_imm_s(array, ARRAY_HEADER);
                        self.e.copy_memory(destination, source, size * count);
                        Val::Scalar(array)
                    }
                    _ => unreachable!("between an array and a C array"),
                }
            }
            Rvalue::Reflect(op, ty, operands, retain) => {
                let identity = self.e.cg.identity(*ty);
                let global = self.e.cg.declare_data_in_func(identity, self.e.fb.func);
                let identity = self.e.fb.ins().symbol_value(POINTER, global);
                match op {
                    ReflectOp::Box => {
                        let value = self.operand(&operands[0]);
                        if *retain {
                            self.e.refcount(*ty, value, false);
                        }
                        let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(*ty).size as i64);
                        let release = self.e.glue_or_null(Glue::Release, *ty);
                        let visit = self.e.glue_or_null(Glue::Visit, *ty);
                        let object = self.e.call_rt(Rt::ReflectBox, &[size, release, identity, visit]).unwrap();
                        let address = self.e.fb.ins().iadd_imm_s(object, REFLECT_VALUE);
                        self.e.store_val(address, *ty, value);
                        Val::Scalar(object)
                    }
                    ReflectOp::Unbox => {
                        let object = self.scalar(&operands[0]);
                        let address = self.e.fb.ins().iadd_imm_s(object, REFLECT_VALUE);
                        let value = self.e.value_at(address, *ty);
                        // The box keeps its own.
                        self.e.refcount(*ty, value, false);
                        value
                    }
                    ReflectOp::Is => {
                        let object = self.scalar(&operands[0]);
                        Val::Scalar(self.e.call_rt(Rt::ReflectIs, &[object, identity]).unwrap())
                    }
                    // The identity holds the name, an immortal string.
                    ReflectOp::Name => Val::Scalar(self.e.fb.ins().load(POINTER, flags(), identity, 0)),
                    ReflectOp::Identity => Val::Scalar(identity),
                    ReflectOp::Describe => {
                        let value = self.operand(&operands[0]);
                        Val::Scalar(self.e.describe(*ty, value, true))
                    }
                    ReflectOp::Capabilities => {
                        let types = self.e.cg.types();
                        let bits = types.is_copy(*ty) as i64
                            | (types.is_equatable(*ty) as i64) << 1
                            | (types.is_key(*ty) as i64) << 2;
                        Val::Scalar(self.e.fb.ins().iconst(types::I64, bits))
                    }
                }
            }
            Rvalue::Layout(measured, align) => {
                let layout = self.e.cg.layout(*measured);
                let value = if *align { layout.align } else { layout.size };
                Val::Scalar(self.e.fb.ins().iconst(types::I64, value as i64))
            }
            Rvalue::FnValue(id) => {
                let data = self.e.cg.fn_value(*id, &self.bodies[id.0 as usize]);
                let global = self.e.cg.declare_data_in_func(data, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Rvalue::Erase(closure, body, retain) => {
                let env = self.operand_ty(closure);
                let value = self.operand(closure);
                if *retain {
                    self.e.refcount(env, value, false);
                }
                let code = self.e.cg.closure_code(*body);
                let code = self.e.cg.declare_func_in_func(code, self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, code);
                let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(env).size as i64);
                let release = self.e.glue_or_null(Glue::Release, env);
                let visit = self.e.glue_or_null(Glue::Visit, env);
                let object = self.e.call_rt(Rt::FnNew, &[code, size, release, visit]).unwrap();
                let captures = self.e.fb.ins().iadd_imm_s(object, FN_HEADER);
                self.e.store_val(captures, env, value);
                Val::Scalar(object)
            }
            Rvalue::TestMembers(members) => {
                self.test_members(members);
                Val::Zero
            }
            Rvalue::Spawn(body, retain) => {
                let TyKind::Fn(_, result, ..) = *self.e.cg.types().kind(self.operand_ty(body)) else {
                    unreachable!("a task runs a `fn()` value")
                };
                let object = self.scalar(body);
                if *retain {
                    self.e.call_rt(Rt::FnRetain, &[object]);
                }
                // How the body returns its result: see `ResultKind` in the runtime.
                let kind = match self.e.cg.class(result) {
                    Class::Zero => 0,
                    Class::Scalar(types::F64) => 2,
                    Class::Scalar(types::F32) => 3,
                    Class::Scalar(_) => 1,
                    Class::Memory => 4,
                };
                let layout = self.e.cg.layout(result);
                let size = if kind == 0 { 0 } else { layout.size };
                let kind = self.e.fb.ins().iconst(types::I8, kind);
                let size = self.e.fb.ins().iconst(POINTER, size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, result);
                let visit = self.e.glue_or_null(Glue::Visit, result);
                Val::Scalar(self.e.call_rt(Rt::Spawn, &[object, kind, size, align, release, visit]).unwrap())
            }
            Rvalue::Select(cases, blocking, span) => Val::Scalar(self.select(cases, *blocking, *span)),
            // A live build's patches link only the runtime's functions.
            Rvalue::Cancelled if self.e.cg.live() => Val::Scalar(self.e.call_rt(Rt::Cancelled, &[]).unwrap()),
            // After a statement whose panics jumped to the unwinding
            // themselves, nothing to ask.
            Rvalue::Unwinding if self.quiet_check => Val::Scalar(self.e.fb.ins().iconst(types::I8, 0)),
            Rvalue::Unwinding => {
                let id = self.e.cg.attention();
                let attention = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                let address = self.e.fb.ins().symbol_value(POINTER, attention);
                let reasons = self.e.fb.ins().load(types::I32, flags(), address, 0);
                let (ask, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                let answer = self.e.fb.append_block_param(done, types::I8);
                let none = self.e.fb.ins().iconst(types::I8, 0);
                self.e.fb.ins().brif(reasons, ask, &[], done, &[none.into()]);
                self.e.fb.set_cold_block(ask);
                self.e.fb.switch_to_block(ask);
                let asked = self.e.call_rt(Rt::Unwinding, &[]).unwrap();
                self.e.fb.ins().jump(done, &[asked.into()]);
                self.e.fb.switch_to_block(done);
                Val::Scalar(answer)
            }
            Rvalue::Cancelled => {
                // Without a reason to ask the runtime, nearly always, no call.
                let id = self.e.cg.attention();
                let attention = self.e.cg.declare_data_in_func(id, self.e.fb.func);
                let address = self.e.fb.ins().symbol_value(POINTER, attention);
                let reasons = self.e.fb.ins().load(types::I32, flags(), address, 0);
                let (ask, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                let answer = self.e.fb.append_block_param(done, types::I8);
                let none = self.e.fb.ins().iconst(types::I8, 0);
                self.e.fb.ins().brif(reasons, ask, &[], done, &[none.into()]);
                self.e.fb.set_cold_block(ask);
                self.e.fb.switch_to_block(ask);
                let asked = self.e.call_rt(Rt::Cancelled, &[]).unwrap();
                self.e.fb.ins().jump(done, &[asked.into()]);
                self.e.fb.switch_to_block(done);
                Val::Scalar(answer)
            }
            // What the probe of the line that starts here said; no line
            // starts here, no restart.
            Rvalue::Restart(point) => Val::Scalar(match self.probed.take() {
                Some(asked) => {
                    self.carrying = true;
                    if let Some((value, number)) = self.site_now {
                        self.point_sites.insert(*point, value);
                        self.e.cg.point_site(*point, number);
                    }
                    self.e.fb.ins().uextend(types::I64, asked)
                }
                None => self.e.fb.ins().iconst(types::I64, 0),
            }),
            Rvalue::Transfer(point) => Val::Scalar(self.transfer(*point)),
            Rvalue::OsrPoint => {
                let state = self.osr_state();
                Val::Scalar(self.e.fb.ins().load(types::I64, flags(), state, 0))
            }
            Rvalue::CallOsr(id, args, state) => {
                let result = if class == Class::Memory { Some(self.pointer_to(place, true)) } else { None };
                match self.call_osr(*id, args, state, result) {
                    Some(value) if result.is_none() => Val::Scalar(value),
                    _ => return,
                }
            }
            Rvalue::TaskOp(op, operands, retain, span) => match self.task_op(*op, operands, retain, place, *span) {
                Some(value) => value,
                None => return,
            },
            Rvalue::Discriminant(source) => {
                let source_ty = self.place_ty(source);
                let value = self.read(source);
                Val::Scalar(self.e.tag(source_ty, value))
            }
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Unary(op, operand) => Val::Scalar(self.unary(*op, operand, span)),
            Rvalue::Increment(operand) => {
                let value = self.scalar(operand);
                Val::Scalar(self.e.fb.ins().iadd_imm_s(value, 1))
            }
            Rvalue::Exact(op, left, right) => {
                let (a, b) = (self.scalar(left), self.scalar(right));
                let ins = self.e.fb.ins();
                Val::Scalar(match op {
                    BinOp::Add => ins.iadd(a, b),
                    BinOp::Sub => ins.isub(a, b),
                    BinOp::Div => ins.udiv(a, b),
                    BinOp::Rem => ins.urem(a, b),
                    _ => ins.imul(a, b),
                })
            }
            Rvalue::Binary(op, left, right) => Val::Scalar(self.binary(*op, left, right, span)),
            Rvalue::Concat(pieces) => Val::Scalar(self.concat(pieces)),
        };
        self.write(place, value);
    }

    /// An intrinsic array operation. Returns the value for scalar results;
    /// results in memory are written to `place` directly.
    fn array_op(&mut self, op: ArrayOp, operands: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        let types = self.e.cg.types();
        let element_of = |ty: Ty| match *types.kind(types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) => element,
            _ => unreachable!("not an array or deque"),
        };
        Some(match op {
            ArrayOp::New => {
                let size = self.e.element_size(element_of(ty));
                let zero = self.e.fb.ins().iconst(POINTER, 0);
                Val::Scalar(self.e.call_rt(Rt::ArrayNew, &[size, zero]).unwrap())
            }
            ArrayOp::WithCapacity => {
                let TyKind::Result(array_ty, _) = *types.kind(ty) else { unreachable!() };
                let capacity = self.scalar(&operands[0]);
                let size = self.e.element_size(element_of(array_ty));
                let zero = self.e.fb.ins().iconst(POINTER, 0);
                // The capacity is a hint; storage grows on demand.
                let array = self.e.call_rt(Rt::ArrayNew, &[size, zero]).unwrap();
                let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, capacity, 0);
                let invalid = self.e.fb.ins().iconst(types::I8, 0);
                let fine = self.e.fb.ins().iconst(types::I8, -1);
                let error = self.e.fb.ins().select(negative, invalid, fine);
                self.write_checked(place, ty, array, error);
                // On failure the unused array is released.
                let (release, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                self.e.fb.ins().brif(negative, release, &[], done, &[]);
                self.e.fb.switch_to_block(release);
                self.e.refcount(array_ty, Val::Scalar(array), true);
                self.e.fb.ins().jump(done, &[]);
                self.e.fb.switch_to_block(done);
                return None;
            }
            ArrayOp::Get => {
                let sequence_ty = self.operand_ty(&operands[0]);
                let element = element_of(sequence_ty);
                let sequence = self.scalar(&operands[0]);
                let index = self.scalar(&operands[1]);
                let (position, inside) = self.position(sequence, index);
                if self.e.cg.class(ty) != Class::Memory {
                    return Some(Val::Scalar(inside));
                }
                let base = self.pointer_to(place, true);
                self.e.fb.ins().store(flags(), inside, base, 0);
                let (found, done) = (self.e.fb.create_block(), self.e.fb.create_block());
                self.e.fb.ins().brif(inside, found, &[], done, &[]);
                self.e.fb.switch_to_block(found);
                let address = self.element_at(sequence, sequence_ty, position);
                let value = self.e.value_at(address, element);
                let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                self.e.init_component(base, &payload, value);
                // The option owns a copy of the element.
                self.e.refcount(element, value, false);
                self.e.fb.ins().jump(done, &[]);
                self.e.fb.switch_to_block(done);
                return None;
            }
            ArrayOp::SliceChecked => {
                let array_ty = self.operand_ty(&operands[0]);
                let element = element_of(array_ty);
                let array = self.scalar(&operands[0]);
                let (start, end) = (self.scalar(&operands[1]), self.scalar(&operands[2]));
                let size = self.e.element_size(element);
                let retain = self.e.glue_or_null(Glue::Retain, element);
                let slice = self.e.call_rt(Rt::ArraySliceChecked, &[array, size, retain, start, end]).unwrap();
                let failed = self.e.fb.ins().icmp_imm_s(IntCC::Equal, slice, 0);
                let invalid_index = self.e.fb.ins().iconst(types::I8, 1);
                let fine = self.e.fb.ins().iconst(types::I8, -1);
                let error = self.e.fb.ins().select(failed, invalid_index, fine);
                self.write_checked(place, ty, slice, error);
                return None;
            }
            ArrayOp::Concat | ArrayOp::Repeat => {
                let element = element_of(ty);
                let (a, b) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let size = self.e.element_size(element);
                let retain = self.e.glue_or_null(Glue::Retain, element);
                let location = self.location(span);
                let rt = if op == ArrayOp::Concat { Rt::ArrayConcat } else { Rt::ArrayRepeat };
                Val::Scalar(self.e.call_rt(rt, &[a, b, size, retain, location]).unwrap())
            }
            ArrayOp::Join => {
                let element = element_of(self.operand_ty(&operands[0]));
                let (array, separator) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let builder = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let size = self.e.element_size(element);
                let display = self.e.glue_address(Glue::Display, element);
                self.e.call_rt(Rt::SbJoin, &[builder, array, size, display, separator]);
                Val::Scalar(self.e.call_rt(Rt::SbFinish, &[builder]).unwrap())
            }
            ArrayOp::NativeOrder | ArrayOp::SortedNatively => {
                let element = self.e.cg.types().repr(element_of(self.operand_ty(&operands[0])));
                let rt = match element {
                    Ty::INT => Some(Rt::ArraySortedInts),
                    Ty::STRING => Some(Rt::ArraySortedStrings),
                    _ => None,
                };
                match (op, rt) {
                    (ArrayOp::NativeOrder, _) => Val::Scalar(self.e.fb.ins().iconst(types::I8, rt.is_some() as i64)),
                    (_, Some(rt)) => {
                        let array = self.scalar(&operands[0]);
                        Val::Scalar(self.e.call_rt(rt, &[array]).unwrap())
                    }
                    // Behind a `nativeOrder` that is false.
                    (_, None) => {
                        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
                        let block = self.e.fb.create_block();
                        self.e.fb.switch_to_block(block);
                        Val::Scalar(self.e.fb.ins().iconst(POINTER, 0))
                    }
                }
            }
            ArrayOp::Contains => {
                let element = self.operand_ty(&operands[0]);
                let value = self.operand(&operands[0]);
                let address = self.in_memory(element, value);
                let array = self.scalar(&operands[1]);
                let size = self.e.element_size(element);
                let eq = self.e.glue_address(Glue::Equal, element);
                Val::Scalar(self.e.call_rt(Rt::ArrayContains, &[array, address, size, eq]).unwrap())
            }
        })
    }

    /// A method that changes the collection at `target`. Returns the value
    /// for scalar results; results in memory are written to `place`.
    fn mutate(
        &mut self,
        mutation: Mutation,
        target: &Place,
        operands: &[Operand],
        retain: &[bool],
        place: &Place,
        span: Span,
    ) -> Option<Val> {
        let ty = self.place_ty(place);
        let collection_ty = self.place_ty(target);
        let values: Vec<Val> = operands.iter().map(|o| self.operand(o)).collect();
        for ((value, &retain), operand) in values.iter().zip(retain).zip(operands) {
            if retain {
                let ty = self.operand_ty(operand);
                self.e.refcount(ty, *value, false);
            }
        }
        if let TyKind::Map(..) | TyKind::Set(_) = self.e.cg.types().kind(self.e.cg.types().repr(collection_ty)) {
            return self.mutate_map(mutation, target, &values, collection_ty, place, ty);
        }
        let (element, deque) = self.e.cg.sequence_parts(collection_ty);
        if mutation == Mutation::Push && !deque {
            self.push(target, element, values[0]);
            return None;
        }
        let size = self.e.element_size(element);
        let retain_glue = self.e.glue_or_null(Glue::Retain, element);
        let release_glue = self.e.glue_or_null(Glue::Release, element);
        // An inserted value read in place may live in the storage the
        // mutation moves (`values.insert(0, values[1])`): copy it first.
        let source = match mutation {
            Mutation::Push | Mutation::PushFront => Some(self.copy_to_stack(element, values[0])),
            Mutation::InsertAt => Some(self.copy_to_stack(element, values[1])),
            _ => None,
        };
        let removes = matches!(mutation, Mutation::Pop | Mutation::PopFront | Mutation::RemoveAt);
        let out = removes.then(|| self.scratch(element));
        let (slot, spilled) = self.array_slot(target);
        let found = match (mutation, source, out) {
            (Mutation::Push, Some(source), _) if !deque => {
                self.e.call_rt(Rt::ArrayPush, &[slot, source, size, retain_glue, release_glue])
            }
            (Mutation::Push | Mutation::PushFront, Some(source), _) => {
                let front = self.e.fb.ins().iconst(types::I8, (mutation == Mutation::PushFront) as i64);
                self.e.call_rt(Rt::DequePush, &[slot, source, size, front, retain_glue, release_glue])
            }
            (Mutation::InsertAt, Some(source), _) => {
                let at = self.scalar_of(values[0]);
                let location = self.location(span);
                self.e.call_rt(Rt::ArrayInsert, &[slot, at, source, size, retain_glue, release_glue, location])
            }
            (Mutation::Pop, _, Some(out)) => {
                self.e.call_rt(Rt::ArrayPop, &[slot, out, size, retain_glue, release_glue])
            }
            (Mutation::PopFront, _, Some(out)) => {
                self.e.call_rt(Rt::DequePopFront, &[slot, out, size, retain_glue, release_glue])
            }
            (Mutation::RemoveAt, _, Some(out)) => {
                let at = self.scalar_of(values[0]);
                self.e.call_rt(Rt::ArrayRemoveAt, &[slot, at, out, size, retain_glue, release_glue])
            }
            (Mutation::Clear, ..) => self.e.call_rt(Rt::ArrayClear, &[slot, size, release_glue]),
            _ => unreachable!("not a sequence mutation"),
        };
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
        match (found, out) {
            (Some(found), Some(out)) => self.removal_result(place, ty, element, out, found),
            _ => None,
        }
    }

    /// `values.push(value)`: in place when the array is the only reference
    /// to its storage and has room, as nearly always; else by the runtime,
    /// which copies or grows it.
    fn push(&mut self, target: &Place, element: Ty, value: Val) {
        let var = match self.storage[target.local.0 as usize] {
            Storage::Var(var) if target.path.is_empty() => Some(var),
            _ => None,
        };
        let address = match var {
            Some(_) => None,
            None => Some(self.pointer_to(target, true)),
        };
        let array = match (var, address) {
            (Some(var), _) => self.e.fb.use_var(var),
            (None, Some(address)) => self.e.fb.ins().load(POINTER, flags(), address, 0),
            (None, None) => unreachable!(),
        };
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), array);
        let length = self.e.fb.ins().load(types::I64, flags(), array, 8);
        let capacity = self.e.fb.ins().load(types::I64, flags(), array, 16);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let room = self.e.fb.ins().icmp(IntCC::UnsignedLessThan, length, capacity);
        let in_place = self.e.fb.ins().band(unique, room);
        let (fast, slow, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(in_place, fast, &[], slow, &[]);
        self.e.fb.switch_to_block(fast);
        let size = self.e.cg.layout(element).size as i64;
        let offset = self.e.fb.ins().imul_imm_s(length, size);
        let at = self.e.fb.ins().iadd(array, offset);
        let at = self.e.fb.ins().iadd_imm_s(at, ARRAY_HEADER);
        self.e.store_val(at, element, value);
        let longer = self.e.fb.ins().iadd_imm_s(length, 1);
        self.e.fb.ins().store(flags(), longer, array, 8);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.set_cold_block(slow);
        self.e.fb.switch_to_block(slow);
        // The value may live in the storage the runtime moves.
        let source = self.copy_to_stack(element, value);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let slot = match (var, address) {
            (Some(var), _) => self.spill(var),
            (None, Some(address)) => address,
            (None, None) => unreachable!(),
        };
        self.e.call_rt(Rt::ArrayPush, &[slot, source, size, retain, release]);
        if let Some(var) = var {
            self.unspill(var, slot);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// The address of a copy of `value` on the stack.
    fn copy_to_stack(&mut self, ty: Ty, value: Val) -> ir::Value {
        let Val::Memory(source) = value else { return self.in_memory(ty, value) };
        let address = self.scratch(ty);
        let size = self.e.cg.layout(ty).size;
        self.e.copy_memory(address, source, size);
        address
    }

    /// `insert`, `remove` or `clear` on a map or set.
    fn mutate_map(
        &mut self,
        mutation: Mutation,
        target: &Place,
        values: &[Val],
        map_ty: Ty,
        place: &Place,
        ty: Ty,
    ) -> Option<Val> {
        let (key, value) = self.e.cg.map_parts(map_ty);
        let is_set = matches!(self.e.cg.types().kind(self.e.cg.types().repr(map_ty)), TyKind::Set(_));
        let glue = self.e.map_glue_address(map_ty);
        if mutation == Mutation::Clear {
            let (slot, spilled) = self.array_slot(target);
            self.e.call_rt(Rt::MapClear, &[slot, glue]);
            if let Some(var) = spilled {
                self.unspill(var, slot);
            }
            return None;
        }
        let key_address = self.in_memory(key, values[0]);
        // A map's result is `V?` with the replaced or removed value. A set's
        // result is a `Bool`.
        let out = if is_set { key_address } else { self.scratch(value) };
        let (slot, spilled) = self.array_slot(target);
        let flag = match mutation {
            Mutation::Insert => {
                let value_address = match values.get(1) {
                    Some(&v) => self.in_memory(value, v),
                    None => key_address,
                };
                self.e.call_rt(Rt::MapInsert, &[slot, key_address, value_address, out, glue]).unwrap()
            }
            _ => self.e.call_rt(Rt::MapRemove, &[slot, key_address, out, glue]).unwrap(),
        };
        if let Some(var) = spilled {
            self.unspill(var, slot);
        }
        match (is_set, mutation) {
            (false, _) => self.removal_result(place, ty, value, out, flag),
            // Set insert reports whether the value was new.
            (true, Mutation::Insert) => Some(Val::Scalar(self.e.fb.ins().bxor_imm_u(flag, 1))),
            (true, _) => Some(Val::Scalar(flag)),
        }
    }

    /// The `T?` result of a removal: `some` with the element the runtime
    /// moved to `out` when `found` is 1, otherwise `none`.
    /// An operation of a `Join` or a `Group` (see the runtime's `tasks`):
    /// the value to store at `place`, or `None` once it is stored.
    fn task_op(&mut self, op: TaskOp, operands: &[Operand], retain: &[bool], place: &Place, span: Span) -> Option<Val> {
        let ty = self.place_ty(place);
        match op {
            TaskOp::Shared(shared) => return self.shared_op(shared, operands, retain, place),
            TaskOp::Atomic(atomic) => return Some(self.atomic_op(atomic, operands, ty, span)),
            TaskOp::Zeroed => return Some(self.zeroed(ty)),
            TaskOp::SyncNew => {
                let TyKind::Handle(handle, state) = *self.e.cg.types().kind(ty) else { unreachable!() };
                if handle.collection() {
                    return Some(Val::Scalar(self.shared_new(handle, state, &operands[0], retain[0])));
                }
            }
            _ => {}
        }
        let handle_ty = operands.first().map(|o| self.operand_ty(o));
        let item = handle_ty.map(|t| self.e.cg.types().handle_result(t));
        // Handles and indices are scalars; the value `send` moves is read
        // where it is used.
        let scalars = match op {
            TaskOp::Send | TaskOp::Offer | TaskOp::TryOffer => &operands[..1],
            TaskOp::SyncNew | TaskOp::RefNew => &operands[..0],
            _ => operands,
        };
        let values: Vec<ir::Value> = scalars.iter().map(|o| self.scalar(o)).collect();
        match op {
            TaskOp::Wait => {
                let item = item.expect("a join");
                let out = self.scratch(item);
                self.e.call_rt(Rt::JoinWait, &[values[0], out]);
                Some(self.e.value_at(out, item))
            }
            TaskOp::GroupNew => {
                let result = self.e.cg.types().handle_result(ty);
                let fallible = matches!(self.e.cg.types().kind(result), TyKind::Result(..));
                let fallible = self.e.fb.ins().iconst(types::I8, fallible as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::GroupNew, &[fallible]).unwrap()))
            }
            TaskOp::GroupAdd => {
                if retain[1] {
                    self.e.call_rt(Rt::HandleRetain, &[values[1]]);
                }
                self.e.call_rt(Rt::GroupAdd, &[values[0], values[1]]);
                None
            }
            TaskOp::GroupNext | TaskOp::GroupTake => {
                let item = item.expect("a group");
                let out = self.scratch(item);
                let found = if op == TaskOp::GroupNext {
                    self.e.call_rt(Rt::GroupNext, &[values[0], out]).unwrap()
                } else {
                    self.e.call_rt(Rt::GroupTake, &[values[0], values[1], out]).unwrap()
                };
                self.removal_result(place, ty, item, out, found)
            }
            TaskOp::GroupLength => Some(Val::Scalar(self.e.call_rt(Rt::GroupLength, &[values[0]]).unwrap())),
            TaskOp::GroupSettle => Some(Val::Scalar(self.e.call_rt(Rt::GroupSettle, &[values[0]]).unwrap())),
            TaskOp::Channel => {
                let sender_ty = self.e.cg.types().components(ty).expect("a pair of ends")[0];
                let value = self.e.cg.types().handle_result(sender_ty);
                let discard = values[0];
                let (capacity, bounded) = match values.get(1) {
                    Some(&capacity) => (capacity, 1),
                    None => (self.e.fb.ins().iconst(types::I64, 0), 0),
                };
                let bounded = self.e.fb.ins().iconst(types::I8, bounded);
                let layout = self.e.cg.layout(value);
                let size = self.e.fb.ins().iconst(POINTER, layout.size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, value);
                let visit = self.e.glue_or_null(Glue::Visit, value);
                let location = self.location(span);
                let channel = self
                    .e
                    .call_rt(Rt::ChannelNew, &[capacity, bounded, size, align, release, visit, location, discard])
                    .unwrap();
                self.live_object(channel, value, 2);
                let sender = self.e.call_rt(Rt::SenderNew, &[channel]).unwrap();
                let pair = self.scratch(ty);
                for (index, end) in [sender, channel].into_iter().enumerate() {
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    self.e.fb.ins().store(flags(), end, pair, slot.offset as i32);
                }
                Some(Val::Memory(pair))
            }
            TaskOp::Send => {
                let item = item.expect("a sender");
                let value = self.operand(&operands[1]);
                if retain[1] {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                let location = self.location(span);
                self.e.call_rt(Rt::ChannelSend, &[values[0], source, location]);
                None
            }
            TaskOp::Offer | TaskOp::TryOffer => {
                let item = item.expect("a sender");
                let value = self.operand(&operands[1]);
                if retain[1] {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                let wait = self.e.fb.ins().iconst(types::I8, (op == TaskOp::Offer) as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelOffer, &[values[0], source, wait]).unwrap()))
            }
            TaskOp::Receive | TaskOp::ReceiveTimeout | TaskOp::WaitTimeout => {
                let item = item.expect("a receiver or a join");
                let out = self.scratch(item);
                let found = match op {
                    TaskOp::Receive => self.e.call_rt(Rt::ChannelReceive, &[values[0], out]),
                    TaskOp::ReceiveTimeout => self.e.call_rt(Rt::ReceiveTimeout, &[values[0], out, values[1]]),
                    _ => self.e.call_rt(Rt::JoinWaitTimeout, &[values[0], out, values[1]]),
                };
                self.removal_result(place, ty, item, out, found.unwrap())
            }
            TaskOp::Cancel => {
                self.e.call_rt(Rt::JoinCancel, &[values[0]]);
                None
            }
            TaskOp::Detach => {
                self.e.call_rt(Rt::JoinDetach, &[values[0]]);
                None
            }
            TaskOp::GroupCancel => {
                self.e.call_rt(Rt::GroupCancel, &[values[0]]);
                None
            }
            TaskOp::SyncNew => {
                // The state moves into the new cell.
                let state = self.e.cg.types().handle_result(ty);
                let value = self.operand(&operands[0]);
                if retain[0] {
                    self.e.refcount(state, value, false);
                }
                let layout = self.e.cg.layout(state);
                let size = self.e.fb.ins().iconst(POINTER, layout.size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, state);
                let visit = self.e.glue_or_null(Glue::Visit, state);
                let cell = self.e.call_rt(Rt::SyncNew, &[size, align, release, visit]).unwrap();
                let address = self.e.call_rt(Rt::SyncValue, &[cell]).unwrap();
                self.e.store_val(address, state, value);
                self.live_object(cell, state, 1);
                Some(Val::Scalar(cell))
            }
            TaskOp::WeakNew => Some(Val::Scalar(self.e.call_rt(Rt::WeakNew, &[values[0]]).unwrap())),
            TaskOp::WeakGet => {
                let reference = self.e.call_rt(Rt::WeakGet, &[values[0]]).unwrap();
                let reference_ty = self.e.cg.types().lookup(&TyKind::Handle(Handle::Ref, item.unwrap())).unwrap();
                self.pointer_option(place, ty, reference_ty, reference)
            }
            TaskOp::RefNew => {
                // The value moves into the new identity.
                let inner = self.e.cg.types().handle_result(ty);
                let value = self.operand(&operands[0]);
                if retain[0] {
                    self.e.refcount(inner, value, false);
                }
                let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(inner).size as i64);
                let object = self.e.call_rt(Rt::RefNew, &[size]).unwrap();
                let address = self.e.ref_value(object);
                self.e.store_val(address, inner, value);
                self.live_object(object, inner, 0);
                Some(Val::Scalar(object))
            }
            TaskOp::Lock(mode) => {
                let mode = match mode {
                    LockMode::Shared => 0,
                    LockMode::Exclusive => 1,
                    LockMode::Try => 2,
                };
                let mode = self.e.fb.ins().iconst(types::I8, mode);
                let location = self.location(span);
                Some(Val::Scalar(self.e.call_rt(Rt::SyncLock, &[values[0], mode, location]).unwrap()))
            }
            TaskOp::SyncAwait => Some(Val::Scalar(self.e.call_rt(Rt::SyncAwait, &[values[0]]).unwrap())),
            TaskOp::GuardHeld => Some(Val::Scalar(self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, values[0], 0))),
            TaskOp::SemaphoreNew | TaskOp::BarrierNew => {
                let location = self.location(span);
                let rt = if op == TaskOp::SemaphoreNew { Rt::SemaphoreNew } else { Rt::BarrierNew };
                Some(Val::Scalar(self.e.call_rt(rt, &[values[0], location]).unwrap()))
            }
            TaskOp::Acquire => {
                let wait = self.e.fb.ins().iconst(types::I8, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SemaphoreAcquire, &[values[0], wait]).unwrap()))
            }
            TaskOp::TryAcquire => {
                let only = self.e.fb.ins().iconst(types::I8, 1);
                let permit = self.e.call_rt(Rt::SemaphoreAcquire, &[values[0], only]).unwrap();
                let permit_ty = self.e.cg.types().lookup(&TyKind::Handle(Handle::Permit, Ty::UNIT)).unwrap();
                let out = self.scratch(permit_ty);
                self.e.fb.ins().store(flags(), permit, out, 0);
                let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, permit, 0);
                self.removal_result(place, ty, permit_ty, out, found)
            }
            // The permit moved into a temporary, whose release gives it back.
            TaskOp::PermitRelease => None,
            TaskOp::BarrierWait => Some(Val::Scalar(self.e.call_rt(Rt::BarrierWait, &[values[0]]).unwrap())),
            TaskOp::Close => {
                self.e.call_rt(Rt::ChannelClose, &[values[0]]);
                None
            }
            TaskOp::CopySender => Some(Val::Scalar(self.e.call_rt(Rt::SenderCopy, &[values[0]]).unwrap())),
            TaskOp::Abandoned => {
                let mode = self.e.fb.ins().iconst(types::I8, 2);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelClosed, &[values[0], mode]).unwrap()))
            }
            TaskOp::IsClosed => {
                let sender = matches!(self.e.cg.types().kind(handle_ty.unwrap()), TyKind::Handle(Handle::Sender, _));
                let sender = self.e.fb.ins().iconst(types::I8, sender as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelClosed, &[values[0], sender]).unwrap()))
            }
            TaskOp::GroupWait => unreachable!("lowered to a loop"),
            TaskOp::Shared(_) | TaskOp::Atomic(_) | TaskOp::Zeroed => unreachable!("handled above"),
        }
    }

    /// `select`: lays out one runtime `Case` per operation (see the
    /// runtime's `tasks`), waits, and stores the result of the operation
    /// that completed in its case's `out`. A value whose send did not
    /// complete is released. Returns the index of that operation.
    fn select(&mut self, cases: &[SelectCase], blocking: bool, span: Span) -> ir::Value {
        const CASE: u32 = 32;
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, CASE * cases.len() as u32, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let base = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let mut outs = Vec::new();
        let mut sent = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let at = (CASE * index as u32) as i32;
            let item = self.e.cg.types().handle_result(self.operand_ty(&case.operands[0]));
            let kind = match case.op {
                Some(TaskOp::Receive) => 0,
                Some(TaskOp::Send) => 1,
                Some(TaskOp::Wait) => 2,
                Some(TaskOp::GroupNext) => 3,
                _ => 4,
            };
            let kind_value = self.e.fb.ins().iconst(types::I8, kind);
            let zero = self.e.fb.ins().iconst(types::I8, 0);
            self.e.fb.ins().store(flags(), kind_value, base, at);
            self.e.fb.ins().store(flags(), zero, base, at + 1);
            let first = self.scalar(&case.operands[0]);
            if kind == 4 {
                let null = self.e.fb.ins().iconst(POINTER, 0);
                self.e.fb.ins().store(flags(), null, base, at + 8);
                self.e.fb.ins().store(flags(), first, base, at + 24);
                continue;
            }
            self.e.fb.ins().store(flags(), first, base, at + 8);
            let value = if kind == 1 {
                let value = self.operand(&case.operands[1]);
                if case.retain {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                if self.e.cg.types().needs_drop(item) {
                    sent.push((index, source, item));
                }
                source
            } else {
                let out = self.scratch(item);
                outs.push((index, case.out, kind, item, out));
                out
            };
            self.e.fb.ins().store(flags(), value, base, at + 16);
        }
        let count = self.e.fb.ins().iconst(POINTER, cases.len() as i64);
        let blocking = self.e.fb.ins().iconst(types::I8, blocking as i64);
        let location = self.location(span);
        let won = self.e.call_rt(Rt::Select, &[base, count, blocking, location]).unwrap();
        for (index, out, kind, item, scratch) in outs {
            let (this, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_this = self.e.fb.ins().icmp_imm_s(IntCC::Equal, won, index as i64);
            self.e.fb.ins().brif(is_this, this, &[], next, &[]);
            self.e.fb.switch_to_block(this);
            let place = Place::local(out);
            if kind == 2 {
                let value = self.e.value_at(scratch, item);
                self.write(&place, value);
            } else {
                let found = self.e.fb.ins().load(types::I8, flags(), base, (CASE * index as u32 + 1) as i32);
                let option = self.place_ty(&place);
                if let Some(value) = self.removal_result(&place, option, item, scratch, found) {
                    self.write(&place, value);
                }
            }
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        for (index, source, item) in sent {
            let (lost, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_this = self.e.fb.ins().icmp_imm_s(IntCC::Equal, won, index as i64);
            self.e.fb.ins().brif(is_this, next, &[], lost, &[]);
            self.e.fb.switch_to_block(lost);
            let value = self.e.value_at(source, item);
            self.e.refcount(item, value, true);
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        won
    }

    /// A value of zeros, which releases nothing.
    fn zeroed(&mut self, ty: Ty) -> Val {
        match self.e.cg.class(ty) {
            Class::Zero => Val::Zero,
            Class::Scalar(t) if t.is_float() => {
                let zero = self.e.fb.ins().f64const(0.0);
                Val::Scalar(if t == types::F32 { self.e.fb.ins().fdemote(types::F32, zero) } else { zero })
            }
            Class::Scalar(t) => Val::Scalar(self.e.int_const(t, 0)),
            Class::Memory => {
                let address = self.scratch(ty);
                let layout = self.e.cg.layout(ty);
                let config = self.e.cg.isa().frontend_config();
                let align = layout.align.clamp(1, 8) as u8;
                self.e.fb.emit_small_memset(config, address, 0, layout.size as u64, align, flags());
                Val::Memory(address)
            }
        }
    }

    /// The address of a value an operation reads, retained first when the
    /// operation takes it over and it was copied rather than moved.
    fn address_of(&mut self, operand: &Operand, retain: bool) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.operand(operand);
        if retain {
            self.e.refcount(ty, value, false);
        }
        self.copy_to_stack(ty, value)
    }

    /// A shared collection made from the value collection it takes over.
    fn shared_new(&mut self, handle: Handle, state: Ty, source: &Operand, retain: bool) -> ir::Value {
        let value = self.operand(source);
        if retain {
            self.e.refcount(state, value, false);
        }
        let pointer = match value {
            Val::Scalar(pointer) => pointer,
            _ => unreachable!("collections are pointers"),
        };
        if let Handle::SyncMap | Handle::SyncSet = handle {
            let (size, offset) = self.e.cg.entry_layout(state);
            let size = self.e.fb.ins().iconst(POINTER, size as i64);
            let offset = self.e.fb.ins().iconst(POINTER, offset as i64);
            let glue = self.e.map_glue_address(state);
            return self.e.call_rt(Rt::SharedMapFrom, &[pointer, size, offset, glue]).unwrap();
        }
        let (element, _) = self.e.cg.sequence_parts(state);
        let size = self.e.element_size(element);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        let visit = self.e.glue_or_null(Glue::Visit, element);
        let rt = match handle {
            Handle::SyncQueue => Rt::SharedQueueFrom,
            Handle::SyncStack => Rt::SharedStackFrom,
            _ => Rt::SharedArrayFrom,
        };
        self.e.call_rt(rt, &[pointer, size, retain, release, visit]).unwrap()
    }

    /// `present` and the address of the payload of an option value, or of
    /// scratch space when it has no payload.
    fn option_parts(&mut self, ty: Ty, value: Val) -> (ir::Value, ir::Value) {
        let TyKind::Option(payload) = *self.e.cg.types().kind(ty) else { unreachable!() };
        let tag = self.e.tag(ty, value);
        let present = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, tag, 0);
        let address = match value {
            Val::Memory(base) => {
                let slot = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                self.e.offset(base, slot.offset)
            }
            _ => self.scratch(payload),
        };
        let null = self.e.fb.ins().iconst(POINTER, 0);
        (present, self.e.fb.ins().select(present, address, null))
    }

    /// An operation of a shared collection (see the runtime's `shared`).
    fn shared_op(&mut self, op: SharedOp, operands: &[Operand], retain: &[bool], place: &Place) -> Option<Val> {
        let ty = self.place_ty(place);
        let TyKind::Handle(handle, state) = *self.e.cg.types().kind(self.operand_ty(&operands[0])) else {
            unreachable!("a shared collection")
        };
        let collection = self.scalar(&operands[0]);
        let map = matches!(handle, Handle::SyncMap | Handle::SyncSet);
        let (key, value) = if map { self.e.cg.map_parts(state) } else { (Ty::INT, self.e.cg.sequence_parts(state).0) };
        let rt = |op: SharedOp| match (handle, op) {
            (Handle::SyncMap | Handle::SyncSet, SharedOp::Length) => Rt::SharedMapLength,
            (Handle::SyncMap | Handle::SyncSet, _) => Rt::SharedMapSnapshot,
            (Handle::SyncQueue, SharedOp::Length) => Rt::SharedQueueLength,
            (Handle::SyncQueue, _) => Rt::SharedQueueSnapshot,
            (Handle::SyncStack, SharedOp::Length) => Rt::SharedStackLength,
            (Handle::SyncStack, _) => Rt::SharedStackSnapshot,
            (_, SharedOp::Length) => Rt::SharedArrayLength,
            _ => Rt::SharedArraySnapshot,
        };
        match op {
            SharedOp::Length | SharedOp::Snapshot => Some(Val::Scalar(self.e.call_rt(rt(op), &[collection]).unwrap())),
            SharedOp::Contains => {
                let key = self.address_of(&operands[1], false);
                let null = self.e.fb.ins().iconst(POINTER, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SharedMapGet, &[collection, key, null]).unwrap()))
            }
            SharedOp::Get | SharedOp::Pop | SharedOp::Remove if !(handle == Handle::SyncSet) => {
                let out = self.scratch(value);
                let found = match (handle, op) {
                    (Handle::SyncMap, SharedOp::Get) | (Handle::SyncMap, _) => {
                        let key = self.address_of(&operands[1], false);
                        let rt = if op == SharedOp::Get { Rt::SharedMapGet } else { Rt::SharedMapRemove };
                        self.e.call_rt(rt, &[collection, key, out])
                    }
                    (_, SharedOp::Pop) => {
                        let rt = if handle == Handle::SyncQueue { Rt::SharedQueuePop } else { Rt::SharedStackPop };
                        self.e.call_rt(rt, &[collection, out])
                    }
                    _ => {
                        let index = self.scalar(&operands[1]);
                        let rt = match handle {
                            Handle::SyncQueue => Rt::SharedQueueGet,
                            Handle::SyncStack => Rt::SharedStackGet,
                            _ => Rt::SharedArrayGet,
                        };
                        self.e.call_rt(rt, &[collection, index, out])
                    }
                };
                self.removal_result(place, ty, value, out, found.unwrap())
            }
            SharedOp::Remove => {
                let key = self.address_of(&operands[1], false);
                let null = self.e.fb.ins().iconst(POINTER, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SharedMapRemove, &[collection, key, null]).unwrap()))
            }
            SharedOp::Insert => {
                let key_address = self.address_of(&operands[1], retain[1]);
                let _ = key;
                if handle == Handle::SyncSet {
                    let unit = self.e.fb.ins().iconst(POINTER, 8);
                    let keep = self.e.fb.ins().iconst(types::I8, 1);
                    let present =
                        self.e.call_rt(Rt::SharedMapInsert, &[collection, key_address, unit, unit, keep]).unwrap();
                    return Some(Val::Scalar(self.e.fb.ins().icmp_imm_s(IntCC::Equal, present, 0)));
                }
                let value_address = self.address_of(&operands[2], retain[2]);
                let old = self.scratch(value);
                let keep = self.e.fb.ins().iconst(types::I8, 0);
                let present =
                    self.e.call_rt(Rt::SharedMapInsert, &[collection, key_address, value_address, old, keep]).unwrap();
                self.removal_result(place, ty, value, old, present)
            }
            SharedOp::Get | SharedOp::Pop => unreachable!("sets have no `get` or `pop`"),
            SharedOp::Push => {
                let address = self.address_of(&operands[1], retain[1]);
                let rt = if handle == Handle::SyncQueue { Rt::SharedQueuePush } else { Rt::SharedStackPush };
                self.e.call_rt(rt, &[collection, address]);
                None
            }
            SharedOp::Set => {
                let index = self.scalar(&operands[1]);
                let address = self.address_of(&operands[2], retain[2]);
                let old = self.scratch(value);
                let found = self.e.call_rt(Rt::SharedArraySet, &[collection, index, address, old]).unwrap();
                self.removal_result(place, ty, value, old, found)
            }
            SharedOp::Exchange => {
                let Operand::Copy(seen) = &operands[4] else { unreachable!("the place for the value seen") };
                // Zeros stay when nothing was seen; they release nothing.
                let out = self.scratch(value);
                let config = self.e.cg.isa().frontend_config();
                let size = self.e.cg.layout(value).size;
                self.e.fb.emit_small_memset(config, out, 0, size as u64, 8, flags());
                let equal = self.e.glue_address(Glue::Equal, value);
                let code = if map {
                    let key = self.address_of(&operands[1], false);
                    let expected_ty = self.operand_ty(&operands[2]);
                    let expected = self.operand(&operands[2]);
                    let (_, expected) = self.option_parts(expected_ty, expected);
                    let desired_ty = self.operand_ty(&operands[3]);
                    let desired = self.operand(&operands[3]);
                    if retain[3] {
                        self.e.refcount(desired_ty, desired, false);
                    }
                    let desired = match desired {
                        Val::Scalar(_) => self.in_memory(desired_ty, desired),
                        _ => self.copy_to_stack(desired_ty, desired),
                    };
                    let (_, desired) = self.option_parts(desired_ty, Val::Memory(desired));
                    self.e.call_rt(Rt::SharedMapExchange, &[collection, key, expected, desired, out, equal])
                } else {
                    let index = self.scalar(&operands[1]);
                    let expected = self.address_of(&operands[2], false);
                    let desired = self.address_of(&operands[3], retain[3]);
                    self.e.call_rt(Rt::SharedArrayExchange, &[collection, index, expected, desired, out, equal])
                };
                let value = self.e.value_at(out, value);
                self.write(seen, value);
                Some(Val::Scalar(self.e.fb.ins().uextend(types::I64, code.unwrap())))
            }
        }
    }

    /// An operation of an atomic integer, `Bool`, `Char` or `Byte`.
    fn atomic_op(&mut self, op: AtomicOp, operands: &[Operand], ty: Ty, span: Span) -> Val {
        let TyKind::Handle(_, state) = *self.e.cg.types().kind(self.operand_ty(&operands[0])) else { unreachable!() };
        let Class::Scalar(clif) = self.e.cg.class(state) else { unreachable!("a scalar") };
        let cell = self.scalar(&operands[0]);
        let width = self.e.fb.ins().iconst(types::I8, clif.bytes() as i64);
        let word = |this: &mut Self, operand: &Operand| {
            let value = this.scalar(operand);
            if clif == types::I64 { value } else { this.e.fb.ins().uextend(types::I64, value) }
        };
        let result = match op {
            AtomicOp::Load => self.e.call_rt(Rt::AtomicLoad, &[cell, width]),
            AtomicOp::Store => {
                let value = word(self, &operands[1]);
                self.e.call_rt(Rt::AtomicStore, &[cell, width, value]);
                return Val::Zero;
            }
            AtomicOp::Swap => {
                let value = word(self, &operands[1]);
                self.e.call_rt(Rt::AtomicSwap, &[cell, width, value])
            }
            AtomicOp::Exchange => {
                let expected = word(self, &operands[1]);
                let desired = word(self, &operands[2]);
                self.e.call_rt(Rt::AtomicExchange, &[cell, width, expected, desired])
            }
            AtomicOp::Add => {
                let delta = word(self, &operands[1]);
                let signed = self.e.cg.types().as_int(state).is_some_and(|i| i.signed());
                let signed = self.e.fb.ins().iconst(types::I8, signed as i64);
                let location = self.location(span);
                self.e.call_rt(Rt::AtomicAdd, &[cell, width, signed, delta, location])
            }
        };
        let _ = ty;
        let result = result.unwrap();
        Val::Scalar(if clif == types::I64 { result } else { self.e.fb.ins().ireduce(clif, result) })
    }

    fn removal_result(&mut self, place: &Place, ty: Ty, element: Ty, out: ir::Value, found: ir::Value) -> Option<Val> {
        if self.e.cg.class(ty) != Class::Memory {
            // The element has no size: the option is just its tag.
            return Some(Val::Scalar(found));
        }
        let base = self.pointer_to(place, true);
        self.e.fb.ins().store(flags(), found, base, 0);
        let (some, done) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(found, some, &[], done, &[]);
        self.e.fb.switch_to_block(some);
        let value = self.e.value_at(out, element);
        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
        self.e.init_component(base, &payload, value);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
        None
    }

    fn scalar_of(&mut self, value: Val) -> ir::Value {
        match value {
            Val::Scalar(value) => value,
            _ => unreachable!("expected a scalar"),
        }
    }

    /// The option of a value a runtime function gives as a pointer, null
    /// for `none`; stored at `place` unless returned.
    fn pointer_option(&mut self, place: &Place, ty: Ty, element: Ty, pointer: ir::Value) -> Option<Val> {
        let out = self.scratch(element);
        self.e.fb.ins().store(flags(), pointer, out, 0);
        let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, pointer, 0);
        self.removal_result(place, ty, element, out, found)
    }

    /// A call of `std.fs`, `std.io` or `console.flush` (see the runtime's
    /// `fs` and `io`): the runtime returns 0 with the value in `out`, or the
    /// index of the error variant plus one; this builds the `T ! E` at
    /// `place`.
    fn status_call(&mut self, rt: Rt, args: &[Operand], place: &Place) {
        let ty = self.place_ty(place);
        let mut values: Vec<ir::Value> = Vec::new();
        for arg in args {
            let value = self.scalar(arg);
            let value = if self.e.fb.func.dfg.value_type(value) == types::I64 {
                value
            } else {
                self.e.fb.ins().uextend(types::I64, value)
            };
            values.push(value);
        }
        let TyKind::Result(success, _) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
        // The value comes as one word.
        let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
        let returns_value = success != Ty::UNIT;
        if returns_value {
            values.push(out);
        }
        let status = self.e.call_rt(rt, &values).unwrap();
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.e.fb.ins().store(flags(), failed, base, 0);
        let (good, bad, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, bad, &[], good, &[]);
        self.e.fb.switch_to_block(bad);
        let error = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        let variant = self.e.fb.ins().iadd_imm_s(status, -1);
        let variant = self.e.fb.ins().ireduce(types::I8, variant);
        self.e.fb.ins().store(flags(), variant, base, error.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(good);
        if returns_value {
            let value = self.fs_value(success, out);
            let slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
            self.e.init_component(base, &slot, value);
        }
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// A call of `std.process`, its value when it is not built at `place`.
    fn process_call(&mut self, function: ProcessFn, args: &[Operand], place: &Place) -> Option<Val> {
        let ty = self.place_ty(place);
        let mut values: Vec<ir::Value> = args.iter().map(|a| self.scalar_or_address(a)).collect();
        let rt = Rt::Process(function);
        let flag = |this: &mut Self, on: bool| this.e.fb.ins().iconst(types::I8, on as i64);
        match function {
            ProcessFn::Configure(setting) => {
                let setting_value = self.e.fb.ins().iconst(types::I64, setting as i64);
                let zero = self.e.fb.ins().iconst(types::I64, 0);
                let (a, b) = match setting {
                    0 | 2 => (values[1], zero),
                    1 => (values[1], values[2]),
                    3 | 7 | 8 => (zero, zero),
                    // An `Input` or `Output`: its tag and the pointer its
                    // payload holds, the same place in every variant.
                    _ => {
                        let enum_ty = self.operand_ty(&args[1]);
                        let value = self.operand(&args[1]);
                        let tag = self.e.tag(enum_ty, value);
                        let tag = self.e.fb.ins().uextend(types::I64, tag);
                        let payload = match value {
                            Val::Memory(address) => {
                                let variants = self.e.cg.types().variants(enum_ty).unwrap();
                                let carrying = variants.iter().position(|v| !v.is_empty()).unwrap();
                                let slot = self.e.cg.slot(enum_ty, Proj::Variant(carrying as u32, 0));
                                self.e.fb.ins().load(types::I64, flags(), address, slot.offset as i32)
                            }
                            _ => zero,
                        };
                        (tag, payload)
                    }
                };
                let plan = values[0];
                return Some(Val::Scalar(self.e.call_rt(rt, &[plan, setting_value, a, b]).unwrap()));
            }
            ProcessFn::Command | ProcessFn::Shell | ProcessFn::Pipe | ProcessFn::Id => {
                return Some(Val::Scalar(self.e.call_rt(rt, &values).unwrap()));
            }
            ProcessFn::Signals => {
                // The array of signals, the stride of its elements, and the
                // size and alignment of the channel's values.
                let TyKind::Handle(_, signal) = *self.e.cg.types().kind(ty) else { unreachable!("a receiver") };
                let stride = self.e.element_size(signal);
                let layout = self.e.cg.layout(signal);
                let [size, align] =
                    [layout.size as i64, layout.align as i64].map(|value| self.e.fb.ins().iconst(types::I64, value));
                return Some(Val::Scalar(self.e.call_rt(rt, &[values[0], stride, size, align]).unwrap()));
            }
            ProcessFn::Stream(which) => {
                let which = self.e.fb.ins().iconst(types::I64, which as i64);
                return Some(Val::Scalar(self.e.call_rt(rt, &[values[0], which]).unwrap()));
            }
            ProcessFn::Text => {
                self.status_call(rt, args, place);
                return None;
            }
            ProcessFn::Run | ProcessFn::Status | ProcessFn::Wait | ProcessFn::Cancel => {
                let on = matches!(function, ProcessFn::Status | ProcessFn::Cancel);
                values.push(flag(self, on));
            }
            ProcessFn::Output | ProcessFn::Check | ProcessFn::Collect(_) => {
                let check = matches!(function, ProcessFn::Check | ProcessFn::Collect(true));
                values.push(flag(self, check));
                // Where the runtime writes each `ExitStatus` of the array.
                let TyKind::Result(result, _) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
                let statuses = self.e.cg.types().components(result).unwrap()[3];
                let TyKind::Array(status) = *self.e.cg.types().kind(statuses) else { unreachable!("an array") };
                let size = self.e.fb.ins().iconst(types::I64, self.e.cg.layout(status).size as i64);
                let success = self.e.cg.slot(status, Proj::Field(1)).offset;
                let success = self.e.fb.ins().iconst(types::I64, success as i64);
                values.extend([size, success]);
            }
            ProcessFn::Start => {}
            ProcessFn::Exit | ProcessFn::Detach | ProcessFn::Signal => unreachable!("they return nothing"),
        }
        self.process_result(rt, values, place);
        None
    }

    /// Calls a runtime function of `std.process` that writes up to six words
    /// at `out` and returns 0 or a `ProcessError` status, and builds the
    /// result at `place`: an `ExitStatus`, a `ProcessOutput` or a `Process`,
    /// or the error, with the status and output of `Failed`.
    fn process_result(&mut self, rt: Rt, mut values: Vec<ir::Value>, place: &Place) {
        const FAILED: i64 = 4;
        let ty = self.place_ty(place);
        let TyKind::Result(success, error) = *self.e.cg.types().kind(ty) else { unreachable!("a result") };
        let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3));
        let out = self.e.fb.ins().stack_addr(POINTER, out, 0);
        values.push(out);
        let status = self.e.call_rt(rt, &values).unwrap();
        let words: Vec<ir::Value> = (0..6).map(|i| self.e.fb.ins().load(types::I64, flags(), out, i * 8)).collect();
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.e.fb.ins().store(flags(), failed, base, 0);
        let (good, bad, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, bad, &[], good, &[]);
        self.e.fb.switch_to_block(bad);
        let value = self.scratch(error);
        let variant = self.e.fb.ins().iadd_imm_s(status, -1);
        let variant = self.e.fb.ins().ireduce(types::I8, variant);
        self.e.fb.ins().store(flags(), variant, value, 0);
        let output_ty = self.e.cg.types().variants(error).unwrap()[FAILED as usize - 1][1];
        if success == output_ty {
            let (failure, other) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_failure = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, FAILED);
            self.e.fb.ins().brif(is_failure, failure, &[], other, &[]);
            self.e.fb.switch_to_block(failure);
            let status_ty = self.e.cg.types().variants(error).unwrap()[FAILED as usize - 1][0];
            let exit = self.record_from_words(status_ty, &words[4..6]);
            let output = self.record_from_words(output_ty, &words[0..4]);
            for (index, component) in [exit, output].into_iter().enumerate() {
                let slot = self.e.cg.slot(error, Proj::Variant(FAILED as u32 - 1, index as u32));
                self.e.init_component(value, &slot, component);
            }
            self.e.fb.ins().jump(other, &[]);
            self.e.fb.switch_to_block(other);
        }
        let slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        self.e.init_component(base, &slot, Val::Memory(value));
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(good);
        let ok = match self.e.cg.types().components(success).map(|c| c.len()) {
            Some(count) => self.record_from_words(success, &words[0..count]),
            None => Val::Scalar(words[0]),
        };
        let slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
        self.e.init_component(base, &slot, ok);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    /// A record whose fields are scalars, from one word each.
    fn record_from_words(&mut self, ty: Ty, words: &[ir::Value]) -> Val {
        let address = self.scratch(ty);
        for (index, &word) in words.iter().enumerate() {
            let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
            let field = match self.e.cg.class(slot.ty) {
                Class::Scalar(t) if t != types::I64 => self.e.fb.ins().ireduce(t, word),
                _ => word,
            };
            self.e.init_component(address, &slot, Val::Scalar(field));
        }
        Val::Memory(address)
    }

    /// An operand as one word: a scalar, or the address of a value in memory.
    fn scalar_or_address(&mut self, operand: &Operand) -> ir::Value {
        match self.operand(operand) {
            Val::Scalar(value) | Val::Memory(value) => value,
            Val::Zero => self.e.fb.ins().iconst(types::I64, 0),
        }
    }

    /// The value a successful status call wrote at `out`.
    fn fs_value(&mut self, ty: Ty, out: ir::Value) -> Val {
        let word = |this: &mut Self, index: i32| this.e.fb.ins().load(types::I64, flags(), out, index * 8);
        match *self.e.cg.types().kind(ty) {
            // `String?` or `Array[Byte]?`: null for `none`.
            TyKind::Option(element) => {
                let pointer = word(self, 0);
                let address = self.scratch(ty);
                let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, pointer, 0);
                self.e.fb.ins().store(flags(), found, address, 0);
                let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
                let _ = element;
                self.e.fb.ins().store(flags(), pointer, address, payload.offset as i32);
                Val::Memory(address)
            }
            TyKind::Bool => {
                let word = word(self, 0);
                Val::Scalar(self.e.fb.ins().ireduce(types::I8, word))
            }
            _ => Val::Scalar(word(self, 0)),
        }
    }

    /// A conversion that cannot fail (18.6).
    fn convert_total(&mut self, value: ir::Value, from: Ty, to: Ty) -> ir::Value {
        let types = self.e.cg.types();
        let to_clif = self.e.cg.scalar_type(to);
        let from_clif = self.e.cg.scalar_type(from);
        match (types.kind(from), types.kind(to)) {
            (TyKind::Float(_), TyKind::Float(_)) if from_clif == to_clif => value,
            (TyKind::Float(_), TyKind::Float(_)) => self.e.fb.ins().fpromote(to_clif, value),
            (TyKind::Int(int), TyKind::Float(_)) if int.bits() == 128 => self.int128_to_float(value, *int, to_clif),
            (_, TyKind::Float(_)) if types.as_int(from).is_some_and(IntTy::signed) => {
                self.e.fb.ins().fcvt_from_sint(to_clif, value)
            }
            (_, TyKind::Float(_)) => self.e.fb.ins().fcvt_from_uint(to_clif, value),
            _ => self.resize(value, from, to_clif),
        }
    }

    /// An integer of 128 bits as a float of type `to`, by the runtime.
    fn int128_to_float(&mut self, value: ir::Value, int: IntTy, to: ir::Type) -> ir::Value {
        let (low, high) = self.e.fb.ins().isplit(value);
        let signed = self.e.fb.ins().iconst(types::I8, int.signed() as i64);
        let single = self.e.fb.ins().iconst(types::I8, (to == types::F32) as i64);
        let float = self.e.call_rt(Rt::Int128ToFloat, &[low, high, signed, single]).unwrap();
        if to == types::F32 { self.e.fb.ins().fdemote(types::F32, float) } else { float }
    }

    /// Calls the runtime with two integers of 128 bits in memory after
    /// `first`, and gives the integer of 128 bits it writes and what it
    /// returns.
    fn int128_call(
        &mut self,
        rt: Rt,
        first: &[ir::Value],
        a: ir::Value,
        b: ir::Value,
    ) -> (ir::Value, Option<ir::Value>) {
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_store(POINTER, a, slot, 0);
        self.e.fb.ins().stack_store(POINTER, b, slot, 16);
        let values = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let out = self.e.fb.ins().stack_addr(POINTER, slot, 32);
        let mut args = first.to_vec();
        args.extend([values, out]);
        let returned = self.e.call_rt(rt, &args);
        (self.e.fb.ins().stack_load(POINTER, types::I128, slot, 32), returned)
    }

    /// Resizes an integer bit pattern, extending by the source's signedness.
    fn resize(&mut self, value: ir::Value, from: Ty, to: ir::Type) -> ir::Value {
        let from_clif = self.e.cg.scalar_type(from);
        let signed = self.e.cg.types().as_int(from).is_some_and(IntTy::signed);
        if from_clif.bits() == to.bits() {
            value
        } else if from_clif.bits() > to.bits() {
            self.e.fb.ins().ireduce(to, value)
        } else if signed {
            self.e.fb.ins().sextend(to, value)
        } else {
            self.e.fb.ins().uextend(to, value)
        }
    }

    /// A conversion that can fail: the converted value (meaningful only on
    /// success) and the error variant, or -1 for success.
    fn convert_checked(&mut self, value: ir::Value, from: Ty, to: Ty) -> (ir::Value, ir::Value) {
        const OUT_OF_RANGE: i64 = 0;
        const NOT_FINITE: i64 = 1;
        const NOT_INTEGRAL: i64 = 2;
        let types = self.e.cg.types();
        let to_clif = self.e.cg.scalar_type(to);
        let int = |ty: Ty| match types.kind(ty) {
            TyKind::Int(int) => Some(*int),
            _ => None,
        };
        // A `Byte` has the range of a `UInt8`.
        let range = |ty: Ty| int(ty).map_or((0, 255), |int| (int.min(), int.max()));
        let ok = self.e.fb.ins().iconst(types::I8, -1);
        let pick = |e: &mut Emit<_>, failed: ir::Value, code: i64, current: ir::Value| {
            let code = e.fb.ins().iconst(types::I8, code);
            e.fb.ins().select(failed, code, current)
        };
        match types.kind(from) {
            TyKind::Float(_) if matches!(types.kind(to), TyKind::Float(_)) => {
                // Float64 to Float32: a finite value that rounds to infinity fails.
                let narrowed = self.e.fb.ins().fdemote(to_clif, value);
                let infinite_before = self.is_infinite(value, types::F64);
                let infinite_after = self.is_infinite(narrowed, types::F32);
                let finite_before = self.e.fb.ins().bxor_imm_u(infinite_before, 1);
                let overflowed = self.e.fb.ins().band(finite_before, infinite_after);
                let error = pick(&mut self.e, overflowed, OUT_OF_RANGE, ok);
                (narrowed, error)
            }
            TyKind::Int(source) if matches!(types.kind(to), TyKind::Float(_)) => {
                // The highest `UInt128`s round to infinity in a `Float32`.
                let source = *source;
                let converted = self.int128_to_float(value, source, to_clif);
                let infinite = self.is_infinite(converted, to_clif);
                (converted, pick(&mut self.e, infinite, OUT_OF_RANGE, ok))
            }
            TyKind::Float(_) => {
                // Classified in order: NotFinite, NotIntegral, OutOfRange (18.6).
                let from_clif = self.e.cg.scalar_type(from);
                let (min, _) = range(to);
                let signed = min < 0;
                let not_nan = self.e.fb.ins().fcmp(FloatCC::Equal, value, value);
                let infinite = self.is_infinite(value, from_clif);
                let finite = self.e.fb.ins().bxor_imm_u(infinite, 1);
                let finite = self.e.fb.ins().band(finite, not_nan);
                let truncated = self.e.fb.ins().trunc(value);
                let integral = self.e.fb.ins().fcmp(FloatCC::Equal, truncated, value);
                // `min` and `max + 1` are powers of two, exact in both
                // formats, or beyond a `Float32` as an infinity.
                let bits = to_clif.bits() as i32 - signed as i32;
                let low = self.float_const(from_clif, min as f64);
                let high = self.float_const(from_clif, 2f64.powi(bits));
                let below = self.e.fb.ins().fcmp(FloatCC::LessThan, value, low);
                let above = self.e.fb.ins().fcmp(FloatCC::GreaterThanOrEqual, value, high);
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                let not_integral = self.e.fb.ins().bxor_imm_u(integral, 1);
                let error = pick(&mut self.e, not_integral, NOT_INTEGRAL, error);
                let not_finite = self.e.fb.ins().bxor_imm_u(finite, 1);
                let error = pick(&mut self.e, not_finite, NOT_FINITE, error);
                // Convert a safe placeholder when the value is invalid.
                let invalid = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
                let zero = self.float_const(from_clif, 0.0);
                let safe = self.e.fb.ins().select(invalid, zero, value);
                if to_clif == types::I128 {
                    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                    let slot = self.e.fb.create_sized_stack_slot(data);
                    let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                    let safe = if from_clif == types::F32 { self.e.fb.ins().fpromote(types::F64, safe) } else { safe };
                    let signed = self.e.fb.ins().iconst(types::I8, signed as i64);
                    self.e.call_rt(Rt::FloatToInt128, &[safe, signed, out]);
                    return (self.e.fb.ins().stack_load(POINTER, types::I128, slot, 0), error);
                }
                // x86 converts to 32 or 64 bits only; the range is already checked.
                let wide = if to_clif.bits() < 32 { types::I32 } else { to_clif };
                let converted = if signed {
                    self.e.fb.ins().fcvt_to_sint(wide, safe)
                } else {
                    self.e.fb.ins().fcvt_to_uint(wide, safe)
                };
                let converted = if wide == to_clif { converted } else { self.e.fb.ins().ireduce(to_clif, converted) };
                (converted, error)
            }
            _ => {
                let (min, max) = range(to);
                let (from_min, from_max) = range(from);
                let signed = from_min < 0;
                // Compare in the source's domain, widened to 64 bits, or to
                // 128 when either side has them.
                let domain = if self.e.cg.scalar_type(from).bits() == 128 || to_clif.bits() == 128 {
                    types::I128
                } else {
                    types::I64
                };
                let wide = self.resize(value, from, domain);
                let below = if min > from_min {
                    let min = self.e.int_const(domain, min);
                    self.e.fb.ins().icmp(IntCC::SignedLessThan, wide, min)
                } else {
                    self.e.fb.ins().iconst(types::I8, 0)
                };
                // `max` is below the source's, so it fits in its domain.
                let above = if max < from_max {
                    let bound = self.e.int_const(domain, max as i128);
                    let cc = if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                    self.e.fb.ins().icmp(cc, wide, bound)
                } else {
                    self.e.fb.ins().iconst(types::I8, 0)
                };
                let outside = self.e.fb.ins().bor(below, above);
                let error = pick(&mut self.e, outside, OUT_OF_RANGE, ok);
                (self.resize(value, from, to_clif), error)
            }
        }
    }

    fn float_const(&mut self, ty: ir::Type, value: f64) -> ir::Value {
        if ty == types::F32 { self.e.fb.ins().f32const(value as f32) } else { self.e.fb.ins().f64const(value) }
    }

    fn is_infinite(&mut self, value: ir::Value, ty: ir::Type) -> ir::Value {
        let magnitude = self.e.fb.ins().fabs(value);
        let infinity = self.float_const(ty, f64::INFINITY);
        self.e.fb.ins().fcmp(FloatCC::Equal, magnitude, infinity)
    }

    /// Writes `some(value)` into `place` when `present`, or `none`.
    fn write_option(&mut self, place: &Place, ty: Ty, present: ir::Value, value: ir::Value) {
        let base = self.pointer_to(place, true);
        self.e.fb.ins().store(flags(), present, base, 0);
        let payload = self.e.cg.slot(ty, Proj::Variant(SOME, 0));
        self.e.fb.ins().store(flags(), value, base, payload.offset as i32);
    }

    /// An integer as one of `ty`, from 128 bits or to them.
    fn fit_width(&mut self, value: ir::Value, ty: ir::Type) -> ir::Value {
        match (self.e.fb.func.dfg.value_type(value).bits(), ty.bits()) {
            (from, to) if from == to => value,
            (from, to) if from > to => self.e.fb.ins().ireduce(ty, value),
            _ => self.e.fb.ins().uextend(ty, value),
        }
    }

    /// The zeros above the highest 1 of an integer of `bits` bits, as an
    /// `Int`; Cranelift counts 32 bits and more.
    fn leading_zeros(&mut self, value: ir::Value, bits: u32) -> ir::Value {
        if bits >= 32 {
            let zeros = self.e.fb.ins().clz(value);
            return self.fit_width(zeros, types::I64);
        }
        let wide = self.e.fb.ins().uextend(types::I32, value);
        let zeros = self.e.fb.ins().clz(wide);
        let zeros = self.e.fb.ins().iadd_imm_s(zeros, -(32 - i64::from(bits)));
        self.e.fb.ins().uextend(types::I64, zeros)
    }

    /// A named operation of a number (18.9) for `place`, of type `ty`, or
    /// `None` when it wrote the place itself.
    fn number(&mut self, op: NumOp, args: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        use NumOp::*;
        let types = self.e.cg.types();
        // The number the operation is of: its receiver's, or what it makes.
        let own = match op {
            Parse | FromBytes | Wrapping | Saturating | FromBits | FromInt | Least | Greatest => {
                match *types.kind(types.repr(ty)) {
                    TyKind::Result(ok, _) => ok,
                    _ => ty,
                }
            }
            _ => self.operand_ty(&args[0]),
        };
        let own = types.repr(own);
        let clif = self.e.cg.scalar_type(own);
        // A `Byte` has the bits of a `UInt8`.
        let int = match *types.kind(own) {
            TyKind::Int(int) => Some(int),
            TyKind::Byte => Some(IntTy::U8),
            _ => None,
        };
        let bits = clif.bits();
        let signed = int.is_some_and(IntTy::signed);
        let values: Vec<ir::Value> = args.iter().map(|a| self.scalar(a)).collect();
        let less = |this: &mut Self, a: ir::Value, b: ir::Value| match clif.is_float() {
            true => this.e.fb.ins().fcmp(FloatCC::LessThan, a, b),
            false => this.e.fb.ins().icmp(if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan }, a, b),
        };
        let wide128 = clif == types::I128;
        // The detail of a panic of the operation: its code, signedness and width.
        let detail = |code: u32| code << 16 | 1 << 9 | (signed as u32) << 8 | bits;
        let value = match op {
            Compare => {
                let (a, b) = (values[0], values[1]);
                let (lt, gt) = (less(self, a, b), less(self, b, a));
                let (lt, gt) = (self.e.fb.ins().uextend(types::I64, lt), self.e.fb.ins().uextend(types::I64, gt));
                let order = self.e.fb.ins().isub(gt, lt);
                if !clif.is_float() {
                    return Some(Val::Scalar(order));
                }
                // NaN after every other value, and equal to itself.
                let a_nan = self.e.fb.ins().fcmp(FloatCC::Unordered, a, a);
                let b_nan = self.e.fb.ins().fcmp(FloatCC::Unordered, b, b);
                let (a_nan, b_nan) =
                    (self.e.fb.ins().uextend(types::I64, a_nan), self.e.fb.ins().uextend(types::I64, b_nan));
                let order = self.e.fb.ins().iadd(order, a_nan);
                self.e.fb.ins().isub(order, b_nan)
            }
            Min => {
                let smaller = less(self, values[1], values[0]);
                self.e.fb.ins().select(smaller, values[1], values[0])
            }
            Max => {
                let larger = less(self, values[0], values[1]);
                self.e.fb.ins().select(larger, values[1], values[0])
            }
            Clamp => {
                let (a, low, high) = (values[0], values[1], values[2]);
                let below = less(self, a, low);
                let raised = self.e.fb.ins().select(below, low, a);
                let above = less(self, high, raised);
                self.e.fb.ins().select(above, high, raised)
            }
            Pow | WrappingPow | CheckedPow | SaturatingPow if int.is_some() => {
                let base = self.widest(values[0], signed);
                let exponent = self.e.fb.ins().sextend(types::I128, values[1]);
                let saturate = self.e.fb.ins().iconst(types::I8, (op == SaturatingPow) as i64);
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let width = self.e.fb.ins().iconst(types::I32, i64::from(bits));
                let (result, status) = self.int128_call(Rt::IntPow, &[saturate, signed_flag, width], base, exponent);
                let status = status.unwrap();
                let negative = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 2);
                self.panic_values_if(negative, 18, span, |_| (detail(8), values[0], values[1], own));
                let result = self.fit_width(result, clif);
                match op {
                    Pow => {
                        let overflow = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 1);
                        self.panic_values_if(overflow, 5, span, |_| (detail(8), values[0], values[1], own));
                        result
                    }
                    CheckedPow => {
                        let fits = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        self.write_option(place, ty, fits, result);
                        return None;
                    }
                    _ => result,
                }
            }
            Pow => {
                let float = |this: &mut Self, v: ir::Value| match clif == types::F32 {
                    true => this.e.fb.ins().fpromote(types::F64, v),
                    false => v,
                };
                let (a, b) = (float(self, values[0]), float(self, values[1]));
                let power = self.e.call_rt(Rt::MathPow, &[a, b]).unwrap();
                if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, power) } else { power }
            }
            WrappingPow | CheckedPow | SaturatingPow => unreachable!("of integers only"),
            Abs if clif.is_float() => self.e.fb.ins().fabs(values[0]),
            Abs => {
                let min = self.e.int_const(clif, int.unwrap().min());
                let least = self.e.fb.ins().icmp(IntCC::Equal, values[0], min);
                self.panic_values_if(least, 5, span, |_| (5 << 16 | 0x100 | bits, values[0], values[0], own));
                self.e.fb.ins().iabs(values[0])
            }
            WrappingAdd => self.e.fb.ins().iadd(values[0], values[1]),
            WrappingSub => self.e.fb.ins().isub(values[0], values[1]),
            WrappingMul => self.e.fb.ins().imul(values[0], values[1]),
            WrappingNeg => self.e.fb.ins().ineg(values[0]),
            CheckedAdd | CheckedSub | CheckedMul | SaturatingAdd | SaturatingSub | SaturatingMul | OverflowingAdd
            | OverflowingSub | OverflowingMul => {
                let (a, b) = (values[0], values[1]);
                let (result, overflow) = match (op, signed) {
                    (CheckedMul | SaturatingMul | OverflowingMul, _) if wide128 => {
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        let (product, overflow) = self.int128_call(Rt::Int128Multiply, &[signed_flag], a, b);
                        (product, overflow.unwrap())
                    }
                    (CheckedAdd | SaturatingAdd | OverflowingAdd, true) => self.e.fb.ins().sadd_overflow(a, b),
                    (CheckedAdd | SaturatingAdd | OverflowingAdd, false) => self.e.fb.ins().uadd_overflow(a, b),
                    (CheckedSub | SaturatingSub | OverflowingSub, true) => self.e.fb.ins().ssub_overflow(a, b),
                    (CheckedSub | SaturatingSub | OverflowingSub, false) => self.e.fb.ins().usub_overflow(a, b),
                    (_, true) => self.e.fb.ins().smul_overflow(a, b),
                    (_, false) => self.e.fb.ins().umul_overflow(a, b),
                };
                match op {
                    CheckedAdd | CheckedSub | CheckedMul => {
                        let fits = self.e.fb.ins().bxor_imm_u(overflow, 1);
                        self.write_option(place, ty, fits, result);
                        return None;
                    }
                    OverflowingAdd | OverflowingSub | OverflowingMul => {
                        let base = self.pointer_to(place, true);
                        let (first, second) = (self.e.cg.slot(ty, Proj::Field(0)), self.e.cg.slot(ty, Proj::Field(1)));
                        self.e.fb.ins().store(flags(), result, base, first.offset as i32);
                        self.e.fb.ins().store(flags(), overflow, base, second.offset as i32);
                        return None;
                    }
                    _ => {}
                }
                // The bound the exact result went past.
                let int = int.unwrap();
                let (min, max) = (self.e.int_const(clif, int.min()), self.e.int_const(clif, int.max() as i128));
                let bound = match (op, signed) {
                    (SaturatingSub, false) => min,
                    (_, false) => max,
                    (SaturatingMul, true) => {
                        let sign = self.e.fb.ins().bxor(a, b);
                        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, sign, 0);
                        self.e.fb.ins().select(negative, min, max)
                    }
                    _ => {
                        let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                        let (low, high) = if op == SaturatingAdd { (min, max) } else { (max, min) };
                        self.e.fb.ins().select(negative, low, high)
                    }
                };
                self.e.fb.ins().select(overflow, bound, result)
            }
            CheckedDiv | CheckedRem => {
                let (a, b) = (values[0], values[1]);
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                let failed = match signed {
                    true => {
                        let int = int.unwrap();
                        let (min, minus_one) = (self.e.int_const(clif, int.min()), self.e.int_const(clif, -1));
                        let is_min = self.e.fb.ins().icmp(IntCC::Equal, a, min);
                        let is_minus_one = self.e.fb.ins().icmp(IntCC::Equal, b, minus_one);
                        let both = self.e.fb.ins().band(is_min, is_minus_one);
                        self.e.fb.ins().bor(zero, both)
                    }
                    false => zero,
                };
                // A divisor that cannot fail; `MIN % -1` is 0, as `MIN % 1`.
                let one = self.e.int_const(clif, 1);
                let divisor = self.e.fb.ins().select(failed, one, b);
                let result = match (wide128, op, signed) {
                    (true, _, _) => {
                        let remainder = self.e.fb.ins().iconst(types::I8, (op == CheckedRem) as i64);
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        self.int128_call(Rt::Int128Divide, &[remainder, signed_flag], a, divisor).0
                    }
                    (_, CheckedDiv, true) => self.e.fb.ins().sdiv(a, divisor),
                    (_, CheckedDiv, false) => self.e.fb.ins().udiv(a, divisor),
                    (_, _, true) => self.e.fb.ins().srem(a, divisor),
                    (_, _, false) => self.e.fb.ins().urem(a, divisor),
                };
                let present = if op == CheckedRem {
                    self.e.fb.ins().bxor_imm_u(zero, 1)
                } else {
                    self.e.fb.ins().bxor_imm_u(failed, 1)
                };
                self.write_option(place, ty, present, result);
                return None;
            }
            CheckedNeg => {
                let zero = self.e.int_const(clif, 0);
                let (result, present) = match signed {
                    true => {
                        let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, values[0]);
                        (result, self.e.fb.ins().bxor_imm_u(overflow, 1))
                    }
                    false => (self.e.fb.ins().ineg(values[0]), self.e.fb.ins().icmp(IntCC::Equal, values[0], zero)),
                };
                self.write_option(place, ty, present, result);
                return None;
            }
            CountOnes => {
                let count = self.e.fb.ins().popcnt(values[0]);
                self.fit_width(count, types::I64)
            }
            LeadingZeros => self.leading_zeros(values[0], bits),
            TrailingZeros if bits < 32 => {
                // A 1 just above the value stops the count at its width.
                let wide = self.e.fb.ins().uextend(types::I32, values[0]);
                let wide = self.e.fb.ins().bor_imm_u(wide, 1 << bits);
                let zeros = self.e.fb.ins().ctz(wide);
                self.e.fb.ins().uextend(types::I64, zeros)
            }
            TrailingZeros => {
                let zeros = self.e.fb.ins().ctz(values[0]);
                self.fit_width(zeros, types::I64)
            }
            RotateLeft | RotateRight => {
                let count = self.e.fb.ins().band_imm_s(values[1], i64::from(bits) - 1);
                match op {
                    RotateLeft => self.e.fb.ins().rotl(values[0], count),
                    _ => self.e.fb.ins().rotr(values[0], count),
                }
            }
            ReverseBytes if bits == 8 => values[0],
            ReverseBytes => self.e.fb.ins().bswap(values[0]),
            ReverseBits => self.e.fb.ins().bitrev(values[0]),
            IsPowerOfTwo => {
                let ones = self.e.fb.ins().popcnt(values[0]);
                self.e.fb.ins().icmp_imm_s(IntCC::Equal, ones, 1)
            }
            NextPowerOfTwo => {
                // 1 for 0 and 1; otherwise the bit above the highest of `x - 1`.
                let at_most_one = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, values[0], 1);
                let two = self.e.int_const(clif, 2);
                let safe = self.e.fb.ins().select(at_most_one, two, values[0]);
                let below = self.e.fb.ins().iadd_imm_s(safe, -1);
                let zeros = self.leading_zeros(below, bits);
                let overflow = self.e.fb.ins().icmp_imm_s(IntCC::Equal, zeros, 0);
                self.panic_values_if(overflow, 5, span, |_| (detail(9), values[0], values[0], own));
                let width = self.e.fb.ins().iconst(types::I64, i64::from(bits));
                let shift = self.e.fb.ins().isub(width, zeros);
                let one = self.e.int_const(clif, 1);
                let power = self.e.fb.ins().ishl(one, shift);
                self.e.fb.ins().select(at_most_one, one, power)
            }
            ToBytes => {
                let value = self.widest(values[0], signed);
                let (low, high) = self.e.fb.ins().isplit(value);
                let count = self.e.fb.ins().iconst(types::I64, i64::from(bits / 8));
                let big = self.e.fb.ins().icmp_imm_s(IntCC::Equal, values[1], 0);
                self.e.call_rt(Rt::IntToBytes, &[low, high, count, big]).unwrap()
            }
            FromBytes => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let slot = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let count = self.e.fb.ins().iconst(types::I64, i64::from(bits / 8));
                let big = self.e.fb.ins().icmp_imm_s(IntCC::Equal, values[2], 0);
                let location = self.location(span);
                self.e.call_rt(Rt::IntFromBytes, &[values[0], values[1], count, big, location, slot]);
                let read = self.e.fb.ins().load(types::I128, flags(), slot, 0);
                self.fit_width(read, clif)
            }
            Wrapping => {
                let from = self.operand_ty(&args[0]);
                self.resize(values[0], from, clif)
            }
            Saturating => {
                let from = self.e.cg.types().repr(self.operand_ty(&args[0]));
                let int = int.unwrap();
                let source = self.e.cg.scalar_type(from);
                if source.is_float() {
                    let value =
                        if source == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                    if wide128 {
                        // Rust's conversion saturates, and gives 0 for NaN.
                        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                        let slot = self.e.fb.create_sized_stack_slot(data);
                        let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                        let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                        self.e.call_rt(Rt::FloatToInt128, &[value, signed_flag, out]);
                        return Some(Val::Scalar(self.e.fb.ins().load(types::I128, flags(), out, 0)));
                    }
                    let wide = if bits < 32 { types::I64 } else { clif };
                    let converted = match signed {
                        true => self.e.fb.ins().fcvt_to_sint_sat(wide, value),
                        false => self.e.fb.ins().fcvt_to_uint_sat(wide, value),
                    };
                    if wide == clif {
                        return Some(Val::Scalar(converted));
                    }
                    let (min, max) = (
                        self.e.fb.ins().iconst(wide, int.min() as i64),
                        self.e.fb.ins().iconst(wide, int.max() as i64),
                    );
                    let below = self.e.fb.ins().icmp(IntCC::SignedLessThan, converted, min);
                    let raised = self.e.fb.ins().select(below, min, converted);
                    let above = self.e.fb.ins().icmp(IntCC::SignedGreaterThan, raised, max);
                    let clamped = self.e.fb.ins().select(above, max, raised);
                    return Some(Val::Scalar(self.e.fb.ins().ireduce(clif, clamped)));
                }
                // Compare in the source's domain, as a checked conversion does.
                let from_int = match *self.e.cg.types().kind(from) {
                    TyKind::Int(from) => from,
                    _ => IntTy::U8,
                };
                let domain = if source.bits() == 128 || bits == 128 { types::I128 } else { types::I64 };
                let wide = self.resize(values[0], from, domain);
                let mut result = self.resize(values[0], from, clif);
                if int.min() > from_int.min() {
                    let bound = self.e.int_const(domain, int.min());
                    let below = self.e.fb.ins().icmp(IntCC::SignedLessThan, wide, bound);
                    let min = self.e.int_const(clif, int.min());
                    result = self.e.fb.ins().select(below, min, result);
                }
                if int.max() < from_int.max() {
                    let bound = self.e.int_const(domain, int.max() as i128);
                    let cc = if from_int.signed() { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                    let above = self.e.fb.ins().icmp(cc, wide, bound);
                    let max = self.e.int_const(clif, int.max() as i128);
                    result = self.e.fb.ins().select(above, max, result);
                }
                result
            }
            Parse if clif.is_float() => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let single = self.e.fb.ins().iconst(types::I8, (clif == types::F32) as i64);
                let ok = self.e.call_rt(Rt::FloatParse, &[values[0], single, out]).unwrap();
                let read = self.e.fb.ins().load(types::F64, flags(), out, 0);
                let read = if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, read) } else { read };
                let (fine, invalid) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 3));
                let error = self.e.fb.ins().select(ok, fine, invalid);
                self.write_checked(place, ty, read, error);
                return None;
            }
            Parse => {
                let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
                let slot = self.e.fb.create_sized_stack_slot(data);
                let out = self.e.fb.ins().stack_addr(POINTER, slot, 0);
                let width = self.e.fb.ins().iconst(types::I32, i64::from(bits));
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let ok = self.e.call_rt(Rt::IntParse, &[values[0], values[1], width, signed_flag, out]).unwrap();
                let read = self.e.fb.ins().load(types::I128, flags(), out, 0);
                let read = self.fit_width(read, clif);
                // `TextError.InvalidData`, the fourth variant.
                let (fine, invalid) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 3));
                let error = self.e.fb.ins().select(ok, fine, invalid);
                self.write_checked(place, ty, read, error);
                return None;
            }
            ToString => {
                let value = self.widest(values[0], signed);
                let (low, high) = self.e.fb.ins().isplit(value);
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                self.e.call_rt(Rt::IntRadix, &[low, high, signed_flag, values[1]]).unwrap()
            }
            Floor => self.e.fb.ins().floor(values[0]),
            Ceil => self.e.fb.ins().ceil(values[0]),
            Round => self.e.fb.ins().nearest(values[0]),
            Truncate => self.e.fb.ins().trunc(values[0]),
            RoundTiesAway => {
                let value =
                    if clif == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                let rounded = self.e.call_rt(Rt::MathRoundAway, &[value]).unwrap();
                if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, rounded) } else { rounded }
            }
            Sqrt => {
                let zero = self.float_const(clif, 0.0);
                let negative = self.e.fb.ins().fcmp(FloatCC::LessThan, values[0], zero);
                let root = self.e.fb.ins().sqrt(values[0]);
                // `MathError.Domain`, its only variant.
                let (fine, domain) = (self.e.fb.ins().iconst(types::I8, -1), self.e.fb.ins().iconst(types::I8, 0));
                let error = self.e.fb.ins().select(negative, domain, fine);
                self.write_checked(place, ty, root, error);
                return None;
            }
            IsNaN => self.e.fb.ins().fcmp(FloatCC::Unordered, values[0], values[0]),
            IsInfinite => self.is_infinite(values[0], clif),
            IsFinite => {
                let magnitude = self.e.fb.ins().fabs(values[0]);
                let infinity = self.float_const(clif, f64::INFINITY);
                self.e.fb.ins().fcmp(FloatCC::LessThan, magnitude, infinity)
            }
            ToFixed => {
                let value =
                    if clif == types::F32 { self.e.fb.ins().fpromote(types::F64, values[0]) } else { values[0] };
                self.e.call_rt(Rt::TextFixed, &[value, values[1]]).unwrap()
            }
            Bits => {
                let pattern = if clif == types::F32 { types::I32 } else { types::I64 };
                self.e.fb.ins().bitcast(pattern, MemFlagsData::new(), values[0])
            }
            FromBits => self.e.fb.ins().bitcast(clif, MemFlagsData::new(), values[0]),
            FromInt => {
                let (converted, error) = self.convert_checked(values[0], Ty::INT, own);
                let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
                let detail = 10 << 16 | 0x100 | bits;
                self.panic_values_if(failed, 5, span, |_| (detail, values[0], values[0], own));
                converted
            }
            Least => self.e.int_const(clif, int.unwrap().min()),
            Greatest => self.e.int_const(clif, int.unwrap().max() as i128),
            Width => self.e.fb.ins().iconst(types::I64, i64::from(bits)),
        };
        Some(Val::Scalar(value))
    }

    /// Writes `ok(value)` or `err(NumericConversionError)` into `place`.
    fn write_checked(&mut self, place: &Place, ty: Ty, value: ir::Value, error: ir::Value) {
        let base = self.pointer_to(place, true);
        let failed = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, error, -1);
        self.e.fb.ins().store(flags(), failed, base, 0);
        // Both payloads share the same storage: write only the active one.
        let (failure, success, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.ins().brif(failed, failure, &[], success, &[]);
        self.e.fb.switch_to_block(failure);
        let err_slot = self.e.cg.slot(ty, Proj::Variant(ERR, 0));
        self.e.fb.ins().store(flags(), error, base, err_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(success);
        let ok_slot = self.e.cg.slot(ty, Proj::Variant(OK, 0));
        self.e.fb.ins().store(flags(), value, base, ok_slot.offset as i32);
        self.e.fb.ins().jump(done, &[]);
        self.e.fb.switch_to_block(done);
    }

    fn call(&mut self, callee: Callee, args: &[Operand], result: Option<ir::Value>) -> Option<ir::Value> {
        if let Callee::Extern(id) = callee {
            let declared = &self.e.cg.externs()[id as usize];
            let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
            return self.c_call(CTarget::Symbol(symbol), &params, result_ty, args, result);
        }
        // A frame that moves passes its parameters, and where the result
        // goes, from where the runtime says, a word each.
        let moved = self.osr_params.take();
        let result = match (moved, result) {
            (Some(base), Some(_)) => Some(self.e.fb.ins().load(POINTER, flags(), base, 0)),
            _ => result,
        };
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        let moved_word = |fb: &mut FunctionBuilder, index: usize, ty: ir::Type| {
            moved.map(|base| fb.ins().load(ty, flags(), base, (index as i32 + 1) * 8))
        };
        let loans = match callee {
            Callee::Fn(id) => self.e.cg.loans(id),
            Callee::Std(_) => Vec::new(),
            Callee::Trait(..) | Callee::Extern(_) => unreachable!("handled before"),
        };
        let mut spilled = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if loans.get(index).copied().unwrap_or(false) {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                let address = self.loan_address(place, &mut spilled);
                values.push(moved_word(&mut self.e.fb, index, POINTER).unwrap_or(address));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) => {
                    let ty = self.e.fb.func.dfg.value_type(value);
                    values.push(moved_word(&mut self.e.fb, index, ty).unwrap_or(value));
                }
                Val::Memory(value) => values.push(moved_word(&mut self.e.fb, index, POINTER).unwrap_or(value)),
                Val::Zero => {}
            }
        }
        let result = match callee {
            Callee::Fn(id) => {
                let state = self.osr_arg.take();
                match self.unwind_to.filter(|_| state.is_none() && self.e.cg.slot_of(id).is_none()) {
                    Some(unwind) => {
                        let func = self.e.cg.function(id);
                        let callee = self.e.cg.declare_func_in_func(func, self.e.fb.func);
                        self.try_call(Called::Direct(callee), &values, unwind, &spilled)
                    }
                    None => {
                        let call = call_body_with(self.e.cg, &mut self.e.fb, id, &values, state);
                        self.e.fb.inst_results(call).first().copied()
                    }
                }
            }
            Callee::Std(StdFn::ConsolePrint) => self.e.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.e.call_rt(Rt::ConsolePrintln, &values),
            Callee::Std(StdFn::ConsoleSet(index)) => {
                let index = self.e.fb.ins().iconst(types::I64, index as i64);
                self.e.call_rt(Rt::ConsoleSet, &[index, values[0]])
            }
            Callee::Std(StdFn::Number(_)) => unreachable!("assigned by `number`"),
            Callee::Std(StdFn::Native(index)) => {
                // Arithmetic the processor does in one instruction.
                let ins = self.e.fb.ins();
                match crate::hir::NATIVES[index as usize].symbol {
                    "tondo_rt_math_sqrt" => Some(ins.sqrt(values[0])),
                    "tondo_rt_math_floor" => Some(ins.floor(values[0])),
                    "tondo_rt_math_ceil" => Some(ins.ceil(values[0])),
                    "tondo_rt_math_trunc" => Some(ins.trunc(values[0])),
                    "tondo_rt_math_round" => Some(ins.nearest(values[0])),
                    _ => self.e.call_rt(Rt::Native(index), &values),
                }
            }
            Callee::Std(StdFn::Process(ProcessFn::Exit)) => self.e.call_rt(Rt::Process(ProcessFn::Exit), &values),
            Callee::Std(StdFn::Process(ProcessFn::Detach)) => self.e.call_rt(Rt::Process(ProcessFn::Detach), &values),
            Callee::Std(StdFn::Process(ProcessFn::Signal)) => {
                // The variant of `process.Signal`.
                let signal = self.operand_ty(&args[1]);
                let value = self.operand(&args[1]);
                let tag = self.e.tag(signal, value);
                let tag = self.e.fb.ins().uextend(types::I64, tag);
                self.e.call_rt(Rt::Process(ProcessFn::Signal), &[values[0], tag])
            }
            Callee::Std(StdFn::ConsoleEprint) => self.e.call_rt(Rt::ConsoleEprint, &values),
            Callee::Std(StdFn::ConsoleEprintln) => self.e.call_rt(Rt::ConsoleEprintln, &values),
            Callee::Std(StdFn::Io(IoFn::BufferClear)) => self.e.call_rt(Rt::Io(IoFn::BufferClear), &values),
            Callee::Std(StdFn::TimeSleep) => self.e.call_rt(Rt::Sleep, &values),
            Callee::Std(StdFn::TestLog) => self.e.call_rt(Rt::TestLog, &values),
            Callee::Std(StdFn::TestTags) => self.e.call_rt(Rt::TestTags, &values),
            Callee::Std(StdFn::TestWhere) => {
                let location = self.location(self.span);
                self.e.call_rt(Rt::TestWhere, &[location])
            }
            Callee::Std(StdFn::PanicUnwind(code)) => {
                let code = self.e.fb.ins().iconst(types::I32, i64::from(code));
                let location = self.location(self.span);
                values.insert(0, code);
                values.push(location);
                self.e.call_rt(Rt::PanicUnwind, &values)
            }
            Callee::Std(StdFn::ContextGet(index)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                self.e.call_rt(Rt::ContextGet, &[index])
            }
            Callee::Std(StdFn::ContextField(index, field)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                let field = self.e.fb.ins().iconst(types::I64, i64::from(field));
                let location = self.location(self.span);
                self.e.call_rt(Rt::ContextField, &[index, field, location])
            }
            Callee::Std(StdFn::ContextSet(index, fields)) => {
                let index = self.e.fb.ins().iconst(types::I64, i64::from(index));
                let fields = self.e.fb.ins().iconst(types::I64, fields as i64);
                // Where it is, for the tools that show which `with` set a
                // field (toolchain 7.6).
                let at = format!("{} in {}", self.e.cg.sources().location(self.span), self.body.name);
                let at = self.e.string_ptr(&at);
                self.e.call_rt(Rt::ContextSet, &[index, values[0], fields, at])
            }
            Callee::Std(StdFn::ContextRestore) => self.e.call_rt(Rt::ContextRestore, &values),
            Callee::Std(StdFn::RecoverEnter) => self.e.call_rt(Rt::RecoverEnter, &[]),
            Callee::Std(StdFn::RecoverEnd) => self.e.call_rt(Rt::RecoverEnd, &[]),
            Callee::Std(StdFn::RecoverLeave) => self.e.call_rt(Rt::RecoverLeave, &[]),
            Callee::Std(StdFn::RecoverCatch) => self.e.call_rt(Rt::RecoverCatch, &[]),
            Callee::Std(StdFn::WithinEnter) => self.e.call_rt(Rt::WithinEnter, &values),
            Callee::Std(StdFn::SpawnDetached) => self.e.call_rt(Rt::SpawnDetached, &[]),
            Callee::Std(StdFn::WithinLeave) => self.e.call_rt(Rt::WithinLeave, &[]),
            Callee::Std(StdFn::WithinCatch) => self.e.call_rt(Rt::WithinCatch, &[]),
            Callee::Std(StdFn::UnwindHold) => self.e.call_rt(Rt::UnwindHold, &[]),
            // A function that returns from here goes to its caller's way,
            // through its frame's return address (15.9).
            Callee::Std(StdFn::UnwindResume) if self.returns => self.e.call_rt(Rt::UnwindReturn, &[]),
            Callee::Std(StdFn::UnwindResume) => self.e.call_rt(Rt::UnwindResume, &[]),
            Callee::Std(StdFn::DeferEnd) => self.e.call_rt(Rt::DeferEnd, &[]),
            Callee::Std(StdFn::CancelReturn) => self.e.call_rt(Rt::CancelReturn, &[]),
            Callee::Std(StdFn::ClockBegin) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::ClockBegin, &values)
            }
            Callee::Std(StdFn::ClockEnd) => self.e.call_rt(Rt::ClockEnd, &values),
            Callee::Std(StdFn::BenchStart) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::BenchStart, &values)
            }
            Callee::Std(StdFn::BenchNext) => self.e.call_rt(Rt::BenchNext, &values),
            Callee::Std(StdFn::BenchEnd) => self.e.call_rt(Rt::BenchEnd, &values),
            Callee::Std(StdFn::Keep) => {
                // The value as a word the runtime is given, so that its
                // computation stays.
                let fb = &mut self.e.fb;
                let word = match values.first().copied() {
                    None => fb.ins().iconst(types::I64, 0),
                    Some(value) => match fb.func.dfg.value_type(value) {
                        types::I64 => value,
                        types::F64 => fb.ins().bitcast(types::I64, MemFlagsData::new(), value),
                        types::F32 => {
                            let bits = fb.ins().bitcast(types::I32, MemFlagsData::new(), value);
                            fb.ins().uextend(types::I64, bits)
                        }
                        types::I128 => fb.ins().ireduce(types::I64, value),
                        _ => fb.ins().uextend(types::I64, value),
                    },
                };
                self.e.call_rt(Rt::Keep, &[word])
            }
            Callee::Std(StdFn::ClockSettle) => self.e.call_rt(Rt::ClockSettle, &values),
            Callee::Std(StdFn::ClockAdvance) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::ClockAdvance, &values)
            }
            Callee::Std(function @ (StdFn::TestAttach | StdFn::TestSnapshot)) => {
                // Their panics name where they were called.
                values.push(self.location(self.span));
                let rt = if function == StdFn::TestAttach { Rt::TestAttach } else { Rt::TestSnapshot };
                self.e.call_rt(rt, &values)
            }
            Callee::Std(
                StdFn::Fs(_)
                | StdFn::Path(_)
                | StdFn::Io(_)
                | StdFn::Env(_)
                | StdFn::Process(_)
                | StdFn::TimeNow
                | StdFn::TimeUnix
                | StdFn::TimeUtc
                | StdFn::TimeUtcNow
                | StdFn::ConsoleFlush
                | StdFn::ConsoleReadLine
                | StdFn::ConsoleStream(_),
            ) => unreachable!("built where they are assigned"),
            Callee::Trait(..) | Callee::Extern(_) => unreachable!("handled before"),
        };
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
    }

    /// Gives the runner the members of a suite: `[node, glue, fn value]`
    /// for each, read while it runs them; then releases the values.
    fn test_members(&mut self, members: &[(u32, Operand, bool)]) {
        // `[node, glue, fn value]` for each member, read by the
        // runner while it runs them.
        let size = (members.len() as u32 * 24).max(8);
        let slot = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
        let address = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let mut objects = Vec::new();
        for (index, (node, member, retain)) in members.iter().enumerate() {
            let ty = self.operand_ty(member);
            let object = self.scalar(member);
            if *retain {
                self.e.refcount(ty, Val::Scalar(object), false);
            }
            let glue = self.e.cg.glue(Glue::Test, ty);
            let glue = self.e.cg.declare_func_in_func(glue, self.e.fb.func);
            let glue = self.e.fb.ins().func_addr(POINTER, glue);
            let node = self.e.fb.ins().iconst(types::I64, *node as i64);
            let offset = index as i32 * 24;
            self.e.fb.ins().store(flags(), node, address, offset);
            self.e.fb.ins().store(flags(), glue, address, offset + 8);
            self.e.fb.ins().store(flags(), object, address, offset + 16);
            objects.push((ty, object));
        }
        let count = self.e.fb.ins().iconst(types::I64, members.len() as i64);
        self.e.call_rt(Rt::TestMembers, &[address, count]);
        for (ty, object) in objects {
            self.e.refcount(ty, Val::Scalar(object), true);
        }
    }

    /// A call through a `fn(...)` value: its code receives the captures'
    /// address first (see `Codegen::uniform_signature`).
    fn call_indirect(&mut self, callee: &Operand, args: &[Operand], result: Option<ir::Value>) -> Option<ir::Value> {
        if let TyKind::CFn(params, result_ty, _) = self.e.cg.types().kind(self.operand_ty(callee)).clone() {
            let code = self.scalar(callee);
            return self.c_call(CTarget::Address(code), &params, result_ty, args, result);
        }
        let TyKind::Fn(params, result_ty, _, modes, _) = self.e.cg.types().kind(self.operand_ty(callee)).clone() else {
            unreachable!("an indirect call goes through a `fn(...)` value")
        };
        let loans: Vec<bool> = modes.iter().map(|m| matches!(m, Mode::Mut | Mode::Var)).collect();
        let object = self.scalar(callee);
        let code = self.e.fb.ins().load(POINTER, flags(), object, 16);
        let captures = self.e.fb.ins().iadd_imm_s(object, FN_HEADER);
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        values.push(captures);
        let mut spilled = Vec::new();
        for (arg, &loan) in args.iter().zip(&loans) {
            if loan {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                values.push(self.loan_address(place, &mut spilled));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) | Val::Memory(value) => values.push(value),
                Val::Zero => {}
            }
        }
        let signature = self.e.cg.uniform_signature(&params, &loans, result_ty);
        let signature = self.e.fb.import_signature(signature);
        let result = match self.unwind_to.filter(|_| !self.e.cg.live()) {
            Some(unwind) => self.try_call(Called::Indirect(signature, code), &values, unwind, &spilled),
            None => {
                let call = self.e.fb.ins().call_indirect(signature, code, &values);
                self.e.fb.inst_results(call).first().copied()
            }
        };
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
    }

    /// A call of compiled code that a panic may unwind (15.9), as a
    /// `try_call`: when the callee unwinds, the runtime makes it return to
    /// the handler, which goes the way the check after the call would have.
    /// A call that returns as usual costs nothing more, and needs no check.
    fn try_call(
        &mut self,
        callee: Called,
        args: &[ir::Value],
        unwind: ir::Block,
        spilled: &[(Variable, ir::Value)],
    ) -> Option<ir::Value> {
        use cranelift_codegen::ir::{BlockArg, BlockCall, ExceptionTableData, ExceptionTableItem};
        let signature = match callee {
            Called::Direct(callee) => self.e.fb.func.dfg.ext_funcs[callee].signature,
            Called::Indirect(signature, _) => signature,
        };
        let returns: Vec<ir::Type> =
            self.e.fb.func.dfg.signatures[signature].returns.iter().map(|r| r.value_type).collect();
        let (next, handler) = (self.e.fb.create_block(), self.e.fb.create_block());
        for &ty in &returns {
            self.e.fb.append_block_param(next, ty);
        }
        let pool = &mut self.e.fb.func.dfg.value_lists;
        let normal = BlockCall::new(next, (0..returns.len() as u32).map(BlockArg::TryCallRet), pool);
        let caught = BlockCall::new(handler, [], pool);
        let table = ExceptionTableData::new(signature, normal, [ExceptionTableItem::Default(caught)]);
        let table = self.e.fb.func.dfg.exception_tables.push(table);
        match callee {
            Called::Direct(callee) => self.e.fb.ins().try_call(callee, args, table),
            Called::Indirect(_, code) => self.e.fb.ins().try_call_indirect(code, args, table),
        };
        self.e.fb.switch_to_block(handler);
        self.e.fb.set_cold_block(handler);
        for &(var, slot) in spilled {
            self.unspill(var, slot);
        }
        self.e.fb.ins().jump(unwind, &[]);
        self.e.fb.switch_to_block(next);
        self.tried = true;
        self.e.fb.block_params(next).first().copied()
    }

    /// The address a loan of `place` passes; a local held in a variable is
    /// spilled to the stack and recorded in `spilled` to reload after the call.
    fn loan_address(&mut self, place: &Place, spilled: &mut Vec<(Variable, ir::Value)>) -> ir::Value {
        if self.e.cg.class(self.place_ty(place)) == Class::Zero {
            return self.e.fb.ins().iconst(POINTER, 0);
        }
        if let Storage::Var(var) = self.storage[place.local.0 as usize]
            && place.path.is_empty()
        {
            let slot = self.spill(var);
            spilled.push((var, slot));
            return slot;
        }
        self.pointer_to(place, true)
    }

    /// The head of the loop that `terminator`, ending block `index`, jumps
    /// back to, when it is only a test: comparisons and a branch. A build
    /// that steps, counts or replaces code goes through the head itself.
    fn loop_test(&self, index: usize, terminator: &Terminator) -> Option<&'a crate::mir::BlockData> {
        let Terminator::Goto(target) = terminator else { return None };
        let head = &self.body.blocks[target.0 as usize];
        let test = head.stmts.len() <= 2
            && head.stmts.iter().all(|stmt| {
                matches!(stmt, Statement::Assign(place, value, _)
                    if place.path.is_empty() && matches!(**value, Rvalue::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt
                        | BinOp::Ge | BinOp::Eq | BinOp::Ne, _, _)))
            })
            && matches!(head.terminator, Terminator::Branch(..));
        let plain = self.debug.is_none() && self.e.cg.coverage().is_none() && !self.e.cg.meta() && !self.e.cg.live();
        let Terminator::Branch(_, then, otherwise) = head.terminator else { return None };
        // One way of the test leaves the loop, so that its copy enters the
        // loop at one block, where Cranelift sees a loop it hoists from: a
        // test whose both ways go on in the loop, the first of `a and b`,
        // would enter it at two.
        let leaves = |way: crate::mir::BlockId| !self.reaches(way, index, target.0 as usize);
        (target.0 as usize <= index && test && plain && (leaves(then) || leaves(otherwise))).then_some(head)
    }

    /// Whether block `to` can be reached from `from` without passing
    /// through `head`.
    fn reaches(&self, from: crate::mir::BlockId, to: usize, head: usize) -> bool {
        let mut seen = vec![false; self.body.blocks.len()];
        let mut pending = vec![from.0 as usize];
        while let Some(block) = pending.pop() {
            if block == to {
                return true;
            }
            if block == head || std::mem::replace(&mut seen[block], true) {
                continue;
            }
            match self.body.blocks[block].terminator {
                Terminator::Goto(next) => pending.push(next.0 as usize),
                Terminator::Branch(_, then, otherwise) => pending.extend([then.0 as usize, otherwise.0 as usize]),
                _ => {}
            }
        }
        false
    }

    fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.e.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                // Every way out of a restart point is on the line its probe
                // was for: none probes it again.
                if self.carrying
                    && let Some(line) = self.debug.as_ref().and_then(|debug| debug.line.clone())
                {
                    self.carried.insert(then.0 as usize, line.clone());
                    self.carried.insert(otherwise.0 as usize, line);
                }
                let condition = self.scalar(condition);
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                // A condition known here, as a check for a panic that asks
                // nothing, is a jump.
                let dfg = &self.e.fb.func.dfg;
                let known = match dfg.value_def(condition) {
                    ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                        ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits() != 0),
                        _ => None,
                    },
                    _ => None,
                };
                match known {
                    Some(true) => self.e.fb.ins().jump(then, &[]),
                    Some(false) => self.e.fb.ins().jump(otherwise, &[]),
                    None => self.e.fb.ins().brif(condition, then, &[], otherwise, &[]),
                };
            }
            Terminator::Return(value) => match self.operand(value) {
                Val::Scalar(value) => {
                    self.e.fb.ins().return_(&[value]);
                }
                // Aggregates were already written through the result pointer.
                Val::Memory(_) | Val::Zero => {
                    self.e.fb.ins().return_(&[]);
                }
            },
            Terminator::Panic { code, message, span } => {
                self.at(*span);
                let message = match message {
                    Some(message) => self.scalar(message),
                    None => self.e.fb.ins().iconst(POINTER, 0),
                };
                self.emit_panic(*code, message, *span);
            }
            Terminator::Unreachable => {
                self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
    }

    fn emit_panic(&mut self, code: u32, message: ir::Value, span: Span) {
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let location = self.location(span);
        let outer = self.e.fb.srcloc();
        let site = self.site(span.start, None);
        self.e.fb.set_srcloc(site);
        self.e.call_rt(Rt::Panic, &[code, message, location]);
        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
        self.e.fb.set_srcloc(outer);
    }

    /// Branches to a cold block that panics with `code` when `condition`
    /// holds, with a message the runtime writes from the values `values`
    /// gives there: the detail, two values and the type they are about (see
    /// `tondo_rt_panic_values`).
    fn panic_values_if(
        &mut self,
        condition: ir::Value,
        code: u32,
        span: Span,
        values: impl FnOnce(&mut Self) -> (u32, ir::Value, ir::Value, Ty),
    ) {
        let (failed, next) = (self.e.fb.create_block(), self.e.fb.create_block());
        self.e.fb.set_cold_block(failed);
        // On `not ok`, a branch on `ok` with the targets swapped, which
        // Cranelift makes one jump with the comparison that gave `ok`.
        let dfg = &self.e.fb.func.dfg;
        let constant = |value: ir::Value| match dfg.value_def(value) {
            ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits()),
                _ => None,
            },
            _ => None,
        };
        let negated = match dfg.value_def(condition) {
            ir::ValueDef::Result(inst, _) => match dfg.insts[inst] {
                ir::InstructionData::Binary { opcode: ir::Opcode::Bxor, args: [ok, one] }
                    if constant(one) == Some(1) =>
                {
                    Some(ok)
                }
                _ => None,
            },
            _ => None,
        };
        match negated {
            Some(ok) => self.e.fb.ins().brif(ok, next, &[], failed, &[]),
            None => self.e.fb.ins().brif(condition, failed, &[], next, &[]),
        };
        self.e.fb.switch_to_block(failed);
        let (detail, a, b, ty) = values(self);
        let a = self.widest(a, detail & 0x100 != 0);
        let b = self.widest(b, detail & 0x200 != 0);
        let name = self.e.cg.types().display(ty);
        let name = self.e.string_ptr(&name);
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let detail = self.e.fb.ins().iconst(types::I32, detail as i64);
        let location = self.location(span);
        let outer = self.e.fb.srcloc();
        let site = self.site(span.start, None);
        self.e.fb.set_srcloc(site);
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        self.e.fb.ins().stack_store(POINTER, a, slot, 0);
        self.e.fb.ins().stack_store(POINTER, b, slot, 16);
        let values = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        match self.unwind_to {
            // A panic a `recover` may stop returns, and unwinds (15.9).
            Some(target) => {
                self.e.call_rt(Rt::PanicValuesUnwind, &[code, detail, values, name, location]);
                self.e.fb.ins().jump(target, &[]);
            }
            None => {
                self.e.call_rt(Rt::PanicValues, &[code, detail, values, name, location]);
                self.e.fb.ins().trap(TrapCode::user(1).unwrap());
            }
        }
        self.e.fb.set_srcloc(outer);
        self.e.fb.switch_to_block(next);
    }

    /// Whether a statement's panics are all checks of its own, whose cold
    /// paths jump to the unwinding: no call that may panic and return.
    fn inline_only(&mut self, statement: &Statement) -> bool {
        let mut scalar = |operand: &Operand| match operand {
            Operand::Copy(place) => {
                let ty = self.body.place_ty(self.e.cg.types(), place);
                matches!(self.e.cg.class(ty), Class::Scalar(_) | Class::Zero)
            }
            Operand::Const(_) => true,
        };
        match statement {
            Statement::Assign(_, rvalue, _) => match &**rvalue {
                Rvalue::Binary(_, a, b) | Rvalue::Exact(_, a, b) => scalar(a) && scalar(b),
                Rvalue::Unary(_, a) | Rvalue::Increment(a) => scalar(a),
                Rvalue::Use(_)
                | Rvalue::Aggregate(_)
                | Rvalue::Variant(..)
                | Rvalue::Discriminant(_)
                | Rvalue::Length(_)
                | Rvalue::Layout(..)
                | Rvalue::Convert(_) => true,
                _ => false,
            },
            _ => true,
        }
    }

    /// An integer as 128 bits.
    fn widest(&mut self, value: ir::Value, signed: bool) -> ir::Value {
        match self.e.fb.func.dfg.value_type(value) {
            types::I128 => value,
            _ if signed => self.e.fb.ins().sextend(types::I128, value),
            _ => self.e.fb.ins().uextend(types::I128, value),
        }
    }

    /// An integer as 64 bits.
    fn wide(&mut self, value: ir::Value, signed: bool) -> ir::Value {
        match self.e.fb.func.dfg.value_type(value) {
            types::I64 => value,
            _ if signed => self.e.fb.ins().sextend(types::I64, value),
            _ => self.e.fb.ins().uextend(types::I64, value),
        }
    }

    // ---- operators -----------------------------------------------------------

    fn unary(&mut self, op: UnaryOp, operand: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(operand);
        let value = self.scalar(operand);
        match op {
            UnaryOp::Not => self.e.fb.ins().bxor_imm_u(value, 1),
            UnaryOp::BitNot => self.e.fb.ins().bnot(value),
            UnaryOp::Neg if matches!(self.e.cg.types().kind(ty), TyKind::Float(_)) => self.e.fb.ins().fneg(value),
            UnaryOp::Neg => {
                let clif = self.e.cg.scalar_type(ty);
                let zero = self.e.int_const(clif, 0);
                let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, value);
                let ty = self.e.cg.types().repr(ty);
                self.panic_values_if(overflow, 5, span, |_| (5 << 16 | 0x100 | clif.bits(), value, value, ty));
                result
            }
        }
    }

    /// `x = "{x}text"`: the text's bytes written after the string's own,
    /// as `push` adds an element, when nothing else holds the string and
    /// its room takes them; or else the runtime's append.
    fn append_text(&mut self, place: &Place, text: &str) -> ir::Value {
        let string = self.scalar(&Operand::Copy(place.clone()));
        let count = self.e.fb.ins().atomic_load(types::I64, flags(), string);
        let length = self.e.fb.ins().load(types::I64, flags(), string, STRING_LENGTH);
        let room = self.e.fb.ins().load(types::I64, flags(), string, STRING_ROOM);
        let longer = self.e.fb.ins().iadd_imm_s(length, text.len() as i64);
        let unique = self.e.fb.ins().icmp_imm_s(IntCC::Equal, count, 1);
        let fits = self.e.fb.ins().icmp(IntCC::UnsignedLessThanOrEqual, longer, room);
        let in_place = self.e.fb.ins().band(unique, fits);
        let (fast, slow, done) = (self.e.fb.create_block(), self.e.fb.create_block(), self.e.fb.create_block());
        let result = self.e.fb.append_block_param(done, POINTER);
        self.e.fb.ins().brif(in_place, fast, &[], slow, &[]);
        self.e.fb.switch_to_block(fast);
        let end = self.e.fb.ins().iadd(string, length);
        // The bytes in words of 8, 4, 2 and 1 bytes, stored as constants.
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            let (width, ty) = match bytes.len() - at {
                8.. => (8, types::I64),
                4..=7 => (4, types::I32),
                2..=3 => (2, types::I16),
                _ => (1, types::I8),
            };
            let mut word = [0u8; 8];
            word[..width].copy_from_slice(&bytes[at..at + width]);
            let value = self.e.fb.ins().iconst(ty, u64::from_le_bytes(word) as i64);
            let unaligned = MemFlagsData::new().with_notrap();
            self.e.fb.ins().store(unaligned, value, end, (STRING_BYTES + at as i64) as i32);
            at += width;
        }
        self.e.fb.ins().store(flags(), longer, string, STRING_LENGTH);
        self.e.fb.ins().jump(done, &[string.into()]);
        self.e.fb.set_cold_block(slow);
        self.e.fb.switch_to_block(slow);
        let text = self.e.string_ptr(text);
        let appended = self.e.call_rt(Rt::StrAppend, &[string, text]).unwrap();
        self.e.fb.ins().jump(done, &[appended.into()]);
        self.e.fb.switch_to_block(done);
        result
    }

    fn concat(&mut self, pieces: &[Piece]) -> ir::Value {
        // A type shown as what it is made of: one that shows itself, as a
        // `Duration` made of an `Int`, goes through its `Display`.
        let plain = |this: &Self, ty: Ty| {
            let types = this.e.cg.types();
            let shows = this.e.cg.display().contains_key(&ty) || types.shown.contains_key(&ty);
            (!shows).then(|| types.kind(types.repr(ty)).clone())
        };
        // `x = "{x}text"` with a short text known now, in place.
        let known = match pieces {
            [Piece::Taken(place), Piece::Text(text) | Piece::Value(Operand::Const(Const::Str(text)), _)] => {
                Some((place, text.clone()))
            }
            [Piece::Taken(place), Piece::Value(Operand::Copy(of), _)] if of.path.is_empty() => {
                self.literal.get(&of.local).cloned().flatten().map(|text| (place, text))
            }
            _ => None,
        };
        if let Some((place, text)) = known
            && (1..=16).contains(&text.len())
        {
            return self.append_text(place, &text);
        }
        // `x = "{x}{s}"` and `x = "{x}{i}"` in one call.
        if let [Piece::Taken(place), Piece::Value(operand, ty)] = pieces
            && let Some(kind) = plain(self, *ty)
        {
            let appended = match kind {
                TyKind::String => Some((Rt::StrAppend, false)),
                TyKind::Int(int) if int.bits() < 128 && (int.signed() || int.bits() < 64) => {
                    Some((Rt::StrAppendInt, int.signed()))
                }
                _ => None,
            };
            if let Some((rt, signed)) = appended {
                let text = self.scalar(&Operand::Copy(place.clone()));
                let value = self.scalar(operand);
                let value = if rt == Rt::StrAppendInt { self.wide(value, signed) } else { value };
                return self.e.call_rt(rt, &[text, value]).unwrap();
            }
        }
        // An integer alone, `"{i}"`, in one call.
        if let [Piece::Value(operand, ty)] = pieces
            && let Some(TyKind::Int(int)) = plain(self, *ty)
            && int.bits() < 128
            && (int.signed() || int.bits() < 64)
        {
            let value = self.scalar(operand);
            let value = self.wide(value, int.signed());
            return self.e.call_rt(Rt::IntText, &[value]).unwrap();
        }
        // A string alone is that string.
        if let [Piece::Value(operand, ty)] = pieces
            && plain(self, *ty).is_some()
            && self.e.cg.types().repr(*ty) == Ty::STRING
        {
            let text = self.scalar(operand);
            self.e.call_rt(Rt::StrRetain, &[text]);
            return text;
        }
        // Room for the text and the strings, and a guess for the rest, so
        // that the text is allocated once.
        let mut known = 0;
        let mut room = None;
        let mut values = Vec::new();
        let mut taken = None;
        let mut released = None;
        for piece in pieces {
            match piece {
                Piece::Text(text) => known += text.len() as i64,
                Piece::Taken(place) => taken = Some((place, self.scalar(&Operand::Copy(place.clone())))),
                Piece::Value(operand, ty) => {
                    let value = self.operand(operand);
                    match (self.e.cg.types().kind(self.e.cg.types().repr(*ty)), value) {
                        (TyKind::String, Val::Scalar(text)) => {
                            let length = self.e.fb.ins().load(types::I64, flags(), text, STRING_LENGTH);
                            room = Some(room.map_or(length, |room| self.e.fb.ins().iadd(room, length)));
                        }
                        (TyKind::Int(int), _) if int.bits() == 128 => known += 40,
                        // At most one digit more than it has, and its sign.
                        (TyKind::Int(_), Val::Scalar(value)) => {
                            let value = self.wide(value, true);
                            let sign = self.e.fb.ins().sshr_imm_u(value, 63);
                            let magnitude = self.e.fb.ins().bxor(value, sign);
                            let magnitude = self.e.fb.ins().bor_imm_u(magnitude, 1);
                            let zeros = self.e.fb.ins().clz(magnitude);
                            let sixty_four = self.e.fb.ins().iconst(types::I64, 64);
                            let bits = self.e.fb.ins().isub(sixty_four, zeros);
                            let digits = self.e.fb.ins().imul_imm_u(bits, 1233);
                            let digits = self.e.fb.ins().ushr_imm_u(digits, 12);
                            room = Some(room.map_or(digits, |room| self.e.fb.ins().iadd(room, digits)));
                            known += 2;
                        }
                        (TyKind::Int(_) | TyKind::Float(_), _) => known += 24,
                        _ => known += 16,
                    }
                    values.push(value);
                }
            }
        }
        let room = match room {
            Some(room) => self.e.fb.ins().iadd_imm_s(room, known),
            None => self.e.fb.ins().iconst(types::I64, known),
        };
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let builder = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        match taken {
            // The string goes on in its own block when nothing else holds
            // it; else it is copied, and released once the text is built.
            Some((_, text)) => {
                let adopted = self.e.call_rt(Rt::SbAdopt, &[builder, text, room]).unwrap();
                released = Some((adopted, text));
            }
            None => {
                self.e.call_rt(Rt::SbStart, &[builder, room]);
            }
        }
        let mut values = values.into_iter();
        for piece in pieces {
            match piece {
                Piece::Taken(_) => {}
                Piece::Text(text) => {
                    let text = self.e.string_ptr(text);
                    self.e.call_rt(Rt::SbStr, &[builder, text]);
                }
                Piece::Value(_, ty) => {
                    let value = values.next().unwrap();
                    self.e.append_display(builder, *ty, value);
                }
            }
        }
        let built = self.e.call_rt(Rt::SbFinish, &[builder]).unwrap();
        if let Some((adopted, text)) = released {
            let (release, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            self.e.fb.ins().brif(adopted, next, &[], release, &[]);
            self.e.fb.switch_to_block(release);
            self.e.call_rt(Rt::StrRelease, &[text]);
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        built
    }

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        self.compared(op, left, right, ty);
        let is_array = |ty: Ty| matches!(self.e.cg.types().kind(self.e.cg.types().repr(ty)), TyKind::Array(_));
        if !matches!(op, BinOp::Eq | BinOp::Ne) && (is_array(ty) || is_array(right_ty)) {
            return self.array_arithmetic(op, left, right, span);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let (a, b) = (self.operand(left), self.operand(right));
            // Integers, characters and booleans differ by one comparison,
            // which a branch on it takes as is.
            let types = self.e.cg.types();
            if op == BinOp::Ne
                && let (Val::Scalar(a), Val::Scalar(b)) = (a, b)
                && matches!(types.kind(types.repr(ty)), TyKind::Int(_) | TyKind::Char | TyKind::Bool)
            {
                return self.e.fb.ins().icmp(IntCC::NotEqual, a, b);
            }
            let equal = self.e.equal(ty, a, b);
            return if op == BinOp::Eq { equal } else { self.e.fb.ins().bxor_imm_u(equal, 1) };
        }
        let (a, b) = (self.scalar(left), self.scalar(right));
        if self.e.cg.types().repr(ty) == Ty::STRING {
            let order = self.e.call_rt(Rt::StrCmp, &[a, b]).unwrap();
            return self.e.fb.ins().icmp_imm_s(int_condition(op, true), order, 0);
        }
        let types = self.e.cg.types();
        if let TyKind::Float(_) = types.kind(ty) {
            // IEEE 754 without traps (18.5); relational comparisons with NaN are false.
            let ins = self.e.fb.ins();
            return match op {
                BinOp::Add => ins.fadd(a, b),
                BinOp::Sub => ins.fsub(a, b),
                BinOp::Mul => ins.fmul(a, b),
                BinOp::Div => ins.fdiv(a, b),
                BinOp::Lt => ins.fcmp(FloatCC::LessThan, a, b),
                BinOp::Le => ins.fcmp(FloatCC::LessThanOrEqual, a, b),
                BinOp::Gt => ins.fcmp(FloatCC::GreaterThan, a, b),
                BinOp::Ge => ins.fcmp(FloatCC::GreaterThanOrEqual, a, b),
                _ => unreachable!("rejected by the checker"),
            };
        }
        // A newtype computes as the integer it holds.
        let ty = types.repr(ty);
        let signed = types.as_int(ty).is_some_and(IntTy::signed);
        let right_signed = types.as_int(types.repr(right_ty)).is_some_and(IntTy::signed);
        // What the runtime needs to tell the operation in a panic.
        let operation = |op: BinOp| {
            let code = match op {
                BinOp::Add => 0,
                BinOp::Sub => 1,
                BinOp::Mul => 2,
                BinOp::Div => 3,
                BinOp::Rem => 4,
                BinOp::Shl => 6,
                _ => 7,
            };
            let bits = self.e.cg.types().as_int(ty).map_or(8, IntTy::bits);
            code << 16 | (right_signed as u32) << 9 | (signed as u32) << 8 | bits
        };
        let detail = operation(op);
        // A product by a constant 1 is the other factor, and cannot
        // overflow: no multiply, which qemu's arm64 emulation gets wrong
        // for `smulh` by a known 1.
        let one = |fb: &FunctionBuilder, value: ir::Value| {
            let ir::ValueDef::Result(inst, _) = fb.func.dfg.value_def(value) else { return false };
            matches!(fb.func.dfg.insts[inst], ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm }
                if imm.bits() == 1)
        };
        if op == BinOp::Mul && (one(&self.e.fb, a) || one(&self.e.fb, b)) {
            return if one(&self.e.fb, b) { a } else { b };
        }
        let wide = self.e.fb.func.dfg.value_type(a) == types::I128;
        match op {
            // Cranelift multiplies 128 bits, but does not tell an overflow.
            BinOp::Mul if wide => {
                let signed_flag = self.e.fb.ins().iconst(types::I8, signed as i64);
                let (result, overflow) = self.int128_call(Rt::Int128Multiply, &[signed_flag], a, b);
                self.panic_values_if(overflow.unwrap(), 5, span, |_| (detail, a, b, ty));
                result
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let ins = self.e.fb.ins();
                let (result, overflow) = match (op, signed) {
                    (BinOp::Add, true) => ins.sadd_overflow(a, b),
                    (BinOp::Add, false) => ins.uadd_overflow(a, b),
                    (BinOp::Sub, true) => ins.ssub_overflow(a, b),
                    (BinOp::Sub, false) => ins.usub_overflow(a, b),
                    (_, true) => ins.smul_overflow(a, b),
                    (_, false) => ins.umul_overflow(a, b),
                };
                // The left operand of a sum or a difference, for the panic's
                // message, from the result, which wraps: the code that goes on
                // needs to keep only the result.
                self.panic_values_if(overflow, 5, span, |this| {
                    let left = match op {
                        BinOp::Add => this.e.fb.ins().isub(result, b),
                        BinOp::Sub => this.e.fb.ins().iadd(result, b),
                        _ => a,
                    };
                    (detail, left, b, ty)
                });
                result
            }
            BinOp::Div | BinOp::Rem => {
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.panic_values_if(zero, 3, span, |_| (detail, a, b, ty));
                let int = types.as_int(ty).unwrap();
                let clif = int_type(int);
                // Cranelift does not divide 128 bits: the runtime does.
                let divide = |this: &mut Self, divisor: ir::Value| {
                    let remainder = this.e.fb.ins().iconst(types::I8, (op == BinOp::Rem) as i64);
                    let signed = this.e.fb.ins().iconst(types::I8, signed as i64);
                    this.int128_call(Rt::Int128Divide, &[remainder, signed], a, divisor).0
                };
                if !signed {
                    return match (wide, op) {
                        (true, _) => divide(self, b),
                        (_, BinOp::Div) => self.e.fb.ins().udiv(a, b),
                        _ => self.e.fb.ins().urem(a, b),
                    };
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let min = self.e.int_const(clif, int.min());
                let minus_one = self.e.int_const(clif, -1);
                let is_min = self.e.fb.ins().icmp(IntCC::Equal, a, min);
                let is_minus_one = self.e.fb.ins().icmp(IntCC::Equal, b, minus_one);
                let both = self.e.fb.ins().band(is_min, is_minus_one);
                if wide {
                    if op == BinOp::Div {
                        self.panic_values_if(both, 5, span, |_| (detail, a, b, ty));
                        return divide(self, b);
                    }
                    let one = self.e.int_const(clif, 1);
                    let divisor = self.e.fb.ins().select(both, one, b);
                    return divide(self, divisor);
                }
                if op == BinOp::Div {
                    self.panic_values_if(both, 5, span, |_| (detail, a, b, ty));
                    self.e.fb.ins().sdiv(a, b)
                } else {
                    let one = self.e.int_const(int_type(int), 1);
                    let divisor = self.e.fb.ins().select(both, one, b);
                    self.e.fb.ins().srem(a, divisor)
                }
            }
            BinOp::BitAnd => self.e.fb.ins().band(a, b),
            BinOp::BitOr => self.e.fb.ins().bor(a, b),
            BinOp::BitXor => self.e.fb.ins().bxor(a, b),
            BinOp::Shl | BinOp::Shr => {
                let width = self.e.cg.scalar_type(ty).bits() as i64;
                if types.as_int(right_ty).is_some_and(IntTy::signed) {
                    let negative = self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, b, 0);
                    self.panic_values_if(negative, 10, span, |_| (detail, a, b, ty));
                }
                let too_wide = self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b, width);
                self.panic_values_if(too_wide, 10, span, |_| (detail, a, b, ty));
                // The count is below the width: a count of 128 bits fits in
                // 64, and one of 128 bits shifts by a count of 64.
                let b = match self.e.fb.func.dfg.value_type(b) {
                    types::I128 => self.e.fb.ins().ireduce(types::I64, b),
                    count if wide && count != types::I64 => self.e.fb.ins().uextend(types::I64, b),
                    _ => b,
                };
                match (op, signed) {
                    (BinOp::Shl, _) => self.e.fb.ins().ishl(a, b),
                    (_, true) => self.e.fb.ins().sshr(a, b),
                    (_, false) => self.e.fb.ins().ushr(a, b),
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => self.e.fb.ins().icmp(int_condition(op, signed), a, b),
            BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeInclusive | BinOp::In => {
                unreachable!("handled above or lowered before codegen")
            }
        }
    }
}

/// The locals a function assigns a string literal and nothing else, nor
/// lends to what could change them: a call, a `select`, a task or a
/// mutation; and the literal, when it is always the same.
fn literals(body: &Body) -> HashMap<Local, Option<String>> {
    let mut assigned: HashMap<Local, bool> = HashMap::new();
    let mut texts: HashMap<Local, Option<String>> = HashMap::new();
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        crate::mir::statement_locals(statement, &mut used, &mut defined);
        let literal = match statement {
            Statement::Assign(place, rvalue, _) if place.path.is_empty() => match &**rvalue {
                Rvalue::Use(Operand::Const(Const::Str(text))) => Some(text),
                _ => None,
            },
            _ => None,
        };
        if let (Some(text), Statement::Assign(place, ..)) = (literal, statement) {
            let same = texts.entry(place.local).or_insert_with(|| Some(text.clone()));
            if same.as_ref() != Some(text) {
                *same = None;
            }
        }
        let literal = literal.is_some();
        let lends = matches!(statement, Statement::Assign(_, rvalue, _) if matches!(**rvalue,
            Rvalue::Call(..) | Rvalue::CallIndirect(..) | Rvalue::CallOsr(..) | Rvalue::Select(..) | Rvalue::TaskOp(..)
            | Rvalue::Spawn(..) | Rvalue::Mutate(..) | Rvalue::Pointer(..)));
        for local in defined {
            *assigned.entry(local).or_insert(literal) &= literal;
        }
        if lends {
            for local in used {
                assigned.insert(local, false);
            }
        }
    }
    let params: HashSet<Local> = body.params.iter().copied().chain(body.ret).collect();
    assigned
        .into_iter()
        .filter(|&(local, literal)| literal && !params.contains(&local))
        .map(|(local, _)| (local, texts.remove(&local).flatten()))
        .collect()
}

fn int_condition(op: BinOp, signed: bool) -> IntCC {
    match (op, signed) {
        (BinOp::Eq, _) => IntCC::Equal,
        (BinOp::Ne, _) => IntCC::NotEqual,
        (BinOp::Lt, true) => IntCC::SignedLessThan,
        (BinOp::Le, true) => IntCC::SignedLessThanOrEqual,
        (BinOp::Gt, true) => IntCC::SignedGreaterThan,
        (BinOp::Ge, true) => IntCC::SignedGreaterThanOrEqual,
        (BinOp::Lt, false) => IntCC::UnsignedLessThan,
        (BinOp::Le, false) => IntCC::UnsignedLessThanOrEqual,
        (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
        (BinOp::Ge, false) => IntCC::UnsignedGreaterThanOrEqual,
        _ => unreachable!(),
    }
}

/// `NotFound` as `not found`.
fn words(name: &str) -> String {
    let mut text = String::new();
    for c in name.chars() {
        if c.is_uppercase() && !text.is_empty() {
            text.push(' ');
        }
        text.extend(c.to_lowercase());
    }
    text
}

/// Whether a statement lets a panic go on after a frame's cleanups.
fn resumes(statement: &Statement) -> bool {
    matches!(statement, Statement::Assign(_, rvalue, _)
        if matches!(**rvalue, Rvalue::Call(Callee::Std(StdFn::UnwindResume), _)))
}

/// Where the panics of the statement at `at` unwind to: the target of the
/// check that follows it, `asked = Unwinding` and a branch on it (15.9).
fn unwind_target(block: &crate::mir::BlockData, at: usize) -> Option<crate::mir::BlockId> {
    let Some(Statement::Assign(asked, rvalue, _)) = block.stmts.get(at + 1) else { return None };
    if !matches!(**rvalue, Rvalue::Unwinding) || !asked.path.is_empty() || at + 2 != block.stmts.len() {
        return None;
    }
    match &block.terminator {
        Terminator::Branch(Operand::Copy(condition), then, _)
            if condition.path.is_empty() && condition.local == asked.local =>
        {
            Some(*then)
        }
        _ => None,
    }
}
