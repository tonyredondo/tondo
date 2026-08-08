use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, WarningProfile, discover_tests, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::project::ProjectPlan;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
};
use tondo_compiler::test_control::{EnvelopeLimits, EnvelopeReport, SnapshotOutcome, Terminal};
use tondo_compiler::test_glob::GlobPattern;
use tondo_compiler::test_plan::{
    CodeownersMode, TestOrder as ProjectTestOrder, TestProjectPlan,
    TestSelector as ProjectTestSelector,
};
use tondo_compiler::test_repeat::{RepeatCampaign, RepeatContext, RepeatPolicy};
use tondo_compiler::test_report::{
    OrderMode as ReportOrderMode, ReportMetadata, ReportOrder, ReportSelection, ReportShard,
    SelectionKind, SnapshotMode, SnapshotStoreIdentity, TestList, TestReport,
};
use tondo_compiler::test_result::{
    AggregateStatus, ArtifactRecord, AttemptPhase, AttemptStatus, BlockedBy, FailureRecord,
    ResultNodeKind, RetryUnit, RetryUnitKind, SkipRecord, SnapshotRecord, SnapshotStatus,
    TestAttempt, TestNode, VirtualTimeRecord,
};
use tondo_compiler::test_retry::{RetryCampaign, RetryContext, RetryPolicy};
use tondo_compiler::test_runtime::{
    LeafProgram, RunError, RuntimeConfig, RuntimeRunner, RuntimeStatus,
};
use tondo_compiler::test_schedule::{OrderMode, ScheduleNode, SchedulePlan, Seed};
use tondo_compiler::test_shard::ShardSpec;
use tondo_compiler::test_snapshots::{SnapshotPolicy, SnapshotStore, SnapshotUpdateStage};

mod project_discovery;
mod test_cli;

const EXIT_DIAGNOSTIC: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_INTERNAL: u8 = 3;

type PreparedCompilation = (CompilationRequest, Option<Arc<[u8]>>);

const USAGE: &str = "\
Tondo bootstrap toolchain

Usage:
  tondo <command> [--diagnostic-format <human|json>] [--warnings core] <source.to>
  tondo <check|run> [--diagnostic-format <human|json>] [--warnings core] [--project <dir>]
  tondo run [--diagnostic-format <human|json>] [--warnings core] <source.to> -- [argument ...]
  tondo test [--project <dir>] [--test-plan <tondo.test.toml>] [options]

Commands:
  fmt      Format one Tondo source file
  check    Analyze one Tondo source file
  run      Compile and run one Tondo script
  test     Discover, compile and run project tests

Options:
  --diagnostic-format <human|json>  Select diagnostic output
  --warnings <core>                 Enable a closed warning profile
  --check                           Verify formatting without writing output (fmt only)
  --project <dir>                   Project directory (default: current directory)
  --test-plan <path>                Optional advanced TOML test-plan sidecar
  --emit-interface <path>           Write the canonical compiled interface on success
  --emit-artifact <path>            Write canonical build metadata on success
  -- [argument ...]                 Pass UTF-8 arguments to a run script
  -h, --help                        Show this help
  -V, --version                     Show version information";

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("tondo: {error}");
            ExitCode::from(EXIT_INTERNAL)
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    if arguments.first().and_then(|argument| argument.to_str()) == Some("__test-worker") {
        return run_test_worker_on_explicit_stack(arguments[1..].to_vec());
    }
    match arguments.as_slice() {
        [argument] if argument == "-h" || argument == "--help" => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        [argument] if argument == "-V" || argument == "--version" => {
            println!(
                "tondo {} (language {}, backend {})",
                env!("CARGO_PKG_VERSION"),
                tondo_compiler::LANGUAGE_EDITION,
                tondo_vm::BACKEND_NAME,
            );
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    if arguments.first().and_then(|argument| argument.to_str()) == Some("test") {
        return run_test_command(&arguments);
    }

    let invocation = match parse_invocation(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let (request, original_source) = match compilation_request(&invocation) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("tondo: {message}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let request = request
        .with_warning_profiles(invocation.warning_profiles.iter().copied())
        .with_program_arguments(invocation.program_arguments.clone());
    let output = execute(request).map_err(|error| error.to_string())?;

    let format_check_failed = invocation.format_check
        && output.status() == CompilationStatus::Success
        && original_source
            .as_deref()
            .is_some_and(|bytes| output.stdout() != bytes);
    if !invocation.format_check {
        io::stdout()
            .write_all(output.stdout())
            .map_err(|error| format!("cannot write command output: {error}"))?;
    }
    emit_products(&invocation, &output)?;

    let rendered = match invocation.diagnostic_format {
        DiagnosticFormat::Human => output.diagnostics().human(),
        DiagnosticFormat::Json => output
            .diagnostics()
            .json_lines()
            .map_err(|error| error.to_string())?,
    };
    eprint!("{rendered}");

    Ok(if format_check_failed {
        ExitCode::from(EXIT_DIAGNOSTIC)
    } else {
        ExitCode::from(output.exit_code())
    })
}

fn run_test_command(arguments: &[OsString]) -> Result<ExitCode, String> {
    let plan = match test_cli::parse(arguments) {
        Ok(plan) => plan,
        Err(message) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let location =
        ProjectLocation::Directory(plan.project.clone().unwrap_or_else(|| PathBuf::from(".")));
    match execute_test_plan_at(&plan, location) {
        Ok(code) => Ok(ExitCode::from(code)),
        Err(TestCommandError::Usage(message)) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            Ok(ExitCode::from(EXIT_USAGE))
        }
        Err(TestCommandError::Internal(message)) => {
            eprintln!("tondo: {message}");
            Ok(ExitCode::from(EXIT_INTERNAL))
        }
        Err(TestCommandError::Diagnostic(message)) => {
            eprintln!("{message}");
            Ok(ExitCode::from(EXIT_DIAGNOSTIC))
        }
    }
}

#[derive(Debug, Clone)]
enum ProjectLocation {
    Directory(PathBuf),
}

#[derive(Debug)]
struct LoadedProject {
    location: ProjectLocation,
    base: PathBuf,
    project: ProjectPlan,
}

impl ProjectLocation {
    fn load(&self) -> Result<LoadedProject, TestCommandError> {
        match self {
            Self::Directory(path) => {
                let root = path.canonicalize().map_err(|error| {
                    TestCommandError::Usage(format!(
                        "cannot resolve project directory `{}`: {error}",
                        path.display()
                    ))
                })?;
                let discovered = project_discovery::discover_for_tests(&root)
                    .map_err(TestCommandError::Usage)?;
                let project =
                    ProjectPlan::parse(&discovered.manifest_bytes, &discovered.lockfile_bytes)
                        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
                Ok(LoadedProject {
                    location: self.clone(),
                    base: discovered.root.clone(),
                    project,
                })
            }
        }
    }
}

#[derive(Debug)]
enum TestCommandError {
    Usage(String),
    Internal(String),
    Diagnostic(String),
}

impl From<tondo_compiler::driver::DriverError> for TestCommandError {
    fn from(error: tondo_compiler::driver::DriverError) -> Self {
        Self::Internal(error.to_string())
    }
}

fn resolve_test_plan_path(
    plan: &test_cli::TestCliPlan,
    base: &Path,
) -> Result<Option<PathBuf>, TestCommandError> {
    if let Some(path) = &plan.test_plan {
        return Ok(Some(path.clone()));
    }
    let adjacent = base.join("tondo.test.toml");
    match fs::symlink_metadata(&adjacent) {
        Ok(_) => Ok(Some(adjacent)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TestCommandError::Usage(format!(
            "cannot inspect test plan `{}`: {error}",
            adjacent.display()
        ))),
    }
}

#[cfg(test)]
fn execute_test_plan(
    plan: &test_cli::TestCliPlan,
    project_path: &Path,
) -> Result<u8, TestCommandError> {
    execute_test_plan_at(plan, ProjectLocation::Directory(project_path.to_owned()))
}

fn load_test_project_plan(
    project: &ProjectPlan,
    path: Option<&Path>,
) -> Result<TestProjectPlan, TestCommandError> {
    let Some(path) = path else {
        return Ok(TestProjectPlan::defaults(project, 1));
    };
    if path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err(TestCommandError::Usage(
            "test plans use TOML; JSON plans are unsupported".into(),
        ));
    }
    let source_bytes = read_input(path, "test plan").map_err(TestCommandError::Usage)?;
    let text = String::from_utf8(source_bytes)
        .map_err(|error| TestCommandError::Usage(format!("invalid test plan UTF-8: {error}")))?;
    let value = toml::from_str::<toml::Value>(&text).map_err(|error| {
        TestCommandError::Usage(format!("invalid test plan `{}`: {error}", path.display()))
    })?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let plan = project
        .parse_test_plan(&bytes)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    Ok(plan)
}

fn overlay_test_project_plan(
    execution: &mut test_cli::TestCliPlan,
    project_plan: &TestProjectPlan,
) -> Result<(), TestCommandError> {
    if !execution.selector_explicit {
        execution.selector = match project_plan.selector() {
            ProjectTestSelector::None => test_cli::TestSelector::All,
            ProjectTestSelector::Filter(value) => test_cli::TestSelector::Filter(value.clone()),
            ProjectTestSelector::Glob(value) => test_cli::TestSelector::Glob(value.clone()),
            ProjectTestSelector::Exact(value) => test_cli::TestSelector::Exact(value.clone()),
        };
    }
    if !execution.codeowners_explicit {
        execution.codeowners = match project_plan.codeowners() {
            CodeownersMode::Auto => test_cli::CodeownersSelection::Auto,
            CodeownersMode::None => test_cli::CodeownersSelection::None,
            CodeownersMode::Path(path) => {
                test_cli::CodeownersSelection::Explicit(PathBuf::from(path))
            }
        };
    }
    if !execution.shard_explicit {
        execution.shard = project_plan.shard().map(|shard| test_cli::TestShard {
            index: shard.index(),
            count: shard.count(),
        });
    }
    if !execution.order_explicit {
        execution.order = match project_plan.order() {
            ProjectTestOrder::Canonical => test_cli::TestOrder::Canonical,
            ProjectTestOrder::Random { seed } => test_cli::TestOrder::Random {
                seed: seed
                    .as_deref()
                    .map(|value| u64::from_str_radix(value, 16))
                    .transpose()
                    .map_err(|_| {
                        TestCommandError::Internal(
                            "canonical test-plan order seed is not a valid u64".into(),
                        )
                    })?,
            },
        };
    }
    if !execution.jobs_explicit {
        execution.jobs = project_plan.policy().jobs();
    }
    if !execution.retry_explicit {
        execution.retry = project_plan.policy().retry();
    }
    if !execution.repeat_explicit {
        execution.repeat = project_plan.policy().repeat();
    }
    if !execution.allow_empty {
        execution.allow_empty = project_plan.policy().allow_empty();
    }
    if !execution.timeout_explicit && execution.timeout_ms.is_none() {
        execution.timeout_ms = Some(project_plan.limits().timeout_ms());
    } else if execution.timeout_explicit && execution.timeout_ms.is_none() {
        return Err(TestCommandError::Usage(
            "`--timeout none` cannot disable the closed test-plan wall-clock limit".into(),
        ));
    } else if execution.timeout_ms > Some(project_plan.limits().timeout_ms()) {
        return Err(TestCommandError::Usage(format!(
            "`--timeout` cannot exceed the closed test-plan limit of {}ms",
            project_plan.limits().timeout_ms()
        )));
    }
    test_cli::validate_combinations(execution)
        .map(|_| ())
        .map_err(TestCommandError::Usage)
}

#[derive(Debug)]
struct OwnershipInfo {
    mode: tondo_compiler::test_report::OwnershipMode,
    source: Option<String>,
    sha256: Option<String>,
    resolution: tondo_compiler::test_owners::OwnershipResolution,
}

#[derive(Debug, Clone)]
struct LoadedSnapshotStore {
    name: String,
    relative: PathBuf,
    max_bytes: u64,
    store: SnapshotStore,
}

#[derive(Debug, Clone)]
struct SnapshotInputs {
    stores: Vec<LoadedSnapshotStore>,
    before_sha256: String,
    update: bool,
}

impl SnapshotInputs {
    fn expected_for(&self, node_id: &str) -> Result<BTreeMap<String, String>, TestCommandError> {
        let mut expected = BTreeMap::new();
        for loaded in &self.stores {
            for entry in loaded.store.entries() {
                if entry.node_id == node_id
                    && expected
                        .insert(entry.name.clone(), entry.value.clone())
                        .is_some()
                {
                    return Err(TestCommandError::Usage(format!(
                        "snapshot name `{}` is duplicated across stores",
                        entry.name
                    )));
                }
            }
        }
        Ok(expected)
    }

    fn stage_and_publish(
        &self,
        base: &Path,
        plan: &test_cli::TestCliPlan,
        attempts: &[CliAttempt],
    ) -> Result<SnapshotMutation, TestCommandError> {
        if !self.update || self.stores.is_empty() {
            return Ok(SnapshotMutation {
                after_sha256: self.before_sha256.clone(),
                published: false,
            });
        }
        let policy = SnapshotPolicy::new(
            plan.jobs,
            matches!(plan.order, test_cli::TestOrder::Canonical),
            plan.shard.is_some(),
            plan.retry > 0,
            plan.repeat > 1,
            plan.allow_flaky,
        );
        let mut stages = self
            .stores
            .iter()
            .map(|loaded| {
                SnapshotUpdateStage::new(loaded.store.clone(), policy)
                    .map_err(|error| TestCommandError::Usage(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for attempt in attempts {
            for (name, value) in &attempt.snapshot_updates {
                let matches = self
                    .stores
                    .iter()
                    .enumerate()
                    .filter(|(_, loaded)| {
                        loaded
                            .store
                            .entries()
                            .iter()
                            .any(|entry| entry.node_id == attempt.id && entry.name == *name)
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let index = match matches.as_slice() {
                    [index] => *index,
                    [] if self.stores.len() == 1 => 0,
                    [] => {
                        return Err(TestCommandError::Usage(format!(
                            "new snapshot `{name}` is ambiguous across stores"
                        )));
                    }
                    _ => {
                        return Err(TestCommandError::Usage(format!(
                            "snapshot `{name}` is duplicated across stores"
                        )));
                    }
                };
                stages[index]
                    .stage(&attempt.id, name, value)
                    .map_err(|error| TestCommandError::Usage(error.to_string()))?;
            }
        }
        if attempts
            .iter()
            .all(|attempt| attempt.status == RuntimeStatus::Passed)
        {
            for stage in &mut stages {
                stage.mark_success();
            }
            let mut staged_stores = Vec::with_capacity(stages.len());
            for (stage, loaded) in stages.iter().zip(&self.stores) {
                let staged = stage
                    .staged_store()
                    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
                let staged_size = staged
                    .canonical_bytes()
                    .map_err(|error| TestCommandError::Internal(error.to_string()))?
                    .len() as u64;
                if staged_size > loaded.max_bytes {
                    return Err(TestCommandError::Usage(format!(
                        "snapshot store `{}` exceeds its closed {} byte limit",
                        loaded.relative.display(),
                        loaded.max_bytes
                    )));
                }
                staged_stores.push(staged);
            }
            let mut published_hashes = Vec::with_capacity(stages.len());
            for ((stage, _staged), loaded) in stages.iter_mut().zip(staged_stores).zip(&self.stores)
            {
                let published = stage
                    .publish(base, &loaded.relative)
                    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
                published_hashes.push((
                    loaded.name.as_str(),
                    published
                        .content_hash()
                        .map_err(|error| TestCommandError::Internal(error.to_string()))?,
                ));
            }
            Ok(SnapshotMutation {
                after_sha256: combined_store_hash(&published_hashes),
                published: true,
            })
        } else {
            Ok(SnapshotMutation {
                after_sha256: self.before_sha256.clone(),
                published: false,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotMutation {
    after_sha256: String,
    published: bool,
}

fn combined_store_hash(stores: &[(&str, String)]) -> String {
    if let [(_, hash)] = stores {
        return hash.strip_prefix("sha256:").unwrap_or(hash).to_owned();
    }
    let mut bytes = Vec::new();
    for (name, hash) in stores {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(hash.as_bytes());
        bytes.push(0);
    }
    tondo_compiler::artifact::sha256(&bytes)
        .strip_prefix("sha256:")
        .unwrap_or_default()
        .to_owned()
}

fn load_snapshot_inputs(
    base: &Path,
    request: &CompilationRequest,
    test_plan: &TestProjectPlan,
    update: bool,
) -> Result<SnapshotInputs, TestCommandError> {
    let package = request.packages().root().as_str();
    let mut stores = Vec::new();
    for descriptor in test_plan.snapshot_stores() {
        let relative = PathBuf::from(descriptor.path());
        let path = base.join(&relative);
        let store = if !path.exists() && (update || test_plan.snapshot_stores_implicit()) {
            SnapshotStore::empty(package)
                .map_err(|error| TestCommandError::Usage(error.to_string()))?
        } else {
            SnapshotStore::load(base, &relative).map_err(|error| {
                TestCommandError::Usage(format!(
                    "cannot load snapshot store `{}`: {error}",
                    descriptor.path()
                ))
            })?
        };
        if store.package != package {
            return Err(TestCommandError::Usage(format!(
                "snapshot store `{}` belongs to package `{}` instead of `{package}`",
                descriptor.path(),
                store.package
            )));
        }
        let canonical_size = store
            .canonical_bytes()
            .map_err(|error| TestCommandError::Usage(error.to_string()))?
            .len() as u64;
        if canonical_size > descriptor.max_bytes() {
            return Err(TestCommandError::Usage(format!(
                "snapshot store `{}` exceeds its closed {} byte limit",
                descriptor.path(),
                descriptor.max_bytes()
            )));
        }
        stores.push(LoadedSnapshotStore {
            name: descriptor.name().to_owned(),
            relative,
            max_bytes: descriptor.max_bytes(),
            store,
        });
    }
    let mut hashes = Vec::with_capacity(stores.len());
    for store in &stores {
        hashes.push((
            store.name.as_str(),
            store
                .store
                .content_hash()
                .map_err(|error| TestCommandError::Usage(error.to_string()))?,
        ));
    }
    let before_sha256 = if hashes.is_empty() {
        SnapshotStore::empty(package)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
            .content_hash()
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
            .strip_prefix("sha256:")
            .unwrap_or_default()
            .to_owned()
    } else {
        combined_store_hash(&hashes)
    };
    Ok(SnapshotInputs {
        stores,
        before_sha256,
        update,
    })
}

fn resolve_ownership(
    plan: &test_cli::TestCliPlan,
    base: &Path,
) -> Result<OwnershipInfo, TestCommandError> {
    let mode = match &plan.codeowners {
        test_cli::CodeownersSelection::Auto => tondo_compiler::test_plan::CodeownersMode::Auto,
        test_cli::CodeownersSelection::None => tondo_compiler::test_plan::CodeownersMode::None,
        test_cli::CodeownersSelection::Explicit(path) => {
            tondo_compiler::test_plan::CodeownersMode::Path(path.to_string_lossy().into_owned())
        }
    };
    let paths: Vec<&str> =
        match &plan.codeowners {
            test_cli::CodeownersSelection::Auto => {
                tondo_compiler::test_owners::AUTO_CODEOWNERS_PATHS.to_vec()
            }
            test_cli::CodeownersSelection::None => Vec::new(),
            test_cli::CodeownersSelection::Explicit(path) => {
                vec![path.to_str().ok_or_else(|| {
                    TestCommandError::Usage("CODEOWNERS path must be UTF-8".into())
                })?]
            }
        };
    let candidates = paths
        .into_iter()
        .map(|path| read_codeowners_candidate(base, path))
        .collect::<Result<Vec<_>, _>>()?;
    let resolution = tondo_compiler::test_owners::resolve(&mode, candidates)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    let ownership_mode = match resolution.mode() {
        "auto" if resolution.source().is_some() => tondo_compiler::test_report::OwnershipMode::Auto,
        "auto" => tondo_compiler::test_report::OwnershipMode::None,
        "explicit" => tondo_compiler::test_report::OwnershipMode::Explicit,
        "none" => tondo_compiler::test_report::OwnershipMode::None,
        other => {
            return Err(TestCommandError::Internal(format!(
                "unknown CODEOWNERS resolution mode `{other}`"
            )));
        }
    };
    Ok(OwnershipInfo {
        mode: ownership_mode,
        source: resolution.source().map(str::to_owned),
        sha256: resolution.sha256().map(str::to_owned),
        resolution,
    })
}

fn read_codeowners_candidate(
    base: &Path,
    relative: &str,
) -> Result<tondo_compiler::test_owners::CodeownersCandidate, TestCommandError> {
    let path = base.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(tondo_compiler::test_owners::CodeownersCandidate::absent(
                relative,
            ));
        }
        Err(error) => {
            return Err(TestCommandError::Usage(format!(
                "cannot inspect CODEOWNERS `{relative}`: {error}"
            )));
        }
    };
    let symlink = metadata.file_type().is_symlink();
    if symlink || !metadata.file_type().is_file() {
        return Ok(
            tondo_compiler::test_owners::CodeownersCandidate::present(relative, Vec::new())
                .with_file_state(false, true, symlink),
        );
    }
    match fs::read(&path) {
        Ok(bytes) => Ok(tondo_compiler::test_owners::CodeownersCandidate::present(
            relative, bytes,
        )),
        Err(_) => Ok(tondo_compiler::test_owners::CodeownersCandidate::present(
            relative,
            Vec::new(),
        )
        .with_file_state(true, false, false)),
    }
}

fn execute_test_plan_at(
    plan: &test_cli::TestCliPlan,
    location: ProjectLocation,
) -> Result<u8, TestCommandError> {
    let loaded = location.load()?;
    let base = loaded.base.as_path();
    let project = &loaded.project;
    let test_plan_path = resolve_test_plan_path(plan, base)?;
    let test_project_plan = load_test_project_plan(project, test_plan_path.as_deref())?;
    let mut execution_plan = plan.clone();
    overlay_test_project_plan(&mut execution_plan, &test_project_plan)?;
    let mut supplied = BTreeMap::new();
    for input in project.required_inputs() {
        let bytes = read_input(
            &base.join(input.path()),
            &format!("{} input `{}`", input.kind().as_str(), input.path()),
        )
        .map_err(TestCommandError::Usage)?;
        supplied.insert(input.path().to_owned(), Arc::<[u8]>::from(bytes));
    }
    let request = project
        .resolve(&supplied)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?
        .into_compilation_request(
            Operation::Check,
            execution_plan.diagnostic_format,
            ResourceLimits::default(),
        )
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    let request = Arc::new(request);
    let snapshot_inputs = load_snapshot_inputs(
        base,
        &request,
        &test_project_plan,
        execution_plan.update_snapshots,
    )?;
    let ownership = resolve_ownership(&execution_plan, base)?;
    let entries = discover_tests(&request)?;
    let selected = select_test_entries(entries, &execution_plan)?;
    if selected.is_empty() {
        if execution_plan.allow_empty {
            if execution_plan.list {
                return Ok(0);
            }
            eprintln!("tondo: no tests selected");
            return Ok(0);
        }
        return Err(TestCommandError::Diagnostic(
            "tondo: no tests matched the selection".into(),
        ));
    }
    if execution_plan.list {
        if execution_plan.test_format == test_cli::TestFormat::Json {
            let list = build_test_list(
                &request,
                &execution_plan,
                &selected,
                &ownership,
                &snapshot_inputs,
            )?;
            let bytes = list
                .canonical_bytes()
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            print!(
                "{}",
                String::from_utf8(bytes)
                    .map_err(|error| { TestCommandError::Internal(error.to_string()) })?
            );
        } else {
            for entry in &selected {
                println!("{}", entry.id());
            }
        }
        return Ok(0);
    }

    let ordered = order_test_entries(selected, &execution_plan)?;
    let worker_project = match &loaded.location {
        ProjectLocation::Directory(path) => path.clone(),
    };
    let worker_test_plan = test_plan_path.clone();
    let worker_timeout = execution_plan.timeout_ms;
    let worker_update_snapshots = execution_plan.update_snapshots;
    let mut grouped_entries = BTreeMap::<(u32, String), Vec<String>>::new();
    for entry in &ordered {
        let root = entry
            .suites()
            .first()
            .cloned()
            .unwrap_or_else(|| entry.id().to_owned());
        grouped_entries
            .entry((entry.file().index(), root))
            .or_default()
            .push(entry.id().to_owned());
    }
    let worker_groups = grouped_entries
        .into_iter()
        .map(|(key, entries)| {
            let has_suites = entries.iter().any(|id| id != &key.1);
            (
                key,
                Arc::new(SharedWorkerGroup {
                    project: worker_project.clone(),
                    entries,
                    timeout_ms: worker_timeout,
                    update_snapshots: worker_update_snapshots,
                    test_plan: worker_test_plan.clone(),
                    has_suites,
                    invocations: Mutex::new(BTreeMap::new()),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let programs = ordered
        .iter()
        .map(|entry| {
            let id = entry.id().to_owned();
            let root = entry
                .suites()
                .first()
                .cloned()
                .unwrap_or_else(|| entry.id().to_owned());
            let retry_group = format!("{}::{root}", entry.file().index());
            let group = worker_groups
                .get(&(entry.file().index(), root))
                .cloned()
                .ok_or_else(|| {
                    TestCommandError::Internal("scheduler lost a suite participation".into())
                })?;
            Ok(LeafProgram::new(id.clone(), move |context| {
                let response = group.response(context.worker().invocation_id(), &id)?;
                let report = EnvelopeReport::decode_process(&response.report)
                    .map_err(|message| RunError::Infrastructure { message })?;
                let updates = response
                    .updates
                    .iter()
                    .map(|update| (update.name.clone(), update.value.clone()))
                    .collect::<Vec<_>>();
                context.merge_worker_report(&report, &updates)?;
                response.error.map_or_else(
                    || {
                        if response.status == "passed" || report.terminal().is_some() {
                            Ok(())
                        } else {
                            Err(RunError::Infrastructure {
                                message: format!("worker returned status `{}`", response.status),
                            })
                        }
                    },
                    WorkerError::into_run_error,
                )
            })
            .with_retry_group(retry_group))
        })
        .collect::<Result<Vec<_>, TestCommandError>>()?;
    let runtime = RuntimeRunner::new(
        RuntimeConfig::new(
            execution_plan.jobs as usize,
            EnvelopeLimits::new(
                test_project_plan.limits().output_bytes(),
                test_project_plan.limits().artifact_bytes(),
                test_project_plan.limits().snapshot_bytes(),
            ),
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let attempts = execute_campaign(&request, &execution_plan, &ordered, programs, runtime)?;
    let suite_attempts = collect_suite_attempts(&worker_groups, &execution_plan)?;
    let mut node_attempts = attempts.clone();
    node_attempts.extend(suite_attempts.iter().map(|attempt| CliAttempt {
        id: attempt.id.clone(),
        iteration: attempt.iteration,
        round: attempt.round,
        unit: (attempt.round > 0).then_some(1),
        status: attempt.status,
        report: attempt.report.clone(),
        error: attempt.error.clone(),
        snapshot_updates: attempt.snapshot_updates.clone(),
    }));
    publish_attempt_artifacts(
        base,
        &execution_plan,
        Some(test_project_plan.artifact_store()),
        &node_attempts,
    )?;
    let snapshot_mutation =
        snapshot_inputs.stage_and_publish(base, &execution_plan, &node_attempts)?;
    let report = build_test_report(
        &request,
        &execution_plan,
        &ordered,
        &ownership,
        &attempts,
        &suite_attempts,
        &snapshot_inputs,
        &snapshot_mutation,
    )?;
    publish_test_outputs(&execution_plan, &report)?;
    if execution_plan.test_format == test_cli::TestFormat::Json {
        print!(
            "{}",
            String::from_utf8(
                report
                    .canonical_bytes()
                    .map_err(|error| { TestCommandError::Internal(error.to_string()) })?
            )
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
        );
    } else {
        for test in report.tests() {
            println!("{} {}", status_label(test.status), test.id);
            if execution_plan.show_output {
                for attempt in &test.attempts {
                    if !attempt.stdout.is_empty() {
                        print!("{}", attempt.stdout);
                    }
                    if !attempt.stderr.is_empty() {
                        eprint!("{}", attempt.stderr);
                    }
                }
            }
        }
    }
    let failed = report.summary().failed > 0
        || (plan.deny_skips && report.summary().skipped + report.summary().blocked_skip > 0);
    Ok(u8::from(failed))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerSnapshotUpdate {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerError {
    kind: String,
    code: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResponse {
    format: String,
    status: String,
    report: Vec<u8>,
    updates: Vec<WorkerSnapshotUpdate>,
    error: Option<WorkerError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBatchResponse {
    format: String,
    responses: Vec<(String, WorkerResponse)>,
    suites: Vec<WorkerSuiteResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerSuiteResponse {
    id: String,
    status: String,
    phase: Option<String>,
    report: Vec<u8>,
    updates: Vec<WorkerSnapshotUpdate>,
    error: Option<WorkerError>,
}

#[derive(Debug, Clone)]
struct WorkerGroupResult {
    leaves: BTreeMap<String, WorkerResponse>,
    suites: Vec<WorkerSuiteResponse>,
}

type WorkerInvocation = Arc<std::sync::OnceLock<Result<WorkerGroupResult, RunError>>>;

const WORKER_RESPONSE_FORMAT: &str = "tondo-test-worker-process/1";
const WORKER_BATCH_RESPONSE_FORMAT: &str = "tondo-test-worker-batch/1";

struct SharedWorkerGroup {
    project: PathBuf,
    entries: Vec<String>,
    timeout_ms: Option<u64>,
    update_snapshots: bool,
    test_plan: Option<PathBuf>,
    has_suites: bool,
    invocations: Mutex<BTreeMap<u64, WorkerInvocation>>,
}

impl SharedWorkerGroup {
    fn response(&self, invocation: u64, id: &str) -> Result<WorkerResponse, RunError> {
        let slot = self
            .invocations
            .lock()
            .map_err(|_| RunError::Infrastructure {
                message: "test participation cache is poisoned".into(),
            })?
            .entry(invocation)
            .or_default()
            .clone();
        let responses = slot.get_or_init(|| {
            spawn_test_worker(
                &self.project,
                &self.entries,
                self.timeout_ms,
                self.update_snapshots,
                self.test_plan.as_deref(),
            )
        });
        responses
            .as_ref()
            .map_err(Clone::clone)?
            .leaves
            .get(id)
            .cloned()
            .ok_or_else(|| RunError::Infrastructure {
                message: format!("test participation omitted leaf `{id}`"),
            })
    }

    fn suite_responses(&self) -> Result<Vec<(u64, WorkerSuiteResponse)>, RunError> {
        let invocations = self
            .invocations
            .lock()
            .map_err(|_| RunError::Infrastructure {
                message: "test participation cache is poisoned".into(),
            })?;
        let mut suites = Vec::new();
        for (invocation, slot) in invocations.iter() {
            let Some(result) = slot.get() else {
                continue;
            };
            let result = match result {
                Ok(result) => result,
                Err(_) if !self.has_suites => continue,
                Err(error) => return Err(error.clone()),
            };
            for suite in &result.suites {
                suites.push((*invocation, suite.clone()));
            }
        }
        Ok(suites)
    }
}

impl WorkerError {
    #[cfg(test)]
    fn from_run_error(error: &RunError) -> Self {
        let kind = match error {
            RunError::Error { .. } => "error",
            RunError::Panic { .. } | RunError::Control(_) => "panic",
            RunError::ResourceLimit { .. } => "resource-limit",
            RunError::Timeout | RunError::ForcedTermination { .. } => "timeout",
            RunError::Infrastructure { .. } => "infrastructure",
            RunError::BlockedSetup { .. } => "blocked-setup",
            RunError::BlockedSkip { .. } => "blocked-skip",
            RunError::Skip { .. } => "skip",
        };
        Self {
            kind: kind.into(),
            code: error.code().map(str::to_owned),
            message: error.to_string(),
        }
    }

    fn into_run_error(self) -> Result<(), RunError> {
        let code = self.code.unwrap_or_else(|| "T3001".into());
        let message = self.message;
        Err(match self.kind.as_str() {
            "error" => RunError::Error { code, message },
            "panic" => RunError::Panic { code, message },
            "resource-limit" => RunError::ResourceLimit { kind: code },
            "timeout" => RunError::Timeout,
            "skip" => RunError::Skip { reason: message },
            "infrastructure" => RunError::Infrastructure { message },
            "blocked-setup" => RunError::BlockedSetup { suite: message },
            "blocked-skip" => RunError::BlockedSkip { suite: message },
            other => RunError::Infrastructure {
                message: format!("unknown worker error kind `{other}`: {message}"),
            },
        })
    }
}

#[cfg(test)]
fn runtime_status_wire(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Passed => "passed",
        RuntimeStatus::Skipped => "skipped",
        RuntimeStatus::FailedError => "failed-error",
        RuntimeStatus::FailedPanic => "failed-panic",
        RuntimeStatus::ResourceLimit => "resource-limit",
        RuntimeStatus::Timeout => "timeout",
        RuntimeStatus::Infrastructure => "infrastructure",
        RuntimeStatus::BlockedSetup => "blocked-setup",
        RuntimeStatus::BlockedSkip => "blocked-skip",
    }
}

fn empty_worker_report() -> Vec<u8> {
    let envelope = tondo_compiler::test_control::EnvelopeHandle::new(
        "worker-empty",
        EnvelopeLimits::new(0, 0, 0),
    );
    envelope.close().expect("fresh worker envelope closes");
    envelope
        .report()
        .expect("closed worker envelope reports")
        .encode_process()
        .expect("worker empty report encodes")
}

fn infrastructure_worker_response(error: impl Into<String>) -> WorkerResponse {
    WorkerResponse {
        format: WORKER_RESPONSE_FORMAT.into(),
        status: "infrastructure".into(),
        report: empty_worker_report(),
        updates: Vec::new(),
        error: Some(WorkerError {
            kind: "infrastructure".into(),
            code: None,
            message: error.into(),
        }),
    }
}

fn spawn_test_worker(
    project: &Path,
    entries: &[String],
    timeout_ms: Option<u64>,
    update_snapshots: bool,
    test_plan: Option<&Path>,
) -> Result<WorkerGroupResult, RunError> {
    let mut command =
        Command::new(
            worker_executable().map_err(|error| RunError::Infrastructure {
                message: format!("cannot locate tondo worker executable: {error}"),
            })?,
        );
    command.arg("__test-worker").arg("--project").arg(project);
    for entry in entries {
        command.arg("--entry").arg(entry);
    }
    if let Some(test_plan) = test_plan {
        command.arg("--test-plan").arg(test_plan);
    }
    if update_snapshots {
        command.arg("--update-snapshots");
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| RunError::Infrastructure {
            message: format!("cannot spawn isolated test worker: {error}"),
        })?;
    let (status, stdout, stderr) = wait_worker(child, timeout_ms)?;
    if stdout.is_empty() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(RunError::Infrastructure {
            message: format!(
                "isolated test worker exited with {status} without a response: {}",
                detail.trim()
            ),
        });
    }
    let response: WorkerBatchResponse =
        serde_json::from_slice(&stdout).map_err(|error| RunError::Infrastructure {
            message: format!("invalid isolated test worker response: {error}"),
        })?;
    if response.format != WORKER_BATCH_RESPONSE_FORMAT {
        return Err(RunError::Infrastructure {
            message: format!(
                "unexpected isolated test worker format `{}`",
                response.format
            ),
        });
    }
    let responses = response.responses.into_iter().collect::<BTreeMap<_, _>>();
    if responses.len() != entries.len() {
        return Err(RunError::Infrastructure {
            message: "isolated test worker returned a duplicate or missing leaf response".into(),
        });
    }
    Ok(WorkerGroupResult {
        leaves: responses,
        suites: response.suites,
    })
}

fn worker_executable() -> Result<PathBuf, io::Error> {
    let current = env::current_exe()?;
    if cfg!(test)
        && let Some(deps) = current.parent()
        && deps.file_name() == Some(OsStr::new("deps"))
        && let Some(target) = deps.parent()
    {
        let binary = target.join(if cfg!(windows) { "tondo.exe" } else { "tondo" });
        if binary.is_file() {
            return Ok(binary);
        }

        // `cargo llvm-cov` puts the instrumented test harness under a
        // separate target directory but does not build a matching binary
        // target.  Reuse the normal Cargo binary when it is available so the
        // process-boundary worker keeps the same command-line contract under
        // coverage as it does for regular unit tests.
        if target.parent().and_then(Path::file_name) == Some(OsStr::new("llvm-cov-target"))
            && let Some(cargo_target) = target.parent().and_then(Path::parent)
        {
            let binary =
                cargo_target
                    .join("debug")
                    .join(if cfg!(windows) { "tondo.exe" } else { "tondo" });
            if binary.is_file() {
                return Ok(binary);
            }
        }
    }
    Ok(current)
}

fn wait_worker(
    mut child: Child,
    timeout_ms: Option<u64>,
) -> Result<(String, Vec<u8>, Vec<u8>), RunError> {
    // Drain both pipes while the worker is running. Waiting for process exit
    // before reading would deadlock a valid worker whose bounded report is
    // larger than the host pipe buffer.
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = join_worker_pipe(stdout_reader, "output")?;
                let stderr = join_worker_pipe(stderr_reader, "diagnostics")?;
                return Ok((status.to_string(), stdout, stderr));
            }
            Ok(None) => {
                if timeout_ms.is_some_and(|limit| started.elapsed() >= Duration::from_millis(limit))
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = join_worker_pipe(stdout_reader, "output");
                    let _ = join_worker_pipe(stderr_reader, "diagnostics");
                    return Err(RunError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_worker_pipe(stdout_reader, "output");
                let _ = join_worker_pipe(stderr_reader, "diagnostics");
                return Err(RunError::Infrastructure {
                    message: format!("cannot poll isolated test worker: {error}"),
                });
            }
        }
    }
}

fn join_worker_pipe(
    reader: Option<std::thread::JoinHandle<io::Result<Vec<u8>>>>,
    stream: &str,
) -> Result<Vec<u8>, RunError> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| RunError::Infrastructure {
            message: format!("isolated worker {stream} reader panicked"),
        })?
        .map_err(|error| RunError::Infrastructure {
            message: format!("cannot read isolated worker {stream}: {error}"),
        })
}

// The isolated worker is a process boundary, but its entry point still runs
// on the platform's main thread. Windows reserves a substantially smaller
// default stack than the Unix runners; compile/test code that is safe under
// the VM's logical stack budget can therefore exhaust the native stack before
// the VM can report its own resource limit. Keep the process boundary and run
// the worker body on a portable, bounded stack instead.
const TEST_WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

fn run_test_worker_on_explicit_stack(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    let worker = std::thread::Builder::new()
        .name("tondo-test-worker".into())
        .stack_size(TEST_WORKER_STACK_SIZE)
        .spawn(move || run_test_worker(&arguments))
        .map_err(|error| format!("cannot create isolated worker stack: {error}"))?;
    worker
        .join()
        .map_err(|_| "isolated test worker stack panicked".to_owned())?
}

fn run_test_worker(arguments: &[OsString]) -> Result<ExitCode, String> {
    let mut project = None;
    let mut test_plan = None;
    let mut entries = Vec::new();
    let mut update_snapshots = false;
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments[index]
            .to_str()
            .ok_or_else(|| "hidden test-worker arguments must be UTF-8".to_owned())?;
        match value {
            "--project" => {
                index += 1;
                project = Some(PathBuf::from(
                    arguments
                        .get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "worker `--project` requires a directory".to_owned())?,
                ));
            }
            "--entry" => {
                index += 1;
                entries.push(
                    arguments
                        .get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "worker `--entry` requires an id".to_owned())?
                        .to_owned(),
                );
            }
            "--test-plan" => {
                index += 1;
                test_plan = Some(PathBuf::from(
                    arguments
                        .get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "worker `--test-plan` requires a path".to_owned())?,
                ));
            }
            "--update-snapshots" => update_snapshots = true,
            other => return Err(format!("unknown hidden worker option `{other}`")),
        }
        index += 1;
    }
    let project = project.ok_or_else(|| "worker project is required".to_owned())?;
    if entries.is_empty() {
        return Err("at least one worker entry is required".to_owned());
    }
    let result =
        match execute_test_worker(&project, test_plan.as_deref(), &entries, update_snapshots) {
            Ok(responses) => responses,
            Err(error) => WorkerGroupResult {
                leaves: entries
                    .iter()
                    .map(|entry| (entry.clone(), infrastructure_worker_response(error.clone())))
                    .collect(),
                suites: Vec::new(),
            },
        };
    let response = WorkerBatchResponse {
        format: WORKER_BATCH_RESPONSE_FORMAT.into(),
        responses: entries
            .into_iter()
            .map(|entry| {
                let response = result
                    .leaves
                    .get(&entry)
                    .cloned()
                    .unwrap_or_else(|| infrastructure_worker_response("worker omitted leaf"));
                (entry, response)
            })
            .collect(),
        suites: result.suites,
    };
    let bytes = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
    io::stdout()
        .write_all(&bytes)
        .map_err(|error| format!("cannot write worker response: {error}"))?;
    io::stdout()
        .write_all(b"\n")
        .map_err(|error| format!("cannot finish worker response: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

fn execute_test_worker(
    project_path: &Path,
    test_plan_path: Option<&Path>,
    entry_ids: &[String],
    update_snapshots: bool,
) -> Result<WorkerGroupResult, String> {
    let location = ProjectLocation::Directory(project_path.to_owned());
    let loaded = location.load().map_err(format_test_command_error)?;
    let base = loaded.base.as_path();
    let project = &loaded.project;
    let test_plan =
        load_test_project_plan(project, test_plan_path).map_err(format_test_command_error)?;
    let mut supplied = BTreeMap::new();
    for input in project.required_inputs() {
        let bytes = read_input(
            &base.join(input.path()),
            &format!("{} input `{}`", input.kind().as_str(), input.path()),
        )?;
        supplied.insert(input.path().to_owned(), Arc::<[u8]>::from(bytes));
    }
    let request = project
        .resolve(&supplied)
        .map_err(|error| error.to_string())?
        .into_compilation_request(
            Operation::Check,
            DiagnosticFormat::Human,
            ResourceLimits::default(),
        )
        .map_err(|error| error.to_string())?;
    let request = Arc::new(request);
    let snapshot_inputs = load_snapshot_inputs(base, &request, &test_plan, update_snapshots)
        .map_err(format_test_command_error)?;
    let entries = discover_tests(&request).map_err(|error| error.to_string())?;
    let by_id = entries
        .into_iter()
        .map(|entry| (entry.id().to_owned(), entry))
        .collect::<BTreeMap<_, _>>();
    let selected = entry_ids
        .iter()
        .map(|id| {
            by_id
                .get(id)
                .cloned()
                .ok_or_else(|| format!("test entry `{id}` was not found"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let limits = test_plan.limits();
    let envelope_limits = EnvelopeLimits::new(
        limits.output_bytes(),
        limits.artifact_bytes(),
        limits.snapshot_bytes(),
    );
    let mut node_ids = selected
        .iter()
        .map(|entry| entry.id().to_owned())
        .collect::<BTreeSet<_>>();
    node_ids.extend(selected.iter().flat_map(|entry| {
        (1..=entry.suites().len()).map(move |depth| {
            let mut parts = entry.id().split("::").collect::<Vec<_>>();
            parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
            parts.join("::")
        })
    }));
    let expected = node_ids
        .into_iter()
        .map(|id| {
            snapshot_inputs
                .expected_for(&id)
                .map(|snapshots| (id, snapshots))
                .map_err(format_test_command_error)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let participation = tondo_compiler::test_backend::TestParticipation::new(
        envelope_limits,
        expected,
        update_snapshots,
    );
    let test_request = request
        .for_test_participation(&selected, participation.clone())
        .map_err(|error| error.to_string())?;
    let output = execute(test_request).map_err(|error| error.to_string())?;
    if output.status() != CompilationStatus::Success {
        let diagnostics = output.diagnostics().human();
        return Err(if diagnostics.is_empty() {
            "test participation failed without a diagnostic".into()
        } else {
            diagnostics
        });
    }

    let executions = participation.executions()?;
    let mut responses = BTreeMap::new();
    for entry in &selected {
        let response = if let Some(execution) = executions.iter().find(|execution| {
            execution.kind == tondo_compiler::test_backend::TestExecutionKind::Leaf
                && execution.id == entry.id()
        }) {
            let error = execution
                .report
                .terminal()
                .is_none()
                .then_some(execution.panic.as_ref())
                .flatten()
                .map(|panic| WorkerError {
                    kind: "panic".into(),
                    code: Some(panic.code.code().into()),
                    message: panic.message.clone(),
                });
            let status = if error.is_some() {
                "failed-panic"
            } else if matches!(execution.report.terminal(), Some(Terminal::Skipped { .. })) {
                "skipped"
            } else if execution.report.terminal().is_some() {
                "failed-panic"
            } else {
                "passed"
            };
            WorkerResponse {
                format: WORKER_RESPONSE_FORMAT.into(),
                status: status.into(),
                report: execution
                    .report
                    .encode_process()
                    .map_err(|error| format!("cannot encode worker report: {error}"))?,
                updates: execution
                    .snapshot_updates
                    .iter()
                    .map(|(name, value)| WorkerSnapshotUpdate {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                error,
            }
        } else if let Some(suite) = executions
            .iter()
            .filter(|execution| {
                execution.kind == tondo_compiler::test_backend::TestExecutionKind::Suite
                    && (execution.panic.is_some() || execution.report.terminal().is_some())
                    && entry
                        .id()
                        .strip_prefix(&execution.id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
            })
            .max_by_key(|execution| execution.id.len())
        {
            let skipped = matches!(suite.report.terminal(), Some(Terminal::Skipped { .. }));
            WorkerResponse {
                format: WORKER_RESPONSE_FORMAT.into(),
                status: if skipped {
                    "blocked-skip".into()
                } else {
                    "blocked-setup".into()
                },
                report: empty_worker_report(),
                updates: Vec::new(),
                error: Some(WorkerError {
                    kind: if skipped {
                        "blocked-skip".into()
                    } else {
                        "blocked-setup".into()
                    },
                    code: None,
                    message: suite.id.clone(),
                }),
            }
        } else {
            infrastructure_worker_response(format!(
                "test participation omitted leaf `{}`",
                entry.id()
            ))
        };
        responses.insert(entry.id().to_owned(), response);
    }
    let mut suites = executions
        .iter()
        .filter(|execution| {
            execution.kind == tondo_compiler::test_backend::TestExecutionKind::Suite
        })
        .map(|execution| {
            let error = execution
                .report
                .terminal()
                .is_none()
                .then_some(execution.panic.as_ref())
                .flatten()
                .map(|panic| WorkerError {
                    kind: "panic".into(),
                    code: Some(panic.code.code().into()),
                    message: panic.message.clone(),
                });
            let status = match execution.report.terminal() {
                Some(Terminal::Skipped { .. }) => "skipped",
                Some(Terminal::ResourceLimit { .. }) => "resource-limit",
                Some(Terminal::FailNow { .. }) | Some(Terminal::CleanupFailure { .. }) => {
                    "failed-panic"
                }
                None if error.is_some() => "failed-panic",
                None => "passed",
            };
            let phase = (status != "passed").then(|| match execution.phase {
                tondo_compiler::test_control::ExecutionPhase::Setup => "setup".to_owned(),
                tondo_compiler::test_control::ExecutionPhase::Body => "body".to_owned(),
                tondo_compiler::test_control::ExecutionPhase::Cleanup
                | tondo_compiler::test_control::ExecutionPhase::Closed => "teardown".to_owned(),
            });
            Ok(WorkerSuiteResponse {
                id: execution.id.clone(),
                status: status.into(),
                phase,
                report: execution
                    .report
                    .encode_process()
                    .map_err(|error| format!("cannot encode suite report: {error}"))?,
                updates: execution
                    .snapshot_updates
                    .iter()
                    .map(|(name, value)| WorkerSnapshotUpdate {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                error,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let selected_suite_ids = selected
        .iter()
        .flat_map(|entry| {
            (1..=entry.suites().len()).map(move |depth| {
                let mut parts = entry.id().split("::").collect::<Vec<_>>();
                parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
                parts.join("::")
            })
        })
        .collect::<BTreeSet<_>>();
    for id in selected_suite_ids {
        if suites.iter().any(|suite| suite.id == id) {
            continue;
        }
        let Some(blocker) = executions
            .iter()
            .filter(|execution| {
                execution.kind == tondo_compiler::test_backend::TestExecutionKind::Suite
                    && (execution.panic.is_some() || execution.report.terminal().is_some())
                    && id
                        .strip_prefix(&execution.id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
            })
            .max_by_key(|execution| execution.id.len())
        else {
            return Err(format!("test participation omitted suite `{id}`"));
        };
        let skipped = matches!(blocker.report.terminal(), Some(Terminal::Skipped { .. }));
        suites.push(WorkerSuiteResponse {
            id,
            status: if skipped {
                "blocked-skip".into()
            } else {
                "blocked-setup".into()
            },
            phase: None,
            report: empty_worker_report(),
            updates: Vec::new(),
            error: Some(WorkerError {
                kind: if skipped {
                    "blocked-skip".into()
                } else {
                    "blocked-setup".into()
                },
                code: None,
                message: blocker.id.clone(),
            }),
        });
    }
    suites.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    Ok(WorkerGroupResult {
        leaves: responses,
        suites,
    })
}

fn format_test_command_error(error: TestCommandError) -> String {
    match error {
        TestCommandError::Usage(message)
        | TestCommandError::Internal(message)
        | TestCommandError::Diagnostic(message) => message,
    }
}

fn select_test_entries(
    entries: Vec<tondo_compiler::test_backend::TestEntry>,
    plan: &test_cli::TestCliPlan,
) -> Result<Vec<tondo_compiler::test_backend::TestEntry>, TestCommandError> {
    let mut selected = entries
        .into_iter()
        .filter(|entry| match &plan.selector {
            test_cli::TestSelector::All => true,
            test_cli::TestSelector::Filter(value) => {
                entry.id().contains(value) || entry.name().contains(value)
            }
            test_cli::TestSelector::Glob(value) => GlobPattern::parse(value)
                .map(|pattern| pattern.matches(entry.id()) || pattern.matches(entry.name()))
                .unwrap_or(false),
            test_cli::TestSelector::Exact(value) => {
                entry.id() == value
                    || entry.name() == value
                    || entry.id().starts_with(&format!("{value}::"))
            }
        })
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| left.id().as_bytes().cmp(right.id().as_bytes()));
    if let Some(shard) = plan.shard {
        let spec = ShardSpec::new(shard.index, shard.count)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        let ids = selected.iter().map(|entry| entry.id()).collect::<Vec<_>>();
        let partition = tondo_compiler::test_shard::ShardResult::partition(ids, spec)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        selected.retain(|entry| partition.ids().any(|id| id == entry.id()));
    }
    Ok(selected)
}

fn order_test_entries(
    entries: Vec<tondo_compiler::test_backend::TestEntry>,
    plan: &test_cli::TestCliPlan,
) -> Result<Vec<tondo_compiler::test_backend::TestEntry>, TestCommandError> {
    let mode = match plan.order {
        test_cli::TestOrder::Canonical => OrderMode::Canonical,
        test_cli::TestOrder::Random { seed } => OrderMode::Random {
            seed: Seed::from_u64(seed.unwrap_or(0)),
        },
    };
    let mut nodes = BTreeMap::<String, ScheduleNode>::new();
    for entry in &entries {
        let suites = suite_ids(entry);
        for (index, id) in suites.iter().enumerate() {
            nodes.entry(id.clone()).or_insert_with(|| {
                ScheduleNode::suite(
                    id.clone(),
                    index.checked_sub(1).map(|parent| suites[parent].clone()),
                )
            });
        }
        nodes.insert(
            entry.id().to_owned(),
            ScheduleNode::test(entry.id().to_owned(), suites.last().cloned()),
        );
    }
    let schedule = SchedulePlan::new(nodes.into_values(), mode, plan.jobs)
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let by_id = entries
        .into_iter()
        .map(|entry| (entry.id().to_owned(), entry))
        .collect::<BTreeMap<_, _>>();
    schedule
        .execution_plan()
        .into_iter()
        .map(|id| {
            by_id
                .get(&id)
                .cloned()
                .ok_or_else(|| TestCommandError::Internal("scheduler lost a test entry".into()))
        })
        .collect()
}

fn suite_ids(entry: &tondo_compiler::test_backend::TestEntry) -> Vec<String> {
    (1..=entry.suites().len())
        .map(|depth| {
            let mut parts = entry.id().split("::").collect::<Vec<_>>();
            parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
            parts.join("::")
        })
        .collect()
}

#[derive(Debug, Clone)]
struct CliAttempt {
    id: String,
    iteration: u32,
    round: u32,
    unit: Option<u32>,
    status: RuntimeStatus,
    report: EnvelopeReport,
    error: Option<RunError>,
    snapshot_updates: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct CliSuiteAttempt {
    id: String,
    iteration: u32,
    round: u32,
    status: RuntimeStatus,
    phase: Option<AttemptPhase>,
    report: EnvelopeReport,
    error: Option<RunError>,
    snapshot_updates: Vec<(String, String)>,
}

fn collect_suite_attempts(
    groups: &BTreeMap<(u32, String), Arc<SharedWorkerGroup>>,
    plan: &test_cli::TestCliPlan,
) -> Result<Vec<CliSuiteAttempt>, TestCommandError> {
    let mut attempts = Vec::new();
    for group in groups.values() {
        for (invocation, suite) in group
            .suite_responses()
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
        {
            let report = EnvelopeReport::decode_process(&suite.report)
                .map_err(TestCommandError::Internal)?;
            let status = match suite.status.as_str() {
                "passed" => RuntimeStatus::Passed,
                "skipped" => RuntimeStatus::Skipped,
                "failed-panic" => RuntimeStatus::FailedPanic,
                "failed-error" => RuntimeStatus::FailedError,
                "resource-limit" => RuntimeStatus::ResourceLimit,
                "timeout" => RuntimeStatus::Timeout,
                "infrastructure" => RuntimeStatus::Infrastructure,
                "blocked-setup" => RuntimeStatus::BlockedSetup,
                "blocked-skip" => RuntimeStatus::BlockedSkip,
                other => {
                    return Err(TestCommandError::Internal(format!(
                        "unknown suite worker status `{other}`"
                    )));
                }
            };
            let phase = match suite.phase.as_deref() {
                None => None,
                Some("setup") => Some(AttemptPhase::Setup),
                Some("teardown") => Some(AttemptPhase::Teardown),
                Some(other) => {
                    return Err(TestCommandError::Internal(format!(
                        "unknown suite worker phase `{other}`"
                    )));
                }
            };
            let error = suite.error.map(|error| error.into_run_error().unwrap_err());
            attempts.push(CliSuiteAttempt {
                id: suite.id,
                iteration: if plan.repeat > 1 {
                    u32::try_from(invocation).map_err(|_| {
                        TestCommandError::Internal("suite iteration overflows u32".into())
                    })?
                } else {
                    1
                },
                round: if plan.retry > 0 {
                    u32::try_from(invocation.saturating_sub(1)).map_err(|_| {
                        TestCommandError::Internal("suite retry round overflows u32".into())
                    })?
                } else {
                    0
                },
                status,
                phase,
                report,
                error,
                snapshot_updates: suite
                    .updates
                    .into_iter()
                    .map(|update| (update.name, update.value))
                    .collect(),
            });
        }
    }
    Ok(attempts)
}

fn execute_campaign(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    programs: Vec<LeafProgram>,
    runtime: RuntimeRunner,
) -> Result<Vec<CliAttempt>, TestCommandError> {
    if plan.repeat > 1 {
        let policy = RepeatPolicy::new(plan.repeat)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        let context = RepeatContext::new(
            entries.iter().map(|entry| entry.id().to_owned()),
            entries.iter().map(|entry| entry.id().to_owned()),
            shard_identity(plan),
            request.target().name(),
            "closed-inputs",
            order_seed(plan),
            tondo_compiler::test_report::CANONICAL_ORDER_ALGORITHM,
            request
                .capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned()),
            campaign_limits(plan),
            "artifact-store",
            "snapshot-store",
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        let report = RepeatCampaign::new(runtime, policy, context)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
            .run(programs)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        return Ok(report
            .attempts()
            .iter()
            .map(|attempt| CliAttempt {
                id: attempt.id().to_owned(),
                iteration: attempt.iteration(),
                round: attempt.round(),
                unit: attempt.unit(),
                status: attempt.status(),
                report: attempt.report().clone(),
                error: None,
                snapshot_updates: Vec::new(),
            })
            .collect());
    }
    if plan.retry > 0 {
        let policy = RetryPolicy::new(plan.retry)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?
            .with_allow_flaky(plan.allow_flaky);
        let context = RetryContext::new(
            shard_identity(plan),
            request.target().name(),
            "closed-inputs",
            order_seed(plan),
            tondo_compiler::test_report::CANONICAL_ORDER_ALGORITHM,
            request
                .capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned()),
            campaign_limits(plan),
            "artifact-store",
            "snapshot-store",
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        let report = RetryCampaign::new(runtime, policy, context)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?
            .run(programs)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        return Ok(report
            .attempts()
            .iter()
            .map(|attempt| CliAttempt {
                id: attempt.id().to_owned(),
                iteration: 1,
                round: attempt.round(),
                unit: attempt.unit(),
                status: attempt.status(),
                report: attempt.report().clone(),
                error: None,
                snapshot_updates: Vec::new(),
            })
            .collect());
    }
    let report = runtime
        .run(programs)
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    Ok(report
        .leaves()
        .iter()
        .map(|leaf| CliAttempt {
            id: leaf.id().to_owned(),
            iteration: 1,
            round: 0,
            unit: None,
            status: leaf.status(),
            report: leaf.report().clone(),
            error: leaf.error().cloned(),
            snapshot_updates: leaf.snapshot_updates().to_vec(),
        })
        .collect())
}

fn campaign_limits(plan: &test_cli::TestCliPlan) -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("timeout_ms".to_owned(), plan.timeout_ms.unwrap_or_default()),
        ("jobs".to_owned(), u64::from(plan.jobs)),
    ])
}

fn shard_identity(plan: &test_cli::TestCliPlan) -> String {
    plan.shard.map_or_else(
        || "all".to_owned(),
        |shard| format!("{}/{}", shard.index, shard.count),
    )
}

fn order_seed(plan: &test_cli::TestCliPlan) -> u64 {
    match plan.order {
        test_cli::TestOrder::Canonical => 0,
        test_cli::TestOrder::Random { seed } => seed.unwrap_or(0),
    }
}

fn build_test_list(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    ownership: &OwnershipInfo,
    snapshots: &SnapshotInputs,
) -> Result<TestList, TestCommandError> {
    let metadata = report_metadata(
        request,
        plan,
        ownership,
        snapshots,
        &SnapshotMutation {
            after_sha256: snapshots.before_sha256.clone(),
            published: false,
        },
    )?;
    let tests = entries
        .iter()
        .map(|entry| {
            let (package, module, path) = identity_parts(entry.id());
            let owners = ownership
                .resolution
                .owners_for(Some(entry.logical_path()))
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            let mut node = TestNode::new(
                entry.id().to_owned(),
                None,
                package,
                ResultNodeKind::Test,
                module,
                entry.name().to_owned(),
                Vec::new(),
            );
            node.path = path;
            node.owners = owners;
            Ok(node)
        })
        .collect::<Result<Vec<_>, TestCommandError>>()?;
    TestList::new(
        metadata,
        SnapshotStoreIdentity {
            format: tondo_compiler::test_report::TEST_SNAPSHOT_FORMAT.into(),
            sha256: snapshots.before_sha256.clone(),
        },
        entries.iter().map(|entry| entry.id().to_owned()).collect(),
        Vec::new(),
        tests,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))
}

fn report_metadata(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    ownership: &OwnershipInfo,
    snapshots: &SnapshotInputs,
    mutation: &SnapshotMutation,
) -> Result<ReportMetadata, TestCommandError> {
    let mut metadata = ReportMetadata::default();
    metadata.target.name = request.target().name().to_owned();
    metadata.target.profile = request.profile().as_str().to_owned();
    metadata.target.capabilities = request
        .capabilities()
        .iter()
        .map(|capability| capability.as_str().to_owned())
        .collect();
    metadata.limits.jobs = plan.jobs;
    metadata.limits.timeout_ms = plan.timeout_ms;
    metadata.policy.deny_skips = plan.deny_skips;
    metadata.policy.allow_flaky = plan.allow_flaky;
    metadata.repeat.count = plan.repeat;
    metadata.retry.max_additional_rounds = plan.retry;
    metadata.ownership = tondo_compiler::test_report::ReportOwnership {
        mode: ownership.mode,
        source: ownership.source.clone(),
        sha256: ownership.sha256.clone(),
    };
    metadata.snapshot_policy.mode = if plan.update_snapshots {
        SnapshotMode::Update
    } else {
        SnapshotMode::Check
    };
    metadata.snapshot_policy.before_sha256 = snapshots.before_sha256.clone();
    metadata.snapshot_policy.after_sha256 = mutation.after_sha256.clone();
    metadata.snapshot_policy.published = plan.update_snapshots.then_some(mutation.published);
    metadata.selection = match &plan.selector {
        test_cli::TestSelector::All => ReportSelection {
            kind: SelectionKind::All,
            value: None,
        },
        test_cli::TestSelector::Filter(value) => ReportSelection {
            kind: SelectionKind::Filter,
            value: Some(value.clone()),
        },
        test_cli::TestSelector::Glob(value) => ReportSelection {
            kind: SelectionKind::Glob,
            value: Some(value.clone()),
        },
        test_cli::TestSelector::Exact(value) => ReportSelection {
            kind: SelectionKind::Exact,
            value: Some(value.clone()),
        },
    };
    metadata.order = match plan.order {
        test_cli::TestOrder::Canonical => ReportOrder {
            mode: ReportOrderMode::Canonical,
            seed: None,
            algorithm: tondo_compiler::test_report::CANONICAL_ORDER_ALGORITHM.into(),
        },
        test_cli::TestOrder::Random { seed } => ReportOrder {
            mode: ReportOrderMode::Random,
            seed: Some(Seed::from_u64(seed.unwrap_or(0)).as_hex()),
            algorithm: tondo_compiler::test_report::RANDOM_ORDER_ALGORITHM.into(),
        },
    };
    metadata.shard = plan.shard.map(|shard| ReportShard {
        index: shard.index,
        count: shard.count,
        algorithm: tondo_compiler::test_report::SHARD_ALGORITHM.into(),
    });
    Ok(metadata)
}

#[allow(clippy::too_many_arguments)]
fn build_test_report(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    ownership: &OwnershipInfo,
    attempts: &[CliAttempt],
    suite_attempts: &[CliSuiteAttempt],
    snapshots: &SnapshotInputs,
    mutation: &SnapshotMutation,
) -> Result<TestReport, TestCommandError> {
    let mut metadata = report_metadata(request, plan, ownership, snapshots, mutation)?;
    metadata.retry.rounds = retry_rounds(plan, entries, attempts);
    let tests = entries
        .iter()
        .map(|entry| {
            let mut selected = attempts
                .iter()
                .filter(|attempt| attempt.id == entry.id())
                .collect::<Vec<_>>();
            selected.sort_by_key(|attempt| {
                (attempt.iteration, attempt.round, attempt.unit.unwrap_or(0))
            });
            if selected.is_empty() {
                return Err(TestCommandError::Internal(format!(
                    "runtime did not return test `{}`",
                    entry.id()
                )));
            }
            let test_attempts = selected
                .iter()
                .enumerate()
                .map(|(index, attempt)| make_test_attempt(index as u32 + 1, attempt))
                .collect::<Result<Vec<_>, _>>()?;
            let (package, module, path) = identity_parts(entry.id());
            let owners = ownership
                .resolution
                .owners_for(Some(entry.logical_path()))
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            let mut node = TestNode::new(
                entry.id().to_owned(),
                None,
                package,
                ResultNodeKind::Test,
                module,
                entry.name().to_owned(),
                test_attempts,
            );
            node.path = path;
            node.owners = owners;
            Ok(node)
        })
        .collect::<Result<Vec<_>, TestCommandError>>()?;
    let mut suite_ids = entries
        .iter()
        .flat_map(|entry| {
            (1..=entry.suites().len()).map(move |depth| {
                let mut parts = entry.id().split("::").collect::<Vec<_>>();
                parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
                parts.join("::")
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    suite_ids.sort_by(|left, right| {
        (left.matches("::").count(), left.as_bytes())
            .cmp(&(right.matches("::").count(), right.as_bytes()))
    });
    let suites = suite_ids
        .iter()
        .map(|id| {
            let mut selected = suite_attempts
                .iter()
                .filter(|attempt| attempt.id == *id)
                .collect::<Vec<_>>();
            selected.sort_by_key(|attempt| (attempt.iteration, attempt.round));
            if selected.is_empty() {
                return Err(TestCommandError::Internal(format!(
                    "runtime did not return suite `{id}`"
                )));
            }
            let attempts = selected
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    let source_attempt = CliAttempt {
                        id: source.id.clone(),
                        iteration: source.iteration,
                        round: source.round,
                        unit: (source.round > 0).then_some(1),
                        status: source.status,
                        report: source.report.clone(),
                        error: source.error.clone(),
                        snapshot_updates: source.snapshot_updates.clone(),
                    };
                    let mut attempt = make_test_attempt(index as u32 + 1, &source_attempt)?;
                    attempt.phase = source.phase;
                    Ok(attempt)
                })
                .collect::<Result<Vec<_>, TestCommandError>>()?;
            let (package, module, path) = identity_parts(id);
            let parent = id.rsplit_once("::").and_then(|(candidate, _)| {
                suite_ids
                    .iter()
                    .any(|suite| suite == candidate)
                    .then(|| candidate.to_owned())
            });
            let name = id.rsplit("::").next().unwrap_or(id).to_owned();
            let mut node = TestNode::new(
                id.clone(),
                parent,
                package,
                ResultNodeKind::Suite,
                module,
                name,
                attempts,
            );
            node.path = path;
            let logical_path = entries
                .iter()
                .find(|entry| {
                    entry
                        .id()
                        .strip_prefix(id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
                })
                .ok_or_else(|| {
                    TestCommandError::Internal(format!("suite `{id}` has no selected descendant"))
                })?
                .logical_path();
            node.owners = ownership
                .resolution
                .owners_for(Some(logical_path))
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            Ok(node)
        })
        .collect::<Result<Vec<_>, TestCommandError>>()?;
    TestReport::assemble(
        metadata,
        entries.iter().map(|entry| entry.id().to_owned()).collect(),
        suites,
        tests,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))
}

fn retry_rounds(
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    attempts: &[CliAttempt],
) -> Vec<tondo_compiler::test_report::ReportRetryRound> {
    if plan.retry == 0 {
        return Vec::new();
    }
    let mut rounds = Vec::new();
    for round in 1..=plan.retry {
        let mut grouped = BTreeMap::<String, (usize, RetryUnitKind, Vec<String>)>::new();
        for (index, entry) in entries.iter().enumerate() {
            if !attempts
                .iter()
                .any(|attempt| attempt.id == entry.id() && attempt.round == round)
            {
                continue;
            }
            let (id, kind) = if entry.suites().is_empty() {
                (entry.id().to_owned(), RetryUnitKind::Test)
            } else {
                let components = entry.id().split("::").collect::<Vec<_>>();
                (components[..4].join("::"), RetryUnitKind::Suite)
            };
            grouped
                .entry(id)
                .and_modify(|(_, _, leaves)| leaves.push(entry.id().to_owned()))
                .or_insert((index, kind, vec![entry.id().to_owned()]));
        }
        let mut units = grouped
            .into_iter()
            .map(|(id, (index, kind, execution_plan))| {
                (
                    index,
                    RetryUnit {
                        kind,
                        id,
                        execution_plan,
                    },
                )
            })
            .collect::<Vec<_>>();
        units.sort_by_key(|(index, _)| *index);
        if !units.is_empty() {
            rounds.push(tondo_compiler::test_report::ReportRetryRound {
                round,
                units: units.into_iter().map(|(_, unit)| unit).collect(),
            });
        }
    }
    rounds
}

fn identity_parts(id: &str) -> (String, String, Vec<String>) {
    let mut parts = id.split("::");
    let package = parts.next().unwrap_or("main").to_owned();
    let _source_class = parts.next();
    let module = parts.next().unwrap_or("test").to_owned();
    let path = parts.map(str::to_owned).collect();
    (package, module, path)
}

fn make_test_attempt(index: u32, source: &CliAttempt) -> Result<TestAttempt, TestCommandError> {
    let status = attempt_status(source.status);
    let mut attempt = TestAttempt::new(index, source.iteration, source.round, source.unit, status);
    attempt.logs = source
        .report
        .logs()
        .iter()
        .map(|log| log.message().to_owned())
        .collect();
    attempt.tags = source.report.tags().clone();
    attempt.stdout = source.report.stdout().to_owned();
    attempt.stderr = source.report.stderr().to_owned();
    attempt.virtual_time = source
        .report
        .virtual_time()
        .iter()
        .map(|record| VirtualTimeRecord {
            index: record.index(),
            elapsed_ns: record.elapsed_ns().to_string(),
            automatic_advances: record.automatic_advances(),
            explicit_advances: record.advances(),
            settles: record.settles(),
        })
        .collect();
    attempt.artifacts = source
        .report
        .artifacts()
        .iter()
        .map(|artifact| {
            let sha256 = artifact
                .sha256()
                .strip_prefix("sha256:")
                .unwrap_or_default()
                .to_owned();
            ArtifactRecord {
                name: artifact.name().to_owned(),
                media_type: artifact.media_type().to_owned(),
                size: artifact.bytes().len() as u64,
                sha256: sha256.clone(),
                object: format!("objects/{sha256}"),
            }
        })
        .collect();
    attempt.snapshots = source
        .report
        .snapshots()
        .iter()
        .map(|snapshot| {
            let (status, expected_sha256, actual_sha256) = match snapshot.outcome() {
                SnapshotOutcome::Matched {
                    expected_sha256,
                    actual_sha256,
                } => (
                    SnapshotStatus::Matched,
                    Some(expected_sha256),
                    actual_sha256,
                ),
                SnapshotOutcome::Missing { actual_sha256 } => (
                    if source
                        .snapshot_updates
                        .iter()
                        .any(|(name, _)| name == snapshot.name())
                    {
                        SnapshotStatus::Created
                    } else {
                        SnapshotStatus::Missing
                    },
                    None,
                    actual_sha256,
                ),
                SnapshotOutcome::Mismatched {
                    expected_sha256,
                    actual_sha256,
                } => (
                    if source
                        .snapshot_updates
                        .iter()
                        .any(|(name, _)| name == snapshot.name())
                    {
                        SnapshotStatus::Updated
                    } else {
                        SnapshotStatus::Mismatched
                    },
                    Some(expected_sha256),
                    actual_sha256,
                ),
            };
            SnapshotRecord {
                name: snapshot.name().to_owned(),
                status,
                expected_sha256: expected_sha256
                    .map(|hash| hash.strip_prefix("sha256:").unwrap_or(hash).to_owned()),
                actual_sha256: actual_sha256
                    .strip_prefix("sha256:")
                    .unwrap_or(actual_sha256)
                    .to_owned(),
            }
        })
        .collect();
    if matches!(
        status,
        AttemptStatus::FailedError
            | AttemptStatus::FailedPanic
            | AttemptStatus::ResourceLimit
            | AttemptStatus::Timeout
            | AttemptStatus::Infrastructure
    ) {
        attempt.failure = failure_record(source);
    }
    if status == AttemptStatus::Skipped {
        attempt.skip = Some(SkipRecord {
            reason: skip_reason(source),
            source: None,
        });
    }
    if matches!(
        status,
        AttemptStatus::BlockedSetup | AttemptStatus::BlockedSkip
    ) && let Some(suite) = source.error.as_ref().and_then(|error| match error {
        RunError::BlockedSetup { suite } | RunError::BlockedSkip { suite } => Some(suite),
        _ => None,
    }) {
        attempt.blocked_by = Some(BlockedBy {
            id: suite.clone(),
            attempt: index,
        });
    }
    Ok(attempt)
}

fn attempt_status(status: RuntimeStatus) -> AttemptStatus {
    match status {
        RuntimeStatus::Passed => AttemptStatus::Passed,
        RuntimeStatus::Skipped => AttemptStatus::Skipped,
        RuntimeStatus::FailedError => AttemptStatus::FailedError,
        RuntimeStatus::FailedPanic => AttemptStatus::FailedPanic,
        RuntimeStatus::ResourceLimit => AttemptStatus::ResourceLimit,
        RuntimeStatus::Timeout => AttemptStatus::Timeout,
        RuntimeStatus::Infrastructure => AttemptStatus::Infrastructure,
        RuntimeStatus::BlockedSetup => AttemptStatus::BlockedSetup,
        RuntimeStatus::BlockedSkip => AttemptStatus::BlockedSkip,
    }
}

fn failure_record(source: &CliAttempt) -> Option<FailureRecord> {
    let status = attempt_status(source.status);
    if !matches!(
        status,
        AttemptStatus::FailedError
            | AttemptStatus::FailedPanic
            | AttemptStatus::ResourceLimit
            | AttemptStatus::Timeout
            | AttemptStatus::Infrastructure
    ) {
        return None;
    }
    if let Some(error) = &source.error {
        let kind = match error {
            RunError::Error { .. } => "error",
            RunError::Panic { .. } | RunError::Control(_) => "panic",
            RunError::ResourceLimit { .. } => "resource-limit",
            RunError::Timeout | RunError::ForcedTermination { .. } => "timeout",
            RunError::Infrastructure { .. } => "infrastructure",
            RunError::Skip { .. } => "skip",
            RunError::BlockedSetup { .. } => "blocked-setup",
            RunError::BlockedSkip { .. } => "blocked-skip",
        };
        return Some(FailureRecord {
            kind: kind.into(),
            code: error.code().map(str::to_owned),
            message: error.to_string().replace(['\r', '\n'], " "),
            source: None,
        });
    }
    let (kind, code, message) = match source.report.terminal() {
        Some(Terminal::FailNow { code, message }) => {
            ("panic", Some((*code).to_owned()), message.clone())
        }
        Some(Terminal::CleanupFailure { code, message }) => {
            ("panic", Some(code.clone()), message.clone())
        }
        Some(Terminal::ResourceLimit { kind }) => {
            ("resource-limit", Some((*kind).to_owned()), kind.to_string())
        }
        Some(Terminal::Skipped { reason }) => ("skip", None, reason.clone()),
        None => ("backend", None, "test attempt failed".into()),
    };
    Some(FailureRecord {
        kind: kind.into(),
        code,
        message: message.replace(['\r', '\n'], " "),
        source: None,
    })
}

fn skip_reason(source: &CliAttempt) -> String {
    if let Some(RunError::Skip { reason }) = &source.error {
        return reason.clone();
    }
    if let Some(Terminal::Skipped { reason }) = source.report.terminal() {
        return reason.clone();
    }
    "test skipped".into()
}

fn publish_attempt_artifacts(
    base: &Path,
    plan: &test_cli::TestCliPlan,
    artifact_store: Option<&tondo_compiler::test_plan::TestArtifactStore>,
    attempts: &[CliAttempt],
) -> Result<(), TestCommandError> {
    let root = plan.artifacts.as_ref().map_or_else(
        || base.join(artifact_store.map_or("target/test-artifacts", |store| store.path())),
        |path| base.join(path),
    );
    let max_bytes = artifact_store.map_or(64 * 1024 * 1024, |store| store.max_bytes());
    for (index, attempt) in attempts.iter().enumerate() {
        if attempt.report.artifacts().is_empty() && plan.artifacts.is_none() {
            continue;
        }
        let identity = format!(
            "{}-{}-{}-{}",
            attempt.id, attempt.iteration, attempt.round, index
        );
        let mut store = tondo_compiler::test_artifacts::ArtifactStore::new(
            &root,
            identity,
            tondo_compiler::test_artifacts::ArtifactLimits::new(max_bytes, 64),
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        for evidence in attempt.report.artifacts() {
            let descriptor = store
                .attach(evidence.name(), evidence.media_type(), evidence.bytes())
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            let expected = evidence.sha256();
            if descriptor.sha256 != expected {
                return Err(TestCommandError::Internal(format!(
                    "artifact digest changed while publishing `{}`",
                    evidence.name()
                )));
            }
        }
        store
            .publish()
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    }
    Ok(())
}

fn status_label(status: AggregateStatus) -> &'static str {
    match status {
        AggregateStatus::Passed => "PASS",
        AggregateStatus::FlakyPass => "FLAKY",
        AggregateStatus::Skipped => "SKIP",
        AggregateStatus::FailedError => "FAIL",
        AggregateStatus::FailedPanic => "PANIC",
        AggregateStatus::ResourceLimit => "LIMIT",
        AggregateStatus::Timeout => "TIMEOUT",
        AggregateStatus::Infrastructure => "INTERNAL",
        AggregateStatus::BlockedSetup => "BLOCKED",
        AggregateStatus::BlockedSkip => "BLOCKED",
    }
}

fn publish_test_outputs(
    plan: &test_cli::TestCliPlan,
    report: &TestReport,
) -> Result<(), TestCommandError> {
    for output in &plan.reports {
        let bytes = match output.format {
            test_cli::TestReportFormat::Json => report
                .canonical_bytes()
                .map_err(|error| TestCommandError::Internal(error.to_string()))?,
            test_cli::TestReportFormat::Junit => {
                tondo_compiler::test_junit::JUnitReport::from_report(report)
                    .map_err(|error| TestCommandError::Internal(error.to_string()))?
                    .into_bytes()
            }
        };
        atomic_publish(&output.path, &bytes)?;
    }
    Ok(())
}

fn atomic_publish(path: &Path, bytes: &[u8]) -> Result<(), TestCommandError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            TestCommandError::Internal(format!("cannot create report directory: {error}"))
        })?;
    }
    let temporary = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        path.extension().and_then(OsStr::to_str).unwrap_or("report")
    ));
    fs::write(&temporary, bytes)
        .map_err(|error| TestCommandError::Internal(format!("cannot write report: {error}")))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        TestCommandError::Internal(format!("cannot publish report: {error}"))
    })
}

#[derive(Debug)]
struct Invocation {
    operation: Operation,
    source_form: SourceForm,
    diagnostic_format: DiagnosticFormat,
    warning_profiles: BTreeSet<WarningProfile>,
    format_check: bool,
    source: Option<PathBuf>,
    project: Option<PathBuf>,
    emit_interface: Option<PathBuf>,
    emit_artifact: Option<PathBuf>,
    program_arguments: Vec<String>,
}

fn parse_invocation(arguments: &[OsString]) -> Result<Invocation, String> {
    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Err("a UTF-8 command is required".into());
    };
    let (operation, source_form) = match command {
        "fmt" => (Operation::Format, SourceForm::Module),
        "check" => (Operation::Check, SourceForm::Module),
        "run" => (Operation::Run, SourceForm::Script),
        _ => return Err(format!("unknown command `{command}`")),
    };

    let mut diagnostic_format = DiagnosticFormat::Human;
    let mut warning_profiles = BTreeSet::new();
    let mut format_check = false;
    let mut source: Option<PathBuf> = None;
    let mut project: Option<PathBuf> = None;
    let mut emit_interface: Option<PathBuf> = None;
    let mut emit_artifact: Option<PathBuf> = None;
    let mut program_arguments = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            if operation != Operation::Run {
                return Err("program arguments are only valid with `tondo run`".into());
            }
            if source.is_none() && project.is_none() {
                return Err("the source file or project must appear before `--`".into());
            }
            program_arguments = arguments[index + 1..]
                .iter()
                .map(|argument| {
                    argument
                        .clone()
                        .into_string()
                        .map_err(|_| "program arguments must be valid UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            break;
        } else if argument == "--diagnostic-format" {
            index += 1;
            let Some(value) = arguments.get(index).and_then(|value| value.to_str()) else {
                return Err("`--diagnostic-format` requires `human` or `json`".into());
            };
            diagnostic_format = parse_diagnostic_format(value)?;
        } else if argument == "--check" {
            if operation != Operation::Format {
                return Err("`--check` is only valid with `tondo fmt`".into());
            }
            format_check = true;
        } else if argument == "--warnings" {
            index += 1;
            let Some(value) = arguments.get(index).and_then(|value| value.to_str()) else {
                return Err("`--warnings` requires `core`".into());
            };
            warning_profiles.insert(parse_warning_profile(value)?);
        } else if argument == "--project" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--project` requires a directory".into());
            };
            if project.replace(PathBuf::from(value)).is_some() {
                return Err("`--project` may appear only once".into());
            }
        } else if argument == "--emit-interface" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--emit-interface` requires a path".into());
            };
            if emit_interface.replace(PathBuf::from(value)).is_some() {
                return Err("`--emit-interface` may appear only once".into());
            }
        } else if argument == "--emit-artifact" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--emit-artifact` requires a path".into());
            };
            if emit_artifact.replace(PathBuf::from(value)).is_some() {
                return Err("`--emit-artifact` may appear only once".into());
            }
        } else if let Some(argument) = argument.to_str() {
            if let Some(value) = argument.strip_prefix("--diagnostic-format=") {
                diagnostic_format = parse_diagnostic_format(value)?;
            } else if let Some(value) = argument.strip_prefix("--warnings=") {
                warning_profiles.insert(parse_warning_profile(value)?);
            } else if argument.starts_with('-') {
                return Err(format!("unknown option `{argument}`"));
            } else if source.replace(PathBuf::from(argument)).is_some() {
                return Err("bootstrap commands accept exactly one source file".into());
            }
        } else if source.replace(PathBuf::from(argument)).is_some() {
            return Err("bootstrap commands accept exactly one source file".into());
        }
        index += 1;
    }

    if source.is_some() && project.is_some() {
        return Err("choose either one source file or `--project`, not both".into());
    }
    if source.is_none() && project.is_none() {
        if operation == Operation::Format {
            return Err("a source file is required for `tondo fmt`".into());
        }
        project = Some(PathBuf::from("."));
    }
    if operation == Operation::Format && project.is_some() {
        return Err("`tondo fmt` accepts a source file, not a project".into());
    }
    if operation == Operation::Format && (emit_interface.is_some() || emit_artifact.is_some()) {
        return Err("build products are only available from `check` or `run`".into());
    }
    if operation == Operation::Format && !warning_profiles.is_empty() {
        return Err("warning profiles are only available from `check` or `run`".into());
    }
    if let Some(source) = &source {
        validate_source_extension(source)?;
        if source.file_name().and_then(OsStr::to_str).is_none() {
            return Err("source filename is not valid UTF-8".into());
        }
    }
    if let (Some(interface), Some(artifact)) = (&emit_interface, &emit_artifact)
        && paths_refer_to_same_location(interface, artifact)
    {
        return Err("interface and artifact outputs require distinct paths".into());
    }
    if let Some(source_path) = &source {
        for output in [&emit_interface, &emit_artifact].into_iter().flatten() {
            if paths_refer_to_same_location(output, source_path) {
                return Err("an emitted product must not overwrite the source file".into());
            }
        }
    }
    Ok(Invocation {
        operation,
        source_form,
        diagnostic_format,
        warning_profiles,
        format_check,
        source,
        project,
        emit_interface,
        emit_artifact,
        program_arguments,
    })
}

fn compilation_request(invocation: &Invocation) -> Result<PreparedCompilation, String> {
    if let Some(project_path) = &invocation.project {
        let (base, manifest_bytes, lockfile_bytes) = discover_cli_project(project_path)?;
        let plan = ProjectPlan::parse(&manifest_bytes, &lockfile_bytes)
            .map_err(|error| error.to_string())?;
        let mut supplied = BTreeMap::new();
        for input in plan.required_inputs() {
            let physical = base.join(input.path());
            reject_product_input_collision(invocation, &physical, input.path())?;
            let bytes = read_input(
                &physical,
                &format!("{} input `{}`", input.kind().as_str(), input.path()),
            )?;
            supplied.insert(input.path().to_owned(), Arc::<[u8]>::from(bytes));
        }
        let request = plan
            .resolve(&supplied)
            .map_err(|error| error.to_string())?
            .into_compilation_request(
                invocation.operation,
                invocation.diagnostic_format,
                ResourceLimits::default(),
            )
            .map_err(|error| error.to_string())?;
        return Ok((request, None));
    }

    let source = invocation
        .source
        .as_ref()
        .expect("parse_invocation requires a source or project");
    let bytes = Arc::<[u8]>::from(read_input(source, "source")?);
    let file_name = source
        .file_name()
        .and_then(OsStr::to_str)
        .expect("parse_invocation validated the UTF-8 source filename");
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::new(
            SourceId::new("root:cli").map_err(|error| error.to_string())?,
            ModulePath::new("main").map_err(|error| error.to_string())?,
            LogicalPath::new(file_name).map_err(|error| error.to_string())?,
            SourceOrigin::Physical,
            bytes.clone(),
        ))
        .map_err(|error| error.to_string())?;
    let request = CompilationRequest::new(
        invocation.operation,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BuildTarget::vm_hosted_capabilities(),
        invocation.diagnostic_format,
        invocation.source_form,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?,
        sources,
        root,
    )
    .map_err(|error| error.to_string())?;
    Ok((request, Some(bytes)))
}

fn discover_cli_project(project_path: &Path) -> Result<(PathBuf, Vec<u8>, Vec<u8>), String> {
    let root = project_path.canonicalize().map_err(|error| {
        format!(
            "cannot resolve project directory `{}`: {error}",
            project_path.display()
        )
    })?;
    let discovered = project_discovery::discover(&root)?;
    Ok((
        discovered.root,
        discovered.manifest_bytes,
        discovered.lockfile_bytes,
    ))
}

fn reject_product_input_collision(
    invocation: &Invocation,
    physical_input: &Path,
    logical_input: &str,
) -> Result<(), String> {
    for output in [&invocation.emit_interface, &invocation.emit_artifact]
        .into_iter()
        .flatten()
    {
        if paths_refer_to_same_location(output, physical_input) {
            return Err(format!(
                "an emitted product must not overwrite project input `{logical_input}`"
            ));
        }
    }
    Ok(())
}

fn paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (fs::canonicalize(left), fs::canonicalize(right)) {
        return left == right;
    }
    match (
        normalized_absolute_path(left),
        normalized_absolute_path(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn normalized_absolute_path(path: &Path) -> Option<PathBuf> {
    let path = std::path::absolute(path).ok()?;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Some(normalized)
}

fn read_input(path: &Path, description: &str) -> Result<Vec<u8>, String> {
    fs::read(path)
        .map_err(|error| format!("cannot read {description} `{}`: {error}", path.display()))
}

fn emit_products(
    invocation: &Invocation,
    output: &tondo_compiler::driver::CompilationOutput,
) -> Result<(), String> {
    if output.status() != CompilationStatus::Success {
        return Ok(());
    }
    if let Some(path) = &invocation.emit_interface {
        let interface = output
            .interface()
            .ok_or_else(|| "successful compilation produced no interface".to_owned())?;
        let bytes = interface.encode().map_err(|error| error.to_string())?;
        fs::write(path, bytes)
            .map_err(|error| format!("cannot write interface `{}`: {error}", path.display()))?;
    }
    if let Some(path) = &invocation.emit_artifact {
        let artifact = output
            .artifact()
            .ok_or_else(|| "successful compilation produced no build artifact".to_owned())?;
        let bytes = artifact.encode().map_err(|error| error.to_string())?;
        fs::write(path, bytes)
            .map_err(|error| format!("cannot write artifact `{}`: {error}", path.display()))?;
    }
    Ok(())
}

fn parse_diagnostic_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "json" => Ok(DiagnosticFormat::Json),
        _ => Err(format!(
            "unknown diagnostic format `{value}`; expected `human` or `json`"
        )),
    }
}

fn parse_warning_profile(value: &str) -> Result<WarningProfile, String> {
    match value {
        "core" => Ok(WarningProfile::Core),
        _ => Err(format!(
            "unknown warning profile `{value}`; expected `core`"
        )),
    }
}

fn validate_source_extension(path: &Path) -> Result<(), String> {
    if path.extension() == Some(OsStr::new("to")) {
        Ok(())
    } else {
        Err("source file must use the `.to` extension".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation_error(values: &[&str]) -> String {
        parse_invocation(&arguments(values)).unwrap_err()
    }

    fn temp_root() -> PathBuf {
        #[cfg(unix)]
        {
            // macOS exposes the temporary directory through /var, which is a
            // symlink to /private/var.  The artifact and snapshot stores
            // intentionally reject symlinked path components, so fixtures
            // must start from the physical path they are validating.
            std::fs::canonicalize(std::env::temp_dir()).unwrap()
        }
        #[cfg(not(unix))]
        {
            std::env::temp_dir()
        }
    }

    fn remove_json_nulls(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                fields.retain(|_, value| !value.is_null());
                for value in fields.values_mut() {
                    remove_json_nulls(value);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    remove_json_nulls(value);
                }
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }

    fn conventional_test_project(source: &[u8]) -> PathBuf {
        let root = temp_root().join(format!(
            "tondo-cli-backend-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/main.to"), b"fn main() {}\n").unwrap();
        fs::write(root.join("tests/smoke.to"), source).unwrap();
        fs::write(root.join("tondo.toml"), "[package]\nname = \"cli\"\n").unwrap();
        let discovered = project_discovery::discover_for_tests(&root).unwrap();
        let project =
            ProjectPlan::parse(&discovered.manifest_bytes, &discovered.lockfile_bytes).unwrap();
        let package = project.selected_source_records().next().unwrap().0;
        fs::write(
            root.join("tests/snapshots.json"),
            SnapshotStore::empty(package)
                .unwrap()
                .canonical_bytes()
                .unwrap(),
        )
        .unwrap();
        let mut test_plan: serde_json::Value = serde_json::from_slice(
            &TestProjectPlan::defaults(&project, 1)
                .canonical_bytes()
                .unwrap(),
        )
        .unwrap();
        remove_json_nulls(&mut test_plan);
        for field in ["timeout_ms", "setup_timeout_ms", "teardown_timeout_ms"] {
            test_plan["limits"][field] = serde_json::json!(10_000);
        }
        let test_plan_toml = toml::to_string(&toml::Value::try_from(test_plan).unwrap()).unwrap();
        fs::write(root.join("tondo.test.toml"), test_plan_toml).unwrap();
        root
    }

    #[test]
    fn parses_json_diagnostics_in_either_option_form() {
        for arguments in [
            vec!["check", "--diagnostic-format", "json", "main.to"],
            vec!["check", "--diagnostic-format=json", "main.to"],
        ] {
            let arguments = arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            let invocation = parse_invocation(&arguments).unwrap();
            assert_eq!(invocation.diagnostic_format, DiagnosticFormat::Json);
        }
    }

    #[test]
    fn rejects_multiple_sources() {
        let arguments = ["check", "one.to", "two.to"].map(OsString::from).to_vec();
        assert!(parse_invocation(&arguments).is_err());
    }

    #[test]
    fn format_check_flag_is_scoped_to_the_formatter() {
        let format = ["fmt", "--check", "main.to"].map(OsString::from).to_vec();
        assert!(parse_invocation(&format).unwrap().format_check);

        for command in ["check", "run"] {
            let arguments = [command, "--check", "main.to"].map(OsString::from).to_vec();
            assert!(parse_invocation(&arguments).is_err());
        }
    }

    #[test]
    fn run_preserves_arguments_after_separator() {
        let arguments = ["run", "main.to", "--", "--flag", "two words"]
            .map(OsString::from)
            .to_vec();
        let invocation = parse_invocation(&arguments).unwrap();
        assert_eq!(invocation.program_arguments, ["--flag", "two words"]);
    }

    #[test]
    fn non_run_commands_reject_program_arguments() {
        for command in ["fmt", "check"] {
            let arguments = [command, "main.to", "--", "argument"]
                .map(OsString::from)
                .to_vec();
            assert!(parse_invocation(&arguments).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_non_utf8_program_arguments() {
        use std::os::unix::ffi::OsStringExt;

        let arguments = vec![
            OsString::from("run"),
            OsString::from("main.to"),
            OsString::from("--"),
            OsString::from_vec(vec![0xff]),
        ];
        assert!(
            parse_invocation(&arguments)
                .unwrap_err()
                .contains("valid UTF-8")
        );
    }

    #[test]
    fn project_products_cannot_overwrite_a_declared_input() {
        let arguments = [
            "check",
            "--project",
            "project",
            "--emit-interface",
            "project/src/main.to",
        ]
        .map(OsString::from)
        .to_vec();
        let invocation = parse_invocation(&arguments).unwrap();
        assert!(matches!(
            reject_product_input_collision(
                &invocation,
                Path::new("project/src/main.to"),
                "src/main.to"
            ),
            Err(message) if message.contains("must not overwrite project input")
        ));
        assert!(paths_refer_to_same_location(
            Path::new("project/build/../src/main.to"),
            Path::new("project/src/main.to")
        ));
        assert!(paths_refer_to_same_location(
            Path::new("project/out/../interface.ti"),
            Path::new("project/interface.ti")
        ));
    }

    #[test]
    fn invocation_rejects_every_ambiguous_or_incomplete_cli_shape() {
        let invalid = [
            (&[][..], "UTF-8 command"),
            (&["unknown", "main.to"], "unknown command"),
            (&["run", "--"], "must appear before `--`"),
            (
                &["check", "--diagnostic-format"],
                "`--diagnostic-format` requires",
            ),
            (
                &["check", "--diagnostic-format", "xml", "main.to"],
                "unknown diagnostic format",
            ),
            (&["check", "--warnings"], "`--warnings` requires"),
            (
                &["check", "--warnings", "all", "main.to"],
                "unknown warning profile",
            ),
            (&["check", "--project"], "`--project` requires"),
            (
                &["check", "--project", "one", "--project", "two"],
                "`--project` may appear only once",
            ),
            (
                &["check", "--emit-interface"],
                "`--emit-interface` requires",
            ),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-interface",
                    "one.ti",
                    "--emit-interface",
                    "two.ti",
                ],
                "`--emit-interface` may appear only once",
            ),
            (&["check", "--emit-artifact"], "`--emit-artifact` requires"),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-artifact",
                    "one.ta",
                    "--emit-artifact",
                    "two.ta",
                ],
                "`--emit-artifact` may appear only once",
            ),
            (&["check", "--unknown", "main.to"], "unknown option"),
            (
                &["check", "main.to", "--project", "project"],
                "choose either",
            ),
            (&["fmt", "--project", "project"], "accepts a source file"),
            (
                &["fmt", "main.to", "--emit-interface", "main.ti"],
                "build products",
            ),
            (&["fmt", "--warnings=core", "main.to"], "warning profiles"),
            (
                &["check", "--lockfile", "tondo.lock.toml", "main.to"],
                "unknown option",
            ),
            (&["check", "main.tondo"], "`.to` extension"),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-interface",
                    "product",
                    "--emit-artifact",
                    "product",
                ],
                "distinct paths",
            ),
            (
                &["check", "main.to", "--emit-artifact", "main.to"],
                "must not overwrite the source file",
            ),
        ];

        for (values, expected) in invalid {
            let error = invocation_error(values);
            assert!(
                error.contains(expected),
                "`{values:?}` returned unexpected error: {error}"
            );
        }

        for values in [
            &["check", "--diagnostic-format", "human", "main.to"][..],
            &["check", "--warnings", "core", "main.to"][..],
            &["check", "--warnings=core", "main.to"][..],
            &["check", "--project", "example"][..],
            &["run", "--project", "example", "--", "arg"][..],
        ] {
            parse_invocation(&arguments(values)).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_commands_and_source_names_are_rejected_at_their_boundaries() {
        use std::os::unix::ffi::OsStringExt;

        assert!(
            parse_invocation(&[OsString::from_vec(vec![0xff])])
                .unwrap_err()
                .contains("UTF-8 command")
        );
        let invalid_name = OsString::from_vec(vec![0xff, b'.', b't', b'o']);
        assert!(
            parse_invocation(&[OsString::from("check"), invalid_name.clone()])
                .unwrap_err()
                .contains("filename is not valid UTF-8")
        );
        assert!(
            parse_invocation(&[
                OsString::from("check"),
                OsString::from("main.to"),
                invalid_name,
            ])
            .unwrap_err()
            .contains("exactly one source file")
        );
    }

    #[test]
    fn backend_helpers_preserve_campaign_and_report_contracts() {
        let plan = test_cli::parse(
            &[
                "test",
                "--timeout",
                "2s",
                "--jobs",
                "3",
                "--shard",
                "1/2",
                "--order",
                "random",
                "--seed",
                "a",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(campaign_limits(&plan)["timeout_ms"], 2_000);
        assert_eq!(campaign_limits(&plan)["jobs"], 3);
        assert_eq!(shard_identity(&plan), "1/2");
        assert_eq!(order_seed(&plan), 10);

        let canonical = test_cli::parse(&["test"].map(OsString::from)).unwrap();
        assert_eq!(shard_identity(&canonical), "all");
        assert_eq!(order_seed(&canonical), 0);
        assert_eq!(
            identity_parts("pkg::integration::module::suite::case"),
            (
                "pkg".into(),
                "module".into(),
                vec!["suite".into(), "case".into()],
            )
        );
        assert_eq!(
            identity_parts("bare"),
            ("bare".into(), "test".into(), Vec::new())
        );

        for (runtime, result) in [
            (RuntimeStatus::Passed, AttemptStatus::Passed),
            (RuntimeStatus::Skipped, AttemptStatus::Skipped),
            (RuntimeStatus::FailedError, AttemptStatus::FailedError),
            (RuntimeStatus::FailedPanic, AttemptStatus::FailedPanic),
            (RuntimeStatus::ResourceLimit, AttemptStatus::ResourceLimit),
            (RuntimeStatus::Timeout, AttemptStatus::Timeout),
            (RuntimeStatus::Infrastructure, AttemptStatus::Infrastructure),
            (RuntimeStatus::BlockedSetup, AttemptStatus::BlockedSetup),
            (RuntimeStatus::BlockedSkip, AttemptStatus::BlockedSkip),
        ] {
            assert_eq!(attempt_status(runtime), result);
        }
        for (status, label) in [
            (AggregateStatus::Passed, "PASS"),
            (AggregateStatus::FlakyPass, "FLAKY"),
            (AggregateStatus::Skipped, "SKIP"),
            (AggregateStatus::FailedError, "FAIL"),
            (AggregateStatus::FailedPanic, "PANIC"),
            (AggregateStatus::ResourceLimit, "LIMIT"),
            (AggregateStatus::Timeout, "TIMEOUT"),
            (AggregateStatus::Infrastructure, "INTERNAL"),
            (AggregateStatus::BlockedSetup, "BLOCKED"),
            (AggregateStatus::BlockedSkip, "BLOCKED"),
        ] {
            assert_eq!(status_label(status), label);
        }

        let envelope = tondo_compiler::test_control::EnvelopeHandle::new(
            "cli-helper",
            EnvelopeLimits::new(1_000, 1_000, 1_000),
        );
        let report = envelope.report().unwrap();
        let failed = CliAttempt {
            id: "pkg::unit::mod::failed".into(),
            iteration: 1,
            round: 0,
            unit: None,
            status: RuntimeStatus::FailedError,
            report: report.clone(),
            error: Some(RunError::Error {
                code: "E-test".into(),
                message: "bad\nvalue".into(),
            }),
            snapshot_updates: Vec::new(),
        };
        let failure = failure_record(&failed).unwrap();
        assert_eq!(failure.kind, "error");
        assert_eq!(failure.code.as_deref(), Some("E-test"));
        assert_eq!(failure.message, "E-test: bad value");
        let attempt = make_test_attempt(1, &failed).unwrap();
        assert!(attempt.failure.is_some());

        let skipped = CliAttempt {
            id: "pkg::unit::mod::skipped".into(),
            iteration: 1,
            round: 0,
            unit: None,
            status: RuntimeStatus::Skipped,
            report: report.clone(),
            error: Some(RunError::Skip {
                reason: "not applicable".into(),
            }),
            snapshot_updates: Vec::new(),
        };
        assert_eq!(skip_reason(&skipped), "not applicable");
        assert!(make_test_attempt(2, &skipped).unwrap().skip.is_some());

        let base = temp_root().join(format!(
            "tondo-cli-helper-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(base.join(".github")).unwrap();
        fs::write(base.join(".github/CODEOWNERS"), b"* @tondo\n").unwrap();
        let owners_plan = test_cli::parse(&["test"].map(OsString::from)).unwrap();
        let owners = resolve_ownership(&owners_plan, &base).unwrap();
        assert_eq!(
            owners
                .resolution
                .owners_for(Some("tests/smoke.to"))
                .unwrap(),
            ["@tondo"]
        );
        let none_plan =
            test_cli::parse(&["test", "--codeowners", "none"].map(OsString::from)).unwrap();
        assert!(
            resolve_ownership(&none_plan, &base)
                .unwrap()
                .resolution
                .owners_for(None)
                .unwrap()
                .is_empty()
        );

        let report_path = base.join("nested/report.json");
        atomic_publish(&report_path, b"ok").unwrap();
        assert_eq!(fs::read(&report_path).unwrap(), b"ok");
        let artifacts_plan =
            test_cli::parse(&["test", "--artifacts", "artifacts"].map(OsString::from)).unwrap();
        publish_attempt_artifacts(&base, &artifacts_plan, None, &[failed]).unwrap();
        assert!(base.join("artifacts").exists());
        fs::remove_dir_all(base).unwrap();

        let project = conventional_test_project(b"test smoke { assert(true) }\n");
        let helper_report = format!(
            "json={}",
            project.join("target/helper-report.json").display()
        );
        let run_plan = test_cli::parse(
            [
                OsString::from("test"),
                OsString::from("--project"),
                OsString::from(project.to_str().unwrap()),
                OsString::from("--order"),
                OsString::from("random"),
                OsString::from("--seed"),
                OsString::from("a"),
                OsString::from("--report"),
                OsString::from(helper_report),
                OsString::from("--test-format"),
                OsString::from("json"),
                OsString::from("--timeout"),
                OsString::from("10s"),
            ]
            .as_slice(),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&run_plan, &project).unwrap(), 0);
        let list_plan = test_cli::parse(
            &[
                "test",
                "--project",
                project.to_str().unwrap(),
                "--list",
                "--test-format",
                "json",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&list_plan, &project).unwrap(), 0);
        let repeat_plan = test_cli::parse(
            &[
                "test",
                "--project",
                project.to_str().unwrap(),
                "--repeat",
                "2",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&repeat_plan, &project).unwrap(), 0);
        for selector in [
            &["--filter", "smoke"][..],
            &["--glob", "*smoke"][..],
            &["--exact", "smoke"][..],
            &["--shard", "1/1"][..],
        ] {
            let mut values = vec!["test", "--project", project.to_str().unwrap()];
            values.extend_from_slice(selector);
            let selected_plan =
                test_cli::parse(&values.into_iter().map(OsString::from).collect::<Vec<_>>())
                    .unwrap();
            assert_eq!(execute_test_plan(&selected_plan, &project).unwrap(), 0);
        }
        let human_list = test_cli::parse(
            &["test", "--project", project.to_str().unwrap(), "--list"].map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&human_list, &project).unwrap(), 0);
        let show_output = test_cli::parse(
            &[
                "test",
                "--project",
                project.to_str().unwrap(),
                "--show-output",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&show_output, &project).unwrap(), 0);
        let too_long = test_cli::parse(
            &[
                "test",
                "--project",
                project.to_str().unwrap(),
                "--timeout",
                "20s",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert!(matches!(
            execute_test_plan(&too_long, &project),
            Err(TestCommandError::Usage(message)) if message.contains("cannot exceed")
        ));
        let no_match = [
            OsString::from("test"),
            OsString::from("--project"),
            project.clone().into(),
            OsString::from("--filter"),
            OsString::from("absent"),
        ];
        assert_eq!(
            run(no_match.to_vec()).unwrap(),
            ExitCode::from(EXIT_DIAGNOSTIC)
        );
        let allow_empty = [
            OsString::from("test"),
            OsString::from("--project"),
            project.clone().into(),
            OsString::from("--filter"),
            OsString::from("absent"),
            OsString::from("--allow-empty"),
        ];
        assert_eq!(run(allow_empty.to_vec()).unwrap(), ExitCode::SUCCESS);
        let allow_empty_list = [
            OsString::from("test"),
            OsString::from("--project"),
            project.clone().into(),
            OsString::from("--filter"),
            OsString::from("absent"),
            OsString::from("--allow-empty"),
            OsString::from("--list"),
        ];
        assert_eq!(run(allow_empty_list.to_vec()).unwrap(), ExitCode::SUCCESS);
        let invalid_report_parent = project.join("report-parent");
        fs::write(&invalid_report_parent, b"not a directory").unwrap();
        let internal_report = [
            OsString::from("test"),
            OsString::from("--project"),
            project.clone().into(),
            OsString::from("--report"),
            OsString::from(format!(
                "json={}",
                invalid_report_parent.join("report.json").display()
            )),
        ];
        assert_eq!(
            run(internal_report.to_vec()).unwrap(),
            ExitCode::from(EXIT_INTERNAL)
        );
        let plain_source = project.join("main.to");
        fs::write(&plain_source, b"fn main() {}\n").unwrap();
        assert_eq!(
            run(vec![OsString::from("check"), plain_source.clone().into()]).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(vec![OsString::from("fmt"), plain_source.clone().into()]).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(vec![
                OsString::from("test"),
                OsString::from("--project"),
                project.clone().into(),
            ])
            .unwrap(),
            ExitCode::SUCCESS
        );
        fs::remove_dir_all(project).unwrap();

        assert_eq!(
            parse_diagnostic_format("json").unwrap(),
            DiagnosticFormat::Json
        );
        assert_eq!(parse_warning_profile("core").unwrap(), WarningProfile::Core);
        assert!(parse_diagnostic_format("xml").is_err());
        assert!(parse_warning_profile("all").is_err());
        assert!(validate_source_extension(Path::new("main.to")).is_ok());
        assert!(validate_source_extension(Path::new("main.txt")).is_err());
        assert!(normalized_absolute_path(Path::new(".")).is_some());
        assert!(read_input(Path::new("missing-input.to"), "source").is_err());
    }

    #[test]
    fn test_plan_loader_rejects_json_paths_before_io() {
        let root = conventional_test_project(b"test smoke { assert(true) }\n");
        let discovered = project_discovery::discover(&root).unwrap();
        let project =
            ProjectPlan::parse(&discovered.manifest_bytes, &discovered.lockfile_bytes).unwrap();
        let error =
            load_test_project_plan(&project, Some(Path::new("tondo.test.json"))).unwrap_err();
        assert!(matches!(
            error,
            TestCommandError::Usage(message) if message.contains("JSON plans are unsupported")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_check_does_not_compile_discovered_test_sources() {
        let root = conventional_test_project(b"this is not valid Tondo\n");
        assert_eq!(
            run(vec![
                OsString::from("check"),
                OsString::from("--project"),
                root.clone().into(),
            ])
            .unwrap(),
            ExitCode::SUCCESS
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_wire_and_snapshot_helpers_cover_closed_boundaries() {
        use tondo_compiler::test_control::{ControlError, EnvelopeHandle};

        let errors = [
            RunError::Error {
                code: "T1".into(),
                message: "error".into(),
            },
            RunError::Panic {
                code: "P1".into(),
                message: "panic".into(),
            },
            RunError::Control(ControlError::FailNow {
                message: "control".into(),
            }),
            RunError::ResourceLimit {
                kind: "output".into(),
            },
            RunError::Timeout,
            RunError::ForcedTermination {
                message: "forced".into(),
            },
            RunError::Infrastructure {
                message: "infrastructure".into(),
            },
            RunError::Skip {
                reason: "skip".into(),
            },
        ];
        for error in &errors {
            assert!(WorkerError::from_run_error(error).into_run_error().is_err());
        }
        for kind in [
            "error",
            "panic",
            "resource-limit",
            "timeout",
            "skip",
            "infrastructure",
            "unknown",
        ] {
            assert!(
                WorkerError {
                    kind: kind.into(),
                    code: None,
                    message: "wire error".into(),
                }
                .into_run_error()
                .is_err()
            );
        }
        for status in [
            RuntimeStatus::Passed,
            RuntimeStatus::Skipped,
            RuntimeStatus::FailedError,
            RuntimeStatus::FailedPanic,
            RuntimeStatus::ResourceLimit,
            RuntimeStatus::Timeout,
            RuntimeStatus::Infrastructure,
            RuntimeStatus::BlockedSetup,
            RuntimeStatus::BlockedSkip,
        ] {
            assert!(!runtime_status_wire(status).is_empty());
        }
        assert_eq!(
            format_test_command_error(TestCommandError::Usage("u".into())),
            "u"
        );
        assert_eq!(
            format_test_command_error(TestCommandError::Internal("i".into())),
            "i"
        );
        assert_eq!(
            format_test_command_error(TestCommandError::Diagnostic("d".into())),
            "d"
        );
        assert!(EnvelopeReport::decode_process(&empty_worker_report()).is_ok());
        let infrastructure = infrastructure_worker_response("worker failed");
        assert_eq!(infrastructure.status, "infrastructure");
        assert_eq!(
            infrastructure
                .error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("worker failed")
        );
        assert!(!combined_store_hash(&[]).is_empty());
        assert_eq!(combined_store_hash(&[("one", "sha256:abc".into())]), "abc");
        assert_eq!(combined_store_hash(&[("one", "abc".into())]), "abc");
        let combined =
            combined_store_hash(&[("one", "sha256:a".into()), ("two", "sha256:b".into())]);
        assert_eq!(combined.len(), 64);

        let empty_envelope =
            EnvelopeHandle::new("snapshot-test", EnvelopeLimits::new(4096, 4096, 4096));
        empty_envelope.close().unwrap();
        let report = empty_envelope.report().unwrap();
        let plan = test_cli::parse(&arguments(&["test", "--update-snapshots"])).unwrap();
        let attempt = CliAttempt {
            id: "pkg::production::smoke::smoke".into(),
            iteration: 1,
            round: 0,
            unit: None,
            status: RuntimeStatus::Passed,
            report: report.clone(),
            error: None,
            snapshot_updates: vec![("new-value".into(), "value".into())],
        };
        let base = temp_root().join(format!(
            "tondo-cli-snapshot-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let empty_store = SnapshotStore::empty("pkg").unwrap();
        let inputs = SnapshotInputs {
            stores: vec![LoadedSnapshotStore {
                name: "default".into(),
                relative: PathBuf::from("snapshots.json"),
                max_bytes: 4096,
                store: empty_store.clone(),
            }],
            before_sha256: "before".into(),
            update: true,
        };
        assert!(inputs.expected_for("missing").unwrap().is_empty());
        let published = inputs
            .stage_and_publish(&base, &plan, std::slice::from_ref(&attempt))
            .unwrap();
        assert!(published.published);
        let stored = SnapshotStore::load(&base, Path::new("snapshots.json")).unwrap();
        assert_eq!(stored.entries()[0].name, "new-value");

        let failed = CliAttempt {
            status: RuntimeStatus::FailedError,
            snapshot_updates: Vec::new(),
            ..CliAttempt {
                id: "pkg::production::smoke::smoke".into(),
                iteration: 1,
                round: 0,
                unit: None,
                status: RuntimeStatus::Passed,
                report: report.clone(),
                error: None,
                snapshot_updates: Vec::new(),
            }
        };
        let not_published = inputs.stage_and_publish(&base, &plan, &[failed]).unwrap();
        assert!(!not_published.published);
        assert_eq!(not_published.after_sha256, "before");

        let no_update = SnapshotInputs {
            update: false,
            ..inputs.clone()
        };
        assert!(
            !no_update
                .stage_and_publish(&base, &plan, &[])
                .unwrap()
                .published
        );
        let no_stores = SnapshotInputs {
            stores: Vec::new(),
            ..inputs.clone()
        };
        assert!(
            !no_stores
                .stage_and_publish(&base, &plan, &[])
                .unwrap()
                .published
        );

        let existing = SnapshotStore::from_entries(
            "pkg",
            [tondo_compiler::test_snapshots::SnapshotEntry {
                node_id: "pkg::production::smoke::smoke".into(),
                name: "known".into(),
                value: "old".into(),
            }],
        )
        .unwrap();
        let duplicate_inputs = SnapshotInputs {
            stores: vec![
                LoadedSnapshotStore {
                    name: "one".into(),
                    relative: PathBuf::from("one.json"),
                    max_bytes: 4096,
                    store: existing.clone(),
                },
                LoadedSnapshotStore {
                    name: "two".into(),
                    relative: PathBuf::from("two.json"),
                    max_bytes: 4096,
                    store: existing.clone(),
                },
            ],
            before_sha256: "before".into(),
            update: true,
        };
        assert!(
            duplicate_inputs
                .expected_for("pkg::production::smoke::smoke")
                .is_err()
        );
        let known_update = CliAttempt {
            id: "pkg::production::smoke::smoke".into(),
            iteration: 1,
            round: 0,
            unit: None,
            status: RuntimeStatus::Passed,
            report: report.clone(),
            error: None,
            snapshot_updates: vec![("known".into(), "new".into())],
        };
        assert!(
            duplicate_inputs
                .stage_and_publish(&base, &plan, &[known_update])
                .is_err()
        );
        let ambiguous_inputs = SnapshotInputs {
            stores: vec![
                LoadedSnapshotStore {
                    name: "one".into(),
                    relative: PathBuf::from("one.json"),
                    max_bytes: 4096,
                    store: empty_store.clone(),
                },
                LoadedSnapshotStore {
                    name: "two".into(),
                    relative: PathBuf::from("two.json"),
                    max_bytes: 4096,
                    store: empty_store.clone(),
                },
            ],
            before_sha256: "before".into(),
            update: true,
        };
        let ambiguous_attempt = CliAttempt {
            snapshot_updates: vec![("new-value".into(), "value".into())],
            ..attempt.clone()
        };
        assert!(
            ambiguous_inputs
                .stage_and_publish(&base, &plan, &[ambiguous_attempt])
                .is_err()
        );
        let limited_inputs = SnapshotInputs {
            stores: vec![LoadedSnapshotStore {
                name: "limited".into(),
                relative: PathBuf::from("limited.json"),
                max_bytes: 1,
                store: empty_store,
            }],
            before_sha256: "before".into(),
            update: true,
        };
        let limited_attempt = CliAttempt {
            snapshot_updates: vec![("new-value".into(), "value".into())],
            ..attempt
        };
        assert!(
            limited_inputs
                .stage_and_publish(&base, &plan, &[limited_attempt])
                .is_err()
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn ownership_selection_and_worker_wait_cover_non_happy_paths() {
        let base = temp_root().join(format!(
            "tondo-cli-owner-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(base.join("CODEOWNERS-dir")).unwrap();
        let explicit = base.join("CODEOWNERS");
        fs::write(&explicit, b"* @tondo\n").unwrap();
        let explicit_plan = test_cli::parse(&[
            OsString::from("test"),
            OsString::from("--codeowners"),
            OsString::from("CODEOWNERS"),
        ])
        .unwrap();
        let owners = resolve_ownership(&explicit_plan, &base).unwrap();
        assert_eq!(
            owners.mode,
            tondo_compiler::test_report::OwnershipMode::Explicit
        );
        let directory_plan = test_cli::parse(&[
            OsString::from("test"),
            OsString::from("--codeowners"),
            OsString::from("CODEOWNERS-dir"),
        ])
        .unwrap();
        assert!(resolve_ownership(&directory_plan, &base).is_err());
        let absent = read_codeowners_candidate(&base, "missing-CODEOWNERS").unwrap();
        assert!(!absent.is_present());
        assert!(run_test_worker_on_explicit_stack(Vec::new()).is_err());
        assert!(run_test_worker(&[]).is_err());
        assert!(run_test_worker(&[OsString::from("--unknown")]).is_err());
        assert!(run_test_worker(&[OsString::from("--project")]).is_err());
        assert!(run_test_worker(&[OsString::from("--entry")]).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&explicit, base.join("CODEOWNERS-link")).unwrap();
            let symlink_plan = test_cli::parse(&[
                OsString::from("test"),
                OsString::from("--codeowners"),
                OsString::from("CODEOWNERS-link"),
            ])
            .unwrap();
            assert!(resolve_ownership(&symlink_plan, &base).is_err());
        }

        #[cfg(unix)]
        {
            let child = Command::new("sh")
                .args(["-c", "printf out; printf err >&2"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let (_status, stdout, stderr) = wait_worker(child, None).unwrap();
            assert_eq!(stdout, b"out");
            assert_eq!(stderr, b"err");
            let child = Command::new("sh")
                .args([
                    "-c",
                    "dd if=/dev/zero bs=131072 count=1 2>/dev/null; dd if=/dev/zero bs=131072 count=1 >&2 2>/dev/null",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let (_status, stdout, stderr) = wait_worker(child, Some(10_000)).unwrap();
            assert_eq!(stdout.len(), 131_072);
            assert_eq!(stderr.len(), 131_072);
            let child = Command::new("sh")
                .args(["-c", "sleep 1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            assert!(matches!(
                wait_worker(child, Some(1)),
                Err(RunError::Timeout)
            ));
        }
        fs::remove_dir_all(base).unwrap();
    }
}
