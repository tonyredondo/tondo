#![doc = "Reference adapter for the portable Tondo 0.1 conformance protocol."]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};
use tondo_compiler::driver::{
    BuildTarget, CapabilityName, CompilationOutput, CompilationRequest, CompilationStatus,
    DiagnosticFormat, HostProfile, Operation, ResourceLimits, SourceForm, WarningProfile, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{
    FileId, LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput,
};
use tondo_compiler::syntax::{LexMode, ParseLimits, ParseMode, format_parsed, lex, parse};
use tondo_conformance::decode_hex;
use tondo_conformance::manifest::MemoryScenario;
use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, AdapterResponse, AdapterResult, CompilationState, Observation,
    WireOperation, WireSourceAction, WireSourceForm,
};
use tondo_conformance::runner::Adapter;
use tondo_vm::runtime::conformance::{MemoryScenario as VmMemoryScenario, run_memory_scenario};

#[derive(Debug, Default)]
pub struct ReferenceAdapter;

impl ReferenceAdapter {
    pub fn handle(&mut self, request: &AdapterRequest) -> AdapterResponse {
        let result = match self.observe(request) {
            Ok(observation) => AdapterResult::Ok { observation },
            Err(message) => AdapterResult::Error { message },
        };
        AdapterResponse {
            protocol: tondo_conformance::ADAPTER_PROTOCOL.into(),
            sequence: request.sequence,
            case_id: request.case_id.clone(),
            result,
        }
    }

    fn observe(&mut self, request: &AdapterRequest) -> Result<Observation, String> {
        validate_target(request)?;
        match &request.action {
            AdapterAction::Describe => Ok(describe()),
            AdapterAction::Source(action) => observe_source(request, action),
            AdapterAction::Semantic(action) => crate::semantic::observe_semantic(request, action),
            AdapterAction::Memory { scenario } => observe_memory(*scenario),
            AdapterAction::Determinism(action) => {
                crate::determinism::observe_determinism(request, action)
            }
            AdapterAction::DocumentFence(action) => {
                crate::document::observe_document_fence(request, action)
            }
        }
    }
}

impl Adapter for ReferenceAdapter {
    fn exchange(&mut self, request: &AdapterRequest) -> Result<AdapterResponse, String> {
        Ok(self.handle(request))
    }
}

fn validate_target(request: &AdapterRequest) -> Result<(), String> {
    if request.target.name != BuildTarget::vm_hosted().name() {
        return Err(format!("unsupported target `{}`", request.target.name));
    }
    if request.target.profile != HostProfile::Hosted.as_str() {
        return Err(format!(
            "unsupported host profile `{}`",
            request.target.profile
        ));
    }
    if request
        .target
        .capabilities
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err("target capabilities must be sorted and unique".into());
    }
    for capability in &request.target.capabilities {
        CapabilityName::new(capability.clone()).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn describe() -> Observation {
    Observation {
        compilation: CompilationState::NotApplicable,
        exit_code: 0,
        diagnostics: Vec::new(),
        stdout_hex: String::new(),
        stderr_hex: String::new(),
        formatted_hex: None,
        data: json!({
            "implementation": "tondo-reference",
            "compiler": tondo_compiler::artifact::COMPILER_ID,
            "compiler_version": env!("CARGO_PKG_VERSION"),
            "language_edition": tondo_compiler::LANGUAGE_EDITION,
            "backend": tondo_vm::BACKEND_NAME,
            "adapter_protocol": tondo_conformance::ADAPTER_PROTOCOL,
            "targets": [{
                "name": BuildTarget::vm_hosted().name(),
                "profile": HostProfile::Hosted.as_str(),
                "capabilities": ["console", "process"]
            }]
        }),
    }
}

pub(crate) struct PreparedSource {
    pub sources: SourceDatabase,
    pub root: FileId,
}

pub(crate) fn prepare_sources(action: &WireSourceAction) -> Result<PreparedSource, String> {
    let mut sources = SourceDatabase::new();
    let mut root = None;
    for source in &action.sources {
        let bytes = decode_hex(&source.contents_hex)?;
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new(source.source_id.clone()).map_err(|error| error.to_string())?,
                ModulePath::new(source.module.clone()).map_err(|error| error.to_string())?,
                LogicalPath::new(source.logical_path.clone()).map_err(|error| error.to_string())?,
                Arc::<[u8]>::from(bytes),
            ))
            .map_err(|error| error.to_string())?;
        if source.logical_path == action.root && root.replace(file).is_some() {
            return Err("source action contains the root path more than once".into());
        }
    }
    let root = root.ok_or_else(|| "source action root was not supplied".to_owned())?;
    Ok(PreparedSource { sources, root })
}

pub(crate) fn source_request(
    request: &AdapterRequest,
    action: &WireSourceAction,
) -> Result<CompilationRequest, String> {
    let prepared = prepare_sources(action)?;
    let packages =
        PackageGraph::loose(&prepared.sources, prepared.root).map_err(|error| error.to_string())?;
    let operation = match action.operation {
        WireOperation::Format => Operation::Format,
        WireOperation::Check => Operation::Check,
        WireOperation::Run => Operation::Run,
    };
    let source_form = match action.form {
        WireSourceForm::Module => SourceForm::Module,
        WireSourceForm::Script => SourceForm::Script,
        WireSourceForm::Fragment => SourceForm::Fragment,
        WireSourceForm::Syntax | WireSourceForm::StandaloneBlock => {
            return Err("syntax-only forms do not use the compilation driver".into());
        }
    };
    let capabilities = request
        .target
        .capabilities
        .iter()
        .map(|capability| {
            CapabilityName::new(capability.clone()).map_err(|error| error.to_string())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let limits = ResourceLimits {
        initial_vm_gc_threshold: action
            .gc_threshold
            .unwrap_or(ResourceLimits::default().initial_vm_gc_threshold),
        ..ResourceLimits::default()
    };
    let warning_profiles = action
        .warning_profiles
        .iter()
        .map(|profile| match profile.as_str() {
            "core" => Ok(WarningProfile::Core),
            _ => Err(format!("unknown warning profile `{profile}`")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    CompilationRequest::new(
        operation,
        tondo_compiler::package::Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        capabilities,
        DiagnosticFormat::Json,
        source_form,
        limits,
        packages,
        prepared.sources,
        prepared.root,
    )
    .map(|request| {
        request
            .with_warning_profiles(warning_profiles)
            .with_program_arguments(action.arguments.clone())
    })
    .map_err(|error| error.to_string())
}

fn observe_source(
    request: &AdapterRequest,
    action: &WireSourceAction,
) -> Result<Observation, String> {
    match action.form {
        WireSourceForm::Syntax | WireSourceForm::StandaloneBlock => observe_syntax_source(action),
        WireSourceForm::Module | WireSourceForm::Script | WireSourceForm::Fragment => {
            let output =
                execute(source_request(request, action)?).map_err(|error| error.to_string())?;
            observation_from_output(output, action.operation)
        }
    }
}

fn observe_syntax_source(action: &WireSourceAction) -> Result<Observation, String> {
    if action.operation != WireOperation::Format {
        return Err("syntax-only conformance actions support only formatting".into());
    }
    if action.sources.len() != 1 || !action.arguments.is_empty() {
        return Err("syntax-only conformance actions require exactly one source".into());
    }
    let prepared = prepare_sources(action)?;
    let lexed = lex(&prepared.sources, prepared.root, LexMode::Fragment)
        .map_err(|error| error.to_string())?;
    let mode = match action.form {
        WireSourceForm::Syntax => ParseMode::SyntaxSequence,
        WireSourceForm::StandaloneBlock => ParseMode::StandaloneBlock,
        _ => unreachable!(),
    };
    let parsed = parse(
        &prepared.sources,
        prepared.root,
        lexed,
        mode,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let mut bag = tondo_compiler::diagnostics::DiagnosticBag::new();
    bag.extend(parsed.diagnostics().iter().cloned());
    let report = bag
        .resolve(tondo_compiler::LANGUAGE_EDITION, &prepared.sources)
        .map_err(|error| error.to_string())?;
    let formatted = if report.is_empty() {
        Some(
            format_parsed(&prepared.sources, prepared.root, &parsed)
                .map_err(|error| error.to_string())?
                .into_bytes(),
        )
    } else {
        None
    };
    observation_from_parts(
        if report.is_empty() {
            CompilationState::Success
        } else {
            CompilationState::Rejected
        },
        u8::from(!report.is_empty()),
        &report,
        Vec::new(),
        formatted,
        Value::Null,
    )
}

pub(crate) fn observation_from_output(
    output: CompilationOutput,
    operation: WireOperation,
) -> Result<Observation, String> {
    let diagnostics = normative_diagnostics(output.diagnostics())?;
    let has_compile_error = diagnostics.iter().any(|diagnostic| {
        diagnostic
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|code| code.starts_with('E'))
    });
    let compilation = if operation == WireOperation::Run {
        if has_compile_error {
            CompilationState::Rejected
        } else {
            CompilationState::Success
        }
    } else {
        match output.status() {
            CompilationStatus::Success => CompilationState::Success,
            CompilationStatus::Rejected => CompilationState::Rejected,
        }
    };
    let formatted = (operation == WireOperation::Format).then(|| output.stdout().to_vec());
    let stdout = if operation == WireOperation::Format {
        Vec::new()
    } else {
        output.stdout().to_vec()
    };
    Ok(Observation {
        compilation,
        exit_code: i32::from(output.exit_code()),
        diagnostics,
        stdout_hex: tondo_conformance::encode_hex(&stdout),
        stderr_hex: String::new(),
        formatted_hex: formatted.map(|bytes| tondo_conformance::encode_hex(&bytes)),
        data: Value::Null,
    })
}

pub(crate) fn observation_from_parts(
    compilation: CompilationState,
    exit_code: u8,
    diagnostics: &tondo_compiler::diagnostics::DiagnosticReport,
    stdout: Vec<u8>,
    formatted: Option<Vec<u8>>,
    data: Value,
) -> Result<Observation, String> {
    let diagnostic_values = normative_diagnostics(diagnostics)?;
    Ok(Observation {
        compilation,
        exit_code: i32::from(exit_code),
        diagnostics: diagnostic_values,
        stdout_hex: tondo_conformance::encode_hex(&stdout),
        stderr_hex: String::new(),
        formatted_hex: formatted.map(|bytes| tondo_conformance::encode_hex(&bytes)),
        data,
    })
}

pub(crate) fn normative_diagnostics(
    diagnostics: &tondo_compiler::diagnostics::DiagnosticReport,
) -> Result<Vec<Value>, String> {
    diagnostics
        .json_lines()
        .map_err(|error| error.to_string())?
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).map_err(|error| error.to_string()))
        .filter_map(|value| match value {
            Ok(value) => {
                let code = value
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if code.starts_with('T') {
                    Some(Err(format!(
                        "reference compiler reached implementation diagnostic `{code}`"
                    )))
                } else if matches!(code.as_bytes().first(), Some(b'E' | b'W' | b'P')) {
                    Some(Ok(value))
                } else {
                    None
                }
            }
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn observe_memory(scenario: MemoryScenario) -> Result<Observation, String> {
    let vm_scenario = match scenario {
        MemoryScenario::ReachableRoots => VmMemoryScenario::ReachableRoots,
        MemoryScenario::UnreachableCycles => VmMemoryScenario::UnreachableCycles,
        MemoryScenario::SustainedPressure => VmMemoryScenario::SustainedPressure,
        MemoryScenario::RetryBeforeOom => VmMemoryScenario::RetryBeforeOom,
    };
    let result = run_memory_scenario(vm_scenario).map_err(|error| error.to_string())?;
    let mut observation = Observation::empty();
    observation.data = json!({
        "schema": "tondo-memory-observation-0.1/1",
        "scenario": result.scenario,
        "collections": result.collections,
        "reclaimed_objects": result.reclaimed_objects,
        "peak_live_objects": result.peak_live_objects,
        "roots_preserved": result.roots_preserved,
        "cycles_reclaimed": result.cycles_reclaimed,
        "retry_before_success": result.retry_before_success,
        "retry_before_oom": result.retry_before_oom
    });
    Ok(observation)
}

pub(crate) fn source_files_by_path(
    action: &WireSourceAction,
) -> BTreeMap<&str, &tondo_conformance::protocol::WireSource> {
    action
        .sources
        .iter()
        .map(|source| (source.logical_path.as_str(), source))
        .collect()
}

mod determinism;
mod document;
mod semantic;

#[cfg(test)]
mod tests {
    use super::*;
    use tondo_conformance::protocol::{TargetSelection, WireSource};

    fn request(action: AdapterAction) -> AdapterRequest {
        AdapterRequest::new(
            1,
            "test",
            TargetSelection {
                name: "tondo-vm-hosted".into(),
                profile: "hosted".into(),
                capabilities: vec!["console".into(), "process".into()],
            },
            action,
        )
    }

    #[test]
    fn describe_is_stable_and_source_programs_use_the_public_driver() {
        let mut adapter = ReferenceAdapter;
        let described = adapter.handle(&request(AdapterAction::Describe));
        assert!(matches!(described.result, AdapterResult::Ok { .. }));

        let source = WireSourceAction {
            operation: WireOperation::Run,
            form: WireSourceForm::Script,
            root: "main.to".into(),
            sources: vec![WireSource {
                source_id: "root:adapter-test".into(),
                module: "main".into(),
                logical_path: "main.to".into(),
                contents_hex: tondo_conformance::encode_hex(
                    b"import std.console\nconsole.print(\"ok\\n\")\n",
                ),
            }],
            warning_profiles: Vec::new(),
            arguments: Vec::new(),
            gc_threshold: None,
        };
        let response = adapter.handle(&request(AdapterAction::Source(source)));
        let AdapterResult::Ok { observation } = response.result else {
            panic!("reference adapter rejected a valid request");
        };
        assert_eq!(observation.compilation, CompilationState::Success);
        assert_eq!(
            tondo_conformance::decode_hex(&observation.stdout_hex).unwrap(),
            b"ok\n"
        );
    }
}
