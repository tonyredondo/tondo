//! Private runtime primitives used by the first native backend.
//!
//! The native code generator only exchanges `u64` tokens with this library.
//! Tokens are capabilities into a process-local table; they are not pointers,
//! object addresses, or a public FFI.  Keeping the table behind a mutex makes
//! the bootstrap implementation deterministic and safe while the compiler
//! ABI remains explicit about the future atomic fast paths.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

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
const MAX_SELECT_ARMS: u32 = 64;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    strong: u32,
    weak: u32,
    root_count: u32,
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

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    objects: BTreeMap<u64, (Entry, Object)>,
    frames: BTreeMap<u64, Frame>,
    next_frame: u64,
    last_status: u64,
    select_rotation: u64,
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
        let id = HANDLE_BIT | self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.objects.insert(
            id,
            (
                Entry {
                    strong: 1,
                    weak: 0,
                    root_count: 0,
                    object: object_kind,
                },
                object,
            ),
        );
        id
    }

    fn object(&self, handle: u64) -> Option<&Object> {
        self.objects.get(&handle).map(|(_, object)| object)
    }

    fn object_mut(&mut self, handle: u64) -> Option<&mut Object> {
        self.objects.get_mut(&handle).map(|(_, object)| object)
    }

    fn entry_mut(&mut self, handle: u64) -> Option<&mut Entry> {
        self.objects.get_mut(&handle).map(|(entry, _)| entry)
    }

    fn valid_handle(handle: u64) -> bool {
        handle & HANDLE_BIT != 0
    }

    fn retain(&mut self, handle: u64) -> u64 {
        let Some(entry) = self.entry_mut(handle) else {
            return STATUS_INVALID_HANDLE;
        };
        if entry.strong == 0 {
            return STATUS_INVALID_HANDLE;
        }
        entry.strong = entry.strong.saturating_add(1);
        STATUS_OK
    }

    fn release(&mut self, handle: u64) -> u64 {
        let Some((entry, _)) = self.objects.get_mut(&handle) else {
            return STATUS_INVALID_HANDLE;
        };
        if entry.strong == 0 {
            return STATUS_DOUBLE_RELEASE;
        }
        entry.strong -= 1;
        if entry.strong == 0 && entry.root_count == 0 && entry.weak == 0 {
            self.objects.remove(&handle);
        }
        STATUS_OK
    }

    fn clone_value(&mut self, handle: u64) -> u64 {
        let Some((entry, object)) = self.objects.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        };
        if entry.strong == 1 {
            return handle;
        }
        self.alloc(object, entry.object)
    }

    fn create_frame(&mut self) -> u64 {
        let frame = self.next_frame;
        self.next_frame = self.next_frame.saturating_add(1);
        self.frames.insert(frame, Frame::default());
        frame
    }

    fn publish_root(&mut self, frame: u64, value: u64) -> u64 {
        if !Self::valid_handle(value) || self.object(value).is_none() {
            return STATUS_INVALID_HANDLE;
        }
        let Some(frame_state) = self.frames.get_mut(&frame) else {
            return STATUS_INVALID_HANDLE;
        };
        *frame_state.roots.entry(value).or_default() += 1;
        if let Some(entry) = self.entry_mut(value) {
            entry.root_count = entry.root_count.saturating_add(1);
        }
        STATUS_OK
    }

    fn unpublish_root(&mut self, frame: u64, value: u64) -> u64 {
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
        if let Some(entry) = self.entry_mut(value) {
            entry.root_count = entry.root_count.saturating_sub(1);
        }
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
        let roots = frame_state.roots.keys().copied().collect::<Vec<_>>();
        for root in roots {
            let _ = self.unpublish_root(frame, root);
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
        self.task_spawn_with_kind(None, value, pending, TaskKind::Thread)
    }

    fn task_wake(&mut self, task: u64) -> u64 {
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state == TaskState::Cancelled || *state == TaskState::Joined {
            return STATUS_INVALID_TRANSITION;
        }
        *state = TaskState::Ready;
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
        *value
    }

    fn task_cancel(&mut self, task: u64) -> u64 {
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return STATUS_INVALID_HANDLE;
        };
        if matches!(*state, TaskState::Cancelled | TaskState::Joined) {
            return STATUS_INVALID_TRANSITION;
        }
        *state = TaskState::Cancelled;
        self.notify_selects(task);
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
        match (arm.kind, self.object_mut(arm.source)) {
            (SelectSourceKind::Task, Some(Object::Task { state, .. })) => match state {
                TaskState::Pending => *state = TaskState::Cancelled,
                TaskState::Ready => *state = TaskState::Joined,
                TaskState::Cancelled | TaskState::Joined => {}
            },
            (SelectSourceKind::OneShot, Some(Object::OneShot { state, .. })) => match state {
                OneShotState::Pending => *state = OneShotState::Cancelled,
                OneShotState::Ready => *state = OneShotState::Consumed,
                OneShotState::Cancelled | OneShotState::Consumed => {}
            },
            (SelectSourceKind::Timer, Some(Object::Timer { state, .. })) => match state {
                TimerState::Pending => *state = TimerState::Cancelled,
                TimerState::Ready => *state = TimerState::Consumed,
                TimerState::Cancelled | TimerState::Consumed => {}
            },
            _ => {}
        }
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
                *value
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
                *value
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
                *value
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
        let Some(Object::Select(state)) = self.object_mut(selection) else {
            return STATUS_INVALID_HANDLE;
        };
        if state.phase != SelectPhase::Preparing
            || state.arms.len() >= state.capacity as usize
            || state.arms.iter().any(|arm| arm.source == source)
        {
            return STATUS_INVALID_TRANSITION;
        }
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
        let Some(Object::OneShot { state, .. }) = self.object_mut(oneshot) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state != OneShotState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        if let Some(Object::OneShot { state, value: slot }) = self.object_mut(oneshot) {
            *slot = value;
            *state = OneShotState::Ready;
        }
        self.notify_selects(oneshot);
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
    with_state(|state| state.task_take(task))
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
        let _ = state.task_take(task);
        STATUS_OK
    })
}

pub extern "C" fn tondo_rt_await(task: u64) -> u64 {
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
}
