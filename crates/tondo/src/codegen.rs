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
    Body, Const, Local, Operand, Overlap, Piece, Place, Proj, Rvalue, SelectCase, Statement, Terminator, project,
};
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

/// Runtime entry points (see `tondo-runtime`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Rt {
    StrRetain,
    StrRelease,
    StrEq,
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
    SbNew,
    SbStr,
    SbI64,
    SbU64,
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
    ArrayRepeat,
    ArrayEq,
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
            Rt::SbNew => ("tondo_rt_sb_new", &[], &[I64]),
            Rt::SbStr => ("tondo_rt_sb_str", &[I64, I64], &[]),
            Rt::SbI64 => ("tondo_rt_sb_i64", &[I64, I64], &[]),
            Rt::SbU64 => ("tondo_rt_sb_u64", &[I64, I64], &[]),
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
            Rt::PanicValues => ("tondo_rt_panic_values", &[I32, I32, I64, I64, I64, I64], &[]),
            Rt::Trace => ("tondo_rt_trace", &[I64], &[]),
            Rt::DebugTable => ("tondo_rt_debug_table", &[I64], &[]),
            Rt::DebugLine => ("tondo_rt_debug_line", &[I64, I64, I64], &[]),
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
            Rt::ReportError => ("tondo_rt_report_error", &[I64], &[]),
            Rt::TestStart => ("tondo_rt_test_start", &[I64, I64], &[]),
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
            Rt::ArrayRepeat => ("tondo_rt_array_repeat", &[I64, I64, I64, I64, I64], &[I64]),
            Rt::ArrayEq => ("tondo_rt_array_eq", &[I64, I64, I64, I64], &[I8]),
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
            Rt::ChannelNew => ("tondo_rt_channel_new", &[I64, I8, I64, I64, I64, I64, I64], &[I64]),
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
                FsFn::Metadata => ("tondo_rt_fs_metadata", &[I64, I64], &[I64]),
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
            },
            Rt::Native(_) => unreachable!("declared from `NATIVES`"),
            Rt::ForeignEnter => ("tondo_rt_foreign_enter", &[], &[]),
            Rt::ForeignLeave => ("tondo_rt_foreign_leave", &[], &[]),
            Rt::CString => ("tondo_rt_c_string", &[I64, I64], &[I64]),
            Rt::CStringFree => ("tondo_rt_c_string_free", &[I64], &[]),
            Rt::CVariadic => ("tondo_rt_c_variadic", &[I64, I64], &[]),
            Rt::Callback => ("tondo_rt_callback", &[I64, I64, I64], &[]),
            Rt::Main => ("tondo_rt_main", &[I64], &[I32]),
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
        _ => types::I64,
    }
}

fn symbol_name(index: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("tondo_f{index}_{clean}")
}

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

/// Compiles the program to an object file; `meta` counts a step at each
/// call and each turn of a loop, for a provider or generator (27.7).
pub fn compile(
    program: &Program,
    bodies: &[Body],
    sources: &SourceMap,
    meta: bool,
    debug: bool,
) -> Result<Vec<u8>, String> {
    let mut flags = settings::builder();
    // A debug build keeps the code as written, statement after statement.
    flags.set("opt_level", if debug { "none" } else { "speed" }).map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    let isa = cranelift_native::builder()
        .map_err(|e| format!("unsupported host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
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
        layouts: HashMap::new(),
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
    };

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
    for (index, export) in program.exports.iter().enumerate() {
        cg.export(index, export)?;
    }
    let mut context = cg.module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    // The IR of every function is built here, in order; `compile_all`
    // compiles them on every core.
    let mut jobs = Vec::new();
    let mut job = |id: FuncId, context: &mut cranelift_codegen::Context, traced: Option<&str>, sites| {
        let func = std::mem::replace(&mut context.func, ir::Function::new());
        jobs.push(Job { id, func, traced: traced.map(str::to_string), sites });
    };
    for (index, body) in bodies.iter().enumerate() {
        context.func.signature = cg.signature(body);
        context.func.name = UserFuncName::user(0, index as u32);
        let sites = FnCodegen::new(&mut cg, body, bodies, &mut context.func, &mut builder_context).run();
        // A test's trace ends at the test, without the runner's tree that
        // calls it.
        let runner = !program.tests.is_empty() && program.main.is_some_and(|main| index == main.0 as usize);
        let traced = !runner && !body.name.starts_with("suite ");
        job(cg.functions[index], &mut context, traced.then_some(body.name.as_str()), sites);
        cg.module.clear_context(&mut context);
    }
    let trace = cg.module.declare_data("tondo_trace", Linkage::Local, false, false).map_err(|e| e.to_string())?;
    match program.main {
        Some(main) => {
            let main = main.0 as usize;
            let entry =
                cg.entry_point(main, bodies[main].result, &program.tests, trace, &mut context, &mut builder_context)?;
            job(entry, &mut context, None, Vec::new());
            cg.module.clear_context(&mut context);
            let start = cg.process_main(entry, &mut context, &mut builder_context)?;
            job(start, &mut context, None, Vec::new());
        }
        None => {
            let init = cg.library_start(trace, &mut context, &mut builder_context)?;
            job(init, &mut context, None, Vec::new());
            cg.library = Some(init);
        }
    }
    cg.module.clear_context(&mut context);
    while let Some((target, thunk)) = cg.pending_thunks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(thunk).signature.clone();
        context.func.name = UserFuncName::user(3, thunk.as_u32());
        define_thunk(&mut cg, &bodies[target.0 as usize], target, &mut context.func, &mut builder_context);
        job(thunk, &mut context, None, Vec::new());
        cg.module.clear_context(&mut context);
    }
    while let Some(callback) = cg.pending_callbacks.pop() {
        context.func.signature = cg.module.declarations().get_function_decl(callback.adapter).signature.clone();
        context.func.name = UserFuncName::user(4, callback.adapter.as_u32());
        define_adapter(&mut cg, &callback, &mut context.func, &mut builder_context);
        job(callback.adapter, &mut context, None, Vec::new());
        cg.module.clear_context(&mut context);
        context.func.signature = cg.module.declarations().get_function_decl(callback.inner).signature.clone();
        context.func.name = UserFuncName::user(5, callback.inner.as_u32());
        define_inner(&mut cg, &bodies[callback.target.0 as usize], &callback, &mut context.func, &mut builder_context);
        job(callback.inner, &mut context, Some(&bodies[callback.target.0 as usize].name), Vec::new());
        cg.module.clear_context(&mut context);
    }
    while let Some((kind, ty, id)) = cg.pending_glue.pop() {
        context.func.signature = cg.glue_signature(kind);
        context.func.name = UserFuncName::user(2, id.as_u32());
        define_glue(&mut cg, kind, ty, &mut context.func, &mut builder_context);
        job(id, &mut context, None, Vec::new());
        cg.module.clear_context(&mut context);
    }
    cg.compile_all(jobs)?;

    cg.trace_table(trace);
    if let Some(debug) = cg.debug {
        cg.debug_table(debug);
    }
    let product = cg.module.finish();
    product.emit().map_err(|e| e.to_string())
}

struct Codegen<'a> {
    module: ObjectModule,
    types: &'a Types,
    sources: &'a SourceMap,
    /// Counts steps, for the `meta` profile.
    meta: bool,
    functions: Vec<FuncId>,
    runtime: HashMap<Rt, FuncId>,
    strings: HashMap<String, DataId>,
    layouts: HashMap<Ty, Layout>,
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
}

/// A binding a debug build's probe shows: its name, its type and the glue
/// that shows it.
type DebugBinding = (String, Ty, FuncId);

/// A compiled function for stack traces: its name, size and, for each
/// call, the offset it returns to and where its source starts.
struct Traced {
    id: FuncId,
    name: String,
    size: u32,
    calls: Vec<(u32, u32)>,
    sites: Vec<(u32, Option<Ty>)>,
}

/// A function whose IR is built, to compile; `traced` names it in stack
/// traces.
struct Job {
    id: FuncId,
    func: ir::Function,
    traced: Option<String>,
    /// Where each statement's code is in the source, and the type of the
    /// local it assigns, so that a sample of the memory it allocates names
    /// its type: the source location of an instruction indexes this, from 1.
    sites: Vec<(u32, Option<Ty>)>,
}

/// A function's machine code, its relocations, and its size and calls for
/// stack traces.
struct Compiled {
    alignment: u64,
    bytes: Vec<u8>,
    relocs: Vec<cranelift_module::ModuleReloc>,
    size: u32,
    calls: Vec<(u32, u32)>,
}

/// Compiles a function's IR to machine code.
fn compile_one(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    context: &mut cranelift_codegen::Context,
    job: &mut Job,
) -> Result<Compiled, String> {
    context.clear();
    context.func = std::mem::replace(&mut job.func, ir::Function::new());
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
    let relocs = code
        .buffer
        .relocs()
        .iter()
        .map(|reloc| cranelift_module::ModuleReloc::from_mach_reloc(reloc, &context.func, job.id))
        .collect();
    Ok(Compiled {
        alignment: code.buffer.min_alignment as u64,
        bytes: code.buffer.data().to_vec(),
        relocs,
        size: code.code_info().total_size,
        calls,
    })
}

impl Codegen<'_> {
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
                let component_align = if c.packed { 1 } else { layout.align };
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
            Layout { size, align: size.max(1), shape: Shape::Scalar }
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
        let mut signature = self.module.make_signature();
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
        if let Class::Scalar(ty) = self.class(body.result) {
            signature.returns.push(AbiParam::new(ty));
        }
        signature
    }

    /// The calling convention of a `fn(...)` value: the result slot when
    /// the result lives in memory, the captures' address, then the
    /// parameters. Closure bodies and thunks follow it.
    fn uniform_signature(&mut self, params: &[Ty], loans: &[bool], result: Ty) -> ir::Signature {
        let mut signature = self.module.make_signature();
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

    fn glue_signature(&mut self, kind: Glue) -> ir::Signature {
        let mut signature = self.module.make_signature();
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

    fn runtime(&mut self, rt: Rt) -> FuncId {
        if let Some(&id) = self.runtime.get(&rt) {
            return id;
        }
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
                let returns: Vec<ir::Type> = (function.result != crate::hir::NativeTy::Unit)
                    .then(|| clif(function.result))
                    .into_iter()
                    .collect();
                native = (params, returns);
                (function.symbol, native.0.as_slice(), native.1.as_slice())
            }
            _ => rt.signature(),
        };
        let mut signature = self.module.make_signature();
        // The C ABI as LLVM implements it expects narrow integer arguments
        // extended to 32 bits by the caller.
        signature.params.extend(
            params.iter().map(
                |&t| {
                    if t.is_int() && t.bits() < 32 { AbiParam::new(t).uext() } else { AbiParam::new(t) }
                },
            ),
        );
        signature.returns.extend(returns.iter().map(|&t| AbiParam::new(t)));
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
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(jobs.len().max(1));
        let size = jobs.len().div_ceil(threads);
        let compiled: Vec<Result<Compiled, String>> = std::thread::scope(|scope| {
            let workers: Vec<_> = jobs
                .chunks_mut(size.max(1))
                .map(|chunk| {
                    scope.spawn(move || {
                        let mut context = cranelift_codegen::Context::new();
                        chunk.iter_mut().map(|job| compile_one(isa, &mut context, job)).collect::<Vec<_>>()
                    })
                })
                .collect();
            workers.into_iter().flat_map(|worker| worker.join().expect("a compiling thread")).collect()
        });
        for (job, code) in jobs.into_iter().zip(compiled) {
            let code = code?;
            self.module
                .define_function_bytes(job.id, code.alignment, &code.bytes, &code.relocs)
                .map_err(|e| format!("{e:?}"))?;
            if let Some(name) = job.traced {
                self.traced.push(Traced { id: job.id, name, size: code.size, calls: code.calls, sites: job.sites });
            }
        }
        Ok(())
    }

    /// The stack trace table (the runtime's `trace::Table`): each function's
    /// code and size, and in one blob its name and calls, then the names
    /// the calls refer to: their files, and the types they assign.
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
                let (source, ty) = function.sites[site as usize - 1];
                let (file, line, column) = self.sources.position(source);
                let file = name(&mut files, file);
                let ty = match ty {
                    Some(ty) => name(&mut files, self.types.display(ty)),
                    None => u32::MAX,
                };
                for value in [offset, line as u32, column as u32, file, ty] {
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
        let mut header = vec![0u8; 32];
        header[8..16].copy_from_slice(&(self.traced.len() as u64).to_le_bytes());
        header[24..32].copy_from_slice(&files_at.to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let functions = self.module.declare_data_in_data(functions, &mut data);
        let blob = self.module.declare_data_in_data(blob, &mut data);
        data.write_data_addr(0, functions, 0);
        data.write_data_addr(16, blob, 0);
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
        let name = self.string(&self.types.display(ty));
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

    /// A static, immortal string object: `[refcount][length][bytes]`.
    fn string(&mut self, text: &str) -> DataId {
        if let Some(&id) = self.strings.get(text) {
            return id;
        }
        let mut bytes = Vec::with_capacity(16 + text.len());
        bytes.extend_from_slice(&IMMORTAL.to_le_bytes());
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
            if let Some(debug) = e.cg.debug {
                let debug = e.cg.module.declare_data_in_func(debug, e.fb.func);
                let debug = e.fb.ins().symbol_value(POINTER, debug);
                e.call_rt(Rt::DebugTable, &[debug]);
            }
            // A test build hands the runner its tree before `main` runs it.
            if let Some(table) = table {
                let table = e.cg.module.declare_data_in_func(table, e.fb.func);
                let address = e.fb.ins().symbol_value(POINTER, table);
                let count = e.fb.ins().iconst(types::I64, tests.len() as i64);
                e.call_rt(Rt::TestStart, &[address, count]);
            }
            let callee = e.cg.module.declare_func_in_func(e.cg.functions[main], e.fb.func);
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
                    e.fb.ins().call(callee, &[address]);
                    Val::Memory(address)
                }
                Class::Scalar(_) => {
                    let call = e.fb.ins().call(callee, &[]);
                    Val::Scalar(e.fb.inst_results(call)[0])
                }
                Class::Zero => {
                    e.fb.ins().call(callee, &[]);
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
                e.call_rt(Rt::ReportError, &[description]);
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

/// Code emission shared by function bodies and glue functions.
struct Emit<'a, 'b> {
    cg: &'a mut Codegen<'b>,
    fb: FunctionBuilder<'a>,
}

impl Emit<'_, '_> {
    fn call_rt(&mut self, rt: Rt, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.runtime(rt);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn call_glue(&mut self, kind: Glue, ty: Ty, args: &[ir::Value]) -> Option<ir::Value> {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        let call = self.fb.ins().call(callee, args);
        self.fb.inst_results(call).first().copied()
    }

    fn glue_address(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        let id = self.cg.glue(kind, ty);
        let callee = self.cg.module.declare_func_in_func(id, self.fb.func);
        self.fb.ins().func_addr(POINTER, callee)
    }

    /// A glue function's address, or null when a retain or release of `ty`
    /// has nothing to do.
    fn glue_or_null(&mut self, kind: Glue, ty: Ty) -> ir::Value {
        if matches!(kind, Glue::Retain | Glue::Release) && !self.cg.types.needs_drop(ty)
            || kind == Glue::Visit && !self.cg.types.can_cycle(ty)
        {
            return self.fb.ins().iconst(POINTER, 0);
        }
        self.glue_address(kind, ty)
    }

    fn map_glue_address(&mut self, ty: Ty) -> ir::Value {
        let id = self.cg.map_glue(ty);
        let global = self.cg.module.declare_data_in_func(id, self.fb.func);
        self.fb.ins().symbol_value(POINTER, global)
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
                let word = match *self.cg.types.kind(self.cg.types.repr(ty)) {
                    TyKind::String => self.call_rt(Rt::StrHash, &[value]).unwrap(),
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
                    let count = self.cg.types.components(ty).unwrap().len() as u32;
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
        if let Some(&id) = self.cg.display.get(&ty) {
            // A user type: its `Display.display` returns an owned string.
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let callee = self.cg.module.declare_func_in_func(self.cg.functions[id.0 as usize], self.fb.func);
            let call = self.fb.ins().call(callee, &args);
            let text = self.fb.inst_results(call)[0];
            self.call_rt(Rt::SbStr, &[builder, text]);
            self.call_rt(Rt::StrRelease, &[text]);
            return;
        }
        let types = self.cg.types;
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
        let types = self.cg.types;
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
        let global = self.cg.module.declare_data_in_func(id, self.fb.func);
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

    fn copy_memory(&mut self, destination: ir::Value, source: ir::Value, size: u32) {
        if size > 64 {
            let config = self.cg.module.isa().frontend_config();
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
                let tag = self.cg.scalar_type(self.cg.types.tag_ty(ty));
                self.fb.ins().load(tag, flags(), address, 0)
            }
            Val::Zero => self.fb.ins().iconst(types::I8, 0),
        }
    }

    /// A string describing an error that reached the end of `main`: its text
    /// for a `String`, `Type.Variant` for an enum, otherwise the type name.
    /// With `display`, a type's own `Display` comes first, and the string
    /// is owned.
    fn describe(&mut self, ty: Ty, value: Val, display: bool) -> ir::Value {
        if display && let Some(&id) = self.cg.display.get(&ty) {
            let args: Vec<ir::Value> = match value {
                Val::Scalar(v) | Val::Memory(v) => vec![v],
                Val::Zero => Vec::new(),
            };
            let callee = self.cg.module.declare_func_in_func(self.cg.functions[id.0 as usize], self.fb.func);
            let call = self.fb.ins().call(callee, &args);
            return self.fb.inst_results(call)[0];
        }
        let types = self.cg.types;
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
        if !self.cg.types.needs_drop(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => match *self.cg.types.kind(self.cg.types.repr(ty)) {
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
                let count = self.cg.types.components(ty).unwrap().len() as u32;
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
        } else if self.cg.types.needs_drop(slot.ty) {
            let address = self.offset(base, slot.offset);
            let value = self.value_at(address, slot.ty);
            self.refcount(slot.ty, value, release);
        }
    }

    /// Reports to the cycle collector's `visitor` the references inside
    /// `value` that may lead to a cycle (see `Types::can_cycle`).
    fn visit(&mut self, ty: Ty, value: Val, visitor: ir::Value) {
        if !self.cg.types.can_cycle(ty) {
            return;
        }
        match (value, self.cg.layout(ty).shape) {
            (Val::Scalar(pointer), _) => {
                let zero = self.fb.ins().iconst(POINTER, 0);
                // The kinds of `cycles::Kind` in the runtime.
                let (kind, a, b) = match *self.cg.types.kind(self.cg.types.repr(ty)) {
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
                let count = self.cg.types.components(ty).unwrap().len() as u32;
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
        if !self.cg.types.can_cycle(slot.ty) {
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
            (Val::Scalar(a), Val::Scalar(b)) => match *self.cg.types.kind(self.cg.types.repr(ty)) {
                TyKind::String => self.call_rt(Rt::StrEq, &[a, b]).unwrap(),
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
                    let count = self.cg.types.components(ty).unwrap().len() as u32;
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
    let callee = cg.module.declare_func_in_func(cg.functions[target.0 as usize], fb.func);
    let call = fb.ins().call(callee, &params);
    let results = fb.inst_results(call).to_vec();
    fb.ins().return_(&results);
    fb.seal_all_blocks();
    let config = cg.module.isa().frontend_config();
    fb.finalize(config);
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
    e: Emit<'a, 'b>,
    body: &'a Body,
    /// Every function, for named functions used as values.
    bodies: &'a [Body],
    blocks: Vec<ir::Block>,
    storage: Vec<Storage>,
    /// Where the statement being compiled is, for panics of the runtime.
    span: Span,
    sites: Vec<(u32, Option<Ty>)>,
    /// In a debug build, what its probes show.
    debug: Option<DebugFrame>,
}

/// What a function of a debug build keeps for its probes (toolchain 7.6):
/// its index among the debug table's functions; each binding it shows, and
/// for one in a variable the slot its value is copied to when a probe
/// runs; a bit for each that holds a value now; their addresses, for the
/// probe; and the line the last probe was for, in the block.
struct DebugFrame {
    function: u32,
    shown: Vec<(Local, Option<ir::StackSlot>)>,
    alive: ir::StackSlot,
    addresses: ir::StackSlot,
    line: Option<(String, usize)>,
}

impl<'a, 'b> FnCodegen<'a, 'b> {
    fn new(
        cg: &'a mut Codegen<'b>,
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
        let debug = cg.debug.is_some().then(|| {
            let mut shown = Vec::new();
            let mut described = Vec::new();
            for (index, name) in body.names.iter().enumerate() {
                let (Some(name), local) = (name, Local(index as u32)) else { continue };
                let copy = match storage[index] {
                    Storage::Zero => continue,
                    Storage::Var(_) => {
                        Some(fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3)))
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
            cg.debug_functions.push((body.name.clone(), described));
            DebugFrame { function: cg.debug_functions.len() as u32 - 1, shown, alive, addresses, line: None }
        });
        FnCodegen { e: Emit { cg, fb }, body, bodies, blocks, storage, span: Span::default(), sites: Vec::new(), debug }
    }

    /// Builds the function's IR; gives where its statements are, which the
    /// source locations of its instructions index.
    fn run(mut self) -> Vec<(u32, Option<Ty>)> {
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
        if let Some(debug) = &self.debug {
            let zero = self.e.fb.ins().iconst(types::I64, 0);
            for word in 0..debug.shown.len().div_ceil(64) {
                self.e.fb.ins().stack_store(POINTER, zero, debug.alive, word as i32 * 8);
            }
            for &param in &self.body.params {
                self.mark(param, true);
            }
        }
        for (index, block) in self.body.blocks.iter().enumerate() {
            if index > 0 {
                self.e.fb.switch_to_block(self.blocks[index]);
            } else if self.e.cg.meta {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            if let Some(debug) = &mut self.debug {
                debug.line = None;
            }
            for stmt in &block.stmts {
                self.statement(stmt);
            }
            // A jump back to an earlier block closes a loop: a step.
            let back = match &block.terminator {
                Terminator::Goto(target) => target.0 as usize <= index,
                Terminator::Branch(_, then, otherwise) => then.0 as usize <= index || otherwise.0 as usize <= index,
                _ => false,
            };
            if back && self.e.cg.meta {
                self.e.call_rt(Rt::MetaStep, &[]);
            }
            self.terminator(&block.terminator);
        }
        self.e.fb.seal_all_blocks();
        let config = self.e.cg.module.isa().frontend_config();
        self.e.fb.finalize(config);
        self.sites
    }

    // ---- places and values ---------------------------------------------------

    fn place_ty(&self, place: &Place) -> Ty {
        self.body.place_ty(self.e.cg.types, place)
    }

    fn operand_ty(&self, operand: &Operand) -> Ty {
        match operand {
            Operand::Copy(place) => self.place_ty(place),
            Operand::Const(Const::Int(_, ty) | Const::Float(_, ty)) => *ty,
            Operand::Const(Const::Char(_)) => Ty::CHAR,
            Operand::Const(Const::Bool(_)) => Ty::BOOL,
            Operand::Const(Const::Str(_)) => Ty::STRING,
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
                && matches!(self.e.cg.types.kind(self.e.cg.types.repr(ty)), TyKind::Map(..) | TyKind::Set(_))
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
                ty = project(self.e.cg.types, ty, proj);
                continue;
            }
            if let Proj::Index(index, span) = proj
                && let TyKind::CArray(element, count) = *self.e.cg.types.kind(ty)
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
            if let Proj::Deref = proj {
                assert!(!write, "the value of a `Ref` is never written");
                let reference = match base {
                    Some(address) => self.e.fb.ins().load(POINTER, flags(), address, 0),
                    None => {
                        let Storage::Var(var) = self.storage[place.local.0 as usize] else { unreachable!() };
                        self.e.fb.use_var(var)
                    }
                };
                base = Some(self.e.fb.ins().iadd_imm_s(reference, REF_HEADER));
                ty = project(self.e.cg.types, ty, proj);
                continue;
            }
            // The value inside an option of a reference is the option.
            if base.is_none()
                && let Proj::Variant(SOME, 0) = proj
                && let Shape::Niche = self.e.cg.layout(ty).shape
            {
                ty = project(self.e.cg.types, ty, proj);
                continue;
            }
            if let Proj::Index(index, span) = proj {
                let element = project(self.e.cg.types, ty, proj);
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
                base = Some(self.element_address(sequence, ty, index, span));
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
        let (position, inside) = self.position(sequence, index);
        let outside = self.e.fb.ins().bxor_imm_u(inside, 1);
        let deque = self.e.cg.sequence_parts(ty).1;
        self.panic_values_if(outside, 1, span, |this| {
            let length = this.e.fb.ins().load(types::I64, flags(), sequence, 8);
            ((deque as u32) << 17 | 0x140, index, length, ty)
        });
        self.element_at(sequence, ty, position)
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
            while let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) {
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
        let code = match *self.e.cg.types.kind(self.e.cg.types.repr(number)) {
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
        let size = self.e.element_size(element);
        let extra = self.e.fb.ins().iconst(POINTER, 0);
        let retain = self.e.glue_or_null(Glue::Retain, element);
        let release = self.e.glue_or_null(Glue::Release, element);
        self.e.call_rt(Rt::ArrayUnique, &[slot, size, extra, retain, release]);
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

    fn operand(&mut self, operand: &Operand) -> Val {
        match operand {
            Operand::Copy(place) => self.read(place),
            Operand::Const(Const::Int(value, ty)) => {
                let clif = self.e.cg.scalar_type(*ty);
                // Narrow immediates are zero-extended bit patterns.
                let bits = clif.bits();
                let pattern = if bits == 64 { *value as i64 } else { (*value as i64) & ((1i64 << bits) - 1) };
                Val::Scalar(self.e.fb.ins().iconst(clif, pattern))
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
        self.sites.push((start, ty));
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

    /// In a debug build, a probe where a line starts: the debugger stops
    /// there when it is asked to, and reads the bindings.
    fn probe(&mut self, span: Span) {
        let Some(mut debug) = self.debug.take() else { return };
        let (file, line, _) = self.e.cg.sources.position(span.start);
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
            self.e.cg.debug_sites.push((span.start, debug.function));
            let site = self.e.fb.ins().iconst(types::I64, self.e.cg.debug_sites.len() as i64 - 1);
            let addresses = self.e.fb.ins().stack_addr(POINTER, debug.addresses, 0);
            let alive = self.e.fb.ins().stack_addr(POINTER, debug.alive, 0);
            self.e.call_rt(Rt::DebugLine, &[site, addresses, alive]);
        }
        self.debug = Some(debug);
    }

    fn location(&mut self, span: Span) -> ir::Value {
        let location = self.e.cg.sources.location(span);
        self.e.string_ptr(&location)
    }

    // ---- statements and terminators ----------------------------------------

    fn statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(place, rvalue, span) => {
                // What it allocates: the collection a method changes, or the
                // value it makes.
                let made = match &**rvalue {
                    Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => target,
                    _ => place,
                };
                self.at_typed(*span, Some(self.body.place_ty(self.e.cg.types, made)));
                self.probe(*span);
                self.assign(place, rvalue, *span);
                if place.path.is_empty() {
                    self.mark(place.local, true);
                }
            }
            Statement::DeferPush(body, place) => {
                let callee =
                    self.e.cg.module.declare_func_in_func(self.e.cg.functions[body.0 as usize], self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, callee);
                let mut spilled = Vec::new();
                let closure = self.loan_address(place, &mut spilled);
                self.e.call_rt(Rt::DeferPush, &[code, closure]);
            }
            Statement::DeferPop => {
                self.e.call_rt(Rt::DeferPop, &[]);
            }
            Statement::Clear(local) => {
                self.mark(*local, false);
                let place = Place::local(*local);
                let ty = self.place_ty(&place);
                match self.e.cg.class(ty) {
                    Class::Zero => {}
                    Class::Scalar(clif) if clif.is_float() => {
                        let zero = self.e.fb.ins().f64const(0.0);
                        let zero = if clif == types::F32 { self.e.fb.ins().fdemote(types::F32, zero) } else { zero };
                        self.write(&place, Val::Scalar(zero));
                    }
                    Class::Scalar(clif) => {
                        let zero = self.e.fb.ins().iconst(clif, 0);
                        self.write(&place, Val::Scalar(zero));
                    }
                    Class::Memory => {
                        let address = self.pointer_to(&place, false);
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
                if !self.e.cg.types.needs_drop(ty) {
                    return;
                }
                let value = self.read(place);
                self.e.refcount(ty, value, matches!(stmt, Statement::Release(_)));
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
                for index in 0..7 {
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
                let tag_ty = self.e.cg.types.tag_ty(ty);
                let tag = self.operand(&Operand::Const(Const::Int(*variant as i128, tag_ty)));
                if let Class::Scalar(_) = class {
                    tag
                } else {
                    let base = self.pointer_to(place, true);
                    self.e.store_val(base, tag_ty, tag);
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
                match self.e.cg.types.kind(ty) {
                    TyKind::Result(target, _) => {
                        let target = *target;
                        let (converted, error) = self.convert_checked(value, from, target);
                        self.write_checked(place, ty, converted, error);
                        return;
                    }
                    _ => Val::Scalar(self.convert_total(value, from, ty)),
                }
            }
            Rvalue::Array(values, retain) => {
                let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) else { unreachable!() };
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
                    let TyKind::Array(element) = *self.e.cg.types.kind(self.e.cg.types.repr(ty)) else {
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
                let TyKind::Pointer(element) = *self.e.cg.types.kind(ty) else { unreachable!("a pointer") };
                let (address, count) = (self.scalar(&operands[0]), self.scalar(&operands[1]));
                let size = self.e.element_size(element);
                let bytes = self.e.fb.ins().imul(count, size);
                Val::Scalar(self.e.fb.ins().iadd(address, bytes))
            }
            Rvalue::Pointer(PointerOp::Write, ..) => unreachable!("a write gives `Unit`"),
            Rvalue::Length(collection) => {
                let collection_ty = self.operand_ty(collection);
                let value = self.scalar(collection);
                Val::Scalar(match self.e.cg.types.kind(self.e.cg.types.repr(collection_ty)) {
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
                    StrOp::Trim | StrOp::LowerAscii | StrOp::UpperAscii => {
                        let transform = match op {
                            StrOp::Trim => 0,
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
                let func = self.e.cg.module.declare_func_in_func(adapter, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::ExternFn(id) => {
                let declared = &self.e.cg.externs[*id as usize];
                let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
                let signature = self.e.cg.c_signature(&params, result_ty);
                let (function, _) = self.e.cg.c_function(&symbol, &signature);
                let func = self.e.cg.module.declare_func_in_func(function, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().func_addr(POINTER, func))
            }
            Rvalue::Fixed(value, span) => {
                let from = self.operand_ty(value);
                let types = self.e.cg.types;
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
                let global = self.e.cg.module.declare_data_in_func(identity, self.e.fb.func);
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
                        let types = self.e.cg.types;
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
                let global = self.e.cg.module.declare_data_in_func(data, self.e.fb.func);
                Val::Scalar(self.e.fb.ins().symbol_value(POINTER, global))
            }
            Rvalue::Erase(closure, body, retain) => {
                let env = self.operand_ty(closure);
                let value = self.operand(closure);
                if *retain {
                    self.e.refcount(env, value, false);
                }
                let callee =
                    self.e.cg.module.declare_func_in_func(self.e.cg.functions[body.0 as usize], self.e.fb.func);
                let code = self.e.fb.ins().func_addr(POINTER, callee);
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
                let TyKind::Fn(_, result, ..) = *self.e.cg.types.kind(self.operand_ty(body)) else {
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
            Rvalue::Cancelled => Val::Scalar(self.e.call_rt(Rt::Cancelled, &[]).unwrap()),
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
            Rvalue::Binary(op, left, right) => Val::Scalar(self.binary(*op, left, right, span)),
            Rvalue::Concat(pieces) => Val::Scalar(self.concat(pieces)),
        };
        self.write(place, value);
    }

    /// An intrinsic array operation. Returns the value for scalar results;
    /// results in memory are written to `place` directly.
    fn array_op(&mut self, op: ArrayOp, operands: &[Operand], place: &Place, ty: Ty, span: Span) -> Option<Val> {
        let types = self.e.cg.types;
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
        if let TyKind::Map(..) | TyKind::Set(_) = self.e.cg.types.kind(self.e.cg.types.repr(collection_ty)) {
            return self.mutate_map(mutation, target, &values, collection_ty, place, ty);
        }
        let (element, deque) = self.e.cg.sequence_parts(collection_ty);
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
        let is_set = matches!(self.e.cg.types.kind(self.e.cg.types.repr(map_ty)), TyKind::Set(_));
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
                let TyKind::Handle(handle, state) = *self.e.cg.types.kind(ty) else { unreachable!() };
                if handle.collection() {
                    return Some(Val::Scalar(self.shared_new(handle, state, &operands[0], retain[0])));
                }
            }
            _ => {}
        }
        let handle_ty = operands.first().map(|o| self.operand_ty(o));
        let item = handle_ty.map(|t| self.e.cg.types.handle_result(t));
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
                let result = self.e.cg.types.handle_result(ty);
                let fallible = matches!(self.e.cg.types.kind(result), TyKind::Result(..));
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
                let sender_ty = self.e.cg.types.components(ty).expect("a pair of ends")[0];
                let value = self.e.cg.types.handle_result(sender_ty);
                let (capacity, bounded) = match values.first() {
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
                    .call_rt(Rt::ChannelNew, &[capacity, bounded, size, align, release, visit, location])
                    .unwrap();
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
                let state = self.e.cg.types.handle_result(ty);
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
                Some(Val::Scalar(cell))
            }
            TaskOp::WeakNew => Some(Val::Scalar(self.e.call_rt(Rt::WeakNew, &[values[0]]).unwrap())),
            TaskOp::WeakGet => {
                let reference = self.e.call_rt(Rt::WeakGet, &[values[0]]).unwrap();
                let reference_ty = self.e.cg.types.lookup(&TyKind::Handle(Handle::Ref, item.unwrap())).unwrap();
                self.pointer_option(place, ty, reference_ty, reference)
            }
            TaskOp::RefNew => {
                // The value moves into the new identity.
                let inner = self.e.cg.types.handle_result(ty);
                let value = self.operand(&operands[0]);
                if retain[0] {
                    self.e.refcount(inner, value, false);
                }
                let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(inner).size as i64);
                let object = self.e.call_rt(Rt::RefNew, &[size]).unwrap();
                let address = self.e.fb.ins().iadd_imm_s(object, REF_HEADER);
                self.e.store_val(address, inner, value);
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
                let permit_ty = self.e.cg.types.lookup(&TyKind::Handle(Handle::Permit, Ty::UNIT)).unwrap();
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
            TaskOp::IsClosed => {
                let sender = matches!(self.e.cg.types.kind(handle_ty.unwrap()), TyKind::Handle(Handle::Sender, _));
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
            let item = self.e.cg.types.handle_result(self.operand_ty(&case.operands[0]));
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
                if self.e.cg.types.needs_drop(item) {
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
            Class::Scalar(t) => Val::Scalar(self.e.fb.ins().iconst(t, 0)),
            Class::Memory => {
                let address = self.scratch(ty);
                let layout = self.e.cg.layout(ty);
                let config = self.e.cg.module.target_config();
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
        let TyKind::Option(payload) = *self.e.cg.types.kind(ty) else { unreachable!() };
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
        let TyKind::Handle(handle, state) = *self.e.cg.types.kind(self.operand_ty(&operands[0])) else {
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
                let config = self.e.cg.module.target_config();
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
        let TyKind::Handle(_, state) = *self.e.cg.types.kind(self.operand_ty(&operands[0])) else { unreachable!() };
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
                let signed = self.e.cg.types.as_int(state).is_some_and(|i| i.signed());
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
        let TyKind::Result(success, _) = *self.e.cg.types.kind(ty) else { unreachable!("a result") };
        // Metadata comes as three words; other values as one pointer.
        let out = self.e.fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 24, 3));
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
                    3 | 7 => (zero, zero),
                    // An `Input` or `Output`: its tag and the pointer its
                    // payload holds, the same place in every variant.
                    _ => {
                        let enum_ty = self.operand_ty(&args[1]);
                        let value = self.operand(&args[1]);
                        let tag = self.e.tag(enum_ty, value);
                        let tag = self.e.fb.ins().uextend(types::I64, tag);
                        let payload = match value {
                            Val::Memory(address) => {
                                let variants = self.e.cg.types.variants(enum_ty).unwrap();
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
                let TyKind::Result(result, _) = *self.e.cg.types.kind(ty) else { unreachable!("a result") };
                let statuses = self.e.cg.types.components(result).unwrap()[3];
                let TyKind::Array(status) = *self.e.cg.types.kind(statuses) else { unreachable!("an array") };
                let size = self.e.fb.ins().iconst(types::I64, self.e.cg.layout(status).size as i64);
                let success = self.e.cg.slot(status, Proj::Field(1)).offset;
                let success = self.e.fb.ins().iconst(types::I64, success as i64);
                values.extend([size, success]);
            }
            ProcessFn::Start => {}
            ProcessFn::Exit | ProcessFn::Detach => unreachable!("they return nothing"),
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
        let TyKind::Result(success, error) = *self.e.cg.types.kind(ty) else { unreachable!("a result") };
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
        let output_ty = self.e.cg.types.variants(error).unwrap()[FAILED as usize - 1][1];
        if success == output_ty {
            let (failure, other) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_failure = self.e.fb.ins().icmp_imm_s(IntCC::Equal, status, FAILED);
            self.e.fb.ins().brif(is_failure, failure, &[], other, &[]);
            self.e.fb.switch_to_block(failure);
            let status_ty = self.e.cg.types.variants(error).unwrap()[FAILED as usize - 1][0];
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
        let ok = match self.e.cg.types.components(success).map(|c| c.len()) {
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
        match *self.e.cg.types.kind(ty) {
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
            // `Metadata`: its kind, size and read-only flag.
            TyKind::Adt(_) => {
                let address = self.scratch(ty);
                let kind = word(self, 0);
                let kind = self.e.fb.ins().ireduce(types::I8, kind);
                let size = word(self, 1);
                let read_only = word(self, 2);
                let read_only = self.e.fb.ins().ireduce(types::I8, read_only);
                for (index, value) in [kind, size, read_only].into_iter().enumerate() {
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    self.e.fb.ins().store(flags(), value, address, slot.offset as i32);
                }
                Val::Memory(address)
            }
            _ => Val::Scalar(word(self, 0)),
        }
    }

    /// A conversion that cannot fail (18.6).
    fn convert_total(&mut self, value: ir::Value, from: Ty, to: Ty) -> ir::Value {
        let types = self.e.cg.types;
        let to_clif = self.e.cg.scalar_type(to);
        let from_clif = self.e.cg.scalar_type(from);
        match (types.kind(from), types.kind(to)) {
            (TyKind::Float(_), TyKind::Float(_)) if from_clif == to_clif => value,
            (TyKind::Float(_), TyKind::Float(_)) => self.e.fb.ins().fpromote(to_clif, value),
            (_, TyKind::Float(_)) if types.as_int(from).is_some_and(IntTy::signed) => {
                self.e.fb.ins().fcvt_from_sint(to_clif, value)
            }
            (_, TyKind::Float(_)) => self.e.fb.ins().fcvt_from_uint(to_clif, value),
            _ => self.resize(value, from, to_clif),
        }
    }

    /// Resizes an integer bit pattern, extending by the source's signedness.
    fn resize(&mut self, value: ir::Value, from: Ty, to: ir::Type) -> ir::Value {
        let from_clif = self.e.cg.scalar_type(from);
        let signed = self.e.cg.types.as_int(from).is_some_and(IntTy::signed);
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
        let types = self.e.cg.types;
        let to_clif = self.e.cg.scalar_type(to);
        let range = |ty: Ty| match types.kind(ty) {
            TyKind::Int(int) => (int.min(), int.max()),
            _ => (0, 255),
        };
        let ok = self.e.fb.ins().iconst(types::I8, -1);
        let pick = |e: &mut Emit, failed: ir::Value, code: i64, current: ir::Value| {
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
            TyKind::Float(_) => {
                // Classified in order: NotFinite, NotIntegral, OutOfRange (18.6).
                let from_clif = self.e.cg.scalar_type(from);
                let (min, max) = range(to);
                let not_nan = self.e.fb.ins().fcmp(FloatCC::Equal, value, value);
                let infinite = self.is_infinite(value, from_clif);
                let finite = self.e.fb.ins().bxor_imm_u(infinite, 1);
                let finite = self.e.fb.ins().band(finite, not_nan);
                let truncated = self.e.fb.ins().trunc(value);
                let integral = self.e.fb.ins().fcmp(FloatCC::Equal, truncated, value);
                // `max + 1` and `min` are powers of two, exact in both formats.
                let low = self.float_const(from_clif, min as f64);
                let high = self.float_const(from_clif, (max + 1) as f64);
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
                // x86 converts to 32 or 64 bits only; the range is already checked.
                let wide = if to_clif.bits() < 32 { types::I32 } else { to_clif };
                let converted = if min < 0 {
                    self.e.fb.ins().fcvt_to_sint(wide, safe)
                } else {
                    self.e.fb.ins().fcvt_to_uint(wide, safe)
                };
                let converted = if wide == to_clif { converted } else { self.e.fb.ins().ireduce(to_clif, converted) };
                (converted, error)
            }
            _ => {
                let (min, max) = range(to);
                let wide = self.resize(value, from, types::I64);
                let signed = types.as_int(from).is_some_and(IntTy::signed);
                // Compare in the source's domain, widened to 64 bits.
                let (below, above) = if signed {
                    let below =
                        self.e.fb.ins().icmp_imm_s(IntCC::SignedLessThan, wide, min.max(i64::MIN as i128) as i64);
                    let above = if max >= i64::MAX as i128 {
                        self.e.fb.ins().iconst(types::I8, 0)
                    } else {
                        self.e.fb.ins().icmp_imm_s(IntCC::SignedGreaterThan, wide, max as i64)
                    };
                    (below, above)
                } else {
                    let below = self.e.fb.ins().iconst(types::I8, 0);
                    let above = if max >= u64::MAX as i128 {
                        self.e.fb.ins().iconst(types::I8, 0)
                    } else {
                        self.e.fb.ins().icmp_imm_s(IntCC::UnsignedGreaterThan, wide, max as i64)
                    };
                    (below, above)
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
            let declared = &self.e.cg.externs[id as usize];
            let (params, result_ty, symbol) = (declared.params.clone(), declared.result, declared.symbol.clone());
            return self.c_call(CTarget::Symbol(symbol), &params, result_ty, args, result);
        }
        let mut values: Vec<ir::Value> = result.into_iter().collect();
        let loans = match callee {
            Callee::Fn(id) => self.e.cg.loans[id.0 as usize].clone(),
            Callee::Std(_) => Vec::new(),
            Callee::Trait(..) | Callee::Extern(_) => unreachable!("handled before"),
        };
        let mut spilled = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if loans.get(index).copied().unwrap_or(false) {
                let Operand::Copy(place) = arg else { unreachable!("a loan is a place") };
                values.push(self.loan_address(place, &mut spilled));
                continue;
            }
            match self.operand(arg) {
                Val::Scalar(value) | Val::Memory(value) => values.push(value),
                Val::Zero => {}
            }
        }
        let result = match callee {
            Callee::Fn(id) => {
                let func = self.e.cg.module.declare_func_in_func(self.e.cg.functions[id.0 as usize], self.e.fb.func);
                let call = self.e.fb.ins().call(func, &values);
                self.e.fb.inst_results(call).first().copied()
            }
            Callee::Std(StdFn::ConsolePrint) => self.e.call_rt(Rt::ConsolePrint, &values),
            Callee::Std(StdFn::ConsolePrintln) => self.e.call_rt(Rt::ConsolePrintln, &values),
            Callee::Std(StdFn::ConsoleSet(index)) => {
                let index = self.e.fb.ins().iconst(types::I64, index as i64);
                self.e.call_rt(Rt::ConsoleSet, &[index, values[0]])
            }
            Callee::Std(StdFn::Native(index)) => self.e.call_rt(Rt::Native(index), &values),
            Callee::Std(StdFn::Process(ProcessFn::Exit)) => self.e.call_rt(Rt::Process(ProcessFn::Exit), &values),
            Callee::Std(StdFn::Process(ProcessFn::Detach)) => self.e.call_rt(Rt::Process(ProcessFn::Detach), &values),
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
            Callee::Std(StdFn::ClockBegin) => {
                values.push(self.location(self.span));
                self.e.call_rt(Rt::ClockBegin, &values)
            }
            Callee::Std(StdFn::ClockEnd) => self.e.call_rt(Rt::ClockEnd, &values),
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
            let glue = self.e.cg.module.declare_func_in_func(glue, self.e.fb.func);
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
        if let TyKind::CFn(params, result_ty, _) = self.e.cg.types.kind(self.operand_ty(callee)).clone() {
            let code = self.scalar(callee);
            return self.c_call(CTarget::Address(code), &params, result_ty, args, result);
        }
        let TyKind::Fn(params, result_ty, _, modes, _) = self.e.cg.types.kind(self.operand_ty(callee)).clone() else {
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
        let call = self.e.fb.ins().call_indirect(signature, code, &values);
        let result = self.e.fb.inst_results(call).first().copied();
        for (var, slot) in spilled {
            self.unspill(var, slot);
        }
        result
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

    fn terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(target) => {
                self.e.fb.ins().jump(self.blocks[target.0 as usize], &[]);
            }
            Terminator::Branch(condition, then, otherwise) => {
                let condition = self.scalar(condition);
                let (then, otherwise) = (self.blocks[then.0 as usize], self.blocks[otherwise.0 as usize]);
                self.e.fb.ins().brif(condition, then, &[], otherwise, &[]);
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
        self.e.fb.ins().brif(condition, failed, &[], next, &[]);
        self.e.fb.switch_to_block(failed);
        let (detail, a, b, ty) = values(self);
        let a = self.wide(a, detail & 0x100 != 0);
        let b = self.wide(b, detail & 0x200 != 0);
        let name = self.e.cg.types.display(ty);
        let name = self.e.string_ptr(&name);
        let code = self.e.fb.ins().iconst(types::I32, code as i64);
        let detail = self.e.fb.ins().iconst(types::I32, detail as i64);
        let location = self.location(span);
        let outer = self.e.fb.srcloc();
        let site = self.site(span.start, None);
        self.e.fb.set_srcloc(site);
        self.e.call_rt(Rt::PanicValues, &[code, detail, a, b, name, location]);
        self.e.fb.ins().trap(TrapCode::user(1).unwrap());
        self.e.fb.set_srcloc(outer);
        self.e.fb.switch_to_block(next);
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
            UnaryOp::Neg if matches!(self.e.cg.types.kind(ty), TyKind::Float(_)) => self.e.fb.ins().fneg(value),
            UnaryOp::Neg => {
                let clif = self.e.cg.scalar_type(ty);
                let zero = self.e.fb.ins().iconst(clif, 0);
                let (result, overflow) = self.e.fb.ins().ssub_overflow(zero, value);
                let ty = self.e.cg.types.repr(ty);
                self.panic_values_if(overflow, 5, span, |_| (5 << 16 | 0x100 | clif.bits(), value, value, ty));
                result
            }
        }
    }

    fn concat(&mut self, pieces: &[Piece]) -> ir::Value {
        let builder = self.e.call_rt(Rt::SbNew, &[]).unwrap();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    let text = self.e.string_ptr(text);
                    self.e.call_rt(Rt::SbStr, &[builder, text]);
                }
                Piece::Value(operand, ty) => {
                    let value = self.operand(operand);
                    self.e.append_display(builder, *ty, value);
                }
            }
        }
        self.e.call_rt(Rt::SbFinish, &[builder]).unwrap()
    }

    fn binary(&mut self, op: BinOp, left: &Operand, right: &Operand, span: Span) -> ir::Value {
        let ty = self.operand_ty(left);
        let right_ty = self.operand_ty(right);
        let is_array = |ty: Ty| matches!(self.e.cg.types.kind(self.e.cg.types.repr(ty)), TyKind::Array(_));
        if !matches!(op, BinOp::Eq | BinOp::Ne) && (is_array(ty) || is_array(right_ty)) {
            return self.array_arithmetic(op, left, right, span);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let (a, b) = (self.operand(left), self.operand(right));
            let equal = self.e.equal(ty, a, b);
            return if op == BinOp::Eq { equal } else { self.e.fb.ins().bxor_imm_u(equal, 1) };
        }
        let (a, b) = (self.scalar(left), self.scalar(right));
        if self.e.cg.types.repr(ty) == Ty::STRING {
            let order = self.e.call_rt(Rt::StrCmp, &[a, b]).unwrap();
            return self.e.fb.ins().icmp_imm_s(int_condition(op, true), order, 0);
        }
        let types = self.e.cg.types;
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
            let bits = self.e.cg.types.as_int(ty).map_or(8, IntTy::bits);
            code << 16 | (right_signed as u32) << 9 | (signed as u32) << 8 | bits
        };
        let detail = operation(op);
        match op {
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
                self.panic_values_if(overflow, 5, span, |_| (detail, a, b, ty));
                result
            }
            BinOp::Div | BinOp::Rem => {
                let zero = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.panic_values_if(zero, 3, span, |_| (detail, a, b, ty));
                if !signed {
                    return if op == BinOp::Div { self.e.fb.ins().udiv(a, b) } else { self.e.fb.ins().urem(a, b) };
                }
                // MIN / -1 overflows; MIN % -1 is 0 (spec 18.3).
                let int = types.as_int(ty).unwrap();
                let is_min = self.e.fb.ins().icmp_imm_s(IntCC::Equal, a, int.min() as i64);
                let is_minus_one = self.e.fb.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let both = self.e.fb.ins().band(is_min, is_minus_one);
                if op == BinOp::Div {
                    self.panic_values_if(both, 5, span, |_| (detail, a, b, ty));
                    self.e.fb.ins().sdiv(a, b)
                } else {
                    let one = self.e.fb.ins().iconst(int_type(int), 1);
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
