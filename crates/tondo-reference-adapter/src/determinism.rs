use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use tondo_compiler::driver::{DiagnosticFormat, Operation, ResourceLimits, execute};
use tondo_compiler::project::ProjectPlan;
use tondo_conformance::decode_hex;
use tondo_conformance::protocol::{
    AdapterRequest, CompilationState, Observation, WireDeterminismAction,
};

pub(crate) fn observe_determinism(
    _request: &AdapterRequest,
    action: &WireDeterminismAction,
) -> Result<Observation, String> {
    let manifest = decode_hex(&action.manifest_hex)?;
    let lockfile = decode_hex(&action.lockfile_hex)?;
    let plan = ProjectPlan::parse(&manifest, &lockfile).map_err(|error| error.to_string())?;
    let mut forward = BTreeMap::new();
    for input in &action.inputs {
        forward.insert(
            input.logical_path.clone(),
            Arc::<[u8]>::from(decode_hex(&input.contents_hex)?),
        );
    }
    let reverse = forward
        .iter()
        .rev()
        .map(|(path, bytes)| (path.clone(), Arc::clone(bytes)))
        .collect::<BTreeMap<_, _>>();
    let first = compile(&plan, &forward)?;
    let second = compile(&plan, &reverse)?;
    let identical = first == second;
    let (_interface, _artifact, diagnostics) = first;
    let mut observation = Observation::empty();
    observation.compilation = if identical {
        CompilationState::Success
    } else {
        CompilationState::Rejected
    };
    observation.exit_code = i32::from(!identical);
    observation.diagnostics = diagnostics;
    observation.data = json!({
        "identical": identical
    });
    Ok(observation)
}

type BuildObservation = (Vec<u8>, Vec<u8>, Vec<Value>);

fn compile(
    plan: &ProjectPlan,
    inputs: &BTreeMap<String, Arc<[u8]>>,
) -> Result<BuildObservation, String> {
    let project = plan.resolve(inputs).map_err(|error| error.to_string())?;
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
    Ok((interface, artifact, diagnostics))
}
