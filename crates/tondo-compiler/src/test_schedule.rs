//! Deterministic test ordering, global job admission and virtual wake queues.
//!
//! The scheduler is intentionally pure. Selection and sharding happen before
//! this module; this boundary only validates the selected tree, computes the
//! canonical/random priority, exposes a structural dispatch plan and models
//! the deterministic queue rules used by virtual-time test domains.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use sha2::{Digest, Sha256};

pub const TEST_SCHEDULE_FORMAT: &str = "tondo-test-schedule-draft/1";
pub const CANONICAL_ORDER_ALGORITHM: &str = "id-byte-order-v1";
pub const RANDOM_ORDER_ALGORITHM: &str = "sha256-tree-v1";
const ORDER_DOMAIN: &[u8] = b"tondo-test-order-v1\0";

/// A seed normalized to the exact sixteen lowercase hexadecimal digits used
/// by reports and replay commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seed(u64);

impl Seed {
    pub fn parse(value: &str) -> Result<Self, SeedError> {
        if value.is_empty() {
            return Err(SeedError::Empty);
        }
        if value.len() > 16 {
            return Err(SeedError::TooLong);
        }
        let mut result = 0_u64;
        for byte in value.bytes() {
            let digit = match byte {
                b'0'..=b'9' => u64::from(byte - b'0'),
                b'a'..=b'f' => u64::from(byte - b'a' + 10),
                b'A'..=b'F' => u64::from(byte - b'A' + 10),
                _ => return Err(SeedError::NonHex),
            };
            result = (result << 4) | digit;
        }
        Ok(Self(result))
    }

    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn as_hex(self) -> String {
        format!("{:016x}", self.0)
    }

    pub const fn bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedError {
    Empty,
    TooLong,
    NonHex,
}

impl fmt::Display for SeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("seed cannot be empty"),
            Self::TooLong => formatter.write_str("seed accepts at most sixteen hexadecimal digits"),
            Self::NonHex => formatter.write_str("seed accepts ASCII hexadecimal digits only"),
        }
    }
}

impl Error for SeedError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderMode {
    Canonical,
    Random { seed: Seed },
}

impl OrderMode {
    pub const fn algorithm(self) -> &'static str {
        match self {
            Self::Canonical => CANONICAL_ORDER_ALGORITHM,
            Self::Random { .. } => RANDOM_ORDER_ALGORITHM,
        }
    }

    pub const fn seed(self) -> Option<Seed> {
        match self {
            Self::Canonical => None,
            Self::Random { seed } => Some(seed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleNodeKind {
    Suite,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleNode {
    id: String,
    parent: Option<String>,
    kind: ScheduleNodeKind,
}

impl ScheduleNode {
    pub fn suite(id: impl Into<String>, parent: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            kind: ScheduleNodeKind::Suite,
        }
    }

    pub fn test(id: impl Into<String>, parent: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            kind: ScheduleNodeKind::Test,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub const fn kind(&self) -> ScheduleNodeKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    ZeroJobs,
    EmptyNodeId,
    DuplicateNode(String),
    UnknownParent { child: String, parent: String },
    LeafParent { child: String, parent: String },
    Cycle(String),
    EmptyTask,
    SequenceOverflow,
    ClockRegression { previous: u64, current: u64 },
    JobLimitReached { limit: u32 },
    ReleaseWithoutPermit,
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroJobs => formatter.write_str("scheduler jobs must be positive"),
            Self::EmptyNodeId => formatter.write_str("scheduler node identity cannot be empty"),
            Self::DuplicateNode(id) => write!(formatter, "scheduler node `{id}` is duplicated"),
            Self::UnknownParent { child, parent } => {
                write!(
                    formatter,
                    "node `{child}` refers to unknown parent `{parent}`"
                )
            }
            Self::LeafParent { child, parent } => {
                write!(formatter, "leaf `{parent}` cannot contain `{child}`")
            }
            Self::Cycle(id) => write!(formatter, "scheduler tree contains a cycle at `{id}`"),
            Self::EmptyTask => formatter.write_str("virtual task identity cannot be empty"),
            Self::SequenceOverflow => formatter.write_str("virtual queue sequence overflowed"),
            Self::ClockRegression { previous, current } => {
                write!(
                    formatter,
                    "virtual clock regressed from {previous} to {current}"
                )
            }
            Self::JobLimitReached { limit } => {
                write!(formatter, "scheduler job limit {limit} is already active")
            }
            Self::ReleaseWithoutPermit => formatter.write_str("scheduler has no active job permit"),
        }
    }
}

impl Error for ScheduleError {}

/// A structural dispatch event. Suite envelopes remain contiguous even when
/// leaf execution is later admitted to parallel workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchEvent {
    EnterSuite(String),
    Test(String),
    ExitSuite(String),
}

/// Validated tree plus deterministic priority mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulePlan {
    nodes: BTreeMap<String, ScheduleNode>,
    children: BTreeMap<String, Vec<String>>,
    roots: Vec<String>,
    mode: OrderMode,
    jobs: u32,
}

impl SchedulePlan {
    pub fn new(
        nodes: impl IntoIterator<Item = ScheduleNode>,
        mode: OrderMode,
        jobs: u32,
    ) -> Result<Self, ScheduleError> {
        if jobs == 0 {
            return Err(ScheduleError::ZeroJobs);
        }
        let mut map = BTreeMap::new();
        for node in nodes {
            if node.id.trim().is_empty() {
                return Err(ScheduleError::EmptyNodeId);
            }
            let id = node.id.clone();
            if map.insert(id.clone(), node).is_some() {
                return Err(ScheduleError::DuplicateNode(id));
            }
        }
        let mut children = BTreeMap::<String, Vec<String>>::new();
        let mut roots = Vec::new();
        for (id, node) in &map {
            let Some(parent) = node.parent.as_deref() else {
                roots.push(id.clone());
                continue;
            };
            let Some(parent_node) = map.get(parent) else {
                return Err(ScheduleError::UnknownParent {
                    child: id.clone(),
                    parent: parent.to_owned(),
                });
            };
            if parent_node.kind == ScheduleNodeKind::Test {
                return Err(ScheduleError::LeafParent {
                    child: id.clone(),
                    parent: parent.to_owned(),
                });
            }
            children
                .entry(parent.to_owned())
                .or_default()
                .push(id.clone());
        }
        roots.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        for values in children.values_mut() {
            values.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        }
        for id in map.keys() {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(id.as_str());
            while let Some(current) = cursor {
                if !seen.insert(current) {
                    return Err(ScheduleError::Cycle(id.clone()));
                }
                cursor = map[current].parent.as_deref();
            }
        }
        Ok(Self {
            nodes: map,
            children,
            roots,
            mode,
            jobs,
        })
    }

    pub fn mode(&self) -> OrderMode {
        self.mode
    }

    pub const fn jobs(&self) -> u32 {
        self.jobs
    }

    pub fn node(&self, id: &str) -> Option<&ScheduleNode> {
        self.nodes.get(id)
    }

    pub fn execution_plan(&self) -> Vec<String> {
        let mut result = Vec::new();
        for root in self.ordered_children(None) {
            self.collect_leaves(&root, &mut result);
        }
        result
    }

    pub fn dispatch_plan(&self) -> Vec<DispatchEvent> {
        let mut result = Vec::new();
        for root in self.ordered_children(None) {
            self.collect_events(&root, &mut result);
        }
        result
    }

    fn ordered_children(&self, parent: Option<&str>) -> Vec<String> {
        let source: &[String] = match parent {
            None => &self.roots,
            Some(id) => self.children.get(id).map_or(&[][..], Vec::as_slice),
        };
        let mut values = source.to_vec();
        if let OrderMode::Random { seed } = self.mode {
            values.sort_by(|left, right| {
                let left_digest = priority_digest(parent.unwrap_or(""), left, seed);
                let right_digest = priority_digest(parent.unwrap_or(""), right, seed);
                left_digest
                    .cmp(&right_digest)
                    .then_with(|| left.as_bytes().cmp(right.as_bytes()))
            });
        } else {
            values.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        }
        values
    }

    fn collect_leaves(&self, id: &str, result: &mut Vec<String>) {
        let node = &self.nodes[id];
        match node.kind {
            ScheduleNodeKind::Test => result.push(id.to_owned()),
            ScheduleNodeKind::Suite => {
                for child in self.ordered_children(Some(id)) {
                    self.collect_leaves(&child, result);
                }
            }
        }
    }

    fn collect_events(&self, id: &str, result: &mut Vec<DispatchEvent>) {
        let node = &self.nodes[id];
        match node.kind {
            ScheduleNodeKind::Test => result.push(DispatchEvent::Test(id.to_owned())),
            ScheduleNodeKind::Suite => {
                result.push(DispatchEvent::EnterSuite(id.to_owned()));
                for child in self.ordered_children(Some(id)) {
                    self.collect_events(&child, result);
                }
                result.push(DispatchEvent::ExitSuite(id.to_owned()));
            }
        }
    }
}

/// Normative random priority for one direct child. The complete digest is
/// compared bytewise before the visible ID tie-break.
pub fn priority_digest(parent_id: &str, child_id: &str, seed: Seed) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ORDER_DOMAIN);
    hasher.update(seed.bytes());
    hasher.update([0]);
    hasher.update(parent_id.as_bytes());
    hasher.update([0]);
    hasher.update(child_id.as_bytes());
    hasher.finalize().into()
}

pub fn digest_hex(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in digest {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

/// Global admission gate for setup, leaf bodies and teardown. A permit is
/// represented by the active counter, so workers cannot exceed the same limit
/// across lifecycle phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimiter {
    limit: u32,
    active: u32,
}

impl JobLimiter {
    pub fn new(limit: u32) -> Result<Self, ScheduleError> {
        if limit == 0 {
            return Err(ScheduleError::ZeroJobs);
        }
        Ok(Self { limit, active: 0 })
    }

    pub const fn limit(self) -> u32 {
        self.limit
    }

    pub const fn active(self) -> u32 {
        self.active
    }

    pub const fn available(self) -> u32 {
        self.limit - self.active
    }

    pub fn try_acquire(&mut self) -> Result<(), ScheduleError> {
        if self.active == self.limit {
            return Err(ScheduleError::JobLimitReached { limit: self.limit });
        }
        self.active += 1;
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), ScheduleError> {
        if self.active == 0 {
            return Err(ScheduleError::ReleaseWithoutPermit);
        }
        self.active -= 1;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerEntry {
    task: String,
    deadline: u64,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadyEntry {
    task: String,
    sequence: u64,
}

/// Deterministic virtual-domain queue. Creation sequence is the tie-break for
/// equal deadlines; wakeups are appended to the ready queue in that same
/// sequence and drained without consulting wall-clock scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualQueue {
    now: u64,
    next_sequence: u64,
    timers: Vec<TimerEntry>,
    ready: Vec<ReadyEntry>,
}

impl Default for VirtualQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualQueue {
    pub const fn new() -> Self {
        Self {
            now: 0,
            next_sequence: 1,
            timers: Vec::new(),
            ready: Vec::new(),
        }
    }

    pub const fn now(&self) -> u64 {
        self.now
    }

    pub fn pending_timers(&self) -> usize {
        self.timers.len()
    }

    pub fn schedule_timer(
        &mut self,
        task: impl Into<String>,
        deadline: u64,
    ) -> Result<u64, ScheduleError> {
        self.enqueue_timer(task.into(), deadline)
    }

    pub fn enqueue_ready(&mut self, task: impl Into<String>) -> Result<u64, ScheduleError> {
        let task = task.into();
        if task.is_empty() {
            return Err(ScheduleError::EmptyTask);
        }
        let sequence = self.take_sequence()?;
        self.ready.push(ReadyEntry { task, sequence });
        Ok(sequence)
    }

    pub fn advance_to(&mut self, now: u64) -> Result<(), ScheduleError> {
        if now < self.now {
            return Err(ScheduleError::ClockRegression {
                previous: self.now,
                current: now,
            });
        }
        self.now = now;
        let mut due = Vec::new();
        let mut pending = Vec::new();
        for timer in self.timers.drain(..) {
            if timer.deadline <= now {
                due.push(timer);
            } else {
                pending.push(timer);
            }
        }
        self.timers = pending;
        due.sort_by_key(|timer| (timer.deadline, timer.sequence));
        self.ready.extend(due.into_iter().map(|timer| ReadyEntry {
            task: timer.task,
            sequence: timer.sequence,
        }));
        Ok(())
    }

    pub fn drain_ready(&mut self) -> Vec<String> {
        self.ready.sort_by_key(|entry| entry.sequence);
        self.ready.drain(..).map(|entry| entry.task).collect()
    }

    fn enqueue_timer(&mut self, task: String, deadline: u64) -> Result<u64, ScheduleError> {
        if task.is_empty() {
            return Err(ScheduleError::EmptyTask);
        }
        let sequence = self.take_sequence()?;
        self.timers.push(TimerEntry {
            task,
            deadline,
            sequence,
        });
        Ok(sequence)
    }

    fn take_sequence(&mut self) -> Result<u64, ScheduleError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(ScheduleError::SequenceOverflow)?;
        Ok(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT: &str = "application::unit::math::arithmetic";
    const ADD: &str = "application::unit::math::arithmetic::addReturnsSum";
    const SUBTRACT: &str = "application::unit::math::arithmetic::subtractReturnsDifference";

    #[test]
    fn seeds_parse_ascii_hex_and_normalize_for_replay() {
        let seed = Seed::parse("5eed").unwrap();
        assert_eq!(seed.value(), 0x5eed);
        assert_eq!(seed.as_hex(), "0000000000005eed");
        assert_eq!(seed.bytes(), 0x5eed_u64.to_be_bytes());
        assert_eq!(Seed::from_u64(seed.value()), seed);
        assert!(matches!(Seed::parse(""), Err(SeedError::Empty)));
        assert!(matches!(
            Seed::parse("1234567890abcdef0"),
            Err(SeedError::TooLong)
        ));
        assert!(matches!(Seed::parse("0x5eed"), Err(SeedError::NonHex)));
        assert!(matches!(Seed::parse("5eed-"), Err(SeedError::NonHex)));
    }

    #[test]
    fn random_priority_matches_both_normative_vectors_and_order() {
        let seed = Seed::parse("0000000000005eed").unwrap();
        let subtract = priority_digest(PARENT, SUBTRACT, seed);
        let add = priority_digest(PARENT, ADD, seed);
        assert_eq!(
            digest_hex(&subtract),
            "00c637b1e275874ed716704cd93b6b8928b23d378fd457c41a38060684094a68"
        );
        assert_eq!(
            digest_hex(&add),
            "9bd8be59d09c20c60e7549466be069ce52f98d8cff6d608878d194029e9650cf"
        );
        assert!(subtract < add);
    }

    #[test]
    fn canonical_and_random_plans_keep_suites_atomic_and_execution_plan_leaf_only() {
        let nodes = [
            ScheduleNode::suite("app", None::<String>),
            ScheduleNode::suite("app::b", Some("app")),
            ScheduleNode::test("app::b::z", Some("app::b")),
            ScheduleNode::test("app::b::a", Some("app::b")),
            ScheduleNode::test("app::root", Some("app")),
        ];
        let canonical = SchedulePlan::new(nodes.clone(), OrderMode::Canonical, 1).unwrap();
        assert_eq!(canonical.mode().algorithm(), CANONICAL_ORDER_ALGORITHM);
        assert_eq!(canonical.jobs(), 1);
        assert_eq!(
            canonical.node("app::b").unwrap().kind(),
            ScheduleNodeKind::Suite
        );
        assert_eq!(
            canonical.execution_plan(),
            ["app::b::a", "app::b::z", "app::root"]
        );
        assert_eq!(
            canonical.dispatch_plan(),
            [
                DispatchEvent::EnterSuite("app".into()),
                DispatchEvent::EnterSuite("app::b".into()),
                DispatchEvent::Test("app::b::a".into()),
                DispatchEvent::Test("app::b::z".into()),
                DispatchEvent::ExitSuite("app::b".into()),
                DispatchEvent::Test("app::root".into()),
                DispatchEvent::ExitSuite("app".into()),
            ]
        );
        let random = SchedulePlan::new(
            nodes,
            OrderMode::Random {
                seed: Seed::from_u64(0x5eed),
            },
            2,
        )
        .unwrap();
        assert_eq!(random.mode().seed().unwrap().as_hex(), "0000000000005eed");
        assert_eq!(random.execution_plan().len(), 3);
        assert!(matches!(
            random.dispatch_plan()[0],
            DispatchEvent::EnterSuite(_)
        ));
    }

    #[test]
    fn random_plan_repeats_exactly_and_is_independent_of_input_order() {
        let first = [
            ScheduleNode::test("app::z", None::<String>),
            ScheduleNode::test("app::a", None::<String>),
            ScheduleNode::test("app::m", None::<String>),
        ];
        let second = [first[2].clone(), first[0].clone(), first[1].clone()];
        let mode = OrderMode::Random {
            seed: Seed::from_u64(0xabc),
        };
        let left = SchedulePlan::new(first, mode, 3).unwrap();
        let right = SchedulePlan::new(second, mode, 3).unwrap();
        assert_eq!(left.execution_plan(), right.execution_plan());
        assert_eq!(left.dispatch_plan(), right.dispatch_plan());
    }

    #[test]
    fn malformed_trees_and_zero_jobs_are_rejected() {
        assert!(matches!(
            SchedulePlan::new([], OrderMode::Canonical, 0),
            Err(ScheduleError::ZeroJobs)
        ));
        assert!(matches!(
            SchedulePlan::new(
                [ScheduleNode::test("", None::<String>)],
                OrderMode::Canonical,
                1
            ),
            Err(ScheduleError::EmptyNodeId)
        ));
        assert!(matches!(
            SchedulePlan::new(
                [
                    ScheduleNode::test("x", None::<String>),
                    ScheduleNode::test("x", None::<String>)
                ],
                OrderMode::Canonical,
                1
            ),
            Err(ScheduleError::DuplicateNode(_))
        ));
        assert!(matches!(
            SchedulePlan::new(
                [ScheduleNode::test("x", Some("missing"))],
                OrderMode::Canonical,
                1
            ),
            Err(ScheduleError::UnknownParent { .. })
        ));
        assert!(matches!(
            SchedulePlan::new(
                [
                    ScheduleNode::test("parent", None::<String>),
                    ScheduleNode::test("child", Some("parent"))
                ],
                OrderMode::Canonical,
                1
            ),
            Err(ScheduleError::LeafParent { .. })
        ));
        assert!(matches!(
            SchedulePlan::new(
                [
                    ScheduleNode::suite("a", Some("b")),
                    ScheduleNode::suite("b", Some("a"))
                ],
                OrderMode::Canonical,
                1
            ),
            Err(ScheduleError::Cycle(_))
        ));
    }

    #[test]
    fn global_job_limiter_covers_setup_body_and_teardown_admission() {
        let mut limiter = JobLimiter::new(2).unwrap();
        assert_eq!(limiter.limit(), 2);
        assert_eq!(limiter.active(), 0);
        assert_eq!(limiter.available(), 2);
        limiter.try_acquire().unwrap();
        limiter.try_acquire().unwrap();
        assert_eq!(limiter.available(), 0);
        assert!(matches!(
            limiter.try_acquire(),
            Err(ScheduleError::JobLimitReached { limit: 2 })
        ));
        limiter.release().unwrap();
        assert_eq!(limiter.active(), 1);
        limiter.release().unwrap();
        assert!(matches!(
            limiter.release(),
            Err(ScheduleError::ReleaseWithoutPermit)
        ));
        assert!(matches!(JobLimiter::new(0), Err(ScheduleError::ZeroJobs)));
    }

    #[test]
    fn virtual_queue_orders_ready_work_and_tied_timers_by_creation_sequence() {
        let mut queue = VirtualQueue::new();
        assert_eq!(queue.now(), 0);
        assert_eq!(queue.enqueue_ready("already-ready").unwrap(), 1);
        assert_eq!(queue.schedule_timer("late-created", 10).unwrap(), 2);
        assert_eq!(queue.schedule_timer("first-tie", 5).unwrap(), 3);
        assert_eq!(queue.schedule_timer("second-tie", 5).unwrap(), 4);
        assert_eq!(queue.pending_timers(), 3);
        queue.advance_to(5).unwrap();
        assert_eq!(
            queue.drain_ready(),
            ["already-ready", "first-tie", "second-tie"]
        );
        assert_eq!(queue.pending_timers(), 1);
        queue.advance_to(10).unwrap();
        assert_eq!(queue.drain_ready(), ["late-created"]);
        assert_eq!(queue.pending_timers(), 0);
        assert!(matches!(
            queue.advance_to(9),
            Err(ScheduleError::ClockRegression { .. })
        ));
    }

    #[test]
    fn virtual_queue_rejects_empty_tasks_and_reports_protocol_constants() {
        let mut queue = VirtualQueue::new();
        assert!(matches!(
            queue.enqueue_ready(""),
            Err(ScheduleError::EmptyTask)
        ));
        assert!(matches!(
            queue.schedule_timer("", 1),
            Err(ScheduleError::EmptyTask)
        ));
        assert_eq!(TEST_SCHEDULE_FORMAT, "tondo-test-schedule-draft/1");
        assert_eq!(CANONICAL_ORDER_ALGORITHM, "id-byte-order-v1");
        assert_eq!(RANDOM_ORDER_ALGORITHM, "sha256-tree-v1");
    }
}
