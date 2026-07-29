#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use tondo_compiler::artifact::{BuildArtifact, CompiledInterface};
use tondo_compiler::driver::{Operation, ResourceLimits, SourceForm};
use tondo_compiler::project::{PrivilegedUnit, ProjectPlan};
use tondo_conformance::manifest::SuiteManifest;
use tondo_conformance::protocol::{AdapterRequest, AdapterResponse};
use tondo_reliability::harness::observe;

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(1024 * 1024)];
    let _ = ProjectPlan::parse(input, input);
    let _ = PrivilegedUnit::decode(input);
    let _ = CompiledInterface::decode(input);
    let _ = BuildArtifact::decode(input);
    let _ = serde_json::from_slice::<SuiteManifest>(input);
    let _ = serde_json::from_slice::<AdapterRequest>(input);
    let _ = serde_json::from_slice::<AdapterResponse>(input);

    let source = &input[..input.len().min(64 * 1024)];
    if let Ok(observation) = observe(
        "fuzz-protocol-diagnostics",
        Arc::<[u8]>::from(source),
        Operation::Check,
        SourceForm::Module,
        ResourceLimits {
            max_syntax_tokens: 16_384,
            max_syntax_nodes: 32_768,
            max_diagnostics: 512,
            ..ResourceLimits::default()
        },
    ) {
        for line in observation.diagnostics_jsonl.lines() {
            let decoded: serde_json::Value = serde_json::from_str(line).unwrap();
            let canonical = serde_json::to_string(&decoded).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&canonical).unwrap(),
                decoded
            );
        }
    }
});
