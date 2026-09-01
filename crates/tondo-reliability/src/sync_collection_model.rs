//! Independent bounded models for the shared `std.sync` collections.
//!
//! The models deliberately do not reuse hosted or native runtime state.  They
//! provide a small sequential oracle, an exhaustive bounded linearizability
//! checker and a cursor/ownership model for deterministic tests and fuzzing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum logical entries accepted by one modeled collection.
pub const MAX_COLLECTION_ENTRIES: usize = 64;
/// Maximum opaque handles retained by one bounded run.
pub const MAX_COLLECTION_HANDLES: usize = 128;
/// Maximum bytes consumed by one collection fuzz input.
pub const MAX_COLLECTION_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum transitions accepted by one collection fuzz input.
pub const MAX_COLLECTION_FUZZ_STEPS: usize = 512;
/// Maximum operations explored by the exhaustive history checker.
pub const MAX_LINEARIZABILITY_OPS: usize = 12;

/// The five nominal shared collection identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CollectionKind {
    Array,
    Map,
    Set,
    Stack,
    Queue,
}

impl CollectionKind {
    pub const ALL: [Self; 5] = [Self::Array, Self::Map, Self::Set, Self::Stack, Self::Queue];
}

/// Expected failures from the reference model.  These are negative cases,
/// not model panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionModelError {
    InvalidIndex,
    Limit,
    StaleHandle,
    WrongKind,
    InvalidCursor,
    CursorExhausted,
    DuplicateHandle,
    Invariant,
}

/// Result values shared by the method and history models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionResult {
    Unit,
    Optional(Option<u64>),
    Bool(bool),
    CompareExchange {
        exchanged: bool,
        observed: Option<u64>,
    },
    Length(usize),
    Values(Vec<u64>),
    Entries(Vec<(u64, u64)>),
}

/// One public collection operation.  The action carries its nominal kind so
/// a history cannot accidentally apply an Array action to a Map state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionAction {
    Length,
    IsEmpty,
    ArrayGet {
        index: usize,
    },
    ArraySet {
        index: usize,
        value: u64,
    },
    ArrayCompareExchange {
        index: usize,
        expected: u64,
        desired: u64,
    },
    ArraySnapshot,
    MapGet {
        key: u64,
    },
    MapContains {
        key: u64,
    },
    MapInsert {
        key: u64,
        value: u64,
    },
    MapRemove {
        key: u64,
    },
    MapCompareExchange {
        key: u64,
        expected: Option<u64>,
        desired: Option<u64>,
    },
    MapSnapshot,
    SetContains {
        value: u64,
    },
    SetInsert {
        value: u64,
    },
    SetRemove {
        value: u64,
    },
    SetSnapshot,
    StackPush {
        value: u64,
    },
    StackPop,
    StackPeek,
    StackSnapshot,
    QueueEnqueue {
        value: u64,
    },
    QueueDequeue,
    QueuePeek,
    QueueSnapshot,
}

impl CollectionAction {
    /// Return the nominal owner selected by this action.
    pub const fn kind(&self) -> Option<CollectionKind> {
        match self {
            Self::Length | Self::IsEmpty => None,
            Self::ArrayGet { .. }
            | Self::ArraySet { .. }
            | Self::ArrayCompareExchange { .. }
            | Self::ArraySnapshot => Some(CollectionKind::Array),
            Self::MapGet { .. }
            | Self::MapContains { .. }
            | Self::MapInsert { .. }
            | Self::MapRemove { .. }
            | Self::MapCompareExchange { .. }
            | Self::MapSnapshot => Some(CollectionKind::Map),
            Self::SetContains { .. }
            | Self::SetInsert { .. }
            | Self::SetRemove { .. }
            | Self::SetSnapshot => Some(CollectionKind::Set),
            Self::StackPush { .. } | Self::StackPop | Self::StackPeek | Self::StackSnapshot => {
                Some(CollectionKind::Stack)
            }
            Self::QueueEnqueue { .. }
            | Self::QueueDequeue
            | Self::QueuePeek
            | Self::QueueSnapshot => Some(CollectionKind::Queue),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MapEntry {
    key: u64,
    value: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueEntry {
    value: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CollectionState {
    Array(Vec<u64>),
    Map {
        entries: Vec<MapEntry>,
        next_generation: u64,
    },
    Set {
        entries: Vec<ValueEntry>,
        next_generation: u64,
    },
    Stack {
        entries: Vec<ValueEntry>,
        next_generation: u64,
    },
    Queue {
        entries: VecDeque<ValueEntry>,
        next_generation: u64,
    },
}

/// An ordinary sequential collection used as an oracle for all five owners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceCollection {
    kind: CollectionKind,
    state: CollectionState,
}

/// Initial content used by a history checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionSeed {
    Array(Vec<u64>),
    Map(Vec<(u64, u64)>),
    Set(Vec<u64>),
    Stack(Vec<u64>),
    Queue(Vec<u64>),
}

impl CollectionSeed {
    pub fn kind(&self) -> CollectionKind {
        match self {
            Self::Array(_) => CollectionKind::Array,
            Self::Map(_) => CollectionKind::Map,
            Self::Set(_) => CollectionKind::Set,
            Self::Stack(_) => CollectionKind::Stack,
            Self::Queue(_) => CollectionKind::Queue,
        }
    }
}

impl ReferenceCollection {
    /// Create an empty collection. Arrays are zero-length unless a seed is
    /// supplied through [`Self::from_seed`].
    pub fn new(kind: CollectionKind) -> Self {
        let state = match kind {
            CollectionKind::Array => CollectionState::Array(Vec::new()),
            CollectionKind::Map => CollectionState::Map {
                entries: Vec::new(),
                next_generation: 1,
            },
            CollectionKind::Set => CollectionState::Set {
                entries: Vec::new(),
                next_generation: 1,
            },
            CollectionKind::Stack => CollectionState::Stack {
                entries: Vec::new(),
                next_generation: 1,
            },
            CollectionKind::Queue => CollectionState::Queue {
                entries: VecDeque::new(),
                next_generation: 1,
            },
        };
        Self { kind, state }
    }

    /// Build a state with the exact initial content used by a history.
    pub fn from_seed(seed: CollectionSeed) -> Result<Self, CollectionModelError> {
        if seed_len(&seed) > MAX_COLLECTION_ENTRIES {
            return Err(CollectionModelError::Limit);
        }
        let mut model = Self::new(seed.kind());
        match seed {
            CollectionSeed::Array(values) => model.state = CollectionState::Array(values),
            CollectionSeed::Map(entries) => {
                let mut next_generation: u64 = 1;
                let mut state = CollectionState::Map {
                    entries: Vec::new(),
                    next_generation,
                };
                for (key, value) in entries {
                    if let CollectionState::Map {
                        entries,
                        next_generation: next,
                    } = &mut state
                    {
                        if entries.iter().any(|entry: &MapEntry| entry.key == key) {
                            continue;
                        }
                        entries.push(MapEntry {
                            key,
                            value,
                            generation: next_generation,
                        });
                        next_generation = next_generation.saturating_add(1);
                        *next = next_generation;
                    }
                }
                model.state = state;
            }
            CollectionSeed::Set(values) => {
                let mut next_generation: u64 = 1;
                let mut seen = BTreeSet::new();
                let entries = values
                    .into_iter()
                    .filter_map(|value| {
                        if !seen.insert(value) {
                            return None;
                        }
                        let generation = next_generation;
                        next_generation = next_generation.saturating_add(1);
                        Some(ValueEntry { value, generation })
                    })
                    .collect();
                model.state = CollectionState::Set {
                    entries,
                    next_generation,
                };
            }
            CollectionSeed::Stack(values) => {
                let next_generation = values.len() as u64 + 1;
                let entries = values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| ValueEntry {
                        value,
                        generation: index as u64 + 1,
                    })
                    .collect();
                model.state = CollectionState::Stack {
                    entries,
                    next_generation,
                };
            }
            CollectionSeed::Queue(values) => {
                let next_generation = values.len() as u64 + 1;
                let entries = values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| ValueEntry {
                        value,
                        generation: index as u64 + 1,
                    })
                    .collect();
                model.state = CollectionState::Queue {
                    entries,
                    next_generation,
                };
            }
        }
        model.assert_invariants()?;
        Ok(model)
    }

    pub fn kind(&self) -> CollectionKind {
        self.kind
    }

    /// Apply one operation at a single model linearization point.
    pub fn apply(
        &mut self,
        action: &CollectionAction,
    ) -> Result<CollectionResult, CollectionModelError> {
        match action {
            CollectionAction::Length => Ok(CollectionResult::Length(self.len())),
            CollectionAction::IsEmpty => Ok(CollectionResult::Bool(self.is_empty())),
            CollectionAction::ArrayGet { index } => match &self.state {
                CollectionState::Array(values) => {
                    Ok(CollectionResult::Optional(values.get(*index).copied()))
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::ArraySet { index, value } => match &mut self.state {
                CollectionState::Array(values) => values
                    .get_mut(*index)
                    .map(|slot| CollectionResult::Optional(Some(std::mem::replace(slot, *value))))
                    .ok_or(CollectionModelError::InvalidIndex),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::ArrayCompareExchange {
                index,
                expected,
                desired,
            } => match &mut self.state {
                CollectionState::Array(values) => {
                    let slot = values
                        .get_mut(*index)
                        .ok_or(CollectionModelError::InvalidIndex)?;
                    let observed = *slot;
                    let exchanged = observed == *expected;
                    if exchanged {
                        *slot = *desired;
                    }
                    Ok(CollectionResult::CompareExchange {
                        exchanged,
                        observed: Some(observed),
                    })
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::ArraySnapshot => match &self.state {
                CollectionState::Array(values) => Ok(CollectionResult::Values(values.clone())),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapGet { key } => match &self.state {
                CollectionState::Map { entries, .. } => {
                    Ok(CollectionResult::Optional(entries.iter().find_map(
                        |entry| (entry.key == *key).then_some(entry.value),
                    )))
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapContains { key } => match &self.state {
                CollectionState::Map { entries, .. } => Ok(CollectionResult::Bool(
                    entries.iter().any(|entry| entry.key == *key),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapInsert { key, value } => match &mut self.state {
                CollectionState::Map {
                    entries,
                    next_generation,
                } => {
                    if let Some(entry) = entries.iter_mut().find(|entry| entry.key == *key) {
                        return Ok(CollectionResult::Optional(Some(std::mem::replace(
                            &mut entry.value,
                            *value,
                        ))));
                    }
                    if entries.len() >= MAX_COLLECTION_ENTRIES {
                        return Err(CollectionModelError::Limit);
                    }
                    let generation = take_generation(next_generation)?;
                    entries.push(MapEntry {
                        key: *key,
                        value: *value,
                        generation,
                    });
                    Ok(CollectionResult::Optional(None))
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapRemove { key } => match &mut self.state {
                CollectionState::Map { entries, .. } => Ok(CollectionResult::Optional(
                    entries
                        .iter()
                        .position(|entry| entry.key == *key)
                        .map(|index| entries.remove(index).value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapCompareExchange {
                key,
                expected,
                desired,
            } => match &mut self.state {
                CollectionState::Map {
                    entries,
                    next_generation,
                } => {
                    let position = entries.iter().position(|entry| entry.key == *key);
                    let observed = position.map(|index| entries[index].value);
                    if observed != *expected {
                        return Ok(CollectionResult::CompareExchange {
                            exchanged: false,
                            observed,
                        });
                    }
                    match (position, desired) {
                        (Some(index), Some(value)) => entries[index].value = *value,
                        (Some(index), None) => {
                            entries.remove(index);
                        }
                        (None, Some(value)) => {
                            if entries.len() >= MAX_COLLECTION_ENTRIES {
                                return Err(CollectionModelError::Limit);
                            }
                            let generation = take_generation(next_generation)?;
                            entries.push(MapEntry {
                                key: *key,
                                value: *value,
                                generation,
                            });
                        }
                        (None, None) => {}
                    }
                    Ok(CollectionResult::CompareExchange {
                        exchanged: true,
                        observed,
                    })
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::MapSnapshot => match &self.state {
                CollectionState::Map { entries, .. } => Ok(CollectionResult::Entries(
                    entries
                        .iter()
                        .map(|entry| (entry.key, entry.value))
                        .collect(),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::SetContains { value } => match &self.state {
                CollectionState::Set { entries, .. } => Ok(CollectionResult::Bool(
                    entries.iter().any(|entry| entry.value == *value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::SetInsert { value } => match &mut self.state {
                CollectionState::Set {
                    entries,
                    next_generation,
                } => {
                    if entries.iter().any(|entry| entry.value == *value) {
                        return Ok(CollectionResult::Bool(false));
                    }
                    if entries.len() >= MAX_COLLECTION_ENTRIES {
                        return Err(CollectionModelError::Limit);
                    }
                    let generation = take_generation(next_generation)?;
                    entries.push(ValueEntry {
                        value: *value,
                        generation,
                    });
                    Ok(CollectionResult::Bool(true))
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::SetRemove { value } => match &mut self.state {
                CollectionState::Set { entries, .. } => Ok(CollectionResult::Bool(
                    entries
                        .iter()
                        .position(|entry| entry.value == *value)
                        .map(|index| {
                            entries.remove(index);
                            true
                        })
                        .unwrap_or(false),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::SetSnapshot => match &self.state {
                CollectionState::Set { entries, .. } => Ok(CollectionResult::Values(
                    entries.iter().map(|entry| entry.value).collect(),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::StackPush { value } => match &mut self.state {
                CollectionState::Stack {
                    entries,
                    next_generation,
                } => {
                    if entries.len() >= MAX_COLLECTION_ENTRIES {
                        return Err(CollectionModelError::Limit);
                    }
                    let generation = take_generation(next_generation)?;
                    entries.push(ValueEntry {
                        value: *value,
                        generation,
                    });
                    Ok(CollectionResult::Unit)
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::StackPop => match &mut self.state {
                CollectionState::Stack { entries, .. } => Ok(CollectionResult::Optional(
                    entries.pop().map(|entry| entry.value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::StackPeek => match &self.state {
                CollectionState::Stack { entries, .. } => Ok(CollectionResult::Optional(
                    entries.last().map(|entry| entry.value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::StackSnapshot => match &self.state {
                CollectionState::Stack { entries, .. } => Ok(CollectionResult::Values(
                    entries.iter().rev().map(|entry| entry.value).collect(),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::QueueEnqueue { value } => match &mut self.state {
                CollectionState::Queue {
                    entries,
                    next_generation,
                } => {
                    if entries.len() >= MAX_COLLECTION_ENTRIES {
                        return Err(CollectionModelError::Limit);
                    }
                    let generation = take_generation(next_generation)?;
                    entries.push_back(ValueEntry {
                        value: *value,
                        generation,
                    });
                    Ok(CollectionResult::Unit)
                }
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::QueueDequeue => match &mut self.state {
                CollectionState::Queue { entries, .. } => Ok(CollectionResult::Optional(
                    entries.pop_front().map(|entry| entry.value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::QueuePeek => match &self.state {
                CollectionState::Queue { entries, .. } => Ok(CollectionResult::Optional(
                    entries.front().map(|entry| entry.value),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
            CollectionAction::QueueSnapshot => match &self.state {
                CollectionState::Queue { entries, .. } => Ok(CollectionResult::Values(
                    entries.iter().map(|entry| entry.value).collect(),
                )),
                _ => Err(CollectionModelError::WrongKind),
            },
        }
    }

    pub fn len(&self) -> usize {
        match &self.state {
            CollectionState::Array(values) => values.len(),
            CollectionState::Map { entries, .. } => entries.len(),
            CollectionState::Set { entries, .. } => entries.len(),
            CollectionState::Stack { entries, .. } => entries.len(),
            CollectionState::Queue { entries, .. } => entries.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Capture one finite structural cursor horizon without copying values.
    pub fn start_cursor(&self) -> CursorModel {
        CursorModel {
            kind: self.kind,
            horizon: self.horizon(),
            position: if self.kind == CollectionKind::Stack {
                u64::MAX
            } else {
                0
            },
            descending: self.kind == CollectionKind::Stack,
            current_key: None,
            exhausted: false,
            seen_generations: BTreeSet::new(),
        }
    }

    fn horizon(&self) -> u64 {
        match &self.state {
            CollectionState::Array(values) => values.len() as u64,
            CollectionState::Map {
                next_generation, ..
            }
            | CollectionState::Set {
                next_generation, ..
            }
            | CollectionState::Stack {
                next_generation, ..
            }
            | CollectionState::Queue {
                next_generation, ..
            } => next_generation.saturating_sub(1),
        }
    }

    pub fn assert_invariants(&self) -> Result<(), CollectionModelError> {
        if self.len() > MAX_COLLECTION_ENTRIES {
            return Err(CollectionModelError::Invariant);
        }
        let mut generations = BTreeSet::new();
        match &self.state {
            CollectionState::Array(_) => {}
            CollectionState::Map {
                entries,
                next_generation,
            } => {
                if entries
                    .iter()
                    .any(|entry| entry.generation == 0 || !generations.insert(entry.generation))
                    || *next_generation == 0
                {
                    return Err(CollectionModelError::Invariant);
                }
            }
            CollectionState::Set {
                entries,
                next_generation,
            }
            | CollectionState::Stack {
                entries,
                next_generation,
            } => {
                if entries
                    .iter()
                    .any(|entry| entry.generation == 0 || !generations.insert(entry.generation))
                    || *next_generation == 0
                {
                    return Err(CollectionModelError::Invariant);
                }
            }
            CollectionState::Queue {
                entries,
                next_generation,
            } => {
                if entries
                    .iter()
                    .any(|entry| entry.generation == 0 || !generations.insert(entry.generation))
                    || *next_generation == 0
                {
                    return Err(CollectionModelError::Invariant);
                }
            }
        }
        Ok(())
    }
}

fn seed_len(seed: &CollectionSeed) -> usize {
    match seed {
        CollectionSeed::Array(values)
        | CollectionSeed::Set(values)
        | CollectionSeed::Stack(values)
        | CollectionSeed::Queue(values) => values.len(),
        CollectionSeed::Map(entries) => entries.len(),
    }
}

fn take_generation(next_generation: &mut u64) -> Result<u64, CollectionModelError> {
    let generation = *next_generation;
    if generation == 0 {
        return Err(CollectionModelError::Invariant);
    }
    *next_generation = next_generation
        .checked_add(1)
        .ok_or(CollectionModelError::Limit)?;
    Ok(generation)
}

/// One item returned by a direct cursor. Map cursors expose their key through
/// the same observation that produced the value; all other owners leave it
/// absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorItem {
    pub value: u64,
    pub key: Option<u64>,
    pub generation: u64,
}

/// Bounded weak cursor state. It retains only a horizon and the last position;
/// `seen_generations` is diagnostic state in the model, never an implementation
/// requirement for the runtime cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorModel {
    kind: CollectionKind,
    horizon: u64,
    position: u64,
    descending: bool,
    current_key: Option<u64>,
    exhausted: bool,
    seen_generations: BTreeSet<u64>,
}

impl CursorModel {
    pub fn horizon(&self) -> u64 {
        self.horizon
    }

    pub fn current_key(&self) -> Option<u64> {
        self.current_key
    }

    pub fn exhausted(&self) -> bool {
        self.exhausted
    }

    pub fn seen_generations(&self) -> &BTreeSet<u64> {
        &self.seen_generations
    }

    /// Read one item at a linearization point. Removed entries may be skipped,
    /// replacements are observed by value, and post-cursor generations are
    /// excluded even when an old key/value is reinserted.
    pub fn next(
        &mut self,
        source: &ReferenceCollection,
    ) -> Result<Option<CursorItem>, CollectionModelError> {
        if source.kind != self.kind {
            return Err(CollectionModelError::WrongKind);
        }
        if self.exhausted {
            return Err(CollectionModelError::CursorExhausted);
        }
        let item = match &source.state {
            CollectionState::Array(values) => {
                if self.position >= self.horizon {
                    None
                } else {
                    let index = usize::try_from(self.position).unwrap_or(usize::MAX);
                    values.get(index).copied().map(|value| CursorItem {
                        value,
                        key: None,
                        generation: self.position.saturating_add(1),
                    })
                }
            }
            CollectionState::Map { entries, .. } => entries
                .iter()
                .filter(|entry| {
                    entry.generation <= self.horizon
                        && if self.descending {
                            entry.generation < self.position
                        } else {
                            entry.generation > self.position
                        }
                })
                .min_by_key(|entry| entry.generation)
                .map(|entry| CursorItem {
                    value: entry.value,
                    key: Some(entry.key),
                    generation: entry.generation,
                }),
            CollectionState::Set { entries, .. } => entries
                .iter()
                .filter(|entry| {
                    entry.generation <= self.horizon
                        && if self.descending {
                            entry.generation < self.position
                        } else {
                            entry.generation > self.position
                        }
                })
                .min_by_key(|entry| entry.generation)
                .map(|entry| CursorItem {
                    value: entry.value,
                    key: None,
                    generation: entry.generation,
                }),
            CollectionState::Stack { entries, .. } => entries
                .iter()
                .filter(|entry| {
                    entry.generation <= self.horizon && entry.generation < self.position
                })
                .max_by_key(|entry| entry.generation)
                .map(|entry| CursorItem {
                    value: entry.value,
                    key: None,
                    generation: entry.generation,
                }),
            CollectionState::Queue { entries, .. } => entries
                .iter()
                .filter(|entry| {
                    entry.generation <= self.horizon && entry.generation > self.position
                })
                .min_by_key(|entry| entry.generation)
                .map(|entry| CursorItem {
                    value: entry.value,
                    key: None,
                    generation: entry.generation,
                }),
        };
        let Some(item) = item else {
            self.exhausted = true;
            self.current_key = None;
            return Ok(None);
        };
        if !self.seen_generations.insert(item.generation) {
            return Err(CollectionModelError::Invariant);
        }
        self.position = item.generation;
        if self.kind == CollectionKind::Array {
            self.position = self.position.saturating_add(0);
        }
        self.current_key = item.key;
        Ok(Some(item))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HandleRecord {
    kind: CollectionKind,
    identity: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorRecord {
    identity: u64,
    cursor: CursorModel,
}

/// Handle table model for copy-by-identity, cursor source retention and exact
/// last-drop cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedCollectionModel {
    next_identity: u64,
    next_token: u64,
    states: BTreeMap<u64, ReferenceCollection>,
    handles: BTreeMap<u64, HandleRecord>,
    cursors: BTreeMap<u64, CursorRecord>,
    cleanup_runs: BTreeMap<u64, u8>,
}

impl Default for SharedCollectionModel {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedCollectionModel {
    pub fn new() -> Self {
        Self {
            next_identity: 1,
            next_token: 1,
            states: BTreeMap::new(),
            handles: BTreeMap::new(),
            cursors: BTreeMap::new(),
            cleanup_runs: BTreeMap::new(),
        }
    }

    pub fn create(&mut self, kind: CollectionKind) -> Result<u64, CollectionModelError> {
        self.create_seed(match kind {
            CollectionKind::Array => CollectionSeed::Array(vec![0; 4]),
            CollectionKind::Map => CollectionSeed::Map(Vec::new()),
            CollectionKind::Set => CollectionSeed::Set(Vec::new()),
            CollectionKind::Stack => CollectionSeed::Stack(Vec::new()),
            CollectionKind::Queue => CollectionSeed::Queue(Vec::new()),
        })
    }

    pub fn create_seed(&mut self, seed: CollectionSeed) -> Result<u64, CollectionModelError> {
        if self.handles.len() >= MAX_COLLECTION_HANDLES {
            return Err(CollectionModelError::Limit);
        }
        let kind = seed.kind();
        let identity = self.next_identity;
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .ok_or(CollectionModelError::Limit)?;
        let handle = self.allocate_token()?;
        self.states
            .insert(identity, ReferenceCollection::from_seed(seed)?);
        self.handles.insert(handle, HandleRecord { kind, identity });
        Ok(handle)
    }

    pub fn copy_handle(&mut self, handle: u64) -> Result<u64, CollectionModelError> {
        let record = *self
            .handles
            .get(&handle)
            .ok_or(CollectionModelError::StaleHandle)?;
        if self.handles.len() >= MAX_COLLECTION_HANDLES {
            return Err(CollectionModelError::Limit);
        }
        let copy = self.allocate_token()?;
        self.handles.insert(copy, record);
        Ok(copy)
    }

    pub fn discard_handle(&mut self, handle: u64) -> Result<(), CollectionModelError> {
        let record = self
            .handles
            .remove(&handle)
            .ok_or(CollectionModelError::StaleHandle)?;
        self.release_identity_if_unused(record.identity);
        Ok(())
    }

    pub fn apply(
        &mut self,
        handle: u64,
        action: &CollectionAction,
    ) -> Result<CollectionResult, CollectionModelError> {
        let record = *self
            .handles
            .get(&handle)
            .ok_or(CollectionModelError::StaleHandle)?;
        if action.kind().is_some_and(|kind| kind != record.kind) {
            return Err(CollectionModelError::WrongKind);
        }
        let state = self
            .states
            .get_mut(&record.identity)
            .ok_or(CollectionModelError::StaleHandle)?;
        let result = state.apply(action)?;
        state.assert_invariants()?;
        Ok(result)
    }

    pub fn start_cursor(&mut self, handle: u64) -> Result<u64, CollectionModelError> {
        let record = *self
            .handles
            .get(&handle)
            .ok_or(CollectionModelError::StaleHandle)?;
        let cursor_state = self
            .states
            .get(&record.identity)
            .ok_or(CollectionModelError::StaleHandle)?
            .start_cursor();
        let cursor = self.allocate_token()?;
        self.cursors.insert(
            cursor,
            CursorRecord {
                identity: record.identity,
                cursor: cursor_state,
            },
        );
        Ok(cursor)
    }

    pub fn cursor_next(&mut self, cursor: u64) -> Result<Option<CursorItem>, CollectionModelError> {
        let identity = self
            .cursors
            .get(&cursor)
            .ok_or(CollectionModelError::InvalidCursor)?
            .identity;
        let source = self
            .states
            .get(&identity)
            .ok_or(CollectionModelError::StaleHandle)?;
        let record = self
            .cursors
            .get_mut(&cursor)
            .ok_or(CollectionModelError::InvalidCursor)?;
        let item = record.cursor.next(source)?;
        if item.is_none() {
            self.cursors.remove(&cursor);
            self.release_identity_if_unused(identity);
        }
        Ok(item)
    }

    pub fn cursor_key(&self, cursor: u64) -> Result<Option<u64>, CollectionModelError> {
        self.cursors
            .get(&cursor)
            .map(|record| record.cursor.current_key())
            .ok_or(CollectionModelError::InvalidCursor)
    }

    pub fn discard_cursor(&mut self, cursor: u64) -> Result<(), CollectionModelError> {
        let record = self
            .cursors
            .remove(&cursor)
            .ok_or(CollectionModelError::InvalidCursor)?;
        self.release_identity_if_unused(record.identity);
        Ok(())
    }

    pub fn live_handles(&self) -> usize {
        self.handles.len()
    }

    pub fn live_cursors(&self) -> usize {
        self.cursors.len()
    }

    pub fn live_collections(&self) -> usize {
        self.states.len()
    }

    pub fn cleanup_runs(&self) -> usize {
        self.cleanup_runs
            .values()
            .map(|runs| usize::from(*runs))
            .sum()
    }

    pub fn assert_invariants(&self) -> Result<(), CollectionModelError> {
        if self.handles.len() > MAX_COLLECTION_HANDLES
            || self
                .handles
                .values()
                .any(|record| !self.states.contains_key(&record.identity))
            || self
                .cursors
                .values()
                .any(|record| !self.states.contains_key(&record.identity))
        {
            return Err(CollectionModelError::Invariant);
        }
        for state in self.states.values() {
            state.assert_invariants()?;
        }
        if self.cleanup_runs.values().any(|runs| *runs > 1) {
            return Err(CollectionModelError::Invariant);
        }
        Ok(())
    }

    fn allocate_token(&mut self) -> Result<u64, CollectionModelError> {
        let token = self.next_token;
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or(CollectionModelError::Limit)?;
        Ok(token)
    }

    fn release_identity_if_unused(&mut self, identity: u64) {
        let retained_by_handle = self
            .handles
            .values()
            .any(|record| record.identity == identity);
        let retained_by_cursor = self
            .cursors
            .values()
            .any(|record| record.identity == identity);
        if !retained_by_handle && !retained_by_cursor && self.states.remove(&identity).is_some() {
            self.cleanup_runs.insert(identity, 1);
        }
    }

    /// Discard every token in a deterministic order and prove exact cleanup.
    pub fn cleanup_all(&mut self) -> Result<(), CollectionModelError> {
        let cursors = self.cursors.keys().copied().collect::<Vec<_>>();
        for cursor in cursors {
            let _ = self.discard_cursor(cursor);
        }
        let handles = self.handles.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            let _ = self.discard_handle(handle);
        }
        self.assert_invariants()?;
        if self.live_handles() != 0 || self.live_cursors() != 0 || self.live_collections() != 0 {
            return Err(CollectionModelError::Invariant);
        }
        Ok(())
    }
}

/// One completed operation in a bounded history.  Invocation/response steps
/// encode only real-time precedence; the checker chooses the linearization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryOperation {
    pub thread: u8,
    pub invocation: u16,
    pub response: u16,
    pub action: CollectionAction,
    pub result: CollectionResult,
}

impl HistoryOperation {
    pub fn new(
        thread: u8,
        invocation: u16,
        response: u16,
        action: CollectionAction,
        result: CollectionResult,
    ) -> Self {
        Self {
            thread,
            invocation,
            response,
            action,
            result,
        }
    }
}

/// Exhaustively check whether a bounded history has a sequential
/// linearization that preserves real-time order and all observed outcomes.
pub fn is_linearizable(
    seed: CollectionSeed,
    history: &[HistoryOperation],
) -> Result<bool, CollectionModelError> {
    if history.len() > MAX_LINEARIZABILITY_OPS
        || history.iter().any(|op| op.response < op.invocation)
    {
        return Err(CollectionModelError::Limit);
    }
    let kind = seed.kind();
    if history.iter().any(|op| {
        op.action
            .kind()
            .is_some_and(|action_kind| action_kind != kind)
    }) {
        return Err(CollectionModelError::WrongKind);
    }
    let mut predecessors = vec![0_u16; history.len()];
    for (index, left) in history.iter().enumerate() {
        for (other, right) in history.iter().enumerate() {
            if index != other && left.response < right.invocation {
                predecessors[other] |= 1_u16 << index;
            }
        }
    }
    let initial = ReferenceCollection::from_seed(seed)?;
    Ok(search_linearization(&initial, history, &predecessors, 0))
}

fn search_linearization(
    state: &ReferenceCollection,
    history: &[HistoryOperation],
    predecessors: &[u16],
    done: u16,
) -> bool {
    if done == (1_u16 << history.len()).saturating_sub(1) {
        return true;
    }
    for (index, operation) in history.iter().enumerate() {
        let bit = 1_u16 << index;
        if done & bit != 0 || predecessors[index] & !done != 0 {
            continue;
        }
        let mut candidate = state.clone();
        let Ok(observed) = candidate.apply(&operation.action) else {
            continue;
        };
        if observed == operation.result
            && search_linearization(&candidate, history, predecessors, done | bit)
        {
            return true;
        }
    }
    false
}

/// Deterministic summary from the collection model fuzz target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionFuzzSummary {
    pub steps: usize,
    pub accepted_operations: usize,
    pub rejected_operations: usize,
    pub histories_checked: usize,
    pub cursor_yields: usize,
    pub aliases_created: usize,
    pub cleanup_runs: usize,
    pub live_handles: usize,
    pub live_cursors: usize,
    pub live_collections: usize,
    pub state_hash: u64,
}

/// Run a bounded, replayable collection schedule.  It exercises aliases,
/// writers, direct cursors, limits, stale handles and exact teardown without
/// making any claim about the runtime's scheduling implementation.
pub fn run_collection_fuzz_case(input: &[u8]) -> Result<CollectionFuzzSummary, String> {
    let input = &input[..input.len().min(MAX_COLLECTION_FUZZ_INPUT_BYTES)];
    let steps = input.len().clamp(1, MAX_COLLECTION_FUZZ_STEPS);
    let input_len = input.len().max(1);
    let mut model = SharedCollectionModel::new();
    let mut handles = CollectionKind::ALL
        .into_iter()
        .map(|kind| {
            model
                .create(kind)
                .map(|handle| (kind, handle))
                .map_err(|error| format!("{error:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut cursors = Vec::new();
    let mut histories = CollectionKind::ALL.map(|kind| {
        ReferenceCollection::from_seed(match kind {
            CollectionKind::Array => CollectionSeed::Array(vec![0; 4]),
            CollectionKind::Map => CollectionSeed::Map(Vec::new()),
            CollectionKind::Set => CollectionSeed::Set(Vec::new()),
            CollectionKind::Stack => CollectionSeed::Stack(Vec::new()),
            CollectionKind::Queue => CollectionSeed::Queue(Vec::new()),
        })
        .expect("bounded history seed must be valid")
    });
    let mut history_operations = CollectionKind::ALL.map(|_| Vec::new());
    let mut accepted_operations = 0;
    let mut rejected_operations = 0;
    let mut histories_checked = 0;
    let mut cursor_yields = 0;
    let mut aliases_created = 0;
    let mut state_hash = 0xcbf2_9ce4_8422_2325_u64;

    for step in 0..steps {
        let byte = input.get(step % input_len).copied().unwrap_or_default();
        let argument = input
            .get((step + 1) % input_len)
            .copied()
            .unwrap_or_default();
        let slot = usize::from(byte) % handles.len();
        let (kind, handle) = handles[slot];
        let action = fuzz_action(kind, byte, argument);
        let result = model.apply(handle, &action);
        match result {
            Ok(_observed) => {
                accepted_operations += 1;
                let history_oracle = &mut histories[slot % CollectionKind::ALL.len()];
                if let Ok(history_result) = history_oracle.apply(&action) {
                    let operations = &mut history_operations[slot % CollectionKind::ALL.len()];
                    if operations.len() < MAX_LINEARIZABILITY_OPS {
                        operations.push(HistoryOperation::new(
                            byte % 3,
                            step as u16,
                            step as u16 + 1,
                            action.clone(),
                            history_result,
                        ));
                    }
                }
            }
            Err(_) => rejected_operations += 1,
        }

        if byte % 11 == 0 {
            let copy = model
                .copy_handle(handle)
                .map_err(|error| format!("{error:?}"))?;
            handles.push((kind, copy));
            aliases_created += 1;
        }
        if byte % 13 == 0
            && let Ok(cursor) = model.start_cursor(handle)
        {
            cursors.push(cursor);
        }
        if byte % 7 == 0
            && let Some(cursor) = cursors.first().copied()
        {
            match model.cursor_next(cursor) {
                Ok(Some(_)) => cursor_yields += 1,
                Ok(None) | Err(CollectionModelError::CursorExhausted) => {
                    cursors.retain(|candidate| *candidate != cursor)
                }
                Err(_) => {}
            }
        }
        if byte % 17 == 0 && handles.len() > CollectionKind::ALL.len() {
            let stale_candidate = handles.remove(0).1;
            let _ = model.discard_handle(stale_candidate);
        }
        let history_index = slot % CollectionKind::ALL.len();
        if history_operations[history_index].len() == MAX_LINEARIZABILITY_OPS {
            histories_checked += 1;
            let seed = history_seed(kind);
            if !is_linearizable(seed.clone(), &history_operations[history_index])
                .map_err(|error| format!("{error:?}"))?
            {
                return Err("generated collection history was not linearizable".into());
            }
            history_operations[history_index].clear();
            histories[history_index] =
                ReferenceCollection::from_seed(seed).map_err(|error| format!("{error:?}"))?;
        }
        model
            .assert_invariants()
            .map_err(|error| format!("{error:?}"))?;
        state_hash = state_hash
            .wrapping_mul(0x1000_0000_01b3)
            .wrapping_add(u64::from(byte) ^ (u64::from(argument) << 8));
    }

    for (index, operations) in history_operations.iter().enumerate() {
        if !operations.is_empty() {
            let kind = CollectionKind::ALL[index];
            if !is_linearizable(history_seed(kind), operations)
                .map_err(|error| format!("{error:?}"))?
            {
                return Err("partial collection history was not linearizable".into());
            }
            histories_checked += 1;
        }
    }
    for cursor in cursors {
        let _ = model.discard_cursor(cursor);
    }
    for (_kind, handle) in handles {
        let _ = model.discard_handle(handle);
    }
    model
        .assert_invariants()
        .map_err(|error| format!("{error:?}"))?;
    if model.live_handles() != 0 || model.live_cursors() != 0 || model.live_collections() != 0 {
        return Err("collection fuzz teardown retained a token".into());
    }
    Ok(CollectionFuzzSummary {
        steps,
        accepted_operations,
        rejected_operations,
        histories_checked,
        cursor_yields,
        aliases_created,
        cleanup_runs: model.cleanup_runs(),
        live_handles: model.live_handles(),
        live_cursors: model.live_cursors(),
        live_collections: model.live_collections(),
        state_hash,
    })
}

fn fuzz_action(kind: CollectionKind, byte: u8, argument: u8) -> CollectionAction {
    let value = u64::from(argument);
    match kind {
        CollectionKind::Array => match byte % 5 {
            0 => CollectionAction::ArrayGet {
                index: usize::from(argument % 6),
            },
            1 => CollectionAction::ArraySet {
                index: usize::from(argument % 6),
                value,
            },
            2 => CollectionAction::ArrayCompareExchange {
                index: usize::from(argument % 6),
                expected: value,
                desired: value.wrapping_add(1),
            },
            3 => CollectionAction::ArraySnapshot,
            _ => CollectionAction::Length,
        },
        CollectionKind::Map => match byte % 7 {
            0 => CollectionAction::MapGet { key: value % 8 },
            1 => CollectionAction::MapContains { key: value % 8 },
            2 => CollectionAction::MapInsert {
                key: value % 8,
                value: value.wrapping_add(1),
            },
            3 => CollectionAction::MapRemove { key: value % 8 },
            4 => CollectionAction::MapCompareExchange {
                key: value % 8,
                expected: if argument.is_multiple_of(2) {
                    None
                } else {
                    Some(value)
                },
                desired: if argument.is_multiple_of(3) {
                    None
                } else {
                    Some(value.wrapping_add(2))
                },
            },
            5 => CollectionAction::MapSnapshot,
            _ => CollectionAction::Length,
        },
        CollectionKind::Set => match byte % 5 {
            0 => CollectionAction::SetContains { value: value % 8 },
            1 | 2 => CollectionAction::SetInsert { value: value % 8 },
            3 => CollectionAction::SetRemove { value: value % 8 },
            _ => CollectionAction::SetSnapshot,
        },
        CollectionKind::Stack => match byte % 5 {
            0 | 1 => CollectionAction::StackPush { value },
            2 => CollectionAction::StackPop,
            3 => CollectionAction::StackPeek,
            _ => CollectionAction::StackSnapshot,
        },
        CollectionKind::Queue => match byte % 5 {
            0 | 1 => CollectionAction::QueueEnqueue { value },
            2 => CollectionAction::QueueDequeue,
            3 => CollectionAction::QueuePeek,
            _ => CollectionAction::QueueSnapshot,
        },
    }
}

fn history_seed(kind: CollectionKind) -> CollectionSeed {
    match kind {
        CollectionKind::Array => CollectionSeed::Array(vec![0; 4]),
        CollectionKind::Map => CollectionSeed::Map(Vec::new()),
        CollectionKind::Set => CollectionSeed::Set(Vec::new()),
        CollectionKind::Stack => CollectionSeed::Stack(Vec::new()),
        CollectionKind::Queue => CollectionSeed::Queue(Vec::new()),
    }
}
