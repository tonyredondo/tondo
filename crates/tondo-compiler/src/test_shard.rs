//! Stable, selection-order-independent test sharding.
//!
//! Sharding is a pure boundary after selection and before execution ordering.
//! It hashes the complete visible test identity with the normative domain
//! separator and treats the SHA-256 digest as a big-endian integer. No host
//! hash map, discovery order, job count, or random seed participates.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use sha2::{Digest, Sha256};

pub const TEST_SHARD_FORMAT: &str = "tondo-test-shard-draft/1";
pub const TEST_SHARD_ALGORITHM: &str = "sha256-mod-v1";
const DOMAIN: &[u8] = b"tondo-test-shard-v1\0";

/// A validated one-based shard selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardSpec {
    index: u32,
    count: u32,
}

impl ShardSpec {
    pub fn new(index: u32, count: u32) -> Result<Self, ShardError> {
        if index == 0 {
            return Err(ShardError::ZeroIndex);
        }
        if count == 0 {
            return Err(ShardError::ZeroCount);
        }
        if index > count {
            return Err(ShardError::IndexOutOfRange { index, count });
        }
        Ok(Self { index, count })
    }

    /// Parse the closed `index/count` spelling used by `--shard`.
    pub fn parse(value: &str) -> Result<Self, ShardError> {
        let (index, count) = value.split_once('/').ok_or(ShardError::InvalidFormat)?;
        if count.contains('/') {
            return Err(ShardError::InvalidFormat);
        }
        let index = parse_decimal(index).ok_or(ShardError::InvalidFormat)?;
        let count = parse_decimal(count).ok_or(ShardError::InvalidFormat)?;
        Self::new(index, count)
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn count(self) -> u32 {
        self.count
    }

    pub fn as_str(self) -> String {
        format!("{}/{}", self.index, self.count)
    }
}

fn parse_decimal(value: &str) -> Option<u32> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardError {
    InvalidFormat,
    ZeroIndex,
    ZeroCount,
    IndexOutOfRange { index: u32, count: u32 },
    EmptyId,
    DuplicateId(String),
}

impl fmt::Display for ShardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => formatter.write_str("shard expects canonical `index/count`"),
            Self::ZeroIndex => formatter.write_str("shard index must be positive"),
            Self::ZeroCount => formatter.write_str("shard count must be positive"),
            Self::IndexOutOfRange { index, count } => {
                write!(formatter, "shard index {index} exceeds count {count}")
            }
            Self::EmptyId => formatter.write_str("test identity cannot be empty"),
            Self::DuplicateId(id) => write!(formatter, "test identity `{id}` is duplicated"),
        }
    }
}

impl Error for ShardError {}

/// One selected test and the complete digest used for its assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardAssignment {
    id: String,
    shard: u32,
    digest: [u8; 32],
}

impl ShardAssignment {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn shard(&self) -> u32 {
        self.shard
    }

    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    pub fn digest_hex(&self) -> String {
        hex_digest(&self.digest)
    }
}

/// The selected part of one shard. An empty result is valid when the input
/// selection was non-empty and no identity hashes into this shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardResult {
    spec: ShardSpec,
    assignments: Vec<ShardAssignment>,
}

impl ShardResult {
    pub fn partition<'a>(
        ids: impl IntoIterator<Item = &'a str>,
        spec: ShardSpec,
    ) -> Result<Self, ShardError> {
        let ids = canonical_ids(ids)?;
        Ok(Self {
            spec,
            assignments: ids
                .into_iter()
                .filter_map(|id| {
                    let (shard, digest) = assign(&id, spec.count);
                    (shard == spec.index).then_some(ShardAssignment { id, shard, digest })
                })
                .collect(),
        })
    }

    pub const fn spec(&self) -> ShardSpec {
        self.spec
    }

    pub fn assignments(&self) -> &[ShardAssignment] {
        &self.assignments
    }

    pub fn ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.assignments.iter().map(|assignment| assignment.id())
    }

    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.assignments
            .iter()
            .any(|assignment| assignment.id == id)
    }
}

/// Partition a selection into every shard, preserving the same canonical
/// input validation and proving the disjoint-union invariant to callers.
pub fn partition_all<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    count: u32,
) -> Result<Vec<ShardResult>, ShardError> {
    ShardSpec::new(1, count)?;
    let ids = canonical_ids(ids)?;
    (1..=count)
        .map(|index| {
            ShardSpec::new(index, count)
                .and_then(|spec| ShardResult::partition(ids.iter().map(String::as_str), spec))
        })
        .collect()
}

fn canonical_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<Vec<String>, ShardError> {
    let mut seen = BTreeSet::<&str>::new();
    let mut canonical: Vec<String> = Vec::new();
    for id in ids {
        if id.is_empty() {
            return Err(ShardError::EmptyId);
        }
        if !seen.insert(id) {
            return Err(ShardError::DuplicateId(id.into()));
        }
        canonical.push(id.to_owned());
    }
    canonical.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(canonical)
}

fn assign(id: &str, count: u32) -> (u32, [u8; 32]) {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(id.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut remainder = 0_u64;
    for byte in digest {
        remainder = (remainder * 256 + u64::from(byte)) % u64::from(count);
    }
    (remainder as u32 + 1, digest)
}

fn hex_digest(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in digest {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR_ID: &str = "application::unit::math::arithmetic::addReturnsSum";
    const VECTOR_DIGEST: &str = "ee5252232b68a78e79fc22b6e8d761a22e2989369358efc402802d22989f2517";

    #[test]
    fn shard_spec_accepts_only_canonical_positive_decimal_ranges() {
        let spec = ShardSpec::parse("2/8").unwrap();
        assert_eq!(spec.index(), 2);
        assert_eq!(spec.count(), 8);
        assert_eq!(spec.as_str(), "2/8");
        assert!(matches!(
            ShardSpec::parse("0/8"),
            Err(ShardError::ZeroIndex)
        ));
        assert!(matches!(
            ShardSpec::parse("1/0"),
            Err(ShardError::ZeroCount)
        ));
        assert!(matches!(
            ShardSpec::parse("9/8"),
            Err(ShardError::IndexOutOfRange { .. })
        ));
        for value in ["", "1", "1/", "/8", "01/8", "1/08", "1/2/3", "+1/2", "1 /2"] {
            assert!(
                matches!(ShardSpec::parse(value), Err(ShardError::InvalidFormat)),
                "{value}"
            );
        }
    }

    #[test]
    fn normative_vector_uses_the_complete_big_endian_sha256_digest() {
        let result = ShardResult::partition([VECTOR_ID], ShardSpec::new(8, 8).unwrap()).unwrap();
        let assignment = &result.assignments()[0];
        assert_eq!(assignment.id(), VECTOR_ID);
        assert_eq!(assignment.shard(), 8);
        assert_eq!(assignment.digest_hex(), VECTOR_DIGEST);
        assert_eq!(assignment.digest().len(), 32);
    }

    #[test]
    fn assignment_does_not_depend_on_discovery_order() {
        let first = ShardResult::partition(
            ["app::z", "app::a", "app::m"],
            ShardSpec::new(1, 3).unwrap(),
        )
        .unwrap();
        let second = ShardResult::partition(
            ["app::m", "app::z", "app::a"],
            ShardSpec::new(1, 3).unwrap(),
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn every_shard_is_disjoint_and_the_union_reconstructs_selection() {
        let ids = ["app::z", "app::a", "app::m", "app::ñ"];
        let shards = partition_all(ids, 4).unwrap();
        assert_eq!(shards.len(), 4);
        let mut union = Vec::new();
        for (left_index, left) in shards.iter().enumerate() {
            union.extend(left.ids().map(str::to_owned));
            for right in shards.iter().skip(left_index + 1) {
                assert!(left.ids().all(|id| !right.contains(id)));
            }
        }
        union.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        assert_eq!(union, ["app::a", "app::m", "app::z", "app::ñ"]);
    }

    #[test]
    fn an_empty_shard_is_valid_after_a_non_empty_selection() {
        let result = ShardResult::partition([VECTOR_ID], ShardSpec::new(1, 8).unwrap()).unwrap();
        assert!(result.is_empty());
        assert!(!result.contains(VECTOR_ID));
    }

    #[test]
    fn one_shard_contains_every_identity_in_canonical_byte_order() {
        let result =
            ShardResult::partition(["é", "a", "z"], ShardSpec::new(1, 1).unwrap()).unwrap();
        assert_eq!(result.ids().collect::<Vec<_>>(), ["a", "z", "é"]);
    }

    #[test]
    fn invalid_selections_are_rejected_before_assignment() {
        let spec = ShardSpec::new(1, 2).unwrap();
        assert!(matches!(
            ShardResult::partition([""], spec),
            Err(ShardError::EmptyId)
        ));
        assert!(matches!(
            ShardResult::partition(["same", "same"], spec),
            Err(ShardError::DuplicateId(id)) if id == "same"
        ));
        assert!(matches!(
            partition_all(["test"], 0),
            Err(ShardError::ZeroCount)
        ));
    }

    #[test]
    fn format_and_algorithm_are_stable_report_constants() {
        assert_eq!(TEST_SHARD_FORMAT, "tondo-test-shard-draft/1");
        assert_eq!(TEST_SHARD_ALGORITHM, "sha256-mod-v1");
    }
}
