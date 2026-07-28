use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use tondo_compiler::driver::{DiagnosticFormat, Operation, ResourceLimits, execute};
use tondo_compiler::project::ProjectPlan;
use tondo_conformance::protocol::{
    AdapterRequest, CompilationState, Observation, WireDeterminismAction,
};
use tondo_conformance::{decode_hex, sha256};

pub(crate) fn observe_determinism(
    _request: &AdapterRequest,
    action: &WireDeterminismAction,
) -> Result<Observation, String> {
    let manifest = decode_hex(&action.manifest_hex)?;
    let lockfile = decode_hex(&action.lockfile_hex)?;
    let plan = ProjectPlan::parse(&manifest, &lockfile).map_err(|error| error.to_string())?;
    let mut inputs = BTreeMap::new();
    for input in &action.inputs {
        if inputs
            .insert(
                input.logical_path.clone(),
                Arc::<[u8]>::from(decode_hex(&input.contents_hex)?),
            )
            .is_some()
        {
            return Err(format!(
                "determinism input `{}` is duplicated",
                input.logical_path
            ));
        }
    }
    let canonical = plan
        .selected_source_paths()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let reverse = canonical.iter().rev().cloned().collect::<Vec<_>>();
    let first = compile(&plan, &inputs, &canonical)?;
    let second = compile(&plan, &inputs, &reverse)?;
    let identical = first == second;
    let diagnostics = first.diagnostics.clone();
    let mut observation = Observation::empty();
    observation.compilation = if identical {
        CompilationState::Success
    } else {
        CompilationState::Rejected
    };
    observation.exit_code = i32::from(!identical);
    observation.diagnostics = diagnostics;
    observation.data = json!({
        "schema": "tondo-determinism-observation-0.1/1",
        "identical": identical,
        "permutations": [
            first.record("canonical", canonical),
            second.record("reverse", reverse)
        ]
    });
    Ok(observation)
}

#[derive(Debug, PartialEq, Eq)]
struct BuildObservation {
    interface: Vec<u8>,
    artifact: Vec<u8>,
    diagnostics: Vec<Value>,
}

impl BuildObservation {
    fn record(&self, name: &str, source_order: Vec<String>) -> Value {
        json!({
            "name": name,
            "source_order": source_order,
            "interface_sha256": sha256(&self.interface),
            "artifact_sha256": sha256(&self.artifact),
            "diagnostics_sha256": sha256(
                &serde_json::to_vec(&self.diagnostics)
                    .expect("diagnostic observations are serializable")
            )
        })
    }
}

fn compile(
    plan: &ProjectPlan,
    inputs: &BTreeMap<String, Arc<[u8]>>,
    source_order: &[String],
) -> Result<BuildObservation, String> {
    let project = plan
        .resolve_with_source_order(inputs, source_order)
        .map_err(|error| error.to_string())?;
    let request = project
        .into_compilation_request(
            Operation::Check,
            DiagnosticFormat::Json,
            ResourceLimits::default(),
        )
        .map_err(|error| error.to_string())?;
    let output = execute(request).map_err(|error| error.to_string())?;
    let diagnostics = output
        .diagnostics()
        .json_lines()
        .map_err(|error| error.to_string())?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let interface = output
        .interface()
        .ok_or_else(|| "determinism build produced no compiled interface".to_owned())?
        .encode()
        .map_err(|error| error.to_string())?;
    let artifact = output
        .artifact()
        .ok_or_else(|| "determinism build produced no build artifact".to_owned())?
        .encode()
        .map_err(|error| error.to_string())?;
    Ok(BuildObservation {
        interface,
        artifact,
        diagnostics,
    })
}
