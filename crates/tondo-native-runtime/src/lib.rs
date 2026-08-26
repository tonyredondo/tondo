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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Object {
    Result { tag: u64, payload: Option<u64> },
    Task { state: TaskState, value: u64 },
    Scope { tasks: Vec<u64>, cancelled: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Pending,
    Ready,
    Cancelled,
    Joined,
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
}

impl State {
    fn new() -> Self {
        Self {
            next_id: 1,
            next_frame: 1,
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

    fn task_spawn(&mut self, scope: Option<u64>, value: u64, pending: bool) -> u64 {
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

    fn task_wake(&mut self, task: u64) -> u64 {
        let Some(Object::Task { state, .. }) = self.object_mut(task) else {
            return STATUS_INVALID_HANDLE;
        };
        if *state == TaskState::Cancelled || *state == TaskState::Joined {
            return STATUS_INVALID_TRANSITION;
        }
        *state = TaskState::Ready;
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
        let Some(Object::Task { state, value }) = self.object_mut(task) else {
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
}
