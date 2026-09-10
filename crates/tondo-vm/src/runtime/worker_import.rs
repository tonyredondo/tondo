//! Result admission while a blocking worker is stopped at an owned host call.

use super::host_import::{HostImportPlanner, HostImportTypes, ImportRecipient, PreparedHostImport};
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestStatus {
    Pending,
    Committed,
    Cancelled,
    Finished,
}

#[derive(Debug)]
struct RequestState {
    status: RequestStatus,
    prepared: Option<PreparedHostImport>,
    _memory: VmMemoryCharge,
}

#[derive(Debug, Clone)]
pub(super) struct WorkerImportControl {
    state: Arc<Mutex<RequestState>>,
}

impl WorkerImportControl {
    pub(super) fn try_cancel(&self) -> Result<bool, VmError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| VmError::invariant("worker import state lock was poisoned"))?;
        if state.status == RequestStatus::Committed {
            // The host owns the linearization point. The worker must wait for
            // its response before resuming or releasing its paused heap.
            return Ok(false);
        }
        if state.status == RequestStatus::Pending {
            state.status = RequestStatus::Cancelled;
        }
        Ok(true)
    }
}

pub(super) struct WorkerImportGuard {
    control: WorkerImportControl,
}

impl WorkerImportGuard {
    pub(super) fn take_prepared(&self) -> Result<Option<PreparedHostImport>, VmError> {
        let mut state = self
            .control
            .state
            .lock()
            .map_err(|_| VmError::invariant("worker import state lock was poisoned"))?;
        match state.status {
            RequestStatus::Committed | RequestStatus::Pending => Ok(state.prepared.take()),
            RequestStatus::Cancelled => {
                Err(VmError::Host("blocking host call was cancelled".into()))
            }
            RequestStatus::Finished => {
                Err(VmError::invariant("worker import response completed twice"))
            }
        }
    }
}

impl Drop for WorkerImportGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.control.state.lock() {
            state.status = RequestStatus::Finished;
            state.prepared.take();
        }
    }
}

/// This context contains capacity and original-account ownership, never VM
/// values or borrowed heap references. The scoped caller stays suspended until
/// its guard finishes. The bridge's immutable program supplies the same typed
/// descriptors to both engines.
#[derive(Debug)]
pub(super) struct PausedHostImport {
    task: usize,
    outcome: BytecodeTypeId,
    budget: VmMemoryBudget,
    capacity: super::super::heap::PausedImportCapacity,
    pub(super) control: WorkerImportControl,
}

pub(crate) const CONTEXT_BYTES: u64 =
    (std::mem::size_of::<PausedHostImport>() + std::mem::size_of::<Mutex<RequestState>>()) as u64;

impl PausedHostImport {
    pub(super) fn guard(&self) -> WorkerImportGuard {
        WorkerImportGuard {
            control: self.control.clone(),
        }
    }
}

pub(super) fn pause_current(
    planner: &mut PendingHostImportPlanner<'_>,
) -> Result<Option<PausedHostImport>, VmError> {
    let (task, outcome) = planner
        .current
        .ok_or_else(|| VmError::invariant("worker import has no current result type"))?;
    let Some(budget) = planner.budget(task) else {
        return Ok(None);
    };
    if planner
        .tasks
        .get(task)
        .is_none_or(|record| record.prepared_host_import.is_some())
    {
        return Err(VmError::invariant(
            "worker import cannot pause a missing or already prepared caller",
        ));
    }
    let capacity = planner
        .heap
        .pause_import_capacity(planner.roots, planner.statistics)?;
    let memory = budget.reserve(CONTEXT_BYTES)?;
    Ok(Some(PausedHostImport {
        task,
        outcome,
        budget,
        capacity,
        control: WorkerImportControl {
            state: Arc::new(Mutex::new(RequestState {
                status: RequestStatus::Pending,
                prepared: None,
                _memory: memory,
            })),
        },
    }))
}

pub(super) struct WorkerHostImportPlanner<'a> {
    pub(super) pending: PendingHostImportPlanner<'a>,
    pub(super) current: PausedHostImport,
}

impl HostImportPlanner for WorkerHostImportPlanner<'_> {
    fn pause_current(&mut self) -> Result<Option<PausedHostImport>, VmError> {
        Err(VmError::invariant(
            "an already paused worker cannot export another heap context",
        ))
    }

    fn prepare(
        &mut self,
        call: u64,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        self.pending.prepare(call, preview)
    }

    fn prepare_current(
        &mut self,
        preview: super::super::VmHostReturnPreview<'_>,
    ) -> Result<Option<PreparedHostImport>, VmError> {
        let state = self
            .current
            .control
            .state
            .lock()
            .map_err(|_| VmError::invariant("worker import state lock was poisoned"))?;
        if state.status != RequestStatus::Pending {
            return Err(VmError::Host(
                "blocking host result is no longer pending".into(),
            ));
        }
        let cost = HostImportTypes {
            program: self.pending.program,
            heap: self.pending.heap,
            limits: self.pending.limits,
            nominal_names: self.pending.nominal_names,
        }
        .host_import_preview_cost(
            self.current.outcome,
            preview,
            Some(&self.current.budget),
        )?;
        let (additional, workspace) = cost.prepared_bytes(self.current.budget.limit())?;
        let mut heap = self.current.budget.reserve(additional)?;
        let workspace = heap.split_off(workspace)?;
        let objects = self.current.capacity.reserve(cost.objects)?;
        Ok(Some(PreparedHostImport {
            ty: self.current.outcome,
            cost,
            heap,
            workspace,
            objects,
            recipient: Some(ImportRecipient::Current {
                task: self.current.task,
            }),
        }))
    }

    fn commit(&mut self, prepared: &mut [Option<PreparedHostImport>]) -> Result<(), VmError> {
        let mut state = self
            .current
            .control
            .state
            .lock()
            .map_err(|_| VmError::invariant("worker import state lock was poisoned"))?;
        if !matches!(
            state.status,
            RequestStatus::Pending | RequestStatus::Committed
        ) {
            return Err(VmError::Host(
                "blocking host call was cancelled before commit".into(),
            ));
        }
        let mut current_index = None;
        for (index, entry) in prepared
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry)))
        {
            if !self.current.capacity.owns(&entry.objects) {
                continue;
            }
            if current_index.is_some()
                || state.prepared.is_some()
                || state.status != RequestStatus::Pending
                || entry.ty != self.current.outcome
                || entry.recipient
                    != Some(ImportRecipient::Current {
                        task: self.current.task,
                    })
                || !entry.heap.budget().same_account(&self.current.budget)
            {
                return Err(VmError::invariant(
                    "worker import recipient changed before commit",
                ));
            }
            current_index = Some(index);
        }
        let current = current_index.and_then(|index| prepared[index].take());
        // Holding the worker's state lock serializes cancellation with the
        // parent's validate-all-then-publish operation. A rejected parent batch
        // leaves the original tokens with the host for ordinary RAII cleanup.
        if let Err(error) = self.pending.commit(prepared) {
            if let Some(index) = current_index {
                prepared[index] = current;
            }
            return Err(error);
        }
        if current.is_some() {
            state.prepared = current;
        }
        state.status = RequestStatus::Committed;
        Ok(())
    }
}
