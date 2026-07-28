use std::error::Error;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::document::extract_fences;
use crate::manifest::{
    CaseAction, ConformanceCase, DeterminismAction, Expectation, LoadedSuite, SemanticQuery,
    SourceAction, SourceForm, SourceOperation,
};
use crate::protocol::{
    AdapterAction, AdapterRequest, AdapterResponse, AdapterResult, CompilationState, DocCategory,
    Observation, TargetSelection, WireBuildInput, WireDeterminismAction, WireDocumentFenceAction,
    WireOperation, WireSemanticAction, WireSource, WireSourceAction, WireSourceForm,
};
use crate::{ADAPTER_PROTOCOL, RESULT_FORMAT, decode_hex, encode_hex, sha256};

pub trait Adapter {
    fn exchange(&mut self, request: &AdapterRequest) -> Result<AdapterResponse, String>;
}

pub struct ProcessAdapter {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ProcessAdapter {
    pub fn spawn(executable: impl AsRef<Path>) -> Result<Self, String> {
        let mut child = Command::new(executable.as_ref())
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start adapter `{}`: {error}",
                    executable.as_ref().display()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "adapter stdin was not piped".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "adapter stdout was not piped".to_owned())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
}

impl Adapter for ProcessAdapter {
    fn exchange(&mut self, request: &AdapterRequest) -> Result<AdapterResponse, String> {
        serde_json::to_writer(&mut self.stdin, request)
            .map_err(|error| format!("cannot encode adapter request: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .and_then(|()| self.stdin.flush())
            .map_err(|error| format!("cannot write adapter request: {error}"))?;
        let mut line = String::new();
        let count = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| format!("cannot read adapter response: {error}"))?;
        if count == 0 {
            let status = self
                .child
                .try_wait()
                .map_err(|error| format!("cannot inspect adapter process: {error}"))?;
            return Err(format!(
                "adapter closed stdout before responding (status {status:?})"
            ));
        }
        serde_json::from_str(line.trim_end())
            .map_err(|error| format!("invalid adapter response JSON: {error}"))
    }
}

impl Drop for ProcessAdapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseResult {
    pub id: String,
    pub group: crate::manifest::CaseGroup,
    pub repetitions: u32,
    pub observation_sha256: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteResult {
    pub format: String,
    pub suite: String,
    pub suite_version: String,
    pub edition: String,
    pub manifest_sha256: String,
    pub adapter: Value,
    pub target: TargetSelection,
    pub passed: bool,
    pub cases: Vec<CaseResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationExpectation {
    compilation: CompilationState,
    exit_code: i32,
    diagnostic_codes: Vec<String>,
    exact_diagnostics: Option<Vec<Value>>,
    stdout_hex: String,
    stderr_hex: String,
    formatted_hex: Option<String>,
    data: Value,
}

#[derive(Debug)]
pub enum RunError {
    Adapter(String),
    Protocol(String),
    Case { id: String, message: String },
    Document(String),
    Json(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adapter(message) => write!(formatter, "adapter failed: {message}"),
            Self::Protocol(message) => write!(formatter, "adapter protocol failed: {message}"),
            Self::Case { id, message } => write!(formatter, "case `{id}` failed: {message}"),
            Self::Document(message) => write!(formatter, "document case failed: {message}"),
            Self::Json(message) => write!(formatter, "result JSON failed: {message}"),
        }
    }
}

impl Error for RunError {}

pub fn run_suite(
    suite: &LoadedSuite,
    adapter: &mut dyn Adapter,
    group: Option<crate::manifest::CaseGroup>,
) -> Result<SuiteResult, RunError> {
    let target = suite
        .manifest()
        .targets
        .first()
        .expect("manifest validation requires one target");
    let target = TargetSelection {
        name: target.name.clone(),
        profile: target.profile.clone(),
        capabilities: target.capabilities.clone(),
    };
    let describe_request = AdapterRequest::new(
        0,
        "adapter/describe",
        target.clone(),
        AdapterAction::Describe,
    );
    let describe = exchange(adapter, &describe_request)?;
    let adapter_description = match describe.result {
        AdapterResult::Ok { observation } => observation.data,
        AdapterResult::Unsupported { reason } => {
            return Err(RunError::Adapter(format!(
                "adapter does not support describe: {reason}"
            )));
        }
        AdapterResult::Error { message } => return Err(RunError::Adapter(message)),
    };

    let mut sequence = 1;
    let mut results = Vec::new();
    for case in &suite.manifest().cases {
        if group.is_some_and(|selected| case.group != selected) {
            continue;
        }
        let observations = execute_case(suite, adapter, case, &mut sequence)?;
        let hashes = observations
            .iter()
            .map(canonical_observation_hash)
            .collect::<Result<Vec<_>, _>>()?;
        results.push(CaseResult {
            id: case.id.clone(),
            group: case.group,
            repetitions: u32::try_from(observations.len()).unwrap_or(u32::MAX),
            observation_sha256: hashes,
        });
    }
    Ok(SuiteResult {
        format: RESULT_FORMAT.into(),
        suite: suite.manifest().suite.clone(),
        suite_version: suite.manifest().version.clone(),
        edition: suite.manifest().edition.clone(),
        manifest_sha256: suite.manifest_sha256(),
        adapter: adapter_description,
        target,
        passed: true,
        cases: results,
    })
}

fn execute_case(
    suite: &LoadedSuite,
    adapter: &mut dyn Adapter,
    case: &ConformanceCase,
    sequence: &mut u64,
) -> Result<Vec<Observation>, RunError> {
    if let CaseAction::Document(action) = &case.action {
        return execute_document_case(suite, adapter, case, action, sequence);
    }
    let action = wire_action(suite, &case.action)?;
    let expected = load_expectations(suite, case)?;
    let mut observations = Vec::with_capacity(case.repeat as usize);
    for repetition in 0..case.repeat {
        let request = AdapterRequest::new(
            *sequence,
            format!("{}#{repetition}", case.id),
            case_target(case),
            action.clone(),
        );
        *sequence = sequence.saturating_add(1);
        let response = exchange(adapter, &request)?;
        let observation = response_observation(case, response)?;
        assert_expected(case, &observation, &expected)?;
        validate_safe_fixes(adapter, case, &action, &observation, repetition, sequence)?;
        validate_coverage_claims(case, &observation)?;
        validate_format_idempotence(adapter, case, &action, &observation, repetition, sequence)?;
        observations.push(observation);
    }
    Ok(observations)
}

fn validate_format_idempotence(
    adapter: &mut dyn Adapter,
    case: &ConformanceCase,
    action: &AdapterAction,
    observation: &Observation,
    repetition: u32,
    sequence: &mut u64,
) -> Result<(), RunError> {
    let AdapterAction::Source(source) = action else {
        return Ok(());
    };
    if source.operation != WireOperation::Format
        || observation.compilation != CompilationState::Success
    {
        return Ok(());
    }
    let formatted = observation
        .formatted_hex
        .as_ref()
        .ok_or_else(|| RunError::Case {
            id: case.id.clone(),
            message: "successful format observation omitted formatter bytes".into(),
        })?;
    let mut second = source.clone();
    let root = second
        .sources
        .iter_mut()
        .find(|candidate| candidate.logical_path == second.root)
        .ok_or_else(|| RunError::Protocol("format action lost its root source".into()))?;
    root.contents_hex.clone_from(formatted);
    let request = AdapterRequest::new(
        *sequence,
        format!("{}#{repetition}/idempotence", case.id),
        case_target(case),
        AdapterAction::Source(second),
    );
    *sequence = sequence.saturating_add(1);
    let second = response_observation(case, exchange(adapter, &request)?)?;
    validate_diagnostic_protocol(&second).map_err(|message| RunError::Case {
        id: case.id.clone(),
        message,
    })?;
    if second.compilation != CompilationState::Success
        || second.exit_code != 0
        || !second.diagnostics.is_empty()
        || second.formatted_hex.as_ref() != Some(formatted)
    {
        return case_failure(
            case,
            "formatting the canonical output did not reproduce identical bytes",
        );
    }
    Ok(())
}

fn validate_safe_fixes(
    adapter: &mut dyn Adapter,
    case: &ConformanceCase,
    action: &AdapterAction,
    observation: &Observation,
    repetition: u32,
    sequence: &mut u64,
) -> Result<(), RunError> {
    let source = match action {
        AdapterAction::Source(source)
        | AdapterAction::Semantic(WireSemanticAction { source, .. })
            if source.operation == WireOperation::Check =>
        {
            source
        }
        _ => return Ok(()),
    };
    for diagnostic in &observation.diagnostics {
        let diagnostic = diagnostic
            .as_object()
            .expect("diagnostic protocol validation ran before safe fix replay");
        let code = diagnostic["code"]
            .as_str()
            .expect("diagnostic code was validated");
        for (fix_index, fix) in diagnostic["fixes"]
            .as_array()
            .expect("diagnostic fixes were validated")
            .iter()
            .enumerate()
        {
            if fix["applicability"] != "safe" {
                continue;
            }
            let mut patched = source.clone();
            apply_fix(&mut patched, fix).map_err(|message| RunError::Case {
                id: case.id.clone(),
                message: format!("safe fix for `{code}` cannot be replayed: {message}"),
            })?;
            let request = AdapterRequest::new(
                *sequence,
                format!("{}#{repetition}/safe-fix-{fix_index}", case.id),
                case_target(case),
                AdapterAction::Source(patched),
            );
            *sequence = sequence.saturating_add(1);
            let fixed = response_observation(case, exchange(adapter, &request)?)?;
            validate_diagnostic_protocol(&fixed).map_err(|message| RunError::Case {
                id: case.id.clone(),
                message,
            })?;
            let remaining_errors = fixed
                .diagnostic_codes()
                .map_err(|message| RunError::Case {
                    id: case.id.clone(),
                    message,
                })?
                .into_iter()
                .filter(|code| !code.starts_with('W'))
                .collect::<Vec<_>>();
            if fixed.compilation != CompilationState::Success
                || fixed.exit_code != 0
                || !remaining_errors.is_empty()
            {
                return case_failure(
                    case,
                    format!(
                        "safe fix for `{code}` did not produce an error-free check; remaining {remaining_errors:?}"
                    ),
                );
            }
        }
    }
    Ok(())
}

fn apply_fix(source: &mut WireSourceAction, fix: &Value) -> Result<(), String> {
    let edits = fix["edits"]
        .as_array()
        .ok_or_else(|| "fix edits must be an array".to_owned())?;
    let mut by_source = std::collections::BTreeMap::<usize, Vec<(usize, usize, Vec<u8>)>>::new();
    for edit in edits {
        let edit = edit
            .as_object()
            .ok_or_else(|| "fix edit must be an object".to_owned())?;
        let source_id = string_field(edit, "source_id")?;
        let module = string_field(edit, "module")?;
        let file = string_field(edit, "file")?;
        let source_index = source
            .sources
            .iter()
            .position(|source| {
                source.source_id == source_id
                    && source.module == module
                    && source.logical_path == file
            })
            .ok_or_else(|| {
                format!(
                    "edit target `{source_id}` module `{module}` file `{file}` is not in the request"
                )
            })?;
        let (start, end) = validate_range(&edit["range"], "fix replay range")?;
        let start = usize::try_from(start.byte)
            .map_err(|_| "fix start does not fit this host".to_owned())?;
        let end =
            usize::try_from(end.byte).map_err(|_| "fix end does not fit this host".to_owned())?;
        by_source.entry(source_index).or_default().push((
            start,
            end,
            string_field(edit, "replacement")?.as_bytes().to_vec(),
        ));
    }
    for (source_index, mut edits) in by_source {
        let selected = source
            .sources
            .get_mut(source_index)
            .ok_or_else(|| "fix source index disappeared".to_owned())?;
        let mut bytes = decode_hex(&selected.contents_hex)?;
        edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
        for (start, end, replacement) in edits {
            if start > end || end > bytes.len() {
                return Err(format!(
                    "edit range {start}..{end} exceeds {} source bytes",
                    bytes.len()
                ));
            }
            bytes.splice(start..end, replacement);
        }
        selected.contents_hex = encode_hex(&bytes);
    }
    Ok(())
}

fn execute_document_case(
    suite: &LoadedSuite,
    adapter: &mut dyn Adapter,
    case: &ConformanceCase,
    action: &crate::manifest::DocumentAction,
    sequence: &mut u64,
) -> Result<Vec<Observation>, RunError> {
    let registered = suite.manifest().registry.errors.iter().cloned().collect();
    let markdown = suite.file(&action.markdown);
    let fences = extract_fences(markdown, &registered)
        .map_err(|error| RunError::Document(error.to_string()))?;
    let fixture_manifest = suite.file(&suite.manifest().fixture_manifest);
    let mut records = Vec::with_capacity(fences.len());
    for fence in fences {
        if fence.category == DocCategory::Pseudocode {
            records.push(pseudocode_record(
                &action.markdown.path,
                &fence,
                &suite.manifest().edition,
            ));
            continue;
        }
        let request = AdapterRequest::new(
            *sequence,
            format!("{}@{}", case.id, fence.fence_byte),
            case_target(case),
            AdapterAction::DocumentFence(WireDocumentFenceAction {
                file: action.markdown.path.clone(),
                fence_byte: fence.fence_byte,
                category: fence.category,
                fixture: fence.fixture.clone(),
                fixture_manifest_hex: encode_hex(fixture_manifest),
                fixture_manifest_sha256: suite.manifest().fixture_manifest.sha256.clone(),
                expected_codes: fence.expected_codes.clone(),
                source_hex: encode_hex(&fence.source),
            }),
        );
        *sequence = sequence.saturating_add(1);
        let response = exchange(adapter, &request)?;
        let observation = response_observation(case, response)?;
        if observation.compilation == CompilationState::Rejected
            && fence.category != DocCategory::CompileFail
        {
            return case_failure(
                case,
                format!("fence at byte {} was rejected", fence.fence_byte),
            );
        }
        records.push(observation.data);
    }
    let observation = Observation {
        compilation: CompilationState::Success,
        exit_code: 0,
        diagnostics: Vec::new(),
        stdout_hex: String::new(),
        stderr_hex: String::new(),
        formatted_hex: None,
        data: Value::Array(records),
    };
    let expected = load_expectations(suite, case)?;
    assert_expected(case, &observation, &expected)?;
    Ok(vec![observation])
}

fn pseudocode_record(file: &str, fence: &crate::document::DocumentFence, edition: &str) -> Value {
    serde_json::json!({
        "file": file,
        "fence_byte": fence.fence_byte,
        "category": "pseudocode",
        "edition": edition,
        "fixture": null,
        "fixture_sha256": null,
        "production": null,
        "source_sha256": fence.source_sha256,
        "formatted_sha256": null,
        "parse_ok": null,
        "typecheck_ok": null,
        "expected_codes": [],
        "actual_codes": []
    })
}

fn wire_action(suite: &LoadedSuite, action: &CaseAction) -> Result<AdapterAction, RunError> {
    Ok(match action {
        CaseAction::Source(action) => AdapterAction::Source(wire_source_action(suite, action)),
        CaseAction::Semantic(action) => AdapterAction::Semantic(WireSemanticAction {
            source: wire_source_action(suite, &action.source),
            queries: action.queries.clone(),
        }),
        CaseAction::Memory { scenario } => AdapterAction::Memory {
            scenario: *scenario,
        },
        CaseAction::Determinism(action) => {
            AdapterAction::Determinism(wire_determinism_action(suite, action))
        }
        CaseAction::Document(_) => {
            return Err(RunError::Protocol(
                "document actions are expanded before adapter dispatch".into(),
            ));
        }
    })
}

fn wire_source_action(suite: &LoadedSuite, action: &SourceAction) -> WireSourceAction {
    WireSourceAction {
        operation: match action.operation {
            SourceOperation::Format => WireOperation::Format,
            SourceOperation::Check => WireOperation::Check,
            SourceOperation::Run => WireOperation::Run,
        },
        form: match action.form {
            SourceForm::Module => WireSourceForm::Module,
            SourceForm::Script => WireSourceForm::Script,
            SourceForm::Fragment => WireSourceForm::Fragment,
            SourceForm::Syntax => WireSourceForm::Syntax,
            SourceForm::StandaloneBlock => WireSourceForm::StandaloneBlock,
        },
        root: action.root.clone(),
        sources: action
            .sources
            .iter()
            .map(|source| WireSource {
                source_id: source.source_id.clone(),
                module: source.module.clone(),
                logical_path: source.logical_path.clone(),
                contents_hex: encode_hex(suite.file(&source.contents)),
            })
            .collect(),
        warning_profiles: action.warning_profiles.clone(),
        arguments: action.arguments.clone(),
        gc_threshold: action.gc_threshold,
    }
}

fn wire_determinism_action(
    suite: &LoadedSuite,
    action: &DeterminismAction,
) -> WireDeterminismAction {
    WireDeterminismAction {
        manifest_hex: encode_hex(suite.file(&action.manifest)),
        lockfile_hex: encode_hex(suite.file(&action.lockfile)),
        inputs: action
            .inputs
            .iter()
            .map(|input| WireBuildInput {
                logical_path: input.logical_path.clone(),
                contents_hex: encode_hex(suite.file(&input.contents)),
            })
            .collect(),
    }
}

fn case_target(case: &ConformanceCase) -> TargetSelection {
    TargetSelection {
        name: case.target.clone(),
        profile: case.profile.clone(),
        capabilities: case.capabilities.clone(),
    }
}

fn exchange(
    adapter: &mut dyn Adapter,
    request: &AdapterRequest,
) -> Result<AdapterResponse, RunError> {
    let response = adapter.exchange(request).map_err(RunError::Adapter)?;
    if response.protocol != ADAPTER_PROTOCOL {
        return Err(RunError::Protocol(format!(
            "response used protocol `{}`",
            response.protocol
        )));
    }
    if response.sequence != request.sequence || response.case_id != request.case_id {
        return Err(RunError::Protocol(
            "response identity does not match its request".into(),
        ));
    }
    Ok(response)
}

fn response_observation(
    case: &ConformanceCase,
    response: AdapterResponse,
) -> Result<Observation, RunError> {
    match response.result {
        AdapterResult::Ok { observation } => Ok(observation),
        AdapterResult::Unsupported { reason } => case_failure(
            case,
            format!("adapter excluded an applicable case instead of implementing it: {reason}"),
        ),
        AdapterResult::Error { message } => case_failure(
            case,
            format!("adapter reported an internal error: {message}"),
        ),
    }
}

fn load_expectations(suite: &LoadedSuite, case: &ConformanceCase) -> Result<Vec<Value>, RunError> {
    let value = suite
        .json_file(case.expectation.pinned_file())
        .map_err(|error| RunError::Json(error.to_string()))?;
    Ok(match &case.expectation {
        Expectation::Exact { .. } => vec![value],
        Expectation::OneOf { .. } => value
            .as_array()
            .expect("manifest validation checks one-of expectations")
            .clone(),
    })
}

fn assert_expected(
    case: &ConformanceCase,
    observation: &Observation,
    expected: &[Value],
) -> Result<(), RunError> {
    validate_diagnostic_protocol(observation).map_err(|message| RunError::Case {
        id: case.id.clone(),
        message,
    })?;
    if let CaseAction::Semantic(action) = &case.action {
        validate_semantic_protocol(&action.queries, observation).map_err(|message| {
            RunError::Case {
                id: case.id.clone(),
                message,
            }
        })?;
    }
    let expected = expected
        .iter()
        .map(|value| {
            serde_json::from_value::<ObservationExpectation>(value.clone())
                .map_err(|error| RunError::Json(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if expected
        .iter()
        .any(|pattern| observation_matches(observation, pattern))
    {
        return Ok(());
    }
    let actual =
        serde_json::to_value(observation).map_err(|error| RunError::Json(error.to_string()))?;
    let actual =
        serde_json::to_string_pretty(&actual).map_err(|error| RunError::Json(error.to_string()))?;
    case_failure(case, format!("observation did not match:\n{actual}"))
}

fn observation_matches(observation: &Observation, expected: &ObservationExpectation) -> bool {
    let codes = observation
        .diagnostic_codes()
        .expect("diagnostic protocol validation ran before matching");
    observation.compilation == expected.compilation
        && observation.exit_code == expected.exit_code
        && codes == expected.diagnostic_codes
        && expected
            .exact_diagnostics
            .as_ref()
            .is_none_or(|diagnostics| diagnostics == &observation.diagnostics)
        && observation.stdout_hex == expected.stdout_hex
        && observation.stderr_hex == expected.stderr_hex
        && observation.formatted_hex == expected.formatted_hex
        && observation.data == expected.data
}

fn validate_diagnostic_protocol(observation: &Observation) -> Result<(), String> {
    let mut previous: Option<DiagnosticKey> = None;
    let mut ids = std::collections::BTreeSet::new();
    for diagnostic in &observation.diagnostics {
        let object = diagnostic
            .as_object()
            .ok_or_else(|| "diagnostic must be a JSON object".to_owned())?;
        require_exact_keys(
            object,
            &[
                "actual",
                "code",
                "expected",
                "file",
                "fixes",
                "id",
                "message",
                "module",
                "range",
                "related",
                "severity",
                "source_id",
            ],
            "diagnostic",
        )?;
        let id = string_field(object, "id")?;
        if !ids.insert(id) {
            return Err(format!("duplicate diagnostic ID `{id}`"));
        }
        let code = string_field(object, "code")?;
        let severity = string_field(object, "severity")?;
        if !matches!(severity, "error" | "warning") {
            return Err(format!(
                "diagnostic `{code}` has invalid severity `{severity}`"
            ));
        }
        if (code.starts_with('W') && severity != "warning")
            || (!code.starts_with('W') && severity != "error")
        {
            return Err(format!(
                "diagnostic `{code}` has inconsistent severity `{severity}`"
            ));
        }
        if code.len() != 5
            || !matches!(code.as_bytes()[0], b'E' | b'W' | b'P')
            || !code.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(format!(
                "conformance observation contains non-normative diagnostic `{code}`"
            ));
        }
        let message = string_field(object, "message")?;
        if message.is_empty() || message.contains('\n') {
            return Err(format!("diagnostic `{code}` has an invalid message"));
        }
        validate_nullable_string(object, "expected")?;
        validate_nullable_string(object, "actual")?;
        let source_id = string_field(object, "source_id")?;
        if source_id.contains('\n') {
            return Err(format!("diagnostic `{code}` has an invalid source ID"));
        }
        let module = nullable_string_field(object, "module")?;
        let file = nullable_string_field(object, "file")?;
        let range = object
            .get("range")
            .ok_or_else(|| "diagnostic lacks `range`".to_owned())?;
        let (start, end) = if range.is_null() {
            if module.is_some() || file.is_some() {
                return Err(format!("diagnostic `{code}` has a partial target location"));
            }
            (None, None)
        } else {
            if module.is_none() || file.is_none() {
                return Err(format!("diagnostic `{code}` has a partial source location"));
            }
            let (start, end) = validate_range(range, "diagnostic range")?;
            (Some(start), Some(end))
        };
        let expected_id = diagnostic_id(
            source_id,
            module,
            file,
            code,
            start.as_ref().map(|position| position.byte),
            end.as_ref().map(|position| position.byte),
        );
        if id != expected_id {
            return Err(format!(
                "diagnostic `{code}` ID `{id}` does not match `{expected_id}`"
            ));
        }

        let related = object["related"]
            .as_array()
            .ok_or_else(|| format!("diagnostic `{code}` related must be an array"))?;
        let mut previous_related = None;
        for item in related {
            let key = validate_related(item)?;
            if previous_related
                .as_ref()
                .is_some_and(|previous| previous >= &key)
            {
                return Err(format!(
                    "diagnostic `{code}` related locations are not sorted and unique"
                ));
            }
            previous_related = Some(key);
        }
        let fixes = object["fixes"]
            .as_array()
            .ok_or_else(|| format!("diagnostic `{code}` fixes must be an array"))?;
        let mut previous_fix = None;
        for fix in fixes {
            let key = validate_fix(fix)?;
            if previous_fix
                .as_ref()
                .is_some_and(|previous| previous >= &key)
            {
                return Err(format!(
                    "diagnostic `{code}` fixes are not sorted and unique"
                ));
            }
            previous_fix = Some(key);
        }

        let key = DiagnosticKey {
            source_id: source_id.to_owned(),
            module: module.map(str::to_owned),
            file: file.map(str::to_owned),
            start: start.as_ref().map(|position| position.byte),
            end: end.as_ref().map(|position| position.byte),
            severity: severity_order(severity),
            code: code.to_owned(),
            message: message.to_owned(),
        };
        if previous.as_ref().is_some_and(|previous| previous >= &key) {
            return Err("diagnostics are not sorted and unique".into());
        }
        previous = Some(key);
    }
    Ok(())
}

fn validate_semantic_protocol(
    requested: &[SemanticQuery],
    observation: &Observation,
) -> Result<(), String> {
    let object = observation
        .data
        .as_object()
        .ok_or_else(|| "semantic observation data must be an object".to_owned())?;
    require_exact_keys(
        object,
        &["expression_check_complete", "queries", "schema"],
        "semantic observation",
    )?;
    if string_field(object, "schema")? != "tondo-semantic-observation-0.1/1" {
        return Err("semantic observation uses an unknown schema".into());
    }
    if !object["expression_check_complete"].is_boolean() {
        return Err("semantic `expression_check_complete` must be boolean".into());
    }
    let results = object["queries"]
        .as_array()
        .ok_or_else(|| "semantic `queries` must be an array".to_owned())?;
    if results.len() != requested.len() {
        return Err(format!(
            "semantic observation returned {} results for {} queries",
            results.len(),
            requested.len()
        ));
    }
    for (index, (query, result)) in requested.iter().zip(results).enumerate() {
        validate_semantic_query_result(query, result)
            .map_err(|message| format!("semantic query {index}: {message}"))?;
    }
    validate_semantic_tree(&observation.data, "semantic observation")
}

fn validate_semantic_query_result(query: &SemanticQuery, value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "result must be an object".to_owned())?;
    let (name, keys): (&str, &[&str]) = match query {
        SemanticQuery::ExpressionType { .. } => ("expression-type", &["query", "type"]),
        SemanticQuery::Entities { .. } => ("entities", &["entities", "query"]),
        SemanticQuery::References { .. } => ("references", &["entity_id", "query", "references"]),
        SemanticQuery::Signature { .. } => ("signature", &["entity_id", "query", "signature"]),
        SemanticQuery::TypeMembers { .. } => ("type-members", &["members", "query", "type"]),
        SemanticQuery::ClosedCallErrors { .. } => ("closed-call-errors", &["errors", "query"]),
        SemanticQuery::TypeFacts { .. } => ("type-facts", &["facts", "query"]),
        SemanticQuery::ExpressionFacts { .. } => ("expression-facts", &["facts", "query"]),
        SemanticQuery::SemanticSnapshot { .. } => (
            "semantic-snapshot",
            &[
                "borrow_bindings",
                "closures",
                "declarations",
                "expressions",
                "file",
                "iterators",
                "opaque_results",
                "ownership",
                "public_types",
                "query",
                "references",
                "schema",
                "unsafe",
            ],
        ),
        SemanticQuery::FormattedAst => (
            "formatted-ast",
            &["encoding", "formatted_hex", "query", "sha256"],
        ),
    };
    require_exact_keys(object, keys, "semantic query result")?;
    if string_field(object, "query")? != name {
        return Err(format!(
            "result tag `{}` does not match requested `{name}`",
            string_field(object, "query")?
        ));
    }
    match query {
        SemanticQuery::SemanticSnapshot { file } => {
            if string_field(object, "schema")? != "tondo-semantic-snapshot-0.1/1" {
                return Err("semantic snapshot uses an unknown schema".into());
            }
            if string_field(object, "file")? != file {
                return Err("semantic snapshot returned a different file".into());
            }
            let ownership = object["ownership"]
                .as_object()
                .ok_or_else(|| "semantic ownership facts must be an object".to_owned())?;
            require_exact_keys(ownership, &["functions", "schema"], "semantic ownership")?;
            if string_field(ownership, "schema")? != "tondo-semantic-ownership-0.1/1" {
                return Err("semantic ownership facts use an unknown schema".into());
            }
        }
        SemanticQuery::FormattedAst => {
            if string_field(object, "encoding")? != "utf-8" {
                return Err("formatted AST encoding must be UTF-8".into());
            }
            decode_hex(string_field(object, "formatted_hex")?)?;
            validate_sha256(string_field(object, "sha256")?)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_semantic_tree(value: &Value, context: &str) -> Result<(), String> {
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_semantic_tree(value, &format!("{context}[{index}]"))?;
            }
        }
        Value::Object(object) => {
            if is_semantic_span(object) {
                validate_semantic_span(object, context)?;
                return Ok(());
            }
            for (field, value) in object {
                if is_semantic_id_field(field) && !value.is_null() {
                    let id = value
                        .as_str()
                        .ok_or_else(|| format!("{context}.{field} must be a semantic ID"))?;
                    validate_semantic_id(id)
                        .map_err(|message| format!("{context}.{field}: {message}"))?;
                }
                validate_semantic_tree(value, &format!("{context}.{field}"))?;
                if let Some(values) = value.as_array()
                    && let Some(keys) = semantic_array_sort_keys(field)
                {
                    validate_semantic_order(values, keys, &format!("{context}.{field}"))?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn is_semantic_span(object: &serde_json::Map<String, Value>) -> bool {
    object.keys().map(String::as_str).collect::<Vec<_>>()
        == ["end", "file", "module", "source_id", "start"]
}

fn validate_semantic_span(
    object: &serde_json::Map<String, Value>,
    context: &str,
) -> Result<(), String> {
    for field in ["source_id", "module", "file"] {
        let value = string_field(object, field)?;
        if value.is_empty() || value.contains(['\n', '\r']) {
            return Err(format!(
                "{context}.{field} must be non-empty and single-line"
            ));
        }
    }
    let start = object["start"]
        .as_u64()
        .ok_or_else(|| format!("{context}.start must be unsigned"))?;
    let end = object["end"]
        .as_u64()
        .ok_or_else(|| format!("{context}.end must be unsigned"))?;
    if start > end {
        return Err(format!("{context} is a reversed semantic span"));
    }
    Ok(())
}

fn is_semantic_id_field(field: &str) -> bool {
    field != "source_id" && (field == "id" || field.ends_with("_id"))
}

fn validate_semantic_id(id: &str) -> Result<(), String> {
    if matches!(id, "sem:receiver:self" | "sem:type:Self") {
        return Ok(());
    }
    let mut parts = id.split(':');
    let valid = parts.next() == Some("sem")
        && parts.next().is_some_and(|kind| {
            !kind.is_empty()
                && kind
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        && parts.next().is_some_and(|digest| {
            digest.len() == 24
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        && parts.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(format!("invalid stable semantic ID `{id}`"))
    }
}

fn semantic_array_sort_keys(field: &str) -> Option<&'static [&'static str]> {
    Some(match field {
        "declarations" => &["declaration", "id"],
        "references" => &["span", "entity_id"],
        "expressions" | "closures" | "opaque_results" | "iterators" | "joins" | "regions"
        | "operations" | "functions" | "dynamic_checks" => &["span", "id"],
        "public_types" => &["identity", "id"],
        "borrow_bindings" | "affine_values" => &["declaration", "id"],
        "loans" | "against" => &["id"],
        "reserve_sites" | "release_sites" => &["span", "block_id", "sequence"],
        "events" => &["span", "block_id", "sequence", "id"],
        "transfers" => &["span", "from_local_id", "to_local_id"],
        _ => return None,
    })
}

fn validate_semantic_order(values: &[Value], keys: &[&str], context: &str) -> Result<(), String> {
    for pair in values.windows(2) {
        if compare_semantic_items(&pair[0], &pair[1], keys) != std::cmp::Ordering::Less {
            return Err(format!("{context} is not sorted and unique"));
        }
    }
    Ok(())
}

fn compare_semantic_items(left: &Value, right: &Value, keys: &[&str]) -> std::cmp::Ordering {
    for key in keys {
        let left = semantic_item_field(left, key);
        let right = semantic_item_field(right, key);
        let ordering = compare_json_values(left, right);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn semantic_item_field<'a>(value: &'a Value, field: &str) -> &'a Value {
    if (field == "id" && value.is_string())
        || (field == "span" && value.as_object().is_some_and(is_semantic_span))
    {
        value
    } else {
        value.get(field).unwrap_or(&Value::Null)
    }
}

fn compare_json_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    };
    match rank(left).cmp(&rank(right)) {
        Ordering::Equal => {}
        ordering => return ordering,
    }
    match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        (Value::Number(left), Value::Number(right)) => match (left.as_i64(), right.as_i64()) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => match (left.as_u64(), right.as_u64()) {
                (Some(left), Some(right)) => left.cmp(&right),
                _ => left.to_string().cmp(&right.to_string()),
            },
        },
        (Value::String(left), Value::String(right)) => left.cmp(right),
        (Value::Array(left), Value::Array(right)) => left
            .iter()
            .zip(right)
            .find_map(|(left, right)| {
                let ordering = compare_json_values(left, right);
                (!ordering.is_eq()).then_some(ordering)
            })
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        (Value::Object(left), Value::Object(right)) => left
            .iter()
            .zip(right)
            .find_map(|((left_key, left), (right_key, right))| {
                let ordering = left_key.cmp(right_key);
                if ordering.is_eq() {
                    let ordering = compare_json_values(left, right);
                    (!ordering.is_eq()).then_some(ordering)
                } else {
                    Some(ordering)
                }
            })
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        _ => unreachable!("equal JSON ranks have matching variants"),
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!("invalid lowercase SHA-256 `{value}`"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    byte: u64,
    line: Option<u64>,
    column: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DiagnosticKey {
    source_id: String,
    module: Option<String>,
    file: Option<String>,
    start: Option<u64>,
    end: Option<u64>,
    severity: u8,
    code: String,
    message: String,
}

fn validate_range(value: &Value, context: &str) -> Result<(Position, Position), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    require_exact_keys(object, &["end", "start"], context)?;
    let start = validate_position(&object["start"], context)?;
    let end = validate_position(&object["end"], context)?;
    if start.byte > end.byte {
        return Err(format!("{context} is reversed"));
    }
    Ok((start, end))
}

fn validate_position(value: &Value, context: &str) -> Result<Position, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} position must be an object"))?;
    let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    if keys != ["byte", "column", "line"] && keys != ["byte"] {
        return Err(format!("{context} position has invalid keys {keys:?}"));
    }
    let byte = object["byte"]
        .as_u64()
        .ok_or_else(|| format!("{context} byte must be unsigned"))?;
    let line = object.get("line").and_then(Value::as_u64);
    let column = object.get("column").and_then(Value::as_u64);
    if object.contains_key("line") && (line.is_none() || column.is_none()) {
        return Err(format!(
            "{context} line and column must both be unsigned when present"
        ));
    }
    Ok(Position { byte, line, column })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelatedKey {
    source_id: String,
    module: String,
    file: String,
    start: Position,
    end: Position,
    message: String,
}

fn validate_related(value: &Value) -> Result<RelatedKey, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "related entry must be an object".to_owned())?;
    require_exact_keys(
        object,
        &["file", "message", "module", "range", "source_id"],
        "related entry",
    )?;
    let (start, end) = validate_range(&object["range"], "related range")?;
    Ok(RelatedKey {
        source_id: string_field(object, "source_id")?.to_owned(),
        module: string_field(object, "module")?.to_owned(),
        file: string_field(object, "file")?.to_owned(),
        start,
        end,
        message: string_field(object, "message")?.to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EditKey {
    source_id: String,
    module: String,
    file: String,
    start: Position,
    end: Position,
    replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FixKey {
    applicability: u8,
    title: String,
    edits: Vec<EditKey>,
}

fn validate_fix(value: &Value) -> Result<FixKey, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "fix must be an object".to_owned())?;
    require_exact_keys(object, &["applicability", "edits", "title"], "fix")?;
    let applicability = match string_field(object, "applicability")? {
        "safe" => 0,
        "requires-decision" => 1,
        value => return Err(format!("fix has invalid applicability `{value}`")),
    };
    let title = string_field(object, "title")?;
    if title.is_empty() {
        return Err("fix title cannot be empty".into());
    }
    let values = object["edits"]
        .as_array()
        .ok_or_else(|| "fix edits must be an array".to_owned())?;
    if values.is_empty() {
        return Err("fix must contain at least one edit".into());
    }
    let mut edits = Vec::with_capacity(values.len());
    for value in values {
        let object = value
            .as_object()
            .ok_or_else(|| "fix edit must be an object".to_owned())?;
        require_exact_keys(
            object,
            &["file", "module", "range", "replacement", "source_id"],
            "fix edit",
        )?;
        let (start, end) = validate_range(&object["range"], "fix edit range")?;
        edits.push(EditKey {
            source_id: string_field(object, "source_id")?.to_owned(),
            module: string_field(object, "module")?.to_owned(),
            file: string_field(object, "file")?.to_owned(),
            start,
            end,
            replacement: string_field(object, "replacement")?.to_owned(),
        });
    }
    if edits.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("fix edits are not sorted and unique".into());
    }
    for pair in edits.windows(2) {
        if pair[0].source_id == pair[1].source_id
            && pair[0].module == pair[1].module
            && pair[0].file == pair[1].file
            && pair[0].end.byte > pair[1].start.byte
        {
            return Err("fix edits overlap".into());
        }
    }
    Ok(FixKey {
        applicability,
        title: title.to_owned(),
        edits,
    })
}

fn require_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), String> {
    let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    if keys == expected {
        Ok(())
    } else {
        Err(format!(
            "{context} has keys {keys:?}, expected {expected:?}"
        ))
    }
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{name}` must be a string"))
}

fn nullable_string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, String> {
    let value = object
        .get(name)
        .ok_or_else(|| format!("missing `{name}`"))?;
    if value.is_null() {
        Ok(None)
    } else {
        value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("`{name}` must be a string or null"))
    }
}

fn validate_nullable_string(
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<(), String> {
    nullable_string_field(object, name).map(drop)
}

fn severity_order(value: &str) -> u8 {
    match value {
        "error" => 0,
        "warning" => 1,
        _ => u8::MAX,
    }
}

fn diagnostic_id(
    source_id: &str,
    module: Option<&str>,
    file: Option<&str>,
    code: &str,
    start: Option<u64>,
    end: Option<u64>,
) -> String {
    let input = format!(
        "0.1\n{source_id}\n{}\n{}\n{code}\n{}\n{}\n",
        module.unwrap_or_default(),
        file.unwrap_or_default(),
        start.map(|value| value.to_string()).unwrap_or_default(),
        end.map(|value| value.to_string()).unwrap_or_default(),
    );
    format!("diag:{}", sha256(input.as_bytes()))
}

fn validate_coverage_claims(
    case: &ConformanceCase,
    observation: &Observation,
) -> Result<(), RunError> {
    let codes = observation
        .diagnostic_codes()
        .map_err(|message| RunError::Case {
            id: case.id.clone(),
            message,
        })?;
    for covered in &case.covers {
        if !codes.contains(&covered.as_str()) {
            return case_failure(
                case,
                format!("declared coverage `{covered}` was not observed"),
            );
        }
    }
    for positive in &case.positive_for {
        if codes.contains(&positive.as_str()) {
            return case_failure(
                case,
                format!("positive neighbor unexpectedly produced `{positive}`"),
            );
        }
    }
    Ok(())
}

fn canonical_observation_hash(observation: &Observation) -> Result<String, RunError> {
    serde_json::to_vec(observation)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| RunError::Json(error.to_string()))
}

fn case_failure<T>(case: &ConformanceCase, message: impl Into<String>) -> Result<T, RunError> {
    Err(RunError::Case {
        id: case.id.clone(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AdapterResponse;

    struct IdentityAdapter;

    impl Adapter for IdentityAdapter {
        fn exchange(&mut self, request: &AdapterRequest) -> Result<AdapterResponse, String> {
            Ok(AdapterResponse::success(request, Observation::empty()))
        }
    }

    #[test]
    fn response_identity_and_protocol_are_checked() {
        let request = AdapterRequest::new(
            1,
            "case",
            TargetSelection {
                name: "target".into(),
                profile: "hosted".into(),
                capabilities: Vec::new(),
            },
            AdapterAction::Describe,
        );
        let mut adapter = IdentityAdapter;
        exchange(&mut adapter, &request).unwrap();

        struct WrongAdapter;
        impl Adapter for WrongAdapter {
            fn exchange(&mut self, request: &AdapterRequest) -> Result<AdapterResponse, String> {
                let mut response = AdapterResponse::success(request, Observation::empty());
                response.sequence += 1;
                Ok(response)
            }
        }
        assert!(exchange(&mut WrongAdapter, &request).is_err());
    }

    #[test]
    fn semantic_protocol_rejects_internal_or_unstable_ids() {
        let observation = Observation {
            data: serde_json::json!({
                "schema": "tondo-semantic-observation-0.1/1",
                "expression_check_complete": true,
                "queries": [{
                    "query": "entities",
                    "entities": [{
                        "id": "symbol#7",
                        "kind": "symbol",
                        "name": "value",
                        "declaration": null
                    }]
                }]
            }),
            ..Observation::empty()
        };
        let queries = [SemanticQuery::Entities {
            file: "case.to".into(),
            start: 0,
            end: 1,
        }];
        let error = validate_semantic_protocol(&queries, &observation).unwrap_err();
        assert!(error.contains("invalid stable semantic ID"), "{error}");
    }

    #[test]
    fn semantic_protocol_rejects_unsorted_source_spans() {
        let data = serde_json::json!({
            "references": [
                {
                    "source_id": "source",
                    "module": "main",
                    "file": "case.to",
                    "start": 10,
                    "end": 11
                },
                {
                    "source_id": "source",
                    "module": "main",
                    "file": "case.to",
                    "start": 2,
                    "end": 3
                }
            ]
        });
        let error = validate_semantic_tree(&data, "test").unwrap_err();
        assert!(error.contains("not sorted"), "{error}");
    }

    #[test]
    fn safe_fix_replay_applies_byte_ranges_to_the_exact_source() {
        let mut source = WireSourceAction {
            operation: WireOperation::Check,
            form: WireSourceForm::Module,
            root: "case.to".into(),
            sources: vec![WireSource {
                source_id: "source".into(),
                module: "main".into(),
                logical_path: "case.to".into(),
                contents_hex: encode_hex(b"type T = {\n    priv value: Int\n}\n"),
            }],
            warning_profiles: Vec::new(),
            arguments: Vec::new(),
            gc_threshold: None,
        };
        let fix = serde_json::json!({
            "edits": [{
                "source_id": "source",
                "module": "main",
                "file": "case.to",
                "range": {
                    "start": {"byte": 15},
                    "end": {"byte": 19}
                },
                "replacement": ""
            }]
        });
        apply_fix(&mut source, &fix).unwrap();
        assert_eq!(
            decode_hex(&source.sources[0].contents_hex).unwrap(),
            b"type T = {\n     value: Int\n}\n"
        );
    }
}
