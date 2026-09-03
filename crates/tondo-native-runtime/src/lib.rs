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
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::thread::ThreadId;
use std::time::Duration;

const HANDLE_BIT: u64 = 1 << 63;
const RESULT_NONE: u64 = 0;
const RESULT_SOME: u64 = 1;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;
/// Private tags used by strong collection compare-exchange results. They are
/// distinct from `Result`/`Option` so a caller can inspect one atomic outcome
/// without racing on the process-wide diagnostic status channel.
const RESULT_CAS_EXCHANGED: u64 = 4;
const RESULT_CAS_MISMATCH: u64 = 5;

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
/// The host capability token is unknown or not selected by the target.
const STATUS_HOST_UNSUPPORTED: u64 = 12;
/// A host handle has reached a terminal cancelled/closed state.
const STATUS_HOST_CLOSED: u64 = 13;
/// A host operation would exceed the resource budget without a partial write.
const STATUS_HOST_LIMIT: u64 = 14;
/// A host handle was cancelled before an operation could make progress.
const STATUS_HOST_CANCELLED: u64 = 15;
/// A task or group child terminated by panicking outside its declared error channel.
const STATUS_PANICKED: u64 = 16;
/// The observed atomic value did not match the compare-exchange expectation.
const STATUS_ATOMIC_MISMATCH: u64 = 17;
/// A memory-order argument is outside the closed native ABI set.
const STATUS_ATOMIC_INVALID_ORDER: u64 = 18;
/// A collection index or operation shape is invalid.
const STATUS_COLLECTION_INVALID: u64 = 19;
/// A channel capacity is negative or cannot be represented by the native
/// resource budget.
const STATUS_CHANNEL_INVALID_CAPACITY: u64 = 20;
/// A non-blocking channel operation found no space in a bounded buffer.
const STATUS_CHANNEL_FULL: u64 = 21;
/// A non-blocking receive found an open channel without a committed value.
const STATUS_CHANNEL_EMPTY: u64 = 22;
/// Blocking executor construction was requested on a target without the
/// promoted target-qualified worker lane.
const STATUS_BLOCKING_UNSUPPORTED_TARGET: u64 = 23;
/// Blocking executor worker count is outside the closed native budget.
const STATUS_BLOCKING_INVALID_WORKERS: u64 = 24;
/// Blocking executor queue capacity is outside the closed native budget.
const STATUS_BLOCKING_INVALID_CAPACITY: u64 = 25;
/// A blocking handle was used with the wrong native object kind.
const STATUS_BLOCKING_INVALID_HANDLE: u64 = 26;
/// A blocking job is still queued or running and has no result to take.
const STATUS_BLOCKING_NOT_READY: u64 = 27;
/// A blocking pool lifecycle transition is not valid for its current state.
const STATUS_BLOCKING_INVALID_TRANSITION: u64 = 28;
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
const MAX_GROUP_CHILDREN: u32 = 64;
const COLLECTION_PRESSURE: u32 = 256;
const HOST_CAP_CONSOLE: u64 = 0;
const HOST_CAP_FILESYSTEM: u64 = 1;
const HOST_CAP_PROCESS: u64 = 2;
const HOST_CAP_CLOCK: u64 = 3;
/// Private host-object tags reserved for the target-qualified blocking ABI.
/// They intentionally do not participate in the public host capability set.
const HOST_CAP_BLOCKING_POOL: u64 = 4;
const HOST_CAP_BLOCKING_JOB: u64 = 5;
const HOST_MAX_BYTES: usize = 1 << 20;

const WORKER_STARTING: u64 = 0;
const WORKER_RUNNING: u64 = 1;
const WORKER_COMPLETED: u64 = 2;
const WORKER_CANCELLED: u64 = 3;
const BLOCKING_JOB_QUEUED: u64 = 0;
const BLOCKING_JOB_RUNNING: u64 = 1;
const BLOCKING_JOB_COMPLETED: u64 = 2;
const BLOCKING_JOB_CANCELLED: u64 = 3;
const BLOCKING_JOB_TAKEN: u64 = 4;

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
        panicked: bool,
        group_owner: Option<u64>,
    },
    Scope {
        tasks: Vec<u64>,
        cancelled: bool,
    },
    Group(GroupData),
    Select(SelectState),
    OneShot {
        state: OneShotState,
        value: u64,
    },
    Timer {
        state: TimerState,
        value: u64,
    },
    /// Capability-gated host resource.  The handle is affine at the source
    /// level; the runtime keeps its state opaque and never exposes an OS
    /// descriptor or address to native code.
    Host {
        capability: u64,
        state: HostState,
        input: Vec<u8>,
        cursor: usize,
        output: Vec<u8>,
    },
    /// A native atomic cell. The numeric value is kept in `State::atomics` so
    /// operations can use the standard library's lock-free atomic primitive
    /// without holding the global handle-table mutex.
    Atomic,
    /// A scheduler parking signal. Waiting is performed only after the signal
    /// has been detached from the global handle table; waking never touches a
    /// cooperative VM worker directly.
    Park,
    /// Shared collection state is stored behind a per-handle synchronization
    /// primitive. The object table keeps only the opaque capability identity;
    /// values never cross the native ABI as pointers.
    SyncArray,
    SyncMap,
    SyncSet,
    SyncStack,
    SyncQueue,
    /// A private, value-only cursor retaining its source collection through
    /// the normal strong-edge graph. Cursor position is stored separately so
    /// concurrent `next` calls serialize without holding the handle-table
    /// mutex while a collection lock is contended.
    SyncCursor {
        collection: u64,
        kind: SyncCursorCollection,
    },
    /// A private channel identity. Endpoint objects retain this identity and
    /// the synchronized state lives in `State::channels`.
    Channel,
    /// An affine sender endpoint. Closing is tracked separately from object
    /// destruction so explicit close and cleanup share one state transition.
    ChannelSender {
        channel: u64,
        closed: bool,
    },
    /// An affine receiver endpoint. The last receiver owns the terminal
    /// obligation to drain committed values through the private drain carrier.
    ChannelReceiver {
        channel: u64,
        closed: bool,
    },
    /// Pending values returned by the native receiver-close bridge. This is an
    /// internal carrier which a future native lowering turns into `Array[T]`;
    /// it keeps managed payloads alive through the normal object graph.
    ChannelDrain {
        values: VecDeque<u64>,
    },
    /// An immutable, bounded byte carrier used by the private host ABI.
    Buffer {
        bytes: Vec<u8>,
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
enum GroupPhase {
    Open,
    Waiting,
    ReadyToConsume,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GroupChild {
    task: u64,
    index: u64,
    queued: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GroupOutcomeRecord {
    index: u64,
    value: u64,
    is_error: bool,
}

/// Native state for the affine `std.async.Group` carrier.
///
/// The group owns one strong edge to every child after `group_add`.  A
/// completion notification only queues the stable insertion index; consuming
/// the queue is the one place that moves the task payload to the caller.  The
/// implementation deliberately keeps the public ABI opaque and exposes only
/// logical indices/statuses for conformance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupData {
    children: Vec<GroupChild>,
    completion_queue: VecDeque<u64>,
    outcomes: Vec<GroupOutcomeRecord>,
    phase: GroupPhase,
    last_index: Option<u64>,
    last_value: u64,
    cleanup_runs: u64,
}

impl Default for GroupData {
    fn default() -> Self {
        Self {
            children: Vec::new(),
            completion_queue: VecDeque::new(),
            outcomes: Vec::new(),
            phase: GroupPhase::Open,
            last_index: None,
            last_value: 0,
            cleanup_runs: 0,
        }
    }
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
enum NativeBlockingLifecycle {
    Open,
    ShuttingDown,
    Cancelling,
    Closed,
    Cancelled,
}

impl NativeBlockingLifecycle {
    fn terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Cancelled)
    }

    fn code(self) -> u64 {
        match self {
            Self::Open => 0,
            Self::ShuttingDown => 1,
            Self::Cancelling => 2,
            Self::Closed => 3,
            Self::Cancelled => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeBlockingJobState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Taken,
}

impl NativeBlockingJobState {
    fn code(self) -> u64 {
        match self {
            Self::Queued => BLOCKING_JOB_QUEUED,
            Self::Running => BLOCKING_JOB_RUNNING,
            Self::Completed => BLOCKING_JOB_COMPLETED,
            Self::Cancelled => BLOCKING_JOB_CANCELLED,
            Self::Taken => BLOCKING_JOB_TAKEN,
        }
    }

    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Taken)
    }
}

#[derive(Debug)]
struct NativeBlockingJobCell {
    state: Mutex<NativeBlockingJob>,
    wake: Condvar,
}

#[derive(Debug)]
struct NativeBlockingJob {
    payload: u64,
    pool: u64,
    state: NativeBlockingJobState,
    worker: u64,
}

type NativeBlockingJobRef = Arc<NativeBlockingJobCell>;

#[derive(Debug)]
struct NativeBlockingPoolState {
    lifecycle: NativeBlockingLifecycle,
    workers: usize,
    capacity: usize,
    queue: VecDeque<NativeBlockingJobRef>,
    active: usize,
}

type NativeBlockingPoolCell = Arc<(Mutex<NativeBlockingPoolState>, Condvar)>;

/// Target-qualified native worker bridge for the blocking executor.
///
/// The current native ABI transports one opaque value token through a bounded
/// OS-worker queue.  It deliberately has no callback or function-pointer ABI:
/// AOT callable lowering remains a separate contract.  This lane proves the
/// physical worker, admission, wakeup and lifecycle semantics without exposing
/// layout or host pointers.
#[derive(Debug)]
struct NativeBlockingPool {
    state: NativeBlockingPoolCell,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl NativeBlockingPool {
    fn new(workers: usize, capacity: usize) -> Result<Arc<Self>, u64> {
        let state = Arc::new((
            Mutex::new(NativeBlockingPoolState {
                lifecycle: NativeBlockingLifecycle::Open,
                workers,
                capacity,
                queue: VecDeque::new(),
                active: 0,
            }),
            Condvar::new(),
        ));
        let pool = Arc::new(Self {
            state: Arc::clone(&state),
            workers: Mutex::new(Vec::with_capacity(workers)),
        });
        for worker in 0..workers {
            let state = Arc::clone(&state);
            let name = format!("tondo-blocking-native-{worker}");
            let handle = std::thread::Builder::new()
                .name(name)
                .spawn(move || native_blocking_worker_loop(worker as u64, state))
                .map_err(|_| STATUS_INVALID_TRANSITION)?;
            pool.workers
                .lock()
                .map_err(|_| STATUS_INVALID_TRANSITION)?
                .push(handle);
        }
        Ok(pool)
    }

    fn can_admit(&self) -> Result<bool, u64> {
        let state = self.state.0.lock().map_err(|_| STATUS_INVALID_TRANSITION)?;
        if state.lifecycle != NativeBlockingLifecycle::Open {
            return Ok(false);
        }
        let admitted = state.active.saturating_add(state.queue.len());
        let limit = if state.capacity == 0 {
            state.workers
        } else {
            state.capacity
        };
        Ok(admitted < limit)
    }

    fn submit(&self, job: NativeBlockingJobRef) -> Result<bool, u64> {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().map_err(|_| STATUS_INVALID_TRANSITION)?;
        if state.lifecycle != NativeBlockingLifecycle::Open {
            return Ok(false);
        }
        let admitted = state.active.saturating_add(state.queue.len());
        let limit = if state.capacity == 0 {
            state.workers
        } else {
            state.capacity
        };
        if admitted >= limit {
            return Ok(false);
        }
        state.queue.push_back(job);
        wake.notify_one();
        Ok(true)
    }

    fn shutdown(&self, cancel: bool) -> u64 {
        let (lock, wake) = &*self.state;
        let mut state = match lock.lock() {
            Ok(state) => state,
            Err(_) => return STATUS_INVALID_TRANSITION,
        };
        if state.lifecycle.terminal()
            || (state.lifecycle != NativeBlockingLifecycle::Open
                && !(cancel && state.lifecycle == NativeBlockingLifecycle::ShuttingDown))
        {
            return STATUS_BLOCKING_INVALID_TRANSITION;
        }
        state.lifecycle = if cancel {
            NativeBlockingLifecycle::Cancelling
        } else {
            NativeBlockingLifecycle::ShuttingDown
        };
        if cancel {
            while let Some(job) = state.queue.pop_front() {
                let mut job_state = match job.state.lock() {
                    Ok(job_state) => job_state,
                    Err(_) => return STATUS_INVALID_TRANSITION,
                };
                if job_state.state == NativeBlockingJobState::Queued {
                    job_state.state = NativeBlockingJobState::Cancelled;
                    job.wake.notify_all();
                }
            }
        }
        if state.active == 0 && state.queue.is_empty() {
            state.lifecycle = if cancel {
                NativeBlockingLifecycle::Cancelled
            } else {
                NativeBlockingLifecycle::Closed
            };
        }
        wake.notify_all();
        while !state.lifecycle.terminal() {
            state = match wake.wait(state) {
                Ok(state) => state,
                Err(_) => return STATUS_INVALID_TRANSITION,
            };
        }
        drop(state);
        self.join_workers();
        if cancel { STATUS_CANCELLED } else { STATUS_OK }
    }

    fn cancel_job(&self, job: &NativeBlockingJobRef) -> u64 {
        let (lock, wake) = &*self.state;
        let mut state = match lock.lock() {
            Ok(state) => state,
            Err(_) => return STATUS_INVALID_TRANSITION,
        };
        let Some(position) = state
            .queue
            .iter()
            .position(|candidate| Arc::ptr_eq(candidate, job))
        else {
            return STATUS_OK;
        };
        let job = state
            .queue
            .remove(position)
            .expect("blocking queue position exists");
        let mut job_state = match job.state.lock() {
            Ok(job_state) => job_state,
            Err(_) => return STATUS_INVALID_TRANSITION,
        };
        if job_state.state == NativeBlockingJobState::Queued {
            job_state.state = NativeBlockingJobState::Cancelled;
            job.wake.notify_all();
        }
        wake.notify_all();
        STATUS_OK
    }

    fn join_workers(&self) {
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for NativeBlockingPool {
    fn drop(&mut self) {
        let _ = self.shutdown(true);
        self.join_workers();
    }
}

fn native_blocking_worker_loop(worker: u64, state: NativeBlockingPoolCell) {
    loop {
        let job = {
            let (lock, wake) = &*state;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            loop {
                if let Some(job) = state.queue.pop_front() {
                    state.active = state.active.saturating_add(1);
                    break job;
                }
                match state.lifecycle {
                    NativeBlockingLifecycle::Closed | NativeBlockingLifecycle::Cancelled => {
                        return;
                    }
                    NativeBlockingLifecycle::ShuttingDown => {
                        state.lifecycle = NativeBlockingLifecycle::Closed;
                        wake.notify_all();
                        return;
                    }
                    NativeBlockingLifecycle::Open | NativeBlockingLifecycle::Cancelling => {
                        state = match wake.wait(state) {
                            Ok(state) => state,
                            Err(_) => return,
                        };
                    }
                }
            }
        };
        {
            let mut job_state = match job.state.lock() {
                Ok(job_state) => job_state,
                Err(_) => return,
            };
            if job_state.state != NativeBlockingJobState::Queued {
                let (lock, wake) = &*state;
                if let Ok(mut state) = lock.lock() {
                    state.active = state.active.saturating_sub(1);
                    wake.notify_all();
                }
                continue;
            }
            job_state.state = NativeBlockingJobState::Running;
            job_state.worker = worker;
            job.wake.notify_all();
        }
        // The token-only lane intentionally performs no user callback.  The
        // worker boundary itself is the target-qualified evidence until AOT
        // callable lowering defines a separate envelope.
        std::thread::yield_now();
        {
            let mut job_state = match job.state.lock() {
                Ok(job_state) => job_state,
                Err(_) => return,
            };
            if job_state.state == NativeBlockingJobState::Running {
                job_state.state = NativeBlockingJobState::Completed;
                job.wake.notify_all();
            }
        }
        let (lock, wake) = &*state;
        let Ok(mut state) = lock.lock() else {
            return;
        };
        state.active = state.active.saturating_sub(1);
        if state.active == 0 && state.queue.is_empty() {
            state.lifecycle = match state.lifecycle {
                NativeBlockingLifecycle::ShuttingDown => NativeBlockingLifecycle::Closed,
                NativeBlockingLifecycle::Cancelling => NativeBlockingLifecycle::Cancelled,
                lifecycle => lifecycle,
            };
        }
        wake.notify_all();
    }
}

fn native_blocking_supported() -> bool {
    cfg!(all(target_arch = "x86_64", target_os = "linux"))
}

#[derive(Debug, Clone, Copy)]
struct ParkingSnapshot {
    epoch: u64,
    waiters: u64,
}

/// A native wakeup edge detached from the global handle table. The epoch
/// closes the check-then-sleep race: a waiter records the observed epoch while
/// holding the signal lock, and a notifier increments it before notifying.
#[derive(Debug)]
struct ParkingSignal {
    snapshot: Mutex<ParkingSnapshot>,
    wake: Condvar,
}

impl ParkingSignal {
    fn new() -> Self {
        Self {
            snapshot: Mutex::new(ParkingSnapshot {
                epoch: 0,
                waiters: 0,
            }),
            wake: Condvar::new(),
        }
    }

    fn epoch(&self) -> u64 {
        self.snapshot
            .lock()
            .expect("native parking signal is not poisoned")
            .epoch
    }

    fn wait(&self, expected: u64, timeout_ns: u64) -> u64 {
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("native parking signal is not poisoned");
        if snapshot.epoch != expected {
            return STATUS_OK;
        }
        snapshot.waiters = snapshot.waiters.saturating_add(1);
        let timed_out = if timeout_ns == u64::MAX {
            while snapshot.epoch == expected {
                snapshot = self
                    .wake
                    .wait(snapshot)
                    .expect("native parking signal is not poisoned");
            }
            false
        } else {
            let timeout = Duration::from_nanos(timeout_ns);
            let (next, result) = self
                .wake
                .wait_timeout_while(snapshot, timeout, |state| state.epoch == expected)
                .expect("native parking signal is not poisoned");
            snapshot = next;
            result.timed_out() && snapshot.epoch == expected
        };
        snapshot.waiters = snapshot.waiters.saturating_sub(1);
        if timed_out {
            STATUS_NOT_READY
        } else {
            STATUS_OK
        }
    }

    fn wake(&self, all: bool) -> u64 {
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("native parking signal is not poisoned");
        snapshot.epoch = snapshot.epoch.saturating_add(1);
        let waiters = snapshot.waiters;
        if all {
            self.wake.notify_all();
        } else {
            self.wake.notify_one();
        }
        waiters
    }

    fn waiters(&self) -> u64 {
        self.snapshot
            .lock()
            .expect("native parking signal is not poisoned")
            .waiters
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
enum HostState {
    Open,
    Cancelled,
    Closed,
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
    Group,
    Select,
    OneShot,
    Timer,
    Host,
    Atomic,
    Park,
    Buffer,
    Weak,
    SyncArray,
    SyncMap,
    SyncSet,
    SyncStack,
    SyncQueue,
    SyncCursor,
    Channel,
    ChannelSender,
    ChannelReceiver,
    ChannelDrain,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncCursorCollection {
    Array,
    Map,
    Set,
    Stack,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncMapEntry {
    key: u64,
    value: u64,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncValueEntry {
    value: u64,
    generation: u64,
}

#[derive(Debug, Default)]
struct SyncMapState {
    entries: Vec<SyncMapEntry>,
    next_generation: u64,
}

#[derive(Debug, Default)]
struct SyncSetState {
    entries: Vec<SyncValueEntry>,
    next_generation: u64,
}

#[derive(Debug, Default)]
struct SyncStackState {
    entries: Vec<SyncValueEntry>,
    next_generation: u64,
}

#[derive(Debug, Default)]
struct SyncQueueState {
    entries: VecDeque<SyncValueEntry>,
    next_generation: u64,
}

#[derive(Debug)]
struct SyncCursorState {
    horizon: u64,
    position: u64,
    descending: bool,
    current_key: Option<u64>,
}

type SyncArrayCell = Arc<RwLock<Vec<u64>>>;
type SyncMapCell = Arc<RwLock<SyncMapState>>;
type SyncSetCell = Arc<RwLock<SyncSetState>>;
type SyncStackCell = Arc<Mutex<SyncStackState>>;
type SyncQueueCell = Arc<Mutex<SyncQueueState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeChannelSendOutcome {
    Committed,
    Closed,
    Full,
    ResourceLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeChannelReceiveOutcome {
    Value(u64),
    Empty,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeChannelSendWaiter {
    id: u64,
    value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeChannelReceiveWaiter {
    id: u64,
}

#[derive(Debug)]
struct NativeChannelState {
    /// `Some(0)` is a rendezvous; `None` is the explicit unbounded form.
    capacity: Option<usize>,
    queue: VecDeque<u64>,
    senders: u64,
    receivers: u64,
    sender_closed: bool,
    receiver_closed: bool,
    next_waiter: u64,
    send_waiters: VecDeque<NativeChannelSendWaiter>,
    receive_waiters: VecDeque<NativeChannelReceiveWaiter>,
    send_results: BTreeMap<u64, NativeChannelSendOutcome>,
    receive_results: BTreeMap<u64, NativeChannelReceiveOutcome>,
}

type NativeChannelCell = Arc<(Mutex<NativeChannelState>, Condvar)>;

impl NativeChannelState {
    fn new(capacity: Option<usize>) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
            senders: 0,
            receivers: 0,
            sender_closed: false,
            receiver_closed: false,
            next_waiter: 1,
            send_waiters: VecDeque::new(),
            receive_waiters: VecDeque::new(),
            send_results: BTreeMap::new(),
            receive_results: BTreeMap::new(),
        }
    }
}

enum SyncCursorSource {
    Array(SyncArrayCell, Arc<ParkingSignal>),
    Map(SyncMapCell, Arc<ParkingSignal>),
    Set(SyncSetCell, Arc<ParkingSignal>),
    Stack(SyncStackCell, Arc<ParkingSignal>),
    Queue(SyncQueueCell, Arc<ParkingSignal>),
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
    blocking_pools: BTreeMap<u64, Arc<NativeBlockingPool>>,
    blocking_jobs: BTreeMap<u64, NativeBlockingJobRef>,
    atomics: BTreeMap<u64, Arc<std::sync::atomic::AtomicU64>>,
    parks: BTreeMap<u64, Arc<ParkingSignal>>,
    sync_arrays: BTreeMap<u64, SyncArrayCell>,
    sync_maps: BTreeMap<u64, SyncMapCell>,
    sync_sets: BTreeMap<u64, SyncSetCell>,
    sync_stacks: BTreeMap<u64, SyncStackCell>,
    sync_queues: BTreeMap<u64, SyncQueueCell>,
    sync_collection_parks: BTreeMap<u64, Arc<ParkingSignal>>,
    sync_cursors: BTreeMap<u64, Arc<Mutex<SyncCursorState>>>,
    channels: BTreeMap<u64, NativeChannelCell>,
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
            Object::Group(group) => {
                children.extend(group.children.iter().map(|child| child.task));
                children.push(group.last_value);
            }
            Object::SyncCursor { collection, .. } => children.push(*collection),
            Object::ChannelSender { channel, .. } | Object::ChannelReceiver { channel, .. } => {
                children.push(*channel);
            }
            Object::ChannelDrain { values } => children.extend(values.iter().copied()),
            Object::Select(selection) => {
                children.extend(selection.arms.iter().map(|arm| arm.source));
            }
            Object::Channel
            | Object::Host { .. }
            | Object::Atomic
            | Object::Park
            | Object::SyncArray
            | Object::SyncMap
            | Object::SyncSet
            | Object::SyncStack
            | Object::SyncQueue
            | Object::Buffer { .. }
            | Object::Weak { .. }
            | Object::Tombstone
            | Object::Result { .. } => {}
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
        if matches!(
            object,
            Object::Host { .. }
                | Object::Weak { .. }
                | Object::Tombstone
                | Object::SyncCursor { .. }
                | Object::Channel
                | Object::ChannelSender { .. }
                | Object::ChannelReceiver { .. }
                | Object::ChannelDrain { .. }
        ) {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        if entry.strong.load() == 1 {
            return handle;
        }
        let object_kind = entry.object;
        let object = object.clone();
        if matches!(
            object,
            Object::Atomic
                | Object::Park
                | Object::SyncArray
                | Object::SyncMap
                | Object::SyncSet
                | Object::SyncStack
                | Object::SyncQueue
        ) {
            let clone = self.alloc(object.clone(), object_kind);
            match object {
                Object::Atomic => {
                    if let Some(shared) = self.atomics.get(&handle).cloned() {
                        self.atomics.insert(clone, shared);
                    }
                }
                Object::Park => {
                    if let Some(shared) = self.parks.get(&handle).cloned() {
                        self.parks.insert(clone, shared);
                    }
                }
                Object::SyncArray => {
                    if let Some(shared) = self.sync_arrays.get(&handle).cloned() {
                        self.sync_arrays.insert(clone, shared);
                    }
                }
                Object::SyncMap => {
                    if let Some(shared) = self.sync_maps.get(&handle).cloned() {
                        self.sync_maps.insert(clone, shared);
                    }
                }
                Object::SyncSet => {
                    if let Some(shared) = self.sync_sets.get(&handle).cloned() {
                        self.sync_sets.insert(clone, shared);
                    }
                }
                Object::SyncStack => {
                    if let Some(shared) = self.sync_stacks.get(&handle).cloned() {
                        self.sync_stacks.insert(clone, shared);
                    }
                }
                Object::SyncQueue => {
                    if let Some(shared) = self.sync_queues.get(&handle).cloned() {
                        self.sync_queues.insert(clone, shared);
                    }
                }
                _ => unreachable!("specialized clone was checked above"),
            }
            if let Some(shared) = self.sync_collection_parks.get(&handle).cloned() {
                self.sync_collection_parks.insert(clone, shared);
            }
            return clone;
        }
        self.alloc(object, object_kind)
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
            self.blocking_pools.remove(&target);
            self.blocking_jobs.remove(&target);
            self.atomics.remove(&target);
            self.parks.remove(&target);
            self.channels.remove(&target);
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
                self.blocking_pools.remove(&handle);
                self.blocking_jobs.remove(&handle);
                self.atomics.remove(&handle);
                self.parks.remove(&handle);
                self.channels.remove(&handle);
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
            Object::Group(group) => {
                for child in group.children.iter().copied() {
                    self.discard_owned_task(child.task, pending);
                }
            }
            Object::Select(selection) => {
                for arm in selection.arms.iter().copied().filter(|arm| arm.owned) {
                    self.discard_select_source_with_pending(arm, pending);
                }
            }
            Object::Atomic => {
                self.atomics.remove(&handle);
            }
            Object::Park => {
                self.parks.remove(&handle);
            }
            Object::SyncArray => {
                self.sync_arrays.remove(&handle);
                self.sync_collection_parks.remove(&handle);
            }
            Object::SyncMap => {
                self.sync_maps.remove(&handle);
                self.sync_collection_parks.remove(&handle);
            }
            Object::SyncSet => {
                self.sync_sets.remove(&handle);
                self.sync_collection_parks.remove(&handle);
            }
            Object::SyncStack => {
                self.sync_stacks.remove(&handle);
                self.sync_collection_parks.remove(&handle);
            }
            Object::SyncQueue => {
                self.sync_queues.remove(&handle);
                self.sync_collection_parks.remove(&handle);
            }
            Object::SyncCursor { .. } => {
                self.sync_cursors.remove(&handle);
            }
            Object::Channel => self.channel_destroy(handle, pending),
            Object::ChannelSender { channel, closed } => {
                self.channel_cleanup_endpoint(*channel, true, *closed, pending);
            }
            Object::ChannelReceiver { channel, closed } => {
                self.channel_cleanup_endpoint(*channel, false, *closed, pending);
            }
            Object::ChannelDrain { .. } => {}
            Object::Host { capability, .. } if *capability == HOST_CAP_BLOCKING_POOL => {
                if let Some(pool) = self.blocking_pools.remove(&handle) {
                    let _ = pool.shutdown(true);
                }
            }
            Object::Host { capability, .. } if *capability == HOST_CAP_BLOCKING_JOB => {
                self.cleanup_blocking_job(handle, pending);
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
                self.clear_task_group_owner(task);
                if kind == TaskKind::Thread
                    && let Some(signal) = self.thread_workers.get(&task)
                {
                    signal.cancel();
                }
                self.clear_runtime_root_into(task, pending);
                self.notify_selects(task);
                self.notify_groups(task);
            }
            TaskState::Ready => {
                self.release_task_value(task, pending);
                if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                    *state = TaskState::Joined;
                }
                self.clear_task_group_owner(task);
                self.notify_selects(task);
                self.notify_groups(task);
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
            if matches!(&object, Object::Atomic) {
                self.atomics.remove(&handle);
            }
            if matches!(&object, Object::Park) {
                self.parks.remove(&handle);
            }
            match &object {
                Object::SyncArray => {
                    self.sync_arrays.remove(&handle);
                    self.sync_collection_parks.remove(&handle);
                }
                Object::SyncMap => {
                    self.sync_maps.remove(&handle);
                    self.sync_collection_parks.remove(&handle);
                }
                Object::SyncSet => {
                    self.sync_sets.remove(&handle);
                    self.sync_collection_parks.remove(&handle);
                }
                Object::SyncStack => {
                    self.sync_stacks.remove(&handle);
                    self.sync_collection_parks.remove(&handle);
                }
                Object::SyncQueue => {
                    self.sync_queues.remove(&handle);
                    self.sync_collection_parks.remove(&handle);
                }
                Object::SyncCursor { .. } => {
                    self.sync_cursors.remove(&handle);
                }
                Object::Channel => self.channel_destroy(handle, &mut pending),
                Object::ChannelSender { channel, closed } => {
                    self.channel_cleanup_endpoint(*channel, true, *closed, &mut pending);
                }
                Object::ChannelReceiver { channel, closed } => {
                    self.channel_cleanup_endpoint(*channel, false, *closed, &mut pending);
                }
                Object::ChannelDrain { .. } => {}
                Object::Host { capability, .. } if *capability == HOST_CAP_BLOCKING_POOL => {
                    if let Some(pool) = self.blocking_pools.remove(&handle) {
                        let _ = pool.shutdown(true);
                    }
                }
                Object::Host { capability, .. } if *capability == HOST_CAP_BLOCKING_JOB => {
                    self.cleanup_blocking_job(handle, &mut pending);
                }
                _ => {}
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
                self.blocking_pools.remove(&handle);
                self.blocking_jobs.remove(&handle);
                self.channels.remove(&handle);
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
                panicked: false,
                group_owner: None,
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

    fn clear_task_group_owner(&mut self, task: u64) {
        if let Some(Object::Task { group_owner, .. }) = self.object_mut(task) {
            *group_owner = None;
        }
    }

    /// Removes a task from its lexical scope when its Join is transferred to
    /// a Group.  The group retains the task before this edge is released, so
    /// the transfer remains atomic even when the caller drops its handle.
    fn detach_task_from_scope(&mut self, task: u64, pending: &mut VecDeque<u64>) {
        let scope = self.objects.iter().find_map(|(handle, (_, object))| {
            matches!(object, Object::Scope { tasks, .. } if tasks.contains(&task))
                .then_some(*handle)
        });
        let Some(scope) = scope else {
            return;
        };
        let removed = self
            .object_mut(scope)
            .and_then(|object| match object {
                Object::Scope { tasks, .. } => tasks
                    .iter()
                    .position(|candidate| *candidate == task)
                    .map(|position| {
                        tasks.remove(position);
                    }),
                _ => None,
            })
            .is_some();
        if removed {
            self.release_strong_edge(task, pending);
        }
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

    /// A native thread has a physical worker barrier in addition to its
    /// logical task state.  Group consumers must cross that barrier before
    /// observing a child, otherwise `next` could return a value while the
    /// worker is still running.  The transition is idempotent for terminal
    /// worker snapshots and keeps the ordinary task notification path.
    fn sync_group_thread(&mut self, task: u64) {
        let Some(signal) = self.thread_worker_signal(task) else {
            return;
        };
        let snapshot = signal.wait();
        let mut pending = VecDeque::new();
        match snapshot.state {
            WorkerState::Completed => {
                let pending_task = matches!(
                    self.object(task),
                    Some(Object::Task {
                        state: TaskState::Pending,
                        ..
                    })
                );
                if pending_task {
                    if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                        *state = TaskState::Ready;
                    }
                    self.notify_selects(task);
                    self.notify_groups(task);
                }
                self.clear_runtime_root_into(task, &mut pending);
            }
            WorkerState::Cancelled => {
                let pending_task = matches!(
                    self.object(task),
                    Some(Object::Task {
                        state: TaskState::Pending,
                        ..
                    })
                );
                if pending_task {
                    self.release_task_value(task, &mut pending);
                    if let Some(Object::Task { state, .. }) = self.object_mut(task) {
                        *state = TaskState::Cancelled;
                    }
                    self.notify_selects(task);
                    self.notify_groups(task);
                }
                self.clear_runtime_root_into(task, &mut pending);
            }
            WorkerState::Starting | WorkerState::Running => {
                unreachable!("thread worker wait only returns terminal snapshots")
            }
        }
        self.drain_destruction(&mut pending);
    }

    fn sync_group_threads(&mut self, children: &[GroupChild]) {
        for child in children {
            self.sync_group_thread(child.task);
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

    fn blocking_pool_ref(&mut self, handle: u64) -> Result<Arc<NativeBlockingPool>, u64> {
        match self.object(handle) {
            Some(Object::Host { capability, .. }) if *capability == HOST_CAP_BLOCKING_POOL => self
                .blocking_pools
                .get(&handle)
                .cloned()
                .ok_or(STATUS_BLOCKING_INVALID_HANDLE),
            _ => Err(STATUS_BLOCKING_INVALID_HANDLE),
        }
    }

    fn blocking_job_ref(&mut self, handle: u64) -> Result<NativeBlockingJobRef, u64> {
        match self.object(handle) {
            Some(Object::Host { capability, .. }) if *capability == HOST_CAP_BLOCKING_JOB => self
                .blocking_jobs
                .get(&handle)
                .cloned()
                .ok_or(STATUS_BLOCKING_INVALID_HANDLE),
            _ => Err(STATUS_BLOCKING_INVALID_HANDLE),
        }
    }

    fn blocking_pool_new(&mut self, workers: i64, capacity: i64) -> u64 {
        if !native_blocking_supported() {
            self.status(STATUS_BLOCKING_UNSUPPORTED_TARGET);
            return 0;
        }
        let Ok(workers) = usize::try_from(workers) else {
            self.status(STATUS_BLOCKING_INVALID_WORKERS);
            return 0;
        };
        if workers == 0 || workers > 4096 {
            self.status(STATUS_BLOCKING_INVALID_WORKERS);
            return 0;
        }
        let Ok(capacity) = usize::try_from(capacity) else {
            self.status(STATUS_BLOCKING_INVALID_CAPACITY);
            return 0;
        };
        if capacity > 1_000_000 {
            self.status(STATUS_BLOCKING_INVALID_CAPACITY);
            return 0;
        }
        let pool = match NativeBlockingPool::new(workers, capacity) {
            Ok(pool) => pool,
            Err(status) => {
                self.status(status);
                return 0;
            }
        };
        let handle = self.alloc(
            Object::Host {
                capability: HOST_CAP_BLOCKING_POOL,
                state: HostState::Open,
                input: Vec::new(),
                cursor: 0,
                output: Vec::new(),
            },
            ObjectKind::Host,
        );
        if handle == 0 {
            drop(pool);
            return 0;
        }
        self.blocking_pools.insert(handle, pool);
        handle
    }

    fn blocking_pool_submit(&mut self, pool_handle: u64, payload: u64) -> u64 {
        if !native_blocking_supported() {
            self.status(STATUS_BLOCKING_UNSUPPORTED_TARGET);
            return 0;
        }
        let pool = match self.blocking_pool_ref(pool_handle) {
            Ok(pool) => pool,
            Err(status) => {
                self.status(status);
                return 0;
            }
        };
        if Self::valid_handle(payload) && !self.live_handle(payload) {
            self.status(STATUS_BLOCKING_INVALID_HANDLE);
            return 0;
        }
        match pool.can_admit() {
            Ok(true) => {}
            Ok(false) => {
                self.status(STATUS_BLOCKING_NOT_READY);
                return 0;
            }
            Err(status) => {
                self.status(status);
                return 0;
            }
        }
        if self.retain(pool_handle) != STATUS_OK {
            return 0;
        }
        if Self::valid_handle(payload) && self.retain(payload) != STATUS_OK {
            let _ = self.release(pool_handle);
            return 0;
        }
        let job = Arc::new(NativeBlockingJobCell {
            state: Mutex::new(NativeBlockingJob {
                payload,
                pool: pool_handle,
                state: NativeBlockingJobState::Queued,
                worker: u64::MAX,
            }),
            wake: Condvar::new(),
        });
        if !pool.submit(Arc::clone(&job)).unwrap_or(false) {
            let mut pending = VecDeque::new();
            if Self::valid_handle(payload) {
                self.release_strong_edge(payload, &mut pending);
            }
            self.release_strong_edge(pool_handle, &mut pending);
            self.drain_destruction(&mut pending);
            self.status(STATUS_BLOCKING_NOT_READY);
            return 0;
        }
        let handle = self.alloc(
            Object::Host {
                capability: HOST_CAP_BLOCKING_JOB,
                state: HostState::Open,
                input: Vec::new(),
                cursor: 0,
                output: Vec::new(),
            },
            ObjectKind::Host,
        );
        if handle == 0 {
            let _ = pool.cancel_job(&job);
            let mut pending = VecDeque::new();
            if Self::valid_handle(payload) {
                self.release_strong_edge(payload, &mut pending);
            }
            self.release_strong_edge(pool_handle, &mut pending);
            self.drain_destruction(&mut pending);
            return 0;
        }
        self.blocking_jobs.insert(handle, job);
        handle
    }

    fn blocking_pool_status(&mut self, handle: u64) -> u64 {
        let pool = match self.blocking_pool_ref(handle) {
            Ok(pool) => pool,
            Err(status) => return self.status(status),
        };
        let (lock, _) = &*pool.state;
        match lock.lock() {
            Ok(state) => state.lifecycle.code(),
            Err(_) => self.status(STATUS_BLOCKING_INVALID_TRANSITION),
        }
    }

    fn blocking_pool_shutdown(&mut self, handle: u64, cancel: bool) -> u64 {
        if !native_blocking_supported() {
            return self.status(STATUS_BLOCKING_UNSUPPORTED_TARGET);
        }
        let pool = match self.blocking_pool_ref(handle) {
            Ok(pool) => pool,
            Err(status) => return self.status(status),
        };
        self.status(pool.shutdown(cancel))
    }

    fn blocking_job_status(&mut self, handle: u64) -> u64 {
        let job = match self.blocking_job_ref(handle) {
            Ok(job) => job,
            Err(status) => return self.status(status),
        };
        match job.state.lock() {
            Ok(state) => state.state.code(),
            Err(_) => self.status(STATUS_BLOCKING_INVALID_TRANSITION),
        }
    }

    fn blocking_job_worker(&mut self, handle: u64) -> u64 {
        let job = match self.blocking_job_ref(handle) {
            Ok(job) => job,
            Err(status) => return self.status(status),
        };
        match job.state.lock() {
            Ok(state) => state.worker,
            Err(_) => self.status(STATUS_BLOCKING_INVALID_TRANSITION),
        }
    }

    fn blocking_job_wait(&mut self, handle: u64) -> u64 {
        let job = match self.blocking_job_ref(handle) {
            Ok(job) => job,
            Err(status) => return self.status(status),
        };
        let (state_lock, wake) = (&job.state, &job.wake);
        let mut state = match state_lock.lock() {
            Ok(state) => state,
            Err(_) => return self.status(STATUS_BLOCKING_INVALID_TRANSITION),
        };
        while !state.state.terminal() {
            state = match wake.wait(state) {
                Ok(state) => state,
                Err(_) => return self.status(STATUS_BLOCKING_INVALID_TRANSITION),
            };
        }
        match state.state {
            NativeBlockingJobState::Cancelled => self.status(STATUS_CANCELLED),
            NativeBlockingJobState::Completed | NativeBlockingJobState::Taken => STATUS_OK,
            NativeBlockingJobState::Queued | NativeBlockingJobState::Running => {
                self.status(STATUS_BLOCKING_NOT_READY)
            }
        }
    }

    fn blocking_job_take(&mut self, handle: u64) -> u64 {
        let job = match self.blocking_job_ref(handle) {
            Ok(job) => job,
            Err(status) => {
                self.status(status);
                return 0;
            }
        };
        let mut state = match job.state.lock() {
            Ok(state) => state,
            Err(_) => {
                self.status(STATUS_BLOCKING_INVALID_TRANSITION);
                return 0;
            }
        };
        match state.state {
            NativeBlockingJobState::Completed => {
                let payload = state.payload;
                if Self::valid_handle(payload) && self.retain(payload) != STATUS_OK {
                    return 0;
                }
                state.state = NativeBlockingJobState::Taken;
                job.wake.notify_all();
                payload
            }
            NativeBlockingJobState::Cancelled => {
                self.status(STATUS_CANCELLED);
                0
            }
            NativeBlockingJobState::Queued | NativeBlockingJobState::Running => {
                self.status(STATUS_BLOCKING_NOT_READY);
                0
            }
            NativeBlockingJobState::Taken => {
                self.status(STATUS_BLOCKING_INVALID_TRANSITION);
                0
            }
        }
    }

    fn blocking_job_cancel(&mut self, handle: u64) -> u64 {
        let job = match self.blocking_job_ref(handle) {
            Ok(job) => job,
            Err(status) => return self.status(status),
        };
        let (state, pool_handle) = match job.state.lock() {
            Ok(state) => (state.state, state.pool),
            Err(_) => return self.status(STATUS_BLOCKING_INVALID_TRANSITION),
        };
        let status = match state {
            NativeBlockingJobState::Queued | NativeBlockingJobState::Running => {
                let pool = match self.blocking_pool_ref(pool_handle) {
                    Ok(pool) => pool,
                    Err(status) => return self.status(status),
                };
                pool.cancel_job(&job)
            }
            NativeBlockingJobState::Cancelled
            | NativeBlockingJobState::Completed
            | NativeBlockingJobState::Taken => STATUS_BLOCKING_INVALID_TRANSITION,
        };
        self.status(status)
    }

    fn cleanup_blocking_job(&mut self, handle: u64, pending: &mut VecDeque<u64>) {
        let Some(job) = self.blocking_jobs.remove(&handle) else {
            return;
        };
        let (payload, pool) = match job.state.lock() {
            Ok(state) => (state.payload, state.pool),
            Err(_) => return,
        };
        if Self::valid_handle(payload) && self.live_handle(payload) {
            self.release_strong_edge(payload, pending);
        }
        if self.live_handle(pool) {
            self.release_strong_edge(pool, pending);
        }
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
        self.notify_groups(task);
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
            let Some(Object::Task {
                state,
                value,
                group_owner,
                ..
            }) = self.object_mut(task)
            else {
                return 0;
            };
            if group_owner.is_some() {
                self.last_status = STATUS_INVALID_TRANSITION;
                return 0;
            }
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
        self.notify_groups(task);
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    /// Publishes a terminal panic marker without confusing it with the
    /// declared `Result` error channel.  The marker is consumed by a group
    /// terminal operation, which drains siblings before propagating
    /// `STATUS_PANICKED`.
    fn task_panic(&mut self, task: u64) -> u64 {
        let Some(Object::Task {
            state, panicked, ..
        }) = self.object_mut(task)
        else {
            return STATUS_INVALID_HANDLE;
        };
        if *state != TaskState::Pending {
            return STATUS_INVALID_TRANSITION;
        }
        *panicked = true;
        *state = TaskState::Ready;
        self.notify_selects(task);
        self.notify_groups(task);
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
        self.notify_groups(task);
        if kind == TaskKind::Thread {
            self.clear_runtime_root(task);
        }
        self.drain_destruction(&mut pending);
        STATUS_OK
    }

    fn group_new(&mut self) -> u64 {
        self.alloc(
            Object::Group(GroupData {
                children: Vec::new(),
                completion_queue: VecDeque::new(),
                outcomes: Vec::new(),
                phase: GroupPhase::Open,
                last_index: None,
                last_value: 0,
                cleanup_runs: 0,
            }),
            ObjectKind::Group,
        )
    }

    fn group_add(&mut self, group: u64, task: u64) -> u64 {
        let phase = match self.object(group) {
            Some(Object::Group(state)) => state.phase,
            Some(_) => {
                self.last_status = STATUS_INVALID_HANDLE;
                return STATUS_INVALID_HANDLE;
            }
            None => {
                self.last_status = STATUS_INVALID_HANDLE;
                return STATUS_INVALID_HANDLE;
            }
        };
        if !matches!(phase, GroupPhase::Open | GroupPhase::Waiting) {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        let child_count = match self.object(group) {
            Some(Object::Group(state)) => state.children.len(),
            _ => return self.status(STATUS_INVALID_HANDLE),
        };
        if child_count >= usize::try_from(MAX_GROUP_CHILDREN).unwrap_or(usize::MAX) {
            return self.status(STATUS_COUNT_OVERFLOW);
        }
        let already_added = match self.object(group) {
            Some(Object::Group(state)) => state.children.iter().any(|child| child.task == task),
            _ => return self.status(STATUS_INVALID_HANDLE),
        };
        if already_added {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        let (task_state, group_owner) = match self.object(task) {
            Some(Object::Task {
                state, group_owner, ..
            }) => (*state, *group_owner),
            _ => return self.status(STATUS_INVALID_HANDLE),
        };
        let registered_in_select = self.objects.values().any(|(_, object)| {
            matches!(
                object,
                Object::Select(selection)
                    if selection.arms.iter().any(|arm| arm.source == task)
            )
        });
        if task_state == TaskState::Joined || group_owner.is_some() || registered_in_select {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        if self.retain(task) != STATUS_OK {
            return self.last_status;
        }
        let mut pending = VecDeque::new();
        self.detach_task_from_scope(task, &mut pending);
        if let Some(Object::Task { group_owner, .. }) = self.object_mut(task) {
            *group_owner = Some(group);
        }
        self.drain_destruction(&mut pending);
        let index = child_count as u64;
        let terminal = matches!(task_state, TaskState::Ready | TaskState::Cancelled);
        let Some(Object::Group(state)) = self.object_mut(group) else {
            self.clear_task_group_owner(task);
            let _ = self.release(task);
            return self.status(STATUS_INVALID_HANDLE);
        };
        state.children.push(GroupChild {
            task,
            index,
            queued: terminal,
        });
        if terminal {
            state.completion_queue.push_back(index);
        }
        STATUS_OK
    }

    fn group_task_snapshot(&self, task: u64) -> Option<(TaskState, bool, u64)> {
        match self.object(task) {
            Some(Object::Task {
                state,
                panicked,
                value,
                ..
            }) => Some((*state, *panicked, *value)),
            _ => None,
        }
    }

    fn group_value_is_error(&self, value: u64) -> bool {
        value == RESULT_ERR
            || matches!(
                self.object(value),
                Some(Object::Result {
                    tag: RESULT_ERR,
                    ..
                })
            )
    }

    fn group_take_task(&mut self, task: u64) -> Option<(u64, bool)> {
        let Some(Object::Task {
            state,
            value,
            panicked,
            ..
        }) = self.object_mut(task)
        else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        match state {
            TaskState::Ready => {
                *state = TaskState::Joined;
                Some((std::mem::take(value), *panicked))
            }
            TaskState::Cancelled => {
                self.last_status = STATUS_CANCELLED;
                None
            }
            _ => {
                self.last_status = STATUS_NOT_READY;
                None
            }
        }
    }

    fn group_remove_child(&mut self, group: u64, index: u64, pending: &mut VecDeque<u64>) {
        let task = {
            let Some(Object::Group(state)) = self.object_mut(group) else {
                return;
            };
            let Some(position) = state.children.iter().position(|child| child.index == index)
            else {
                return;
            };
            let child = state.children.remove(position);
            state.completion_queue.retain(|queued| *queued != index);
            child.task
        };
        self.clear_task_group_owner(task);
        self.release_strong_edge(task, pending);
    }

    fn group_snapshot(&self, group: u64) -> Option<GroupData> {
        match self.object(group) {
            Some(Object::Group(state)) => Some(state.clone()),
            _ => None,
        }
    }

    fn group_record_outcomes(&mut self, group: u64, children: &[GroupChild]) {
        let outcomes = children
            .iter()
            .filter_map(|child| {
                let (state, panicked, value) = self.group_task_snapshot(child.task)?;
                if state != TaskState::Ready || panicked {
                    return None;
                }
                let (is_error, value) = match self.object(value) {
                    Some(Object::Result { tag, payload }) if *tag == RESULT_ERR => {
                        (true, payload.as_ref().copied().unwrap_or(0))
                    }
                    Some(Object::Result { tag, payload }) if *tag == RESULT_OK => {
                        (false, payload.as_ref().copied().unwrap_or(0))
                    }
                    _ if value == RESULT_ERR => (true, value),
                    _ => (false, value),
                };
                Some(GroupOutcomeRecord {
                    index: child.index,
                    value,
                    is_error,
                })
            })
            .collect::<Vec<_>>();
        if let Some(Object::Group(state)) = self.object_mut(group) {
            state.outcomes = outcomes;
        }
    }

    fn group_outcome_record(&mut self, group: u64, position: u64) -> Option<GroupOutcomeRecord> {
        let Ok(position) = usize::try_from(position) else {
            self.last_status = STATUS_INVALID_TRANSITION;
            return None;
        };
        match self.object(group) {
            Some(Object::Group(state)) => state.outcomes.get(position).copied().or_else(|| {
                self.last_status = STATUS_INVALID_TRANSITION;
                None
            }),
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                None
            }
        }
    }

    fn group_cancel_pending(&mut self, children: &[GroupChild]) {
        for child in children {
            if matches!(
                self.group_task_snapshot(child.task),
                Some((TaskState::Pending, ..))
            ) {
                let _ = self.task_cancel(child.task);
            }
        }
    }

    fn group_has_invalid_child(&self, children: &[GroupChild]) -> bool {
        children.iter().any(|child| {
            !matches!(
                self.group_task_snapshot(child.task),
                Some((
                    TaskState::Pending | TaskState::Ready | TaskState::Cancelled,
                    ..
                ))
            )
        })
    }

    fn group_abort_invalid_child(&mut self, group: u64) -> u64 {
        let snapshot = self.group_snapshot(group).unwrap_or_default();
        self.group_cancel_pending(&snapshot.children);
        let snapshot = self.group_snapshot(group).unwrap_or_default();
        let mut pending = VecDeque::new();
        self.group_finish(group, &snapshot.children, None, &mut pending);
        self.drain_destruction(&mut pending);
        self.last_status = STATUS_INVALID_TRANSITION;
        STATUS_INVALID_TRANSITION
    }

    fn group_finish(
        &mut self,
        group: u64,
        children: &[GroupChild],
        keep_index: Option<u64>,
        pending: &mut VecDeque<u64>,
    ) {
        let previous_last_value = match self.object_mut(group) {
            Some(Object::Group(state)) => std::mem::take(&mut state.last_value),
            _ => 0,
        };
        if self.live_handle(previous_last_value) {
            self.release_strong_edge(previous_last_value, pending);
        }
        let mut kept_value = 0;
        let mut kept_index = None;
        for child in children {
            let taken = self.group_take_task(child.task);
            if let Some((value, _panicked)) = taken {
                if keep_index == Some(child.index) {
                    kept_value = value;
                    kept_index = Some(child.index);
                } else if self.live_handle(value) {
                    self.release_strong_edge(value, pending);
                }
            }
            self.group_remove_child(group, child.index, pending);
        }
        if let Some(Object::Group(state)) = self.object_mut(group) {
            state.completion_queue.clear();
            state.phase = GroupPhase::Consumed;
            state.last_index = kept_index;
            state.last_value = kept_value;
            state.cleanup_runs = state.cleanup_runs.saturating_add(children.len() as u64);
        }
    }

    fn group_next(&mut self, group: u64) -> u64 {
        let Some(snapshot) = self.group_snapshot(group) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        };
        self.sync_group_threads(&snapshot.children);
        let Some(snapshot) = self.group_snapshot(group) else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        };
        if snapshot.phase == GroupPhase::Consumed {
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        let mut pending = VecDeque::new();
        loop {
            let Some(index) = self.object_mut(group).and_then(|object| match object {
                Object::Group(state) => state.completion_queue.pop_front(),
                _ => None,
            }) else {
                let has_pending = self.group_snapshot(group).is_some_and(|state| {
                    state.children.iter().any(|child| {
                        matches!(
                            self.group_task_snapshot(child.task),
                            Some((TaskState::Pending, ..))
                        )
                    })
                });
                if let Some(Object::Group(state)) = self.object_mut(group) {
                    state.phase = if has_pending {
                        GroupPhase::Waiting
                    } else {
                        GroupPhase::ReadyToConsume
                    };
                    state.last_index = None;
                    state.last_value = 0;
                }
                self.last_status = if has_pending {
                    STATUS_NOT_READY
                } else {
                    STATUS_OK
                };
                return 0;
            };
            let Some(child) = self.group_snapshot(group).and_then(|state| {
                state
                    .children
                    .into_iter()
                    .find(|child| child.index == index)
            }) else {
                continue;
            };
            let Some((task_state, panicked, _)) = self.group_task_snapshot(child.task) else {
                let _ = self.group_abort_invalid_child(group);
                return 0;
            };
            if task_state == TaskState::Joined {
                let _ = self.group_abort_invalid_child(group);
                return 0;
            }
            if task_state == TaskState::Cancelled {
                self.group_remove_child(group, index, &mut pending);
                continue;
            }
            if panicked {
                let children = self.group_snapshot(group).unwrap_or_default().children;
                self.group_cancel_pending(&children);
                let children = self.group_snapshot(group).unwrap_or_default().children;
                self.group_finish(group, &children, None, &mut pending);
                self.drain_destruction(&mut pending);
                self.last_status = STATUS_PANICKED;
                return 0;
            }
            let Some((value, _)) = self.group_take_task(child.task) else {
                continue;
            };
            self.group_remove_child(group, index, &mut pending);
            if let Some(Object::Group(state)) = self.object_mut(group) {
                state.phase = GroupPhase::ReadyToConsume;
                state.last_index = Some(index);
                // `next` transfers the task payload directly to its caller;
                // unlike `all`, it must not leave a second Group-owned edge.
                state.last_value = 0;
            }
            self.drain_destruction(&mut pending);
            self.last_status = STATUS_OK;
            return value;
        }
    }

    fn group_all(&mut self, group: u64) -> u64 {
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        self.sync_group_threads(&snapshot.children);
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if snapshot.phase == GroupPhase::Consumed {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        let has_failure = snapshot.children.iter().any(|child| {
            self.group_task_snapshot(child.task)
                .is_some_and(|(state, panicked, value)| {
                    state == TaskState::Ready && (panicked || self.group_value_is_error(value))
                })
        });
        if has_failure {
            self.group_cancel_pending(&snapshot.children);
        }
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if self.group_has_invalid_child(&snapshot.children) {
            return self.group_abort_invalid_child(group);
        }
        if snapshot.children.iter().any(|child| {
            matches!(
                self.group_task_snapshot(child.task),
                Some((TaskState::Pending, ..))
            )
        }) {
            if let Some(Object::Group(state)) = self.object_mut(group) {
                state.phase = GroupPhase::Waiting;
            }
            return self.status(STATUS_NOT_READY);
        }
        self.group_record_outcomes(group, &snapshot.children);
        let mut first_error = None;
        let mut first_error_index = None;
        let mut panicked = false;
        for child in &snapshot.children {
            let Some((state, child_panicked, value)) = self.group_task_snapshot(child.task) else {
                continue;
            };
            if state == TaskState::Ready && child_panicked {
                panicked = true;
            } else if state == TaskState::Ready
                && first_error.is_none()
                && self.group_value_is_error(value)
            {
                first_error = Some(value);
                first_error_index = Some(child.index);
            }
        }
        let mut pending = VecDeque::new();
        self.group_finish(
            group,
            &snapshot.children,
            if panicked { None } else { first_error_index },
            &mut pending,
        );
        self.drain_destruction(&mut pending);
        if panicked {
            self.last_status = STATUS_PANICKED;
            STATUS_PANICKED
        } else if first_error.is_some() {
            self.last_status = RESULT_ERR;
            RESULT_ERR
        } else {
            STATUS_OK
        }
    }

    fn group_settle(&mut self, group: u64) -> u64 {
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        self.sync_group_threads(&snapshot.children);
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if snapshot.phase == GroupPhase::Consumed {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        let has_panic = snapshot.children.iter().any(|child| {
            self.group_task_snapshot(child.task)
                .is_some_and(|(state, panicked, _)| state == TaskState::Ready && panicked)
        });
        if has_panic {
            self.group_cancel_pending(&snapshot.children);
        }
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if self.group_has_invalid_child(&snapshot.children) {
            return self.group_abort_invalid_child(group);
        }
        if snapshot.children.iter().any(|child| {
            matches!(
                self.group_task_snapshot(child.task),
                Some((TaskState::Pending, ..))
            )
        }) {
            if let Some(Object::Group(state)) = self.object_mut(group) {
                state.phase = GroupPhase::Waiting;
            }
            return self.status(STATUS_NOT_READY);
        }
        self.group_record_outcomes(group, &snapshot.children);
        let mut pending = VecDeque::new();
        self.group_finish(group, &snapshot.children, None, &mut pending);
        self.drain_destruction(&mut pending);
        if has_panic {
            self.last_status = STATUS_PANICKED;
            STATUS_PANICKED
        } else {
            STATUS_OK
        }
    }

    fn group_cancel(&mut self, group: u64) -> u64 {
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        self.sync_group_threads(&snapshot.children);
        let Some(snapshot) = self.group_snapshot(group) else {
            return self.status(STATUS_INVALID_HANDLE);
        };
        if snapshot.phase == GroupPhase::Consumed {
            return self.status(STATUS_INVALID_TRANSITION);
        }
        if self.group_has_invalid_child(&snapshot.children) {
            return self.group_abort_invalid_child(group);
        }
        self.group_cancel_pending(&snapshot.children);
        let snapshot = self.group_snapshot(group).unwrap_or_default();
        let mut pending = VecDeque::new();
        self.group_finish(group, &snapshot.children, None, &mut pending);
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

    fn host_result(&mut self, tag: u64, payload: Option<u64>) -> u64 {
        self.alloc(Object::Result { tag, payload }, ObjectKind::Result)
    }

    fn host_error(&mut self, status: u64) -> u64 {
        self.last_status = status;
        self.host_result(RESULT_ERR, Some(status))
    }

    fn host_open(&mut self, capability: u64) -> u64 {
        let input = match capability {
            // The bootstrap host uses deterministic fixtures.  A production
            // provider injects the bytes through the same capability boundary;
            // no ambient filesystem, process or clock is consulted here.
            HOST_CAP_CONSOLE => Vec::new(),
            HOST_CAP_FILESYSTEM => b"tondo-native-filesystem\n".to_vec(),
            HOST_CAP_PROCESS => b"tondo-native-process\n".to_vec(),
            HOST_CAP_CLOCK => Vec::new(),
            _ => {
                self.last_status = STATUS_HOST_UNSUPPORTED;
                return 0;
            }
        };
        let handle = self.alloc(
            Object::Host {
                capability,
                state: HostState::Open,
                input,
                cursor: 0,
                output: Vec::new(),
            },
            ObjectKind::Host,
        );
        if handle != 0
            && let Some(capture) = self.diagnostic.as_mut()
        {
            capture.resources_acquired = capture.resources_acquired.saturating_add(1);
        }
        handle
    }

    fn host_read(&mut self, handle: u64, max_bytes: u64) -> u64 {
        let Ok(limit) = usize::try_from(max_bytes) else {
            return self.host_error(STATUS_HOST_LIMIT);
        };
        if limit > HOST_MAX_BYTES {
            return self.host_error(STATUS_HOST_LIMIT);
        }
        let (chunk, status) = match self.object_mut(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                (None, STATUS_BLOCKING_INVALID_HANDLE)
            }
            Some(Object::Host {
                state,
                input,
                cursor,
                ..
            }) => match state {
                HostState::Open => {
                    let end = cursor.saturating_add(limit).min(input.len());
                    let chunk = input[*cursor..end].to_vec();
                    *cursor = end;
                    (Some(chunk), STATUS_OK)
                }
                HostState::Cancelled => (None, STATUS_HOST_CANCELLED),
                HostState::Closed => (None, STATUS_HOST_CLOSED),
            },
            Some(_) => (None, STATUS_INVALID_HANDLE),
            None => (None, STATUS_INVALID_HANDLE),
        };
        if status != STATUS_OK {
            return self.host_error(status);
        }
        let buffer = self.alloc(
            Object::Buffer {
                bytes: chunk.expect("successful host read always returns a buffer"),
            },
            ObjectKind::Buffer,
        );
        if buffer == 0 {
            return self.host_error(STATUS_HOST_LIMIT);
        }
        let result = self.host_result(RESULT_OK, Some(buffer));
        // The result carrier owns the returned buffer.  Transfer the initial
        // allocation owner so callers can retain the payload explicitly when
        // they outlive the carrier.
        let _ = self.release(buffer);
        result
    }

    fn host_write(&mut self, handle: u64, buffer: u64) -> u64 {
        let bytes = match self.object(buffer) {
            Some(Object::Buffer { bytes }) => bytes.clone(),
            Some(_) => return self.host_error(STATUS_INVALID_HANDLE),
            None => return self.host_error(STATUS_INVALID_HANDLE),
        };
        let status = match self.object_mut(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                STATUS_BLOCKING_INVALID_HANDLE
            }
            Some(Object::Host { state, output, .. }) => match state {
                HostState::Open => {
                    if output.len().saturating_add(bytes.len()) > HOST_MAX_BYTES {
                        STATUS_HOST_LIMIT
                    } else {
                        output.extend_from_slice(&bytes);
                        STATUS_OK
                    }
                }
                HostState::Cancelled => STATUS_HOST_CANCELLED,
                HostState::Closed => STATUS_HOST_CLOSED,
            },
            Some(_) | None => STATUS_INVALID_HANDLE,
        };
        if status != STATUS_OK {
            return self.host_error(status);
        }
        self.host_result(RESULT_OK, Some(bytes.len() as u64))
    }

    fn host_output(&mut self, handle: u64) -> u64 {
        let bytes = match self.object(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                return self.host_error(STATUS_BLOCKING_INVALID_HANDLE);
            }
            Some(Object::Host { output, .. }) => output.clone(),
            Some(_) | None => return self.host_error(STATUS_INVALID_HANDLE),
        };
        let buffer = self.alloc(Object::Buffer { bytes }, ObjectKind::Buffer);
        if buffer == 0 {
            return self.host_error(STATUS_HOST_LIMIT);
        }
        let result = self.host_result(RESULT_OK, Some(buffer));
        let _ = self.release(buffer);
        result
    }

    fn host_cancel(&mut self, handle: u64) -> u64 {
        let status = match self.object_mut(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                STATUS_BLOCKING_INVALID_HANDLE
            }
            Some(Object::Host { state, .. }) => match state {
                HostState::Open => {
                    *state = HostState::Cancelled;
                    STATUS_OK
                }
                HostState::Cancelled | HostState::Closed => STATUS_INVALID_TRANSITION,
            },
            Some(_) | None => STATUS_INVALID_HANDLE,
        };
        self.status(status)
    }

    fn host_close(&mut self, handle: u64) -> u64 {
        let status = match self.object_mut(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                STATUS_BLOCKING_INVALID_HANDLE
            }
            Some(Object::Host {
                state,
                input,
                output,
                ..
            }) => match state {
                HostState::Open | HostState::Cancelled => {
                    *state = HostState::Closed;
                    input.clear();
                    output.clear();
                    STATUS_OK
                }
                HostState::Closed => STATUS_INVALID_TRANSITION,
            },
            Some(_) | None => STATUS_INVALID_HANDLE,
        };
        if status == STATUS_OK
            && let Some(capture) = self.diagnostic.as_mut()
        {
            capture.resources_released = capture.resources_released.saturating_add(1);
        }
        self.status(status)
    }

    fn host_state(&mut self, handle: u64) -> u64 {
        match self.object(handle) {
            Some(Object::Host { capability, .. }) if *capability >= HOST_CAP_BLOCKING_POOL => {
                self.last_status = STATUS_BLOCKING_INVALID_HANDLE;
                u64::MAX
            }
            Some(Object::Host { state, .. }) => match state {
                HostState::Open => 0,
                HostState::Cancelled => 1,
                HostState::Closed => 2,
            },
            Some(_) | None => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn buffer_from_byte(&mut self, value: u64) -> u64 {
        let Ok(value) = u8::try_from(value) else {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        };
        self.alloc(Object::Buffer { bytes: vec![value] }, ObjectKind::Buffer)
    }

    fn buffer_len(&mut self, handle: u64) -> u64 {
        match self.object(handle) {
            Some(Object::Buffer { bytes }) => bytes.len() as u64,
            Some(_) | None => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn buffer_byte(&mut self, handle: u64, index: u64) -> u64 {
        let Ok(index) = usize::try_from(index) else {
            self.last_status = STATUS_INVALID_TRANSITION;
            return u64::MAX;
        };
        match self.object(handle) {
            Some(Object::Buffer { bytes }) => bytes.get(index).copied().map_or_else(
                || {
                    self.last_status = STATUS_INVALID_TRANSITION;
                    u64::MAX
                },
                u64::from,
            ),
            Some(_) | None => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn atomic_new(&mut self, initial: u64) -> u64 {
        let handle = self.alloc(Object::Atomic, ObjectKind::Atomic);
        if handle != 0 {
            self.atomics
                .insert(handle, Arc::new(std::sync::atomic::AtomicU64::new(initial)));
        }
        handle
    }

    fn atomic_cell(&mut self, handle: u64) -> Option<Arc<std::sync::atomic::AtomicU64>> {
        if !matches!(self.object(handle), Some(Object::Atomic)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.atomics.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn park_new(&mut self) -> u64 {
        let handle = self.alloc(Object::Park, ObjectKind::Park);
        if handle != 0 {
            self.parks.insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn park_signal(&mut self, handle: u64) -> Option<Arc<ParkingSignal>> {
        if !matches!(self.object(handle), Some(Object::Park)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(signal) = self.parks.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(signal)
    }

    fn sync_array_cell(&mut self, handle: u64) -> Option<SyncArrayCell> {
        if !matches!(self.object(handle), Some(Object::SyncArray)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.sync_arrays.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn sync_map_cell(&mut self, handle: u64) -> Option<SyncMapCell> {
        if !matches!(self.object(handle), Some(Object::SyncMap)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.sync_maps.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn sync_set_cell(&mut self, handle: u64) -> Option<SyncSetCell> {
        if !matches!(self.object(handle), Some(Object::SyncSet)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.sync_sets.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn sync_stack_cell(&mut self, handle: u64) -> Option<SyncStackCell> {
        if !matches!(self.object(handle), Some(Object::SyncStack)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.sync_stacks.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn sync_queue_cell(&mut self, handle: u64) -> Option<SyncQueueCell> {
        if !matches!(self.object(handle), Some(Object::SyncQueue)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.sync_queues.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn channel_payload_retain(&mut self, value: u64) -> u64 {
        if !Self::valid_handle(value) {
            return STATUS_OK;
        }
        if !self.live_handle(value) {
            return self.status(STATUS_INVALID_HANDLE);
        }
        self.retain(value)
    }

    fn channel_payload_release(&mut self, value: u64, pending: &mut VecDeque<u64>) {
        if self.live_handle(value) {
            self.release_strong_edge(value, pending);
        }
    }

    fn channel_cell(&mut self, handle: u64) -> Option<NativeChannelCell> {
        if !matches!(self.object(handle), Some(Object::Channel)) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(cell) = self.channels.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(cell)
    }

    fn channel_endpoint_cell(
        &mut self,
        endpoint: u64,
        sender: bool,
    ) -> Option<(u64, NativeChannelCell)> {
        let channel = match self.object(endpoint) {
            Some(Object::ChannelSender { channel, closed }) if sender && !closed => *channel,
            Some(Object::ChannelReceiver { channel, closed }) if !sender && !closed => *channel,
            Some(Object::ChannelSender { .. }) | Some(Object::ChannelReceiver { .. }) => {
                self.last_status = STATUS_INVALID_TRANSITION;
                return None;
            }
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                return None;
            }
        };
        let Some(cell) = self.channels.get(&channel).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some((channel, cell))
    }

    fn channel_new(&mut self, capacity: Option<usize>) -> u64 {
        if capacity.is_some_and(|capacity| capacity > HOST_MAX_BYTES) {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        }
        let handle = self.alloc(Object::Channel, ObjectKind::Channel);
        if handle != 0 {
            self.channels.insert(
                handle,
                Arc::new((
                    Mutex::new(NativeChannelState::new(capacity)),
                    Condvar::new(),
                )),
            );
        }
        handle
    }

    fn channel_endpoint(&mut self, channel: u64, sender: bool) -> u64 {
        let Some(cell) = self.channel_cell(channel) else {
            return 0;
        };
        {
            let (lock, _) = &*cell;
            let mut state = lock.lock().expect("native channel state is not poisoned");
            if sender {
                if state.sender_closed || state.senders == u64::MAX {
                    self.last_status = STATUS_HOST_CLOSED;
                    return 0;
                }
                state.senders += 1;
            } else {
                if state.receiver_closed || state.receivers == u64::MAX {
                    self.last_status = STATUS_HOST_CLOSED;
                    return 0;
                }
                state.receivers += 1;
            }
        }
        let object = if sender {
            Object::ChannelSender {
                channel,
                closed: false,
            }
        } else {
            Object::ChannelReceiver {
                channel,
                closed: false,
            }
        };
        let kind = if sender {
            ObjectKind::ChannelSender
        } else {
            ObjectKind::ChannelReceiver
        };
        let endpoint = self.alloc(object, kind);
        if endpoint == 0 {
            let (lock, wake) = &*cell;
            let mut state = lock.lock().expect("native channel state is not poisoned");
            if sender {
                state.senders = state.senders.saturating_sub(1);
            } else {
                state.receivers = state.receivers.saturating_sub(1);
            }
            wake.notify_all();
        }
        endpoint
    }

    fn channel_close_sender(&mut self, endpoint: u64) -> u64 {
        let Some((channel, cell)) = self.channel_endpoint_cell(endpoint, true) else {
            return self.last_status;
        };
        let (lock, wake) = &*cell;
        {
            let mut state = lock.lock().expect("native channel state is not poisoned");
            state.senders = state.senders.saturating_sub(1);
            if state.senders == 0 {
                state.sender_closed = true;
            }
            wake.notify_all();
        }
        if let Some(Object::ChannelSender { closed, .. }) = self.object_mut(endpoint) {
            *closed = true;
        } else {
            self.last_status = STATUS_INVALID_HANDLE;
            return STATUS_INVALID_HANDLE;
        }
        let _ = channel;
        STATUS_OK
    }

    fn channel_close_receiver(&mut self, endpoint: u64) -> u64 {
        let Some((channel, cell)) = self.channel_endpoint_cell(endpoint, false) else {
            return self.last_status;
        };
        let values = {
            let (lock, wake) = &*cell;
            let mut state = lock.lock().expect("native channel state is not poisoned");
            state.receivers = state.receivers.saturating_sub(1);
            let last = state.receivers == 0;
            if last {
                state.receiver_closed = true;
                let waiters = state.send_waiters.drain(..).collect::<Vec<_>>();
                for waiter in waiters {
                    state
                        .send_results
                        .insert(waiter.id, NativeChannelSendOutcome::Closed);
                }
            }
            let values = if last {
                state.queue.drain(..).collect::<VecDeque<_>>()
            } else {
                VecDeque::new()
            };
            wake.notify_all();
            values
        };
        if let Some(Object::ChannelReceiver { closed, .. }) = self.object_mut(endpoint) {
            *closed = true;
        } else {
            self.last_status = STATUS_INVALID_HANDLE;
            return 0;
        }
        self.channel_drain_new(channel, values)
    }

    fn channel_drain_new(&mut self, _channel: u64, values: VecDeque<u64>) -> u64 {
        let retained_values = values.iter().copied().collect::<Vec<_>>();
        let drain = self.alloc(Object::ChannelDrain { values }, ObjectKind::ChannelDrain);
        let mut pending = VecDeque::new();
        for value in retained_values {
            self.channel_payload_release(value, &mut pending);
        }
        self.drain_destruction(&mut pending);
        if drain == 0 {
            return 0;
        }
        drain
    }

    fn channel_cleanup_endpoint(
        &mut self,
        channel: u64,
        sender: bool,
        closed: bool,
        pending: &mut VecDeque<u64>,
    ) {
        if closed {
            return;
        }
        let Some(cell) = self.channels.get(&channel).cloned() else {
            return;
        };
        let (drained, waiting) = {
            let (lock, wake) = &*cell;
            let mut state = lock.lock().expect("native channel state is not poisoned");
            if sender {
                state.senders = state.senders.saturating_sub(1);
                if state.senders == 0 {
                    state.sender_closed = true;
                }
                wake.notify_all();
                (VecDeque::new(), Vec::new())
            } else {
                state.receivers = state.receivers.saturating_sub(1);
                if state.receivers != 0 {
                    wake.notify_all();
                    (VecDeque::new(), Vec::new())
                } else {
                    state.receiver_closed = true;
                    let waiting = state.send_waiters.drain(..).collect::<Vec<_>>();
                    let drained = state.queue.drain(..).collect::<VecDeque<_>>();
                    for waiter in &waiting {
                        state
                            .send_results
                            .insert(waiter.id, NativeChannelSendOutcome::Closed);
                    }
                    self.last_status = if drained.is_empty() {
                        self.last_status
                    } else {
                        STATUS_INVALID_TRANSITION
                    };
                    wake.notify_all();
                    (
                        drained,
                        waiting.into_iter().map(|waiter| waiter.value).collect(),
                    )
                }
            }
        };
        for value in drained {
            self.channel_payload_release(value, pending);
        }
        for value in waiting {
            self.channel_payload_release(value, pending);
        }
    }

    fn channel_destroy(&mut self, handle: u64, pending: &mut VecDeque<u64>) {
        let Some(cell) = self.channels.remove(&handle) else {
            return;
        };
        let (queued, waiting) = {
            let (lock, wake) = &*cell;
            let mut state = lock.lock().expect("native channel state is not poisoned");
            let queued = state.queue.drain(..).collect::<Vec<_>>();
            let waiting = state
                .send_waiters
                .drain(..)
                .map(|waiter| waiter.value)
                .collect::<Vec<_>>();
            state.receive_waiters.clear();
            state.send_results.clear();
            state.receive_results.clear();
            wake.notify_all();
            (queued, waiting)
        };
        for value in queued.into_iter().chain(waiting) {
            self.channel_payload_release(value, pending);
        }
    }

    fn channel_drain_len(&mut self, drain: u64) -> u64 {
        match self.object(drain) {
            Some(Object::ChannelDrain { values }) => values.len() as u64,
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                u64::MAX
            }
        }
    }

    fn channel_drain_next(&mut self, drain: u64) -> u64 {
        let Some(value) = self.object(drain).and_then(|object| match object {
            Object::ChannelDrain { values } => values.front().copied(),
            _ => None,
        }) else {
            if !matches!(self.object(drain), Some(Object::ChannelDrain { .. })) {
                self.last_status = STATUS_INVALID_HANDLE;
            }
            return 0;
        };
        if self.channel_payload_retain(value) != STATUS_OK {
            return 0;
        }
        let removed = match self.object_mut(drain) {
            Some(Object::ChannelDrain { values }) => values.pop_front(),
            _ => None,
        };
        if removed != Some(value) {
            let _ = self.release(value);
            self.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
        let mut pending = VecDeque::new();
        self.release_strong_edge(value, &mut pending);
        self.drain_destruction(&mut pending);
        value
    }

    fn channel_waiters(&mut self, channel: u64) -> u64 {
        let Some(cell) = self.channel_cell(channel) else {
            return u64::MAX;
        };
        let (lock, _) = &*cell;
        let state = lock.lock().expect("native channel state is not poisoned");
        (state.send_waiters.len() + state.receive_waiters.len()) as u64
    }

    fn sync_collection_park(&mut self, handle: u64) -> Option<Arc<ParkingSignal>> {
        if !matches!(
            self.object(handle),
            Some(
                Object::SyncArray
                    | Object::SyncMap
                    | Object::SyncSet
                    | Object::SyncStack
                    | Object::SyncQueue
            )
        ) {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        }
        let Some(park) = self.sync_collection_parks.get(&handle).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some(park)
    }

    fn sync_cursor_source(
        &mut self,
        collection: u64,
    ) -> Option<(SyncCursorCollection, SyncCursorSource)> {
        let kind = match self.object(collection) {
            Some(Object::SyncArray) => SyncCursorCollection::Array,
            Some(Object::SyncMap) => SyncCursorCollection::Map,
            Some(Object::SyncSet) => SyncCursorCollection::Set,
            Some(Object::SyncStack) => SyncCursorCollection::Stack,
            Some(Object::SyncQueue) => SyncCursorCollection::Queue,
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                return None;
            }
        };
        let Some(park) = self.sync_collection_parks.get(&collection).cloned() else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        let source = match kind {
            SyncCursorCollection::Array => self
                .sync_arrays
                .get(&collection)
                .cloned()
                .map(|cell| SyncCursorSource::Array(cell, park)),
            SyncCursorCollection::Map => self
                .sync_maps
                .get(&collection)
                .cloned()
                .map(|cell| SyncCursorSource::Map(cell, park)),
            SyncCursorCollection::Set => self
                .sync_sets
                .get(&collection)
                .cloned()
                .map(|cell| SyncCursorSource::Set(cell, park)),
            SyncCursorCollection::Stack => self
                .sync_stacks
                .get(&collection)
                .cloned()
                .map(|cell| SyncCursorSource::Stack(cell, park)),
            SyncCursorCollection::Queue => self
                .sync_queues
                .get(&collection)
                .cloned()
                .map(|cell| SyncCursorSource::Queue(cell, park)),
        };
        let Some(source) = source else {
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        Some((kind, source))
    }

    fn sync_cursor_allocate(
        &mut self,
        collection: u64,
        kind: SyncCursorCollection,
        horizon: u64,
    ) -> u64 {
        let cursor = self.alloc(
            Object::SyncCursor { collection, kind },
            ObjectKind::SyncCursor,
        );
        if cursor != 0 {
            self.sync_cursors.insert(
                cursor,
                Arc::new(Mutex::new(SyncCursorState {
                    horizon,
                    position: if kind == SyncCursorCollection::Stack {
                        u64::MAX
                    } else {
                        0
                    },
                    descending: kind == SyncCursorCollection::Stack,
                    current_key: None,
                })),
            );
        }
        cursor
    }

    fn sync_cursor_context(
        &mut self,
        cursor: u64,
    ) -> Option<(
        Arc<Mutex<SyncCursorState>>,
        SyncCursorCollection,
        SyncCursorSource,
    )> {
        let (collection, kind) = match self.object(cursor) {
            Some(Object::SyncCursor { collection, kind }) => (*collection, *kind),
            _ => {
                self.last_status = STATUS_INVALID_HANDLE;
                return None;
            }
        };
        if self.retain(cursor) != STATUS_OK {
            return None;
        }
        let Some(cursor_state) = self.sync_cursors.get(&cursor).cloned() else {
            let _ = self.release(cursor);
            self.last_status = STATUS_INVALID_HANDLE;
            return None;
        };
        let Some((_, source)) = self.sync_cursor_source(collection) else {
            let _ = self.release(cursor);
            return None;
        };
        Some((cursor_state, kind, source))
    }

    fn sync_array_from_values(&mut self, values: Vec<u64>) -> u64 {
        if values.len() > HOST_MAX_BYTES {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        }
        let handle = self.alloc(Object::SyncArray, ObjectKind::SyncArray);
        if handle != 0 {
            self.sync_arrays
                .insert(handle, Arc::new(RwLock::new(values)));
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_array_new(&mut self, length: u64) -> u64 {
        let Ok(length) = usize::try_from(length) else {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        };
        if length > HOST_MAX_BYTES {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        }
        self.sync_array_from_values(vec![0; length])
    }

    fn sync_map_new(&mut self) -> u64 {
        let handle = self.alloc(Object::SyncMap, ObjectKind::SyncMap);
        if handle != 0 {
            self.sync_maps.insert(
                handle,
                Arc::new(RwLock::new(SyncMapState {
                    next_generation: 1,
                    ..SyncMapState::default()
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_map_from_entries(&mut self, entries: Vec<(u64, u64)>) -> u64 {
        if entries.len() > HOST_MAX_BYTES {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        }
        let handle = self.alloc(Object::SyncMap, ObjectKind::SyncMap);
        if handle != 0 {
            let next_generation = entries.len() as u64 + 1;
            let entries = entries
                .into_iter()
                .enumerate()
                .map(|(index, (key, value))| SyncMapEntry {
                    key,
                    value,
                    generation: index as u64 + 1,
                })
                .collect();
            self.sync_maps.insert(
                handle,
                Arc::new(RwLock::new(SyncMapState {
                    entries,
                    next_generation,
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_set_new(&mut self) -> u64 {
        let handle = self.alloc(Object::SyncSet, ObjectKind::SyncSet);
        if handle != 0 {
            self.sync_sets.insert(
                handle,
                Arc::new(RwLock::new(SyncSetState {
                    next_generation: 1,
                    ..SyncSetState::default()
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_set_from_values(&mut self, values: Vec<u64>) -> u64 {
        if values.len() > HOST_MAX_BYTES {
            self.last_status = STATUS_HOST_LIMIT;
            return 0;
        }
        let handle = self.alloc(Object::SyncSet, ObjectKind::SyncSet);
        if handle != 0 {
            let next_generation = values.len() as u64 + 1;
            let entries = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| SyncValueEntry {
                    value,
                    generation: index as u64 + 1,
                })
                .collect();
            self.sync_sets.insert(
                handle,
                Arc::new(RwLock::new(SyncSetState {
                    entries,
                    next_generation,
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_stack_new(&mut self) -> u64 {
        let handle = self.alloc(Object::SyncStack, ObjectKind::SyncStack);
        if handle != 0 {
            self.sync_stacks.insert(
                handle,
                Arc::new(Mutex::new(SyncStackState {
                    next_generation: 1,
                    ..SyncStackState::default()
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
    }

    fn sync_queue_new(&mut self) -> u64 {
        let handle = self.alloc(Object::SyncQueue, ObjectKind::SyncQueue);
        if handle != 0 {
            self.sync_queues.insert(
                handle,
                Arc::new(Mutex::new(SyncQueueState {
                    next_generation: 1,
                    ..SyncQueueState::default()
                })),
            );
            self.sync_collection_parks
                .insert(handle, Arc::new(ParkingSignal::new()));
        }
        handle
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

    /// Records a terminal child exactly once for every group that owns it.
    /// Notifications are derived from the task transition, so a group added
    /// after a task is already ready queues that child during `group_add`.
    fn notify_groups(&mut self, source: u64) {
        let terminal = matches!(
            self.object(source),
            Some(Object::Task {
                state: TaskState::Ready | TaskState::Cancelled,
                ..
            })
        );
        if !terminal {
            return;
        }
        let groups = self
            .objects
            .iter()
            .filter_map(|(handle, (_, object))| {
                matches!(object, Object::Group(_)).then_some(*handle)
            })
            .collect::<Vec<_>>();
        for group_handle in groups {
            let Some(Object::Group(group)) = self.object_mut(group_handle) else {
                continue;
            };
            let Some(child) = group
                .children
                .iter_mut()
                .find(|child| child.task == source && !child.queued)
            else {
                continue;
            };
            child.queued = true;
            group.completion_queue.push_back(child.index);
            if group.phase == GroupPhase::Waiting {
                group.phase = GroupPhase::ReadyToConsume;
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
        self.notify_groups(arm.source);
    }

    fn take_select_source(&mut self, source: u64, kind: SelectSourceKind) -> u64 {
        match (kind, self.object_mut(source)) {
            (
                SelectSourceKind::Task,
                Some(Object::Task {
                    state,
                    value,
                    group_owner,
                    ..
                }),
            ) => {
                if group_owner.is_some() {
                    self.last_status = STATUS_INVALID_TRANSITION;
                    return 0;
                }
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
        if kind == SelectSourceKind::Task
            && matches!(
                self.object(source),
                Some(Object::Task {
                    group_owner: Some(_),
                    ..
                })
            )
        {
            return STATUS_INVALID_TRANSITION;
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

/// Acquires a per-collection read lock without spinning. Native workers wait
/// on the collection's epoch signal when a writer owns the lock; the signal
/// is detached from the global handle table, so unrelated handles continue to
/// make progress.
fn native_sync_read<T, R>(
    cell: &Arc<RwLock<T>>,
    park: &Arc<ParkingSignal>,
    operation: impl Fn(&T) -> R,
) -> Result<R, u64> {
    loop {
        let expected = park.epoch();
        match cell.try_read() {
            Ok(values) => {
                let result = operation(&values);
                drop(values);
                park.wake(false);
                return Ok(result);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                let status = park.wait(expected, u64::MAX);
                if status != STATUS_OK {
                    return Err(status);
                }
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(STATUS_INVALID_TRANSITION);
            }
        }
    }
}

/// Acquires a per-collection write lock using the same epoch parking protocol
/// as reads. A successful mutation wakes one native waiter after releasing the
/// lock; no operation holds the global handle-table mutex while waiting.
fn native_sync_write<T, R>(
    cell: &Arc<RwLock<T>>,
    park: &Arc<ParkingSignal>,
    operation: impl Fn(&mut T) -> R,
) -> Result<R, u64> {
    loop {
        let expected = park.epoch();
        match cell.try_write() {
            Ok(mut values) => {
                let result = operation(&mut values);
                drop(values);
                park.wake(false);
                return Ok(result);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                let status = park.wait(expected, u64::MAX);
                if status != STATUS_OK {
                    return Err(status);
                }
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(STATUS_INVALID_TRANSITION);
            }
        }
    }
}

/// Mutex-backed stacks and queues use the same parking signal while keeping a
/// single FIFO/LIFO mutation critical section per identity.
fn native_sync_mutex<T, R>(
    cell: &Arc<Mutex<T>>,
    park: &Arc<ParkingSignal>,
    operation: impl Fn(&mut T) -> R,
) -> Result<R, u64> {
    loop {
        let expected = park.epoch();
        match cell.try_lock() {
            Ok(mut values) => {
                let result = operation(&mut values);
                drop(values);
                park.wake(false);
                return Ok(result);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                let status = park.wait(expected, u64::MAX);
                if status != STATUS_OK {
                    return Err(status);
                }
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(STATUS_INVALID_TRANSITION);
            }
        }
    }
}

fn sync_cursor_horizon(source: &SyncCursorSource) -> Result<u64, u64> {
    match source {
        SyncCursorSource::Array(cell, park) => {
            native_sync_read(cell, park, |values| values.len() as u64)
        }
        SyncCursorSource::Map(cell, park) => {
            native_sync_read(cell, park, |state| state.next_generation.saturating_sub(1))
        }
        SyncCursorSource::Set(cell, park) => {
            native_sync_read(cell, park, |state| state.next_generation.saturating_sub(1))
        }
        SyncCursorSource::Stack(cell, park) => {
            native_sync_mutex(cell, park, |state| state.next_generation.saturating_sub(1))
        }
        SyncCursorSource::Queue(cell, park) => {
            native_sync_mutex(cell, park, |state| state.next_generation.saturating_sub(1))
        }
    }
}

/// Reads one native cursor item under the collection's own synchronization
/// primitive. The cursor state mutex is held by the caller, but the global
/// handle table is never held while a collection worker parks.
fn sync_cursor_next_value(
    source: &SyncCursorSource,
    cursor: &mut SyncCursorState,
) -> Result<Option<(u64, Option<u64>)>, u64> {
    let horizon = cursor.horizon;
    let position = cursor.position;
    let descending = cursor.descending;
    match source {
        SyncCursorSource::Array(cell, park) => {
            if position >= horizon {
                cursor.current_key = None;
                return Ok(None);
            }
            let value = native_sync_read(cell, park, |values| {
                usize::try_from(position)
                    .ok()
                    .and_then(|index| values.get(index).copied())
            })?;
            let Some(value) = value else {
                cursor.position = horizon;
                cursor.current_key = None;
                return Ok(None);
            };
            cursor.position = position.saturating_add(1);
            cursor.current_key = None;
            Ok(Some((value, None)))
        }
        SyncCursorSource::Map(cell, park) => {
            let item = native_sync_read(cell, park, |state| {
                let eligible = state.entries.iter().filter(|entry| {
                    entry.generation <= horizon
                        && if descending {
                            entry.generation < position
                        } else {
                            entry.generation > position
                        }
                });
                if descending {
                    eligible
                        .max_by_key(|entry| entry.generation)
                        .map(|entry| (entry.generation, entry.key, entry.value))
                } else {
                    eligible
                        .min_by_key(|entry| entry.generation)
                        .map(|entry| (entry.generation, entry.key, entry.value))
                }
            })?;
            let Some((generation, key, value)) = item else {
                cursor.current_key = None;
                return Ok(None);
            };
            cursor.position = generation;
            cursor.current_key = Some(key);
            Ok(Some((value, Some(key))))
        }
        SyncCursorSource::Set(cell, park) => {
            let item = native_sync_read(cell, park, |state| {
                let eligible = state.entries.iter().filter(|entry| {
                    entry.generation <= horizon
                        && if descending {
                            entry.generation < position
                        } else {
                            entry.generation > position
                        }
                });
                if descending {
                    eligible
                        .max_by_key(|entry| entry.generation)
                        .map(|entry| (entry.generation, entry.value))
                } else {
                    eligible
                        .min_by_key(|entry| entry.generation)
                        .map(|entry| (entry.generation, entry.value))
                }
            })?;
            let Some((generation, value)) = item else {
                cursor.current_key = None;
                return Ok(None);
            };
            cursor.position = generation;
            cursor.current_key = None;
            Ok(Some((value, None)))
        }
        SyncCursorSource::Stack(cell, park) => {
            let item = native_sync_mutex(cell, park, |state| {
                state
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.generation <= horizon
                            && if descending {
                                entry.generation < position
                            } else {
                                entry.generation > position
                            }
                    })
                    .map(|entry| (entry.generation, entry.value))
                    .max_by_key(|(generation, _)| *generation)
            })?;
            let Some((generation, value)) = item else {
                cursor.current_key = None;
                return Ok(None);
            };
            cursor.position = generation;
            cursor.current_key = None;
            Ok(Some((value, None)))
        }
        SyncCursorSource::Queue(cell, park) => {
            let item = native_sync_mutex(cell, park, |state| {
                state
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.generation <= horizon
                            && if descending {
                                entry.generation < position
                            } else {
                                entry.generation > position
                            }
                    })
                    .map(|entry| (entry.generation, entry.value))
                    .min_by_key(|(generation, _)| *generation)
            })?;
            let Some((generation, value)) = item else {
                cursor.current_key = None;
                return Ok(None);
            };
            cursor.position = generation;
            cursor.current_key = None;
            Ok(Some((value, None)))
        }
    }
}

fn atomic_order(order: u64) -> Option<Ordering> {
    match order {
        0 => Some(Ordering::Relaxed),
        1 => Some(Ordering::Acquire),
        2 => Some(Ordering::Release),
        3 => Some(Ordering::AcqRel),
        4 => Some(Ordering::SeqCst),
        _ => None,
    }
}

fn atomic_load_order(order: u64) -> Option<Ordering> {
    matches!(order, 0 | 1 | 4)
        .then(|| atomic_order(order))
        .flatten()
}

fn atomic_store_order(order: u64) -> Option<Ordering> {
    matches!(order, 0 | 2 | 4)
        .then(|| atomic_order(order))
        .flatten()
}

fn atomic_cas_failure_order(order: u64) -> Option<Ordering> {
    matches!(order, 0 | 1 | 4)
        .then(|| atomic_order(order))
        .flatten()
}

/// Returns whether a compare-exchange failure ordering is valid for the
/// success ordering.  Release and Acquire are not interchangeable ranks:
/// `Release` success only permits a Relaxed failure, while `Acquire` success
/// permits Relaxed or Acquire.  Keeping this matrix explicit prevents passing
/// an ordering pair that the Rust atomic primitive would reject at runtime.
fn atomic_cas_orders_compatible(success: u64, failure: u64) -> bool {
    match success {
        0 => failure == 0,
        1 => matches!(failure, 0 | 1),
        2 => failure == 0,
        3 => matches!(failure, 0 | 1),
        4 => matches!(failure, 0 | 1 | 4),
        _ => false,
    }
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

/// Opens a capability-gated host resource. Capability ids are private ABI
/// values: console=0, filesystem=1, process=2 and clock=3. Unknown or
/// unselected capabilities fail closed and return zero.
pub extern "C" fn tondo_rt_host_open(capability: u64) -> u64 {
    with_state(|state| state.host_open(capability))
}

/// Reads at most `max_bytes` from a host handle. The returned opaque Result
/// carries a Buffer on success or the private status code on error. EOF is a
/// successful empty Buffer, allowing partial reads without exposing pointers.
pub extern "C" fn tondo_rt_host_read(handle: u64, max_bytes: u64) -> u64 {
    with_state(|state| state.host_read(handle, max_bytes))
}

/// Writes one immutable Buffer atomically to a host handle and returns a
/// Result whose success payload is the number of bytes written.
pub extern "C" fn tondo_rt_host_write(handle: u64, buffer: u64) -> u64 {
    with_state(|state| state.host_write(handle, buffer))
}

/// Returns a snapshot of the bytes written to a host handle so far.
pub extern "C" fn tondo_rt_host_output(handle: u64) -> u64 {
    with_state(|state| state.host_output(handle))
}

/// Cancels an open host handle. Cancellation is terminal until the owner
/// closes the handle; operations never silently continue after this point.
pub extern "C" fn tondo_rt_host_cancel(handle: u64) -> u64 {
    with_state(|state| state.host_cancel(handle))
}

/// Closes a host handle and releases its buffers exactly once.
pub extern "C" fn tondo_rt_host_close(handle: u64) -> u64 {
    with_state(|state| state.host_close(handle))
}

/// Returns 0=open, 1=cancelled or 2=closed for a live host handle.
pub extern "C" fn tondo_rt_host_status(handle: u64) -> u64 {
    with_state(|state| state.host_state(handle))
}

/// Creates a one-byte immutable Buffer without exposing a native pointer.
pub extern "C" fn tondo_rt_buffer_from_byte(value: u64) -> u64 {
    with_state(|state| state.buffer_from_byte(value))
}

/// Returns the byte length of an opaque Buffer.
pub extern "C" fn tondo_rt_buffer_len(buffer: u64) -> u64 {
    with_state(|state| state.buffer_len(buffer))
}

/// Reads one byte from an opaque Buffer, returning `u64::MAX` for an invalid
/// handle or out-of-range index and recording the corresponding status.
pub extern "C" fn tondo_rt_buffer_byte(buffer: u64, index: u64) -> u64 {
    with_state(|state| state.buffer_byte(buffer, index))
}

/// Creates a native atomic scalar. The handle is opaque and the cell itself
/// is safe to use from multiple OS threads; callers still cross the ordinary
/// retain/mark-shared ownership boundary before publishing the handle.
pub extern "C" fn tondo_rt_atomic_new(value: u64) -> u64 {
    with_state(|state| state.atomic_new(value))
}

/// Loads an atomic scalar using the closed MemoryOrder ABI (0=Relaxed,
/// 1=Acquire, 4=SeqCst). Invalid orders return zero and set the status
/// channel to `STATUS_ATOMIC_INVALID_ORDER`.
pub extern "C" fn tondo_rt_atomic_load(atomic: u64, order: u64) -> u64 {
    let Some(order) = atomic_load_order(order) else {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return 0;
    };
    let Some(cell) = with_state(|state| state.atomic_cell(atomic)) else {
        return 0;
    };
    cell.load(order)
}

/// Stores an atomic scalar using Relaxed, Release or SeqCst ordering.
pub extern "C" fn tondo_rt_atomic_store(atomic: u64, value: u64, order: u64) -> u64 {
    let Some(order) = atomic_store_order(order) else {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return STATUS_ATOMIC_INVALID_ORDER;
    };
    let Some(cell) = with_state(|state| state.atomic_cell(atomic)) else {
        return STATUS_INVALID_HANDLE;
    };
    cell.store(value, order);
    STATUS_OK
}

/// Swaps an atomic scalar and returns its previous value. All five memory
/// orders are valid for this read-modify-write operation.
pub extern "C" fn tondo_rt_atomic_swap(atomic: u64, value: u64, order: u64) -> u64 {
    let Some(order) = atomic_order(order) else {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return 0;
    };
    let Some(cell) = with_state(|state| state.atomic_cell(atomic)) else {
        return 0;
    };
    cell.swap(value, order)
}

/// Performs a non-spurious compare-exchange and returns the observed value.
/// `tondo_rt_last_status()` is `STATUS_OK` on exchange and
/// `STATUS_ATOMIC_MISMATCH` when the observed value differs from `expected`.
pub extern "C" fn tondo_rt_atomic_compare_exchange(
    atomic: u64,
    expected: u64,
    desired: u64,
    success: u64,
    failure: u64,
) -> u64 {
    let success_code = success;
    let failure_code = failure;
    let Some(success) = atomic_order(success_code) else {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return 0;
    };
    let Some(failure) = atomic_cas_failure_order(failure_code) else {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return 0;
    };
    if !atomic_cas_orders_compatible(success_code, failure_code) {
        with_state(|state| state.last_status = STATUS_ATOMIC_INVALID_ORDER);
        return 0;
    }
    let Some(cell) = with_state(|state| state.atomic_cell(atomic)) else {
        return 0;
    };
    match cell.compare_exchange(expected, desired, success, failure) {
        Ok(previous) => {
            with_state(|state| state.last_status = STATUS_OK);
            previous
        }
        Err(observed) => {
            with_state(|state| state.last_status = STATUS_ATOMIC_MISMATCH);
            observed
        }
    }
}

/// Creates a native parking signal. The signal is an epoch, not a queue: a
/// waiter must re-check its predicate after every successful wakeup.
pub extern "C" fn tondo_rt_sync_park_new() -> u64 {
    with_state(|state| state.park_new())
}

/// Reads the current parking epoch without waiting.
pub extern "C" fn tondo_rt_sync_park_epoch(park: u64) -> u64 {
    let Some(signal) = with_state(|state| state.park_signal(park)) else {
        return u64::MAX;
    };
    signal.epoch()
}

/// Parks the calling native worker until the epoch changes or the supplied
/// timeout expires. `u64::MAX` is an unbounded wait; the cooperative VM never
/// calls this function directly and uses its poll/park state instead.
pub extern "C" fn tondo_rt_sync_park_wait(park: u64, expected_epoch: u64, timeout_ns: u64) -> u64 {
    let Some(signal) = with_state(|state| state.park_signal(park)) else {
        return STATUS_INVALID_HANDLE;
    };
    signal.wait(expected_epoch, timeout_ns)
}

/// Advances the parking epoch and wakes one waiter (`all=0`) or all waiters
/// (`all!=0`). The return value is the number of waiters observed before the
/// notification; it is diagnostic only and does not affect correctness.
pub extern "C" fn tondo_rt_sync_park_wake(park: u64, all: u64) -> u64 {
    let Some(signal) = with_state(|state| state.park_signal(park)) else {
        return u64::MAX;
    };
    signal.wake(all != 0)
}

/// Returns the number of workers currently registered in a parking wait.
pub extern "C" fn tondo_rt_sync_park_waiters(park: u64) -> u64 {
    let Some(signal) = with_state(|state| state.park_signal(park)) else {
        return u64::MAX;
    };
    signal.waiters()
}

/// Creates a private native channel identity. The caller obtains affine
/// endpoints with `tondo_rt_channel_sender` and
/// `tondo_rt_channel_receiver`, then releases this identity.
pub extern "C" fn tondo_rt_channel_bounded(capacity: i64) -> u64 {
    let Ok(capacity) = usize::try_from(capacity) else {
        with_state(|state| state.last_status = STATUS_CHANNEL_INVALID_CAPACITY);
        return 0;
    };
    with_state(|state| state.channel_new(Some(capacity)))
}

/// Creates the explicitly unbounded native channel form. The queue remains
/// subject to `HOST_MAX_BYTES` so resource exhaustion is recoverable.
pub extern "C" fn tondo_rt_channel_unbounded() -> u64 {
    with_state(|state| state.channel_new(None))
}

pub extern "C" fn tondo_rt_channel_sender(channel: u64) -> u64 {
    with_state(|state| state.channel_endpoint(channel, true))
}

pub extern "C" fn tondo_rt_channel_receiver(channel: u64) -> u64 {
    with_state(|state| state.channel_endpoint(channel, false))
}

pub extern "C" fn tondo_rt_channel_sender_fork(sender: u64) -> u64 {
    with_state(|state| {
        let Some((channel, _)) = state.channel_endpoint_cell(sender, true) else {
            return 0;
        };
        state.channel_endpoint(channel, true)
    })
}

pub extern "C" fn tondo_rt_channel_receiver_fork(receiver: u64) -> u64 {
    with_state(|state| {
        let Some((channel, _)) = state.channel_endpoint_cell(receiver, false) else {
            return 0;
        };
        state.channel_endpoint(channel, false)
    })
}

pub extern "C" fn tondo_rt_channel_sender_close(sender: u64) -> u64 {
    with_state(|state| state.channel_close_sender(sender))
}

/// Closes a receiver and returns a private drain carrier. The carrier is
/// consumed with `tondo_rt_channel_drain_next`; native lowering maps it to the
/// public `Array[T]` result without exposing a layout or pointer.
pub extern "C" fn tondo_rt_channel_receiver_close(receiver: u64) -> u64 {
    with_state(|state| state.channel_close_receiver(receiver))
}

pub extern "C" fn tondo_rt_channel_send(sender: u64, value: u64) -> u64 {
    let (cell, status) = with_state(|state| {
        let Some((_, cell)) = state.channel_endpoint_cell(sender, true) else {
            return (None, state.last_status);
        };
        let status = state.channel_payload_retain(value);
        (Some(cell), status)
    });
    if status != STATUS_OK {
        return 0;
    }
    let Some(cell) = cell else {
        return 0;
    };
    native_channel_send_result(native_channel_send(&cell, value), value)
}

pub extern "C" fn tondo_rt_channel_try_send(sender: u64, value: u64) -> u64 {
    let (cell, status) = with_state(|state| {
        let Some((_, cell)) = state.channel_endpoint_cell(sender, true) else {
            return (None, state.last_status);
        };
        let status = state.channel_payload_retain(value);
        (Some(cell), status)
    });
    if status != STATUS_OK {
        return 0;
    }
    let Some(cell) = cell else {
        return 0;
    };
    native_channel_send_result(native_channel_try_send(&cell, value), value)
}

pub extern "C" fn tondo_rt_channel_receive(receiver: u64) -> u64 {
    let cell = with_state(|state| {
        state
            .channel_endpoint_cell(receiver, false)
            .map(|(_, cell)| cell)
    });
    let Some(cell) = cell else {
        return 0;
    };
    native_channel_receive_result(native_channel_receive(&cell))
}

pub extern "C" fn tondo_rt_channel_try_receive(receiver: u64) -> u64 {
    let cell = with_state(|state| {
        state
            .channel_endpoint_cell(receiver, false)
            .map(|(_, cell)| cell)
    });
    let Some(cell) = cell else {
        return 0;
    };
    native_channel_try_receive_result(native_channel_try_receive(&cell))
}

pub extern "C" fn tondo_rt_channel_drain_len(drain: u64) -> u64 {
    with_state(|state| state.channel_drain_len(drain))
}

pub extern "C" fn tondo_rt_channel_drain_next(drain: u64) -> u64 {
    with_state(|state| state.channel_drain_next(drain))
}

pub extern "C" fn tondo_rt_channel_waiters(channel: u64) -> u64 {
    with_state(|state| state.channel_waiters(channel))
}

fn native_collection_result(tag: u64, payload: Option<u64>) -> u64 {
    with_state(|state| state.host_result(tag, payload))
}

fn native_collection_error(status: u64) -> u64 {
    with_state(|state| state.host_error(status))
}

fn native_collection_status(status: u64) {
    with_state(|state| state.last_status = status);
}

fn native_optional_result(value: Option<u64>) -> u64 {
    native_collection_result(value.map_or(RESULT_NONE, |_| RESULT_SOME), value)
}

/// Advances every native waiter that can be committed without blocking. The
/// queues themselves are the FIFO registration order; completed outcomes stay
/// in the result maps until their owning call consumes them.
fn native_channel_progress(state: &mut NativeChannelState) -> bool {
    let mut changed = false;
    if state.receiver_closed {
        while let Some(waiter) = state.send_waiters.pop_front() {
            state
                .send_results
                .insert(waiter.id, NativeChannelSendOutcome::Closed);
            changed = true;
        }
    }
    loop {
        if let Some(receiver) = state.receive_waiters.front().copied() {
            if let Some(value) = state.queue.pop_front() {
                state.receive_waiters.pop_front();
                state
                    .receive_results
                    .insert(receiver.id, NativeChannelReceiveOutcome::Value(value));
                changed = true;
                continue;
            }
            if let Some(sender) = state.send_waiters.pop_front() {
                state.receive_waiters.pop_front();
                state
                    .send_results
                    .insert(sender.id, NativeChannelSendOutcome::Committed);
                state.receive_results.insert(
                    receiver.id,
                    NativeChannelReceiveOutcome::Value(sender.value),
                );
                changed = true;
                continue;
            }
        }
        if state.receiver_closed {
            while let Some(receiver) = state.receive_waiters.pop_front() {
                state
                    .receive_results
                    .insert(receiver.id, NativeChannelReceiveOutcome::Closed);
                changed = true;
            }
            break;
        }
        if state.sender_closed && state.queue.is_empty() && state.send_waiters.is_empty() {
            while let Some(receiver) = state.receive_waiters.pop_front() {
                state
                    .receive_results
                    .insert(receiver.id, NativeChannelReceiveOutcome::Closed);
                changed = true;
            }
            break;
        }
        let room = state.capacity.is_none_or(|capacity| {
            state.queue.len() < capacity && state.queue.len() < HOST_MAX_BYTES
        });
        if room && let Some(sender) = state.send_waiters.pop_front() {
            state.queue.push_back(sender.value);
            state
                .send_results
                .insert(sender.id, NativeChannelSendOutcome::Committed);
            changed = true;
            continue;
        }
        break;
    }
    changed
}

fn native_channel_next_waiter(state: &mut NativeChannelState) -> Option<u64> {
    let id = state.next_waiter;
    state.next_waiter = state.next_waiter.checked_add(1)?;
    Some(id)
}

fn native_channel_send(cell: &NativeChannelCell, value: u64) -> NativeChannelSendOutcome {
    let (lock, wake) = &**cell;
    let mut state = lock.lock().expect("native channel state is not poisoned");
    if state.receiver_closed || state.receivers == 0 {
        return NativeChannelSendOutcome::Closed;
    }
    let room = state
        .capacity
        .is_none_or(|capacity| state.queue.len() < capacity && state.queue.len() < HOST_MAX_BYTES)
        && (state.capacity.is_some() || state.queue.len() < HOST_MAX_BYTES);
    if state.receive_waiters.is_empty() && room {
        state.queue.push_back(value);
        wake.notify_all();
        return NativeChannelSendOutcome::Committed;
    }
    if state.capacity.is_none() && state.queue.len() >= HOST_MAX_BYTES {
        return NativeChannelSendOutcome::ResourceLimit;
    }
    let Some(id) = native_channel_next_waiter(&mut state) else {
        return NativeChannelSendOutcome::ResourceLimit;
    };
    state
        .send_waiters
        .push_back(NativeChannelSendWaiter { id, value });
    loop {
        let _ = native_channel_progress(&mut state);
        if let Some(outcome) = state.send_results.remove(&id) {
            wake.notify_all();
            return outcome;
        }
        state = wake
            .wait(state)
            .expect("native channel state is not poisoned");
    }
}

fn native_channel_try_send(cell: &NativeChannelCell, value: u64) -> NativeChannelSendOutcome {
    let (lock, wake) = &**cell;
    let mut state = lock.lock().expect("native channel state is not poisoned");
    let _ = native_channel_progress(&mut state);
    if state.receiver_closed || state.receivers == 0 {
        return NativeChannelSendOutcome::Closed;
    }
    if let Some(receiver) = state.receive_waiters.pop_front() {
        state
            .receive_results
            .insert(receiver.id, NativeChannelReceiveOutcome::Value(value));
        wake.notify_all();
        return NativeChannelSendOutcome::Committed;
    }
    if state
        .capacity
        .is_some_and(|capacity| state.queue.len() >= capacity)
    {
        return NativeChannelSendOutcome::Full;
    }
    if state.capacity.is_none() && state.queue.len() >= HOST_MAX_BYTES {
        return NativeChannelSendOutcome::ResourceLimit;
    }
    state.queue.push_back(value);
    wake.notify_all();
    NativeChannelSendOutcome::Committed
}

fn native_channel_receive(cell: &NativeChannelCell) -> NativeChannelReceiveOutcome {
    let (lock, wake) = &**cell;
    let mut state = lock.lock().expect("native channel state is not poisoned");
    let _ = native_channel_progress(&mut state);
    if let Some(value) = state.queue.pop_front() {
        let _ = native_channel_progress(&mut state);
        wake.notify_all();
        return NativeChannelReceiveOutcome::Value(value);
    }
    if state.sender_closed {
        return NativeChannelReceiveOutcome::Closed;
    }
    let Some(id) = native_channel_next_waiter(&mut state) else {
        return NativeChannelReceiveOutcome::Closed;
    };
    state
        .receive_waiters
        .push_back(NativeChannelReceiveWaiter { id });
    loop {
        let _ = native_channel_progress(&mut state);
        if let Some(outcome) = state.receive_results.remove(&id) {
            wake.notify_all();
            return outcome;
        }
        state = wake
            .wait(state)
            .expect("native channel state is not poisoned");
    }
}

fn native_channel_try_receive(cell: &NativeChannelCell) -> NativeChannelReceiveOutcome {
    let (lock, wake) = &**cell;
    let mut state = lock.lock().expect("native channel state is not poisoned");
    let _ = native_channel_progress(&mut state);
    if let Some(value) = state.queue.pop_front() {
        let _ = native_channel_progress(&mut state);
        wake.notify_all();
        return NativeChannelReceiveOutcome::Value(value);
    }
    if let Some(sender) = state.send_waiters.pop_front() {
        state
            .send_results
            .insert(sender.id, NativeChannelSendOutcome::Committed);
        wake.notify_all();
        return NativeChannelReceiveOutcome::Value(sender.value);
    }
    if state.sender_closed || state.receiver_closed {
        NativeChannelReceiveOutcome::Closed
    } else {
        NativeChannelReceiveOutcome::Empty
    }
}

fn native_channel_send_result(outcome: NativeChannelSendOutcome, value: u64) -> u64 {
    with_state(|state| {
        let mut pending = VecDeque::new();
        let result = match outcome {
            NativeChannelSendOutcome::Committed => state.host_result(RESULT_OK, None),
            NativeChannelSendOutcome::Closed => {
                state.last_status = STATUS_HOST_CLOSED;
                state.host_result(RESULT_ERR, Some(value))
            }
            NativeChannelSendOutcome::Full => {
                state.last_status = STATUS_CHANNEL_FULL;
                state.host_result(RESULT_ERR, Some(value))
            }
            NativeChannelSendOutcome::ResourceLimit => {
                state.last_status = STATUS_HOST_LIMIT;
                state.host_result(RESULT_ERR, Some(value))
            }
        };
        if !matches!(outcome, NativeChannelSendOutcome::Committed) {
            state.channel_payload_release(value, &mut pending);
            state.drain_destruction(&mut pending);
        }
        result
    })
}

fn native_channel_receive_result(outcome: NativeChannelReceiveOutcome) -> u64 {
    with_state(|state| {
        let mut pending = VecDeque::new();
        let result = match outcome {
            NativeChannelReceiveOutcome::Value(value) => {
                let result = state.host_result(RESULT_SOME, Some(value));
                state.channel_payload_release(value, &mut pending);
                result
            }
            NativeChannelReceiveOutcome::Closed => state.host_result(RESULT_NONE, None),
            NativeChannelReceiveOutcome::Empty => {
                state.last_status = STATUS_CHANNEL_EMPTY;
                RESULT_NONE
            }
        };
        state.drain_destruction(&mut pending);
        result
    })
}

fn native_channel_try_receive_result(outcome: NativeChannelReceiveOutcome) -> u64 {
    with_state(|state| {
        let mut pending = VecDeque::new();
        let result = match outcome {
            NativeChannelReceiveOutcome::Value(value) => {
                let result = state.host_result(RESULT_SOME, Some(value));
                state.channel_payload_release(value, &mut pending);
                result
            }
            NativeChannelReceiveOutcome::Empty => {
                state.last_status = STATUS_CHANNEL_EMPTY;
                RESULT_NONE
            }
            NativeChannelReceiveOutcome::Closed => {
                state.last_status = STATUS_HOST_CLOSED;
                state.host_result(RESULT_ERR, Some(STATUS_HOST_CLOSED))
            }
        };
        state.drain_destruction(&mut pending);
        result
    })
}

fn next_sync_generation(next_generation: &mut u64) -> Result<u64, u64> {
    let generation = *next_generation;
    let Some(next) = generation.checked_add(1) else {
        return Err(STATUS_COUNT_OVERFLOW);
    };
    *next_generation = next;
    Ok(generation)
}

/// Starts a private native cursor. The returned handle owns one strong edge
/// to the source collection and captures only its finite structural horizon;
/// it never snapshots collection contents.
pub extern "C" fn tondo_rt_sync_cursor_start(collection: u64) -> u64 {
    let Some((kind, source)) = with_state(|state| {
        if state.retain(collection) != STATUS_OK {
            return None;
        }
        match state.sync_cursor_source(collection) {
            Some(source) => Some(source),
            None => {
                let _ = state.release(collection);
                None
            }
        }
    }) else {
        return 0;
    };
    let horizon = match sync_cursor_horizon(&source) {
        Ok(horizon) => horizon,
        Err(status) => {
            with_state(|state| {
                let _ = state.release(collection);
            });
            native_collection_status(status);
            return 0;
        }
    };
    with_state(|state| {
        let cursor = state.sync_cursor_allocate(collection, kind, horizon);
        let _ = state.release(collection);
        cursor
    })
}

/// Returns the next scalar value from a private native cursor. Map cursors
/// return the value and expose the corresponding key through
/// `tondo_rt_sync_cursor_key`; all other collection kinds return their value
/// directly. The result is an owned opaque `Option` record.
pub extern "C" fn tondo_rt_sync_cursor_next(cursor: u64) -> u64 {
    let Some((cursor_state, _kind, source)) = with_state(|state| state.sync_cursor_context(cursor))
    else {
        return 0;
    };
    let outcome = match cursor_state.lock() {
        Ok(mut state) => sync_cursor_next_value(&source, &mut state),
        Err(_) => Err(STATUS_INVALID_TRANSITION),
    };
    let result = match outcome {
        Ok(Some((value, _))) => native_optional_result(Some(value)),
        Ok(None) => native_optional_result(None),
        Err(status) => {
            native_collection_status(status);
            0
        }
    };
    with_state(|state| {
        let _ = state.release(cursor);
    });
    result
}

/// Returns the key produced by the most recent successful `next` on a Map
/// cursor. Calling it for another collection kind or before a value exists is
/// an invalid transition and returns zero.
pub extern "C" fn tondo_rt_sync_cursor_key(cursor: u64) -> u64 {
    let Some((cursor_state, kind, _source)) = with_state(|state| state.sync_cursor_context(cursor))
    else {
        return 0;
    };
    let key = if kind == SyncCursorCollection::Map {
        cursor_state.lock().ok().and_then(|state| state.current_key)
    } else {
        None
    };
    let result = match key {
        Some(key) => key,
        None => {
            native_collection_status(STATUS_INVALID_TRANSITION);
            0
        }
    };
    with_state(|state| {
        let _ = state.release(cursor);
    });
    result
}

/// Creates a fixed-length native `sync.Array` whose slots initially contain
/// zero-valued scalar carriers. The compiler's generic lowering supplies real
/// values through `set`; this private ABI never exposes a pointer or layout.
pub extern "C" fn tondo_rt_sync_array_new(length: u64) -> u64 {
    with_state(|state| state.sync_array_new(length))
}

pub extern "C" fn tondo_rt_sync_array_length(array: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, Vec::len).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        |length| length as u64,
    )
}

pub extern "C" fn tondo_rt_sync_array_is_empty(array: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, Vec::is_empty).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_array_get(array: u64, index: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return 0;
    };
    let value = native_sync_read(&cell, &park, |values| {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index).copied())
    });
    match value {
        Ok(value) => native_optional_result(value),
        Err(status) => {
            native_collection_status(status);
            0
        }
    }
}

pub extern "C" fn tondo_rt_sync_array_set(array: u64, index: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return 0;
    };
    let outcome = native_sync_write(&cell, &park, |values| {
        let Ok(index) = usize::try_from(index) else {
            return Err(STATUS_COLLECTION_INVALID);
        };
        values
            .get_mut(index)
            .map(|slot| std::mem::replace(slot, value))
            .ok_or(STATUS_COLLECTION_INVALID)
    });
    match outcome {
        Ok(Ok(previous)) => native_collection_result(RESULT_OK, Some(previous)),
        Ok(Err(status)) | Err(status) => native_collection_error(status),
    }
}

/// Performs a strong, non-spurious compare-exchange and returns an opaque
/// `Result` carrying the observed scalar. `last_status` is `STATUS_OK` after
/// an exchange and `STATUS_ATOMIC_MISMATCH` when the expectation did not
/// match; the cell is never modified on a mismatch.
pub extern "C" fn tondo_rt_sync_array_compare_exchange(
    array: u64,
    index: u64,
    expected: u64,
    desired: u64,
) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return 0;
    };
    let outcome = native_sync_write(&cell, &park, |values| {
        let Ok(index) = usize::try_from(index) else {
            return Err(STATUS_COLLECTION_INVALID);
        };
        let Some(slot) = values.get_mut(index) else {
            return Err(STATUS_COLLECTION_INVALID);
        };
        let observed = *slot;
        let exchanged = observed == expected;
        if exchanged {
            *slot = desired;
        }
        Ok((observed, exchanged))
    });
    let (observed, exchanged) = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(status)) | Err(status) => return native_collection_error(status),
    };
    native_collection_status(if exchanged {
        STATUS_OK
    } else {
        STATUS_ATOMIC_MISMATCH
    });
    native_collection_result(
        if exchanged {
            RESULT_CAS_EXCHANGED
        } else {
            RESULT_CAS_MISMATCH
        },
        Some(observed),
    )
}

pub extern "C" fn tondo_rt_sync_array_snapshot(array: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_array_cell(array)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(array)) else {
        return 0;
    };
    let values = match native_sync_read(&cell, &park, Clone::clone) {
        Ok(values) => values,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    with_state(|state| state.sync_array_from_values(values))
}

pub extern "C" fn tondo_rt_sync_map_new() -> u64 {
    with_state(State::sync_map_new)
}

pub extern "C" fn tondo_rt_sync_map_length(map: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| state.entries.len()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        |length| length as u64,
    )
}

pub extern "C" fn tondo_rt_sync_map_is_empty(map: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| state.entries.is_empty()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_map_get(map: u64, key: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return 0;
    };
    let value = match native_sync_read(&cell, &park, |state| {
        state
            .entries
            .iter()
            .find_map(|entry| (entry.key == key).then_some(entry.value))
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_map_contains(map: u64, key: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| {
        state.entries.iter().any(|entry| entry.key == key)
    })
    .map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_map_insert(map: u64, key: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return 0;
    };
    let outcome = native_sync_write(&cell, &park, |state| {
        if let Some(index) = state.entries.iter().position(|entry| entry.key == key) {
            Ok(Some(std::mem::replace(
                &mut state.entries[index].value,
                value,
            )))
        } else if state.entries.len() >= HOST_MAX_BYTES {
            Err(STATUS_HOST_LIMIT)
        } else {
            let generation = next_sync_generation(&mut state.next_generation)?;
            state.entries.push(SyncMapEntry {
                key,
                value,
                generation,
            });
            Ok(None)
        }
    });
    match outcome {
        Ok(Ok(previous)) => native_collection_result(RESULT_OK, previous),
        Ok(Err(status)) | Err(status) => native_collection_error(status),
    }
}

pub extern "C" fn tondo_rt_sync_map_remove(map: u64, key: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return 0;
    };
    let value = match native_sync_write(&cell, &park, |state| {
        state
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .map(|index| state.entries.remove(index).value)
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_map_compare_exchange(
    map: u64,
    key: u64,
    expected: u64,
    expected_some: u64,
    desired: u64,
    desired_some: u64,
) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return 0;
    };
    let outcome = match native_sync_write(&cell, &park, |state| {
        let position = state.entries.iter().position(|entry| entry.key == key);
        let observed = position.map(|index| state.entries[index].value);
        let matches = match (expected_some != 0, observed) {
            (true, Some(value)) => value == expected,
            (false, None) => true,
            _ => false,
        };
        if matches {
            match (desired_some != 0, position) {
                (true, Some(index)) => state.entries[index].value = desired,
                (true, None) if state.entries.len() < HOST_MAX_BYTES => {
                    let generation = next_sync_generation(&mut state.next_generation)?;
                    state.entries.push(SyncMapEntry {
                        key,
                        value: desired,
                        generation,
                    });
                }
                (true, None) => return Err(STATUS_HOST_LIMIT),
                (false, Some(index)) => {
                    state.entries.remove(index);
                }
                (false, None) => {}
            }
        }
        Ok((observed, matches))
    }) {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(status)) | Err(status) => return native_collection_error(status),
    };
    native_collection_status(if outcome.1 {
        STATUS_OK
    } else {
        STATUS_ATOMIC_MISMATCH
    });
    native_collection_result(
        if outcome.1 {
            RESULT_CAS_EXCHANGED
        } else {
            RESULT_CAS_MISMATCH
        },
        outcome.0,
    )
}

pub extern "C" fn tondo_rt_sync_map_snapshot(map: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_map_cell(map)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(map)) else {
        return 0;
    };
    let entries = match native_sync_read(&cell, &park, |state| {
        state
            .entries
            .iter()
            .map(|entry| (entry.key, entry.value))
            .collect::<Vec<_>>()
    }) {
        Ok(entries) => entries,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    with_state(|state| state.sync_map_from_entries(entries))
}

pub extern "C" fn tondo_rt_sync_set_new() -> u64 {
    with_state(State::sync_set_new)
}

pub extern "C" fn tondo_rt_sync_set_length(set: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| state.entries.len()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        |length| length as u64,
    )
}

pub extern "C" fn tondo_rt_sync_set_is_empty(set: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| state.entries.is_empty()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_set_contains(set: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return u64::MAX;
    };
    native_sync_read(&cell, &park, |state| {
        state.entries.iter().any(|entry| entry.value == value)
    })
    .map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_set_insert(set: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return 0;
    };
    let outcome = native_sync_write(&cell, &park, |state| {
        if state.entries.iter().any(|entry| entry.value == value) {
            Ok(false)
        } else if state.entries.len() >= HOST_MAX_BYTES {
            Err(STATUS_HOST_LIMIT)
        } else {
            let generation = next_sync_generation(&mut state.next_generation)?;
            state.entries.push(SyncValueEntry { value, generation });
            Ok(true)
        }
    });
    match outcome {
        Ok(Ok(inserted)) => native_collection_result(RESULT_OK, Some(u64::from(inserted))),
        Ok(Err(status)) | Err(status) => native_collection_error(status),
    }
}

pub extern "C" fn tondo_rt_sync_set_remove(set: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return 0;
    };
    match native_sync_write(&cell, &park, |state| {
        state
            .entries
            .iter()
            .position(|candidate| candidate.value == value)
            .map(|index| {
                state.entries.remove(index);
                true
            })
            .unwrap_or(false)
    }) {
        Ok(removed) => removed as u64,
        Err(status) => {
            native_collection_status(status);
            0
        }
    }
}

pub extern "C" fn tondo_rt_sync_set_snapshot(set: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_set_cell(set)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(set)) else {
        return 0;
    };
    let values = match native_sync_read(&cell, &park, |state| {
        state.entries.iter().map(|entry| entry.value).collect()
    }) {
        Ok(values) => values,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    with_state(|state| state.sync_set_from_values(values))
}

pub extern "C" fn tondo_rt_sync_stack_new() -> u64 {
    with_state(State::sync_stack_new)
}

pub extern "C" fn tondo_rt_sync_stack_length(stack: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return u64::MAX;
    };
    native_sync_mutex(&cell, &park, |state| state.entries.len()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        |length| length as u64,
    )
}

pub extern "C" fn tondo_rt_sync_stack_is_empty(stack: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return u64::MAX;
    };
    native_sync_mutex(&cell, &park, |state| state.entries.is_empty()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_stack_push(stack: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return 0;
    };
    let outcome = native_sync_mutex(&cell, &park, |state| {
        if state.entries.len() >= HOST_MAX_BYTES {
            Err(STATUS_HOST_LIMIT)
        } else {
            let generation = next_sync_generation(&mut state.next_generation)?;
            state.entries.push(SyncValueEntry { value, generation });
            Ok(())
        }
    });
    match outcome {
        Ok(Ok(())) => native_collection_result(RESULT_OK, None),
        Ok(Err(status)) | Err(status) => native_collection_error(status),
    }
}

pub extern "C" fn tondo_rt_sync_stack_pop(stack: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return 0;
    };
    let value = match native_sync_mutex(&cell, &park, |state| {
        state.entries.pop().map(|entry| entry.value)
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_stack_peek(stack: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return 0;
    };
    let value = match native_sync_mutex(&cell, &park, |state| {
        state.entries.last().map(|entry| entry.value)
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_stack_snapshot(stack: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_stack_cell(stack)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(stack)) else {
        return 0;
    };
    let mut values: Vec<u64> = match native_sync_mutex(&cell, &park, |state| {
        state.entries.iter().map(|entry| entry.value).collect()
    }) {
        Ok(values) => values,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    values.reverse();
    with_state(|state| state.sync_array_from_values(values))
}

pub extern "C" fn tondo_rt_sync_queue_new() -> u64 {
    with_state(State::sync_queue_new)
}

pub extern "C" fn tondo_rt_sync_queue_length(queue: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return u64::MAX;
    };
    native_sync_mutex(&cell, &park, |state| state.entries.len()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        |length| length as u64,
    )
}

pub extern "C" fn tondo_rt_sync_queue_is_empty(queue: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return u64::MAX;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return u64::MAX;
    };
    native_sync_mutex(&cell, &park, |state| state.entries.is_empty()).map_or_else(
        |status| {
            native_collection_status(status);
            u64::MAX
        },
        u64::from,
    )
}

pub extern "C" fn tondo_rt_sync_queue_enqueue(queue: u64, value: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return 0;
    };
    let outcome = native_sync_mutex(&cell, &park, |state| {
        if state.entries.len() >= HOST_MAX_BYTES {
            Err(STATUS_HOST_LIMIT)
        } else {
            let generation = next_sync_generation(&mut state.next_generation)?;
            state
                .entries
                .push_back(SyncValueEntry { value, generation });
            Ok(())
        }
    });
    match outcome {
        Ok(Ok(())) => native_collection_result(RESULT_OK, None),
        Ok(Err(status)) | Err(status) => native_collection_error(status),
    }
}

pub extern "C" fn tondo_rt_sync_queue_dequeue(queue: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return 0;
    };
    let value = match native_sync_mutex(&cell, &park, |state| {
        state.entries.pop_front().map(|entry| entry.value)
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_queue_peek(queue: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return 0;
    };
    let value = match native_sync_mutex(&cell, &park, |state| {
        state.entries.front().map(|entry| entry.value)
    }) {
        Ok(value) => value,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    native_optional_result(value)
}

pub extern "C" fn tondo_rt_sync_queue_snapshot(queue: u64) -> u64 {
    let Some(cell) = with_state(|state| state.sync_queue_cell(queue)) else {
        return 0;
    };
    let Some(park) = with_state(|state| state.sync_collection_park(queue)) else {
        return 0;
    };
    let values = match native_sync_mutex(&cell, &park, |state| {
        state.entries.iter().map(|entry| entry.value).collect()
    }) {
        Ok(values) => values,
        Err(status) => {
            native_collection_status(status);
            return 0;
        }
    };
    with_state(|state| state.sync_array_from_values(values))
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

/// Creates the private target-qualified blocking worker lane.  The ABI only
/// returns opaque process-local tokens; it does not expose worker pointers or
/// an AOT callback convention.
pub extern "C" fn tondo_rt_blocking_pool_new(workers: i64, capacity: i64) -> u64 {
    with_state(|state| state.blocking_pool_new(workers, capacity))
}

/// Submits one opaque value token to a bounded blocking lane.  A zero return
/// means admission failed; `tondo_rt_last_status` distinguishes saturation,
/// an invalid handle and an unsupported target.
pub extern "C" fn tondo_rt_blocking_pool_submit(pool: u64, payload: u64) -> u64 {
    with_state(|state| state.blocking_pool_submit(pool, payload))
}

/// Returns the private pool lifecycle code: open=0, shutting_down=1,
/// cancelling=2, closed=3 and cancelled=4.
pub extern "C" fn tondo_rt_blocking_pool_status(pool: u64) -> u64 {
    with_state(|state| state.blocking_pool_status(pool))
}

/// Drains already admitted jobs and joins the worker lane.
pub extern "C" fn tondo_rt_blocking_pool_shutdown(pool: u64) -> u64 {
    with_state(|state| state.blocking_pool_shutdown(pool, false))
}

/// Cancels queued jobs, lets a running token finish safely, and joins workers.
pub extern "C" fn tondo_rt_blocking_pool_cancel(pool: u64) -> u64 {
    with_state(|state| state.blocking_pool_shutdown(pool, true))
}

/// Returns the private job state: queued=0, running=1, completed=2,
/// cancelled=3 and taken=4.
pub extern "C" fn tondo_rt_blocking_job_status(job: u64) -> u64 {
    with_state(|state| state.blocking_job_status(job))
}

/// Returns the zero-based native worker index after a job starts.
pub extern "C" fn tondo_rt_blocking_job_worker(job: u64) -> u64 {
    with_state(|state| state.blocking_job_worker(job))
}

/// Waits for a job to reach a terminal state without consuming its payload.
pub extern "C" fn tondo_rt_blocking_job_wait(job: u64) -> u64 {
    with_state(|state| state.blocking_job_wait(job))
}

/// Transfers the completed opaque payload token to the caller exactly once.
pub extern "C" fn tondo_rt_blocking_job_take(job: u64) -> u64 {
    with_state(|state| state.blocking_job_take(job))
}

/// Cancels a queued job.  Running work is never force-killed and therefore
/// returns success while it completes normally.
pub extern "C" fn tondo_rt_blocking_job_cancel(job: u64) -> u64 {
    with_state(|state| state.blocking_job_cancel(job))
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

/// Marks a pending child as an uncaught panic.  This is a private conformance
/// hook; source-level code reports panics through the compiler's unwind edge.
pub extern "C" fn tondo_rt_task_panic(task: u64) -> u64 {
    with_state(|state| state.task_panic(task))
}

/// Creates an affine native Group carrier.
pub extern "C" fn tondo_rt_group_new() -> u64 {
    with_state(State::group_new)
}

/// Transfers one task join into a Group.  The caller retains its own handle
/// until it explicitly releases it, matching the runtime's other ownership
/// transfer boundaries.
pub extern "C" fn tondo_rt_group_add(group: u64, task: u64) -> u64 {
    with_state(|state| state.group_add(group, task))
}

/// Consumes the next completion and returns its payload.  A zero return is
/// disambiguated by `tondo_rt_last_status` and `tondo_rt_group_next_index`.
pub extern "C" fn tondo_rt_group_next(group: u64) -> u64 {
    with_state(|state| state.group_next(group))
}

/// Returns the insertion index selected by the latest successful `next`.
pub extern "C" fn tondo_rt_group_next_index(group: u64) -> u64 {
    with_state(|state| {
        state
            .object(group)
            .and_then(|object| match object {
                Object::Group(group) => group.last_index,
                _ => None,
            })
            .unwrap_or(u64::MAX)
    })
}

/// Returns the number of children still owned by a Group.
pub extern "C" fn tondo_rt_group_remaining(group: u64) -> u64 {
    with_state(|state| match state.object(group) {
        Some(Object::Group(group)) => group.children.len() as u64,
        _ => {
            state.last_status = STATUS_INVALID_HANDLE;
            u64::MAX
        }
    })
}

/// Completes `all`, returning `0` for success or the declared error/panic tag.
/// A first error payload is transferred to `group_last_value`.
pub extern "C" fn tondo_rt_group_all(group: u64) -> u64 {
    with_state(|state| state.group_all(group))
}

/// Completes `settle`, preserving one terminal outcome per child while
/// discarding payload carriers after the caller has observed the status.
pub extern "C" fn tondo_rt_group_settle(group: u64) -> u64 {
    with_state(|state| state.group_settle(group))
}

/// Cancels, drains and consumes every remaining child.
pub extern "C" fn tondo_rt_group_cancel(group: u64) -> u64 {
    with_state(|state| state.group_cancel(group))
}

/// Returns and clears the error payload selected by the latest `all`.
pub extern "C" fn tondo_rt_group_last_value(group: u64) -> u64 {
    with_state(|state| match state.object_mut(group) {
        Some(Object::Group(group)) => std::mem::take(&mut group.last_value),
        _ => {
            state.last_status = STATUS_INVALID_HANDLE;
            0
        }
    })
}

/// Returns the number of scalar terminal outcomes retained by `all`/`settle`.
/// The records are conformance diagnostics, not a public value container.
pub extern "C" fn tondo_rt_group_outcome_count(group: u64) -> u64 {
    with_state(|state| match state.object(group) {
        Some(Object::Group(group)) => group.outcomes.len() as u64,
        _ => {
            state.last_status = STATUS_INVALID_HANDLE;
            u64::MAX
        }
    })
}

/// Returns the stable insertion index of a retained scalar outcome.
pub extern "C" fn tondo_rt_group_outcome_index(group: u64, position: u64) -> u64 {
    with_state(|state| {
        state
            .group_outcome_record(group, position)
            .map(|record| record.index)
            .unwrap_or(u64::MAX)
    })
}

/// Returns the scalar logical payload of a retained outcome. Managed payload
/// handles are intentionally not exposed through this conformance-only view.
pub extern "C" fn tondo_rt_group_outcome_value(group: u64, position: u64) -> u64 {
    with_state(|state| {
        let Some(record) = state.group_outcome_record(group, position) else {
            return u64::MAX;
        };
        if State::valid_handle(record.value) && state.live_handle(record.value) {
            state.last_status = STATUS_INVALID_TRANSITION;
            return u64::MAX;
        }
        record.value
    })
}

/// Returns whether a retained scalar outcome is an error (`1`) or success (`0`).
pub extern "C" fn tondo_rt_group_outcome_is_error(group: u64, position: u64) -> u64 {
    with_state(|state| {
        state
            .group_outcome_record(group, position)
            .map(|record| u64::from(record.is_error))
            .unwrap_or(u64::MAX)
    })
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
                group_owner: None,
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
        if matches!(
            state.object(task),
            Some(Object::Task {
                group_owner: Some(_),
                ..
            })
        ) {
            state.last_status = STATUS_INVALID_TRANSITION;
            return 0;
        }
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
    fn native_blocking_pool_runs_bounded_jobs_and_transfers_payload_once() {
        let _guard = test_guard();
        tondo_rt_reset();
        if !native_blocking_supported() {
            assert_eq!(
                tondo_rt_blocking_pool_new(1, 1),
                0,
                "unsupported targets must not expose the native lane"
            );
            assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_UNSUPPORTED_TARGET);
            return;
        }

        let pool = tondo_rt_blocking_pool_new(1, 2);
        assert_ne!(pool, 0);
        assert_eq!(tondo_rt_blocking_pool_status(pool), 0);
        let job = tondo_rt_blocking_pool_submit(pool, 42);
        assert_ne!(job, 0);
        assert_eq!(tondo_rt_blocking_job_wait(job), STATUS_OK);
        assert_eq!(tondo_rt_blocking_job_status(job), BLOCKING_JOB_COMPLETED);
        assert_eq!(tondo_rt_blocking_job_worker(job), 0);
        assert_eq!(tondo_rt_blocking_job_take(job), 42);
        assert_eq!(tondo_rt_blocking_job_status(job), BLOCKING_JOB_TAKEN);
        assert_eq!(tondo_rt_blocking_job_take(job), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_INVALID_TRANSITION);
        assert_eq!(
            tondo_rt_blocking_job_cancel(job),
            STATUS_BLOCKING_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_blocking_pool_shutdown(pool), STATUS_OK);
        assert_eq!(tondo_rt_blocking_pool_status(pool), 3);
        assert_eq!(
            tondo_rt_blocking_pool_shutdown(pool),
            STATUS_BLOCKING_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_release(job), STATUS_OK);
        assert_eq!(tondo_rt_release(pool), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_blocking_pool_preserves_managed_payloads_and_rejects_invalid_budget() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_blocking_pool_new(0, 1), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_INVALID_WORKERS);
        assert_eq!(tondo_rt_blocking_pool_new(1, -1), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_INVALID_CAPACITY);
        if !native_blocking_supported() {
            return;
        }

        let pool = tondo_rt_blocking_pool_new(1, 1);
        let value = tondo_rt_result_new(RESULT_OK, 7, 1);
        assert_eq!(
            tondo_rt_blocking_pool_submit(HANDLE_BIT | 999_999, value),
            0,
            "invalid pool admission must return no job handle"
        );
        assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_INVALID_HANDLE);
        assert_eq!(
            tondo_rt_blocking_pool_submit(pool, HANDLE_BIT | 999_999),
            0,
            "invalid payload admission must return no job handle"
        );
        assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_INVALID_HANDLE);
        let job = tondo_rt_blocking_pool_submit(pool, value);
        assert_ne!(job, 0);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        assert_eq!(tondo_rt_blocking_job_wait(job), STATUS_OK);
        let transferred = tondo_rt_blocking_job_take(job);
        assert_ne!(transferred, 0);
        assert_eq!(tondo_rt_result_tag(transferred), RESULT_OK);
        assert_eq!(tondo_rt_result_payload(transferred), 7);
        assert_eq!(tondo_rt_blocking_pool_cancel(pool), STATUS_CANCELLED);
        assert_eq!(tondo_rt_blocking_pool_status(pool), 4);
        assert_eq!(tondo_rt_release(job), STATUS_OK);
        assert_eq!(tondo_rt_release(transferred), STATUS_OK);
        assert_eq!(tondo_rt_release(pool), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_blocking_queue_cancellation_is_atomic_before_worker_admission() {
        let _guard = test_guard();
        tondo_rt_reset();
        if !native_blocking_supported() {
            return;
        }
        let pool = NativeBlockingPool::new(1, 1).expect("native target has workers");
        let job = Arc::new(NativeBlockingJobCell {
            state: Mutex::new(NativeBlockingJob {
                payload: 11,
                pool: HANDLE_BIT | 91,
                state: NativeBlockingJobState::Queued,
                worker: u64::MAX,
            }),
            wake: Condvar::new(),
        });
        let (lock, wake) = &*pool.state;
        let mut state = lock.lock().expect("pool state is not poisoned");
        state.queue.push_back(Arc::clone(&job));
        assert_eq!(state.active, 0);
        drop(state);
        assert_eq!(pool.cancel_job(&job), STATUS_OK);
        assert_eq!(
            job.state.lock().expect("job state is not poisoned").state,
            NativeBlockingJobState::Cancelled
        );
        wake.notify_all();
        assert_eq!(pool.shutdown(true), STATUS_CANCELLED);
    }

    #[derive(Debug, Clone, Copy)]
    struct NativeExecutorPerfObservation {
        nanos: u128,
        operations: u64,
        accepted: u64,
        pending: u64,
        waits: u64,
        bridge_events: u64,
        queued_peak: u64,
        active_peak: u64,
        worker_starts: u64,
        logical_memory_bytes: u64,
        live_handles: u64,
    }

    #[derive(Debug, Clone, Copy)]
    enum NativeExecutorPerfWorkload {
        Startup,
        Roundtrip1,
        Roundtrip4,
        Throughput4,
        Saturation1,
        Drain4,
    }

    impl NativeExecutorPerfWorkload {
        fn id(self) -> &'static str {
            match self {
                Self::Startup => "native-startup-1",
                Self::Roundtrip1 => "native-roundtrip-1",
                Self::Roundtrip4 => "native-roundtrip-4",
                Self::Throughput4 => "native-throughput-4",
                Self::Saturation1 => "native-saturation-1",
                Self::Drain4 => "native-drain-4",
            }
        }

        fn operation(self) -> &'static str {
            match self {
                Self::Startup => "startup",
                Self::Roundtrip1 | Self::Roundtrip4 => "roundtrip",
                Self::Throughput4 => "throughput",
                Self::Saturation1 => "saturation",
                Self::Drain4 => "drain",
            }
        }

        fn workers(self) -> usize {
            match self {
                Self::Startup | Self::Roundtrip1 | Self::Saturation1 => 1,
                Self::Roundtrip4 | Self::Throughput4 | Self::Drain4 => 4,
            }
        }

        fn capacity(self) -> usize {
            match self {
                Self::Startup | Self::Roundtrip1 | Self::Saturation1 => 1,
                Self::Roundtrip4 => 4,
                Self::Throughput4 => 32,
                Self::Drain4 => 8,
            }
        }

        fn operations(self) -> usize {
            match self {
                Self::Startup | Self::Roundtrip1 => 1,
                Self::Roundtrip4 => 4,
                Self::Throughput4 => 32,
                Self::Saturation1 | Self::Drain4 => 8,
            }
        }
    }

    const NATIVE_EXECUTOR_PERF_WARMUPS: usize = 3;
    const NATIVE_EXECUTOR_PERF_SAMPLES: usize = 9;
    const NATIVE_EXECUTOR_PERF_WORKLOADS: [NativeExecutorPerfWorkload; 6] = [
        NativeExecutorPerfWorkload::Startup,
        NativeExecutorPerfWorkload::Roundtrip1,
        NativeExecutorPerfWorkload::Roundtrip4,
        NativeExecutorPerfWorkload::Throughput4,
        NativeExecutorPerfWorkload::Saturation1,
        NativeExecutorPerfWorkload::Drain4,
    ];

    fn native_executor_perf_logical_memory_bytes(workers: usize, capacity: usize) -> u64 {
        let limit = if capacity == 0 { workers } else { capacity };
        (std::mem::size_of::<NativeBlockingPoolState>()
            + workers * std::mem::size_of::<std::thread::JoinHandle<()>>()
            + limit * std::mem::size_of::<NativeBlockingJobRef>()) as u64
    }

    fn native_executor_perf_snapshot(pool: u64) -> (usize, usize) {
        with_state(|state| {
            state
                .blocking_pools
                .get(&pool)
                .and_then(|pool| {
                    pool.state
                        .0
                        .lock()
                        .ok()
                        .map(|state| (state.queue.len(), state.active))
                })
                .unwrap_or((0, 0))
        })
    }

    fn native_executor_perf_record_peak(pool: u64, queued_peak: &mut u64, active_peak: &mut u64) {
        let (queued, active) = native_executor_perf_snapshot(pool);
        *queued_peak = (*queued_peak).max(queued as u64);
        *active_peak = (*active_peak).max(active as u64);
    }

    fn native_executor_perf_take(job: u64, waits: &mut u64, bridge_events: &mut u64) {
        assert_eq!(tondo_rt_blocking_job_wait(job), STATUS_OK);
        *waits = waits.saturating_add(1);
        assert_eq!(tondo_rt_blocking_job_status(job), BLOCKING_JOB_COMPLETED);
        assert_eq!(tondo_rt_blocking_job_take(job), 42);
        assert_eq!(tondo_rt_release(job), STATUS_OK);
        *bridge_events = bridge_events.saturating_add(1);
    }

    fn native_executor_perf_sample(
        workload: NativeExecutorPerfWorkload,
    ) -> NativeExecutorPerfObservation {
        let workers = workload.workers();
        let capacity = workload.capacity();
        let operations = workload.operations() as u64;
        let logical_memory_bytes = native_executor_perf_logical_memory_bytes(workers, capacity);
        tondo_rt_reset();

        if matches!(workload, NativeExecutorPerfWorkload::Startup) {
            let start = std::time::Instant::now();
            let pool = tondo_rt_blocking_pool_new(workers as i64, capacity as i64);
            assert_ne!(
                pool, 0,
                "native performance startup must admit the target lane"
            );
            assert_eq!(tondo_rt_blocking_pool_shutdown(pool), STATUS_OK);
            assert_eq!(tondo_rt_blocking_pool_status(pool), 3);
            assert_eq!(tondo_rt_release(pool), STATUS_OK);
            let live_handles = tondo_rt_live_objects();
            let observation = NativeExecutorPerfObservation {
                nanos: start.elapsed().as_nanos().max(1),
                operations,
                accepted: 0,
                pending: 0,
                waits: 0,
                bridge_events: 0,
                queued_peak: 0,
                active_peak: 0,
                worker_starts: workers as u64,
                logical_memory_bytes,
                live_handles,
            };
            tondo_rt_reset();
            return observation;
        }

        let pool = tondo_rt_blocking_pool_new(workers as i64, capacity as i64);
        assert_ne!(pool, 0, "native performance pool must be admitted");
        let mut jobs = Vec::with_capacity(workload.operations());
        let mut accepted = 0_u64;
        let mut pending = 0_u64;
        let mut waits = 0_u64;
        let mut bridge_events = 0_u64;
        let mut queued_peak = 0_u64;
        let mut active_peak = 0_u64;

        let drain = matches!(workload, NativeExecutorPerfWorkload::Drain4);
        let start = if drain {
            while accepted < operations {
                let job = tondo_rt_blocking_pool_submit(pool, 42);
                if job != 0 {
                    accepted += 1;
                    jobs.push(job);
                    native_executor_perf_record_peak(pool, &mut queued_peak, &mut active_peak);
                } else {
                    assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_NOT_READY);
                    pending = pending.saturating_add(1);
                    if let Some(job) = jobs.first().copied() {
                        jobs.remove(0);
                        native_executor_perf_take(job, &mut waits, &mut bridge_events);
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
            std::time::Instant::now()
        } else {
            std::time::Instant::now()
        };

        if !drain {
            while accepted < operations {
                let job = tondo_rt_blocking_pool_submit(pool, 42);
                if job != 0 {
                    accepted += 1;
                    jobs.push(job);
                    native_executor_perf_record_peak(pool, &mut queued_peak, &mut active_peak);
                } else {
                    assert_eq!(tondo_rt_last_status(), STATUS_BLOCKING_NOT_READY);
                    pending = pending.saturating_add(1);
                    if let Some(job) = jobs.first().copied() {
                        jobs.remove(0);
                        native_executor_perf_take(job, &mut waits, &mut bridge_events);
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
        }

        if drain {
            assert_eq!(tondo_rt_blocking_pool_shutdown(pool), STATUS_OK);
        } else {
            for job in jobs.drain(..) {
                native_executor_perf_take(job, &mut waits, &mut bridge_events);
            }
        }
        let nanos = start.elapsed().as_nanos().max(1);
        if drain {
            for job in jobs.drain(..) {
                native_executor_perf_take(job, &mut waits, &mut bridge_events);
            }
        }
        assert_eq!(accepted, operations);
        assert_eq!(bridge_events, operations);
        if drain {
            assert_eq!(tondo_rt_blocking_pool_status(pool), 3);
        } else {
            assert_eq!(tondo_rt_blocking_pool_shutdown(pool), STATUS_OK);
        }
        assert_eq!(tondo_rt_release(pool), STATUS_OK);
        let live_handles = tondo_rt_live_objects();
        tondo_rt_reset();
        NativeExecutorPerfObservation {
            nanos,
            operations,
            accepted,
            pending,
            waits,
            bridge_events,
            queued_peak,
            active_peak,
            worker_starts: workers as u64,
            logical_memory_bytes,
            live_handles,
        }
    }

    #[test]
    fn native_blocking_performance_probe() {
        let _guard = test_guard();
        if !native_blocking_supported() {
            println!("TONDO_EXECUTOR_PERF_UNSUPPORTED\tnative-runtime\tx86_64-unknown-linux-gnu");
            return;
        }
        for _ in 0..NATIVE_EXECUTOR_PERF_WARMUPS {
            for workload in NATIVE_EXECUTOR_PERF_WORKLOADS {
                let _ = native_executor_perf_sample(workload);
            }
        }
        for _ in 0..NATIVE_EXECUTOR_PERF_SAMPLES {
            for workload in NATIVE_EXECUTOR_PERF_WORKLOADS {
                let observation = native_executor_perf_sample(workload);
                println!(
                    "TONDO_EXECUTOR_PERF\tnative-runtime\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    workload.id(),
                    workload.operation(),
                    workload.workers(),
                    workload.capacity(),
                    observation.nanos,
                    observation.operations,
                    observation.accepted,
                    observation.pending,
                    observation.waits,
                    observation.bridge_events,
                    observation.queued_peak,
                    observation.active_peak,
                    observation.worker_starts,
                    observation.logical_memory_bytes,
                    observation.live_handles,
                );
            }
        }
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

    #[test]
    fn hosted_capability_handles_support_partial_io_and_opaque_buffers() {
        let _guard = test_guard();
        tondo_rt_reset();
        let host = tondo_rt_host_open(HOST_CAP_FILESYSTEM);
        assert_ne!(host, 0);
        assert_eq!(tondo_rt_host_status(host), 0);

        let first = tondo_rt_host_read(host, 5);
        assert_eq!(tondo_rt_result_tag(first), RESULT_OK);
        let first_buffer = tondo_rt_result_payload(first);
        assert_eq!(tondo_rt_buffer_len(first_buffer), 5);
        assert_eq!(tondo_rt_buffer_byte(first_buffer, 0), b't' as u64);
        assert_eq!(tondo_rt_buffer_byte(first_buffer, 4), b'o' as u64);
        assert_eq!(tondo_rt_retain(first_buffer), STATUS_OK);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        assert_eq!(tondo_rt_release(first_buffer), STATUS_OK);

        let second = tondo_rt_host_read(host, 1024);
        assert_eq!(tondo_rt_result_tag(second), RESULT_OK);
        let second_buffer = tondo_rt_result_payload(second);
        assert_eq!(
            tondo_rt_buffer_len(second_buffer),
            b"-native-filesystem\n".len() as u64
        );
        assert_eq!(tondo_rt_release(second), STATUS_OK);

        let eof = tondo_rt_host_read(host, 1);
        assert_eq!(tondo_rt_result_tag(eof), RESULT_OK);
        assert_eq!(tondo_rt_buffer_len(tondo_rt_result_payload(eof)), 0);
        assert_eq!(tondo_rt_release(eof), STATUS_OK);

        assert_eq!(tondo_rt_host_close(host), STATUS_OK);
        assert_eq!(tondo_rt_host_status(host), 2);
        assert_eq!(tondo_rt_host_close(host), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_release(host), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn hosted_console_write_is_atomic_and_cancellation_is_terminal() {
        let _guard = test_guard();
        tondo_rt_reset();
        let host = tondo_rt_host_open(HOST_CAP_CONSOLE);
        let byte = tondo_rt_buffer_from_byte(b'X' as u64);
        let write = tondo_rt_host_write(host, byte);
        assert_eq!(tondo_rt_result_tag(write), RESULT_OK);
        assert_eq!(tondo_rt_result_payload(write), 1);
        assert_eq!(tondo_rt_release(write), STATUS_OK);

        let output = tondo_rt_host_output(host);
        assert_eq!(tondo_rt_result_tag(output), RESULT_OK);
        let output_buffer = tondo_rt_result_payload(output);
        assert_eq!(tondo_rt_buffer_len(output_buffer), 1);
        assert_eq!(tondo_rt_buffer_byte(output_buffer, 0), b'X' as u64);
        assert_eq!(tondo_rt_release(output), STATUS_OK);
        assert_eq!(tondo_rt_release(byte), STATUS_OK);

        assert_eq!(tondo_rt_host_cancel(host), STATUS_OK);
        assert_eq!(tondo_rt_host_status(host), 1);
        let cancelled = tondo_rt_host_read(host, 1);
        assert_eq!(tondo_rt_result_tag(cancelled), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(cancelled), STATUS_HOST_CANCELLED);
        assert_eq!(tondo_rt_release(cancelled), STATUS_OK);
        assert_eq!(tondo_rt_host_close(host), STATUS_OK);
        assert_eq!(tondo_rt_release(host), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn hosted_boundary_rejects_unknown_capabilities_stale_handles_and_limits() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_host_open(99), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_HOST_UNSUPPORTED);
        let host = tondo_rt_host_open(HOST_CAP_PROCESS);
        let byte = tondo_rt_buffer_from_byte(1);
        let too_large = tondo_rt_host_read(host, (HOST_MAX_BYTES + 1) as u64);
        assert_eq!(tondo_rt_result_tag(too_large), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(too_large), STATUS_HOST_LIMIT);
        assert_eq!(tondo_rt_release(too_large), STATUS_OK);
        assert_eq!(tondo_rt_buffer_byte(byte, 1), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        let invalid_write = tondo_rt_host_write(0, byte);
        assert_eq!(tondo_rt_result_tag(invalid_write), RESULT_ERR);
        assert_eq!(
            tondo_rt_result_payload(invalid_write),
            STATUS_INVALID_HANDLE
        );
        assert_eq!(tondo_rt_release(invalid_write), STATUS_OK);
        assert_eq!(tondo_rt_release(byte), STATUS_OK);
        assert_eq!(tondo_rt_host_close(host), STATUS_OK);
        let after_close = tondo_rt_host_output(host);
        assert_eq!(tondo_rt_result_tag(after_close), RESULT_OK);
        assert_eq!(tondo_rt_buffer_len(tondo_rt_result_payload(after_close)), 0);
        assert_eq!(tondo_rt_release(after_close), STATUS_OK);
        assert_eq!(tondo_rt_release(host), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_next_preserves_completion_order_and_affine_none() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let first = tondo_rt_task_spawn(10, 1);
        let second = tondo_rt_task_spawn(20, 1);
        assert_eq!(tondo_rt_group_add(group, first), STATUS_OK);
        assert_eq!(tondo_rt_group_add(group, second), STATUS_OK);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        assert_eq!(tondo_rt_release(second), STATUS_OK);
        assert_eq!(tondo_rt_group_next(group), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_NOT_READY);

        assert_eq!(tondo_rt_task_wake(second), STATUS_OK);
        assert_eq!(tondo_rt_group_next(group), 20);
        assert_eq!(tondo_rt_group_next_index(group), 1);
        assert_eq!(tondo_rt_group_remaining(group), 1);
        assert_eq!(tondo_rt_task_wake(first), STATUS_OK);
        assert_eq!(tondo_rt_group_next(group), 10);
        assert_eq!(tondo_rt_group_next_index(group), 0);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_group_next(group), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_OK);
        assert_eq!(tondo_rt_group_cancel(group), STATUS_OK);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_all_records_scalar_outcomes_in_insertion_order() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let first = tondo_rt_task_spawn(1, 0);
        let second = tondo_rt_task_spawn(2, 0);
        assert_eq!(tondo_rt_group_add(group, first), STATUS_OK);
        assert_eq!(tondo_rt_group_add(group, second), STATUS_OK);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        assert_eq!(tondo_rt_release(second), STATUS_OK);
        assert_eq!(tondo_rt_group_all(group), STATUS_OK);
        assert_eq!(tondo_rt_group_outcome_count(group), 2);
        assert_eq!(tondo_rt_group_outcome_index(group, 0), 0);
        assert_eq!(tondo_rt_group_outcome_index(group, 1), 1);
        assert_eq!(tondo_rt_group_outcome_value(group, 0), 1);
        assert_eq!(tondo_rt_group_outcome_value(group, 1), 2);
        assert_eq!(tondo_rt_group_outcome_is_error(group, 0), 0);
        assert_eq!(tondo_rt_group_outcome_is_error(group, 1), 0);
        assert_eq!(tondo_rt_group_outcome_index(group, 2), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_all_selects_lowest_error_and_cancels_pending_siblings() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let pending = tondo_rt_task_spawn(91, 1);
        let later_error = tondo_rt_result_new(RESULT_ERR, 12, 1);
        let error_task = tondo_rt_task_spawn(later_error, 0);
        assert_eq!(tondo_rt_group_add(group, pending), STATUS_OK);
        assert_eq!(tondo_rt_group_add(group, error_task), STATUS_OK);
        assert_eq!(tondo_rt_release(pending), STATUS_OK);
        assert_eq!(tondo_rt_release(error_task), STATUS_OK);

        assert_eq!(tondo_rt_group_all(group), RESULT_ERR);
        assert_eq!(tondo_rt_task_poll(pending), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_group_outcome_count(group), 1);
        assert_eq!(tondo_rt_group_outcome_index(group, 0), 1);
        assert_eq!(tondo_rt_group_outcome_value(group, 0), 12);
        assert_eq!(tondo_rt_group_outcome_is_error(group, 0), 1);
        let error = tondo_rt_group_last_value(group);
        assert_eq!(tondo_rt_result_tag(error), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(error), 12);
        assert_eq!(tondo_rt_release(error), STATUS_OK);
        assert_eq!(tondo_rt_release(later_error), STATUS_OK);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_settle_drains_mixed_results_without_fabricating_cancelled_errors() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let success = tondo_rt_result_new(RESULT_OK, 7, 1);
        let failure = tondo_rt_result_new(RESULT_ERR, 8, 1);
        let success_task = tondo_rt_task_spawn(success, 0);
        let failure_task = tondo_rt_task_spawn(failure, 0);
        assert_eq!(tondo_rt_group_add(group, success_task), STATUS_OK);
        assert_eq!(tondo_rt_group_add(group, failure_task), STATUS_OK);
        assert_eq!(tondo_rt_release(success_task), STATUS_OK);
        assert_eq!(tondo_rt_release(failure_task), STATUS_OK);
        assert_eq!(tondo_rt_group_settle(group), STATUS_OK);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_group_last_value(group), 0);
        assert_eq!(tondo_rt_group_outcome_count(group), 2);
        assert_eq!(tondo_rt_group_outcome_index(group, 0), 0);
        assert_eq!(tondo_rt_group_outcome_index(group, 1), 1);
        assert_eq!(tondo_rt_group_outcome_value(group, 0), 7);
        assert_eq!(tondo_rt_group_outcome_value(group, 1), 8);
        assert_eq!(tondo_rt_group_outcome_is_error(group, 0), 0);
        assert_eq!(tondo_rt_group_outcome_is_error(group, 1), 1);
        assert_eq!(tondo_rt_release(success), STATUS_OK);
        assert_eq!(tondo_rt_release(failure), STATUS_OK);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_panic_drains_cleanup_and_rejects_reuse() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let panicking = tondo_rt_task_spawn(0, 1);
        let sibling = tondo_rt_task_spawn(0, 1);
        assert_eq!(tondo_rt_group_add(group, panicking), STATUS_OK);
        assert_eq!(tondo_rt_group_add(group, sibling), STATUS_OK);
        assert_eq!(tondo_rt_release(panicking), STATUS_OK);
        assert_eq!(tondo_rt_release(sibling), STATUS_OK);
        assert_eq!(tondo_rt_task_panic(panicking), STATUS_OK);
        assert_eq!(tondo_rt_group_next(group), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_PANICKED);
        assert_eq!(tondo_rt_task_poll(sibling), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_group_next(group), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_rejects_invalid_children_and_drops_pending_children_once() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        assert_eq!(tondo_rt_group_add(group, 0), STATUS_INVALID_HANDLE);
        let ready = tondo_rt_task_spawn(5, 0);
        assert_eq!(tondo_rt_task_take(ready), 5);
        assert_eq!(tondo_rt_group_add(group, ready), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_release(ready), STATUS_OK);

        let pending = tondo_rt_task_spawn(6, 1);
        assert_eq!(tondo_rt_group_add(group, pending), STATUS_OK);
        assert_eq!(tondo_rt_release(pending), STATUS_OK);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_task_poll(pending), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_rejects_duplicate_and_external_join_without_sticking() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let pending = tondo_rt_task_spawn(8, 1);
        assert_eq!(tondo_rt_group_add(group, pending), STATUS_OK);
        assert_eq!(
            tondo_rt_group_add(group, pending),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_release(pending), STATUS_OK);

        assert_eq!(tondo_rt_task_wake(pending), STATUS_OK);
        assert_eq!(tondo_rt_task_take(pending), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_group_next(group), 8);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_owns_unobserved_error_until_group_drop() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let error = tondo_rt_result_new(RESULT_ERR, 33, 1);
        let task = tondo_rt_task_spawn(error, 0);
        assert_eq!(tondo_rt_group_add(group, task), STATUS_OK);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_group_all(group), RESULT_ERR);
        // The caller keeps only its original Result ownership.  The Group
        // must release the selected payload even when its getter is skipped.
        assert_eq!(tondo_rt_release(error), STATUS_OK);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_transfer_detaches_a_scoped_join() {
        let _guard = test_guard();
        tondo_rt_reset();
        let scope = tondo_rt_scope_enter();
        let task = tondo_rt_scope_spawn(scope, 44, 0);
        let group = tondo_rt_group_new();
        assert_eq!(tondo_rt_group_add(group, task), STATUS_OK);
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_scope_join(scope, task), STATUS_INVALID_TRANSITION);
        assert_eq!(tondo_rt_group_next(group), 44);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_release(scope), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_rejects_cross_group_and_select_aliases() {
        let _guard = test_guard();
        tondo_rt_reset();
        let first_group = tondo_rt_group_new();
        let second_group = tondo_rt_group_new();
        let selection = tondo_rt_select_begin(1);
        let task = tondo_rt_task_spawn(55, 1);
        assert_eq!(tondo_rt_group_add(first_group, task), STATUS_OK);
        assert_eq!(
            tondo_rt_group_add(second_group, task),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(
            tondo_rt_select_register_join(selection, task),
            STATUS_INVALID_TRANSITION
        );
        assert_eq!(tondo_rt_release(task), STATUS_OK);
        assert_eq!(tondo_rt_task_wake(task), STATUS_OK);
        assert_eq!(tondo_rt_group_next(first_group), 55);
        assert_eq!(tondo_rt_release(selection), STATUS_OK);
        assert_eq!(tondo_rt_release(second_group), STATUS_OK);
        assert_eq!(tondo_rt_release(first_group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_group_waits_for_thread_workers_before_consuming() {
        let _guard = test_guard();
        tondo_rt_reset();
        let group = tondo_rt_group_new();
        let thread = tondo_rt_thread_spawn(66, 1);
        assert_eq!(tondo_rt_group_add(group, thread), STATUS_OK);
        assert_eq!(tondo_rt_group_all(group), STATUS_OK);
        assert_eq!(tondo_rt_thread_worker_status(thread), WORKER_COMPLETED);
        assert_eq!(tondo_rt_group_remaining(group), 0);
        assert_eq!(tondo_rt_release(thread), STATUS_OK);
        assert_eq!(tondo_rt_release(group), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_sync_collections_preserve_order_and_share_state_across_workers() {
        let _guard = test_guard();
        tondo_rt_reset();

        let array = tondo_rt_sync_array_new(2);
        assert!(array & HANDLE_BIT != 0);
        assert_eq!(tondo_rt_sync_array_length(array), 2);
        assert_eq!(tondo_rt_sync_array_is_empty(array), 0);
        let set_result = tondo_rt_sync_array_set(array, 0, 7);
        assert_eq!(tondo_rt_result_tag(set_result), RESULT_OK);
        assert_eq!(tondo_rt_result_payload(set_result), 0);
        assert_eq!(tondo_rt_release(set_result), STATUS_OK);
        let get = tondo_rt_sync_array_get(array, 0);
        assert_eq!(tondo_rt_result_tag(get), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(get), 7);
        assert_eq!(tondo_rt_release(get), STATUS_OK);
        let invalid = tondo_rt_sync_array_set(array, 9, 1);
        assert_eq!(tondo_rt_result_tag(invalid), RESULT_ERR);
        assert_eq!(tondo_rt_last_status(), STATUS_COLLECTION_INVALID);
        assert_eq!(tondo_rt_release(invalid), STATUS_OK);

        let mismatch = tondo_rt_sync_array_compare_exchange(array, 0, 8, 9);
        assert_eq!(tondo_rt_result_tag(mismatch), RESULT_CAS_MISMATCH);
        assert_eq!(tondo_rt_result_payload(mismatch), 7);
        assert_eq!(tondo_rt_last_status(), STATUS_ATOMIC_MISMATCH);
        assert_eq!(tondo_rt_release(mismatch), STATUS_OK);
        let exchanged = tondo_rt_sync_array_compare_exchange(array, 0, 7, 9);
        assert_eq!(tondo_rt_result_tag(exchanged), RESULT_CAS_EXCHANGED);
        assert_eq!(tondo_rt_result_payload(exchanged), 7);
        assert_eq!(tondo_rt_last_status(), STATUS_OK);
        assert_eq!(tondo_rt_release(exchanged), STATUS_OK);

        let array_snapshot = tondo_rt_sync_array_snapshot(array);
        assert_eq!(tondo_rt_sync_array_length(array_snapshot), 2);
        let snapshot_value = tondo_rt_sync_array_get(array_snapshot, 0);
        assert_eq!(tondo_rt_result_payload(snapshot_value), 9);
        assert_eq!(tondo_rt_release(snapshot_value), STATUS_OK);

        // Copying a shared handle keeps one collection identity rather than
        // making a copy-on-write value with independent contents.
        assert_eq!(tondo_rt_mark_shared(array), STATUS_OK);
        assert_eq!(tondo_rt_retain(array), STATUS_OK);
        let shared_copy = tondo_rt_cow_clone(array);
        assert_ne!(shared_copy, array);
        let shared_set = tondo_rt_sync_array_set(shared_copy, 1, 11);
        assert_eq!(tondo_rt_result_tag(shared_set), RESULT_OK);
        assert_eq!(tondo_rt_release(shared_set), STATUS_OK);
        let shared_get = tondo_rt_sync_array_get(array, 1);
        assert_eq!(tondo_rt_result_payload(shared_get), 11);
        assert_eq!(tondo_rt_release(shared_get), STATUS_OK);
        assert_eq!(tondo_rt_release(shared_copy), STATUS_OK);
        assert_eq!(tondo_rt_release(array), STATUS_OK);
        assert_eq!(tondo_rt_release(array), STATUS_OK);
        assert_eq!(tondo_rt_release(array_snapshot), STATUS_OK);

        let map = tondo_rt_sync_map_new();
        let inserted = tondo_rt_sync_map_insert(map, 1, 10);
        assert_eq!(tondo_rt_result_tag(inserted), RESULT_OK);
        assert_eq!(tondo_rt_result_tag(inserted), RESULT_OK);
        assert_eq!(tondo_rt_release(inserted), STATUS_OK);
        let replaced = tondo_rt_sync_map_insert(map, 1, 12);
        assert_eq!(tondo_rt_result_payload(replaced), 10);
        assert_eq!(tondo_rt_release(replaced), STATUS_OK);
        assert_eq!(tondo_rt_sync_map_contains(map, 1), 1);
        let removed = tondo_rt_sync_map_remove(map, 1);
        assert_eq!(tondo_rt_result_tag(removed), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(removed), 12);
        assert_eq!(tondo_rt_release(removed), STATUS_OK);
        assert_eq!(tondo_rt_sync_map_contains(map, 1), 0);
        let absent_cas = tondo_rt_sync_map_compare_exchange(map, 1, 0, 0, 13, 1);
        assert_eq!(tondo_rt_result_tag(absent_cas), RESULT_CAS_EXCHANGED);
        assert_eq!(tondo_rt_last_status(), STATUS_OK);
        assert_eq!(tondo_rt_release(absent_cas), STATUS_OK);
        let map_snapshot = tondo_rt_sync_map_snapshot(map);
        assert_eq!(tondo_rt_sync_map_length(map_snapshot), 1);
        assert_eq!(tondo_rt_release(map_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(map), STATUS_OK);

        let set = tondo_rt_sync_set_new();
        let first_insert = tondo_rt_sync_set_insert(set, 4);
        assert_eq!(tondo_rt_result_payload(first_insert), 1);
        assert_eq!(tondo_rt_release(first_insert), STATUS_OK);
        let duplicate = tondo_rt_sync_set_insert(set, 4);
        assert_eq!(tondo_rt_result_payload(duplicate), 0);
        assert_eq!(tondo_rt_release(duplicate), STATUS_OK);
        assert_eq!(tondo_rt_sync_set_contains(set, 4), 1);
        assert_eq!(tondo_rt_sync_set_remove(set, 4), 1);
        assert_eq!(tondo_rt_sync_set_remove(set, 4), 0);
        let set_snapshot = tondo_rt_sync_set_snapshot(set);
        assert_eq!(tondo_rt_sync_set_length(set_snapshot), 0);
        assert_eq!(tondo_rt_release(set_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(set), STATUS_OK);

        let stack = tondo_rt_sync_stack_new();
        for value in [1, 2] {
            let pushed = tondo_rt_sync_stack_push(stack, value);
            assert_eq!(tondo_rt_result_tag(pushed), RESULT_OK);
            assert_eq!(tondo_rt_release(pushed), STATUS_OK);
        }
        let stack_peek = tondo_rt_sync_stack_peek(stack);
        assert_eq!(tondo_rt_result_payload(stack_peek), 2);
        assert_eq!(tondo_rt_release(stack_peek), STATUS_OK);
        let stack_snapshot = tondo_rt_sync_stack_snapshot(stack);
        let stack_first = tondo_rt_sync_array_get(stack_snapshot, 0);
        assert_eq!(tondo_rt_result_payload(stack_first), 2);
        assert_eq!(tondo_rt_release(stack_first), STATUS_OK);
        assert_eq!(tondo_rt_release(stack_snapshot), STATUS_OK);
        let stack_pop = tondo_rt_sync_stack_pop(stack);
        assert_eq!(tondo_rt_result_payload(stack_pop), 2);
        assert_eq!(tondo_rt_release(stack_pop), STATUS_OK);
        assert_eq!(tondo_rt_release(stack), STATUS_OK);

        let queue = tondo_rt_sync_queue_new();
        for value in [3, 4] {
            let enqueued = tondo_rt_sync_queue_enqueue(queue, value);
            assert_eq!(tondo_rt_result_tag(enqueued), RESULT_OK);
            assert_eq!(tondo_rt_release(enqueued), STATUS_OK);
        }
        let queue_snapshot = tondo_rt_sync_queue_snapshot(queue);
        let queue_first = tondo_rt_sync_array_get(queue_snapshot, 0);
        assert_eq!(tondo_rt_result_payload(queue_first), 3);
        assert_eq!(tondo_rt_release(queue_first), STATUS_OK);
        assert_eq!(tondo_rt_release(queue_snapshot), STATUS_OK);
        let queue_first = tondo_rt_sync_queue_dequeue(queue);
        assert_eq!(tondo_rt_result_payload(queue_first), 3);
        assert_eq!(tondo_rt_release(queue_first), STATUS_OK);

        // The array CAS path and the queue path both make progress from
        // several native workers without touching the global table while the
        // collection lock is held.
        let counter = tondo_rt_sync_array_new(1);
        let workers = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        loop {
                            let observed = tondo_rt_sync_array_get(counter, 0);
                            let value = tondo_rt_result_payload(observed);
                            assert_eq!(tondo_rt_release(observed), STATUS_OK);
                            let exchanged =
                                tondo_rt_sync_array_compare_exchange(counter, 0, value, value + 1);
                            let tag = tondo_rt_result_tag(exchanged);
                            assert_eq!(tondo_rt_release(exchanged), STATUS_OK);
                            if tag == RESULT_CAS_EXCHANGED {
                                break;
                            }
                            assert_eq!(tag, RESULT_CAS_MISMATCH);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("native collection worker must finish");
        }
        let final_value = tondo_rt_sync_array_get(counter, 0);
        assert_eq!(tondo_rt_result_payload(final_value), 400);
        assert_eq!(tondo_rt_release(final_value), STATUS_OK);
        assert_eq!(tondo_rt_release(counter), STATUS_OK);

        let producers = (0..4)
            .map(|worker| {
                std::thread::spawn(move || {
                    for offset in 0..25 {
                        let result = tondo_rt_sync_queue_enqueue(queue, worker * 25 + offset);
                        assert_eq!(tondo_rt_result_tag(result), RESULT_OK);
                        assert_eq!(tondo_rt_release(result), STATUS_OK);
                    }
                })
            })
            .collect::<Vec<_>>();
        for producer in producers {
            producer.join().expect("native queue producer must finish");
        }
        assert_eq!(tondo_rt_sync_queue_length(queue), 101);
        let consumers = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        let result = tondo_rt_sync_queue_dequeue(queue);
                        assert_eq!(tondo_rt_result_tag(result), RESULT_SOME);
                        assert_eq!(tondo_rt_release(result), STATUS_OK);
                    }
                })
            })
            .collect::<Vec<_>>();
        for consumer in consumers {
            consumer.join().expect("native queue consumer must finish");
        }
        assert_eq!(tondo_rt_sync_queue_length(queue), 1);
        let last = tondo_rt_sync_queue_dequeue(queue);
        assert_eq!(tondo_rt_result_tag(last), RESULT_SOME);
        assert_eq!(tondo_rt_release(last), STATUS_OK);
        assert_eq!(tondo_rt_release(queue), STATUS_OK);
        assert_eq!(tondo_rt_sync_array_new(HOST_MAX_BYTES as u64 + 1), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_HOST_LIMIT);

        // Collection capabilities are type-checked by the object table and
        // cannot be reused after their terminal release.
        let atomic = tondo_rt_atomic_new(0);
        assert_eq!(tondo_rt_sync_array_length(atomic), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_release(atomic), STATUS_OK);
        let stale = tondo_rt_sync_array_new(0);
        assert_eq!(tondo_rt_release(stale), STATUS_OK);
        assert_eq!(tondo_rt_sync_array_is_empty(stale), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_sync_cursor_is_finite_ordered_and_generation_safe() {
        let _guard = test_guard();
        tondo_rt_reset();

        // Array cursors keep the fixed horizon but observe a value replacement
        // made before the corresponding slot is read.
        let array = tondo_rt_sync_array_new(2);
        let set = tondo_rt_sync_array_set(array, 0, 11);
        assert_eq!(tondo_rt_release(set), STATUS_OK);
        let cursor = tondo_rt_sync_cursor_start(array);
        assert_ne!(cursor, 0);
        let replacement = tondo_rt_sync_array_set(array, 1, 22);
        assert_eq!(tondo_rt_release(replacement), STATUS_OK);
        let first = tondo_rt_sync_cursor_next(cursor);
        assert_eq!(tondo_rt_result_tag(first), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(first), 11);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        let second = tondo_rt_sync_cursor_next(cursor);
        assert_eq!(tondo_rt_result_payload(second), 22);
        assert_eq!(tondo_rt_release(second), STATUS_OK);
        let end = tondo_rt_sync_cursor_next(cursor);
        assert_eq!(tondo_rt_result_tag(end), RESULT_NONE);
        assert_eq!(tondo_rt_release(end), STATUS_OK);

        // A remove/reinsert receives a new generation and therefore cannot
        // enter a cursor whose horizon was captured before the reinsertion.
        let map = tondo_rt_sync_map_new();
        for (key, value) in [(1, 10), (2, 20)] {
            let inserted = tondo_rt_sync_map_insert(map, key, value);
            assert_eq!(tondo_rt_release(inserted), STATUS_OK);
        }
        let map_cursor = tondo_rt_sync_cursor_start(map);
        let removed = tondo_rt_sync_map_remove(map, 1);
        assert_eq!(tondo_rt_release(removed), STATUS_OK);
        let reinserted = tondo_rt_sync_map_insert(map, 1, 30);
        assert_eq!(tondo_rt_release(reinserted), STATUS_OK);
        let map_value = tondo_rt_sync_cursor_next(map_cursor);
        assert_eq!(tondo_rt_result_payload(map_value), 20);
        assert_eq!(tondo_rt_sync_cursor_key(map_cursor), 2);
        assert_eq!(tondo_rt_release(map_value), STATUS_OK);
        let map_end = tondo_rt_sync_cursor_next(map_cursor);
        assert_eq!(tondo_rt_result_tag(map_end), RESULT_NONE);
        assert_eq!(tondo_rt_release(map_end), STATUS_OK);

        let set = tondo_rt_sync_set_new();
        for value in [4, 5] {
            let inserted = tondo_rt_sync_set_insert(set, value);
            assert_eq!(tondo_rt_release(inserted), STATUS_OK);
        }
        let set_cursor = tondo_rt_sync_cursor_start(set);
        assert_eq!(tondo_rt_sync_set_remove(set, 4), 1);
        let reinserted = tondo_rt_sync_set_insert(set, 6);
        assert_eq!(tondo_rt_release(reinserted), STATUS_OK);
        let set_value = tondo_rt_sync_cursor_next(set_cursor);
        assert_eq!(tondo_rt_result_payload(set_value), 5);
        assert_eq!(tondo_rt_release(set_value), STATUS_OK);
        let set_end = tondo_rt_sync_cursor_next(set_cursor);
        assert_eq!(tondo_rt_result_tag(set_end), RESULT_NONE);
        assert_eq!(tondo_rt_release(set_end), STATUS_OK);

        // Stack is non-destructive and top-down; a push after start is out of
        // horizon while removing the top before `next` simply skips it.
        let stack = tondo_rt_sync_stack_new();
        for value in [1, 2] {
            let pushed = tondo_rt_sync_stack_push(stack, value);
            assert_eq!(tondo_rt_release(pushed), STATUS_OK);
        }
        let stack_cursor = tondo_rt_sync_cursor_start(stack);
        let pushed = tondo_rt_sync_stack_push(stack, 3);
        assert_eq!(tondo_rt_release(pushed), STATUS_OK);
        let popped = tondo_rt_sync_stack_pop(stack);
        assert_eq!(tondo_rt_result_payload(popped), 3);
        assert_eq!(tondo_rt_release(popped), STATUS_OK);
        let top = tondo_rt_sync_cursor_next(stack_cursor);
        assert_eq!(tondo_rt_result_payload(top), 2);
        assert_eq!(tondo_rt_release(top), STATUS_OK);
        let bottom = tondo_rt_sync_cursor_next(stack_cursor);
        assert_eq!(tondo_rt_result_payload(bottom), 1);
        assert_eq!(tondo_rt_release(bottom), STATUS_OK);

        // Queue is FIFO and additions after the horizon are excluded.
        let queue = tondo_rt_sync_queue_new();
        for value in [7, 8] {
            let enqueued = tondo_rt_sync_queue_enqueue(queue, value);
            assert_eq!(tondo_rt_release(enqueued), STATUS_OK);
        }
        let queue_cursor = tondo_rt_sync_cursor_start(queue);
        let enqueued = tondo_rt_sync_queue_enqueue(queue, 9);
        assert_eq!(tondo_rt_release(enqueued), STATUS_OK);
        let front = tondo_rt_sync_cursor_next(queue_cursor);
        assert_eq!(tondo_rt_result_payload(front), 7);
        assert_eq!(tondo_rt_release(front), STATUS_OK);
        let dequeued = tondo_rt_sync_queue_dequeue(queue);
        assert_eq!(tondo_rt_result_payload(dequeued), 7);
        assert_eq!(tondo_rt_release(dequeued), STATUS_OK);
        let next = tondo_rt_sync_cursor_next(queue_cursor);
        assert_eq!(tondo_rt_result_payload(next), 8);
        assert_eq!(tondo_rt_release(next), STATUS_OK);
        let queue_end = tondo_rt_sync_cursor_next(queue_cursor);
        assert_eq!(tondo_rt_result_tag(queue_end), RESULT_NONE);
        assert_eq!(tondo_rt_release(queue_end), STATUS_OK);

        // A cursor keeps its source alive, and stale/wrong handles fail closed.
        assert_eq!(tondo_rt_release(array), STATUS_OK);
        assert_eq!(tondo_rt_sync_array_length(array), 2);
        assert_eq!(tondo_rt_release(cursor), STATUS_OK);
        assert_eq!(tondo_rt_sync_cursor_next(cursor), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_sync_array_length(array), u64::MAX);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        let wrong = tondo_rt_atomic_new(0);
        assert_eq!(tondo_rt_sync_cursor_start(wrong), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_release(wrong), STATUS_OK);
        assert_eq!(tondo_rt_release(map_cursor), STATUS_OK);
        assert_eq!(tondo_rt_release(map), STATUS_OK);
        assert_eq!(tondo_rt_release(set_cursor), STATUS_OK);
        assert_eq!(tondo_rt_release(set), STATUS_OK);
        assert_eq!(tondo_rt_release(stack_cursor), STATUS_OK);
        assert_eq!(tondo_rt_release(stack), STATUS_OK);
        assert_eq!(tondo_rt_release(queue_cursor), STATUS_OK);
        assert_eq!(tondo_rt_release(queue), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_sync_collections_cover_empty_and_invalid_edges() {
        let _guard = test_guard();
        tondo_rt_reset();

        // Every collection entrypoint must fail closed for a valid object of
        // another kind.  Result-producing operations return zero rather than
        // manufacturing an unowned result record.
        let wrong = tondo_rt_atomic_new(0);
        assert_ne!(wrong, 0);
        assert_eq!(tondo_rt_sync_array_length(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_array_is_empty(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_array_get(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_array_set(wrong, 0, 1), 0);
        assert_eq!(tondo_rt_sync_array_compare_exchange(wrong, 0, 0, 1), 0);
        assert_eq!(tondo_rt_sync_array_snapshot(wrong), 0);
        assert_eq!(tondo_rt_sync_map_length(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_map_is_empty(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_map_get(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_map_contains(wrong, 0), u64::MAX);
        assert_eq!(tondo_rt_sync_map_insert(wrong, 0, 1), 0);
        assert_eq!(tondo_rt_sync_map_remove(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_map_compare_exchange(wrong, 0, 0, 0, 1, 1), 0);
        assert_eq!(tondo_rt_sync_map_snapshot(wrong), 0);
        assert_eq!(tondo_rt_sync_set_length(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_set_is_empty(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_set_contains(wrong, 0), u64::MAX);
        assert_eq!(tondo_rt_sync_set_insert(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_set_remove(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_set_snapshot(wrong), 0);
        assert_eq!(tondo_rt_sync_stack_length(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_stack_is_empty(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_stack_push(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_stack_pop(wrong), 0);
        assert_eq!(tondo_rt_sync_stack_peek(wrong), 0);
        assert_eq!(tondo_rt_sync_stack_snapshot(wrong), 0);
        assert_eq!(tondo_rt_sync_queue_length(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_queue_is_empty(wrong), u64::MAX);
        assert_eq!(tondo_rt_sync_queue_enqueue(wrong, 0), 0);
        assert_eq!(tondo_rt_sync_queue_dequeue(wrong), 0);
        assert_eq!(tondo_rt_sync_queue_peek(wrong), 0);
        assert_eq!(tondo_rt_sync_queue_snapshot(wrong), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_INVALID_HANDLE);
        assert_eq!(tondo_rt_release(wrong), STATUS_OK);

        let array = tondo_rt_sync_array_new(0);
        assert_eq!(tondo_rt_sync_array_length(array), 0);
        assert_eq!(tondo_rt_sync_array_is_empty(array), 1);
        let value = tondo_rt_sync_array_get(array, 0);
        assert_eq!(tondo_rt_result_tag(value), RESULT_NONE);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        let invalid = tondo_rt_sync_array_set(array, u64::MAX, 1);
        assert_eq!(tondo_rt_result_tag(invalid), RESULT_ERR);
        assert_eq!(tondo_rt_last_status(), STATUS_COLLECTION_INVALID);
        assert_eq!(tondo_rt_release(invalid), STATUS_OK);
        let snapshot = tondo_rt_sync_array_snapshot(array);
        assert_ne!(snapshot, 0);
        assert_eq!(tondo_rt_sync_array_is_empty(snapshot), 1);
        assert_eq!(tondo_rt_release(snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(array), STATUS_OK);

        let map = tondo_rt_sync_map_new();
        assert_eq!(tondo_rt_sync_map_length(map), 0);
        assert_eq!(tondo_rt_sync_map_is_empty(map), 1);
        let missing = tondo_rt_sync_map_get(map, 1);
        assert_eq!(tondo_rt_result_tag(missing), RESULT_NONE);
        assert_eq!(tondo_rt_release(missing), STATUS_OK);
        assert_eq!(tondo_rt_sync_map_contains(map, 1), 0);
        let absent = tondo_rt_sync_map_remove(map, 1);
        assert_eq!(tondo_rt_result_tag(absent), RESULT_NONE);
        assert_eq!(tondo_rt_release(absent), STATUS_OK);
        let inserted = tondo_rt_sync_map_insert(map, 1, 10);
        assert_eq!(tondo_rt_result_tag(inserted), RESULT_OK);
        assert_eq!(tondo_rt_release(inserted), STATUS_OK);
        let exchanged = tondo_rt_sync_map_compare_exchange(map, 1, 10, 1, 11, 1);
        assert_eq!(tondo_rt_result_tag(exchanged), RESULT_CAS_EXCHANGED);
        assert_eq!(tondo_rt_result_payload(exchanged), 10);
        assert_eq!(tondo_rt_release(exchanged), STATUS_OK);
        let mismatch = tondo_rt_sync_map_compare_exchange(map, 1, 10, 1, 12, 1);
        assert_eq!(tondo_rt_result_tag(mismatch), RESULT_CAS_MISMATCH);
        assert_eq!(tondo_rt_result_payload(mismatch), 11);
        assert_eq!(tondo_rt_release(mismatch), STATUS_OK);
        let remove = tondo_rt_sync_map_compare_exchange(map, 1, 11, 1, 0, 0);
        assert_eq!(tondo_rt_result_tag(remove), RESULT_CAS_EXCHANGED);
        assert_eq!(tondo_rt_release(remove), STATUS_OK);
        let map_snapshot = tondo_rt_sync_map_snapshot(map);
        assert_eq!(tondo_rt_sync_map_is_empty(map_snapshot), 1);
        assert_eq!(tondo_rt_release(map_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(map), STATUS_OK);

        let set = tondo_rt_sync_set_new();
        assert_eq!(tondo_rt_sync_set_length(set), 0);
        assert_eq!(tondo_rt_sync_set_is_empty(set), 1);
        assert_eq!(tondo_rt_sync_set_contains(set, 1), 0);
        assert_eq!(tondo_rt_sync_set_remove(set, 1), 0);
        let inserted = tondo_rt_sync_set_insert(set, 1);
        assert_eq!(tondo_rt_result_payload(inserted), 1);
        assert_eq!(tondo_rt_release(inserted), STATUS_OK);
        let duplicate = tondo_rt_sync_set_insert(set, 1);
        assert_eq!(tondo_rt_result_payload(duplicate), 0);
        assert_eq!(tondo_rt_release(duplicate), STATUS_OK);
        let set_snapshot = tondo_rt_sync_set_snapshot(set);
        assert_eq!(tondo_rt_sync_set_length(set_snapshot), 1);
        assert_eq!(tondo_rt_release(set_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(set), STATUS_OK);

        let stack = tondo_rt_sync_stack_new();
        assert_eq!(tondo_rt_sync_stack_length(stack), 0);
        assert_eq!(tondo_rt_sync_stack_is_empty(stack), 1);
        let value = tondo_rt_sync_stack_pop(stack);
        assert_eq!(tondo_rt_result_tag(value), RESULT_NONE);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        let value = tondo_rt_sync_stack_peek(stack);
        assert_eq!(tondo_rt_result_tag(value), RESULT_NONE);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        let pushed = tondo_rt_sync_stack_push(stack, 7);
        assert_eq!(tondo_rt_result_tag(pushed), RESULT_OK);
        assert_eq!(tondo_rt_release(pushed), STATUS_OK);
        let stack_snapshot = tondo_rt_sync_stack_snapshot(stack);
        assert_eq!(tondo_rt_sync_array_length(stack_snapshot), 1);
        assert_eq!(tondo_rt_release(stack_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(stack), STATUS_OK);

        let queue = tondo_rt_sync_queue_new();
        assert_eq!(tondo_rt_sync_queue_length(queue), 0);
        assert_eq!(tondo_rt_sync_queue_is_empty(queue), 1);
        let value = tondo_rt_sync_queue_dequeue(queue);
        assert_eq!(tondo_rt_result_tag(value), RESULT_NONE);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        let value = tondo_rt_sync_queue_peek(queue);
        assert_eq!(tondo_rt_result_tag(value), RESULT_NONE);
        assert_eq!(tondo_rt_release(value), STATUS_OK);
        let enqueued = tondo_rt_sync_queue_enqueue(queue, 8);
        assert_eq!(tondo_rt_result_tag(enqueued), RESULT_OK);
        assert_eq!(tondo_rt_release(enqueued), STATUS_OK);
        let queue_snapshot = tondo_rt_sync_queue_snapshot(queue);
        assert_eq!(tondo_rt_sync_array_length(queue_snapshot), 1);
        assert_eq!(tondo_rt_release(queue_snapshot), STATUS_OK);
        assert_eq!(tondo_rt_release(queue), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_sync_atomics_are_linearizable_across_threads() {
        let _guard = test_guard();
        tondo_rt_reset();
        let atomic = tondo_rt_atomic_new(0);
        assert!(atomic & HANDLE_BIT != 0);
        assert_eq!(tondo_rt_atomic_load(atomic, 0), 0);
        assert_eq!(tondo_rt_atomic_store(atomic, 1, 2), STATUS_OK);
        assert_eq!(tondo_rt_atomic_swap(atomic, 2, 3), 1);

        let workers = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        loop {
                            let observed = tondo_rt_atomic_load(atomic, 0);
                            let status = tondo_rt_atomic_compare_exchange(
                                atomic,
                                observed,
                                observed.saturating_add(1),
                                3,
                                1,
                            );
                            if status == observed {
                                break;
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("atomic worker must finish");
        }
        assert_eq!(tondo_rt_atomic_load(atomic, 4), 402);
        assert_eq!(tondo_rt_atomic_compare_exchange(atomic, 999, 0, 3, 1), 402);
        assert_eq!(tondo_rt_last_status(), STATUS_ATOMIC_MISMATCH);
        assert_eq!(tondo_rt_atomic_compare_exchange(atomic, 402, 0, 2, 1), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_ATOMIC_INVALID_ORDER);
        assert_eq!(tondo_rt_atomic_load(atomic, 2), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_ATOMIC_INVALID_ORDER);
        assert_eq!(tondo_rt_release(atomic), STATUS_OK);

        let park = tondo_rt_sync_park_new();
        let epoch = tondo_rt_sync_park_epoch(park);
        assert_eq!(tondo_rt_sync_park_wait(park, epoch, 0), STATUS_NOT_READY);
        let waiter = std::thread::spawn(move || tondo_rt_sync_park_wait(park, epoch, u64::MAX));
        for _ in 0..100_000 {
            if tondo_rt_sync_park_waiters(park) == 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert!(tondo_rt_sync_park_waiters(park) >= 1);
        assert_eq!(tondo_rt_sync_park_wake(park, 0), 1);
        assert_eq!(
            waiter.join().expect("parking waiter must finish"),
            STATUS_OK
        );
        assert_eq!(tondo_rt_sync_park_epoch(park), epoch + 1);
        assert_eq!(tondo_rt_release(park), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_channel_bounded_fifo_errors_and_terminal_drain() {
        let _guard = test_guard();
        tondo_rt_reset();
        assert_eq!(tondo_rt_channel_bounded(-1), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_CHANNEL_INVALID_CAPACITY);
        assert_eq!(tondo_rt_channel_bounded((HOST_MAX_BYTES + 1) as i64), 0);
        assert_eq!(tondo_rt_last_status(), STATUS_HOST_LIMIT);

        let channel = tondo_rt_channel_bounded(2);
        let sender = tondo_rt_channel_sender(channel);
        let receiver = tondo_rt_channel_receiver(channel);
        assert!(channel & HANDLE_BIT != 0);
        assert!(sender & HANDLE_BIT != 0);
        assert!(receiver & HANDLE_BIT != 0);
        assert_eq!(tondo_rt_release(channel), STATUS_OK);

        for value in [1, 2] {
            let result = tondo_rt_channel_try_send(sender, value);
            assert_eq!(tondo_rt_result_tag(result), RESULT_OK);
            assert_eq!(tondo_rt_release(result), STATUS_OK);
        }
        let full = tondo_rt_channel_try_send(sender, 3);
        assert_eq!(tondo_rt_result_tag(full), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(full), 3);
        assert_eq!(tondo_rt_last_status(), STATUS_CHANNEL_FULL);
        assert_eq!(tondo_rt_release(full), STATUS_OK);

        for value in [1, 2] {
            let result = tondo_rt_channel_try_receive(receiver);
            assert_eq!(tondo_rt_result_tag(result), RESULT_SOME);
            assert_eq!(tondo_rt_result_payload(result), value);
            assert_eq!(tondo_rt_release(result), STATUS_OK);
        }
        assert_eq!(tondo_rt_channel_try_receive(receiver), RESULT_NONE);
        assert_eq!(tondo_rt_last_status(), STATUS_CHANNEL_EMPTY);

        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        let closed = tondo_rt_channel_try_receive(receiver);
        assert_eq!(tondo_rt_result_tag(closed), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(closed), STATUS_HOST_CLOSED);
        assert_eq!(tondo_rt_release(closed), STATUS_OK);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_channel_drain_next(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let drain_channel = tondo_rt_channel_bounded(2);
        let sender = tondo_rt_channel_sender(drain_channel);
        let receiver = tondo_rt_channel_receiver(drain_channel);
        assert_eq!(tondo_rt_release(drain_channel), STATUS_OK);
        for value in [31, 32] {
            let result = tondo_rt_channel_try_send(sender, value);
            assert_eq!(tondo_rt_result_tag(result), RESULT_OK);
            assert_eq!(tondo_rt_release(result), STATUS_OK);
        }
        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 2);
        assert_eq!(tondo_rt_channel_drain_next(drain), 31);
        assert_eq!(tondo_rt_channel_drain_next(drain), 32);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_channel_drain_next(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }

    #[test]
    fn native_channel_rendezvous_fifo_wakeup_and_close_preserve_payloads() {
        let _guard = test_guard();
        tondo_rt_reset();

        let rendezvous = tondo_rt_channel_bounded(0);
        let sender = tondo_rt_channel_sender(rendezvous);
        let receiver = tondo_rt_channel_receiver(rendezvous);
        assert_eq!(tondo_rt_release(rendezvous), STATUS_OK);
        let waiting_receiver = std::thread::spawn(move || tondo_rt_channel_receive(receiver));
        for _ in 0..100_000 {
            if tondo_rt_channel_waiters(rendezvous) >= 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert!(tondo_rt_channel_waiters(rendezvous) >= 1);
        let sent = tondo_rt_channel_send(sender, 7);
        assert_eq!(tondo_rt_result_tag(sent), RESULT_OK);
        assert_eq!(tondo_rt_release(sent), STATUS_OK);
        let received = waiting_receiver
            .join()
            .expect("rendezvous receiver must finish");
        assert_eq!(tondo_rt_result_tag(received), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(received), 7);
        assert_eq!(tondo_rt_release(received), STATUS_OK);
        assert_eq!(tondo_rt_channel_waiters(rendezvous), 0);
        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let bounded = tondo_rt_channel_bounded(1);
        let sender = tondo_rt_channel_sender(bounded);
        let receiver = tondo_rt_channel_receiver(bounded);
        assert_eq!(tondo_rt_release(bounded), STATUS_OK);
        let first = tondo_rt_channel_try_send(sender, 11);
        assert_eq!(tondo_rt_result_tag(first), RESULT_OK);
        assert_eq!(tondo_rt_release(first), STATUS_OK);
        let pending_sender = std::thread::spawn(move || tondo_rt_channel_send(sender, 12));
        for _ in 0..100_000 {
            if tondo_rt_channel_waiters(bounded) >= 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(tondo_rt_channel_waiters(bounded), 1);
        let received = tondo_rt_channel_receive(receiver);
        assert_eq!(tondo_rt_result_tag(received), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(received), 11);
        assert_eq!(tondo_rt_release(received), STATUS_OK);
        let sent = pending_sender.join().expect("buffered sender must finish");
        assert_eq!(tondo_rt_result_tag(sent), RESULT_OK);
        assert_eq!(tondo_rt_release(sent), STATUS_OK);
        let received = tondo_rt_channel_receive(receiver);
        assert_eq!(tondo_rt_result_tag(received), RESULT_SOME);
        assert_eq!(tondo_rt_result_payload(received), 12);
        assert_eq!(tondo_rt_release(received), STATUS_OK);
        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let closed_channel = tondo_rt_channel_bounded(0);
        let sender = tondo_rt_channel_sender(closed_channel);
        let receiver = tondo_rt_channel_receiver(closed_channel);
        assert_eq!(tondo_rt_release(closed_channel), STATUS_OK);
        let pending_sender = std::thread::spawn(move || tondo_rt_channel_send(sender, 21));
        for _ in 0..100_000 {
            if tondo_rt_channel_waiters(closed_channel) >= 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(tondo_rt_channel_waiters(closed_channel), 1);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        let closed = pending_sender.join().expect("closed sender must wake");
        assert_eq!(tondo_rt_result_tag(closed), RESULT_ERR);
        assert_eq!(tondo_rt_result_payload(closed), 21);
        assert_eq!(tondo_rt_last_status(), STATUS_HOST_CLOSED);
        assert_eq!(tondo_rt_release(closed), STATUS_OK);
        assert_eq!(tondo_rt_channel_waiters(closed_channel), 0);
        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);

        let sender_closed_channel = tondo_rt_channel_bounded(0);
        let sender = tondo_rt_channel_sender(sender_closed_channel);
        let receiver = tondo_rt_channel_receiver(sender_closed_channel);
        assert_eq!(tondo_rt_release(sender_closed_channel), STATUS_OK);
        let pending_receiver = std::thread::spawn(move || tondo_rt_channel_receive(receiver));
        for _ in 0..100_000 {
            if tondo_rt_channel_waiters(sender_closed_channel) >= 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(tondo_rt_channel_waiters(sender_closed_channel), 1);
        assert_eq!(tondo_rt_channel_sender_close(sender), STATUS_OK);
        let closed_receive = pending_receiver
            .join()
            .expect("last sender close must wake receiver");
        assert_eq!(tondo_rt_result_tag(closed_receive), RESULT_NONE);
        assert_eq!(tondo_rt_release(closed_receive), STATUS_OK);
        let drain = tondo_rt_channel_receiver_close(receiver);
        assert_eq!(tondo_rt_channel_drain_len(drain), 0);
        assert_eq!(tondo_rt_release(drain), STATUS_OK);
        assert_eq!(tondo_rt_release(sender), STATUS_OK);
        assert_eq!(tondo_rt_release(receiver), STATUS_OK);
        assert_eq!(tondo_rt_live_objects(), 0);
    }
}
