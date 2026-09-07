//! Bounded phase notifications and external worker deadline supervision.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tondo_compiler::test_backend::{TestExecutionKind, TestPhaseObserver};

pub const FRAME_PREFIX: &[u8] = b"tondo-test-phase/1 ";
pub const MAX_FRAME_BYTES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Body,
    Setup,
    Teardown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub sequence: u64,
    pub id: String,
    pub phase: Option<Phase>,
}

#[derive(Debug)]
struct WorkerPhase {
    sequence: u64,
    id: String,
    timed_out: bool,
    notified: bool,
}

#[derive(Debug, Default)]
pub struct WorkerPhases {
    stack: Mutex<(u64, Vec<WorkerPhase>)>,
    pub requested: Arc<AtomicU64>,
}

fn emit(event: &Event) -> Result<(), String> {
    let mut bytes = FRAME_PREFIX.to_vec();
    bytes.extend(serde_json::to_vec(event).map_err(|error| error.to_string())?);
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("worker phase notification exceeds its bound".into());
    }
    io::stderr()
        .lock()
        .write_all(&bytes)
        .map_err(|error| error.to_string())
}

impl TestPhaseObserver for WorkerPhases {
    fn enter(&self, id: &str, kind: TestExecutionKind) -> Result<(), String> {
        let mut state = self
            .stack
            .lock()
            .map_err(|_| "worker phase lock is poisoned")?;
        state.0 = state
            .0
            .checked_add(1)
            .ok_or("worker phase sequence exhausted")?;
        let sequence = state.0;
        state.1.push(WorkerPhase {
            sequence,
            id: id.into(),
            timed_out: false,
            notified: false,
        });
        emit(&Event {
            sequence,
            id: id.into(),
            phase: Some(match kind {
                TestExecutionKind::Leaf => Phase::Body,
                TestExecutionKind::Suite => Phase::Setup,
            }),
        })
    }

    fn cleanup(&self) -> Result<(), String> {
        let mut state = self
            .stack
            .lock()
            .map_err(|_| "worker phase lock is poisoned")?;
        state.0 = state
            .0
            .checked_add(1)
            .ok_or("worker phase sequence exhausted")?;
        let sequence = state.0;
        let active = state
            .1
            .last_mut()
            .ok_or("suite cleanup has no active phase")?;
        active.timed_out |= self.requested.load(Ordering::Acquire) == active.sequence;
        active.sequence = sequence;
        emit(&Event {
            sequence,
            id: active.id.clone(),
            phase: Some(Phase::Teardown),
        })
    }

    fn finish(&self, id: &str) -> Result<bool, String> {
        let mut state = self
            .stack
            .lock()
            .map_err(|_| "worker phase lock is poisoned")?;
        let active = state.1.pop().ok_or("finished test has no active phase")?;
        if active.id != id {
            return Err("worker phase nesting differs".into());
        }
        emit(&Event {
            sequence: active.sequence,
            id: id.into(),
            phase: None,
        })?;
        Ok(active.timed_out || self.requested.load(Ordering::Acquire) == active.sequence)
    }

    fn timeout_pending(&self) -> bool {
        let sequence = self.requested.load(Ordering::Acquire);
        if sequence == 0 {
            return false;
        }
        self.stack.lock().is_ok_and(|state| {
            state
                .1
                .iter()
                .any(|phase| phase.sequence == sequence && !phase.notified)
        })
    }

    fn take_timeout(&self) -> Option<String> {
        let sequence = self.requested.load(Ordering::Acquire);
        if sequence == 0 {
            return None;
        }
        let mut state = self.stack.lock().ok()?;
        let _ = self
            .requested
            .compare_exchange(sequence, 0, Ordering::AcqRel, Ordering::Acquire);
        let phase = state
            .1
            .iter_mut()
            .find(|phase| phase.sequence == sequence && !phase.notified)?;
        phase.timed_out = true;
        phase.notified = true;
        Some(phase.id.clone())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub body: u64,
    pub setup: u64,
    pub teardown: u64,
}

impl Limits {
    fn duration(self, phase: Phase) -> Duration {
        Duration::from_millis(match phase {
            Phase::Body => self.body,
            Phase::Setup => self.setup,
            Phase::Teardown => self.teardown,
        })
    }
}

#[derive(Debug)]
struct ActivePhase {
    sequence: u64,
    id: String,
    phase: Phase,
    elapsed: Duration,
    since: Instant,
    requested: Option<Instant>,
}

#[derive(Debug)]
pub struct Watchdog {
    limits: Limits,
    stack: Vec<ActivePhase>,
    last_sequence: u64,
    idle_since: Option<Instant>,
    pub expired: BTreeMap<String, Phase>,
}

impl Watchdog {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            stack: Vec::new(),
            last_sequence: 0,
            idle_since: Some(Instant::now()),
            expired: BTreeMap::new(),
        }
    }

    pub fn event(&mut self, event: Event, now: Instant) -> io::Result<()> {
        if event.id.is_empty() {
            return Err(io::Error::other("empty worker phase id"));
        }
        if let Some(phase) = event.phase {
            if event.sequence != self.last_sequence + 1 {
                return Err(io::Error::other("worker phase sequence did not advance"));
            }
            self.last_sequence = event.sequence;
            self.idle_since = None;
            if phase == Phase::Teardown {
                let current = self
                    .stack
                    .pop()
                    .ok_or_else(|| io::Error::other("teardown has no setup"))?;
                if current.id != event.id || current.phase != Phase::Setup {
                    return Err(io::Error::other("teardown does not match its setup"));
                }
            } else if let Some(parent) = self.stack.last_mut() {
                if parent.phase != Phase::Setup
                    || !event.id.starts_with(&(parent.id.clone() + "::"))
                {
                    return Err(io::Error::other("worker phase parent differs"));
                }
                parent.elapsed += now.saturating_duration_since(parent.since);
            }
            self.stack.push(ActivePhase {
                sequence: event.sequence,
                id: event.id,
                phase,
                elapsed: Duration::ZERO,
                since: now,
                requested: None,
            });
        } else {
            let current = self
                .stack
                .pop()
                .ok_or_else(|| io::Error::other("finished phase has no start"))?;
            if current.id != event.id || current.sequence != event.sequence {
                return Err(io::Error::other("finished phase identity differs"));
            }
            if let Some(parent) = self.stack.last_mut() {
                parent.since = now;
            } else {
                self.idle_since = Some(now);
            }
        }
        Ok(())
    }

    /// The supervisor keeps enforcing a finite cleanup grace even while the
    /// worker is inside a non-cooperative host operation or user cleanup.
    pub fn poll(&mut self, now: Instant) -> (Option<u64>, bool) {
        let Some(current) = self.stack.last_mut() else {
            return (None, false);
        };
        if let Some(requested) = current.requested {
            return (
                None,
                now.saturating_duration_since(requested) >= Duration::from_secs(1),
            );
        }
        if current.elapsed + now.saturating_duration_since(current.since)
            >= self.limits.duration(current.phase)
        {
            current.requested = Some(now);
            self.expired.insert(current.id.clone(), current.phase);
            return (Some(current.sequence), false);
        }
        (None, false)
    }

    pub fn idle_expired(&self, now: Instant) -> bool {
        self.idle_since.is_some_and(|started| {
            now.saturating_duration_since(started) >= Duration::from_secs(30)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, id: &str, phase: Option<Phase>) -> Event {
        Event {
            sequence,
            id: id.into(),
            phase,
        }
    }

    #[test]
    fn watchdog_pauses_parents_and_uses_independent_phase_caps() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut watchdog = Watchdog::new(Limits {
            body: 100,
            setup: 50,
            teardown: 200,
        });
        watchdog
            .event(event(1, "suite", Some(Phase::Setup)), at(0))
            .unwrap();
        watchdog
            .event(event(2, "suite::first", Some(Phase::Body)), at(40))
            .unwrap();
        assert_eq!(watchdog.poll(at(139)), (None, false));
        watchdog
            .event(event(2, "suite::first", None), at(139))
            .unwrap();
        watchdog
            .event(event(3, "suite::second", Some(Phase::Body)), at(140))
            .unwrap();
        assert_eq!(watchdog.poll(at(239)), (None, false));
        watchdog
            .event(event(3, "suite::second", None), at(239))
            .unwrap();
        assert_eq!(watchdog.poll(at(247)), (None, false));
        watchdog
            .event(event(4, "suite", Some(Phase::Teardown)), at(248))
            .unwrap();
        assert_eq!(watchdog.poll(at(447)), (None, false));
        assert_eq!(watchdog.poll(at(448)), (Some(4), false));
        assert_eq!(watchdog.poll(at(1447)), (None, false));
        assert_eq!(watchdog.poll(at(1448)), (None, true));
        watchdog.event(event(4, "suite", None), at(1450)).unwrap();
        assert_eq!(watchdog.expired.get("suite"), Some(&Phase::Teardown));
    }

    #[test]
    fn watchdog_rejects_invalid_nesting_and_replayed_transitions() {
        let now = Instant::now();
        for invalid in [
            event(0, "suite", Some(Phase::Setup)),
            event(2, "other", Some(Phase::Body)),
            event(2, "suite", Some(Phase::Body)),
            event(2, "other", Some(Phase::Teardown)),
            event(2, "suite", None),
            event(2, "", Some(Phase::Setup)),
        ] {
            let mut watchdog = Watchdog::new(Limits {
                body: 1,
                setup: 1,
                teardown: 1,
            });
            watchdog
                .event(event(1, "suite", Some(Phase::Setup)), now)
                .unwrap();
            assert!(watchdog.event(invalid, now).is_err());
        }
    }
}
