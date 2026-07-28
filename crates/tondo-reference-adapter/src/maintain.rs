use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tondo_conformance::document::{DocumentFence, extract_fences};
use tondo_conformance::manifest::{
    CaseAction, CaseGroup, ConformanceCase, DocumentAction, Expectation, MemoryScenario,
    NormativeRegistry, PinnedFile, SemanticAction, SemanticQuery, SourceAction, SourceFile,
    SourceForm, SourceOperation, SuiteManifest, TargetDeclaration,
};
use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, AdapterResult, CompilationState, DocCategory, Observation,
    TargetSelection, WireDocumentFenceAction, WireOperation, WireSemanticAction, WireSource,
    WireSourceAction, WireSourceForm,
};
use tondo_reference_adapter::ReferenceAdapter;

const ROOT: &str = "conformance/0.1";
const SPECIFICATION: &str = "TONDO_LANGUAGE_SPEC.md";
const FIXTURE_MANIFEST: &str = "conformance/0.1/fixtures/tondo-fixture-manifest.txt";
const MANIFEST: &str = "conformance/0.1/manifest.json";

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments != ["bless"] {
        eprintln!("usage: tondo-conformance-maintain bless");
        return ExitCode::from(2);
    }
    match bless() {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("tondo-conformance-maintain: {message}");
            ExitCode::from(1)
        }
    }
}

fn bless() -> Result<String, String> {
    let root = workspace_root();
    let registry = extract_registry(&fs::read(root.join(SPECIFICATION)).map_err(io_error)?)?;
    let target = TargetSelection {
        name: "tondo-vm-hosted".into(),
        profile: "hosted".into(),
        capabilities: vec!["console".into(), "process".into()],
    };
    let mut adapter = ReferenceAdapter;
    let mut sequence = 1;
    let mut cases = Vec::new();
    for (directory, group) in source_groups() {
        let path = root.join(ROOT).join("cases").join(directory);
        if !path.exists() {
            continue;
        }
        let mut sources = Vec::new();
        collect_extension(&path, "to", &mut sources)?;
        sources.sort();
        for source in sources {
            let case =
                bless_source_case(&root, group, &source, &target, &mut adapter, &mut sequence)?;
            cases.push(case);
        }
    }
    bless_memory_cases(&root, &target, &mut adapter, &mut sequence, &mut cases)?;
    bless_document_case(
        &root,
        &registry,
        &target,
        &mut adapter,
        &mut sequence,
        &mut cases,
    )?;
    cases.sort_by(|left, right| left.id.cmp(&right.id));

    let specification = pinned(&root, SPECIFICATION)?;
    let fixture_manifest = pinned(&root, FIXTURE_MANIFEST)?;
    if fixture_manifest.sha256 != "1b6ab9f853b7ef4b94b4b9aaff6297e20556f81e8d99c322bed03854453d76c2"
    {
        return Err("appendix C fixture manifest does not have its normative hash".into());
    }
    let manifest = SuiteManifest {
        format: tondo_conformance::SUITE_FORMAT.into(),
        suite: tondo_conformance::SUITE_NAME.into(),
        version: "0.1.0".into(),
        edition: "0.1".into(),
        adapter_protocol: tondo_conformance::ADAPTER_PROTOCOL.into(),
        specification,
        fixture_manifest,
        registry,
        targets: vec![TargetDeclaration {
            name: target.name,
            profile: target.profile,
            capabilities: target.capabilities,
        }],
        cases,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    write_generated(&root.join(MANIFEST), &bytes)?;
    Ok(format!(
        "wrote {} cases to {} ({})",
        manifest.cases.len(),
        MANIFEST,
        tondo_conformance::sha256(&bytes)
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SourceMeta {
    operation: Option<SourceOperation>,
    form: Option<SourceForm>,
    capabilities: Option<Vec<String>>,
    repeat: Option<u32>,
    positive_for: Vec<String>,
    requirements: Vec<String>,
    arguments: Vec<String>,
    gc_threshold: Option<u32>,
    warning_profiles: Vec<String>,
    contents_hex: Option<String>,
    additional_sources: Vec<AdditionalSource>,
    queries: Vec<SemanticQuery>,
    exact_diagnostics: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditionalSource {
    path: String,
    module: String,
    logical_path: String,
}

#[derive(Debug, Serialize)]
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

fn bless_source_case(
    root: &Path,
    group: CaseGroup,
    source: &Path,
    target: &TargetSelection,
    adapter: &mut ReferenceAdapter,
    sequence: &mut u64,
) -> Result<ConformanceCase, String> {
    let group_directory = source
        .ancestors()
        .find(|ancestor| {
            ancestor
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "cases")
        })
        .ok_or_else(|| format!("cannot derive group for `{}`", source.display()))?;
    let group_name = group_directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "case group path is not UTF-8".to_owned())?;
    let relative = source
        .strip_prefix(group_directory)
        .map_err(|error| error.to_string())?;
    let mut case_suffix = relative
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    if case_suffix.is_empty() {
        return Err(format!("invalid source case `{}`", source.display()));
    }
    case_suffix = case_suffix
        .split('/')
        .map(canonical_case_component)
        .collect::<Vec<_>>()
        .join("/");
    let id = format!("{group_name}/{case_suffix}");
    let metadata_path = source.with_extension("meta.json");
    let metadata = if metadata_path.exists() {
        serde_json::from_slice::<SourceMeta>(&fs::read(&metadata_path).map_err(io_error)?)
            .map_err(|error| format!("{}: {error}", metadata_path.display()))?
    } else {
        SourceMeta::default()
    };
    let operation = metadata.operation.unwrap_or(match group {
        CaseGroup::LexParseFormat => SourceOperation::Format,
        CaseGroup::Runtime | CaseGroup::Concurrency | CaseGroup::Hosted => SourceOperation::Run,
        _ => SourceOperation::Check,
    });
    let form = metadata.form.unwrap_or(match group {
        CaseGroup::Runtime | CaseGroup::Concurrency | CaseGroup::Hosted => SourceForm::Script,
        _ => SourceForm::Module,
    });
    let capabilities = metadata
        .capabilities
        .unwrap_or_else(|| target.capabilities.clone());
    require_sorted_unique(&format!("{id} capabilities"), &capabilities)?;
    require_sorted_unique(
        &format!("{id} warning profiles"),
        &metadata.warning_profiles,
    )?;
    let (source_bytes, source_path) = if let Some(encoded) = &metadata.contents_hex {
        let bytes = tondo_conformance::decode_hex(encoded)?;
        let generated = source.with_extension("input");
        write_generated(&generated, &bytes)?;
        (bytes, logical_path(root, &generated)?)
    } else {
        (
            fs::read(source).map_err(io_error)?,
            logical_path(root, source)?,
        )
    };
    let source_id = format!("suite:{id}");
    let mut wire_sources = vec![WireSource {
        source_id: source_id.clone(),
        module: "main".into(),
        logical_path: "case.to".into(),
        contents_hex: tondo_conformance::encode_hex(&source_bytes),
    }];
    let mut manifest_sources = vec![SourceFile {
        source_id: source_id.clone(),
        module: "main".into(),
        logical_path: "case.to".into(),
        contents: PinnedFile {
            path: source_path,
            sha256: tondo_conformance::sha256(&source_bytes),
        },
    }];
    for additional in &metadata.additional_sources {
        validate_case_input_path(&additional.path)?;
        let physical = source
            .parent()
            .expect("source cases have a parent directory")
            .join(&additional.path);
        let bytes = fs::read(&physical).map_err(io_error)?;
        wire_sources.push(WireSource {
            source_id: source_id.clone(),
            module: additional.module.clone(),
            logical_path: additional.logical_path.clone(),
            contents_hex: tondo_conformance::encode_hex(&bytes),
        });
        manifest_sources.push(SourceFile {
            source_id: source_id.clone(),
            module: additional.module.clone(),
            logical_path: additional.logical_path.clone(),
            contents: PinnedFile {
                path: logical_path(root, &physical)?,
                sha256: tondo_conformance::sha256(&bytes),
            },
        });
    }
    wire_sources.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    manifest_sources.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    let wire = WireSourceAction {
        operation: wire_operation(operation),
        form: wire_form(form),
        root: "case.to".into(),
        sources: wire_sources,
        warning_profiles: metadata.warning_profiles.clone(),
        arguments: metadata.arguments.clone(),
        gc_threshold: metadata.gc_threshold,
    };
    let action = if metadata.queries.is_empty() {
        AdapterAction::Source(wire.clone())
    } else {
        AdapterAction::Semantic(WireSemanticAction {
            source: wire.clone(),
            queries: metadata.queries.clone(),
        })
    };
    let request = AdapterRequest::new(*sequence, id.clone(), target.clone(), action);
    *sequence = sequence.saturating_add(1);
    let observation = exchange(adapter, &request)?;

    let expected_codes = read_codes(&source.with_extension("codes"))?;
    let actual_codes = observation
        .diagnostic_codes()?
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if actual_codes != expected_codes {
        return Err(format!(
            "{id} produced {actual_codes:?}, expected {expected_codes:?}"
        ));
    }
    let expected_exit = expected_exit(source, group)?;
    if observation.exit_code != expected_exit {
        return Err(format!(
            "{id} exited {}, expected {expected_exit}",
            observation.exit_code
        ));
    }
    compare_bytes(
        &id,
        "stdout",
        &source.with_extension("stdout"),
        &observation.stdout_hex,
    )?;
    compare_bytes(
        &id,
        "stderr",
        &source.with_extension("runtime-stderr"),
        &observation.stderr_hex,
    )?;
    if operation == SourceOperation::Format {
        compare_required_bytes(
            &id,
            "formatted output",
            &source.with_extension("formatted"),
            observation.formatted_hex.as_deref(),
        )?;
    } else if observation.formatted_hex.is_some() {
        return Err(format!("{id} unexpectedly produced formatter bytes"));
    }
    let expectation = ObservationExpectation {
        compilation: observation.compilation,
        exit_code: observation.exit_code,
        diagnostic_codes: expected_codes.clone(),
        exact_diagnostics: metadata
            .exact_diagnostics
            .then(|| observation.diagnostics.clone()),
        stdout_hex: observation.stdout_hex.clone(),
        stderr_hex: observation.stderr_hex.clone(),
        formatted_hex: observation.formatted_hex.clone(),
        data: observation.data.clone(),
    };
    let expectation_path = source.with_extension("expect.json");
    write_generated(
        &expectation_path,
        &serde_json::to_vec(&expectation).map_err(|error| error.to_string())?,
    )?;

    let mut covers = expected_codes
        .iter()
        .filter(|code| matches!(code.as_bytes().first(), Some(b'E' | b'W' | b'P')))
        .cloned()
        .collect::<Vec<_>>();
    covers.sort();
    covers.dedup();
    let mut positive_for = metadata.positive_for;
    positive_for.sort();
    positive_for.dedup();
    let mut requirements = metadata.requirements;
    requirements.sort();
    requirements.dedup();
    let manifest_action = SourceAction {
        operation,
        form,
        root: "case.to".into(),
        sources: manifest_sources,
        warning_profiles: metadata.warning_profiles,
        arguments: metadata.arguments,
        gc_threshold: metadata.gc_threshold,
    };
    Ok(ConformanceCase {
        id,
        group,
        target: target.name.clone(),
        profile: target.profile.clone(),
        capabilities,
        repeat: metadata.repeat.unwrap_or(1),
        covers,
        positive_for,
        requirements,
        action: if metadata.queries.is_empty() {
            CaseAction::Source(manifest_action)
        } else {
            CaseAction::Semantic(SemanticAction {
                source: manifest_action,
                queries: metadata.queries,
            })
        },
        expectation: Expectation::Exact {
            observation: pinned(root, &logical_path(root, &expectation_path)?)?,
        },
    })
}

fn bless_memory_cases(
    root: &Path,
    target: &TargetSelection,
    adapter: &mut ReferenceAdapter,
    sequence: &mut u64,
    cases: &mut Vec<ConformanceCase>,
) -> Result<(), String> {
    for (name, scenario, requirement) in [
        (
            "reachable-roots",
            MemoryScenario::ReachableRoots,
            "MEM-CONF-001",
        ),
        (
            "unreachable-cycles",
            MemoryScenario::UnreachableCycles,
            "MEM-CONF-001",
        ),
        (
            "sustained-pressure",
            MemoryScenario::SustainedPressure,
            "CONF-010",
        ),
        (
            "retry-before-oom",
            MemoryScenario::RetryBeforeOom,
            "CONF-010",
        ),
    ] {
        let id = format!("memory/{name}");
        let request = AdapterRequest::new(
            *sequence,
            id.clone(),
            target.clone(),
            AdapterAction::Memory { scenario },
        );
        *sequence = sequence.saturating_add(1);
        let observation = exchange(adapter, &request)?;
        let expectation = pattern(&observation, false)?;
        let expectation_path = root
            .join(ROOT)
            .join("cases/memory")
            .join(format!("{name}.expect.json"));
        write_generated(
            &expectation_path,
            &serde_json::to_vec(&expectation).map_err(|error| error.to_string())?,
        )?;
        cases.push(ConformanceCase {
            id,
            group: CaseGroup::Memory,
            target: target.name.clone(),
            profile: target.profile.clone(),
            capabilities: target.capabilities.clone(),
            repeat: 1,
            covers: Vec::new(),
            positive_for: Vec::new(),
            requirements: vec![requirement.into()],
            action: CaseAction::Memory { scenario },
            expectation: Expectation::Exact {
                observation: pinned(root, &logical_path(root, &expectation_path)?)?,
            },
        });
    }
    Ok(())
}

fn bless_document_case(
    root: &Path,
    registry: &NormativeRegistry,
    target: &TargetSelection,
    adapter: &mut ReferenceAdapter,
    sequence: &mut u64,
    cases: &mut Vec<ConformanceCase>,
) -> Result<(), String> {
    let id = "documentation/language-spec".to_owned();
    let specification = fs::read(root.join(SPECIFICATION)).map_err(io_error)?;
    let fixture_manifest = fs::read(root.join(FIXTURE_MANIFEST)).map_err(io_error)?;
    let errors = registry.errors.iter().cloned().collect::<BTreeSet<_>>();
    let fences = extract_fences(&specification, &errors).map_err(|error| error.to_string())?;
    let mut records = Vec::with_capacity(fences.len());
    for fence in &fences {
        if fence.category == DocCategory::Pseudocode {
            records.push(pseudocode_record(fence));
            continue;
        }
        let request = AdapterRequest::new(
            *sequence,
            format!("{id}@{}", fence.fence_byte),
            target.clone(),
            AdapterAction::DocumentFence(WireDocumentFenceAction {
                file: SPECIFICATION.into(),
                fence_byte: fence.fence_byte,
                category: fence.category,
                fixture: fence.fixture.clone(),
                fixture_manifest_hex: tondo_conformance::encode_hex(&fixture_manifest),
                fixture_manifest_sha256: tondo_conformance::sha256(&fixture_manifest),
                expected_codes: fence.expected_codes.clone(),
                source_hex: tondo_conformance::encode_hex(&fence.source),
            }),
        );
        *sequence = sequence.saturating_add(1);
        let observation = exchange(adapter, &request)?;
        if observation.compilation == CompilationState::Rejected {
            return Err(format!(
                "{id} fence at byte {} did not satisfy its category:\nrecord={}\ndiagnostics={}",
                fence.fence_byte,
                serde_json::to_string_pretty(&observation.data)
                    .map_err(|error| error.to_string())?,
                serde_json::to_string_pretty(&observation.diagnostics)
                    .map_err(|error| error.to_string())?
            ));
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
    let expectation = pattern(&observation, false)?;
    let expectation_path = root
        .join(ROOT)
        .join("cases/documentation/language-spec.expect.json");
    write_generated(
        &expectation_path,
        &serde_json::to_vec(&expectation).map_err(|error| error.to_string())?,
    )?;
    cases.push(ConformanceCase {
        id,
        group: CaseGroup::Documentation,
        target: target.name.clone(),
        profile: target.profile.clone(),
        capabilities: target.capabilities.clone(),
        repeat: 1,
        covers: Vec::new(),
        positive_for: Vec::new(),
        requirements: vec!["CONF-002".into(), "CONF-003".into()],
        action: CaseAction::Document(DocumentAction {
            markdown: pinned(root, SPECIFICATION)?,
        }),
        expectation: Expectation::Exact {
            observation: pinned(root, &logical_path(root, &expectation_path)?)?,
        },
    });
    Ok(())
}

fn exchange(
    adapter: &mut ReferenceAdapter,
    request: &AdapterRequest,
) -> Result<Observation, String> {
    let response =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adapter.handle(request)))
            .map_err(|payload| {
                let message = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic");
                format!("adapter panicked for case `{}`: {message}", request.case_id)
            })?;
    match response.result {
        AdapterResult::Ok { observation } => Ok(observation),
        AdapterResult::Unsupported { reason } => Err(format!(
            "case `{}` is unsupported: {reason}",
            request.case_id
        )),
        AdapterResult::Error { message } => {
            Err(format!("case `{}` failed: {message}", request.case_id))
        }
    }
}

fn pattern(
    observation: &Observation,
    exact_diagnostics: bool,
) -> Result<ObservationExpectation, String> {
    Ok(ObservationExpectation {
        compilation: observation.compilation,
        exit_code: observation.exit_code,
        diagnostic_codes: observation
            .diagnostic_codes()?
            .into_iter()
            .map(str::to_owned)
            .collect(),
        exact_diagnostics: exact_diagnostics.then(|| observation.diagnostics.clone()),
        stdout_hex: observation.stdout_hex.clone(),
        stderr_hex: observation.stderr_hex.clone(),
        formatted_hex: observation.formatted_hex.clone(),
        data: observation.data.clone(),
    })
}

fn pseudocode_record(fence: &DocumentFence) -> Value {
    json!({
        "file": SPECIFICATION,
        "fence_byte": fence.fence_byte,
        "category": "pseudocode",
        "edition": "0.1",
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

fn source_groups() -> [(&'static str, CaseGroup); 7] {
    [
        ("lex-parse-format", CaseGroup::LexParseFormat),
        ("compile-fail", CaseGroup::CompileFail),
        ("compile-pass", CaseGroup::CompilePass),
        ("semantic-queries", CaseGroup::SemanticQueries),
        ("runtime", CaseGroup::Runtime),
        ("concurrency", CaseGroup::Concurrency),
        ("hosted", CaseGroup::Hosted),
    ]
}

fn wire_operation(operation: SourceOperation) -> WireOperation {
    match operation {
        SourceOperation::Format => WireOperation::Format,
        SourceOperation::Check => WireOperation::Check,
        SourceOperation::Run => WireOperation::Run,
    }
}

fn wire_form(form: SourceForm) -> WireSourceForm {
    match form {
        SourceForm::Module => WireSourceForm::Module,
        SourceForm::Script => WireSourceForm::Script,
        SourceForm::Fragment => WireSourceForm::Fragment,
        SourceForm::Syntax => WireSourceForm::Syntax,
        SourceForm::StandaloneBlock => WireSourceForm::StandaloneBlock,
    }
}

fn extract_registry(specification: &[u8]) -> Result<NormativeRegistry, String> {
    let text = std::str::from_utf8(specification).map_err(|error| error.to_string())?;
    let section = text
        .split_once("### 22.2 Códigos estables")
        .and_then(|(_, tail)| tail.split_once("### 22.3 Salida estructurada"))
        .map(|(section, _)| section)
        .ok_or_else(|| "cannot locate the normative diagnostic registry".to_owned())?;
    let mut errors = BTreeSet::new();
    let mut warnings = BTreeSet::new();
    let mut panics = BTreeSet::new();
    for line in section.lines() {
        let Some(rest) = line.strip_prefix("| `") else {
            continue;
        };
        let Some(code) = rest.get(..5) else {
            continue;
        };
        if rest.as_bytes().get(5..8) != Some(b"` |") {
            continue;
        }
        let destination = match code.as_bytes()[0] {
            b'E' => &mut errors,
            b'W' => &mut warnings,
            b'P' => &mut panics,
            _ => continue,
        };
        if code.as_bytes()[1..].iter().all(u8::is_ascii_digit) {
            destination.insert(code.to_owned());
        }
    }
    if (errors.len(), warnings.len(), panics.len()) != (78, 11, 11) {
        return Err(format!(
            "unexpected registry sizes E={} W={} P={}",
            errors.len(),
            warnings.len(),
            panics.len()
        ));
    }
    Ok(NormativeRegistry {
        errors: errors.into_iter().collect(),
        warnings: warnings.into_iter().collect(),
        panics: panics.into_iter().collect(),
    })
}

fn read_codes(path: &Path) -> Result<Vec<String>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(io_error)?;
    Ok(contents
        .lines()
        .filter(|line| matches!(line.as_bytes().first(), Some(b'E' | b'W' | b'P')))
        .map(str::to_owned)
        .collect())
}

fn expected_exit(source: &Path, group: CaseGroup) -> Result<i32, String> {
    let path = source.with_extension("exit");
    if path.exists() {
        return fs::read_to_string(&path)
            .map_err(io_error)?
            .trim()
            .parse::<i32>()
            .map_err(|error| format!("{}: {error}", path.display()));
    }
    Ok(if group == CaseGroup::CompileFail {
        1
    } else {
        0
    })
}

fn compare_bytes(id: &str, name: &str, path: &Path, actual_hex: &str) -> Result<(), String> {
    let expected = if path.exists() {
        fs::read(path).map_err(io_error)?
    } else {
        Vec::new()
    };
    let actual = tondo_conformance::decode_hex(actual_hex)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{id} {name} differs; declare expected bytes in {}",
            path.display()
        ))
    }
}

fn compare_required_bytes(
    id: &str,
    name: &str,
    path: &Path,
    actual_hex: Option<&str>,
) -> Result<(), String> {
    let expected = fs::read(path)
        .map_err(|error| format!("{id} {name} requires {}: {error}", path.display()))?;
    let actual = actual_hex
        .ok_or_else(|| format!("{id} produced no {name}"))
        .and_then(tondo_conformance::decode_hex)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{id} {name} differs from {}", path.display()))
    }
}

fn pinned(root: &Path, logical: &str) -> Result<PinnedFile, String> {
    let path = root.join(logical);
    let bytes = fs::read(&path).map_err(io_error)?;
    Ok(PinnedFile {
        path: logical.to_owned(),
        sha256: tondo_conformance::sha256(&bytes),
    })
}

fn logical_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|error| error.to_string())?
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| "suite path is not UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn validate_case_input_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("additional source paths must be relative normal paths".into());
    }
    Ok(())
}

fn collect_extension(
    directory: &Path,
    extension: &str,
    output: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path.is_dir() {
            collect_extension(&path, extension, output)?;
        } else if path.extension().is_some_and(|actual| actual == extension) {
            output.push(path);
        }
    }
    Ok(())
}

fn canonical_case_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn require_sorted_unique(name: &str, values: &[String]) -> Result<(), String> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(format!("{name} must be sorted and unique"))
    } else {
        Ok(())
    }
}

fn write_generated(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::write(path, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> String {
    error.to_string()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
