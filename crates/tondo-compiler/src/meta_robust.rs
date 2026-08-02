//! Bounded protocol probing used by deterministic properties and fuzzing.

use crate::meta::{MetaRequest, MetaResponse, MetaSnapshot};

pub const MAX_META_FUZZ_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaProtocolProbe {
    pub snapshot_accepted: bool,
    pub request_accepted: bool,
    pub response_accepted: bool,
}

/// Exercises every untrusted serialized meta boundary without allocation
/// proportional to an unbounded fuzzer input.
pub fn probe_meta_protocols(input: &[u8]) -> MetaProtocolProbe {
    let input = &input[..input.len().min(MAX_META_FUZZ_INPUT_BYTES)];
    MetaProtocolProbe {
        snapshot_accepted: MetaSnapshot::decode(input).is_ok(),
        request_accepted: MetaRequest::decode(input).is_ok(),
        response_accepted: MetaResponse::decode(input).is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use serde_json::{Value, json};

    use super::*;
    use crate::meta::{
        META_API, META_MODEL, MetaContractError, MetaInput, MetaLimits, MetaModelError,
        MetaOutputSpec, MetaRoot, MetaSourceMapEntry, MetaSpan,
    };

    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn deterministic_arbitrary_protocol_corpus_never_panics() {
        let mut state = 0x2f81_a421_d7c3_9905_u64;
        for length in (0..=4096).chain([65_535, MAX_META_FUZZ_INPUT_BYTES + 1]) {
            let bounded = length.min(MAX_META_FUZZ_INPUT_BYTES + 1);
            let mut bytes = Vec::with_capacity(bounded);
            for _ in 0..bounded {
                bytes.push(next(&mut state) as u8);
            }
            let first = catch_unwind(AssertUnwindSafe(|| probe_meta_protocols(&bytes))).unwrap();
            let second = catch_unwind(AssertUnwindSafe(|| probe_meta_protocols(&bytes))).unwrap();
            assert_eq!(first, second);
        }
    }

    #[test]
    fn canonical_records_reject_schema_revisions_and_byte_mutations() {
        let snapshot =
            MetaSnapshot::new([MetaRoot::new("workspace:app@1", "app").unwrap()], [], []).unwrap();
        let canonical = snapshot.canonical_bytes().unwrap();
        assert!(probe_meta_protocols(&canonical).snapshot_accepted);

        let mut revision: Value = serde_json::from_slice(&canonical).unwrap();
        revision["format"] = json!("tondo-meta-model-0.1/2");
        assert!(matches!(
            MetaSnapshot::decode(&serde_json::to_vec(&revision).unwrap()),
            Err(MetaModelError::UnsupportedFormat(_))
        ));
        let mut extended: Value = serde_json::from_slice(&canonical).unwrap();
        extended["future"] = Value::Null;
        assert!(MetaSnapshot::decode(&serde_json::to_vec(&extended).unwrap()).is_err());

        for index in 0..canonical.len() {
            let mut mutated = canonical.clone();
            mutated[index] ^= 0x80;
            assert!(!probe_meta_protocols(&mutated).snapshot_accepted);
        }
    }

    #[test]
    fn request_api_revision_and_cycles_fail_before_execution() {
        let duplicate = MetaRoot::new("workspace:app@1", "app").unwrap();
        assert!(matches!(
            MetaSnapshot::new([duplicate.clone(), duplicate], [], []),
            Err(MetaModelError::Duplicate { kind, .. }) if kind == "root"
        ));

        let snapshot = MetaSnapshot::new([], [], []).unwrap();
        let request = MetaRequest::new(
            snapshot,
            [MetaInput::new("schema", b"v1").unwrap()],
            [MetaOutputSpec::new("generated/schema.to", "schema").unwrap()],
            MetaLimits::new(100, 100, 100).unwrap(),
        )
        .unwrap();
        let mut wire: Value = serde_json::from_slice(&request.canonical_bytes().unwrap()).unwrap();
        assert_eq!(wire["api"], META_API);
        wire["api"] = json!("tondo-std-meta-0.1/2");
        assert!(matches!(
            MetaRequest::decode(&serde_json::to_vec(&wire).unwrap()),
            Err(MetaContractError::UnsupportedApi(_))
        ));
        assert_eq!(META_MODEL, "tondo-meta-model-0.1/1");
    }

    #[test]
    fn hostile_outputs_maps_utf8_collisions_and_limits_are_atomic() {
        let snapshot = MetaSnapshot::new([], [], []).unwrap();
        let output = MetaOutputSpec::new("generated/schema.to", "schema").unwrap();
        let request =
            MetaRequest::new(snapshot, [], [output], MetaLimits::new(10, 10, 8).unwrap()).unwrap();
        let mut builder = request.into_source_builder();
        assert!(matches!(
            builder.add_source("generated/schema.to", "schema", [0xff]),
            Err(MetaContractError::InvalidSourceUtf8(_))
        ));
        let invalid_map = MetaSourceMapEntry::new(1, 2, MetaSpan::new(0, 0, 1).unwrap()).unwrap();
        assert_eq!(
            builder.add_mapped_source(
                "generated/schema.to",
                "schema",
                "é".as_bytes(),
                [invalid_map]
            ),
            Err(MetaContractError::InvalidSourceMap)
        );
        assert_eq!(
            builder.add_source("generated/schema.to", "schema", b"123456789"),
            Err(MetaContractError::OutputLimit { limit: 8 })
        );
        builder
            .add_source("generated/schema.to", "schema", b"ok")
            .unwrap();
        assert!(matches!(
            builder.add_source("generated/schema.to", "schema", b"again"),
            Err(MetaContractError::DuplicateOutput(_))
        ));
        assert_eq!(builder.finish().unwrap().outputs()[0].bytes(), b"ok");
    }

    #[test]
    fn source_map_constructor_and_zero_budgets_reject_boundaries() {
        assert_eq!(
            MetaSourceMapEntry::new(2, 1, MetaSpan::new(0, 0, 0).unwrap()),
            Err(MetaContractError::InvalidSourceMap)
        );
        for limits in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
            assert_eq!(
                MetaLimits::new(limits.0, limits.1, limits.2),
                Err(MetaContractError::InvalidLimit)
            );
        }
    }
}
