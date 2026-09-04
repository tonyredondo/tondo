use std::collections::BTreeSet;
use std::hint::black_box;
use std::mem::size_of;
use std::thread;
use std::time::Instant;

use super::*;

const EXECUTOR_PERF_WARMUPS: usize = 3;
const EXECUTOR_PERF_SAMPLES: usize = 9;

#[derive(Debug, Clone, Copy)]
struct ExecutorPerfObservation {
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
enum HostedWorkload {
    Startup,
    Roundtrip1,
    Roundtrip4,
    Throughput4,
    Saturation1,
    Drain4,
}

impl HostedWorkload {
    fn id(self) -> &'static str {
        match self {
            Self::Startup => "hosted-startup-1",
            Self::Roundtrip1 => "hosted-roundtrip-1",
            Self::Roundtrip4 => "hosted-roundtrip-4",
            Self::Throughput4 => "hosted-throughput-4",
            Self::Saturation1 => "hosted-saturation-1",
            Self::Drain4 => "hosted-drain-4",
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
            Self::Startup => 1,
            Self::Roundtrip1 => 1,
            Self::Roundtrip4 => 4,
            Self::Throughput4 => 32,
            Self::Saturation1 => 8,
            Self::Drain4 => 8,
        }
    }
}

const HOSTED_WORKLOADS: [HostedWorkload; 6] = [
    HostedWorkload::Startup,
    HostedWorkload::Roundtrip1,
    HostedWorkload::Roundtrip4,
    HostedWorkload::Throughput4,
    HostedWorkload::Saturation1,
    HostedWorkload::Drain4,
];

fn hosted_logical_memory_bytes(workers: usize, capacity: usize) -> u64 {
    let limit = if capacity == 0 { workers } else { capacity };
    (size_of::<BlockingBridgeState>()
        + workers * size_of::<thread::JoinHandle<()>>()
        + limit * size_of::<BlockingJob>()) as u64
}

fn bridge_snapshot(bridge: &BlockingExecutionBridge) -> (usize, usize) {
    bridge
        .state
        .0
        .lock()
        .map(|state| (state.queue.len(), state.active))
        .unwrap_or((0, 0))
}

fn record_bridge_peak(
    bridge: &BlockingExecutionBridge,
    queued_peak: &mut u64,
    active_peak: &mut u64,
) {
    let (queued, active) = bridge_snapshot(bridge);
    *queued_peak = (*queued_peak).max(queued as u64);
    *active_peak = (*active_peak).max(active as u64);
}

fn validate_hosted_completion(completion: BlockingCompletion) -> Result<(), VmError> {
    match completion {
        BlockingCompletion::Returned(RuntimeValue::ResultOk(value))
            if *value == RuntimeValue::Integer(42) =>
        {
            Ok(())
        }
        BlockingCompletion::Panicked(panic) => Err(VmError::invariant(format!(
            "executor performance job panicked: {}",
            panic.message
        ))),
        BlockingCompletion::Failed(error) => Err(VmError::invariant(format!(
            "executor performance job failed: {error:?}"
        ))),
        BlockingCompletion::Cancelled => {
            Err(VmError::invariant("executor performance job was cancelled"))
        }
        completion => Err(VmError::invariant(format!(
            "executor performance job returned an unexpected completion: {completion:?}"
        ))),
    }
}

fn collect_hosted_completions(
    bridge: &BlockingExecutionBridge,
    jobs: &[u64],
    completed: &mut BTreeSet<u64>,
    bridge_events: &mut u64,
) -> Result<(), VmError> {
    for job in jobs {
        if completed.contains(job) {
            continue;
        }
        if let Some(completion) = bridge.poll(*job)? {
            validate_hosted_completion(completion)?;
            completed.insert(*job);
            *bridge_events = bridge_events.saturating_add(1);
        }
    }
    Ok(())
}

fn finish_hosted_jobs(
    bridge: &BlockingExecutionBridge,
    jobs: &[u64],
    completed: &mut BTreeSet<u64>,
    waits: &mut u64,
    bridge_events: &mut u64,
) -> Result<(), VmError> {
    while completed.len() < jobs.len() {
        let before = completed.len();
        bridge.wait()?;
        *waits = waits.saturating_add(1);
        collect_hosted_completions(bridge, jobs, completed, bridge_events)?;
        if completed.len() == before {
            thread::yield_now();
        }
    }
    Ok(())
}

fn finish_hosted_shutdown(
    bridge: &BlockingExecutionBridge,
    jobs: &[u64],
    completed: &mut BTreeSet<u64>,
    waits: &mut u64,
    bridge_events: &mut u64,
) -> Result<(), VmError> {
    bridge.shutdown()?;
    while bridge.lifecycle()? != RuntimePoolLifecycle::Closed || completed.len() < jobs.len() {
        bridge.wait()?;
        *waits = waits.saturating_add(1);
        collect_hosted_completions(bridge, jobs, completed, bridge_events)?;
        thread::yield_now();
    }
    Ok(())
}

fn submit_hosted_job(
    bridge: &BlockingExecutionBridge,
    jobs: &mut Vec<u64>,
    queued_peak: &mut u64,
    active_peak: &mut u64,
) -> Result<BlockingAdmission, VmError> {
    let admission = bridge.submit(BytecodeFunctionId::new(2), Vec::new())?;
    if let BlockingAdmission::Accepted(job) = admission {
        jobs.push(job);
        record_bridge_peak(bridge, queued_peak, active_peak);
    }
    Ok(admission)
}

fn hosted_sample(workload: HostedWorkload) -> Result<ExecutorPerfObservation, VmError> {
    hosted_sample_with_operations(workload, workload.operations())
}

fn hosted_sample_with_operations(
    workload: HostedWorkload,
    operation_count: usize,
) -> Result<ExecutorPerfObservation, VmError> {
    let workers = workload.workers();
    let capacity = workload.capacity();
    let operations = operation_count as u64;
    let logical_memory_bytes = hosted_logical_memory_bytes(workers, capacity);

    if matches!(workload, HostedWorkload::Startup) {
        let start = Instant::now();
        let (program, _) = executor_program();
        let trace = derive_trace_metadata(&program)?;
        let bridge = BlockingExecutionBridge::new(
            &program,
            &trace,
            pressure_limits(),
            ValueCopyStrategy::default(),
            workers,
            capacity,
        )?;
        let mut waits = 0;
        let mut completed = BTreeSet::new();
        let mut bridge_events = 0;
        finish_hosted_shutdown(&bridge, &[], &mut completed, &mut waits, &mut bridge_events)?;
        drop(bridge);
        return Ok(ExecutorPerfObservation {
            nanos: start.elapsed().as_nanos().max(1),
            operations,
            accepted: 0,
            pending: 0,
            waits,
            bridge_events,
            queued_peak: 0,
            active_peak: 0,
            worker_starts: workers as u64,
            logical_memory_bytes,
            live_handles: 0,
        });
    }

    let (program, _) = executor_program();
    let trace = derive_trace_metadata(&program)?;
    let bridge = BlockingExecutionBridge::new(
        &program,
        &trace,
        pressure_limits(),
        ValueCopyStrategy::default(),
        workers,
        capacity,
    )?;
    let mut jobs = Vec::with_capacity(operation_count);
    let mut completed = BTreeSet::new();
    let mut waits = 0_u64;
    let mut bridge_events = 0;
    let mut pending = 0_u64;
    let mut queued_peak = 0;
    let mut active_peak = 0;

    if matches!(workload, HostedWorkload::Drain4) {
        for _ in 0..operation_count {
            loop {
                match submit_hosted_job(&bridge, &mut jobs, &mut queued_peak, &mut active_peak)? {
                    BlockingAdmission::Accepted(_) => break,
                    BlockingAdmission::Pending => {
                        pending = pending.saturating_add(1);
                        bridge.wait()?;
                        waits = waits.saturating_add(1);
                        collect_hosted_completions(
                            &bridge,
                            &jobs,
                            &mut completed,
                            &mut bridge_events,
                        )?;
                    }
                    BlockingAdmission::Closed | BlockingAdmission::Cancelled => {
                        return Err(VmError::invariant(
                            "executor performance drain admission became terminal",
                        ));
                    }
                }
            }
        }
        let start = Instant::now();
        finish_hosted_shutdown(
            &bridge,
            &jobs,
            &mut completed,
            &mut waits,
            &mut bridge_events,
        )?;
        let nanos = start.elapsed().as_nanos().max(1);
        assert_eq!(completed.len(), jobs.len());
        drop(bridge);
        return Ok(ExecutorPerfObservation {
            nanos,
            operations,
            accepted: jobs.len() as u64,
            pending,
            waits,
            bridge_events,
            queued_peak,
            active_peak,
            worker_starts: workers as u64,
            logical_memory_bytes,
            live_handles: 0,
        });
    }

    let start = Instant::now();
    while jobs.len() < operation_count {
        match submit_hosted_job(&bridge, &mut jobs, &mut queued_peak, &mut active_peak)? {
            BlockingAdmission::Accepted(_) => {}
            BlockingAdmission::Pending => {
                pending = pending.saturating_add(1);
                bridge.wait()?;
                waits = waits.saturating_add(1);
                collect_hosted_completions(&bridge, &jobs, &mut completed, &mut bridge_events)?;
            }
            BlockingAdmission::Closed | BlockingAdmission::Cancelled => {
                return Err(VmError::invariant(
                    "executor performance admission became terminal",
                ));
            }
        }
    }
    finish_hosted_jobs(
        &bridge,
        &jobs,
        &mut completed,
        &mut waits,
        &mut bridge_events,
    )?;
    let nanos = start.elapsed().as_nanos().max(1);
    finish_hosted_shutdown(
        &bridge,
        &jobs,
        &mut completed,
        &mut waits,
        &mut bridge_events,
    )?;
    assert_eq!(completed.len(), jobs.len());
    black_box(&jobs);
    drop(bridge);
    Ok(ExecutorPerfObservation {
        nanos,
        operations,
        accepted: jobs.len() as u64,
        pending,
        waits,
        bridge_events,
        queued_peak,
        active_peak,
        worker_starts: workers as u64,
        logical_memory_bytes,
        live_handles: 0,
    })
}

fn print_hosted_observation(workload: HostedWorkload, observation: ExecutorPerfObservation) {
    println!(
        "TONDO_EXECUTOR_PERF\thosted-vm\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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

#[test]
fn executor_performance_probe() {
    for _ in 0..EXECUTOR_PERF_WARMUPS {
        for workload in HOSTED_WORKLOADS {
            hosted_sample(workload).expect("hosted executor performance warmup should pass");
        }
    }
    for _ in 0..EXECUTOR_PERF_SAMPLES {
        for workload in HOSTED_WORKLOADS {
            let observation =
                hosted_sample(workload).expect("hosted executor performance sample should pass");
            print_hosted_observation(workload, observation);
        }
    }

    let forced_backpressure = hosted_sample_with_operations(HostedWorkload::Drain4, 16)
        .expect("hosted executor backpressure edge should pass");
    assert_eq!(forced_backpressure.operations, 16);
    assert_eq!(forced_backpressure.accepted, 16);
    assert!(forced_backpressure.pending > 0);
    assert_eq!(forced_backpressure.bridge_events, 16);
    assert!(hosted_logical_memory_bytes(2, 0) > hosted_logical_memory_bytes(2, 1));

    let panic = VmPanic {
        code: PanicCode::ExplicitPanic,
        message: "performance edge".into(),
        span: BytecodeSpan {
            file: 0,
            start: 0,
            end: 0,
        },
        stack: Vec::new(),
        suppressed: Vec::new(),
    };
    for completion in [
        BlockingCompletion::Panicked(panic),
        BlockingCompletion::Failed(VmError::Host("performance edge".into())),
        BlockingCompletion::Cancelled,
        BlockingCompletion::Returned(RuntimeValue::Integer(7)),
    ] {
        assert!(validate_hosted_completion(completion).is_err());
    }
}
