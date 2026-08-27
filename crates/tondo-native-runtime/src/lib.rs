//! Private runtime primitives used by the first native backend.
//!
//! The native code generator only exchanges `u64` tokens with this library.
//! Tokens are capabilities into a process-local table; they are not pointers,
//! object addresses, or a public FFI.  Keeping the table behind a mutex makes
//! the bootstrap implementation deterministic and safe while the compiler
//! ABI remains explicit about the future atomic fast paths.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

const HANDLE_BIT: u64 = 1 << 63;
const RESULT_NONE: u64 = 0;
const RESULT_SOME: u64 = 1;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;

const STATUS_OK: u64 = 0;
const STATUS_INVALID_HANDLE: u64 = 1;
const STATUS_DOUBLE_RELEASE: u64 = 2;
const STATUS_INVALID_TRANSITION: u64 = 3;
const STATUS_MISSING_ROOT: u64 = 4;
const STATUS_DOUBLE_CLEANUP: u64 = 5;
const STATUS_NOT_READY: u64 = 6;
const STATUS_CANCELLED: u64 = 7;
/// A selection with an `else` arm completed without a ready source.
const STATUS_SELECT_ELSE: u64 = 8;
const STATUS_WEAK_DEAD: u64 = 9;
const STATUS_COUNT_OVERFLOW: u64 = 10;
const STATUS_NOT_WEAK: u64 = 11;
const STATUS_DIAG_CLEAN: u64 = 0;
const STATUS_DIAG_FINDING: u64 = 1;
const STATUS_DIAG_CAPTURED: u64 = 2;
const STATUS_DIAG_UNSUPPORTED: u64 = 3;
const DIAG_PROFILE_RACE: u64 = 0;
const DIAG_PROFILE_LEAK: u64 = 1;
const DIAG_PROFILE_CRASH: u64 = 2;
const DIAG_FIELD_STATUS: u64 = 0;
const DIAG_FIELD_TASK_IDS: u64 = 1;
const DIAG_FIELD_THREAD_IDS: u64 = 2;
const DIAG_FIELD_HAPPENS_BEFORE: u64 = 3;
const DIAG_FIELD_ROOTS: u64 = 4;
const DIAG_FIELD_RETAINERS: u64 = 5;
const DIAG_FIELD_CYCLES_RECLAIMED: u64 = 6;
const DIAG_FIELD_FFI_ALLOCATIONS: u64 = 7;
const DIAG_FIELD_RESOURCES_ACQUIRED: u64 = 8;
const DIAG_FIELD_RESOURCES_RELEASED: u64 = 9;
const DIAG_FIELD_UNWIND_FRAMES: u64 = 10;
const DIAG_FIELD_SOURCE_MAPS: u64 = 11;
const DIAG_FIELD_REDACTED: u64 = 12;
const DIAG_FIELD_PAYLOADS_OMITTED: u64 = 13;
const DIAG_FIELD_CORRUPTION_REJECTED: u64 = 14;
const DIAG_FIELD_LIMIT_ENFORCED: u64 = 15;
const DIAG_FIELD_PROFILE: u64 = 16;
const DIAG_FIELD_MODE: u64 = 17;
const MAX_SELECT_ARMS: u32 = 64;
const COLLECTION_PRESSURE: u32 = 256;

const WORKER_STARTING: u64 = 0;
const WORKER_RUNNING: u64 = 1;
const WORKER_COMPLETED: u64 = 2;
const WORKER_CANCELLED: u64 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Object {
    Result {
        tag: u64,
        payload: Option<u64>,
    },
    Task {
        state: TaskState,
        value: u64,
        kind: TaskKind,
    },
    Scope {
        tasks: Vec<u64>,
        cancelled: bool,
    },
    Select(SelectState),
    OneShot {
        state: OneShotState,
        value: u64,
    },
    Timer {
        state: TimerState,
        value: u64,
    },
    /// A weak handle keeps only the target's tombstone metadata alive.  It
    /// never contributes to strong liveness and is upgraded explicitly.
    Weak {
        target: u64,
    },
    /// Destroyed payload retained solely while weak handles still exist.
    Tombstone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Pending,
    Ready,
    Cancelled,
    Joined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Task,
    Thread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerState {
    Starting,
    Running,
    Completed,
    Cancelled,
}

impl WorkerState {
    fn code(self) -> u64 {
        match self {
            Self::Starting => WORKER_STARTING,
            Self::Running => WORKER_RUNNING,
            Self::Completed => WORKER_COMPLETED,
            Self::Cancelled => WORKER_CANCELLED,
        }
    }

    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkerSnapshot {
    state: WorkerState,
    runs: u64,
    distinct_thread: bool,
}

#[derive(Debug)]
struct WorkerSignal {
    parent: ThreadId,
    snapshot: Mutex<WorkerSnapshot>,
    wake: Condvar,
}

impl WorkerSignal {
    fn new(parent: ThreadId) -> Self {
        Self {
            parent,
            snapshot: Mutex::new(WorkerSnapshot {
                state: WorkerState::Starting,
                runs: 0,
                distinct_thread: false,
            }),
            wake: Condvar::new(),
        }
    }

    fn run(&self) {
        {
            let mut snapshot = self
                .snapshot
                .lock()
                .expect("native worker signal is not poisoned");
            if snapshot.state == WorkerState::Cancelled {
                self.wake.notify_all();
                return;
            }
            snapshot.state = WorkerState::Running;
            snapshot.runs = snapshot.runs.saturating_add(1);
            snapshot.distinct_thread = std::thread::current().id() != self.parent;
            self.wake.notify_all();
        }
        std::thread::yield_now();
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("native worker signal is not poisoned");
        if snapshot.state == WorkerState::Running {
            snapshot.state = WorkerState::Completed;
            self.wake.notify_all();
        }
    }

    fn cancel(&self) {
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("native worker signal is not poisoned");
        if !snapshot.state.terminal() {
            snapshot.state = WorkerState::Cancelled;
            self.wake.notify_all();
        }
    }

    fn wait(&self) -> WorkerSnapshot {
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("native worker signal is not poisoned");
        while !snapshot.state.terminal() {
            snapshot = self
                .wake
                .wait(snapshot)
                .expect("native worker signal is not poisoned");
        }
        *snapshot
    }

    fn snapshot(&self) -> WorkerSnapshot {
        *self
            .snapshot
            .lock()
            .expect("native worker signal is not poisoned")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OneShotState {
    Pending,
    Ready,
    Cancelled,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerState {
    Pending,
    Ready,
    Cancelled,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectPhase {
    Preparing,
    Waiting,
    Committed,
    Consumed,
    Else,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectSourceKind {
    Task,
    OneShot,
    Timer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectArm {
    source: u64,
    kind: SelectSourceKind,
    owned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectState {
    capacity: u32,
    arms: Vec<SelectArm>,
    phase: SelectPhase,
    winner: Option<usize>,
    winner_taken: bool,
    wakeups: u64,
}

#[derive(Debug)]
enum StrongCount {
    Local(u32),
    Shared(AtomicU32),
}

impl StrongCount {
    fn load(&self) -> u32 {
        match self {
            Self::Local(value) => *value,
            Self::Shared(value) => value.load(Ordering::Acquire),
        }
    }

    fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    fn mark_shared(&mut self) -> u64 {
        if self.is_shared() {
            return STATUS_OK;
        }
        let Self::Local(value) = self else {
            unreachable!("shared count was checked above");
        };
        let value = *value;
        *self = Self::Shared(AtomicU32::new(value));
        STATUS_OK
    }

    fn increment(&mut self) -> u64 {
        match self {
            Self::Local(value) => {
                let Some(next) = value.checked_add(1) else {
                    return STATUS_COUNT_OVERFLOW;
                };
                *value = next;
                STATUS_OK
            }
            Self::Shared(value) => value
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(1)
                })
                .map(|_| STATUS_OK)
                .unwrap_or(STATUS_COUNT_OVERFLOW),
        }
    }

    fn decrement(&mut self) -> u64 {
        match self {
            Self::Local(value) => {
                if *value == 0 {
                    return STATUS_DOUBLE_RELEASE;
                }
                *value -= 1;
                STATUS_OK
            }
            Self::Shared(value) => value
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_sub(1)
                })
                .map(|_| STATUS_OK)
                .unwrap_or(STATUS_DOUBLE_RELEASE),
        }
    }
}

#[derive(Debug)]
struct Entry {
    strong: StrongCount,
    weak: u32,
    root_count: u32,
    runtime_roots: u32,
    alive: bool,
    object: ObjectKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectKind {
    Result,
    Task,
    Scope,
    Select,
    OneShot,
    Timer,
    Weak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeferSlot {
    id: u64,
    active: bool,
}

#[derive(Debug, Default)]
struct Frame {
    roots: BTreeMap<u64, u32>,
    defers: Vec<DeferSlot>,
    terminal: bool,
}

/// Bounded, opt-in observability for the native diagnostic lane.
///
/// The capture stores logical identities and counters only.  It never retains
/// addresses, OS thread IDs, user payloads or physical paths, so enabling the
/// lane cannot accidentally turn the private runtime ABI into a layout/FFI
/// promise.  The normal runtime keeps this as `None` and therefore pays no
/// collection or serialization cost.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticCapture {
    profile: u64,
    mode: u64,
    status: u64,
    task_ids: BTreeSet<u64>,
    thread_ids: BTreeSet<u64>,
    happens_before_edges: u64,
    roots: u64,
    retainers: u64,
    cycles_reclaimed: u64,
    ffi_allocations: u64,
    resources_acquired: u64,
    resources_released: u64,
    unwind_frames: u64,
    source_maps: u64,
    redacted: bool,
    payloads_omitted: bool,
    corruption_rejected: bool,
    limit_enforced: bool,
}

impl DiagnosticCapture {
    fn new(profile: u64, mode: u64) -> Self {
        Self {
            profile,
            mode,
            status: STATUS_DIAG_UNSUPPORTED,
            task_ids: BTreeSet::new(),
            thread_ids: BTreeSet::new(),
            happens_before_edges: 0,
            roots: 0,
            retainers: 0,
            cycles_reclaimed: 0,
            ffi_allocations: 0,
            resources_acquired: 0,
            resources_released: 0,
            unwind_frames: 0,
            source_maps: 0,
            redacted: true,
            payloads_omitted: true,
            corruption_rejected: false,
            limit_enforced: false,
        }
    }

    fn field(&self, field: u64) -> u64 {
        match field {
            DIAG_FIELD_STATUS => self.status,
            DIAG_FIELD_TASK_IDS => self.task_ids.len() as u64,
            DIAG_FIELD_THREAD_IDS => self.thread_ids.len() as u64,
            DIAG_FIELD_HAPPENS_BEFORE => self.happens_before_edges,
            DIAG_FIELD_ROOTS => self.roots,
            DIAG_FIELD_RETAINERS => self.retainers,
            DIAG_FIELD_CYCLES_RECLAIMED => self.cycles_reclaimed,
            DIAG_FIELD_FFI_ALLOCATIONS => self.ffi_allocations,
            DIAG_FIELD_RESOURCES_ACQUIRED => self.resources_acquired,
            DIAG_FIELD_RESOURCES_RELEASED => self.resources_released,
            DIAG_FIELD_UNWIND_FRAMES => self.unwind_frames,
            DIAG_FIELD_SOURCE_MAPS => self.source_maps,
            DIAG_FIELD_REDACTED => u64::from(self.redacted),
            DIAG_FIELD_PAYLOADS_OMITTED => u64::from(self.payloads_omitted),
            DIAG_FIELD_CORRUPTION_REJECTED => u64::from(self.corruption_rejected),
            DIAG_FIELD_LIMIT_ENFORCED => u64::from(self.limit_enforced),
            DIAG_FIELD_PROFILE => self.profile,
            DIAG_FIELD_MODE => self.mode,
            _ => u64::MAX,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    objects: BTreeMap<u64, (Entry, Object)>,
    frames: BTreeMap<u64, Frame>,
    next_frame: u64,
    last_status: u64,
    select_rotation: u64,
    thread_workers: BTreeMap<u64, Arc<WorkerSignal>>,
    allocations_since_collection: u32,
    diagnostic: Option<DiagnosticCapture>,
}

impl State {
    fn new() -> Self {
        Self {
            next_id: 1,
            next_frame: 1,
            select_rotation: 0,
            ..Self::default()
        }
    }

    fn alloc(&mut self, object: Object, object_kind: ObjectKind) -> u64 {
        let Some(next_id) = self.next_id.checked_add(1) else {
            self.last_status = STATUS_COUNT_OVERFLOW;
            return 0;
        };
        let id = HANDLE_BIT | self.next_id;
        self.next_id = next_id;
        if let Some(capture) = self.diagnostic.as_mut()
            && object_kind == ObjectKind::Task
        {
            capture.task_ids.insert(id);
        }
        self.objects.insert(
            id,
            (
                Entry {
                    strong: StrongCount::Local(1),
                    weak: 0,
                    root_count: 0,
                    runtime_roots: 0,
                    alive: true,
                    object: object_kind,
                },
                object,
            ),
        );
        self.retain_children(id);
        self.allocations_since_collection = self.allocations_since_collection.saturating_add(1);
        if self.allocations_since_collection >= COLLECTION_PRESSURE {
            let _ = self.collect_cycles();
        }
        id
    }

    fn object(&self, handle: u64) -> Option<&Object> {
        self.objects
            .get(&handle)
            .and_then(|(entry, object)| entry.alive.then_some(object))
    }

    fn object_mut(&mut self, handle: u64) -> Option<&mut Object> {
        self.objects
            .get_mut(&handle)
            .and_then(|(entry, object)| entry.alive.then_some(object))
    }

    fn entry_mut(&mut self, handle: u64) -> Option<&mut Entry> {
        self.objects
            .get_mut(&handle)
            .and_then(|(entry, _)| entry.alive.then_some(entry))
    }

    fn raw_entry(&self, handle: u64) -> Option<&Entry> {
        self.objects.get(&handle).map(|(entry, _)| entry)
    }

    fn raw_object(&self, handle: u64) -> Option<&Object> {
        self.objects.get(&handle).map(|(_, object)| object)
    }

    fn valid_handle(handle: u64) -> bool {
        handle & HANDLE_BIT != 0
    }

    fn live_handle(&self, handle: u64) -> bool {
        Self::valid_handle(handle)
            && self.raw_entry(handle).is_some_and(|entry| {
                entry.alive
                    && (entry.strong.load() > 0 || entry.root_count > 0 || entry.runtime_roots > 0)
            })
    }

    fn status(&mut self, status: u64) -> u64 {
        if status != STATUS_OK {
            self.last_status = status;
        }
        status
    }

    fn strong_children_of(object: &Object) -> Vec<u64> {
        let mut children = Vec::new();
        match object {
            Object::Result {
                payload: Some(value),
                ..
            }
            | Object::Task { value, .. }
            | Object::OneShot { value, .. }
            | Object::Timer { value, .. } => children.push(*value),
            Object::Scope { tasks, .. } => children.extend(tasks.iter().copied()),
            Object::Select(selection) => {
                children.extend(selection.arms.iter().map(|arm| arm.source));
            }
            Object::Weak { .. } | Object::Tombstone | Object::Result { .. } => {}
        }
        children
    }

    fn retain_children(&mut self, owner: u64) {
        let children = self
            .raw_object(owner)
            .map(Self::strong_children_of)
            .unwrap_or_default();
        for child in children {
            if self.live_handle(child) {
                let _ = self.retain(child);
            }
        }
    }

    fn retain(&mut self, handle: u64) -> u64 {
        let status = {
            let Some(entry) = self.entry_mut(handle) else {
                return self.status(STATUS_INVALID_HANDLE);
            };
            if entry.strong.load() == 0 && entry.root_count == 0 && entry.runtime_roots == 0 {
                STATUS_INVALID_HANDLE
            } else {
                entry.strong.increment()
            }
        };
        self.status(status)
    }

    fn release(&mut self, handle: u64) -> u64 {
        let Some(entry) = self.entry_mut(handle) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        let status = entry.strong.decrement();
        if status != STATUS_OK {
            return self.status(status);
        }
        let mut pending = VecDeque::new();
        self.queue_if_unowned(handle, &mut pending);
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn clone_value(&mut self, handle: u64) -> u64 {
        let Some((entry, object)) = self.objects.get(&handle) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        };
        if !entry.alive
            || (entry.strong.load() == 0 && entry.root_count == 0 && entry.runtime_roots == 0)
        {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        }
        if matches!(object, Object::Weak { .. } | Object::Tombstone) {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        if entry.strong.load() == 1 {
            return handle;
        }
        self.alloc(object.clone(), entry.object)
    }

    fn mark_shared(&mut self, handle: u64) -> u64 {
        let status = {
            let Some(entry) = self.entry_mut(handle) else {
                return self.status(STATUS_INVALID_HANDLE);
            };
            if entry.strong.load() == 0 {
                STATUS_INVALID_HANDLE
            } else {
                entry.strong.mark_shared()
            }
        };
        self.status(status)
    }

    fn arc_kind(&mut self, handle: u64) -> u64 {
        let Some(entry) = self.raw_entry(handle) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return u64::MAX;
        };
        if !entry.alive || entry.strong.load() == 0 {
            self.last_status = STATUS_INVALID_HANDLE;
            return u64::MAX;
        }
        u64::from(entry.strong.is_shared())
    }

    fn strong_count(&mut self, handle: u64) -> u64 {
        let Some(entry) = self.raw_entry(handle) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return u64::MAX;
        };
        if !entry.alive {
            self.last_status = STATUS_INVALID_HANDLE;
            return u64::MAX;
        }
        u64::from(entry.strong.load())
    }

    fn weak_count(&mut self, handle: u64) -> u64 {
        let Some(entry) = self.raw_entry(handle) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return u64::MAX;
        };
        u64::from(entry.weak)
    }

    fn live_object_count(&self) -> u64 {
        self.objects
            .values()
            .filter(|(entry, _)| entry.alive)
            .count() as u64
    }

    fn queue_if_unowned(&mut self, handle: u64, pending: &mut VecDeque<u64>) {
        let Some(entry) = self.raw_entry(handle) else {
            return;
        };
        if entry.alive
            && entry.strong.load() == 0
            && entry.root_count == 0
            && entry.runtime_roots == 0
        {
            pending.push_back(handle);
        }
    }

    fn mark_destroyed(&mut self, handle: u64) -> Option<Object> {
        let (entry, object) = self.objects.get_mut(&handle)?;
        if !entry.alive
            || entry.strong.load() != 0
            || entry.root_count != 0
            || entry.runtime_roots != 0
        {
            return None;
        }
        entry.alive = false;
        Some(std::mem::replace(object, Object::Tombstone))
    }

    fn mark_collected(&mut self, handle: u64) -> Option<Object> {
        let (entry, object) = self.objects.get_mut(&handle)?;
        if !entry.alive || entry.root_count != 0 || entry.runtime_roots != 0 {
            return None;
        }
        entry.alive = false;
        Some(std::mem::replace(object, Object::Tombstone))
    }

    fn decrement_weak_edge(&mut self, target: u64) {
        let remove = if let Some((entry, _)) = self.objects.get_mut(&target) {
            entry.weak = entry.weak.saturating_sub(1);
            !entry.alive && entry.weak == 0
        } else {
            false
        };
        if remove {
            self.objects.remove(&target);
            self.thread_workers.remove(&target);
        }
    }

    fn release_strong_edge(&mut self, child: u64, pending: &mut VecDeque<u64>) {
        let Some(entry) = self.entry_mut(child) else {
            return;
        };
        if entry.strong.decrement() == STATUS_OK {
            self.queue_if_unowned(child, pending);
        }
    }

    fn drain_destruction(&mut self, pending: &mut VecDeque<u64>) {
        while let Some(handle) = pending.pop_front() {
            let Some(object) = self.mark_destroyed(handle) else {
                continue;
            };
            self.cleanup_destroyed_object(handle, &object, pending);
            let children = Self::strong_children_of(&object);
            if let Object::Weak { target } = object {
                self.decrement_weak_edge(target);
            }
            for child in children {
                self.release_strong_edge(child, pending);
            }
            let remove = self
                .raw_entry(handle)
                .is_some_and(|entry| !entry.alive && entry.weak == 0);
            if remove {
                self.objects.remove(&handle);
                self.thread_workers.remove(&handle);
            }
        }
    }

    /// Drops owner-scoped state before the object's strong edges are released.
    /// A scope owns its children and therefore cancels/discards them when the
    /// scope itself goes away; a select owns only the arms explicitly marked
    /// `owned`.  Task payloads are moved out before their child edge is
    /// decremented so cancellation and destruction are both leak-free.
    fn cleanup_destroyed_object(
        &mut self,
        handle: u64,
        object: &Object,
        pending: &mut VecDeque<u64>,
    ) {
        match object {
            Object::Task {
                kind: TaskKind::Thread,
                ..
            } => {
                if let Some(signal) = self.thread_workers.get(&handle) {
                    signal.cancel();
                }
            }
            Object::Scope { tasks, .. } => {
                for task in tasks.iter().copied() {
                    self.discard_owned_task(task, pending);
                }
            }
            Object::Select(selection) => {
                for arm in selection.arms.iter().copied().filter(|arm| arm.owned) {
                    self.discard_select_source_with_pending(arm, pending);
                }
            }
            _ => {}
        }
    }

    fn discard_owned_task(&mut self, task: u64, pending: &mut VecDeque<u64>) {
        let (state, kind) = match self.object(task) {
            Some(Object::Task { state, kind, .. }) => (*state, *kind),
            _ => return,
        };
        match state {
            TaskState::Pending => {
                self.release_task_value(task, pending);
                if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                    *state = TaskState::Cancelled;
                }
                if kind == TaskKind::Thread
                    && let Some(signal) = self.thread_workers.get(&task)
                {
                    signal.cancel();
                }
                self.clear_runtime_root_into(task, pending);
                self.notify_selects(task);
            }
            TaskState::Ready => {
                self.release_task_value(task, pending);
                if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                    *state = TaskState::Joined;
                }
                self.notify_selects(task);
            }
            TaskState::Cancelled | TaskState::Joined => {}
        }
    }

    fn weak_new(&mut self, target: u64) -> u64 {
        let Some(entry) = self.raw_entry(target) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if !entry.alive {
            return self.status(STATUS_WEAK_DEAD);
        }
        if entry.weak == u32::MAX {
            return self.status(STATUS_COUNT_OVERFLOW);
        }
        if let Some(entry) = self.objects.get_mut(&target).map(|(entry, _)| entry) {
            entry.weak += 1;
        }
        let weak = self.alloc(Object::Weak { target }, ObjectKind::Weak);
        if weak == 0 {
            self.decrement_weak_edge(target);
        }
        weak
    }

    fn weak_upgrade(&mut self, weak: u64) -> u64 {
        let Some(Object::Weak { target }) = self.object(weak).cloned() else {
            self.last_status = if self.raw_entry(weak).is_some() {
                STATUS_NOT_WEAK
            } else {
                STATUS_INVALID_HANDLE
            };
            return 0;
        };
        let Some(entry) = self.entry_mut(target) else {
            self.last_status = STATUS_WEAK_DEAD;
            return 0;
        };
        if !entry.alive {
            self.last_status = STATUS_WEAK_DEAD;
            return 0;
        }
        if entry.strong.increment() != STATUS_OK {
            self.last_status = STATUS_COUNT_OVERFLOW;
            return 0;
        }
        target
    }

    fn collect_cycles(&mut self) -> u64 {
        self.allocations_since_collection = 0;
        let live = self
            .objects
            .iter()
            .filter_map(|(handle, (entry, _))| entry.alive.then_some(*handle))
            .collect::<Vec<_>>();
        if live.is_empty() {
            return 0;
        }

        // Trial deletion: subtract every strong edge from the observed strong
        // count.  The remainder represents an external owner.  Roots and
        // runtime pins are additional external owners even though they are
        // deliberately tracked outside the user-visible strong count.
        let mut internal_incoming = BTreeMap::<u64, u32>::new();
        for handle in &live {
            internal_incoming.insert(*handle, 0);
        }
        for handle in &live {
            if let Some(object) = self.object(*handle) {
                for child in Self::strong_children_of(object) {
                    if let Some(incoming) = internal_incoming.get_mut(&child) {
                        *incoming = incoming.saturating_add(1);
                    }
                }
            }
        }

        let mut reachable = BTreeSet::new();
        let mut queue = VecDeque::new();
        for handle in &live {
            let Some((entry, _)) = self.objects.get(handle) else {
                continue;
            };
            let external = entry
                .strong
                .load()
                .saturating_sub(internal_incoming[handle]);
            if external > 0 || entry.root_count > 0 || entry.runtime_roots > 0 {
                queue.push_back(*handle);
            }
        }
        while let Some(handle) = queue.pop_front() {
            if !reachable.insert(handle) {
                continue;
            }
            let children = self
                .object(handle)
                .map(Self::strong_children_of)
                .unwrap_or_default();
            queue.extend(
                children
                    .into_iter()
                    .filter(|child| !reachable.contains(child)),
            );
        }

        let doomed = live
            .into_iter()
            .filter(|handle| !reachable.contains(handle))
            .collect::<BTreeSet<_>>();
        if doomed.is_empty() {
            return 0;
        }

        // Remove a whole unreachable component as one unit.  Internal edges
        // are not decremented individually (that would report transient
        // underflows); only edges crossing out of the component are released.
        let mut removed = Vec::new();
        for handle in &doomed {
            if let Some(object) = self.mark_collected(*handle) {
                removed.push((*handle, object));
            }
        }
        let mut pending = VecDeque::new();
        for (handle, object) in removed {
            if matches!(
                &object,
                Object::Task {
                    kind: TaskKind::Thread,
                    ..
                }
            ) && let Some(signal) = self.thread_workers.get(&handle)
            {
                signal.cancel();
            }
            let children = Self::strong_children_of(&object);
            if let Object::Weak { target } = &object {
                self.decrement_weak_edge(*target);
            }
            for child in children {
                if !doomed.contains(&child) {
                    self.release_strong_edge(child, &mut pending);
                }
            }
            if self
                .raw_entry(handle)
                .is_some_and(|entry| !entry.alive && entry.weak == 0)
            {
                self.objects.remove(&handle);
                self.thread_workers.remove(&handle);
            }
        }
        self.drain_destruction(&mut pending);
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.cycles_reclaimed = capture.cycles_reclaimed.saturating_add(doomed.len() as u64);
        }
        doomed.len() as u64
    }

    fn quiesce(&mut self) -> u64 {
        self.collect_cycles()
    }

    fn create_frame(&mut self) -> u64 {
        let frame = self.next_frame;
        self.next_frame = self.next_frame.saturating_add(1);
        self.frames.insert(frame, Frame::default());
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.unwind_frames = capture.unwind_frames.saturating_add(1);
        }
        frame
    }

    fn publish_root(&mut self, frame: u64, value: u64) -> u64 {
        if !self.live_handle(value) {
            return STATUS_INVALID_HANDLE;
        }
        let Some(entry) = self.raw_entry(value) else {
            return STATUS_INVALID_HANDLE;
        };
        if entry.root_count == u32::MAX {
            return STATUS_COUNT_OVERFLOW;
        }
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        let next_count = frame_state
            .roots
            .get(&value)
            .copied()
            .unwrap_or(0)
            .checked_add(1);
        let Some(next_count) = next_count else {
            return STATUS_COUNT_OVERFLOW;
        };
        frame_state.roots.insert(value, next_count);
        if let Some(entry) = self.entry_mut(value) {
            entry.root_count += 1;
        }
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.roots = capture.roots.saturating_add(1);
        }
        STATUS_OK
    }

    fn unpublish_root(&mut self, frame: u64, value: u64) -> u64 {
        let Some(entry) = self.raw_entry(value) else {
            return STATUS_INVALID_HANDLE;
        };
        if !entry.alive || entry.root_count == 0 {
            return STATUS_MISSING_ROOT;
        }
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        let Some(count) = frame_state.roots.get_mut(&value) else {
            return STATUS_MISSING_ROOT;
        };
        *count -= 1;
        if *count == 0 {
            frame_state.roots.remove(&value);
        }
        self.entry_mut(value)
            .expect("root entry was validated above")
            .root_count -= 1;
        let mut pending = VecDeque::new();
        self.queue_if_unowned(value, &mut pending);
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn register_defer(&mut self, frame: u64, id: u64) -> u64 {
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        if frame_state
            .defers
            .iter()
            .any(|defer| defer.id == id && defer.active)
        {
            return STATUS_INVALID_TRANSITION;
        }
        frame_state.defers.push(DeferSlot { id, active: true });
        STATUS_OK
    }

    fn disarm_defer(&mut self, frame: u64, id: u64) -> u64 {
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        let Some(defer) = frame_state
            .defers
            .iter_mut()
            .rev()
            .find(|defer| defer.id == id && defer.active)
        else {
            return STATUS_DOUBLE_CLEANUP;
        };
        defer.active = false;
        STATUS_OK
    }

    fn cleanup_frame(&mut self, frame: u64, aborting: bool) -> u64 {
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        if frame_state.terminal {
            return STATUS_DOUBLE_CLEANUP;
        }
        // LIFO is the language contract.  Abort and normal return use the
        // same exact-once state transition; the caller only chooses whether
        // the surrounding result is returned or trapped.
        for defer in frame_state.defers.iter_mut().rev() {
            if defer.active {
                defer.active = false;
            }
        }
        frame_state.terminal = true;
        let roots = frame_state
            .roots
            .iter()
            .map(|(root, count)| (*root, *count))
            .collect::<Vec<_>>();
        for (root, count) in roots {
            for _ in 0..count {
                let _ = self.unpublish_root(frame, root);
            }
        }
        let _ = aborting;
        STATUS_OK
    }

    fn task_spawn_with_kind(
        &mut self,
        scope: Option<u64>,
        value: u64,
        pending: bool,
        kind: TaskKind,
    ) -> u64 {
        if let Some(scope) = scope {
            match self.object(scope) {
                Some(Object::Scope {
                    cancelled: false, ..
                }) => {}
                Some(Object::Scope { .. }) => {
                    self.last_status = STATUS_INVALID_TRANSITION;
                    return 0;
                }
                _ => {
                    self.last_status = STATUS_INVALID_HANDLE;
                    return 0;
                }
            }
        }
        let task = self.alloc(
            Object::Task {
                state: if pending {
                    TaskState::Pending
                } else {
                    TaskState::Ready
                },
                value,
                kind,
            },
            ObjectKind::Task,
        );
        if task == 0 {
            return 0;
        }
        if let Some(scope) = scope {
            if let Some(Object::Scope { tasks, .. }) = self.object_mut(scope) {
                tasks.push(task);
                let _ = self.retain(task);
            } else {
                self.last_status = STATUS_INVALID_HANDLE;
            }
        }
        task
    }

    fn task_spawn(&mut self, scope: Option<u64>, value: u64, pending: bool) -> u64 {
        self.task_spawn_with_kind(scope, value, pending, TaskKind::Task)
    }

    fn thread_spawn(&mut self, value: u64, pending: bool) -> u64 {
        let task = self.task_spawn_with_kind(None, value, pending, TaskKind::Thread);
        if task == 0 {
            return 0;
        }
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.thread_ids.insert(task);
        }
        if let Some(entry) = self.entry_mut(task) {
            entry.runtime_roots = 1;
        }
        let signal = Arc::new(WorkerSignal::new(std::thread::current().id()));
        self.thread_workers.insert(task, Arc::clone(&signal));
        let worker_signal = Arc::clone(&signal);
        if std::thread::Builder::new()
            .name(format!("tondo-thread-{}", task & !HANDLE_BIT))
            .spawn(move || worker_signal.run())
            .is_err()
        {
            signal.cancel();
            if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                *state = TaskState::Cancelled;
            }
            self.clear_runtime_root(task);
            self.last_status = STATUS_INVALID_TRANSITION;
        }
        task
    }

    fn release_task_value(&mut self, task: u64, pending: &mut VecDeque<u64>) {
        let value = match self.object_mut(task) {
            Some(Object::Task { value, .. }) => std::mem::take(value),
            _ => return,
        };
        if self.live_handle(value) {
            self.release_strong_edge(value, pending);
        }
    }

    fn release_slot_value(
        &mut self,
        source: u64,
        kind: SelectSourceKind,
        pending: &mut VecDeque<u64>,
    ) {
        let value = match (kind, self.object_mut(source)) {
            (SelectSourceKind::Task, Some(Object::Task { value, .. }))
            | (SelectSourceKind::OneShot, Some(Object::OneShot { value, .. }))
            | (SelectSourceKind::Timer, Some(Object::Timer { value, .. })) => std::mem::take(value),
            _ => return,
        };
        if self.live_handle(value) {
            self.release_strong_edge(value, pending);
        }
    }

    fn replace_task_value(&mut self, task: u64, value: u64, pending: &mut VecDeque<u64>) -> u64 {
        if !matches!(self.object(task), Some(Object::Task { .. })) {
            return STATUS_INVALID_HANDLE;
        }
        if self.live_handle(value) {
            let status = self.retain(value);
            if status != STATUS_OK {
                return status;
            }
        }
        let previous = match self.object_mut(task) {
            Some(Object::Task { value: slot, .. }) => std::mem::replace(slot, value),
            _ => unreachable!("task was validated before retaining its value"),
        };
        if self.live_handle(previous) {
            self.release_strong_edge(previous, pending);
        }
        STATUS_OK
    }

    fn clear_runtime_root_into(&mut self, task: u64, pending: &mut VecDeque<u64>) {
        if let Some(entry) = self.entry_mut(task) {
            entry.runtime_roots = 0;
        }
        self.queue_if_unowned(task, pending);
    }

    fn clear_runtime_root(&mut self, task: u64) {
        let mut pending = VecDeque::new();
        self.clear_runtime_root_into(task, &mut pending);
        self.drain_destruction(&mut pending);
    }

    fn clear_runtime_root_if_terminal(&mut self, task: u64) {
        let terminal = matches!(
            self.object(task),
            Some(Object::Task {
                state: TaskState::Ready | TaskState::Cancelled,
                ..
            })
        );
        if terminal {
            self.clear_runtime_root(task);
        }
    }

    fn thread_worker_signal(&self, task: u64) -> Option<Arc<WorkerSignal>> {
        match self.object(task) {
            Some(Object::Task {
                kind: TaskKind::Thread,
                ..
            }) => self.thread_workers.get(&task).cloned(),
            _ => None,
        }
    }

    fn thread_worker_snapshot(&self, task: u64) -> Option<WorkerSnapshot> {
        self.thread_worker_signal(task)
            .map(|signal| signal.snapshot())
    }

    fn thread_worker_status(&self, task: u64) -> u64 {
        self.thread_worker_snapshot(task)
            .map_or(u64::MAX, |snapshot| snapshot.state.code())
    }

    fn thread_worker_runs(&self, task: u64) -> u64 {
        self.thread_worker_snapshot(task)
            .map_or(u64::MAX, |snapshot| snapshot.runs)
    }

    fn thread_worker_distinct(&self, task: u64) -> u64 {
        self.thread_worker_snapshot(task)
            .map_or(u64::MAX, |snapshot| u64::from(snapshot.distinct_thread))
    }

    fn task_wake(&mut self, task: u64) -> u64 {
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state == TaskState::Cancelled || *state == TaskState::Joined {
            return STATUS_INVALID_TRANSITION;
        }
        *state = TaskState::Ready;
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.happens_before_edges = capture.happens_before_edges.saturating_add(1);
        }
        self.notify_selects(task);
        STATUS_OK
    }

    fn task_poll(&self, task: u64) -> u64 {
        match self.object(task) {
            Some(Object::Task {
                state: TaskState::Pending,
                ..
            }) => 0,
            Some(Object::Task {
                state: TaskState::Ready,
                ..
            }) => 1,
            Some(Object::Task {
                state: TaskState::Cancelled,
                ..
            }) => 2,
            Some(Object::Task {
                state: TaskState::Joined,
                ..
            }) => 3,
            None => STATUS_INVALID_HANDLE,
            _ => STATUS_INVALID_HANDLE,
        }
    }

    fn task_take(&mut self, task: u64) -> u64 {
        let value = {
            let Some(Object::Task { state, value, .. }) = self.object_mut(task) else {
                return 0;
            };
            if *state != TaskState::Ready {
                self.last_status = if *state == TaskState::Cancelled {
                    STATUS_CANCELLED
                } else {
                    STATUS_NOT_READY
                };
                return 0;
            }
            *state = TaskState::Joined;
            std::mem::take(value)
        };
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.happens_before_edges = capture.happens_before_edges.saturating_add(1);
        }
        value
    }

    /// Completes a task whose callable body was deliberately published as a
    /// pending handle. The native lowering evaluates that body at the join
    /// edge, then commits the value through this single transition so the
    /// ordinary await/select ownership rules remain unchanged.
    fn task_complete(&mut self, task: u64, value: u64) -> u64 {
        let state = match self.object(task) {
            Some(Object::Task { state, .. }) => *state,
            _ => return STATUS_INVALID_HANDLE,
        };
        if state != TaskState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        let mut pending = VecDeque::new();
        let status = self.replace_task_value(task, value, &mut pending);
        if status != STATUS_OK {
            self.drain_destruction(&mut pending);
            return self.status(status);
        }
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        *state = TaskState::Ready;
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.happens_before_edges = capture.happens_before_edges.saturating_add(1);
        }
        self.notify_selects(task);
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn task_cancel(&mut self, task: u64) -> u64 {
        let (state, kind) = match self.object(task) {
            Some(Object::Task { state, kind, .. }) => (*state, *kind),
            _ => return STATUS_INVALID_HANDLE,
        };
        if state != TaskState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        let mut pending = VecDeque::new();
        self.release_task_value(task, &mut pending);
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return STATUS_INVALID_HANDLE;
        };
        *state = TaskState::Cancelled;
        if kind == TaskKind::Thread
            && let Some(signal) = self.thread_workers.get(&task)
        {
            signal.cancel();
        }
        self.notify_selects(task);
        if kind == TaskKind::Thread {
            self.clear_runtime_root(task);
        }
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn scope_remove_task(&mut self, scope: u64, task: u64, pending: &mut VecDeque<u64>) -> u64 {
        let Some(Object::Scope { tasks, .. }) = self.object_mut(scope) else {
            return STATUS_INVALID_HANDLE;
        };
        let Some(index) = tasks.iter().position(|candidate| *candidate == task) else {
            return STATUS_INVALID_TRANSITION;
        };
        tasks.remove(index);
        self.release_strong_edge(task, pending);
        STATUS_OK
    }

    fn source_kind(&self, source: u64) -> Option<SelectSourceKind> {
        match self.object(source) {
            Some(Object::Task { .. }) => Some(SelectSourceKind::Task),
            Some(Object::OneShot { .. }) => Some(SelectSourceKind::OneShot),
            Some(Object::Timer { .. }) => Some(SelectSourceKind::Timer),
            _ => None,
        }
    }

    fn source_ready(&self, source: u64, kind: SelectSourceKind) -> Option<bool> {
        match (kind, self.object(source)) {
            (SelectSourceKind::Task, Some(Object::Task { state, .. })) => {
                Some(matches!(state, TaskState::Ready | TaskState::Cancelled))
            }
            (SelectSourceKind::OneShot, Some(Object::OneShot { state, .. })) => Some(matches!(
                state,
                OneShotState::Ready | OneShotState::Cancelled
            )),
            (SelectSourceKind::Timer, Some(Object::Timer { state, .. })) => {
                Some(matches!(state, TimerState::Ready | TimerState::Cancelled))
            }
            _ => None,
        }
    }

    fn notify_selects(&mut self, source: u64) {
        let selections = self
            .objects
            .iter()
            .filter_map(|(handle, (_, object))| match object {
                Object::Select(selection)
                    if selection.phase == SelectPhase::Waiting
                        && selection.arms.iter().any(|arm| arm.source == source) =>
                {
                    Some(*handle)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for selection in selections {
            if let Some(Object::Select(state)) = self.object_mut(selection) {
                state.wakeups = state.wakeups.saturating_add(1);
            }
        }
    }

    fn discard_select_source(&mut self, arm: SelectArm) {
        let mut pending = VecDeque::new();
        self.discard_select_source_with_pending(arm, &mut pending);
        self.drain_destruction(&mut pending);
    }

    fn discard_select_source_with_pending(&mut self, arm: SelectArm, pending: &mut VecDeque<u64>) {
        let mut discard_value = false;
        let mut cancel_thread = false;
        match (arm.kind, self.object_mut(arm.source)) {
            (SelectSourceKind::Task, Some(Object::Task { state, kind, .. })) => match state {
                TaskState::Pending => {
                    *state = TaskState::Cancelled;
                    discard_value = true;
                    cancel_thread = *kind == TaskKind::Thread;
                }
                TaskState::Ready => {
                    *state = TaskState::Joined;
                    discard_value = true;
                }
                TaskState::Cancelled | TaskState::Joined => {}
            },
            (SelectSourceKind::OneShot, Some(Object::OneShot { state, .. })) => match state {
                OneShotState::Pending => {
                    *state = OneShotState::Cancelled;
                    discard_value = true;
                }
                OneShotState::Ready => {
                    *state = OneShotState::Consumed;
                    discard_value = true;
                }
                OneShotState::Cancelled | OneShotState::Consumed => {}
            },
            (SelectSourceKind::Timer, Some(Object::Timer { state, .. })) => match state {
                TimerState::Pending => {
                    *state = TimerState::Cancelled;
                    discard_value = true;
                }
                TimerState::Ready => {
                    *state = TimerState::Consumed;
                    discard_value = true;
                }
                TimerState::Cancelled | TimerState::Consumed => {}
            },
            _ => {}
        }
        if discard_value {
            self.release_slot_value(arm.source, arm.kind, pending);
        }
        if cancel_thread && let Some(signal) = self.thread_workers.get(&arm.source) {
            signal.cancel();
            self.clear_runtime_root_into(arm.source, pending);
        }
        self.notify_selects(arm.source);
    }

    fn take_select_source(&mut self, source: u64, kind: SelectSourceKind) -> u64 {
        match (kind, self.object_mut(source)) {
            (SelectSourceKind::Task, Some(Object::Task { state, value, .. })) => {
                if *state != TaskState::Ready {
                    self.last_status = if *state == TaskState::Cancelled {
                        STATUS_CANCELLED
                    } else {
                        STATUS_NOT_READY
                    };
                    return 0;
                }
                *state = TaskState::Joined;
                std::mem::take(value)
            }
            (SelectSourceKind::OneShot, Some(Object::OneShot { state, value })) => {
                if *state != OneShotState::Ready {
                    self.last_status = if *state == OneShotState::Cancelled {
                        STATUS_CANCELLED
                    } else {
                        STATUS_NOT_READY
                    };
                    return 0;
                }
                *state = OneShotState::Consumed;
                std::mem::take(value)
            }
            (SelectSourceKind::Timer, Some(Object::Timer { state, value })) => {
                if *state != TimerState::Ready {
                    self.last_status = if *state == TimerState::Cancelled {
                        STATUS_CANCELLED
                    } else {
                        STATUS_NOT_READY
                    };
                    return 0;
                }
                *state = TimerState::Consumed;
                std::mem::take(value)
            }
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                0
            }
        }
    }

    fn select_begin(&mut self, capacity: u64) -> u64 {
        if !(1..=u64::from(MAX_SELECT_ARMS)).contains(&capacity) {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        self.alloc(
            Object::Select(SelectState {
                capacity: capacity as u32,
                arms: Vec::with_capacity(capacity as usize),
                phase: SelectPhase::Preparing,
                winner: None,
                winner_taken: false,
                wakeups: 0,
            }),
            ObjectKind::Select,
        )
    }

    fn select_register(
        &mut self,
        selection: u64,
        source: u64,
        kind: SelectSourceKind,
        owned: bool,
    ) -> u64 {
        if self.source_kind(source) != Some(kind) {
            return STATUS_INVALID_HANDLE;
        }
        let Some(Object::Select(state)) = self.object(selection) else {
            return STATUS_INVALID_HANDLE;
        };
        if state.phase != SelectPhase::Preparing
            || state.arms.len() >= state.capacity as usize
            || state.arms.iter().any(|arm| arm.source == source)
        {
            return STATUS_INVALID_TRANSITION;
        }
        if self.retain(source) != STATUS_OK {
            return self.last_status;
        }
        let Some(Object::Select(state)) = self.object_mut(selection) else {
            let _ = self.release(source);
            return STATUS_INVALID_HANDLE;
        };
        state.arms.push(SelectArm {
            source,
            kind,
            owned,
        });
        STATUS_OK
    }

    fn select_commit(&mut self, selection: u64, else_allowed: bool) -> u64 {
        let Some(Object::Select(snapshot)) = self.object(selection).cloned() else {
            return STATUS_INVALID_HANDLE;
        };
        if !matches!(
            snapshot.phase,
            SelectPhase::Preparing | SelectPhase::Waiting
        ) || snapshot.arms.len() != snapshot.capacity as usize
        {
            return STATUS_INVALID_TRANSITION;
        }
        // Commit scans the ready set in process-local round-robin order.  The
        // rotation is advanced only after a committed winner or an `else`, so
        // repeated ready sets cannot starve a later arm.
        let start = (self.select_rotation as usize) % snapshot.arms.len();
        let winner = (0..snapshot.arms.len()).find_map(|offset| {
            let index = (start + offset) % snapshot.arms.len();
            self.source_ready(snapshot.arms[index].source, snapshot.arms[index].kind)
                .filter(|ready| *ready)
                .map(|_| index)
        });
        let Some(winner) = winner else {
            if else_allowed {
                for arm in snapshot.arms {
                    if arm.owned {
                        self.discard_select_source(arm);
                    }
                }
                if let Some(Object::Select(state)) = self.object_mut(selection) {
                    state.phase = SelectPhase::Else;
                }
                self.select_rotation = self.select_rotation.wrapping_add(1);
                return STATUS_SELECT_ELSE;
            }
            if let Some(Object::Select(state)) = self.object_mut(selection) {
                state.phase = SelectPhase::Waiting;
            }
            return STATUS_NOT_READY;
        };

        for (index, arm) in snapshot.arms.iter().copied().enumerate() {
            if index != winner && arm.owned {
                self.discard_select_source(arm);
            }
        }
        if let Some(Object::Select(state)) = self.object_mut(selection) {
            state.phase = SelectPhase::Committed;
            state.winner = Some(winner);
        }
        self.select_rotation = self.select_rotation.wrapping_add(1);
        STATUS_OK
    }

    fn select_winner(&mut self, selection: u64) -> u64 {
        match self.object(selection) {
            Some(Object::Select(state))
                if matches!(state.phase, SelectPhase::Committed | SelectPhase::Consumed) =>
            {
                state.winner.map_or_else(
                    || {
                        self.last_status = STATUS_INVALID_TRANSITION;
                        u64::MAX
                    },
                    |winner| winner as u64,
                )
            }
            Some(Object::Select(_)) => {
                self.last_status = STATUS_INVALID_TRANSITION;
                u64::MAX
            }
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn select_take(&mut self, selection: u64) -> u64 {
        let Some(Object::Select(snapshot)) = self.object(selection).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        };
        if snapshot.phase != SelectPhase::Committed || snapshot.winner_taken {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        let Some(winner) = snapshot.winner else {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        };
        let arm = snapshot.arms[winner];
        if let Some(Object::Select(state)) = self.object_mut(selection) {
            state.winner_taken = true;
            state.phase = SelectPhase::Consumed;
        }
        self.take_select_source(arm.source, arm.kind)
    }

    fn select_thread_source(&self, selection: u64) -> Option<u64> {
        let Some(Object::Select(state)) = self.object(selection) else {
            return None;
        };
        if state.phase != SelectPhase::Committed || state.winner_taken {
            return None;
        }
        let winner = state.winner?;
        let arm = state.arms.get(winner)?;
        (arm.kind == SelectSourceKind::Task)
            .then_some(arm.source)
            .filter(|source| {
                matches!(
                    self.object(*source),
                    Some(Object::Task {
                        kind: TaskKind::Thread,
                        ..
                    })
                )
            })
    }

    fn select_rollback(&mut self, selection: u64) -> u64 {
        let Some(Object::Select(snapshot)) = self.object(selection).cloned() else {
            return STATUS_INVALID_HANDLE;
        };
        if !matches!(
            snapshot.phase,
            SelectPhase::Preparing | SelectPhase::Waiting
        ) {
            return STATUS_INVALID_TRANSITION;
        }
        for arm in snapshot.arms {
            if arm.owned {
                self.discard_select_source(arm);
            }
        }
        if let Some(Object::Select(state)) = self.object_mut(selection) {
            state.phase = SelectPhase::RolledBack;
        }
        STATUS_OK
    }

    fn select_wakeups(&mut self, selection: u64) -> u64 {
        match self.object(selection) {
            Some(Object::Select(state)) => state.wakeups,
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn oneshot_new(&mut self) -> u64 {
        self.alloc(
            Object::OneShot {
                state: OneShotState::Pending,
                value: 0,
            },
            ObjectKind::OneShot,
        )
    }

    fn oneshot_complete(&mut self, oneshot: u64, value: u64) -> u64 {
        let Some(Object::OneShot { state, .. }) = self.object(oneshot) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state != OneShotState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        let mut pending = VecDeque::new();
        if self.live_handle(value) && self.retain(value) != STATUS_OK {
            return self.last_status;
        }
        if let Some(Object::OneShot { state, value: slot }) = self.object_mut(oneshot) {
            let previous = std::mem::replace(slot, value);
            *state = OneShotState::Ready;
            if self.live_handle(previous) {
                self.release_strong_edge(previous, &mut pending);
            }
        }
        self.notify_selects(oneshot);
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn oneshot_cancel(&mut self, oneshot: u64) -> u64 {
        let Some(Object::OneShot { state, .. }) = self.object_mut(oneshot) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state != OneShotState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        *state = OneShotState::Cancelled;
        self.notify_selects(oneshot);
        STATUS_OK
    }

    fn time_new(&mut self, value: u64) -> u64 {
        self.alloc(
            Object::Timer {
                state: TimerState::Pending,
                value,
            },
            ObjectKind::Timer,
        )
    }

    fn time_fire(&mut self, timer: u64) -> u64 {
        let Some(Object::Timer { state, .. }) = self.object_mut(timer) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state != TimerState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        *state = TimerState::Ready;
        self.notify_selects(timer);
        STATUS_OK
    }

    /// Executes one deterministic native diagnostic scenario.  The scenario
    /// deliberately goes through the same task, frame, root and ARC paths as
    /// generated code; only the envelope fields are private diagnostics.
    fn diagnostic_probe(&mut self, profile: u64, mode: u64) -> u64 {
        self.diagnostic = Some(DiagnosticCapture::new(profile, mode));
        let status = match profile {
            DIAG_PROFILE_RACE => self.diagnostic_race(mode),
            DIAG_PROFILE_LEAK => self.diagnostic_leak(mode),
            DIAG_PROFILE_CRASH => self.diagnostic_crash(mode),
            _ => STATUS_DIAG_UNSUPPORTED,
        };
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.status = status;
        }
        status
    }

    fn diagnostic_race(&mut self, mode: u64) -> u64 {
        if mode > 1 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        let first = self.task_spawn(None, 0, true);
        let second = self.task_spawn(None, 0, true);
        if first == 0 || second == 0 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        let _ = self.task_wake(first);
        let _ = self.task_wake(second);
        let _ = self.task_take(first);
        let _ = self.task_take(second);
        let _ = self.release(first);
        let _ = self.release(second);
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.source_maps = 2;
            capture.unwind_frames = capture.unwind_frames.max(2);
        }
        if mode == 1 {
            STATUS_DIAG_FINDING
        } else {
            STATUS_DIAG_CLEAN
        }
    }

    fn diagnostic_leak(&mut self, mode: u64) -> u64 {
        if mode > 2 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        if mode == 2 {
            let first = self.alloc(
                Object::Result {
                    tag: RESULT_OK,
                    payload: None,
                },
                ObjectKind::Result,
            );
            let second = self.alloc(
                Object::Result {
                    tag: RESULT_OK,
                    payload: None,
                },
                ObjectKind::Result,
            );
            if first == 0 || second == 0 {
                return STATUS_DIAG_UNSUPPORTED;
            }
            if let Some(Object::Result { payload, .. }) = self.object_mut(first) {
                *payload = Some(second);
            }
            let _ = self.retain(second);
            if let Some(Object::Result { payload, .. }) = self.object_mut(second) {
                *payload = Some(first);
            }
            let _ = self.retain(first);
            let _ = self.release(first);
            let _ = self.release(second);
            let _ = self.collect_cycles();
            if let Some(capture) = self.diagnostic.as_mut() {
                capture.source_maps = 1;
                capture.unwind_frames = capture.unwind_frames.max(1);
                capture.resources_acquired = 1;
                capture.resources_released = 1;
            }
            return STATUS_DIAG_CLEAN;
        }

        let frame = self.create_frame();
        let value = self.alloc(
            Object::Result {
                tag: RESULT_OK,
                payload: Some(7),
            },
            ObjectKind::Result,
        );
        if frame == 0 || value == 0 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        let _ = self.publish_root(frame, value);
        let _ = self.register_defer(frame, 7);
        let _ = self.cleanup_frame(frame, false);
        let _ = self.release(value);
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.source_maps = 1;
            capture.retainers = u64::from(mode == 1) * 2;
            capture.ffi_allocations = u64::from(mode == 1);
            capture.resources_acquired = 1;
            capture.resources_released = u64::from(mode == 0);
        }
        if mode == 1 {
            STATUS_DIAG_FINDING
        } else {
            STATUS_DIAG_CLEAN
        }
    }

    fn diagnostic_crash(&mut self, mode: u64) -> u64 {
        if mode > 2 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        let frame = self.create_frame();
        let value = self.alloc(
            Object::Result {
                tag: RESULT_ERR,
                payload: Some(13),
            },
            ObjectKind::Result,
        );
        if frame == 0 || value == 0 {
            return STATUS_DIAG_UNSUPPORTED;
        }
        let _ = self.publish_root(frame, value);
        let _ = self.register_defer(frame, 13);
        let _ = self.cleanup_frame(frame, true);
        let _ = self.release(value);
        if let Some(capture) = self.diagnostic.as_mut() {
            capture.source_maps = 3;
            capture.unwind_frames = capture.unwind_frames.max(2);
            capture.task_ids.insert(HANDLE_BIT | 1);
            capture.task_ids.insert(HANDLE_BIT | 2);
            capture.thread_ids.insert(HANDLE_BIT | 3);
            capture.happens_before_edges = capture.happens_before_edges.max(1);
            capture.corruption_rejected = mode >= 1;
            capture.limit_enforced = mode == 2;
            capture.ffi_allocations = 1;
            capture.resources_acquired = 2;
            capture.resources_released = 2;
        }
        STATUS_DIAG_CAPTURED
    }

    fn diagnostic_field(&mut self, field: u64) -> u64 {
        self.diagnostic.as_ref().map_or_else(
            || {
                self.last_status = STATUS_INVALID_TRANSITION;
                u64::MAX
            },
            |capture| capture.field(field),
        )
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::new()))
}

fn with_state<T>(function: impl FnOnce(&mut State) -> T) -> T {
    let mut state = state()
        .lock()
        .expect("native runtime state is not poisoned");
    function(&mut state)
}

/// Resets process-local native state. This is test-only in production: native
/// retries start a fresh worker process instead of sharing this table.
pub extern "C" fn tondo_rt_reset() {
    with_state(|state| *state = State::new());
}

/// Clears the private native diagnostic capture without changing runtime
/// ownership state.  The native test runner calls this at a process boundary;
/// production code does not observe the capture.
pub extern "C" fn tondo_rt_diag_reset() {
    with_state(|state| state.diagnostic = None);
}

/// Runs one bounded diagnostic scenario.  Profile values are race=0, leaks=1
/// and crash=2; mode is profile-specific and intentionally not a public API.
pub extern "C" fn tondo_rt_diag_probe(profile: u64, mode: u64) -> u64 {
    with_state(|state| state.diagnostic_probe(profile, mode))
}

/// Reads a logical diagnostic field from the last capture.  Invalid fields or
/// a missing capture return `u64::MAX` and set the private status channel.
pub extern "C" fn tondo_rt_diag_field(field: u64) -> u64 {
    with_state(|state| state.diagnostic_field(field))
}

pub extern "C" fn tondo_rt_result_new(tag: u64, payload: u64, has_payload: u64) -> u64 {
    with_state(|state| {
        if !(RESULT_NONE..=RESULT_ERR).contains(&tag) {
            state.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        state.alloc(
            Object::Result {
                tag,
                payload: (has_payload != 0).then_some(payload),
            },
            ObjectKind::Result,
        )
    })
}

pub extern "C" fn tondo_rt_result_tag(value: u64) -> u64 {
    with_state(|state| match state.object(value) {
        Some(Object::Result { tag, .. }) => *tag,
        // Raw tags remain accepted for tag-only scalar MIR. This preserves
        // backwards compatibility while managed values use opaque handles.
        None if matches!(value, RESULT_NONE | RESULT_SOME | RESULT_OK | RESULT_ERR) => value,
        _ => {
            state.last_status = STATUS_INVALID_HANDLE;
            u64::MAX
        }
    })
}

pub extern "C" fn tondo_rt_result_payload(value: u64) -> u64 {
    with_state(|state| match state.object(value) {
        Some(Object::Result {
            payload: Some(payload),
            ..
        }) => *payload,
        _ => {
            state.last_status = STATUS_INVALID_HANDLE;
            0
        }
    })
}

pub extern "C" fn tondo_rt_retain(value: u64) -> u64 {
    with_state(|state| state.retain(value))
}

pub extern "C" fn tondo_rt_release(value: u64) -> u64 {
    with_state(|state| state.release(value))
}

/// Marks a value as crossing a `Send`/`Share` boundary.  Subsequent retain and
/// release operations use the shared atomic counter representation.
pub extern "C" fn tondo_rt_mark_shared(value: u64) -> u64 {
    with_state(|state| state.mark_shared(value))
}

/// Returns `0` for local ARC counts and `1` for shared atomic counts.  This is
/// an internal diagnostic hook, not a user-visible layout or FFI guarantee.
pub extern "C" fn tondo_rt_arc_kind(value: u64) -> u64 {
    with_state(|state| state.arc_kind(value))
}

/// Returns the current strong count for an alive opaque value.
pub extern "C" fn tondo_rt_arc_strong_count(value: u64) -> u64 {
    with_state(|state| state.strong_count(value))
}

/// Returns the number of weak handles retaining target tombstone metadata.
pub extern "C" fn tondo_rt_arc_weak_count(value: u64) -> u64 {
    with_state(|state| state.weak_count(value))
}

/// Creates a runtime-managed weak handle without retaining the target.
pub extern "C" fn tondo_rt_weak_new(value: u64) -> u64 {
    with_state(|state| state.weak_new(value))
}

/// Attempts to turn a weak handle back into a strong handle.  A dead target
/// returns zero and sets `STATUS_WEAK_DEAD` in the private status channel.
pub extern "C" fn tondo_rt_weak_upgrade(value: u64) -> u64 {
    with_state(|state| state.weak_upgrade(value))
}

/// Runs the trial-deletion collector at a quiescent point and returns the
/// number of unreachable object components reclaimed.
pub extern "C" fn tondo_rt_collect_cycles() -> u64 {
    with_state(State::quiesce)
}

/// Explicit quiescence boundary used by async frames and diagnostic runs.
pub extern "C" fn tondo_rt_quiesce() -> u64 {
    with_state(State::quiesce)
}

/// Returns the count of alive opaque objects.  Tombstones held only by weak
/// handles are intentionally excluded.
pub extern "C" fn tondo_rt_live_objects() -> u64 {
    with_state(|state| state.live_object_count())
}

pub extern "C" fn tondo_rt_cow_clone(value: u64) -> u64 {
    with_state(|state| state.clone_value(value))
}

pub extern "C" fn tondo_rt_last_status() -> u64 {
    with_state(|state| state.last_status)
}

pub extern "C" fn tondo_rt_frame_enter() -> u64 {
    with_state(State::create_frame)
}

pub extern "C" fn tondo_rt_frame_publish_root(frame: u64, value: u64) -> u64 {
    with_state(|state| state.publish_root(frame, value))
}

pub extern "C" fn tondo_rt_frame_unpublish_root(frame: u64, value: u64) -> u64 {
    with_state(|state| state.unpublish_root(frame, value))
}

pub extern "C" fn tondo_rt_frame_register_defer(frame: u64, id: u64) -> u64 {
    with_state(|state| state.register_defer(frame, id))
}

pub extern "C" fn tondo_rt_frame_disarm_defer(frame: u64, id: u64) -> u64 {
    with_state(|state| state.disarm_defer(frame, id))
}

pub extern "C" fn tondo_rt_frame_cleanup(frame: u64, aborting: u64) -> u64 {
    with_state(|state| state.cleanup_frame(frame, aborting != 0))
}

pub extern "C" fn tondo_rt_frame_leave(frame: u64, aborting: u64) -> u64 {
    with_state(|state| {
        let status = state.cleanup_frame(frame, aborting != 0);
        state.frames.remove(&frame);
        status
    })
}

pub extern "C" fn tondo_rt_host_call(kind: u64, argument: u64) -> u64 {
    with_state(|state| match kind {
        // Host calls use the same result record and never expose host memory.
        0 => state.alloc(
            Object::Result {
                tag: RESULT_OK,
                payload: Some(argument),
            },
            ObjectKind::Result,
        ),
        1 => state.alloc(
            Object::Result {
                tag: RESULT_ERR,
                payload: Some(argument),
            },
            ObjectKind::Result,
        ),
        _ => {
            state.last_status = STATUS_INVALID_TRANSITION;
            0
        }
    })
}

pub extern "C" fn tondo_rt_scope_enter() -> u64 {
    with_state(|state| {
        state.alloc(
            Object::Scope {
                tasks: Vec::new(),
                cancelled: false,
            },
            ObjectKind::Scope,
        )
    })
}

pub extern "C" fn tondo_rt_scope_spawn(scope: u64, value: u64, pending: u64) -> u64 {
    with_state(|state| state.task_spawn(Some(scope), value, pending != 0))
}

pub extern "C" fn tondo_rt_task_spawn(value: u64, pending: u64) -> u64 {
    with_state(|state| state.task_spawn(None, value, pending != 0))
}

/// Creates an OS-worker lane handle.  The bootstrap runtime uses the same
/// state machine as a task; the native thread scheduler supplies the worker
/// later without changing Join/select ownership semantics.
pub extern "C" fn tondo_rt_thread_spawn(value: u64, pending: u64) -> u64 {
    with_state(|state| state.thread_spawn(value, pending != 0))
}

/// Returns the private worker lifecycle state for a native `Thread` handle.
/// The numeric values are intentionally ABI-local: starting=0, running=1,
/// completed=2 and cancelled=3.  Invalid or non-thread handles return `u64::MAX`.
pub extern "C" fn tondo_rt_thread_worker_status(task: u64) -> u64 {
    with_state(|state| state.thread_worker_status(task))
}

/// Returns the number of worker entries executed for a `Thread` handle.
pub extern "C" fn tondo_rt_thread_worker_runs(task: u64) -> u64 {
    with_state(|state| state.thread_worker_runs(task))
}

/// Reports whether the worker ran on a thread distinct from its spawner.
pub extern "C" fn tondo_rt_thread_worker_distinct(task: u64) -> u64 {
    with_state(|state| state.thread_worker_distinct(task))
}

/// Waits for the physical worker without consuming the logical `Join` value.
/// `Join`/`await` use the same barrier internally; this entry is only for
/// diagnostics and native-runtime verification.
pub extern "C" fn tondo_rt_thread_worker_wait(task: u64) -> u64 {
    let Some(snapshot) = wait_for_thread_worker(task) else {
        return STATUS_INVALID_HANDLE;
    };
    with_state(|state| state.clear_runtime_root_if_terminal(task));
    if snapshot.state == WorkerState::Cancelled {
        STATUS_CANCELLED
    } else {
        STATUS_OK
    }
}

fn wait_for_thread_worker(task: u64) -> Option<WorkerSnapshot> {
    let signal = with_state(|state| state.thread_worker_signal(task));
    signal.map(|signal| signal.wait())
}

pub extern "C" fn tondo_rt_task_poll(task: u64) -> u64 {
    with_state(|state| state.task_poll(task))
}

pub extern "C" fn tondo_rt_task_wake(task: u64) -> u64 {
    with_state(|state| state.task_wake(task))
}

pub extern "C" fn tondo_rt_task_cancel(task: u64) -> u64 {
    with_state(|state| state.task_cancel(task))
}

pub extern "C" fn tondo_rt_task_take(task: u64) -> u64 {
    if let Some(snapshot) = wait_for_thread_worker(task) {
        with_state(|state| state.clear_runtime_root_if_terminal(task));
        if snapshot.state == WorkerState::Cancelled {
            with_state(|state| state.last_status = STATUS_CANCELLED);
            return 0;
        }
    }
    with_state(|state| state.task_take(task))
}

/// Publishes the result of a deferred callable body and makes its handle
/// ready.  This is a private compiler/runtime ABI entry; user code still sees
/// only `Join`/`await` and never this transition or a native task layout.
pub extern "C" fn tondo_rt_task_complete(task: u64, value: u64) -> u64 {
    with_state(|state| state.task_complete(task, value))
}

pub extern "C" fn tondo_rt_scope_cancel(scope: u64) -> u64 {
    with_state(|state| {
        let Some(Object::Scope { tasks, cancelled }) = state.object(scope).cloned() else {
            return STATUS_INVALID_HANDLE;
        };
        if cancelled {
            return STATUS_INVALID_TRANSITION;
        }
        for task in tasks {
            let _ = state.task_cancel(task);
        }
        if let Some(Object::Scope { cancelled, .. }) = state.object_mut(scope) {
            *cancelled = true;
        }
        STATUS_OK
    })
}

pub extern "C" fn tondo_rt_scope_join(scope: u64, task: u64) -> u64 {
    if wait_for_thread_worker(task).is_some() {
        with_state(|state| state.clear_runtime_root_if_terminal(task));
    }
    with_state(|state| {
        let Some(Object::Scope { tasks, cancelled }) = state.object(scope).cloned() else {
            return STATUS_INVALID_HANDLE;
        };
        if cancelled || !tasks.contains(&task) {
            return STATUS_INVALID_TRANSITION;
        }
        if !matches!(
            state.object(task),
            Some(Object::Task {
                state: TaskState::Ready,
                ..
            })
        ) {
            return STATUS_INVALID_TRANSITION;
        }
        let mut pending = VecDeque::new();
        // Scope join has no value return channel.  Consume the task payload
        // and release the ownership transferred by `task_take` instead of
        // silently dropping a managed handle on the floor.
        let value = state.task_take(task);
        if state.live_handle(value) {
            state.release_strong_edge(value, &mut pending);
        }
        let status = state.scope_remove_task(scope, task, &mut pending);
        state.drain_destruction(&mut pending);
        status
    })
}

pub extern "C" fn tondo_rt_await(task: u64) -> u64 {
    if let Some(snapshot) = wait_for_thread_worker(task) {
        with_state(|state| state.clear_runtime_root_if_terminal(task));
        if snapshot.state == WorkerState::Cancelled {
            with_state(|state| state.last_status = STATUS_CANCELLED);
            return 0;
        }
    }
    with_state(|state| {
        if state.task_poll(task) != 1 {
            state.last_status = STATUS_NOT_READY;
            return 0;
        }
        state.task_take(task)
    })
}

/// Opens one native selection region.  Registration is prepare-only; commit
/// is the single linearization point and rollback is the failure edge.
pub extern "C" fn tondo_rt_select_begin(capacity: u64) -> u64 {
    with_state(|state| state.select_begin(capacity))
}

pub extern "C" fn tondo_rt_select_register_task(selection: u64, task: u64, owned: u64) -> u64 {
    with_state(|state| state.select_register(selection, task, SelectSourceKind::Task, owned != 0))
}

pub extern "C" fn tondo_rt_select_register_join(selection: u64, task: u64) -> u64 {
    with_state(|state| state.select_register(selection, task, SelectSourceKind::Task, false))
}

pub extern "C" fn tondo_rt_select_register_oneshot(
    selection: u64,
    oneshot: u64,
    owned: u64,
) -> u64 {
    with_state(|state| {
        state.select_register(selection, oneshot, SelectSourceKind::OneShot, owned != 0)
    })
}

pub extern "C" fn tondo_rt_select_register_time(selection: u64, timer: u64, owned: u64) -> u64 {
    with_state(|state| state.select_register(selection, timer, SelectSourceKind::Timer, owned != 0))
}

pub extern "C" fn tondo_rt_select_commit(selection: u64, else_allowed: u64) -> u64 {
    with_state(|state| state.select_commit(selection, else_allowed != 0))
}

pub extern "C" fn tondo_rt_select_winner(selection: u64) -> u64 {
    with_state(|state| state.select_winner(selection))
}

pub extern "C" fn tondo_rt_select_take(selection: u64) -> u64 {
    let thread = with_state(|state| state.select_thread_source(selection));
    if let Some(thread) = thread
        && let Some(snapshot) = wait_for_thread_worker(thread)
    {
        with_state(|state| state.clear_runtime_root_if_terminal(thread));
        if snapshot.state == WorkerState::Cancelled {
            with_state(|state| state.last_status = STATUS_CANCELLED);
            return 0;
        }
    }
    with_state(|state| state.select_take(selection))
}

pub extern "C" fn tondo_rt_select_rollback(selection: u64) -> u64 {
    with_state(|state| state.select_rollback(selection))
}

pub extern "C" fn tondo_rt_select_wakeups(selection: u64) -> u64 {
    with_state(|state| state.select_wakeups(selection))
}

pub extern "C" fn tondo_rt_oneshot_new() -> u64 {
    with_state(State::oneshot_new)
}

pub extern "C" fn tondo_rt_oneshot_complete(oneshot: u64, value: u64) -> u64 {
    with_state(|state| state.oneshot_complete(oneshot, value))
}

pub extern "C" fn tondo_rt_oneshot_cancel(oneshot: u64) -> u64 {
    with_state(|state| state.oneshot_cancel(oneshot))
}

pub extern "C" fn tondo_rt_time_new(value: u64) -> u64 {
    with_state(|state| state.time_new(value))
}

pub extern "C" fn tondo_rt_time_fire(timer: u64) -> u64 {
    with_state(|state| state.time_fire(timer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("native runtime test lock is not poisoned")
    }

    #[test]
    fn result_records_are_opaque_and_round_trip_payloads() {
        let _guard = test_guard();
        tondo_rt_reset();
        let result = tondo_rt_result_new(RESULT_OK, 42, 1);
        assert!(result & HANDLE_BIT != 0);
        assert_eq!(tondo_rt_result_tag(result), RESULT_OK);
        assert_eq!(tondo_rt_result_payload(result), 42);
        assert_eq!(tondo_rt_release(result), STATUS_OK);
        assert_eq!(tondo_rt_result_tag(result), u64::MAX);
        let none = tondo_rt_result_new(RESULT_NONE, 0, 0);
        assert_eq!(tondo_rt_result_tag(none), RESULT_NONE);
        assert_eq!(tondo_rt_release(none), STATUS_OK);
    }

    #[test]
    fn roots_and_cleanup_are_exactly_once() {
        let _guard = test_guard();
        tondo_rt_reset();
        let frame = tondo_rt_frame_enter();
        let value = tondo_rt_result_new(RESULT_SOME, 9, 1);
        assert_eq!(tondo_rt_frame_publish_root(frame, value), STATUS_OK);
        assert_eq!(tondo_rt_frame_register_defer(frame, 7), STATUS_OK);
        assert_eq!(tondo_rt_frame_cleanup(frame, 0), STATUS_OK);
        assert_eq!(tondo_rt_frame_cleanup(frame, 0), STATUS_DOUBLE_CLEANUP);
        assert_eq!(tondo_rt_frame_leave(frame, 0), STATUS_DOUBLE_CLEANUP);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
    }

    #[test]
    fn cow_clones_shared_values_but_reuses_unique_values() {
        let _guard = test_guard();
        tondo_rt_reset();
        let value = tondo_rt_result_new(RESULT_OK, 1, 1);
        assert_eq!(tondo_rt_cow_clone(value), value);
        assert_eq!(tondo_rt_retain(value), STATUS_OK);
        let clone = tondo_rt_cow_clone(value);
        assert_ne!(clone, value);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        assert_eq!(tondo_rt_release(clone), STATUS_OK);
    }

    #[test]
    fn tasks_publish_wake_join_and_cancel_without_leaking_scope_state() {
        let _guard = test_guard();
        tondo_rt_reset();
        let scope = tondo_rt_scope_enter();
        let task = tondo_rt_scope_spawn(scope, 77, 1);
        assert_eq!(tondo_rt_task_poll(task), 0);
        assert_eq!(tondo_rt_task_wake(task), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(task), 1);
        assert_eq!(tondo_rt_scope_join(scope, task), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(task), 3);

        let cancelled = tondo_rt_scope_spawn(scope, 88, 1);
        assert_eq!(tondo_rt_scope_cancel(scope), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(cancelled), 2);
        assert_eq!(tondo_rt_scope_cancel(scope), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_scope_spawn(scope, 99, 1), 0);
    }

    #[test]
    fn deferred_task_body_commits_once_before_the_common_await_transition() {
        let _guard = test_guard();
        tondo_rt_reset();
        let task = tondo_rt_task_spawn(0, 1);
        assert_eq!(tondo_rt_task_poll(task), 0);
        assert_eq!(tondo_rt_task_complete(task, 42), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(task), 1);
        assert_eq!(tondo_rt_await(task), 42);
        assert_eq!(tondo_rt_task_poll(task), 3);
        assert_eq!(tondo_rt_task_complete(task, 99), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_await(task), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_NOT_READY);

        let ready = tondo_rt_task_spawn(7, 0);
        assert_eq!(tondo_rt_task_complete(ready, 8), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_task_cancel(0), STATUS_INVALID_HANDLE);
    }

    #[test]
    fn native_thread_uses_a_distinct_worker_and_join_waits_for_completion() {
        let _guard = test_guard();
        tondo_rt_reset();
        let task = tondo_rt_thread_spawn(123, 0);
        assert_ne!(task, 0);
        assert_eq!(tondo_rt_thread_worker_wait(task), STATUS_OK);
        assert_eq!(tondo_rt_thread_worker_status(task), WORKER_COMPLETED);
        assert_eq!(tondo_rt_thread_worker_runs(task), 1);
        assert_eq!(tondo_rt_thread_worker_distinct(task), 1);
        assert_eq!(tondo_rt_task_take(task), 123);
        assert_eq!(tondo_rt_task_poll(task), 3);
        assert_eq!(tondo_rt_release(task), STATUS_OK);

        let pending = tondo_rt_thread_spawn(456, 1);
        assert_eq!(tondo_rt_thread_worker_wait(pending), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(pending), 0);
        assert_eq!(tondo_rt_task_cancel(pending), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(pending), 2);
        assert_eq!(tondo_rt_task_take(pending), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_CANCELLED);
        assert_eq!(tondo_rt_thread_worker_status(0), u64::MAX);
        assert_eq!(tondo_rt_release(pending), STATUS_OK);

        let awaited = tondo_rt_thread_spawn(789, 0);
        assert_eq!(tondo_rt_await(awaited), 789);
        assert_eq!(tondo_rt_task_poll(awaited), 3);
        assert_eq!(tondo_rt_release(awaited), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let cancelled_before_start = WorkerSignal::new(std::thread::current().id());
        cancelled_before_start.cancel();
        cancelled_before_start.run();
        let snapshot = cancelled_before_start.snapshot();
        assert_eq!(snapshot.state, WorkerState::Cancelled);
        assert_eq!(snapshot.runs, 0);
    }

    #[test]
    fn await_and_cancellation_reject_invalid_task_transitions() {
        let _guard = test_guard();
        tondo_rt_reset();
        let task = tondo_rt_task_spawn(41, 1);
        assert_eq!(tondo_rt_await(task), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_NOT_READY);
        assert_eq!(tondo_rt_task_wake(task), STATUS_OK);
        assert_eq!(tondo_rt_await(task), 41);
        assert_eq!(tondo_rt_await(task), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_NOT_READY);

        let scope = tondo_rt_scope_enter();
        let pending = tondo_rt_scope_spawn(scope, 52, 1);
        assert_eq!(
            tondo_rt_scope_join(scope, pending),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_scope_cancel(scope), STATUS_OK);
        assert_eq!(tondo_rt_task_wake(pending), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_task_cancel(pending), STATUS_INVALID_TRANSITION);
        assert_eq!(
            tondo_rt_scope_join(scope, pending),
            STATUS_INVALID_TRANSITION
        );
    }

    #[test]
    fn native_select_commits_a_ready_join_with_round_robin_fairness() {
        let _guard = test_guard();
        tondo_rt_reset();
        let selection = tondo_rt_select_begin(2);
        let first = tondo_rt_task_spawn(11, 0);
        let second = tondo_rt_thread_spawn(22, 0);
        assert_eq!(tondo_rt_select_register_join(selection, first), STATUS_OK);
        assert_eq!(
            tondo_rt_select_register_task(selection, second, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_winner(selection), 0);
        assert_eq!(tondo_rt_select_take(selection), 11);
        assert_eq!(tondo_rt_task_poll(first), 3);

        let next = tondo_rt_select_begin(2);
        let left = tondo_rt_task_spawn(31, 0);
        let right = tondo_rt_task_spawn(32, 0);
        assert_eq!(tondo_rt_select_register_join(next, left), STATUS_OK);
        assert_eq!(tondo_rt_select_register_join(next, right), STATUS_OK);
        assert_eq!(tondo_rt_select_commit(next, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_winner(next), 1);
        assert_eq!(tondo_rt_select_take(next), 32);

        let thread_selection = tondo_rt_select_begin(1);
        let thread = tondo_rt_thread_spawn(43, 0);
        assert_eq!(
            tondo_rt_select_register_join(thread_selection, thread),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(thread_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(thread_selection), 43);
        assert_eq!(tondo_rt_thread_worker_status(thread), WORKER_COMPLETED);
    }

    #[test]
    fn native_select_waits_for_wakeup_and_adapts_one_shot_and_time() {
        let _guard = test_guard();
        tondo_rt_reset();
        let selection = tondo_rt_select_begin(1);
        let task = tondo_rt_task_spawn(41, 1);
        assert_eq!(tondo_rt_select_register_join(selection, task), STATUS_OK);
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_NOT_READY);
        assert_eq!(tondo_rt_select_wakeups(selection), 0);
        assert_eq!(tondo_rt_task_wake(task), STATUS_OK);
        assert_eq!(tondo_rt_select_wakeups(selection), 1);
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(selection), 41);

        let oneshot = tondo_rt_oneshot_new();
        let one_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_oneshot(one_selection, oneshot, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(one_selection, 0), STATUS_NOT_READY);
        assert_eq!(tondo_rt_oneshot_complete(oneshot, 52), STATUS_OK);
        assert_eq!(tondo_rt_select_wakeups(one_selection), 1);
        assert_eq!(tondo_rt_select_commit(one_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(one_selection), 52);

        let timer = tondo_rt_time_new(63);
        let time_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_time(time_selection, timer, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(time_selection, 0), STATUS_NOT_READY);
        assert_eq!(tondo_rt_time_fire(timer), STATUS_OK);
        assert_eq!(tondo_rt_select_commit(time_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(time_selection), 63);

        let cancelled_task = tondo_rt_task_spawn(64, 1);
        let cancelled_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_join(cancelled_selection, cancelled_task),
            STATUS_OK
        );
        assert_eq!(
            tondo_rt_select_commit(cancelled_selection, 0),
            STATUS_NOT_READY
        );
        assert_eq!(tondo_rt_task_cancel(cancelled_task), STATUS_OK);
        assert_eq!(tondo_rt_select_wakeups(cancelled_selection), 1);
        assert_eq!(tondo_rt_select_commit(cancelled_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(cancelled_selection), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_CANCELLED);
    }

    #[test]
    fn native_select_rollback_and_else_discard_only_owned_sources() {
        let _guard = test_guard();
        tondo_rt_reset();
        let selection = tondo_rt_select_begin(2);
        let owned = tondo_rt_task_spawn(71, 1);
        let borrowed = tondo_rt_task_spawn(72, 1);
        assert_eq!(
            tondo_rt_select_register_task(selection, owned, 1),
            STATUS_OK
        );
        assert_eq!(
            tondo_rt_select_register_join(selection, borrowed),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_NOT_READY);
        assert_eq!(tondo_rt_select_rollback(selection), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(owned), 2);
        assert_eq!(tondo_rt_task_poll(borrowed), 0);
        assert_eq!(
            tondo_rt_select_rollback(selection),
            STATUS_INVALID_TRANSITION
        );

        let else_selection = tondo_rt_select_begin(1);
        let discarded = tondo_rt_task_spawn(81, 1);
        assert_eq!(
            tondo_rt_select_register_task(else_selection, discarded, 1),
            STATUS_OK
        );
        assert_eq!(
            tondo_rt_select_commit(else_selection, 1),
            STATUS_SELECT_ELSE
        );
        assert_eq!(tondo_rt_task_poll(discarded), 2);
    }

    #[test]
    fn native_select_rejects_bad_capacity_sources_and_phase_transitions() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_select_begin(0), 0);
        assert_eq!(tondo_rt_select_begin(u64::from(MAX_SELECT_ARMS) + 1), 0);

        let selection = tondo_rt_select_begin(1);
        let task = tondo_rt_task_spawn(91, 1);
        let other = tondo_rt_task_spawn(92, 1);
        assert_eq!(
            tondo_rt_select_register_oneshot(selection, task, 0),
            STATUS_INVALID_HANDLE
        );
        assert_eq!(tondo_rt_select_register_join(selection, other), STATUS_OK);
        assert_eq!(
            tondo_rt_select_register_join(selection, other),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_select_winner(selection), u64::MAX);
        assert_eq!(tondo_rt_select_take(selection), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_NOT_READY);
        assert_eq!(tondo_rt_select_rollback(selection), STATUS_OK);
        assert_eq!(tondo_rt_select_winner(selection), u64::MAX);
        assert_eq!(tondo_rt_select_wakeups(selection), 0);
        assert_eq!(tondo_rt_select_wakeups(0), u64::MAX);
        assert_eq!(tondo_rt_select_commit(0, 0), STATUS_INVALID_HANDLE);

        let ready = tondo_rt_task_spawn(101, 0);
        let invalid = tondo_rt_select_begin(1);
        assert_eq!(tondo_rt_select_register_join(invalid, ready), STATUS_OK);
        assert_eq!(tondo_rt_select_commit(invalid, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(invalid), 101);
        assert_eq!(tondo_rt_select_take(invalid), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
    }

    #[test]
    fn native_select_one_shot_and_time_transitions_are_exactly_once() {
        let _guard = test_guard();
        tondo_rt_reset();
        let oneshot = tondo_rt_oneshot_new();
        assert_eq!(tondo_rt_oneshot_complete(oneshot, 1), STATUS_OK);
        assert_eq!(
            tondo_rt_oneshot_complete(oneshot, 2),
            STATUS_INVALID_TRANSITION
        );
        let selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_oneshot(selection, oneshot, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(selection), 1);
        assert_eq!(tondo_rt_select_take(selection), 0);

        let cancelled = tondo_rt_oneshot_new();
        assert_eq!(tondo_rt_oneshot_cancel(cancelled), STATUS_OK);
        assert_eq!(
            tondo_rt_oneshot_cancel(cancelled),
            STATUS_INVALID_TRANSITION
        );
        let cancel_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_oneshot(cancel_selection, cancelled, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(cancel_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(cancel_selection), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_CANCELLED);

        let timer = tondo_rt_time_new(3);
        assert_eq!(tondo_rt_time_fire(timer), STATUS_OK);
        assert_eq!(tondo_rt_time_fire(timer), STATUS_INVALID_TRANSITION);
        let time_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_time(time_selection, timer, 0),
            STATUS_OK
        );
        assert_eq!(tondo_rt_select_commit(time_selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(time_selection), 3);

        let fresh_oneshot = tondo_rt_oneshot_new();
        assert!(fresh_oneshot & HANDLE_BIT != 0);
        assert_eq!(tondo_rt_oneshot_complete(0, 0), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_time_fire(0), STATUS_INVALID_HANDLE);
    }

    #[test]
    fn native_select_state_machine_covers_all_source_states() {
        let _guard = test_guard();
        let mut state = State::new();

        let result = state.alloc(
            Object::Result {
                tag: RESULT_OK,
                payload: Some(1),
            },
            ObjectKind::Result,
        );
        assert_eq!(state.source_kind(result), None);
        assert_eq!(state.source_ready(result, SelectSourceKind::Task), None);

        let scope = state.alloc(
            Object::Scope {
                tasks: Vec::new(),
                cancelled: false,
            },
            ObjectKind::Scope,
        );
        assert_eq!(state.task_spawn(Some(0), 1, true), 0);
        assert_eq!(state.task_spawn(Some(result), 1, true), 0);
        let pending = state.task_spawn(Some(scope), 2, true);
        let ready = state.task_spawn(Some(scope), 3, false);
        let standalone = state.task_spawn(None, 4, true);
        let thread = state.thread_spawn(5, false);
        assert_eq!(state.source_kind(pending), Some(SelectSourceKind::Task));
        assert_eq!(state.source_kind(thread), Some(SelectSourceKind::Task));
        assert_eq!(
            state.source_ready(pending, SelectSourceKind::Task),
            Some(false)
        );
        assert_eq!(state.source_ready(ready, SelectSourceKind::OneShot), None);
        assert_eq!(state.task_poll(thread), 1);
        assert_eq!(state.task_take(ready), 3);
        assert_eq!(state.task_cancel(standalone), STATUS_OK);
        assert_eq!(state.task_poll(standalone), 2);
        if let Some(Object::Scope { cancelled, .. }) = state.object_mut(scope) {
            *cancelled = true;
        }
        assert_eq!(state.task_spawn(Some(scope), 6, true), 0);

        let oneshot_pending = state.oneshot_new();
        let oneshot_ready = state.oneshot_new();
        let oneshot_cancelled = state.oneshot_new();
        assert_eq!(state.oneshot_complete(oneshot_ready, 7), STATUS_OK);
        assert_eq!(state.oneshot_cancel(oneshot_cancelled), STATUS_OK);
        assert_eq!(
            state.source_kind(oneshot_pending),
            Some(SelectSourceKind::OneShot)
        );
        assert_eq!(
            state.source_ready(oneshot_pending, SelectSourceKind::OneShot),
            Some(false)
        );
        assert_eq!(
            state.source_ready(oneshot_ready, SelectSourceKind::OneShot),
            Some(true)
        );
        assert_eq!(
            state.source_ready(oneshot_cancelled, SelectSourceKind::OneShot),
            Some(true)
        );

        let timer_pending = state.time_new(8);
        let timer_ready = state.time_new(9);
        assert_eq!(state.time_fire(timer_ready), STATUS_OK);
        assert_eq!(
            state.source_kind(timer_pending),
            Some(SelectSourceKind::Timer)
        );
        assert_eq!(
            state.source_ready(timer_pending, SelectSourceKind::Timer),
            Some(false)
        );
        assert_eq!(
            state.source_ready(timer_ready, SelectSourceKind::Timer),
            Some(true)
        );

        let selection = state.select_begin(2);
        let unrelated = state.task_spawn(None, 10, true);
        assert_eq!(
            state.select_register(selection, pending, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(
            state.select_register(selection, oneshot_pending, SelectSourceKind::OneShot, false),
            STATUS_OK
        );
        assert_eq!(state.select_commit(selection, false), STATUS_NOT_READY);
        state.notify_selects(unrelated);
        assert_eq!(state.select_wakeups(selection), 0);
        state.notify_selects(pending);
        assert_eq!(state.select_wakeups(selection), 1);

        let task_pending = state.task_spawn(None, 11, true);
        let task_ready = state.task_spawn(None, 12, false);
        let task_cancelled = state.task_spawn(None, 13, true);
        let task_joined = state.task_spawn(None, 14, false);
        assert_eq!(state.task_cancel(task_cancelled), STATUS_OK);
        assert_eq!(state.task_take(task_joined), 14);
        state.discard_select_source(SelectArm {
            source: task_pending,
            kind: SelectSourceKind::Task,
            owned: true,
        });
        state.discard_select_source(SelectArm {
            source: task_ready,
            kind: SelectSourceKind::Task,
            owned: true,
        });
        state.discard_select_source(SelectArm {
            source: task_cancelled,
            kind: SelectSourceKind::Task,
            owned: true,
        });
        state.discard_select_source(SelectArm {
            source: task_joined,
            kind: SelectSourceKind::Task,
            owned: true,
        });

        let one_pending = state.oneshot_new();
        let one_ready = state.oneshot_new();
        let one_cancelled = state.oneshot_new();
        let one_consumed = state.oneshot_new();
        assert_eq!(state.oneshot_complete(one_ready, 15), STATUS_OK);
        assert_eq!(state.oneshot_cancel(one_cancelled), STATUS_OK);
        assert_eq!(state.oneshot_complete(one_consumed, 16), STATUS_OK);
        assert_eq!(
            state.take_select_source(one_consumed, SelectSourceKind::OneShot),
            16
        );
        for source in [one_pending, one_ready, one_cancelled, one_consumed] {
            state.discard_select_source(SelectArm {
                source,
                kind: SelectSourceKind::OneShot,
                owned: true,
            });
        }

        let time_pending = state.time_new(17);
        let time_ready = state.time_new(18);
        let time_cancelled = state.time_new(19);
        assert_eq!(state.time_fire(time_ready), STATUS_OK);
        if let Some(Object::Timer {
            state: timer_state, ..
        }) = state.object_mut(time_cancelled)
        {
            *timer_state = TimerState::Cancelled;
        }
        for source in [time_pending, time_ready, time_cancelled] {
            state.discard_select_source(SelectArm {
                source,
                kind: SelectSourceKind::Timer,
                owned: true,
            });
        }
        state.discard_select_source(SelectArm {
            source: result,
            kind: SelectSourceKind::Timer,
            owned: true,
        });

        let take_task_pending = state.task_spawn(None, 20, true);
        assert_eq!(
            state.take_select_source(take_task_pending, SelectSourceKind::Task),
            0
        );
        assert_eq!(state.last_status, STATUS_NOT_READY);
        let take_task_cancelled = state.task_spawn(None, 21, true);
        assert_eq!(state.task_cancel(take_task_cancelled), STATUS_OK);
        assert_eq!(
            state.take_select_source(take_task_cancelled, SelectSourceKind::Task),
            0
        );
        assert_eq!(state.last_status, STATUS_CANCELLED);
        let take_task_ready = state.task_spawn(None, 22, false);
        assert_eq!(
            state.take_select_source(take_task_ready, SelectSourceKind::Task),
            22
        );
        let take_one_pending = state.oneshot_new();
        assert_eq!(
            state.take_select_source(take_one_pending, SelectSourceKind::OneShot),
            0
        );
        assert_eq!(state.last_status, STATUS_NOT_READY);
        let take_one_cancelled = state.oneshot_new();
        assert_eq!(state.oneshot_cancel(take_one_cancelled), STATUS_OK);
        assert_eq!(
            state.take_select_source(take_one_cancelled, SelectSourceKind::OneShot),
            0
        );
        assert_eq!(state.last_status, STATUS_CANCELLED);
        let take_one_ready = state.oneshot_new();
        assert_eq!(state.oneshot_complete(take_one_ready, 23), STATUS_OK);
        assert_eq!(
            state.take_select_source(take_one_ready, SelectSourceKind::OneShot),
            23
        );
        let take_time_pending = state.time_new(24);
        assert_eq!(
            state.take_select_source(take_time_pending, SelectSourceKind::Timer),
            0
        );
        assert_eq!(state.last_status, STATUS_NOT_READY);
        let take_time_cancelled = state.time_new(25);
        if let Some(Object::Timer {
            state: timer_state, ..
        }) = state.object_mut(take_time_cancelled)
        {
            *timer_state = TimerState::Cancelled;
        }
        assert_eq!(
            state.take_select_source(take_time_cancelled, SelectSourceKind::Timer),
            0
        );
        assert_eq!(state.last_status, STATUS_CANCELLED);
        let take_time_ready = state.time_new(26);
        assert_eq!(state.time_fire(take_time_ready), STATUS_OK);
        assert_eq!(
            state.take_select_source(take_time_ready, SelectSourceKind::Timer),
            26
        );
        assert_eq!(state.take_select_source(result, SelectSourceKind::Task), 0);
        assert_eq!(state.last_status, STATUS_INVALID_HANDLE);
    }

    #[test]
    fn native_select_state_machine_covers_registration_and_phase_errors() {
        let _guard = test_guard();
        let mut state = State::new();
        assert_eq!(state.select_begin(0), 0);
        assert_eq!(state.select_begin(u64::from(MAX_SELECT_ARMS) + 1), 0);

        let task = state.task_spawn(None, 30, true);
        let other = state.task_spawn(None, 31, true);
        let result = state.alloc(
            Object::Result {
                tag: RESULT_ERR,
                payload: None,
            },
            ObjectKind::Result,
        );
        let full = state.select_begin(1);
        assert_eq!(
            state.select_register(full, result, SelectSourceKind::Task, false),
            STATUS_INVALID_HANDLE
        );
        assert_eq!(
            state.select_register(0, task, SelectSourceKind::Task, false),
            STATUS_INVALID_HANDLE
        );
        assert_eq!(
            state.select_register(full, task, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(
            state.select_register(full, other, SelectSourceKind::Task, false),
            STATUS_INVALID_TRANSITION
        );

        let duplicate = state.select_begin(2);
        assert_eq!(
            state.select_register(duplicate, task, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(
            state.select_register(duplicate, task, SelectSourceKind::Task, false),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(
            state.select_commit(duplicate, false),
            STATUS_INVALID_TRANSITION
        );

        let waiting = state.select_begin(1);
        let pending = state.task_spawn(None, 32, true);
        assert_eq!(
            state.select_register(waiting, pending, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(state.select_commit(waiting, false), STATUS_NOT_READY);
        assert_eq!(
            state.select_register(waiting, other, SelectSourceKind::Task, false),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(state.select_commit(waiting, true), STATUS_SELECT_ELSE);
        assert_eq!(
            state.select_commit(waiting, false),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(state.select_rollback(waiting), STATUS_INVALID_TRANSITION);

        let committed = state.select_begin(1);
        let ready = state.task_spawn(None, 33, false);
        assert_eq!(
            state.select_register(committed, ready, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(state.select_commit(committed, false), STATUS_OK);
        assert_eq!(
            state.select_commit(committed, false),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(state.select_rollback(committed), STATUS_INVALID_TRANSITION);

        let with_loser = state.select_begin(2);
        let winner_task = state.task_spawn(None, 36, false);
        let loser_task = state.task_spawn(None, 37, true);
        assert_eq!(
            state.select_register(with_loser, winner_task, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(
            state.select_register(with_loser, loser_task, SelectSourceKind::Task, true),
            STATUS_OK
        );
        assert_eq!(state.select_commit(with_loser, false), STATUS_OK);
        assert_eq!(state.task_poll(loser_task), 2);
        assert_eq!(state.select_take(with_loser), 36);

        let no_winner = state.select_begin(1);
        let no_winner_task = state.task_spawn(None, 34, true);
        assert_eq!(
            state.select_register(no_winner, no_winner_task, SelectSourceKind::Task, false),
            STATUS_OK
        );
        assert_eq!(state.select_commit(no_winner, false), STATUS_NOT_READY);
        if let Some(Object::Select(selection)) = state.object_mut(no_winner) {
            selection.phase = SelectPhase::Committed;
            selection.winner = None;
        }
        assert_eq!(state.select_winner(no_winner), u64::MAX);
        assert_eq!(state.select_take(no_winner), 0);
        assert_eq!(state.last_status, STATUS_INVALID_TRANSITION);
        assert_eq!(state.select_winner(0), u64::MAX);
        assert_eq!(state.select_take(0), 0);
        assert_eq!(state.select_rollback(0), STATUS_INVALID_HANDLE);
        assert_eq!(state.select_wakeups(0), u64::MAX);
        assert_eq!(state.oneshot_cancel(0), STATUS_INVALID_HANDLE);

        let missing_source = state.select_begin(1);
        let released = state.task_spawn(None, 35, true);
        assert_eq!(
            state.select_register(missing_source, released, SelectSourceKind::Task, true),
            STATUS_OK
        );
        assert_eq!(state.release(released), STATUS_OK);
        assert_eq!(state.select_commit(missing_source, false), STATUS_NOT_READY);
        assert_eq!(state.select_rollback(missing_source), STATUS_OK);
    }

    #[test]
    fn arc_local_and_shared_counts_are_exact_across_worker_retain_release() {
        let _guard = test_guard();
        tondo_rt_reset();
        let value = tondo_rt_result_new(RESULT_OK, 1, 1);
        assert_eq!(tondo_rt_arc_kind(value), 0);
        assert_eq!(tondo_rt_arc_strong_count(value), 1);
        assert_eq!(tondo_rt_mark_shared(value), STATUS_OK);
        assert_eq!(tondo_rt_arc_kind(value), 1);

        let workers = (0..6)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..128 {
                        assert_eq!(tondo_rt_retain(value), STATUS_OK);
                        assert_eq!(tondo_rt_release(value), STATUS_OK);
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("ARC worker must finish");
        }
        assert_eq!(tondo_rt_arc_strong_count(value), 1);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_payload_edges_transfer_on_join_and_release_on_cancel() {
        let _guard = test_guard();
        tondo_rt_reset();

        let payload = tondo_rt_result_new(RESULT_OK, 7, 1);
        let task = tondo_rt_task_spawn(payload, 0);
        assert_eq!(tondo_rt_arc_strong_count(payload), 2);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_task_take(task), payload);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let cancelled_payload = tondo_rt_result_new(RESULT_OK, 8, 1);
        let cancelled = tondo_rt_task_spawn(cancelled_payload, 1);
        assert_eq!(tondo_rt_release(cancelled_payload), STATUS_OK);
        assert_eq!(tondo_rt_task_cancel(cancelled), STATUS_OK);
        assert_eq!(tondo_rt_result_tag(cancelled_payload), u64::MAX);
        assert_eq!(tondo_rt_release(cancelled), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_task_completion_replaces_payload_without_leaking_the_old_value() {
        let _guard = test_guard();
        tondo_rt_reset();
        let old = tondo_rt_result_new(RESULT_OK, 11, 1);
        let task = tondo_rt_task_spawn(old, 1);
        assert_eq!(tondo_rt_release(old), STATUS_OK);
        let replacement = tondo_rt_result_new(RESULT_OK, 12, 1);
        assert_eq!(tondo_rt_task_complete(task, replacement), STATUS_OK);
        assert_eq!(tondo_rt_result_tag(old), u64::MAX);
        assert_eq!(tondo_rt_release(replacement), STATUS_OK);
        assert_eq!(tondo_rt_task_take(task), replacement);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_release(replacement), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_frames_keep_roots_alive_until_normal_or_abort_cleanup() {
        let _guard = test_guard();
        tondo_rt_reset();
        for aborting in [0, 1] {
            let frame = tondo_rt_frame_enter();
            let value = tondo_rt_result_new(RESULT_OK, aborting, 1);
            assert_eq!(tondo_rt_frame_publish_root(frame, value), STATUS_OK);
            assert_eq!(tondo_rt_release(value), STATUS_OK);
            assert_eq!(tondo_rt_result_tag(value), RESULT_OK);
            assert_eq!(tondo_rt_frame_leave(frame, aborting), STATUS_OK);
            assert_eq!(tondo_rt_result_tag(value), u64::MAX);
        }
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_scope_drop_cancels_children_and_drains_their_payload_edges() {
        let _guard = test_guard();
        tondo_rt_reset();
        let scope = tondo_rt_scope_enter();
        let payload = tondo_rt_result_new(RESULT_OK, 21, 1);
        let task = tondo_rt_scope_spawn(scope, payload, 1);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_release(scope), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(task), 2);
        assert_eq!(tondo_rt_result_tag(payload), u64::MAX);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_scope_join_releases_a_consumed_managed_payload() {
        let _guard = test_guard();
        tondo_rt_reset();
        let scope = tondo_rt_scope_enter();
        let payload = tondo_rt_result_new(RESULT_OK, 26, 1);
        let task = tondo_rt_scope_spawn(scope, payload, 0);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_scope_join(scope, task), STATUS_OK);
        assert_eq!(tondo_rt_result_tag(payload), u64::MAX);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_release(scope), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_thread_terminal_clears_runtime_pin_after_worker_barrier() {
        let _guard = test_guard();
        tondo_rt_reset();
        let payload = tondo_rt_result_new(RESULT_OK, 31, 1);
        let thread = tondo_rt_thread_spawn(payload, 0);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_thread_worker_wait(thread), STATUS_OK);
        assert_eq!(tondo_rt_task_take(thread), payload);
        assert_eq!(tondo_rt_release(thread), STATUS_OK);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_select_registration_owns_sources_until_selection_teardown() {
        let _guard = test_guard();
        tondo_rt_reset();
        let payload = tondo_rt_result_new(RESULT_OK, 41, 1);
        let task = tondo_rt_task_spawn(payload, 1);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        let selection = tondo_rt_select_begin(1);
        assert_eq!(tondo_rt_select_register_join(selection, task), STATUS_OK);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_task_wake(task), STATUS_OK);
        assert_eq!(tondo_rt_select_commit(selection, 0), STATUS_OK);
        assert_eq!(tondo_rt_select_take(selection), payload);
        assert_eq!(tondo_rt_release(selection), STATUS_OK);
        assert_eq!(tondo_rt_release(payload), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let owned_payload = tondo_rt_result_new(RESULT_OK, 42, 1);
        let owned_task = tondo_rt_task_spawn(owned_payload, 1);
        let owned_selection = tondo_rt_select_begin(1);
        assert_eq!(
            tondo_rt_select_register_task(owned_selection, owned_task, 1),
            STATUS_OK
        );
        assert_eq!(tondo_rt_release(owned_payload), STATUS_OK);
        assert_eq!(tondo_rt_select_rollback(owned_selection), STATUS_OK);
        assert_eq!(tondo_rt_release(owned_selection), STATUS_OK);
        assert_eq!(tondo_rt_result_tag(owned_payload), u64::MAX);
        assert_eq!(tondo_rt_release(owned_task), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_cycle_collection_reclaims_independent_cycles_and_keeps_weak_tombstones() {
        let _guard = test_guard();
        tondo_rt_reset();

        let rooted_left = tondo_rt_task_spawn(0, 1);
        let rooted_right = tondo_rt_task_spawn(rooted_left, 1);
        assert_eq!(tondo_rt_task_complete(rooted_left, rooted_right), STATUS_OK);
        let frame = tondo_rt_frame_enter();
        assert_eq!(tondo_rt_frame_publish_root(frame, rooted_left), STATUS_OK);
        assert_eq!(tondo_rt_release(rooted_left), STATUS_OK);
        assert_eq!(tondo_rt_release(rooted_right), STATUS_OK);
        assert_eq!(tondo_rt_collect_cycles(), 0);
        assert_eq!(tondo_rt_live_objects(), 2);
        assert_eq!(tondo_rt_frame_unpublish_root(frame, rooted_left), STATUS_OK);
        assert_eq!(tondo_rt_frame_leave(frame, 0), STATUS_OK);
        assert_eq!(tondo_rt_collect_cycles(), 2);
        assert_eq!(tondo_rt_live_objects(), 0);

        let left = tondo_rt_task_spawn(0, 1);
        let right = tondo_rt_task_spawn(left, 1);
        assert_eq!(tondo_rt_task_complete(left, right), STATUS_OK);
        let weak = tondo_rt_weak_new(left);
        assert_eq!(tondo_rt_arc_weak_count(left), 1);
        assert_eq!(tondo_rt_release(left), STATUS_OK);
        assert_eq!(tondo_rt_release(right), STATUS_OK);
        assert_eq!(tondo_rt_collect_cycles(), 2);
        assert_eq!(tondo_rt_live_objects(), 1);
        assert_eq!(tondo_rt_weak_upgrade(weak), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_WEAK_DEAD);
        assert_eq!(tondo_rt_weak_new(left), STATUS_WEAK_DEAD);
        assert_eq!(tondo_rt_release(weak), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let first = tondo_rt_task_spawn(0, 1);
        let second = tondo_rt_task_spawn(first, 1);
        assert_eq!(tondo_rt_task_complete(first, second), STATUS_OK);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        assert_eq!(tondo_rt_release(second), STATUS_OK);
        for index in 0..COLLECTION_PRESSURE {
            let filler = tondo_rt_result_new(RESULT_OK, u64::from(index), 1);
            assert_eq!(tondo_rt_release(filler), STATUS_OK);
        }
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_weak_upgrade_is_strong_while_alive_and_never_resurrects_after_release() {
        let _guard = test_guard();
        tondo_rt_reset();
        let target = tondo_rt_result_new(RESULT_OK, 55, 1);
        let weak = tondo_rt_weak_new(target);
        let upgraded = tondo_rt_weak_upgrade(weak);
        assert_eq!(upgraded, target);
        assert_eq!(tondo_rt_arc_strong_count(target), 2);
        assert_eq!(tondo_rt_release(target), STATUS_OK);
        assert_eq!(tondo_rt_release(upgraded), STATUS_OK);
        assert_eq!(tondo_rt_arc_weak_count(target), 1);

        let attempts = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    assert_eq!(tondo_rt_weak_upgrade(weak), 0);
                    assert_eq!(tondo_rt_last_status(), STATUS_WEAK_DEAD);
                })
            })
            .collect::<Vec<_>>();
        for attempt in attempts {
            attempt.join().expect("weak upgrade probe must finish");
        }
        assert_eq!(tondo_rt_release(weak), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_weak_upgrade_linearizes_concurrent_alive_attempts() {
        let _guard = test_guard();
        tondo_rt_reset();
        let target = tondo_rt_result_new(RESULT_OK, 57, 1);
        assert_eq!(tondo_rt_mark_shared(target), STATUS_OK);
        let weak = tondo_rt_weak_new(target);

        let attempts = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let upgraded = tondo_rt_weak_upgrade(weak);
                    assert_eq!(upgraded, target);
                    assert_eq!(tondo_rt_release(upgraded), STATUS_OK);
                })
            })
            .collect::<Vec<_>>();
        for attempt in attempts {
            attempt.join().expect("alive weak upgrade must finish");
        }
        assert_eq!(tondo_rt_arc_strong_count(target), 1);
        assert_eq!(tondo_rt_release(target), STATUS_OK);
        assert_eq!(tondo_rt_weak_upgrade(weak), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_WEAK_DEAD);
        assert_eq!(tondo_rt_release(weak), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn arc_weak_handles_are_not_cloneable_and_double_release_is_fail_closed() {
        let _guard = test_guard();
        tondo_rt_reset();
        let target = tondo_rt_result_new(RESULT_OK, 61, 1);
        let weak = tondo_rt_weak_new(target);
        assert_eq!(tondo_rt_cow_clone(weak), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_release(target), STATUS_OK);
        assert_eq!(tondo_rt_release(weak), STATUS_OK);
        assert_eq!(tondo_rt_release(weak), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_diagnostic_capture_reports_logical_race_and_redaction_fields() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_diag_probe(DIAG_PROFILE_RACE, 0), STATUS_DIAG_CLEAN);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_PROFILE), DIAG_PROFILE_RACE);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_TASK_IDS), 2);
        assert!(tondo_rt_diag_field(DIAG_FIELD_HAPPENS_BEFORE) >= 2);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_SOURCE_MAPS), 2);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_REDACTED), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_PAYLOADS_OMITTED), 1);

        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_RACE, 1),
            STATUS_DIAG_FINDING
        );
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_MODE), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_STATUS), STATUS_DIAG_FINDING);
        tondo_rt_diag_reset();
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_STATUS), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
    }

    #[test]
    fn native_diagnostic_capture_distinguishes_growth_and_arc_cycle_recovery() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_diag_probe(DIAG_PROFILE_LEAK, 0), STATUS_DIAG_CLEAN);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_ROOTS), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_RETAINERS), 0);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_RESOURCES_ACQUIRED), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_RESOURCES_RELEASED), 1);

        tondo_rt_reset();
        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_LEAK, 1),
            STATUS_DIAG_FINDING
        );
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_RETAINERS), 2);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_FFI_ALLOCATIONS), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_RESOURCES_RELEASED), 0);

        tondo_rt_reset();
        assert_eq!(tondo_rt_diag_probe(DIAG_PROFILE_LEAK, 2), STATUS_DIAG_CLEAN);
        assert!(tondo_rt_diag_field(DIAG_FIELD_CYCLES_RECLAIMED) >= 2);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_diagnostic_capture_records_crash_limits_and_rejects_unknown_profiles() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_CRASH, 0),
            STATUS_DIAG_CAPTURED
        );
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_UNWIND_FRAMES), 2);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_SOURCE_MAPS), 3);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_CORRUPTION_REJECTED), 0);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_LIMIT_ENFORCED), 0);

        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_CRASH, 1),
            STATUS_DIAG_CAPTURED
        );
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_CORRUPTION_REJECTED), 1);
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_LIMIT_ENFORCED), 0);
        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_CRASH, 2),
            STATUS_DIAG_CAPTURED
        );
        assert_eq!(tondo_rt_diag_field(DIAG_FIELD_LIMIT_ENFORCED), 1);

        assert_eq!(tondo_rt_diag_probe(99, 0), STATUS_DIAG_UNSUPPORTED);
        assert_eq!(
            tondo_rt_diag_field(DIAG_FIELD_STATUS),
            STATUS_DIAG_UNSUPPORTED
        );
        assert_eq!(
            tondo_rt_diag_probe(DIAG_PROFILE_RACE, 2),
            STATUS_DIAG_UNSUPPORTED
        );
    }
}
