use std::panic::{AssertUnwindSafe, catch_unwind};

use tondo_compiler::artifact::{BuildArtifact, CompiledInterface};
use tondo_compiler::project::{PrivilegedUnit, ProjectPlan};
use tondo_conformance::manifest::SuiteManifest;
use tondo_conformance::protocol::{AdapterRequest, AdapterResponse};
use tondo_reliability::generator::Generator;
use tondo_reliability::harness::check;
use tondo_reliability::workspace_root;

#[test]
fn arbitrary_protocol_bytes_are_bounded_deterministic_and_never_panic() {
    for seed in 0..8_192 {
        let mut generator = Generator::new(seed);
        let bytes = generator.bytes(512);
        let exercise = || {
            let first = protocol_observation(&bytes);
            let second = protocol_observation(&bytes);
            assert_eq!(first, second, "seed {seed}");
        };
        assert!(
            catch_unwind(AssertUnwindSafe(exercise)).is_ok(),
            "protocol decoder panicked for seed {seed}"
        );
    }
}

fn protocol_observation(bytes: &[u8]) -> Vec<bool> {
    vec![
        ProjectPlan::parse(bytes, bytes).is_ok(),
        PrivilegedUnit::decode(bytes).is_ok(),
        CompiledInterface::decode(bytes).is_ok(),
        BuildArtifact::decode(bytes).is_ok(),
        serde_json::from_slice::<SuiteManifest>(bytes).is_ok(),
        serde_json::from_slice::<AdapterRequest>(bytes).is_ok(),
        serde_json::from_slice::<AdapterResponse>(bytes).is_ok(),
    ]
}

#[test]
fn diagnostics_json_is_canonical_and_round_trips_without_environment() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let observation = check("diagnostics-protocol", "fn invalid(): Int { \"text\" }\n").unwrap();
    let replay = check("diagnostics-protocol", "fn invalid(): Int { \"text\" }\n").unwrap();
    assert!(!observation.accepted);
    assert_eq!(observation.diagnostics_jsonl, replay.diagnostics_jsonl);
    let lines = observation.diagnostics_jsonl.lines().collect::<Vec<_>>();
    assert!(!lines.is_empty());
    for line in lines {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let round_trip: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(round_trip, value);
        let encoded = line.as_bytes();
        assert!(!encoded.windows(2).any(|window| window == b": "));
        assert!(!line.contains(&root.display().to_string()));
    }
}
