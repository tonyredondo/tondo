//! OS requests and the private worker cancellation channel for `tondo test`.
//!
//! The signal thread records requests and notifies its supervisor. Supervision and output
//! rollback run on ordinary threads, with the same real grace profile as the
//! compiler's interruption coordinator.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tondo_compiler::test_interrupt::{
    CancellationAck, InterruptOrigin, InterruptRequest, InterruptionCoordinator, InterruptionPhase,
    OutputLedger,
};
use tondo_compiler::test_limits::LimitProfile;

#[derive(Default)]
struct Interruption {
    worker: bool,
    completed: Mutex<bool>,
    requested: Arc<AtomicBool>,
    requests: AtomicUsize,
    worker_requests: AtomicUsize,
    isolation_lost: AtomicBool,
    worker_clean: AtomicBool,
}

static INTERRUPTION: OnceLock<Interruption> = OnceLock::new();
pub const WORKER_REQUEST_FRAME: &[u8] = b"tondo-test-worker-interrupt-request/1\n";

pub fn install(worker: bool) -> Result<(), String> {
    INTERRUPTION
        .set(Interruption {
            worker,
            ..Interruption::default()
        })
        .map_err(|_| "test interruption handler already installed".to_owned())?;
    ctrlc::try_set_handler(request)
        .map_err(|error| format!("cannot install test interruption handler: {error}"))
}

pub fn request() {
    if let Some(state) = INTERRUPTION.get() {
        let completed = state
            .completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *completed {
            return;
        }
        let previous = state.requests.fetch_add(1, Ordering::AcqRel);
        state.requested.store(true, Ordering::Release);
        drop(completed);
        if previous < 2 && state.worker {
            // This callback runs on ctrlc's ordinary thread, not inside an OS
            // signal handler. The private frame precedes worker diagnostics.
            let _ = io::stderr().write_all(WORKER_REQUEST_FRAME);
        }
    }
}

pub fn worker_requested_interruption(count: usize) {
    if let Some(state) = INTERRUPTION.get() {
        let completed = state
            .completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*completed {
            state
                .worker_requests
                .fetch_max(count.min(2), Ordering::AcqRel);
            state.requested.store(true, Ordering::Release);
        }
    }
}

/// A supervisor lease closes as a consequence of its accepted request. It is
/// not another external signal and must not force a second-request teardown.
pub fn supervisor_requested_interruption() {
    worker_requested_interruption(1);
}

/// Linearizes successful publication against signal delivery. Requests
/// accepted before this point force rollback; a completed invocation is closed.
pub fn finish_publication() -> bool {
    let Some(state) = INTERRUPTION.get() else {
        return true;
    };
    let mut completed = state
        .completed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.requested.load(Ordering::Acquire) {
        return false;
    }
    *completed = true;
    true
}

pub fn token() -> Arc<AtomicBool> {
    INTERRUPTION.get().map_or_else(
        || Arc::new(AtomicBool::new(false)),
        |state| state.requested.clone(),
    )
}

pub fn requests() -> usize {
    INTERRUPTION.get().map_or(0, |state| {
        state
            .requests
            .load(Ordering::Acquire)
            .max(state.worker_requests.load(Ordering::Acquire))
            .max(usize::from(state.requested.load(Ordering::Acquire)))
    })
}

pub fn isolation_lost() {
    if let Some(state) = INTERRUPTION.get() {
        state.isolation_lost.store(true, Ordering::Release);
    }
}

/// An unusable isolation provider aborts the invocation even without an OS
/// signal. Stop dispatch, cancel peer workers and prevent output publication.
pub fn abort_isolation(message: &str) {
    eprintln!("tondo test: {message}");
    if let Some(state) = INTERRUPTION.get() {
        state.isolation_lost.store(true, Ordering::Release);
        state.requested.store(true, Ordering::Release);
    }
}

pub fn worker_clean() {
    if let Some(state) = INTERRUPTION.get() {
        state.worker_clean.store(true, Ordering::Release);
    }
}

pub fn exit_code(worker: bool) -> Option<u8> {
    let state = INTERRUPTION.get()?;
    state.requested.load(Ordering::Acquire).then(|| {
        if state.isolation_lost.load(Ordering::Acquire)
            || (worker && !state.worker_clean.load(Ordering::Acquire))
        {
            3
        } else {
            4
        }
    })
}

/// One active worker session in the invocation. Real process exit and pipe
/// closure are required before the model's cleanup acknowledgement is used.
pub struct WorkerCancellation {
    coordinator: InterruptionCoordinator,
    started: Instant,
    observed: usize,
}

impl WorkerCancellation {
    pub fn new() -> Self {
        Self {
            coordinator: InterruptionCoordinator::new(
                LimitProfile::default(),
                ["worker".into()],
                OutputLedger::new([]),
                0,
            )
            .expect("the fixed worker cancellation profile is valid"),
            started: Instant::now(),
            observed: 0,
        }
    }

    pub fn poll(&mut self, requests: usize) -> (bool, bool) {
        let now = self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let first = self.observed == 0 && requests > 0;
        // Only first and second requests have distinct semantics.
        for _ in self.observed..requests.min(2) {
            self.coordinator
                .request(
                    InterruptRequest::new(
                        InterruptOrigin::Supervisor,
                        "external test interruption",
                    )
                    .expect("fixed interruption reason"),
                    now,
                )
                .expect("monotonic worker interruption request");
        }
        self.observed = requests.min(2);
        let forced = self
            .coordinator
            .poll(now)
            .expect("monotonic worker grace clock")
            == InterruptionPhase::LostIsolation;
        (first, forced)
    }

    pub fn requested(&self) -> bool {
        self.observed > 0
    }

    pub fn close(&mut self) {
        self.coordinator
            .acknowledge_cancel("worker", CancellationAck::complete())
            .expect("reaped worker completed cancellation");
        self.coordinator
            .close_worker("worker")
            .expect("acknowledged worker session closes");
    }
}
