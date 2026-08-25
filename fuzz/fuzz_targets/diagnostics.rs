#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_vm::bytecode::BytecodeSpan;
use tondo_vm::runtime::{
    DiagnosticConfig, DiagnosticEvent, DiagnosticHeapOperation, DiagnosticMemoryAccess,
    DiagnosticQuiescencePhase, DiagnosticResourceState, DiagnosticSource, DiagnosticTaskState,
    DiagnosticTrace, DumpIdentity, DumpTermination, capture_dump, detect_leaks, detect_races,
};

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(65_536)];
    let mode = match input.first().copied().unwrap_or(0) {
        b'R' => 0,
        b'L' => 1,
        _ => 2,
    };
    let positive = input.get(1).copied().unwrap_or(0) & 1 == 1;
    let trace = match mode {
        0 => race_trace(positive, input),
        1 => leak_trace(positive, input),
        _ => dump_trace(input),
    };

    let first_race = detect_races(&trace);
    let second_race = detect_races(&trace);
    assert_eq!(first_race, second_race);
    let first_leak = detect_leaks(&trace);
    let second_leak = detect_leaks(&trace);
    assert_eq!(first_leak, second_leak);

    if mode == 0 && positive {
        assert!(first_race.has_findings());
    }
    if mode == 0 && !positive {
        assert!(first_race.is_clean());
    }
    if mode == 1 && positive {
        assert!(first_leak.has_findings());
    }
    if mode == 1 && !positive {
        assert!(first_leak.is_clean());
    }

    let identity = DumpIdentity {
        run_id: "fuzz-run".into(),
        attempt_id: format!("attempt-{}", input.len()),
        shard: "0/1".into(),
        profile: "crash".into(),
        target: "fuzz".into(),
        backend: "bytecode-vm".into(),
        toolchain: "nightly-2026-07-28".into(),
        source_revision: "fuzz".into(),
    };
    let termination = DumpTermination {
        reason: "returned".into(),
        program_exit_status: Some(0),
        command_exit_status: Some(0),
    };
    let encoded = capture_dump(&trace, identity, termination).unwrap();
    let analysis = tondo_vm::runtime::analyze_dump(&encoded).unwrap();
    assert!(analysis.task_count <= 2);
});

fn source(offset: u8) -> DiagnosticSource {
    DiagnosticSource {
        function: "fuzz".into(),
        span: BytecodeSpan {
            file: 1,
            start: u32::from(offset),
            end: u32::from(offset).saturating_add(1),
        },
    }
}

fn base_trace(events: Vec<DiagnosticEvent>) -> DiagnosticTrace {
    DiagnosticTrace {
        format: "tondo-diagnostic-runtime/1",
        config: DiagnosticConfig::default(),
        events,
        scheduler_tail: Vec::new(),
        roots: Vec::new(),
        resources: Vec::new(),
        source_maps: vec![source(0)],
        events_seen: 0,
        truncated: false,
    }
}

fn task(id: u64, parent: Option<u64>, state: DiagnosticTaskState) -> DiagnosticEvent {
    DiagnosticEvent::Task {
        id,
        parent,
        state,
        stack: vec![source(0)],
    }
}

fn race_trace(positive: bool, _input: &[u8]) -> DiagnosticTrace {
    let mut events = vec![task(1, None, DiagnosticTaskState::Created)];
    if positive {
        events.extend([
            task(2, Some(1), DiagnosticTaskState::Created),
            DiagnosticEvent::Synchronization {
                task_id: 2,
                operation: tondo_vm::runtime::DiagnosticSynchronization::Spawn,
                peer: Some(1),
                source: Some(source(0)),
            },
            DiagnosticEvent::Memory {
                access: DiagnosticMemoryAccess::Write,
                range: range(1, 7),
                source: source(0),
                stack: vec![source(0)],
            },
            DiagnosticEvent::Memory {
                access: DiagnosticMemoryAccess::Read,
                range: range(2, 7),
                source: source(0),
                stack: vec![source(0)],
            },
        ]);
    } else {
        events.extend([
            DiagnosticEvent::Memory {
                access: DiagnosticMemoryAccess::Read,
                range: range(1, 7),
                source: source(0),
                stack: vec![source(0)],
            },
            DiagnosticEvent::Memory {
                access: DiagnosticMemoryAccess::Read,
                range: range(1, 7),
                source: source(0),
                stack: vec![source(0)],
            },
        ]);
    }
    base_trace(events)
}

fn range(task_id: u64, storage_id: u64) -> tondo_vm::runtime::DiagnosticRange {
    tondo_vm::runtime::DiagnosticRange {
        task_id,
        frame: 0,
        slot: 0,
        projections: 0,
        storage_id: Some(storage_id),
        path_hash: 0,
    }
}

fn leak_trace(positive: bool, input: &[u8]) -> DiagnosticTrace {
    let mut events = Vec::new();
    let first = u64::from(input.get(2).copied().unwrap_or(1).max(1));
    for (index, object_id) in if positive {
        vec![first, first + 1, first + 2]
    } else {
        vec![first]
    }
    .into_iter()
    .enumerate()
    {
        events.push(DiagnosticEvent::Heap {
            object_id,
            operation: DiagnosticHeapOperation::Allocate,
            bytes: 8 + u64::try_from(index).unwrap_or(0),
            owner_task: 1,
            source: Some(source(0)),
            stack: vec![source(0)],
        });
        events.push(DiagnosticEvent::Quiescence {
            task_id: 1,
            phase: DiagnosticQuiescencePhase::Begin,
        });
        events.push(DiagnosticEvent::Roots {
            task_id: 1,
            object_ids: if positive {
                (0..=index).map(|offset| first + offset as u64).collect()
            } else {
                Vec::new()
            },
            retainers: Vec::new(),
        });
        events.push(DiagnosticEvent::Quiescence {
            task_id: 1,
            phase: DiagnosticQuiescencePhase::End,
        });
    }
    if !positive {
        events.push(DiagnosticEvent::Resource {
            id: first,
            kind: "File".into(),
            state: DiagnosticResourceState::Released,
            owner_task: 1,
            source: None,
            stack: Vec::new(),
        });
    }
    base_trace(events)
}

fn dump_trace(input: &[u8]) -> DiagnosticTrace {
    base_trace(vec![
        DiagnosticEvent::Thread {
            id: u64::from(input.first().copied().unwrap_or(0)),
            state: tondo_vm::runtime::DiagnosticThreadState::Started,
        },
        task(1, None, DiagnosticTaskState::Complete),
    ])
}
