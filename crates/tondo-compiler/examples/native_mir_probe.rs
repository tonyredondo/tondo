//! Emit a bounded, path-independent inventory of the verified MIR for native
//! backend selection.  This is an evaluation probe, not a native compiler.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::Path;

use serde::Serialize;
use tondo_compiler::artifact::sha256;
use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_vm::bytecode::BytecodeFunctionId;
use tondo_vm::runtime::{RejectingHost, RuntimeValue, VmOutcome, execute_with_arguments};

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ProbeReport {
    format: &'static str,
    backend: &'static str,
    profile: &'static str,
    fixtures: Vec<FixtureObservation>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct FixtureObservation {
    fixture: String,
    fixture_sha256: String,
    status: &'static str,
    exit_code: u8,
    diagnostic_codes: Vec<String>,
    stdout_sha256: String,
    mir: Option<tondo_compiler::mir::MirSummary>,
    vm_scalar: Vec<VmScalarObservation>,
    vm_managed: Vec<VmManagedObservation>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VmScalarObservation {
    function_ordinal: u32,
    arguments: Vec<i64>,
    status: &'static str,
    result: Option<i64>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VmManagedObservation {
    function_ordinal: u32,
    arguments: Vec<i64>,
    status: &'static str,
    tag: Option<u64>,
    payload: Option<i64>,
    payload_text: Option<String>,
    diagnostics: Vec<String>,
}

fn main() {
    let paths = env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        eprintln!("usage: native_mir_probe <fixture.to> [fixture.to ...]");
        std::process::exit(2);
    }

    let mut fixtures = Vec::with_capacity(paths.len());
    for path in paths {
        match observe_fixture(Path::new(&path)) {
            Ok(observation) => fixtures.push(observation),
            Err(error) => {
                eprintln!("native MIR probe: {error}");
                std::process::exit(1);
            }
        }
    }

    let report = ProbeReport {
        format: "tondo-native-mir-probe/1",
        backend: "bytecode-vm-oracle",
        profile: "hosted",
        fixtures,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("probe report is serializable")
    );
}

fn observe_fixture(path: &Path) -> Result<FixtureObservation, String> {
    let path_text = path
        .to_str()
        .ok_or_else(|| format!("fixture path is not UTF-8: {}", path.display()))?;
    if path.is_absolute() || path_text.contains("..") || !path_text.ends_with(".to") {
        return Err(format!(
            "fixture path is not a safe workspace `.to` path: {path_text}"
        ));
    }
    let source = fs::read(path).map_err(|error| format!("cannot read `{path_text}`: {error}"))?;
    let source_hash = sha256(&source);
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("native-evaluation:{path_text}"))
                .map_err(|error| error.to_string())?,
            ModulePath::new("native_evaluation").map_err(|error| error.to_string())?,
            LogicalPath::new(path_text).map_err(|error| error.to_string())?,
            source,
        ))
        .map_err(|error| error.to_string())?;
    let packages = PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?;
    let request = CompilationRequest::new(
        Operation::Run,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BuildTarget::vm_hosted_capabilities(),
        DiagnosticFormat::Json,
        SourceForm::Script,
        ResourceLimits::default(),
        packages,
        sources,
        root,
    )
    .map_err(|error| error.to_string())?
    .with_bytecode_observation();
    let output = execute(request).map_err(|error| error.to_string())?;
    let diagnostics = output
        .diagnostics()
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect::<Vec<_>>();
    let vm_scalar = output
        .mir_summary()
        .and_then(|summary| summary.backend.as_ref())
        .zip(output.bytecode())
        .map(|(backend, bytecode)| {
            backend
                .functions
                .iter()
                .filter(|function| function.supported && function.return_type == "Int")
                .flat_map(|function| {
                    scalar_case_arguments_for_function(function)
                        .into_iter()
                        .map(|arguments| {
                            let runtime_arguments = arguments
                                .iter()
                                .copied()
                                .map(|value| RuntimeValue::Integer(i128::from(value)))
                                .collect::<Vec<_>>();
                            let mut host = RejectingHost;
                            let execution = execute_with_arguments(
                                bytecode,
                                BytecodeFunctionId::new(function.ordinal),
                                runtime_arguments,
                                &mut host,
                            );
                            match execution {
                                Ok(execution) => match execution.outcome {
                                    VmOutcome::Returned(RuntimeValue::Integer(value)) => {
                                        VmScalarObservation {
                                            function_ordinal: function.ordinal,
                                            arguments,
                                            status: "returned",
                                            result: i64::try_from(value).ok(),
                                            diagnostics: Vec::new(),
                                        }
                                    }
                                    VmOutcome::Returned(_) => VmScalarObservation {
                                        function_ordinal: function.ordinal,
                                        arguments,
                                        status: "returned-non-int",
                                        result: None,
                                        diagnostics: vec!["vm-non-int-result".to_owned()],
                                    },
                                    VmOutcome::Panicked(_) => VmScalarObservation {
                                        function_ordinal: function.ordinal,
                                        arguments,
                                        status: "panicked",
                                        result: None,
                                        diagnostics: vec!["vm-panic".to_owned()],
                                    },
                                },
                                Err(error) => VmScalarObservation {
                                    function_ordinal: function.ordinal,
                                    arguments,
                                    status: "error",
                                    result: None,
                                    diagnostics: vec![format!("vm-error:{error}")],
                                },
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let vm_managed = output
        .mir_summary()
        .and_then(|summary| summary.backend.as_ref())
        .zip(output.bytecode())
        .map(|(backend, bytecode)| {
            backend
                .functions
                .iter()
                .filter(|function| {
                    function.supported
                        && function.return_type != "Int"
                        && function
                            .parameter_types
                            .iter()
                            .all(|ty| matches!(ty.as_str(), "Int" | "Bool"))
                })
                .flat_map(|function| {
                    managed_case_arguments_for_function(function)
                        .into_iter()
                        .map(|arguments| {
                            let runtime_arguments = arguments
                                .iter()
                                .zip(&function.parameter_types)
                                .map(|(value, ty)| {
                                    if ty == "Bool" {
                                        RuntimeValue::Bool(*value != 0)
                                    } else {
                                        RuntimeValue::Integer(i128::from(*value))
                                    }
                                })
                                .collect::<Vec<_>>();
                            let mut host = RejectingHost;
                            let execution = execute_with_arguments(
                                bytecode,
                                BytecodeFunctionId::new(function.ordinal),
                                runtime_arguments,
                                &mut host,
                            );
                            match execution {
                                Ok(execution) => match execution.outcome {
                                    VmOutcome::Returned(value) => {
                                        let (tag, payload, payload_text) =
                                            managed_value_summary(&value);
                                        VmManagedObservation {
                                            function_ordinal: function.ordinal,
                                            arguments,
                                            status: if tag.is_some() {
                                                "returned"
                                            } else {
                                                "returned-non-managed"
                                            },
                                            tag,
                                            payload,
                                            payload_text,
                                            diagnostics: if tag.is_some() {
                                                Vec::new()
                                            } else {
                                                vec!["vm-non-managed-result".to_owned()]
                                            },
                                        }
                                    }
                                    VmOutcome::Panicked(_) => VmManagedObservation {
                                        function_ordinal: function.ordinal,
                                        arguments,
                                        status: "panicked",
                                        tag: None,
                                        payload: None,
                                        payload_text: None,
                                        diagnostics: vec!["vm-panic".to_owned()],
                                    },
                                },
                                Err(error) => VmManagedObservation {
                                    function_ordinal: function.ordinal,
                                    arguments,
                                    status: "error",
                                    tag: None,
                                    payload: None,
                                    payload_text: None,
                                    diagnostics: vec![format!("vm-error:{error}")],
                                },
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(FixtureObservation {
        fixture: path_text.to_owned(),
        fixture_sha256: source_hash,
        status: match output.status() {
            CompilationStatus::Success => "passed",
            CompilationStatus::Rejected => "rejected",
        },
        exit_code: output.exit_code(),
        diagnostic_codes: diagnostics,
        stdout_sha256: sha256(output.stdout()),
        mir: output.mir_summary().cloned(),
        vm_scalar,
        vm_managed,
    })
}

fn managed_case_arguments_for_function(
    function: &tondo_compiler::mir::MirBackendFunction,
) -> Vec<Vec<i64>> {
    if function.parameter_types.is_empty() {
        return vec![Vec::new()];
    }
    let nominal = function
        .parameter_types
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            if ty == "Bool" {
                i64::from(index % 2 == 0)
            } else {
                20 + index as i64
            }
        })
        .collect::<Vec<_>>();
    let mut cases = vec![nominal];
    if function.parameter_types.len() == 1 && function.parameter_types[0] == "Bool" {
        cases.extend([vec![0], vec![1]]);
    }
    cases
}

fn managed_value_summary(value: &RuntimeValue) -> (Option<u64>, Option<i64>, Option<String>) {
    match value {
        RuntimeValue::OptionNone => (Some(0), None, None),
        RuntimeValue::OptionSome(value) => scalar_payload(value)
            .map_or((Some(1), None, None), |(payload, text)| {
                (Some(1), payload, text)
            }),
        RuntimeValue::ResultOk(value) => scalar_payload(value)
            .map_or((Some(2), None, None), |(payload, text)| {
                (Some(2), payload, text)
            }),
        RuntimeValue::ResultErr(value) => scalar_payload(value)
            .map_or((Some(3), None, None), |(payload, text)| {
                (Some(3), payload, text)
            }),
        _ => (None, None, None),
    }
}

fn scalar_payload(value: &RuntimeValue) -> Option<(Option<i64>, Option<String>)> {
    match value {
        RuntimeValue::Integer(value) => i64::try_from(*value).ok().map(|value| (Some(value), None)),
        RuntimeValue::Bool(value) => Some((Some(i64::from(*value)), None)),
        RuntimeValue::String(value) => Some((Some(string_payload(value)), Some(value.clone()))),
        _ => None,
    }
}

fn string_payload(value: &str) -> i64 {
    (value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        hash.wrapping_mul(0x100000001b3)
            .wrapping_add(u64::from(byte))
    }) & ((1_u64 << 56) - 1)) as i64
}

fn scalar_case_arguments(parameters: &[u32]) -> Vec<Vec<i64>> {
    if parameters.is_empty() {
        return vec![Vec::new()];
    }
    let nominal = parameters
        .iter()
        .enumerate()
        .map(|(index, _)| 20_i64 + index as i64)
        .collect::<Vec<_>>();
    let mut cases = vec![nominal.clone()];
    if parameters.len() == 1 {
        cases.extend([vec![i64::MAX], vec![i64::MIN], vec![-1], vec![0], vec![1]]);
    } else {
        for (left, right) in [
            (i64::MAX, 1),
            (i64::MIN, 1),
            (i64::MIN, -1),
            (1, 0),
            (1, 64),
            (1, -1),
            (0, 1),
        ] {
            let mut case = nominal.clone();
            case[0] = left;
            case[1] = right;
            cases.push(case);
        }
    }
    cases
}

fn scalar_case_arguments_for_function(
    function: &tondo_compiler::mir::MirBackendFunction,
) -> Vec<Vec<i64>> {
    if !control_flow_has_cycle(function) {
        return scalar_case_arguments(&function.parameters);
    }
    if function.parameters.is_empty() {
        return vec![Vec::new()];
    }
    let nominal = function
        .parameters
        .iter()
        .enumerate()
        .map(|(index, _)| 3_i64 + index as i64)
        .collect::<Vec<_>>();
    let zero = vec![0; function.parameters.len()];
    let one = vec![1; function.parameters.len()];
    vec![nominal, zero, one]
}

fn control_flow_has_cycle(function: &tondo_compiler::mir::MirBackendFunction) -> bool {
    let blocks = function
        .blocks
        .iter()
        .filter(|block| block.kind == "normal")
        .collect::<Vec<_>>();
    let normal_ordinals = blocks
        .iter()
        .map(|block| block.ordinal)
        .collect::<BTreeSet<_>>();
    let mut state = BTreeMap::<u32, u8>::new();
    for block in &blocks {
        if state.get(&block.ordinal).copied().unwrap_or_default() != 0 {
            continue;
        }
        let mut stack = vec![(block.ordinal, false)];
        while let Some((current, expanded)) = stack.pop() {
            if expanded {
                state.insert(current, 2);
                continue;
            }
            match state.get(&current).copied().unwrap_or_default() {
                2 => continue,
                1 => return true,
                _ => {}
            }
            state.insert(current, 1);
            stack.push((current, true));
            let Some(current_block) = blocks.iter().find(|candidate| candidate.ordinal == current)
            else {
                continue;
            };
            for target in backend_terminator_successors(&current_block.terminator)
                .into_iter()
                .filter(|target| normal_ordinals.contains(target))
            {
                match state.get(&target).copied().unwrap_or_default() {
                    0 => stack.push((target, false)),
                    1 => return true,
                    _ => {}
                }
            }
        }
    }
    false
}

fn backend_terminator_successors(
    terminator: &tondo_compiler::mir::MirBackendTerminator,
) -> Vec<u32> {
    match terminator {
        tondo_compiler::mir::MirBackendTerminator::Return
        | tondo_compiler::mir::MirBackendTerminator::Marker { .. } => Vec::new(),
        tondo_compiler::mir::MirBackendTerminator::Goto { target } => vec![*target],
        tondo_compiler::mir::MirBackendTerminator::SwitchBool {
            if_true, if_false, ..
        } => vec![*if_true, *if_false],
        tondo_compiler::mir::MirBackendTerminator::SwitchTag {
            cases, otherwise, ..
        } => cases
            .iter()
            .map(|(_, target)| *target)
            .chain(std::iter::once(*otherwise))
            .collect(),
        tondo_compiler::mir::MirBackendTerminator::Invoke { target, .. } => {
            target.iter().copied().collect()
        }
    }
}
