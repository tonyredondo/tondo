//! Deterministic virtual-time domains used by test attempts.
//!
//! The domain is deliberately independent from wall-clock time and from the
//! host scheduler.  A caller registers tasks, marks them ready/blocked, and
//! schedules local timers.  The coordinator can then settle ready work or
//! advance to an exact deadline.  Filesystem, network and process waits are
//! represented as `External` and can never be advanced automatically.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

pub const TEST_VIRTUAL_TIME_FORMAT: &str = "tondo-test-virtual-time-0.1/1";
pub const P2003_DEADLOCK: &str = "P2003";
pub const P2004_OVERLAP: &str = "P2004";
pub const P2005_RANGE: &str = "P2005";

/// A wait that is safe for automatic virtual-time advancement, or an
/// explicitly external wait which must remain under the real timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WaitKind {
    Timer,
    Join,
    LocalSync,
    External,
}

impl WaitKind {
    pub const fn auto_advanceable(self) -> bool {
        matches!(self, Self::Timer | Self::Join | Self::LocalSync)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskState {
    Ready,
    Blocked(WaitKind),
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerInfo {
    id: String,
    task: String,
    deadline: u64,
    sequence: u64,
}

impl TimerInfo {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn task(&self) -> &str {
        &self.task
    }

    pub const fn deadline(&self) -> u64 {
        self.deadline
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleReport {
    now: u64,
    fired_timers: Vec<String>,
    ready_tasks: Vec<String>,
}

impl SettleReport {
    pub const fn now(&self) -> u64 {
        self.now
    }

    pub fn fired_timers(&self) -> &[String] {
        &self.fired_timers
    }

    pub fn ready_tasks(&self) -> &[String] {
        &self.ready_tasks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoAdvance {
    Quiescent,
    Ready { tasks: Vec<String> },
    Advanced { from: u64, to: u64, timer: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualTimeError {
    EmptyTask,
    DuplicateTask(String),
    UnknownTask(String),
    EmptyTimer,
    DuplicateTimer(String),
    TimerNotFound(String),
    InvalidDuration,
    Overflow,
    ClockRegression { previous: u64, current: u64 },
    SequenceOverflow,
    Deadlock,
    ExternalWait(String),
    Livelock { limit: u32 },
}

impl VirtualTimeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Deadlock | Self::ExternalWait(_) | Self::Livelock { .. } => P2003_DEADLOCK,
            Self::InvalidDuration
            | Self::Overflow
            | Self::ClockRegression { .. }
            | Self::SequenceOverflow => P2005_RANGE,
            Self::EmptyTask
            | Self::DuplicateTask(_)
            | Self::UnknownTask(_)
            | Self::EmptyTimer
            | Self::DuplicateTimer(_)
            | Self::TimerNotFound(_) => "E2100",
        }
    }
}

impl fmt::Display for VirtualTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTask => formatter.write_str("virtual task identity cannot be empty"),
            Self::DuplicateTask(id) => write!(formatter, "virtual task `{id}` is duplicated"),
            Self::UnknownTask(id) => write!(formatter, "virtual task `{id}` is unknown"),
            Self::EmptyTimer => formatter.write_str("virtual timer identity cannot be empty"),
            Self::DuplicateTimer(id) => write!(formatter, "virtual timer `{id}` is duplicated"),
            Self::TimerNotFound(id) => write!(formatter, "virtual timer `{id}` is unknown"),
            Self::InvalidDuration => formatter.write_str("virtual duration is negative"),
            Self::Overflow => formatter.write_str("virtual time overflows its representable range"),
            Self::ClockRegression { previous, current } => {
                write!(
                    formatter,
                    "virtual clock regressed from {previous} to {current}"
                )
            }
            Self::SequenceOverflow => formatter.write_str("virtual scheduling sequence overflowed"),
            Self::Deadlock => {
                formatter.write_str("virtual domain is blocked without a local wake-up")
            }
            Self::ExternalWait(id) => {
                write!(formatter, "task `{id}` waits on an external operation")
            }
            Self::Livelock { limit } => write!(
                formatter,
                "virtual domain exceeded auto-advance limit {limit}"
            ),
        }
    }
}

impl Error for VirtualTimeError {}

/// One deterministic domain.  It is created per attempt/phase and is never
/// shared with another envelope or another retry/repeat iteration.
#[derive(Debug, Clone)]
pub struct VirtualDomain {
    now: u64,
    tasks: BTreeMap<String, TaskState>,
    timers: BTreeMap<String, TimerInfo>,
    ready: VecDeque<String>,
    ready_set: BTreeSet<String>,
    sequence: u64,
    auto_steps: u32,
    max_auto_steps: u32,
}

impl Default for VirtualDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualDomain {
    pub const fn new() -> Self {
        Self {
            now: 0,
            tasks: BTreeMap::new(),
            timers: BTreeMap::new(),
            ready: VecDeque::new(),
            ready_set: BTreeSet::new(),
            sequence: 0,
            auto_steps: 0,
            max_auto_steps: 100_000,
        }
    }

    pub const fn now(&self) -> u64 {
        self.now
    }

    pub const fn max_auto_steps(&self) -> u32 {
        self.max_auto_steps
    }

    pub const fn auto_steps(&self) -> u32 {
        self.auto_steps
    }

    pub const fn set_max_auto_steps(mut self, limit: u32) -> Self {
        self.max_auto_steps = limit;
        self
    }

    pub fn register_task(&mut self, id: impl Into<String>) -> Result<(), VirtualTimeError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(VirtualTimeError::EmptyTask);
        }
        if self.tasks.contains_key(&id) {
            return Err(VirtualTimeError::DuplicateTask(id));
        }
        self.tasks.insert(id.clone(), TaskState::Ready);
        self.ready_set.insert(id.clone());
        self.ready.push_back(id);
        Ok(())
    }

    pub fn task_state(&self, id: &str) -> Result<TaskState, VirtualTimeError> {
        self.tasks
            .get(id)
            .copied()
            .ok_or_else(|| VirtualTimeError::UnknownTask(id.into()))
    }

    pub fn set_ready(&mut self, id: &str) -> Result<(), VirtualTimeError> {
        self.ensure_task(id)?;
        self.tasks.insert(id.into(), TaskState::Ready);
        if self.ready_set.insert(id.into()) {
            self.ready.push_back(id.into());
        }
        Ok(())
    }

    pub fn block(&mut self, id: &str, wait: WaitKind) -> Result<(), VirtualTimeError> {
        self.ensure_task(id)?;
        self.tasks.insert(id.into(), TaskState::Blocked(wait));
        self.remove_ready(id);
        Ok(())
    }

    pub fn complete(&mut self, id: &str) -> Result<(), VirtualTimeError> {
        self.ensure_task(id)?;
        self.tasks.insert(id.into(), TaskState::Completed);
        self.remove_ready(id);
        self.timers.retain(|_, timer| timer.task != id);
        Ok(())
    }

    pub fn schedule_timer(
        &mut self,
        id: impl Into<String>,
        task: &str,
        delay_ns: i128,
    ) -> Result<TimerInfo, VirtualTimeError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(VirtualTimeError::EmptyTimer);
        }
        if self.timers.contains_key(&id) {
            return Err(VirtualTimeError::DuplicateTimer(id));
        }
        self.ensure_task(task)?;
        let delay = duration(delay_ns)?;
        let deadline = self
            .now
            .checked_add(delay)
            .ok_or(VirtualTimeError::Overflow)?;
        let sequence = self.next_sequence()?;
        let timer = TimerInfo {
            id: id.clone(),
            task: task.into(),
            deadline,
            sequence,
        };
        self.timers.insert(id, timer.clone());
        self.block(task, WaitKind::Timer)?;
        Ok(timer)
    }

    pub fn reschedule_timer(
        &mut self,
        id: &str,
        delay_ns: i128,
    ) -> Result<TimerInfo, VirtualTimeError> {
        let old = self
            .timers
            .remove(id)
            .ok_or_else(|| VirtualTimeError::TimerNotFound(id.into()))?;
        self.schedule_timer(id.to_owned(), &old.task, delay_ns)
    }

    pub fn cancel_timer(&mut self, id: &str) -> Result<(), VirtualTimeError> {
        let timer = self
            .timers
            .remove(id)
            .ok_or_else(|| VirtualTimeError::TimerNotFound(id.into()))?;
        if self.task_state(&timer.task)? == TaskState::Blocked(WaitKind::Timer) {
            self.set_ready(&timer.task)?;
        }
        Ok(())
    }

    pub fn pending_timers(&self) -> Vec<TimerInfo> {
        let mut timers = self.timers.values().cloned().collect::<Vec<_>>();
        timers.sort_by_key(|timer| (timer.deadline, timer.sequence, timer.id.clone()));
        timers
    }

    pub fn settle(&mut self) -> Result<SettleReport, VirtualTimeError> {
        let mut fired_timers = Vec::new();
        while let Some(timer) = self.next_due_timer() {
            self.timers.remove(&timer.id);
            if self.tasks.get(&timer.task) == Some(&TaskState::Blocked(WaitKind::Timer)) {
                self.set_ready(&timer.task)?;
            }
            fired_timers.push(timer.id);
        }
        let mut ready_tasks = Vec::new();
        while let Some(task) = self.ready.pop_front() {
            self.ready_set.remove(&task);
            ready_tasks.push(task);
        }
        Ok(SettleReport {
            now: self.now,
            fired_timers,
            ready_tasks,
        })
    }

    pub fn advance(&mut self, duration_ns: i128) -> Result<SettleReport, VirtualTimeError> {
        let duration = duration(duration_ns)?;
        let target = self
            .now
            .checked_add(duration)
            .ok_or(VirtualTimeError::Overflow)?;
        self.advance_to(target)
    }

    pub fn advance_to(&mut self, target: u64) -> Result<SettleReport, VirtualTimeError> {
        if target < self.now {
            return Err(VirtualTimeError::ClockRegression {
                previous: self.now,
                current: target,
            });
        }
        self.now = target;
        self.settle()
    }

    /// Advance exactly once to the next deterministic local deadline.  The
    /// caller must settle and run/complete the awakened tasks before asking
    /// for another automatic step.
    pub fn auto_advance_once(&mut self) -> Result<AutoAdvance, VirtualTimeError> {
        if !self.ready.is_empty() || self.tasks.values().any(|state| *state == TaskState::Ready) {
            let tasks = self
                .tasks
                .iter()
                .filter_map(|(id, state)| (*state == TaskState::Ready).then_some(id.clone()))
                .collect();
            return Ok(AutoAdvance::Ready { tasks });
        }
        let blocked = self
            .tasks
            .iter()
            .filter(|(_, state)| matches!(state, TaskState::Blocked(_)));
        let mut has_local_block = false;
        for (id, state) in blocked {
            let TaskState::Blocked(wait) = state else {
                unreachable!()
            };
            if wait == &WaitKind::External {
                return Err(VirtualTimeError::ExternalWait(id.clone()));
            }
            has_local_block = true;
        }
        let Some(timer) = self.next_timer() else {
            if has_local_block {
                return Err(VirtualTimeError::Deadlock);
            }
            return Ok(AutoAdvance::Quiescent);
        };
        if self.auto_steps >= self.max_auto_steps {
            return Err(VirtualTimeError::Livelock {
                limit: self.max_auto_steps,
            });
        }
        let from = self.now;
        self.auto_steps = self.auto_steps.saturating_add(1);
        self.advance_to(timer.deadline)?;
        Ok(AutoAdvance::Advanced {
            from,
            to: timer.deadline,
            timer: timer.id,
        })
    }

    pub fn auto_advance_until_ready_or_quiescent(
        &mut self,
    ) -> Result<AutoAdvance, VirtualTimeError> {
        loop {
            match self.auto_advance_once()? {
                advanced @ AutoAdvance::Advanced { .. } => {
                    if !self.ready.is_empty() {
                        return Ok(advanced);
                    }
                }
                result => return Ok(result),
            }
        }
    }

    pub fn is_quiescent(&self) -> bool {
        self.ready.is_empty()
            && self
                .tasks
                .values()
                .all(|state| *state == TaskState::Completed)
            && self.timers.is_empty()
    }

    fn ensure_task(&self, id: &str) -> Result<(), VirtualTimeError> {
        if self.tasks.contains_key(id) {
            Ok(())
        } else {
            Err(VirtualTimeError::UnknownTask(id.into()))
        }
    }

    fn next_sequence(&mut self) -> Result<u64, VirtualTimeError> {
        let sequence = self.sequence;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(VirtualTimeError::SequenceOverflow)?;
        Ok(sequence)
    }

    fn remove_ready(&mut self, id: &str) {
        self.ready_set.remove(id);
        self.ready.retain(|queued| queued != id);
    }

    fn next_timer(&self) -> Option<TimerInfo> {
        self.timers
            .values()
            .min_by_key(|timer| (timer.deadline, timer.sequence, timer.id.clone()))
            .cloned()
    }

    fn next_due_timer(&self) -> Option<TimerInfo> {
        self.timers
            .values()
            .filter(|timer| timer.deadline <= self.now)
            .min_by_key(|timer| (timer.deadline, timer.sequence, timer.id.clone()))
            .cloned()
    }
}

fn duration(value: i128) -> Result<u64, VirtualTimeError> {
    if value < 0 {
        return Err(VirtualTimeError::InvalidDuration);
    }
    u64::try_from(value).map_err(|_| VirtualTimeError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_are_exact_and_ties_use_creation_order() {
        let mut domain = VirtualDomain::new();
        domain.register_task("a").unwrap();
        domain.register_task("b").unwrap();
        domain.schedule_timer("second", "b", 10).unwrap();
        domain.schedule_timer("first", "a", 10).unwrap();
        assert_eq!(
            domain.advance(10).unwrap().fired_timers(),
            ["second", "first"]
        );
        assert_eq!(domain.now(), 10);
        assert_eq!(domain.pending_timers(), []);
    }

    #[test]
    fn settle_returns_ready_tasks_without_wall_clock_sleep() {
        let mut domain = VirtualDomain::new();
        domain.register_task("task").unwrap();
        domain.block("task", WaitKind::Timer).unwrap();
        domain.schedule_timer("timer", "task", 0).unwrap();
        let report = domain.settle().unwrap();
        assert_eq!(report.fired_timers(), ["timer"]);
        assert_eq!(report.ready_tasks(), ["task"]);
        assert_eq!(report.now(), 0);
    }

    #[test]
    fn automatic_advance_only_uses_local_waits_and_never_overshoots() {
        let mut domain = VirtualDomain::new();
        domain.register_task("task").unwrap();
        domain.schedule_timer("wake", "task", 25).unwrap();
        assert_eq!(
            domain.auto_advance_once().unwrap(),
            AutoAdvance::Advanced {
                from: 0,
                to: 25,
                timer: "wake".into()
            }
        );
        assert_eq!(domain.now(), 25);
        assert!(matches!(
            domain.auto_advance_once().unwrap(),
            AutoAdvance::Ready { .. }
        ));
        domain.complete("task").unwrap();
        assert_eq!(domain.auto_advance_once().unwrap(), AutoAdvance::Quiescent);
    }

    #[test]
    fn deadlock_and_external_waits_are_distinct_and_coded() {
        let mut deadlocked = VirtualDomain::new();
        deadlocked.register_task("joiner").unwrap();
        deadlocked.block("joiner", WaitKind::Join).unwrap();
        assert_eq!(
            deadlocked.auto_advance_once(),
            Err(VirtualTimeError::Deadlock)
        );
        assert_eq!(VirtualTimeError::Deadlock.code(), P2003_DEADLOCK);

        let mut external = VirtualDomain::new();
        external.register_task("io").unwrap();
        external.block("io", WaitKind::External).unwrap();
        assert_eq!(
            external.auto_advance_once(),
            Err(VirtualTimeError::ExternalWait("io".into()))
        );
    }

    #[test]
    fn duration_range_regression_duplicate_and_unknown_inputs_are_rejected() {
        let mut domain = VirtualDomain::new();
        assert_eq!(domain.register_task(""), Err(VirtualTimeError::EmptyTask));
        domain.register_task("task").unwrap();
        assert_eq!(
            domain.register_task("task"),
            Err(VirtualTimeError::DuplicateTask("task".into()))
        );
        assert_eq!(
            domain.block("missing", WaitKind::Join),
            Err(VirtualTimeError::UnknownTask("missing".into()))
        );
        assert_eq!(
            domain.schedule_timer("negative", "task", -1),
            Err(VirtualTimeError::InvalidDuration)
        );
        assert_eq!(
            domain.schedule_timer("huge", "task", i128::MAX),
            Err(VirtualTimeError::Overflow)
        );
        assert_eq!(
            domain.advance_to(0),
            Ok(SettleReport {
                now: 0,
                fired_timers: vec![],
                ready_tasks: vec!["task".into()]
            })
        );
        assert_eq!(
            domain.advance_to(0),
            Ok(SettleReport {
                now: 0,
                fired_timers: vec![],
                ready_tasks: vec![]
            })
        );
        assert_eq!(domain.advance(-1), Err(VirtualTimeError::InvalidDuration));
        assert_eq!(
            domain.advance_to(0),
            Ok(SettleReport {
                now: 0,
                fired_timers: vec![],
                ready_tasks: vec![]
            })
        );
        assert_eq!(domain.advance_to(1).unwrap().now(), 1);
        assert_eq!(
            domain.advance_to(0),
            Err(VirtualTimeError::ClockRegression {
                previous: 1,
                current: 0
            })
        );
        assert_eq!(
            domain.cancel_timer("missing"),
            Err(VirtualTimeError::TimerNotFound("missing".into()))
        );
    }

    #[test]
    fn cancel_and_reschedule_preserve_isolation_and_livelock_limit() {
        let mut domain = VirtualDomain::new();
        domain.register_task("task").unwrap();
        let first = domain.schedule_timer("timer", "task", 5).unwrap();
        let second = domain.reschedule_timer("timer", 10).unwrap();
        assert_eq!(first.sequence(), 0);
        assert_eq!(second.sequence(), 1);
        assert_eq!(second.deadline(), 10);
        domain.cancel_timer("timer").unwrap();
        assert_eq!(domain.task_state("task"), Ok(TaskState::Ready));
        domain.complete("task").unwrap();
        assert!(domain.is_quiescent());

        let mut limited = VirtualDomain::new().set_max_auto_steps(0);
        limited.register_task("task").unwrap();
        limited.schedule_timer("timer", "task", 1).unwrap();
        assert_eq!(
            limited.auto_advance_once(),
            Err(VirtualTimeError::Livelock { limit: 0 })
        );
    }

    #[test]
    fn display_and_wait_kind_contracts_are_stable() {
        assert!(WaitKind::Timer.auto_advanceable());
        assert!(WaitKind::Join.auto_advanceable());
        assert!(WaitKind::LocalSync.auto_advanceable());
        assert!(!WaitKind::External.auto_advanceable());
        assert!(VirtualTimeError::Deadlock.to_string().contains("blocked"));
        assert_eq!(VirtualTimeError::Overflow.code(), P2005_RANGE);
        assert_eq!(TEST_VIRTUAL_TIME_FORMAT, "tondo-test-virtual-time-0.1/1");
    }
}
