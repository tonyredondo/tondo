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
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, DiagnosticProfile,
    Edition, HostProfile, Operation, ResourceLimits, SourceForm, WarningProfile, discover_tests,
    execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::project::ProjectPlan;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
};
use tondo_compiler::test_control::{EnvelopeLimits, EnvelopeReport, SnapshotOutcome, Terminal};
use tondo_compiler::test_glob::GlobPattern;
use tondo_compiler::test_inputs::{TestInputPlan, TestInputProfile};
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
    AggregateStatus, ArtifactRecord, AttemptPhase, AttemptStatus, BlockedBy, DiagnosticPrivacy,
    DiagnosticRecord, DiagnosticStatus, FailureRecord, ResultNodeKind, SkipRecord, SnapshotRecord,
    SnapshotStatus, TestAttempt, TestNode, VirtualTimeRecord,
};
use tondo_compiler::test_retry::RetryContext;
use tondo_compiler::test_runtime::{
    LeafProgram, RunError, RuntimeConfig, RuntimeRunner, RuntimeStatus,
};
use tondo_compiler::test_schedule::{OrderMode, ScheduleNode, SchedulePlan, Seed};
use tondo_compiler::test_shard::ShardSpec;
use tondo_compiler::test_snapshots::{SnapshotPolicy, SnapshotStore, SnapshotUpdateStage};
use tondo_vm::runtime::{
    DiagnosticTrace, DumpIdentity, DumpTermination, MAX_DUMP_BYTES, capture_dump, detect_leaks,
    detect_races,
};

mod doc_test;
mod project_discovery;
mod test_campaign;
mod test_cli;
mod test_deadline;
mod test_human;
mod test_inputs;
mod test_interrupt;
mod test_outputs;
mod test_processes;

const EXIT_DIAGNOSTIC: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_INTERNAL: u8 = 3;
const CLI_STACK_SIZE: usize = 8 * 1024 * 1024;

type PreparedCompilation = (CompilationRequest, Option<Arc<[u8]>>);

const USAGE: &str = "\
Tondo bootstrap toolchain

Usage:
  tondo <command> [--diagnostic-format <human|json>] [--warnings core] <source.to>
  tondo <check|run> [--diagnostic-format <human|json>] [--warnings core] [--project <dir>]
  tondo build [--project <dir>] [--emit-artifact <path>]
  tondo lock [--project <dir>]
  tondo run [--diagnostic-format <human|json>] [--warnings core] <source.to> -- [argument ...]
  tondo test [--project <dir>] [--test-plan <tondo.test.toml>] [--diagnostics <profiles>] [options]
  tondo dump analyze <file.tdump> [--format human|json]

Commands:
  fmt      Format one Tondo source file
  check    Analyze one Tondo source file
  build    Analyze and materialize a closed native build envelope
  lock     Resolve local source packages into tondo.lock.toml
  run      Compile and run one Tondo script
  doc-test Validate Tondo examples embedded in Markdown
  test     Discover, compile and run project tests
  dump     Analyze an offline diagnostic dump

Options:
  --diagnostic-format <human|json>  Select diagnostic output
  --warnings <core>                 Enable a closed warning profile
  --check                           Verify formatting without writing output (fmt only)
  --project <dir>                   Project directory (default: current directory)
  --test-plan <path>                Optional advanced TOML test-plan sidecar
  --process-cgroup <absolute-path>  Delegated Linux cgroup-v2 root for process tests
  --diagnostics <profiles>          Dynamic profiles: race,leaks,crash or all
  --emit-interface <path>           Write the canonical compiled interface on success
  --emit-artifact <path>            Write canonical build metadata on success
  -- [argument ...]                 Pass UTF-8 arguments to a run script
  -h, --help                        Show this help
  -V, --version                     Show version information";

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if matches!(
        arguments.first().and_then(|argument| argument.to_str()),
        Some("test" | "__test-worker")
    ) && let Err(error) = test_interrupt::install(
        arguments
            .first()
            .is_some_and(|argument| argument == "__test-worker"),
    ) {
        eprintln!("tondo: {error}");
        return ExitCode::from(EXIT_INTERNAL);
    }
    // The compiler deliberately accepts substantially larger programs than a
    // platform's default process stack can accommodate.  In particular,
    // Windows reserves about 1 MiB for the executable entry thread, while
    // recursive semantic passes may need several MiB even though the source
    // itself is within every language resource limit.  Keep the public CLI
    // entry point tiny and run the actual command on the same explicit stack
    // used by the isolated test worker below.  This makes CLI behavior
    // deterministic across hosts without changing the compiler API or the
    // Tondo runtime's logical stack limits.
    let command = std::thread::Builder::new()
        .name("tondo-cli".into())
        .stack_size(CLI_STACK_SIZE)
        .spawn(move || run(arguments));
    let result = match command {
        Ok(command) => command
            .join()
            .map_err(|_| "CLI command stack panicked".to_owned()),
        Err(error) => Err(format!("cannot create CLI command stack: {error}")),
    };
    match result {
        Ok(Ok(code)) => code,
        Ok(Err(error)) | Err(error) => {
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
    if arguments.first().and_then(|argument| argument.to_str()) == Some("doc-test") {
        return run_doc_test_command(&arguments);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("dump") {
        return run_dump_command(&arguments);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("lock") {
        let root = match &arguments[1..] {
            [] => env::current_dir().map_err(|error| error.to_string())?,
            [option, path] if option == "--project" => PathBuf::from(path),
            _ => {
                eprintln!("tondo: usage: tondo lock [--project <dir>]");
                return Ok(ExitCode::from(EXIT_USAGE));
            }
        };
        let bytes = match project_discovery::resolve_local_lock(&root) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("tondo: {error}");
                return Ok(ExitCode::from(EXIT_DIAGNOSTIC));
            }
        };
        atomic_publish(&root.join("tondo.lock.toml"), &bytes).map_err(|error| match error {
            TestCommandError::Usage(message)
            | TestCommandError::Internal(message)
            | TestCommandError::Diagnostic(message) => message,
        })?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut invocation = match parse_invocation(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    if invocation.build && invocation.emit_artifact.is_none() {
        invocation.emit_artifact = Some(default_build_artifact_path(&invocation));
    }
    let (request, original_source) = match compilation_request(&invocation) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("tondo: {message}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let request = request
        .with_warning_profiles(invocation.warning_profiles.iter().copied())
        .with_diagnostic_profiles(invocation.diagnostic_profiles.iter().copied())
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

fn run_doc_test_command(arguments: &[OsString]) -> Result<ExitCode, String> {
    match doc_test::execute(arguments) {
        Ok(output) => {
            io::stdout()
                .write_all(&output)
                .map_err(|error| format!("cannot write doc-test output: {error}"))?;
            Ok(ExitCode::SUCCESS)
        }
        Err(doc_test::DocTestError::Usage(message)) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            Ok(ExitCode::from(EXIT_USAGE))
        }
        Err(doc_test::DocTestError::Diagnostic(message)) => {
            eprintln!("tondo doc-test: {message}");
            Ok(ExitCode::from(EXIT_DIAGNOSTIC))
        }
        Err(doc_test::DocTestError::Internal(message)) => Err(message),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpOutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DumpInvocation {
    path: PathBuf,
    format: DumpOutputFormat,
}

fn parse_dump_invocation(arguments: &[OsString]) -> Result<DumpInvocation, String> {
    if arguments.first().and_then(|argument| argument.to_str()) != Some("dump")
        || arguments.get(1).and_then(|argument| argument.to_str()) != Some("analyze")
    {
        return Err("usage: tondo dump analyze <file.tdump> [--format human|json]".into());
    }
    let path = arguments
        .get(2)
        .cloned()
        .map(PathBuf::from)
        .ok_or_else(|| "`tondo dump analyze` requires a `.tdump` path".to_owned())?;
    if path.extension() != Some(OsStr::new("tdump")) {
        return Err("dump input must use the `.tdump` extension".into());
    }
    let mut format = DumpOutputFormat::Human;
    let mut format_seen = false;
    let mut index = 3;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .ok_or_else(|| "dump options must be valid UTF-8".to_owned())?;
        let value = if argument == "--format" {
            index += 1;
            arguments
                .get(index)
                .and_then(|argument| argument.to_str())
                .ok_or_else(|| "`--format` requires `human` or `json`".to_owned())?
        } else if let Some(value) = argument.strip_prefix("--format=") {
            value
        } else {
            return Err(format!("unknown dump option `{argument}`"));
        };
        if format_seen {
            return Err("`--format` may appear only once".into());
        }
        format_seen = true;
        format = match value {
            "human" => DumpOutputFormat::Human,
            "json" => DumpOutputFormat::Json,
            _ => {
                return Err(format!(
                    "unknown dump format `{value}`; expected `human` or `json`"
                ));
            }
        };
        index += 1;
    }
    Ok(DumpInvocation { path, format })
}

fn run_dump_command(arguments: &[OsString]) -> Result<ExitCode, String> {
    let invocation = match parse_dump_invocation(arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let bytes = match fs::read(&invocation.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "tondo dump analyze: cannot read `{}`: {error}",
                invocation.path.display()
            );
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let analysis = match tondo_vm::runtime::analyze_dump(&bytes) {
        Ok(analysis) => analysis,
        Err(error) => {
            eprintln!("tondo dump analyze: {error}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    match invocation.format {
        DumpOutputFormat::Human => print!("{}", analysis.render_human()),
        DumpOutputFormat::Json => println!(
            "{}",
            analysis
                .to_json()
                .map_err(|error| format!("cannot render dump analysis: {error}"))?
        ),
    }
    Ok(ExitCode::SUCCESS)
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
    let result = execute_test_plan_at(&plan, location);
    if let Some(code) = test_interrupt::exit_code(false) {
        eprintln!(
            "tondo test: interrupted{}; complete outputs were not published",
            if code == 3 {
                " (worker isolation was lost)"
            } else {
                ""
            }
        );
        return Ok(ExitCode::from(code));
    }
    match result {
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
    base: PathBuf,
    project: ProjectPlan,
    production: Option<ProjectPlan>,
    documents: ProjectDocuments,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectDocuments {
    manifest: Vec<u8>,
    lockfile: Vec<u8>,
    production: Option<(Vec<u8>, Vec<u8>)>,
    test_dependencies: Option<Vec<u8>>,
    runtime_inputs: Vec<u8>,
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
                let production = discovered
                    .production
                    .as_ref()
                    .map(|(manifest, lock)| ProjectPlan::parse(manifest, lock))
                    .transpose()
                    .map_err(|error| TestCommandError::Usage(error.to_string()))?;
                Ok(LoadedProject {
                    base: discovered.root.clone(),
                    project,
                    production,
                    documents: ProjectDocuments {
                        manifest: discovered.manifest_bytes,
                        lockfile: discovered.lockfile_bytes,
                        production: discovered.production,
                        test_dependencies: discovered.test_dependencies,
                        runtime_inputs: discovered.runtime_inputs,
                    },
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
        match error {
            tondo_compiler::driver::DriverError::TestDependency(_)
            | tondo_compiler::driver::DriverError::Artifact(_) => {
                Self::Diagnostic(error.to_string())
            }
            _ => Self::Internal(error.to_string()),
        }
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
    production: Option<&ProjectPlan>,
    path: Option<&Path>,
    test_dependencies: Option<&[u8]>,
) -> Result<TestProjectPlan, TestCommandError> {
    let Some(path) = path else {
        let plan = TestProjectPlan::for_discovered_project(production, project, 1)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        return tondo_compiler::test_dependencies::TestDependencySources::default_test_plan(
            plan,
            test_dependencies,
        )
        .map_err(TestCommandError::Diagnostic);
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
    let plan = match production {
        Some(production) => TestProjectPlan::parse(production, &bytes),
        None => TestProjectPlan::parse_test_only(project, &bytes),
    }
    .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    // A sidecar describes the discovered target. Its source metadata must not
    // silently rename, reclassify or replace any of the files about to be read.
    let actual = TestProjectPlan::for_discovered_project(production, project, plan.policy().jobs())
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    let config = tondo_compiler::test_discovery::DiscoveryConfig::from_plan(&actual)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    let entries = actual
        .sources()
        .iter()
        .map(|source| {
            tondo_compiler::test_discovery::DiscoveryEntry::new(
                source.physical_path(),
                source.logical_path(),
                source.module(),
            )
        })
        .collect();
    let discovered = tondo_compiler::test_discovery::discover(&config, entries)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    tondo_compiler::test_discovery::reconcile_plan(&plan, &discovered)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    for source in plan.sources() {
        if !actual.sources().iter().any(|candidate| {
            candidate.physical_path() == source.physical_path()
                && candidate.package() == source.package()
        }) {
            return Err(TestCommandError::Usage(format!(
                "test plan changed the discovered package for `{}`",
                source.physical_path()
            )));
        }
    }
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
    package: &str,
    test_plan: &TestProjectPlan,
    update: bool,
) -> Result<SnapshotInputs, TestCommandError> {
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

fn validate_test_compilation(
    request: CompilationRequest,
    format: DiagnosticFormat,
) -> Result<Option<tondo_compiler::artifact::CompiledInterface>, TestCommandError> {
    let checked = execute(request)?;
    validate_test_output(&checked, format)?;
    Ok(checked.interface().cloned())
}

fn validate_test_output(
    checked: &tondo_compiler::driver::CompilationOutput,
    format: DiagnosticFormat,
) -> Result<(), TestCommandError> {
    if checked.status() != CompilationStatus::Success {
        let diagnostics = match format {
            DiagnosticFormat::Human => checked.diagnostics().human(),
            DiagnosticFormat::Json => checked
                .diagnostics()
                .json_lines()
                .map_err(|error| TestCommandError::Internal(error.to_string()))?,
        };
        return Err(TestCommandError::Diagnostic(diagnostics));
    }
    Ok(())
}

/// Compile every test body before selection can omit one. Compilation must not
/// enter a suite, execute a leaf, or consume the invocation's runtime budget.
fn compile_test_target(
    request: &CompilationRequest,
    entries: &[tondo_compiler::test_backend::TestEntry],
    plan: &TestProjectPlan,
    format: DiagnosticFormat,
) -> Result<(), TestCommandError> {
    let mut by_file = BTreeMap::new();
    for entry in entries {
        by_file
            .entry(entry.file())
            .or_insert_with(Vec::new)
            .push(entry.clone());
    }
    for entries in by_file.into_values() {
        let limits = plan.limits();
        let participation = tondo_compiler::test_backend::TestParticipation::new(
            EnvelopeLimits::new(
                limits.output_bytes(),
                limits.artifact_bytes(),
                limits.snapshot_bytes(),
            ),
            BTreeMap::new(),
            false,
        );
        let compilation = request.for_test_participation(&entries, participation)?;
        let compiled = tondo_compiler::driver::compile(compilation)?;
        validate_test_output(&compiled, format)?;
        if compiled.bytecode().is_none() {
            return Err(TestCommandError::Internal(
                "successful test compilation omitted verified bytecode".into(),
            ));
        }
    }
    Ok(())
}

fn prepare_test_request(
    project: &ProjectPlan,
    production: Option<&ProjectPlan>,
    supplied: &BTreeMap<String, Arc<[u8]>>,
    plan: &TestProjectPlan,
    format: DiagnosticFormat,
) -> Result<CompilationRequest, TestCommandError> {
    // Production and test compilation consume the same pinned source bytes.
    let production_output = if let Some(production) = production {
        let inputs = production
            .required_inputs()
            .map(|input| {
                supplied
                    .get(input.path())
                    .cloned()
                    .map(|bytes| (input.path().to_owned(), bytes))
                    .ok_or_else(|| {
                        TestCommandError::Internal(format!(
                            "test graph omitted production input `{}`",
                            input.path()
                        ))
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let request = production
            .resolve(&inputs)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?
            .into_compilation_request(Operation::Check, format, ResourceLimits::default())
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        let checked = execute(request)?;
        validate_test_output(&checked, format)?;
        Some(checked)
    } else {
        None
    };
    let test_limits = ResourceLimits {
        max_vm_steps: plan.limits().instructions(),
        max_vm_heap_bytes: plan.limits().memory_bytes(),
        ..ResourceLimits::default()
    };
    let project_inputs = project
        .required_inputs()
        .filter_map(|input| {
            supplied
                .get(input.path())
                .cloned()
                .map(|bytes| (input.path().to_owned(), bytes))
        })
        .collect();
    let mut request = project
        .resolve(&project_inputs)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?
        .into_compilation_request(Operation::Check, format, test_limits)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?
        .with_test_project_plan(plan)?;
    let mut interfaces = request.build_inputs().dependency_interfaces().clone();
    if let Some(production) = production_output {
        let interface = production.interface().cloned().ok_or_else(|| {
            TestCommandError::Internal(
                "successful production compilation omitted its interface".into(),
            )
        })?;
        let package = tondo_compiler::package::PackageId::new(interface.package_id())
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        interfaces.insert(package, interface);
        request = request.with_production_compilation(production)?;
    }
    let inputs = request
        .build_inputs()
        .clone()
        .with_dependency_interfaces(interfaces, true);
    Ok(request.with_declared_build_inputs(inputs))
}

fn execute_test_plan_at(
    plan: &test_cli::TestCliPlan,
    location: ProjectLocation,
) -> Result<u8, TestCommandError> {
    let loaded = location.load()?;
    let base = loaded.base.as_path();
    let project = &loaded.project;
    let test_plan_path = resolve_test_plan_path(plan, base)?;
    let test_project_plan = load_test_project_plan(
        project,
        loaded.production.as_ref(),
        test_plan_path.as_deref(),
        loaded.documents.test_dependencies.as_deref(),
    )?;
    let mut execution_plan = plan.clone();
    overlay_test_project_plan(&mut execution_plan, &test_project_plan)?;
    let dependencies = tondo_compiler::test_dependencies::TestDependencySources::parse(
        loaded.production.as_ref().unwrap_or(project),
        &test_project_plan,
        loaded.documents.test_dependencies.as_deref(),
    )
    .map_err(TestCommandError::Diagnostic)?;
    let mut supplied = BTreeMap::new();
    for input in project
        .required_inputs()
        .chain(dependencies.required_inputs())
    {
        if supplied.contains_key(input.path()) {
            return Err(TestCommandError::Diagnostic(format!(
                "test dependency reuses project input `{}`",
                input.path()
            )));
        }
        let bytes = read_input(
            &base.join(input.path()),
            &format!("{} input `{}`", input.kind().as_str(), input.path()),
        )
        .map_err(TestCommandError::Diagnostic)?;
        supplied.insert(input.path().to_owned(), Arc::<[u8]>::from(bytes));
    }
    let request = Arc::new(
        prepare_test_request(
            &loaded.project,
            loaded.production.as_ref(),
            &supplied,
            &test_project_plan,
            execution_plan.diagnostic_format,
        )?
        .with_test_dependencies(&dependencies, &supplied)?,
    );
    for compilation in request.test_compilation_requests()? {
        validate_test_compilation(compilation, execution_plan.diagnostic_format)?;
    }
    let entries = discover_tests(&request)?;
    compile_test_target(
        &request,
        &entries,
        &test_project_plan,
        execution_plan.diagnostic_format,
    )?;
    let snapshot_inputs = load_snapshot_inputs(
        base,
        project.root_package_id(),
        &test_project_plan,
        execution_plan.update_snapshots,
    )?;
    let ownership = resolve_ownership(&execution_plan, base)?;
    let identity = TestInvocationIdentity::capture(
        &loaded.documents,
        &supplied,
        &test_project_plan,
        &execution_plan,
        &snapshot_inputs,
        &ownership,
    )?;
    let runtime_inputs = test_inputs::CapturedInputs::capture(
        &identity.inputs,
        &snapshot_inputs,
        test_project_plan.limits().memory_bytes(),
    )
    .map_err(TestCommandError::Diagnostic)?;
    let (matched, selected) = select_test_entries(entries, &execution_plan)?;
    // A selector with no matches needs --allow-empty. A shard may receive no
    // leaves from a nonempty selection and must still emit its ordinary report.
    if matched == 0 && !execution_plan.allow_empty {
        return Err(TestCommandError::Diagnostic(
            "tondo: no tests matched the selection".into(),
        ));
    }
    let ordered = order_test_entries(selected, &execution_plan)?;
    if execution_plan.list {
        let list = build_test_list(
            &request,
            &execution_plan,
            &ordered,
            &ownership,
            &snapshot_inputs,
            &identity,
        )?;
        if execution_plan.test_format == test_cli::TestFormat::Json {
            let bytes = list
                .canonical_bytes()
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            print!(
                "{}",
                String::from_utf8(bytes)
                    .map_err(|error| { TestCommandError::Internal(error.to_string()) })?
            );
        } else {
            test_human::write_list(&list, &mut io::stdout().lock())
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        }
        return Ok(0);
    }

    let process_root = test_processes::prepare(
        execution_plan.process_cgroup.as_deref(),
        request
            .capabilities()
            .iter()
            .any(|value| value.as_str() == "process"),
    )
    .map_err(|error| {
        TestCommandError::Internal(format!("process isolation unavailable: {error}"))
    })?;
    let worker_timeout = execution_plan.timeout_ms;
    let worker_update_snapshots = execution_plan.update_snapshots;
    let diagnostic_run_id = diagnostic_run_id(&request, &execution_plan, &ordered, &identity);
    let diagnostic_source_revision = diagnostic_source_revision(&request);
    let diagnostic_shard = shard_identity(&execution_plan);
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
    let entries_by_id = ordered
        .iter()
        .map(|entry| (entry.id(), entry))
        .collect::<BTreeMap<_, _>>();
    let worker_groups = grouped_entries
        .into_iter()
        .map(|(key, entries)| {
            let has_suites = entries.iter().any(|id| id != &key.1);
            let participation = entries
                .iter()
                .map(|id| (*entries_by_id[id.as_str()]).clone())
                .collect::<Vec<_>>();
            let input = Arc::new(
                WorkerInput::capture(
                    &request,
                    &test_project_plan,
                    &snapshot_inputs,
                    &participation,
                    &runtime_inputs,
                )?
                .encode()
                .map_err(TestCommandError::Internal)?,
            );
            Ok((
                key,
                Arc::new(SharedWorkerGroup {
                    project_root: base.to_owned(),
                    process_root: process_root.clone(),
                    temporary_filesystem: request
                        .capabilities()
                        .iter()
                        .any(|value| value.as_str() == "filesystem"),
                    input,
                    entries,
                    timeout_ms: worker_timeout,
                    phase_limits: test_deadline::Limits {
                        body: worker_timeout.unwrap_or(test_project_plan.limits().timeout_ms()),
                        setup: test_project_plan.limits().setup_timeout_ms().min(
                            if execution_plan.timeout_explicit {
                                worker_timeout.unwrap_or(u64::MAX)
                            } else {
                                u64::MAX
                            },
                        ),
                        teardown: test_project_plan.limits().teardown_timeout_ms().min(
                            if execution_plan.timeout_explicit {
                                worker_timeout.unwrap_or(u64::MAX)
                            } else {
                                u64::MAX
                            },
                        ),
                    },
                    update_snapshots: worker_update_snapshots,
                    has_suites,
                    diagnostics: execution_plan.diagnostics.clone(),
                    run_id: diagnostic_run_id.clone(),
                    source_revision: diagnostic_source_revision.clone(),
                    shard: diagnostic_shard.clone(),
                    invocations: Mutex::new(BTreeMap::new()),
                }),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, TestCommandError>>()?;
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
    .map_err(|error| TestCommandError::Internal(error.to_string()))?
    .with_interruption(test_interrupt::token());
    let mut initial_plan = execution_plan.clone();
    initial_plan.retry = 0;
    let mut attempts = execute_campaign(
        &request,
        &initial_plan,
        &ordered,
        programs,
        runtime,
        &identity,
        &snapshot_inputs,
    )?;
    check_worker_inputs(&worker_groups)?;
    attach_worker_diagnostics(&worker_groups, &mut attempts)?;
    let mut suite_attempts = collect_suite_attempts(&worker_groups, &initial_plan)?;
    let retry_rounds = test_campaign::retry_units(
        &execution_plan,
        &ordered,
        &worker_groups,
        &mut attempts,
        &mut suite_attempts,
        &request,
        &identity,
        &snapshot_inputs,
    )?;
    let mut node_attempts = attempts.clone();
    node_attempts.extend(suite_attempts.iter().map(|attempt| CliAttempt {
        id: attempt.id.clone(),
        iteration: attempt.iteration,
        round: attempt.round,
        unit: attempt.unit,
        invocation: attempt.invocation,
        status: attempt.status,
        report: attempt.report.clone(),
        error: attempt.error.clone(),
        snapshot_updates: attempt.snapshot_updates.clone(),
        diagnostics: attempt.diagnostics.clone(),
        diagnostic_artifacts: attempt.diagnostic_artifacts.clone(),
    }));
    if test_interrupt::requests() > 0 {
        return Ok(test_interrupt::exit_code(false).unwrap_or(4));
    }
    let mut output_paths = execution_plan
        .reports
        .iter()
        .map(|output| output.path.clone())
        .collect::<Vec<_>>();
    if execution_plan.update_snapshots {
        output_paths.extend(
            snapshot_inputs
                .stores
                .iter()
                .map(|store| base.join(&store.relative)),
        );
    }
    for (index, attempt) in node_attempts.iter().enumerate() {
        if let Some(store) = attempt_artifact_store(
            base,
            &execution_plan,
            Some(test_project_plan.artifact_store()),
            attempt,
            index,
        )? {
            output_paths.push(store.manifest_path());
        }
    }
    let mut output_transaction =
        test_outputs::OutputTransaction::capture(output_paths).map_err(|error| {
            TestCommandError::Internal(format!("cannot preserve final outputs: {error}"))
        })?;
    publish_attempt_artifacts(
        base,
        &execution_plan,
        Some(test_project_plan.artifact_store()),
        &node_attempts,
    )?;
    let snapshot_mutation = if ordered.is_empty() {
        SnapshotMutation {
            after_sha256: snapshot_inputs.before_sha256.clone(),
            published: false,
        }
    } else {
        snapshot_inputs.stage_and_publish(base, &execution_plan, &node_attempts)?
    };
    let report = build_test_report(
        &request,
        &execution_plan,
        &ordered,
        &ownership,
        &attempts,
        &suite_attempts,
        &snapshot_inputs,
        &snapshot_mutation,
        &identity,
        retry_rounds,
    )?;
    publish_test_outputs(&execution_plan, &report)?;
    if !test_interrupt::finish_publication() {
        if let Err(error) = output_transaction.rollback() {
            test_interrupt::isolation_lost();
            return Err(TestCommandError::Internal(format!(
                "cannot restore final outputs: {error}"
            )));
        }
        return Ok(test_interrupt::exit_code(false).unwrap_or(4));
    }
    output_transaction.commit().map_err(|error| {
        TestCommandError::Internal(format!("cannot remove output backups: {error}"))
    })?;
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
        test_human::write_report(
            &report,
            execution_plan.show_output,
            &mut io::stdout().lock(),
            &mut io::stderr().lock(),
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    }
    if let Some(exit) = diagnostic_exit_status(&report) {
        return Ok(exit);
    }
    let failed = report.summary().failed > 0
        || (plan.deny_skips && report.summary().skipped + report.summary().blocked_skip > 0);
    Ok(u8::from(failed))
}

fn diagnostic_exit_status(report: &TestReport) -> Option<u8> {
    let statuses = report
        .tests()
        .iter()
        .chain(report.suites())
        .flat_map(|node| node.attempts.iter())
        .flat_map(|attempt| attempt.diagnostics.iter())
        .map(|diagnostic| diagnostic.status);
    let mut unsupported = false;
    let mut finding = false;
    let mut failed = false;
    for status in statuses {
        match status {
            DiagnosticStatus::Unsupported => unsupported = true,
            DiagnosticStatus::Finding => finding = true,
            DiagnosticStatus::Failed => failed = true,
            DiagnosticStatus::Clean => {}
        }
    }
    if failed {
        Some(3)
    } else if unsupported {
        Some(2)
    } else if finding {
        Some(1)
    } else {
        None
    }
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
    error_type: Option<String>,
    source: Option<tondo_compiler::test_result::SourceSpan>,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerDiagnosticArtifact {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerDiagnostic {
    record: DiagnosticRecord,
    artifacts: Vec<WorkerDiagnosticArtifact>,
}

struct DiagnosticWorkerContext<'a> {
    profiles: &'a BTreeSet<DiagnosticProfile>,
    run_id: &'a str,
    source_revision: &'a str,
    shard: &'a str,
    invocation: u64,
}

struct DiagnosticReportContext<'a> {
    profiles: &'a BTreeSet<DiagnosticProfile>,
    trace: Option<&'a DiagnosticTrace>,
    run_id: &'a str,
    attempt_id: &'a str,
    shard: &'a str,
    target: &'a str,
    source_revision: &'a str,
    program_exit_status: i32,
    command_exit_status: i32,
    crashed: bool,
}

const WORKER_INPUT_FORMAT: &str = "tondo-test-worker-input/5";
// Includes verified code, constants and snapshot values. Reject excess before
// starting an attempt. This transport is private to the same CLI executable.
const MAX_WORKER_INPUT_BYTES: usize = 512 * 1024 * 1024;
// Recursive constants can exceed serde_json's default depth of 128 while
// remaining within the compiler's source nesting budget. Preflight the private
// frame iteratively before deserializing on the worker's explicit 8 MiB stack.
const MAX_WORKER_INPUT_DEPTH: usize = 1024;

fn check_worker_input_depth(bytes: &[u8], limit: usize) -> Result<(), String> {
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    if depth == limit {
                        return Err("closed worker input exceeds the nesting limit".into());
                    }
                    depth += 1;
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

struct WorkerBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for WorkerBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "worker JSON exceeds the process transport limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_bounded_worker_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let mut buffer = WorkerBuffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut buffer, value).map_err(|error| error.to_string())?;
    Ok(buffer.bytes)
}

fn read_worker_diagnostics(
    reader: impl Read,
    phases: Option<Arc<Mutex<test_deadline::Watchdog>>>,
) -> io::Result<Vec<u8>> {
    // First and second OS requests have different supervision semantics. Each
    // worker reports its own count; simultaneous delivery to several workers
    // is merged by maximum, not mistaken for repeated cancellation.
    use std::io::BufRead;
    let mut reader = io::BufReader::new(reader);
    let mut diagnostics = Vec::new();
    let mut count = 0;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            return Ok(diagnostics);
        }
        if line == test_interrupt::WORKER_REQUEST_FRAME {
            count += 1;
            if count > 2 {
                return Err(io::Error::other("too many worker interruption frames"));
            }
            test_interrupt::worker_requested_interruption(count);
            continue;
        }
        if let Some(payload) = line.strip_prefix(test_deadline::FRAME_PREFIX) {
            if line.len() > test_deadline::MAX_FRAME_BYTES {
                return Err(io::Error::other("worker phase frame exceeds its bound"));
            }
            let event = serde_json::from_slice(payload).map_err(io::Error::other)?;
            phases
                .as_ref()
                .ok_or_else(|| io::Error::other("unexpected worker phase frame"))?
                .lock()
                .map_err(|_| io::Error::other("worker watchdog lock is poisoned"))?
                .event(event, Instant::now())?;
        } else {
            if diagnostics.len() + line.len() > 1024 * 1024 {
                return Err(io::Error::other(
                    "worker diagnostics exceed the process transport limit",
                ));
            }
            diagnostics.extend(line);
        }
    }
}

fn read_bounded_worker_pipe(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::other(
            "worker pipe exceeds the process transport limit",
        ));
    }
    Ok(bytes)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerTestEntry {
    id: String,
    suites: Vec<String>,
}

impl WorkerTestEntry {
    fn id(&self) -> &str {
        &self.id
    }
    fn suites(&self) -> &[String] {
        &self.suites
    }
}

/// One coordinator-compiled participation. Workers receive no source graph
/// from which they could repeat parsing, resolution or lowering.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerInput {
    format: String,
    source_revision: String,
    target: String,
    temporary_filesystem: bool,
    process_isolation: bool,
    program: tondo_vm::bytecode::BytecodeProgram,
    entry: tondo_vm::bytecode::BytecodeFunctionId,
    limits: tondo_vm::runtime::VmLimits,
    output_bytes: u64,
    artifact_bytes: u64,
    snapshot_bytes: u64,
    virtual_timers: u64,
    entries: Vec<WorkerTestEntry>,
    expected: BTreeMap<String, BTreeMap<String, String>>,
    runtime_inputs: test_inputs::CapturedInputs,
    source_files: BTreeMap<u32, String>,
}

impl WorkerInput {
    fn capture(
        request: &CompilationRequest,
        plan: &TestProjectPlan,
        snapshots: &SnapshotInputs,
        entries: &[tondo_compiler::test_backend::TestEntry],
        runtime_inputs: &test_inputs::CapturedInputs,
    ) -> Result<Self, TestCommandError> {
        let limits = plan.limits();
        let participation = tondo_compiler::test_backend::TestParticipation::new(
            EnvelopeLimits::new(
                limits.output_bytes(),
                limits.artifact_bytes(),
                limits.snapshot_bytes(),
            ),
            BTreeMap::new(),
            false,
        );
        let participation_request = request.for_test_participation(entries, participation)?;
        let source_files = participation_request
            .sources()
            .iter()
            .map(|(file, source)| (file.index(), source.path().to_string()))
            .collect();
        let compiled = tondo_compiler::driver::compile(participation_request)?;
        validate_test_output(&compiled, request.diagnostic_format())?;
        let (program, entry) = compiled.into_compiled_program().ok_or_else(|| {
            TestCommandError::Internal(
                "successful participation omitted its compiled program".into(),
            )
        })?;
        Ok(Self {
            format: WORKER_INPUT_FORMAT.into(),
            source_revision: diagnostic_source_revision(request),
            target: request.target().name().to_owned(),
            temporary_filesystem: request
                .capabilities()
                .iter()
                .any(|value| value.as_str() == "filesystem"),
            process_isolation: request
                .capabilities()
                .iter()
                .any(|value| value.as_str() == "process"),
            program,
            entry,
            limits: request.runtime_limits(),
            output_bytes: limits.output_bytes(),
            artifact_bytes: limits.artifact_bytes(),
            snapshot_bytes: limits.snapshot_bytes(),
            virtual_timers: limits.virtual_timers(),
            entries: entries
                .iter()
                .map(|entry| WorkerTestEntry {
                    id: entry.id().to_owned(),
                    suites: entry.suites().to_vec(),
                })
                .collect(),
            expected: test_node_ids(entries)
                .into_iter()
                .map(|id| snapshots.expected_for(&id).map(|expected| (id, expected)))
                .collect::<Result<_, _>>()?,
            runtime_inputs: runtime_inputs.clone(),
            source_files,
        })
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        let bytes = encode_bounded_worker_json(self, MAX_WORKER_INPUT_BYTES)?;
        check_worker_input_depth(&bytes, MAX_WORKER_INPUT_DEPTH)?;
        Ok(bytes)
    }

    fn read(reader: impl Read, expected_hash: &str, limit: usize) -> Result<Self, String> {
        if expected_hash.is_empty() {
            return Err("closed worker input hash is required".into());
        }
        let mut bytes = Vec::new();
        reader
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read closed worker input: {error}"))?;
        if bytes.len() > limit {
            return Err("closed worker input exceeds the process transport limit".into());
        }
        if tondo_compiler::artifact::sha256(&bytes) != expected_hash {
            return Err("closed worker input hash mismatch".into());
        }
        check_worker_input_depth(&bytes, MAX_WORKER_INPUT_DEPTH)?;
        let mut decoder = serde_json::Deserializer::from_slice(&bytes);
        decoder.disable_recursion_limit();
        let input = Self::deserialize(&mut decoder)
            .map_err(|error| format!("invalid closed worker input: {error}"))?;
        decoder
            .end()
            .map_err(|error| format!("invalid closed worker input: {error}"))?;
        if input.format != WORKER_INPUT_FORMAT {
            return Err(format!(
                "unsupported closed worker input format `{}`",
                input.format
            ));
        }
        input.limits.validate().map_err(|error| error.to_string())?;
        input
            .envelope_limits()
            .profile()
            .validate()
            .map_err(|error| error.to_string())?;
        Ok(input)
    }

    fn envelope_limits(&self) -> EnvelopeLimits {
        EnvelopeLimits::new(self.output_bytes, self.artifact_bytes, self.snapshot_bytes)
            .with_execution_limits(self.limits.max_steps, self.virtual_timers)
    }
}

fn test_node_ids(entries: &[tondo_compiler::test_backend::TestEntry]) -> BTreeSet<String> {
    let mut ids = entries
        .iter()
        .map(|entry| entry.id().to_owned())
        .collect::<BTreeSet<_>>();
    ids.extend(entries.iter().flat_map(|entry| {
        (1..=entry.suites().len()).map(move |depth| {
            let mut parts = entry.id().split("::").collect::<Vec<_>>();
            parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
            parts.join("::")
        })
    }));
    ids
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResponse {
    format: String,
    status: String,
    report: Vec<u8>,
    updates: Vec<WorkerSnapshotUpdate>,
    error: Option<WorkerError>,
    diagnostics: Vec<WorkerDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBatchResponse {
    format: String,
    responses: Vec<(String, WorkerResponse)>,
    suites: Vec<WorkerSuiteResponse>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerInterruptResponse {
    format: String,
    cleanup_complete: bool,
}

fn worker_interrupt_response(code: u8) -> Result<ExitCode, String> {
    let bytes = serde_json::to_vec(&WorkerInterruptResponse {
        format: "tondo-test-worker-interrupt/1".into(),
        cleanup_complete: code == 4,
    })
    .map_err(|error| error.to_string())?;
    io::stdout()
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    Ok(ExitCode::from(code))
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
    diagnostics: Vec<WorkerDiagnostic>,
}

#[derive(Debug, Clone)]
struct WorkerGroupResult {
    leaves: BTreeMap<String, WorkerResponse>,
    suites: Vec<WorkerSuiteResponse>,
}

type WorkerInvocation = Arc<std::sync::OnceLock<Result<WorkerGroupResult, RunError>>>;

// Diagnostic payloads extend the child-process wire shape; keep the protocol
// version explicit so a stale worker cannot be mistaken for a complete one.
const WORKER_RESPONSE_FORMAT: &str = "tondo-test-worker-process/2";
const WORKER_BATCH_RESPONSE_FORMAT: &str = "tondo-test-worker-batch/2";

struct SharedWorkerGroup {
    project_root: PathBuf,
    process_root: Option<test_processes::ProcessRoot>,
    temporary_filesystem: bool,
    input: Arc<Vec<u8>>,
    entries: Vec<String>,
    timeout_ms: Option<u64>,
    phase_limits: test_deadline::Limits,
    update_snapshots: bool,
    has_suites: bool,
    diagnostics: BTreeSet<DiagnosticProfile>,
    run_id: String,
    source_revision: String,
    shard: String,
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
            let diagnostic_context = DiagnosticWorkerContext {
                profiles: &self.diagnostics,
                run_id: &self.run_id,
                source_revision: &self.source_revision,
                shard: &self.shard,
                invocation,
            };
            spawn_test_worker(
                &self.project_root,
                self.process_root.as_ref(),
                self.temporary_filesystem,
                self.input.clone(),
                &self.entries,
                self.timeout_ms,
                self.phase_limits,
                self.update_snapshots,
                &diagnostic_context,
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

fn check_worker_inputs(
    groups: &BTreeMap<(u32, String), Arc<SharedWorkerGroup>>,
) -> Result<(), TestCommandError> {
    for group in groups.values() {
        let invocations = group
            .invocations
            .lock()
            .map_err(|_| TestCommandError::Internal("worker cache is poisoned".into()))?;
        for result in invocations
            .values()
            .filter_map(|slot| slot.get().and_then(|result| result.as_ref().ok()))
        {
            check_worker_group_inputs(result)?;
        }
    }
    Ok(())
}

fn check_worker_group_inputs(result: &WorkerGroupResult) -> Result<(), TestCommandError> {
    if let Some(error) = result
        .leaves
        .values()
        .filter_map(|response| response.error.as_ref())
        .find(|error| error.kind == "cleanup")
    {
        return Err(TestCommandError::Internal(error.message.clone()));
    }
    if let Some(error) = result
        .leaves
        .values()
        .filter_map(|response| response.error.as_ref())
        .find(|error| error.kind == "input")
    {
        return Err(TestCommandError::Diagnostic(error.message.clone()));
    }
    Ok(())
}

impl WorkerError {
    fn from_execution(
        execution: &tondo_compiler::test_backend::TestNodeExecution,
        source_files: &BTreeMap<u32, String>,
    ) -> Option<Self> {
        if execution.timed_out {
            return Some(Self {
                kind: "timeout".into(),
                code: None,
                error_type: None,
                source: None,
                message: "test phase exceeded its wall-clock deadline".into(),
            });
        }
        if let Some(panic) = execution
            .panic
            .as_ref()
            .and_then(tondo_vm::runtime::VmPanic::language_panic)
        {
            return Some(Self {
                kind: "panic".into(),
                code: Some(panic.code.code().into()),
                error_type: None,
                source: None,
                message: panic.message.clone(),
            });
        }
        if execution.report.terminal().is_some() {
            return None;
        }
        execution.error_type.as_ref().map(|error_type| Self {
            kind: "error".into(),
            code: None,
            error_type: Some(error_type.clone()),
            source: execution.error_span.and_then(|span| {
                (span.start < span.end).then_some(())?;
                Some(tondo_compiler::test_result::SourceSpan {
                    file: source_files.get(&span.file)?.clone(),
                    start: u64::from(span.start),
                    end: u64::from(span.end),
                })
            }),
            message: format!("unhandled test error: `{error_type}`"),
        })
    }

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
            error_type: error.error_type().map(str::to_owned),
            source: error.source().cloned(),
            message: error.to_string(),
        }
    }

    fn into_run_error(self) -> Result<(), RunError> {
        if self.kind == "error" {
            return Err(match self.error_type.filter(|name| !name.is_empty()) {
                Some(error_type) => RunError::Error {
                    code: self.code,
                    error_type,
                    message: self.message,
                    source: self.source,
                },
                None => RunError::Infrastructure {
                    message: "recoverable worker error omitted its type identity".into(),
                },
            });
        }
        let code = self.code.unwrap_or_else(|| "T3001".into());
        let message = self.message;
        Err(match self.kind.as_str() {
            "panic" => RunError::Panic { code, message },
            "resource-limit" => RunError::ResourceLimit { kind: code },
            "timeout" => RunError::Timeout,
            "skip" => RunError::Skip { reason: message },
            "infrastructure" | "input" | "cleanup" => RunError::Infrastructure { message },
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
            source: None,
            error_type: None,
            kind: "infrastructure".into(),
            code: None,
            message: error.into(),
        }),
        diagnostics: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_test_worker(
    project_root: &Path,
    process_root: Option<&test_processes::ProcessRoot>,
    temporary_filesystem: bool,
    input: Arc<Vec<u8>>,
    entries: &[String],
    timeout_ms: Option<u64>,
    phase_limits: test_deadline::Limits,
    update_snapshots: bool,
    diagnostic_context: &DiagnosticWorkerContext<'_>,
) -> Result<WorkerGroupResult, RunError> {
    let mut processes = process_root
        .map(test_processes::ProcessRoot::create)
        .transpose()
        .map_err(|error| {
            isolation_failure(format!(
                "cannot create isolated worker process group: {error}"
            ))
        })?;
    let mut root = temporary_filesystem
        .then(|| tondo_compiler::test_temporaries::TemporaryRoot::create(project_root))
        .transpose()
        .map_err(|error| {
            isolation_failure(format!(
                "cannot create isolated worker temporary root: {error}"
            ))
        })?;
    let result = spawn_test_worker_with_root(
        project_root,
        root.as_ref().map(|root| root.path()),
        processes.as_mut(),
        input,
        entries,
        timeout_ms,
        phase_limits,
        update_snapshots,
        diagnostic_context,
    );
    // Also close an empty group when executable lookup, spawn or admission
    // failed. The inner wait closes a running group before joining pipes.
    let process_cleanup = processes
        .as_mut()
        .map(test_processes::ProcessGroup::close)
        .transpose();
    if let Err(error) = process_cleanup {
        if let Some(root) = &mut root {
            root.retain_after_failed_isolation();
        }
        return Err(isolation_failure(format!(
            "isolated worker process cleanup failed: {error}; temporary root retained: {:?}",
            root.as_ref().map(|root| root.path())
        )));
    }
    // The inner call reaps the worker on every return path. Cleanup precedes
    // any result, snapshot or artifact publication, including forced exit.
    if let Some(root) = &mut root
        && let Err(error) = root.cleanup()
    {
        let message = format!("isolated worker temporary cleanup failed: {error}");
        test_interrupt::abort_isolation(&message);
        return Ok(WorkerGroupResult {
            leaves: entries
                .iter()
                .map(|id| {
                    let mut response = infrastructure_worker_response(message.clone());
                    response.error.as_mut().unwrap().kind = "cleanup".into();
                    (id.clone(), response)
                })
                .collect(),
            suites: Vec::new(),
        });
    }
    result
}

fn isolation_failure(message: String) -> RunError {
    test_interrupt::abort_isolation(&message);
    RunError::Infrastructure { message }
}

#[allow(clippy::too_many_arguments)]
fn spawn_test_worker_with_root(
    project_root: &Path,
    temporary_root: Option<&Path>,
    mut processes: Option<&mut test_processes::ProcessGroup>,
    input: Arc<Vec<u8>>,
    entries: &[String],
    timeout_ms: Option<u64>,
    phase_limits: test_deadline::Limits,
    update_snapshots: bool,
    diagnostic_context: &DiagnosticWorkerContext<'_>,
) -> Result<WorkerGroupResult, RunError> {
    if test_interrupt::requests() > 0 {
        return Err(RunError::Infrastructure {
            message: "test invocation interrupted before dispatch".into(),
        });
    }
    let mut command =
        Command::new(
            worker_executable().map_err(|error| RunError::Infrastructure {
                message: format!("cannot locate tondo worker executable: {error}"),
            })?,
        );
    command
        .current_dir(project_root)
        .arg("__test-worker")
        .arg("--input-bytes")
        .arg(input.len().to_string())
        .arg("--input-sha256")
        .arg(tondo_compiler::artifact::sha256(&input));
    command
        .arg("--source-revision")
        .arg(diagnostic_context.source_revision);
    if let Some(root) = temporary_root {
        command.arg("--temporary-root").arg(root);
    }
    if let Some(group) = &processes {
        command.arg("--process-cgroup").arg(group.path());
    }
    for entry in entries {
        command.arg("--entry").arg(entry);
    }
    if update_snapshots {
        command.arg("--update-snapshots");
    }
    if !diagnostic_context.profiles.is_empty() {
        command.arg("--diagnostics").arg(
            diagnostic_context
                .profiles
                .iter()
                .map(|profile| profile.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        command.arg("--run-id").arg(diagnostic_context.run_id);
        command.arg("--shard").arg(diagnostic_context.shard);
        command
            .arg("--invocation")
            .arg(diagnostic_context.invocation.to_string());
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| RunError::Infrastructure {
            message: format!("cannot spawn isolated test worker: {error}"),
        })?;
    // The worker is blocked on sealed stdin. No Tondo instruction can execute
    // until its whole OS thread group has entered the provider's fresh cgroup.
    if let Some(group) = &mut processes
        && let Err(error) = group.admit(child.id())
    {
        let reaped = reap_terminated_worker(&mut child);
        return Err(isolation_failure(format!(
            "cannot admit isolated test worker: {error}; reap={reaped:?}"
        )));
    }
    let mut stdin = child
        .stdin
        .take()
        .expect("worker stdin was explicitly piped");
    // Feed the input concurrently with output draining and the wall-clock
    // deadline; a pipe-sized input must not block timeout enforcement.
    let (control, requests) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&input)?;
        while let Ok(request) = requests.recv() {
            stdin.write_all(&request)?;
        }
        Ok::<(), io::Error>(())
    });
    let phases = Arc::new(Mutex::new(test_deadline::Watchdog::new(phase_limits)));
    let result = wait_worker_controlled(
        child,
        timeout_ms,
        Some(&control),
        Some(phases.clone()),
        processes,
    );
    drop(control);
    let (status, stdout, stderr) = result?;
    writer
        .join()
        .map_err(|_| RunError::Infrastructure {
            message: "closed worker input writer panicked".into(),
        })?
        .map_err(|error| RunError::Infrastructure {
            message: format!("cannot send closed worker input: {error}"),
        })?;
    if stdout.is_empty() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(RunError::Infrastructure {
            message: format!(
                "isolated test worker exited with {status} without a response: {}",
                detail.trim()
            ),
        });
    }
    let mut response: WorkerBatchResponse =
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
    for (id, phase) in &phases
        .lock()
        .map_err(|_| RunError::Infrastructure {
            message: "worker watchdog lock is poisoned".into(),
        })?
        .expired
    {
        let error = || {
            Some(WorkerError {
                source: None,
                error_type: None,
                kind: "timeout".into(),
                code: None,
                message: "test phase exceeded its wall-clock deadline".into(),
            })
        };
        if let Some((_, leaf)) = response.responses.iter_mut().find(|(leaf, _)| leaf == id) {
            leaf.status = "timeout".into();
            leaf.error = error();
        }
        if let Some(suite) = response.suites.iter_mut().find(|suite| &suite.id == id) {
            suite.status = "timeout".into();
            suite.phase = Some(
                if *phase == test_deadline::Phase::Teardown {
                    "teardown"
                } else {
                    "setup"
                }
                .into(),
            );
            suite.error = error();
        }
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

#[cfg(test)]
fn wait_worker(
    child: Child,
    timeout_ms: Option<u64>,
) -> Result<(String, Vec<u8>, Vec<u8>), RunError> {
    wait_worker_controlled(child, timeout_ms, None, None, None)
}

fn wait_worker_controlled(
    mut child: Child,
    timeout_ms: Option<u64>,
    control: Option<&std::sync::mpsc::Sender<Vec<u8>>>,
    phases: Option<Arc<Mutex<test_deadline::Watchdog>>>,
    processes: Option<&mut test_processes::ProcessGroup>,
) -> Result<(String, Vec<u8>, Vec<u8>), RunError> {
    // Drain both pipes while the worker is running. Waiting for process exit
    // before reading would deadlock a valid worker whose bounded report is
    // larger than the host pipe buffer.
    let stdout_reader = child.stdout.take().map(|pipe| {
        std::thread::spawn(move || read_bounded_worker_pipe(pipe, MAX_WORKER_INPUT_BYTES))
    });
    let phase_reader = phases.clone();
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_worker_diagnostics(pipe, phase_reader)));
    let started = Instant::now();
    let mut cancellation = test_interrupt::WorkerCancellation::new();
    let outcome = loop {
        let requests = test_interrupt::requests();
        let (first_request, forced) = cancellation.poll(requests);
        if first_request && let Some(control) = control {
            let _ = control.send(vec![b'C']);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if forced {
                    test_interrupt::isolation_lost();
                    break Err(RunError::Infrastructure {
                        message: "interrupted worker exceeded cleanup grace".into(),
                    });
                }
                if !cancellation.requested()
                    && let Some(phases) = &phases
                {
                    let mut phases = match phases.lock() {
                        Ok(phases) => phases,
                        Err(_) => {
                            break Err(RunError::Infrastructure {
                                message: "worker watchdog lock is poisoned".into(),
                            });
                        }
                    };
                    let (expired, stuck) = phases.poll(Instant::now());
                    if let Some(sequence) = expired
                        && let Some(control) = control
                    {
                        let mut frame = vec![b'T'];
                        frame.extend_from_slice(&sequence.to_le_bytes());
                        let _ = control.send(frame);
                    }
                    if stuck || phases.idle_expired(Instant::now()) {
                        drop(phases);
                        break Err(RunError::Infrastructure {
                            message: "test phase worker did not complete isolated cleanup".into(),
                        });
                    }
                }
                if phases.is_none()
                    && !cancellation.requested()
                    && timeout_ms
                        .is_some_and(|limit| started.elapsed() >= Duration::from_millis(limit))
                {
                    break Err(RunError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => {
                break Err(RunError::Infrastructure {
                    message: format!("cannot poll isolated test worker: {error}"),
                });
            }
        }
    };
    // A successfully reaped leader can still have descendants holding these
    // pipes. Terminate the entire group before joining either pipe reader.
    let contained = processes
        .map(test_processes::ProcessGroup::close)
        .transpose();
    let reaped = if outcome.is_err() {
        reap_terminated_worker(&mut child)
    } else {
        Ok(())
    };
    contained.map_err(|error| {
        test_interrupt::isolation_lost();
        RunError::Infrastructure {
            message: format!("isolated worker process cleanup failed: {error}"),
        }
    })?;
    reaped?;
    let stdout = join_worker_pipe(stdout_reader, "output")?;
    let stderr = join_worker_pipe(stderr_reader, "diagnostics")?;
    let status = outcome?;
    let worker_interrupt = serde_json::from_slice::<WorkerInterruptResponse>(&stdout)
        .ok()
        .filter(|response| response.format == "tondo-test-worker-interrupt/1");
    let requests = test_interrupt::requests();
    let (_, forced) = cancellation.poll(requests);
    if worker_interrupt.is_some() || status.code() == Some(4) || cancellation.requested() {
        if requests == 0 {
            test_interrupt::worker_requested_interruption(1);
            cancellation.poll(1);
        }
        if forced
            || worker_interrupt.is_some_and(|response| !response.cleanup_complete)
            || !matches!(status.code(), Some(0 | 4))
        {
            test_interrupt::isolation_lost();
            eprintln!(
                "tondo test: interrupted worker cleanup failed: {}",
                String::from_utf8_lossy(&stderr).trim()
            );
        } else {
            cancellation.close();
        }
        return Err(RunError::Infrastructure {
            message: "isolated worker interrupted".into(),
        });
    }
    Ok((status.to_string(), stdout, stderr))
}

fn reap_terminated_worker(child: &mut Child) -> Result<(), RunError> {
    let killed = child.kill();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            outcome => {
                test_interrupt::isolation_lost();
                return Err(RunError::Infrastructure {
                    message: format!(
                        "cannot reap isolated worker: kill={killed:?}, reap={outcome:?}"
                    ),
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
fn run_test_worker_on_explicit_stack(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    let worker = std::thread::Builder::new()
        .name("tondo-test-worker".into())
        .stack_size(CLI_STACK_SIZE)
        .spawn(move || run_test_worker(&arguments))
        .map_err(|error| format!("cannot create isolated worker stack: {error}"))?;
    worker
        .join()
        .map_err(|_| "isolated test worker stack panicked".to_owned())?
}

fn run_test_worker(arguments: &[OsString]) -> Result<ExitCode, String> {
    let mut temporary_root = None;
    let mut process_cgroup = None;
    let mut input_bytes = None;
    let mut input_hash = String::new();
    let mut entries = Vec::new();
    let mut update_snapshots = false;
    let mut diagnostic_profiles = BTreeSet::new();
    let mut run_id = String::new();
    let mut source_revision = String::new();
    let mut shard = "all".to_owned();
    let mut invocation = 0_u64;
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments[index]
            .to_str()
            .ok_or_else(|| "hidden test-worker arguments must be UTF-8".to_owned())?;
        match value {
            "--process-cgroup" => {
                index += 1;
                let path = PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or_else(|| "worker `--process-cgroup` requires a path".to_owned())?,
                );
                if !path.is_absolute() || process_cgroup.is_some() {
                    return Err("worker process cgroup must be one absolute path".into());
                }
                process_cgroup = Some(path);
            }
            "--temporary-root" => {
                index += 1;
                let path = PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or_else(|| "worker `--temporary-root` requires a path".to_owned())?,
                );
                if !path.is_absolute() || temporary_root.is_some() {
                    return Err("worker temporary root must be one absolute path".into());
                }
                temporary_root = Some(path);
            }
            "--input-bytes" => {
                index += 1;
                input_bytes = Some(
                    arguments
                        .get(index)
                        .and_then(|value| value.to_str())
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|value| *value <= MAX_WORKER_INPUT_BYTES)
                        .ok_or_else(|| {
                            "worker `--input-bytes` requires a bounded length".to_owned()
                        })?,
                );
            }
            "--input-sha256" => {
                index += 1;
                input_hash = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--input-sha256` requires a hash".to_owned())?
                    .to_owned();
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
            "--update-snapshots" => update_snapshots = true,
            "--diagnostics" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--diagnostics` requires profiles".to_owned())?;
                diagnostic_profiles = test_cli::parse_diagnostics(value)?;
            }
            "--run-id" => {
                index += 1;
                run_id = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--run-id` requires a value".to_owned())?
                    .to_owned();
            }
            "--source-revision" => {
                index += 1;
                source_revision = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--source-revision` requires a value".to_owned())?
                    .to_owned();
            }
            "--shard" => {
                index += 1;
                shard = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--shard` requires a value".to_owned())?
                    .to_owned();
            }
            "--invocation" => {
                index += 1;
                invocation = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "worker `--invocation` requires a value".to_owned())?
                    .parse()
                    .map_err(|_| "worker `--invocation` must be a u64".to_owned())?;
            }
            other => return Err(format!("unknown hidden worker option `{other}`")),
        }
        index += 1;
    }
    if entries.is_empty() {
        return Err("at least one worker entry is required".to_owned());
    }
    let input = if input_hash.is_empty() {
        Err("closed worker input hash is required".to_owned())
    } else {
        input_bytes
            .ok_or_else(|| "worker `--input-bytes` is required".to_owned())
            .and_then(|input_bytes| {
                WorkerInput::read(
                    io::stdin().lock().take(input_bytes as u64),
                    &input_hash,
                    MAX_WORKER_INPUT_BYTES,
                )
            })
    };
    // The remaining stdin byte is a cancellation request, separate from the
    // immutable hashed payload. EOF means the coordinator closed the session.
    let phases = Arc::new(test_deadline::WorkerPhases::default());
    if input.is_ok() {
        let requested = phases.requested.clone();
        std::thread::spawn(move || {
            let mut input = io::stdin().lock();
            loop {
                let mut request = [0];
                if input.read_exact(&mut request).is_err() || request[0] != b'T' {
                    test_interrupt::supervisor_requested_interruption();
                    break;
                }
                let mut sequence = [0; 8];
                if input.read_exact(&mut sequence).is_err() {
                    test_interrupt::supervisor_requested_interruption();
                    break;
                }
                requested.store(
                    u64::from_le_bytes(sequence),
                    std::sync::atomic::Ordering::Release,
                );
            }
        });
    }
    if test_interrupt::requests() > 0 {
        return worker_interrupt_response(4);
    }
    let result = match input.and_then(|input| {
        execute_test_worker(
            input,
            temporary_root,
            process_cgroup,
            &entries,
            update_snapshots,
            phases,
            &DiagnosticWorkerContext {
                profiles: &diagnostic_profiles,
                run_id: &run_id,
                source_revision: &source_revision,
                shard: &shard,
                invocation,
            },
        )
    }) {
        Ok(responses) => responses,
        Err(error) => WorkerGroupResult {
            leaves: entries
                .iter()
                .map(|entry| (entry.clone(), infrastructure_worker_response(error.clone())))
                .collect(),
            suites: Vec::new(),
        },
    };
    if let Some(code) = test_interrupt::exit_code(true) {
        if code == 3
            && let Some(error) = result
                .leaves
                .values()
                .find_map(|response| response.error.as_ref())
        {
            eprintln!("tondo test: partial worker cleanup: {}", error.message);
        }
        return worker_interrupt_response(code);
    }
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
    // Reserve the final newline inside the response frame's byte budget.
    let bytes = encode_bounded_worker_json(&response, MAX_WORKER_INPUT_BYTES - 1)?;
    io::stdout()
        .write_all(&bytes)
        .map_err(|error| format!("cannot write worker response: {error}"))?;
    io::stdout()
        .write_all(b"\n")
        .map_err(|error| format!("cannot finish worker response: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

fn execute_test_worker(
    input: WorkerInput,
    temporary_root: Option<PathBuf>,
    process_cgroup: Option<PathBuf>,
    entry_ids: &[String],
    update_snapshots: bool,
    phases: Arc<test_deadline::WorkerPhases>,
    diagnostic_context: &DiagnosticWorkerContext<'_>,
) -> Result<WorkerGroupResult, String> {
    if input.temporary_filesystem != temporary_root.is_some() {
        return Err("temporary root does not match the compiled filesystem capability".into());
    }
    if input.process_isolation != process_cgroup.is_some() {
        return Err("process isolation does not match the compiled process capability".into());
    }
    if let Some(path) = &process_cgroup {
        test_processes::validate_worker(path)
            .map_err(|error| format!("invalid worker process isolation: {error}"))?;
    }
    if input.source_revision != diagnostic_context.source_revision {
        return Err("closed worker source revision mismatch".into());
    }
    if input.target != BuildTarget::vm_hosted().name() {
        return Err("compiled worker target is unsupported".into());
    }
    let selection = entry_ids.iter().cloned().collect::<BTreeSet<_>>();
    if selection.len() != entry_ids.len() || selection.is_empty() {
        return Err("worker selection is empty or duplicated".into());
    }
    let selected = input
        .entries
        .iter()
        .filter(|entry| selection.contains(entry.id()))
        .collect::<Vec<_>>();
    if selected
        .iter()
        .map(|entry| entry.id())
        .ne(entry_ids.iter().map(String::as_str))
    {
        return Err("worker selection differs from the compiled participation".into());
    }
    let envelope_limits = input.envelope_limits();
    for entry in &selected {
        if !input.expected.contains_key(entry.id()) {
            return Err(format!(
                "compiled worker omitted snapshot expectations for `{}`",
                entry.id()
            ));
        }
        for depth in 1..=entry.suites().len() {
            let mut parts = entry.id().split("::").collect::<Vec<_>>();
            parts.truncate(parts.len() - 1 - (entry.suites().len() - depth));
            let id = parts.join("::");
            if !input.expected.contains_key(&id) {
                return Err(format!(
                    "compiled worker omitted snapshot expectations for `{id}`"
                ));
            }
        }
    }
    let (runtime_plan, mut materialized) = match input.runtime_inputs.materialize() {
        Ok(values) => values,
        Err(error) => {
            test_interrupt::worker_clean();
            return Ok(WorkerGroupResult {
                leaves: entry_ids
                    .iter()
                    .map(|id| {
                        let mut response = infrastructure_worker_response(error.clone());
                        response.error.as_mut().unwrap().kind = "input".into();
                        (id.clone(), response)
                    })
                    .collect(),
                suites: Vec::new(),
            });
        }
    };
    let environment = test_inputs::CapturedInputs::environment(&runtime_plan, &materialized)?;
    let mut participation = tondo_compiler::test_backend::TestParticipation::new(
        envelope_limits,
        input.expected,
        update_snapshots,
    )
    .with_interruption(test_interrupt::token())
    .with_selection(selection)
    .with_phases(phases);
    if let Some(root) = temporary_root {
        participation = participation.with_temporary_root(root);
    }
    let execution = tondo_compiler::test_backend::execute_compiled_with_environment(
        &input.program,
        input.entry,
        input.limits,
        participation.clone(),
        (!diagnostic_context.profiles.is_empty())
            .then(tondo_vm::runtime::DiagnosticConfig::default),
        environment,
    );
    materialized.revoke().map_err(|error| error.to_string())?;
    let execution = execution.map_err(|error| error.to_string())?;
    test_interrupt::worker_clean();
    if test_interrupt::requests() > 0 {
        return Err("test participation interrupted after cleanup".into());
    }
    let trace = execution.diagnostics;
    match execution.outcome {
        tondo_vm::runtime::VmOutcome::Returned(tondo_vm::runtime::RuntimeValue::Unit) => {}
        tondo_vm::runtime::VmOutcome::Panicked(panic) => {
            return Err(format!("{}: {}", panic.code.code(), panic.message));
        }
        _ => return Err("compiled test entry returned an invalid outcome".into()),
    }

    let executions = participation.executions()?;
    let mut responses = BTreeMap::new();
    for entry in &selected {
        let response = if let Some(execution) = executions.iter().find(|execution| {
            execution.kind == tondo_compiler::test_backend::TestExecutionKind::Leaf
                && execution.id == entry.id()
        }) {
            let error = WorkerError::from_execution(execution, &input.source_files);
            let status = if execution.timed_out {
                "timeout"
            } else if error.as_ref().is_some_and(|error| error.kind == "error") {
                "failed-error"
            } else if error.is_some() {
                "failed-panic"
            } else if matches!(
                execution.report.terminal(),
                Some(Terminal::ResourceLimit { .. })
            ) {
                "resource-limit"
            } else if matches!(execution.report.terminal(), Some(Terminal::Skipped { .. })) {
                "skipped"
            } else if execution.report.terminal().is_some() {
                "failed-panic"
            } else {
                "passed"
            };
            let crashed = error.as_ref().is_some_and(|error| error.kind == "panic");
            let attempt_id = format!("{}#{}", entry.id(), diagnostic_context.invocation);
            let diagnostics = diagnostic_reports_for(&DiagnosticReportContext {
                profiles: diagnostic_context.profiles,
                trace: trace.as_ref(),
                run_id: diagnostic_context.run_id,
                attempt_id: &attempt_id,
                shard: diagnostic_context.shard,
                target: &input.target,
                source_revision: diagnostic_context.source_revision,
                program_exit_status: if crashed { 101 } else { 0 },
                command_exit_status: 0,
                crashed,
            });
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
                diagnostics,
            }
        } else if let Some(suite) = executions
            .iter()
            .filter(|execution| {
                execution.kind == tondo_compiler::test_backend::TestExecutionKind::Suite
                    && (execution.timed_out
                        || execution.panic.is_some()
                        || execution.error_type.is_some()
                        || execution.report.terminal().is_some())
                    && entry
                        .id()
                        .strip_prefix(&execution.id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
            })
            .max_by_key(|execution| execution.id.len())
        {
            let skipped = matches!(suite.report.terminal(), Some(Terminal::Skipped { .. }))
                && suite
                    .panic
                    .as_ref()
                    .and_then(tondo_vm::runtime::VmPanic::language_panic)
                    .is_none();
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
                    source: None,
                    error_type: None,
                    kind: if skipped {
                        "blocked-skip".into()
                    } else {
                        "blocked-setup".into()
                    },
                    code: None,
                    message: suite.id.clone(),
                }),
                diagnostics: diagnostic_reports_for(&DiagnosticReportContext {
                    profiles: diagnostic_context.profiles,
                    trace: trace.as_ref(),
                    run_id: diagnostic_context.run_id,
                    attempt_id: &format!("{}#{}", entry.id(), diagnostic_context.invocation),
                    shard: diagnostic_context.shard,
                    target: &input.target,
                    source_revision: diagnostic_context.source_revision,
                    program_exit_status: 1,
                    command_exit_status: 0,
                    crashed: false,
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
            let error = WorkerError::from_execution(execution, &input.source_files);
            let status = match execution.report.terminal() {
                _ if execution.timed_out => "timeout",
                _ if error.as_ref().is_some_and(|error| error.kind == "error") => "failed-error",
                _ if error.is_some() => "failed-panic",
                Some(Terminal::Skipped { .. }) => "skipped",
                Some(Terminal::ResourceLimit { .. }) => "resource-limit",
                Some(Terminal::FailNow { .. }) | Some(Terminal::CleanupFailure { .. }) => {
                    "failed-panic"
                }
                None => "passed",
            };
            let phase = (status != "passed").then(|| match execution.phase {
                tondo_compiler::test_control::ExecutionPhase::Setup => "setup".to_owned(),
                tondo_compiler::test_control::ExecutionPhase::Body => "body".to_owned(),
                tondo_compiler::test_control::ExecutionPhase::Cleanup
                | tondo_compiler::test_control::ExecutionPhase::Closed => "teardown".to_owned(),
            });
            let crashed = status == "failed-panic";
            let diagnostics = diagnostic_reports_for(&DiagnosticReportContext {
                profiles: diagnostic_context.profiles,
                trace: trace.as_ref(),
                run_id: diagnostic_context.run_id,
                attempt_id: &format!("{}#{}", execution.id, diagnostic_context.invocation),
                shard: diagnostic_context.shard,
                target: &input.target,
                source_revision: diagnostic_context.source_revision,
                program_exit_status: if crashed { 101 } else { 0 },
                command_exit_status: 0,
                crashed,
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
                diagnostics,
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
                    && (execution.timed_out
                        || execution.panic.is_some()
                        || execution.report.terminal().is_some())
                    && id
                        .strip_prefix(&execution.id)
                        .is_some_and(|suffix| suffix.starts_with("::"))
            })
            .max_by_key(|execution| execution.id.len())
        else {
            return Err(format!("test participation omitted suite `{id}`"));
        };
        let skipped = matches!(blocker.report.terminal(), Some(Terminal::Skipped { .. }))
            && blocker
                .panic
                .as_ref()
                .and_then(tondo_vm::runtime::VmPanic::language_panic)
                .is_none();
        suites.push(WorkerSuiteResponse {
            id: id.clone(),
            status: if skipped {
                "blocked-skip".into()
            } else {
                "blocked-setup".into()
            },
            phase: None,
            report: empty_worker_report(),
            updates: Vec::new(),
            error: Some(WorkerError {
                source: None,
                error_type: None,
                kind: if skipped {
                    "blocked-skip".into()
                } else {
                    "blocked-setup".into()
                },
                code: None,
                message: blocker.id.clone(),
            }),
            diagnostics: diagnostic_reports_for(&DiagnosticReportContext {
                profiles: diagnostic_context.profiles,
                trace: trace.as_ref(),
                run_id: diagnostic_context.run_id,
                attempt_id: &format!("{}#{}", id, diagnostic_context.invocation),
                shard: diagnostic_context.shard,
                target: &input.target,
                source_revision: diagnostic_context.source_revision,
                program_exit_status: 1,
                command_exit_status: 0,
                crashed: false,
            }),
        });
    }
    suites.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    Ok(WorkerGroupResult {
        leaves: responses,
        suites,
    })
}

fn diagnostic_reports_for(context: &DiagnosticReportContext<'_>) -> Vec<WorkerDiagnostic> {
    context
        .profiles
        .iter()
        .map(|profile| {
            let mut status = DiagnosticStatus::Clean;
            let mut observations = 0_u64;
            let mut limitations = Vec::new();
            let mut artifacts = Vec::new();
            let mut payloads = Vec::new();
            match (profile, context.trace) {
                (DiagnosticProfile::Race, Some(trace)) => {
                    let report = detect_races(trace);
                    observations = report.observations;
                    status = match report.status {
                        tondo_vm::runtime::RaceStatus::Clean => DiagnosticStatus::Clean,
                        tondo_vm::runtime::RaceStatus::Finding => DiagnosticStatus::Finding,
                        tondo_vm::runtime::RaceStatus::Unsupported => DiagnosticStatus::Unsupported,
                    };
                    limitations = report
                        .limitations
                        .iter()
                        .map(|limitation| format!("{limitation:?}"))
                        .collect();
                }
                (DiagnosticProfile::Leaks, Some(trace)) => {
                    let report = detect_leaks(trace);
                    observations = report.observations;
                    status = match report.status {
                        tondo_vm::runtime::LeakStatus::Clean => DiagnosticStatus::Clean,
                        tondo_vm::runtime::LeakStatus::Finding => DiagnosticStatus::Finding,
                        tondo_vm::runtime::LeakStatus::Unsupported => DiagnosticStatus::Unsupported,
                    };
                    limitations = report
                        .limitations
                        .iter()
                        .map(|limitation| format!("{limitation:?}"))
                        .collect();
                }
                (DiagnosticProfile::Crash, Some(trace)) => {
                    if context.crashed {
                        status = DiagnosticStatus::Finding;
                        let termination = DumpTermination {
                            reason: "panic".into(),
                            program_exit_status: Some(context.program_exit_status),
                            command_exit_status: Some(context.command_exit_status),
                        };
                        let identity = DumpIdentity {
                            run_id: context.run_id.into(),
                            attempt_id: context.attempt_id.into(),
                            shard: context.shard.into(),
                            profile: profile.as_str().into(),
                            target: context.target.into(),
                            backend: "vm-hosted".into(),
                            toolchain: env!("CARGO_PKG_VERSION").into(),
                            source_revision: context.source_revision.into(),
                        };
                        match capture_dump(trace, identity, termination) {
                            Ok(bytes) if bytes.len() <= MAX_DUMP_BYTES => {
                                let sha256 = tondo_compiler::artifact::sha256(&bytes)
                                    .strip_prefix("sha256:")
                                    .unwrap_or_default()
                                    .to_owned();
                                let name = "diagnostic-crash.tdump".to_owned();
                                artifacts.push(ArtifactRecord {
                                    name: name.clone(),
                                    media_type: "application/x-tondo-dump".into(),
                                    size: bytes.len() as u64,
                                    sha256,
                                    object: format!(
                                        "objects/{}",
                                        tondo_compiler::artifact::sha256(&bytes)
                                            .strip_prefix("sha256:")
                                            .unwrap_or_default()
                                    ),
                                });
                                payloads.push(WorkerDiagnosticArtifact {
                                    name,
                                    media_type: "application/x-tondo-dump".into(),
                                    bytes,
                                });
                            }
                            Ok(_) => {
                                status = DiagnosticStatus::Unsupported;
                                limitations.push("report-byte-limit".into());
                            }
                            Err(_) => {
                                status = DiagnosticStatus::Failed;
                                limitations.push("dump-capture-failed".into());
                            }
                        }
                    }
                }
                (_, None) => {
                    status = DiagnosticStatus::Unsupported;
                    limitations.push("diagnostic-trace-unavailable".into());
                }
            }
            limitations.sort();
            limitations.dedup();
            let record = DiagnosticRecord {
                format: tondo_compiler::test_result::DIAGNOSTIC_REPORT_FORMAT.into(),
                run_id: context.run_id.into(),
                attempt_id: context.attempt_id.into(),
                shard: context.shard.into(),
                profile: profile.as_str().into(),
                status,
                target: context.target.into(),
                backend: "vm-hosted".into(),
                toolchain: env!("CARGO_PKG_VERSION").into(),
                source_revision: context.source_revision.into(),
                observations,
                limitations,
                artifacts,
                privacy: DiagnosticPrivacy {
                    payloads: "omitted-by-default".into(),
                    secrets: "never-emitted-by-default".into(),
                    paths: "logical-only".into(),
                    network_upload: false,
                },
                program_exit_status: Some(context.program_exit_status),
                command_exit_status: Some(context.command_exit_status),
            };
            WorkerDiagnostic {
                record,
                artifacts: payloads,
            }
        })
        .collect()
}

fn select_test_entries(
    entries: Vec<tondo_compiler::test_backend::TestEntry>,
    plan: &test_cli::TestCliPlan,
) -> Result<(usize, Vec<tondo_compiler::test_backend::TestEntry>), TestCommandError> {
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
    let matched = selected.len();
    if let Some(shard) = plan.shard {
        let spec = ShardSpec::new(shard.index, shard.count)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        let ids = selected.iter().map(|entry| entry.id()).collect::<Vec<_>>();
        let partition = tondo_compiler::test_shard::ShardResult::partition(ids, spec)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        selected.retain(|entry| partition.ids().any(|id| id == entry.id()));
    }
    Ok((matched, selected))
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
    invocation: u64,
    status: RuntimeStatus,
    report: EnvelopeReport,
    error: Option<RunError>,
    snapshot_updates: Vec<(String, String)>,
    diagnostics: Vec<WorkerDiagnostic>,
    diagnostic_artifacts: Vec<WorkerDiagnosticArtifact>,
}

#[derive(Debug, Clone)]
struct CliSuiteAttempt {
    id: String,
    iteration: u32,
    round: u32,
    unit: Option<u32>,
    invocation: u64,
    status: RuntimeStatus,
    phase: Option<AttemptPhase>,
    report: EnvelopeReport,
    error: Option<RunError>,
    snapshot_updates: Vec<(String, String)>,
    diagnostics: Vec<WorkerDiagnostic>,
    diagnostic_artifacts: Vec<WorkerDiagnosticArtifact>,
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
            let diagnostics = suite.diagnostics.clone();
            let diagnostic_artifacts = diagnostics
                .iter()
                .flat_map(|diagnostic| diagnostic.artifacts.clone())
                .collect();
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
                invocation,
                unit: None,
                status,
                phase,
                report,
                error,
                snapshot_updates: suite
                    .updates
                    .into_iter()
                    .map(|update| (update.name, update.value))
                    .collect(),
                diagnostics,
                diagnostic_artifacts,
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
    identity: &TestInvocationIdentity,
    snapshots: &SnapshotInputs,
) -> Result<Vec<CliAttempt>, TestCommandError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    if plan.repeat > 1 {
        let policy = RepeatPolicy::new(plan.repeat)
            .map_err(|error| TestCommandError::Usage(error.to_string()))?;
        let context = RepeatContext::new(
            entries.iter().map(|entry| entry.id().to_owned()),
            entries.iter().map(|entry| entry.id().to_owned()),
            shard_identity(plan),
            request.target().name(),
            identity.inputs.public_sha256(),
            order_seed(plan),
            tondo_compiler::test_report::CANONICAL_ORDER_ALGORITHM,
            request
                .capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned()),
            campaign_limits(plan),
            &identity.artifact_store_sha256,
            &snapshots.before_sha256,
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
                invocation: attempt.worker().invocation_id(),
                status: attempt.status(),
                report: attempt.report().clone(),
                error: attempt.error().cloned(),
                snapshot_updates: attempt.snapshot_updates().to_vec(),
                diagnostics: Vec::new(),
                diagnostic_artifacts: Vec::new(),
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
            invocation: leaf.worker().invocation_id(),
            status: leaf.status(),
            report: leaf.report().clone(),
            error: leaf.error().cloned(),
            snapshot_updates: leaf.snapshot_updates().to_vec(),
            diagnostics: Vec::new(),
            diagnostic_artifacts: Vec::new(),
        })
        .collect())
}

fn attach_worker_diagnostics(
    groups: &BTreeMap<(u32, String), Arc<SharedWorkerGroup>>,
    attempts: &mut [CliAttempt],
) -> Result<(), TestCommandError> {
    for attempt in attempts {
        let Some(group) = groups
            .values()
            .find(|group| group.entries.contains(&attempt.id))
        else {
            continue;
        };
        if group.diagnostics.is_empty() {
            continue;
        }
        match group.response(attempt.invocation, &attempt.id) {
            Ok(response) => {
                if response.diagnostics.is_empty() {
                    let attempt_id = format!("{}#{}", attempt.id, attempt.invocation);
                    attempt.diagnostics = failed_worker_diagnostics(group, &attempt_id);
                    attempt.diagnostic_artifacts = Vec::new();
                } else {
                    attempt.diagnostics = response.diagnostics.clone();
                    attempt.diagnostic_artifacts = response
                        .diagnostics
                        .iter()
                        .flat_map(|diagnostic| diagnostic.artifacts.clone())
                        .collect();
                }
            }
            Err(_) => {
                let attempt_id = format!("{}#{}", attempt.id, attempt.invocation);
                attempt.diagnostics = failed_worker_diagnostics(group, &attempt_id);
                attempt.diagnostic_artifacts = Vec::new();
            }
        }
    }
    Ok(())
}

fn failed_worker_diagnostics(group: &SharedWorkerGroup, attempt_id: &str) -> Vec<WorkerDiagnostic> {
    let mut diagnostics = diagnostic_reports_for(&DiagnosticReportContext {
        profiles: &group.diagnostics,
        trace: None,
        run_id: &group.run_id,
        attempt_id,
        shard: &group.shard,
        target: "tondo-vm-hosted",
        source_revision: &group.source_revision,
        program_exit_status: 3,
        command_exit_status: 3,
        crashed: false,
    });
    for diagnostic in &mut diagnostics {
        diagnostic.record.status = DiagnosticStatus::Failed;
        diagnostic.record.limitations = vec!["worker-failed".into()];
    }
    diagnostics
}

fn campaign_limits(plan: &test_cli::TestCliPlan) -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("timeout_ms".to_owned(), plan.timeout_ms.unwrap_or_default()),
        ("jobs".to_owned(), u64::from(plan.jobs)),
    ])
}

fn diagnostic_source_revision(request: &CompilationRequest) -> String {
    let mut bytes = Vec::new();
    for (_, source) in request.sources().iter() {
        bytes.extend_from_slice(source.path().as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(source.bytes());
        bytes.push(0);
    }
    tondo_compiler::artifact::sha256(&bytes)
        .strip_prefix("sha256:")
        .unwrap_or_default()
        .to_owned()
}

fn diagnostic_run_id(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    ordered: &[tondo_compiler::test_backend::TestEntry],
    identity: &TestInvocationIdentity,
) -> String {
    let mut bytes = diagnostic_source_revision(request).into_bytes();
    bytes.push(0);
    bytes.extend_from_slice(identity.inputs.public_sha256().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(identity.resource_profile_sha256.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(identity.artifact_store_sha256.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(shard_identity(plan).as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(order_seed(plan).to_string().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(plan.retry.to_string().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(plan.repeat.to_string().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(plan.jobs.to_string().as_bytes());
    bytes.push(0);
    for profile in &plan.diagnostics {
        bytes.extend_from_slice(profile.as_str().as_bytes());
        bytes.push(0);
    }
    bytes.push(0);
    bytes.extend_from_slice(selector_identity(&plan.selector).as_bytes());
    bytes.push(0);
    for entry in ordered {
        bytes.extend_from_slice(entry.id().as_bytes());
        bytes.push(0);
    }
    let hash = tondo_compiler::artifact::sha256(&bytes);
    format!("run-{}", hash.strip_prefix("sha256:").unwrap_or_default())
}

fn selector_identity(selector: &test_cli::TestSelector) -> String {
    match selector {
        test_cli::TestSelector::All => "all".to_owned(),
        test_cli::TestSelector::Filter(value) => format!("filter:{value}"),
        test_cli::TestSelector::Glob(value) => format!("glob:{value}"),
        test_cli::TestSelector::Exact(value) => format!("exact:{value}"),
    }
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

struct TestInvocationIdentity {
    inputs: TestInputPlan,
    resource_profile_sha256: String,
    artifact_store_sha256: String,
}

impl TestInvocationIdentity {
    fn capture(
        documents: &ProjectDocuments,
        supplied: &BTreeMap<String, Arc<[u8]>>,
        plan: &TestProjectPlan,
        execution: &test_cli::TestCliPlan,
        snapshots: &SnapshotInputs,
        ownership: &OwnershipInfo,
    ) -> Result<Self, TestCommandError> {
        let mut hashes = Vec::new();
        for source in plan.sources() {
            let bytes = supplied.get(source.physical_path()).ok_or_else(|| {
                TestCommandError::Internal(format!(
                    "missing captured test source `{}`",
                    source.physical_path()
                ))
            })?;
            hashes.push((
                source.input().to_owned(),
                source.logical_path().to_owned(),
                TestInputProfile::Build,
                tondo_compiler::artifact::sha256(bytes),
            ));
        }
        for (path, bytes) in supplied {
            if !plan
                .sources()
                .iter()
                .any(|source| source.physical_path() == path)
            {
                hashes.push((
                    format!("project-input:{path}"),
                    path.clone(),
                    TestInputProfile::Build,
                    tondo_compiler::artifact::sha256(bytes),
                ));
            }
        }
        let plan_bytes = plan
            .canonical_bytes()
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        if let Some(bytes) = &documents.test_dependencies {
            hashes.push((
                "plan:test-dependencies".into(),
                "test-dependencies".into(),
                TestInputProfile::Build,
                tondo_compiler::artifact::sha256(bytes),
            ));
        }
        for (name, bytes) in [
            ("project-manifest", documents.manifest.as_slice()),
            ("project-lockfile", documents.lockfile.as_slice()),
            ("test-plan", plan_bytes.as_slice()),
        ] {
            hashes.push((
                format!("plan:{name}"),
                name.into(),
                TestInputProfile::Build,
                tondo_compiler::artifact::sha256(bytes),
            ));
        }
        if let Some((manifest, lockfile)) = &documents.production {
            for (name, bytes) in [
                ("production-manifest", manifest),
                ("production-lockfile", lockfile),
            ] {
                hashes.push((
                    format!("plan:{name}"),
                    name.into(),
                    TestInputProfile::Build,
                    tondo_compiler::artifact::sha256(bytes),
                ));
            }
        }
        for loaded in &snapshots.stores {
            hashes.push((
                format!("snapshot:{}", loaded.name),
                loaded.relative.to_string_lossy().into_owned(),
                TestInputProfile::Runtime,
                loaded
                    .store
                    .content_hash()
                    .map_err(|error| TestCommandError::Internal(error.to_string()))?,
            ));
        }
        if let (Some(source), Some(hash)) = (&ownership.source, &ownership.sha256) {
            hashes.push((
                "codeowners".into(),
                source.clone(),
                TestInputProfile::Build,
                format!("sha256:{hash}"),
            ));
        }
        let inputs = TestInputPlan::from_public_hashes(plan, hashes)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        let inputs = inputs
            .with_runtime_inputs(plan, &documents.runtime_inputs)
            .map_err(|error| TestCommandError::Diagnostic(error.to_string()))?;
        let plan_value: serde_json::Value = serde_json::from_slice(&plan_bytes)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        let vm_defaults = ResourceLimits::default();
        let envelope = EnvelopeLimits::new(
            plan.limits().output_bytes(),
            plan.limits().artifact_bytes(),
            plan.limits().snapshot_bytes(),
        )
        .with_execution_limits(plan.limits().instructions(), plan.limits().virtual_timers())
        .profile();
        let mut resource_profile = serde_json::json!({
            "limits": plan_value["limits"], "jobs": execution.jobs, "timeout_ms": execution.timeout_ms,
            "hosted_accounting": "tondo-test-resources/2",
            "vm_stack_depth": vm_defaults.max_vm_stack_depth,
            "vm_frame_base_bytes": tondo_vm::runtime::TEST_FRAME_BASE_BYTES,
            "vm_frame_slot_bytes": tondo_vm::runtime::TEST_FRAME_SLOT_BYTES,
            "vm_frame_loan_bytes": tondo_vm::runtime::TEST_FRAME_LOAN_BYTES,
            "host_buffer_bytes": tondo_vm::runtime::TEST_HOST_BUFFER_BYTES,
            "detached_value_bytes": tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES,
            "sync_generation_bytes": tondo_vm::runtime::TEST_SYNC_GENERATION_BYTES,
            "host_job_bytes": tondo_vm::runtime::TEST_HOST_JOB_BYTES,
            "host_channel_bytes": tondo_vm::runtime::TEST_HOST_CHANNEL_BYTES,
            "text_diff_memory_model": tondo_compiler::test_limits::TEXT_DIFF_MEMORY_MODEL,
            "text_diff_record_bytes": tondo_compiler::test_limits::TEXT_DIFF_RECORD_BYTES,
            "text_diff_hunk_bytes": tondo_compiler::test_limits::TEXT_DIFF_HUNK_BYTES,
            "text_diff_transfer_model": tondo_compiler::test_limits::TEXT_DIFF_TRANSFER_MODEL,
            "assertion_diagnostic_model": tondo_compiler::test_limits::ASSERTION_DIAGNOSTIC_MODEL,
            "shrink_memory_model": tondo_compiler::test_limits::SHRINK_MEMORY_MODEL,
            "text_output_memory_model": tondo_compiler::test_limits::TEXT_OUTPUT_MEMORY_MODEL,
            "generation_id_record_bytes": tondo_compiler::test_limits::GENERATION_ID_RECORD_BYTES,
            "tolerance_error_result_bytes": tondo_compiler::test_limits::TOLERANCE_ERROR_RESULT_BYTES,
            "temp_error_result_bytes": tondo_compiler::test_limits::TEMP_ERROR_RESULT_BYTES,
            "generation_error_result_bytes": tondo_compiler::test_limits::GENERATION_ERROR_RESULT_BYTES,
            "temporary_root_model": tondo_compiler::test_temporaries::TEMPORARY_ROOT_MODEL,
            "process_isolation_model": test_processes::PROCESS_ISOLATION_MODEL,
            "vm_task_bytes": tondo_vm::runtime::TEST_TASK_BYTES,
            "vm_task_scope_bytes": tondo_vm::runtime::TEST_TASK_SCOPE_BYTES,
            "vm_scheduler_value_bytes": tondo_vm::runtime::TEST_SCHEDULER_VALUE_BYTES,
            "vm_select_base_bytes": tondo_vm::runtime::TEST_SELECT_BASE_BYTES,
            "vm_select_arm_bytes": tondo_vm::runtime::TEST_SELECT_ARM_BYTES,
            "vm_cleanup_base_bytes": tondo_vm::runtime::TEST_CLEANUP_BASE_BYTES,
            "vm_scheduler_handle_bytes": tondo_vm::runtime::TEST_SCHEDULER_HANDLE_BYTES,
            "vm_executor_worker_bytes": tondo_vm::runtime::TEST_EXECUTOR_WORKER_BYTES,
            "vm_heap_objects": vm_defaults.max_vm_heap_objects,
            "vm_entry_instructions": tondo_vm::runtime::TEST_ENTRY_INSTRUCTIONS,
            "vm_containment_instructions": tondo_vm::runtime::TEST_CONTAINMENT_INSTRUCTIONS,
            "evidence_work": envelope.work(),
            "evidence_metadata_bytes": envelope.metadata(),
            "artifact_count": envelope.artifact_count(),
            "snapshot_count": envelope.snapshot_count(),
        });
        resource_profile["host_argument_snapshot"] = serde_json::json!({
            "model": tondo_vm::runtime::TEST_SNAPSHOT_MEMORY_MODEL,
            "depth": tondo_vm::runtime::TEST_SNAPSHOT_MAX_DEPTH,
            "frame_bytes": tondo_vm::runtime::TEST_SNAPSHOT_FRAME_BYTES,
            "string_import_model": tondo_vm::runtime::TEST_STRING_IMPORT_MEMORY_MODEL,
            "host_return_model": tondo_vm::runtime::TEST_HOST_RETURN_MEMORY_MODEL,
            "fixed_host_return_model": tondo_compiler::test_limits::FIXED_HOST_RETURN_MEMORY_MODEL,
            "collection_host_return_model": tondo_compiler::test_limits::COLLECTION_HOST_RETURN_MEMORY_MODEL,
            "channel_receive_model": tondo_compiler::test_limits::CHANNEL_RECEIVE_MEMORY_MODEL,
            "channel_endpoint_model": tondo_compiler::test_limits::CHANNEL_ENDPOINT_MEMORY_MODEL,
            "channel_completion_model": tondo_compiler::test_limits::CHANNEL_COMPLETION_MEMORY_MODEL,
            "sync_guard_return_model": tondo_compiler::test_limits::SYNC_GUARD_RETURN_MEMORY_MODEL,
            "sync_wait_return_model": tondo_compiler::test_limits::SYNC_WAIT_RETURN_MEMORY_MODEL,
            "sync_value_return_model": tondo_compiler::test_limits::SYNC_VALUE_RETURN_MEMORY_MODEL,
            "host_import_model": tondo_vm::runtime::TEST_HOST_IMPORT_MEMORY_MODEL,
            "host_import_frame_bytes": tondo_vm::runtime::TEST_HOST_IMPORT_FRAME_BYTES,
            "host_import_admission_bytes": tondo_vm::runtime::TEST_HOST_IMPORT_ADMISSION_BYTES,
            "worker_host_import_context_bytes": tondo_vm::runtime::TEST_WORKER_HOST_IMPORT_CONTEXT_BYTES,
            "host_import_max_depth": tondo_vm::runtime::TEST_SNAPSHOT_MAX_DEPTH,
            "detached_walk_frame_bytes": tondo_vm::runtime::TEST_DETACHED_WALK_FRAME_BYTES,
            "job_argument_model": tondo_vm::runtime::TEST_HOST_JOB_ARGUMENT_MODEL,
            "text_measure_model": tondo_vm::runtime::TEST_TEXT_MEASURE_MEMORY_MODEL,
        });
        resource_profile["place_validation"] = serde_json::json!({
            "model": tondo_vm::runtime::TEST_PLACE_MEMORY_MODEL,
            "path_bytes": tondo_vm::runtime::TEST_PLACE_PATH_BYTES,
            "component_bytes": tondo_vm::runtime::TEST_PLACE_COMPONENT_BYTES,
            "index_bytes": tondo_vm::runtime::TEST_PLACE_INDEX_BYTES,
        });
        resource_profile["slice_memory_model"] = tondo_vm::runtime::TEST_SLICE_MEMORY_MODEL.into();
        resource_profile["assertion_display_model"] =
            tondo_vm::runtime::TEST_ASSERTION_DISPLAY_MODEL.into();
        let hash = tondo_compiler::artifact::sha256(
            &serde_json::to_vec(&resource_profile)
                .map_err(|error| TestCommandError::Internal(error.to_string()))?,
        );
        let artifact_store = serde_json::json!({"descriptor": plan_value["artifact_store"], "output_override": execution.artifacts});
        let artifact_hash = tondo_compiler::artifact::sha256(
            &serde_json::to_vec(&artifact_store)
                .map_err(|error| TestCommandError::Internal(error.to_string()))?,
        );
        Ok(Self {
            inputs,
            resource_profile_sha256: hash.trim_start_matches("sha256:").to_owned(),
            artifact_store_sha256: artifact_hash.trim_start_matches("sha256:").to_owned(),
        })
    }
}

fn build_test_list(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    ownership: &OwnershipInfo,
    snapshots: &SnapshotInputs,
    identity: &TestInvocationIdentity,
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
        identity,
    )?;
    let mut suites = BTreeMap::new();
    let tests = entries
        .iter()
        .map(|entry| {
            let (package, module, path) = identity_parts(entry.id());
            let owners = ownership
                .resolution
                .owners_for(Some(entry.logical_path()))
                .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            let ancestors = suite_ids(entry);
            for (index, id) in ancestors.iter().enumerate() {
                suites.entry(id.clone()).or_insert_with(|| {
                    let (package, module, path) = identity_parts(id);
                    let mut suite = TestNode::new(
                        id.clone(),
                        index.checked_sub(1).map(|parent| ancestors[parent].clone()),
                        package,
                        ResultNodeKind::Suite,
                        module,
                        id.rsplit("::").next().expect("a suite ID has a name"),
                        Vec::new(),
                    );
                    suite.path = path;
                    suite.owners = owners.clone();
                    suite
                });
            }
            let mut node = TestNode::new(
                entry.id().to_owned(),
                ancestors.last().cloned(),
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
        suites.into_values().collect(),
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
    identity: &TestInvocationIdentity,
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
    metadata.inputs.public_sha256 = identity.inputs.public_sha256().to_owned();
    metadata.inputs.secret_profile_sha256 =
        identity.inputs.secret_profile_sha256().map(str::to_owned);
    metadata.inputs.secret_count = identity.inputs.secret_count();
    metadata.inputs.reproducibility = match identity.inputs.reproducibility() {
        tondo_compiler::test_inputs::TestReproducibility::Closed => {
            tondo_compiler::test_report::InputReproducibility::Closed
        }
        tondo_compiler::test_inputs::TestReproducibility::SecretDependentVersioned => {
            tondo_compiler::test_report::InputReproducibility::SecretDependentVersioned
        }
        tondo_compiler::test_inputs::TestReproducibility::SecretDependentUnversioned => {
            tondo_compiler::test_report::InputReproducibility::SecretDependentUnversioned
        }
    };
    metadata.limits.resource_profile_sha256 = identity.resource_profile_sha256.clone();
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
    identity: &TestInvocationIdentity,
    retry_rounds: Vec<tondo_compiler::test_report::ReportRetryRound>,
) -> Result<TestReport, TestCommandError> {
    let mut metadata = report_metadata(request, plan, ownership, snapshots, mutation, identity)?;
    metadata.retry.rounds = retry_rounds;
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
                .map(|(index, source)| {
                    let mut attempt = make_test_attempt(index as u32 + 1, source)?;
                    bind_blocked_attempt(&mut attempt, source, suite_attempts)?;
                    Ok::<_, TestCommandError>(attempt)
                })
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
                let prefix = format!("{id}::");
                let cause = attempts.iter().find_map(|attempt| {
                    attempt
                        .id
                        .starts_with(&prefix)
                        .then_some(attempt.error.as_ref())
                        .flatten()
                });
                let detail = cause.map_or_else(String::new, |error| format!(": {error}"));
                return Err(TestCommandError::Internal(format!(
                    "runtime did not return suite `{id}`{detail}"
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
                        unit: source.unit,
                        invocation: source.invocation,
                        status: source.status,
                        report: source.report.clone(),
                        error: source.error.clone(),
                        snapshot_updates: source.snapshot_updates.clone(),
                        diagnostics: source.diagnostics.clone(),
                        diagnostic_artifacts: source.diagnostic_artifacts.clone(),
                    };
                    let mut attempt = make_test_attempt(index as u32 + 1, &source_attempt)?;
                    bind_blocked_attempt(&mut attempt, &source_attempt, suite_attempts)?;
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

fn bind_blocked_attempt(
    attempt: &mut TestAttempt,
    source: &CliAttempt,
    suites: &[CliSuiteAttempt],
) -> Result<(), TestCommandError> {
    if let Some(blocked) = &mut attempt.blocked_by {
        let mut causes = suites
            .iter()
            .filter(|suite| suite.id == blocked.id)
            .collect::<Vec<_>>();
        causes.sort_by_key(|suite| (suite.iteration, suite.round, suite.unit));
        let index = causes
            .iter()
            .position(|suite| suite.invocation == source.invocation)
            .ok_or_else(|| {
                TestCommandError::Internal(
                    "blocked attempt has no causal suite participation".into(),
                )
            })?;
        blocked.attempt = index as u32 + 1;
    }
    Ok(())
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
    for diagnostic in &source.diagnostics {
        for artifact in &diagnostic.record.artifacts {
            if !attempt
                .artifacts
                .iter()
                .any(|existing| existing.name == artifact.name)
            {
                attempt.artifacts.push(artifact.clone());
            }
        }
    }
    attempt.diagnostics = source
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.record.clone())
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
            error_type: error.error_type().map(str::to_owned),
            message: error.to_string().replace(['\r', '\n'], " "),
            source: error.source().cloned(),
            stack: Vec::new(),
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
        error_type: None,
        message: message.replace(['\r', '\n'], " "),
        source: None,
        stack: Vec::new(),
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
    for (index, attempt) in attempts.iter().enumerate() {
        let Some(mut store) = attempt_artifact_store(base, plan, artifact_store, attempt, index)?
        else {
            continue;
        };
        publish_attempt_artifact_values(&mut store, attempt)?;
    }
    Ok(())
}

fn attempt_artifact_store(
    base: &Path,
    plan: &test_cli::TestCliPlan,
    artifact_store: Option<&tondo_compiler::test_plan::TestArtifactStore>,
    attempt: &CliAttempt,
    index: usize,
) -> Result<Option<tondo_compiler::test_artifacts::ArtifactStore>, TestCommandError> {
    let root = plan.artifacts.as_ref().map_or_else(
        || base.join(artifact_store.map_or("target/test-artifacts", |store| store.path())),
        |path| base.join(path),
    );
    let max_bytes = artifact_store.map_or(64 * 1024 * 1024, |store| store.max_bytes());
    if attempt.report.artifacts().is_empty()
        && attempt.diagnostic_artifacts.is_empty()
        && plan.artifacts.is_none()
    {
        return Ok(None);
    }
    let identity = format!(
        "{}-{}-{}-{}",
        attempt.id, attempt.iteration, attempt.round, index
    );
    // A suite can retain both setup and teardown evidence. The store must
    // admit the independently bounded records already accepted by its worker.
    let max_items = tondo_compiler::test_limits::LimitProfile::default()
        .artifact_count()
        .checked_mul(2)
        .and_then(|count| count.checked_add(attempt.diagnostic_artifacts.len() as u64))
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(|| TestCommandError::Internal("artifact item budget overflows".into()))?;
    tondo_compiler::test_artifacts::ArtifactStore::new(
        &root,
        identity,
        tondo_compiler::test_artifacts::ArtifactLimits::new(max_bytes, max_items),
    )
    .map(Some)
    .map_err(|error| TestCommandError::Internal(error.to_string()))
}

fn publish_attempt_artifact_values(
    store: &mut tondo_compiler::test_artifacts::ArtifactStore,
    attempt: &CliAttempt,
) -> Result<(), TestCommandError> {
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
    for evidence in &attempt.diagnostic_artifacts {
        let descriptor = store
            .attach(&evidence.name, &evidence.media_type, &evidence.bytes)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        let expected = tondo_compiler::artifact::sha256(&evidence.bytes);
        if descriptor.sha256 != expected {
            return Err(TestCommandError::Internal(format!(
                "diagnostic artifact digest changed while publishing `{}`",
                evidence.name
            )));
        }
    }
    store
        .publish()
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
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
    build: bool,
    diagnostic_format: DiagnosticFormat,
    diagnostic_profiles: BTreeSet<DiagnosticProfile>,
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
        "build" => (Operation::Check, SourceForm::Module),
        "run" => (Operation::Run, SourceForm::Script),
        _ => return Err(format!("unknown command `{command}`")),
    };

    let mut diagnostic_format = DiagnosticFormat::Human;
    let mut diagnostic_profiles = BTreeSet::new();
    let mut diagnostics_seen = false;
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
        } else if argument == "--diagnostics" {
            if diagnostics_seen {
                return Err("`--diagnostics` may appear only once".into());
            }
            diagnostics_seen = true;
            index += 1;
            let Some(value) = arguments.get(index).and_then(|value| value.to_str()) else {
                return Err("`--diagnostics` requires one or more profiles".into());
            };
            diagnostic_profiles = test_cli::parse_diagnostics(value)?;
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
            } else if let Some(value) = argument.strip_prefix("--diagnostics=") {
                if diagnostics_seen {
                    return Err("`--diagnostics` may appear only once".into());
                }
                diagnostics_seen = true;
                diagnostic_profiles = test_cli::parse_diagnostics(value)?;
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
    if !diagnostic_profiles.is_empty() && operation != Operation::Run {
        return Err("`--diagnostics` is only valid with `tondo run` or `tondo test`".into());
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
        build: command == "build",
        diagnostic_format,
        diagnostic_profiles,
        warning_profiles,
        format_check,
        source,
        project,
        emit_interface,
        emit_artifact,
        program_arguments,
    })
}

fn default_build_artifact_path(invocation: &Invocation) -> PathBuf {
    let root = invocation
        .project
        .as_deref()
        .or_else(|| invocation.source.as_deref().and_then(Path::parent))
        .unwrap_or_else(|| Path::new("."));
    root.join("build").join("tondo.artifact.json")
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
        return Ok((
            request.with_diagnostic_profiles(invocation.diagnostic_profiles.iter().copied()),
            None,
        ));
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
    Ok((
        request.with_diagnostic_profiles(invocation.diagnostic_profiles.iter().copied()),
        Some(bytes),
    ))
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
        let artifact_hash = tondo_compiler::artifact::sha256(&bytes);
        write_atomic(path, &bytes)
            .map_err(|error| format!("cannot write artifact `{}`: {error}", path.display()))?;
        if invocation.build {
            let native_manifest = serde_json::json!({
                "format": "tondo-native-build/1",
                "compiler": tondo_compiler::artifact::COMPILER_ID,
                "edition": tondo_compiler::LANGUAGE_EDITION,
                "artifact_sha256": artifact_hash,
                "target": artifact.target(),
                "profile": artifact.profile(),
                "candidates": ["cranelift", "llvm"],
                "backend": "cranelift",
                "status": "backend-selected",
                "promotion": "pending-gate-n1",
                "execution": "tondo-run-uses-the-same-source-and-closed-plan",
                "ambient_lookup": false,
                "backend_flags": false
            });
            let manifest = serde_json::to_vec(&native_manifest)
                .map_err(|error| format!("cannot encode native build manifest: {error}"))?;
            let manifest_path = path.with_extension("native.json");
            write_atomic(&manifest_path, &manifest).map_err(|error| {
                format!(
                    "cannot write native build manifest `{}`: {error}",
                    manifest_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        path.extension()
            .and_then(OsStr::to_str)
            .unwrap_or("artifact")
    ));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_containment_closes_inherited_pipes_after_leader_exit_and_forced_timeout() {
        let root = env::var_os("TONDO_TEST_PROCESS_CGROUP")
            .map(PathBuf::from)
            .expect(
                "process lifecycle tests require an explicit delegated TONDO_TEST_PROCESS_CGROUP",
            );
        let provider = test_processes::prepare(Some(&root), true).unwrap().unwrap();
        for (script, timeout, normal) in [
            (
                "read ready; /bin/sleep 60 & printf out; printf err >&2; exit 0",
                10_000,
                true,
            ),
            ("read ready; /bin/sleep 60 & wait", 50, false),
        ] {
            let mut group = provider.create().unwrap();
            assert!(test_processes::validate_worker(group.path()).is_err());
            let mut child = Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            group.admit(child.id()).unwrap();
            child.stdin.take().unwrap().write_all(b"ready\n").unwrap();
            let outcome =
                wait_worker_controlled(child, Some(timeout), None, None, Some(&mut group));
            if normal {
                let (_, stdout, stderr) = outcome.unwrap();
                assert_eq!(stdout, b"out");
                assert_eq!(stderr, b"err");
            } else {
                assert!(matches!(outcome, Err(RunError::Timeout)), "{outcome:?}");
            }
            assert!(!group.path().exists());
            group.close().unwrap();
        }
    }

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
        fs::write(root.join("tondo.toml"), "[package]\nname = \"cli\"\n[target]\ncapabilities = [\"console\", \"filesystem\", \"clock\", \"environment\", \"threads\"]\n").unwrap();
        let discovered = project_discovery::discover_for_tests(&root).unwrap();
        let project =
            ProjectPlan::parse(&discovered.manifest_bytes, &discovered.lockfile_bytes).unwrap();
        let (manifest, lockfile) = discovered.production.as_ref().unwrap();
        let production = ProjectPlan::parse(manifest, lockfile).unwrap();
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
            &TestProjectPlan::for_discovered_project(Some(&production), &project, 1)
                .unwrap()
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
    fn explicit_test_plan_rejects_global_fail_fast_before_empty_selection() {
        let root = conventional_test_project(b"test smoke { assert(true) }\n");
        let path = root.join("tondo.test.toml");
        let mut value: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["policy"]["fail_fast"] = toml::Value::Boolean(true);
        fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
        let outcomes = [false, true].map(|list| {
            let mut arguments = vec![
                OsString::from("test"),
                OsString::from("--filter"),
                OsString::from("missing"),
                OsString::from("--allow-empty"),
            ];
            if list {
                arguments.push(OsString::from("--list"));
            }
            let plan = test_cli::parse(&arguments).unwrap();
            execute_test_plan(&plan, &root)
        });
        fs::remove_dir_all(&root).unwrap();
        for outcome in outcomes {
            assert!(
                matches!(outcome, Err(TestCommandError::Usage(ref message))
                    if message.contains("policy.fail_fast")),
                "unsupported fail-fast must reject the sidecar before selection: {outcome:?}"
            );
        }
    }

    #[test]
    fn closed_worker_input_executes_after_project_removal_and_preserves_snapshots() {
        let root = conventional_test_project(
            b"import std.testing\ntest smoke { testing.snapshot(\"golden\", \"value\") }\n",
        );
        let loaded = ProjectLocation::Directory(root.clone()).load().unwrap();
        let plan = load_test_project_plan(
            &loaded.project,
            loaded.production.as_ref(),
            Some(&root.join("tondo.test.toml")),
            loaded.documents.test_dependencies.as_deref(),
        )
        .unwrap();
        let supplied = loaded
            .project
            .required_inputs()
            .map(|input| {
                (
                    input.path().to_owned(),
                    Arc::<[u8]>::from(fs::read(root.join(input.path())).unwrap()),
                )
            })
            .collect();
        let request = prepare_test_request(
            &loaded.project,
            loaded.production.as_ref(),
            &supplied,
            &plan,
            DiagnosticFormat::Human,
        )
        .unwrap();
        let entries = discover_tests(&request).unwrap();
        let ids = entries
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["cli::integration::smoke::smoke"]);
        let store = SnapshotStore::from_entries(
            request.packages().root().as_str(),
            [tondo_compiler::test_snapshots::SnapshotEntry {
                node_id: ids[0].clone(),
                name: "golden".into(),
                value: "value".into(),
            }],
        )
        .unwrap();
        fs::write(
            root.join("tests/snapshots.json"),
            store.canonical_bytes().unwrap(),
        )
        .unwrap();
        let snapshots =
            load_snapshot_inputs(&root, loaded.project.root_package_id(), &plan, false).unwrap();
        let execution = test_cli::parse(&[OsString::from("test")]).unwrap();
        let ownership = resolve_ownership(&execution, &root).unwrap();
        let identity = TestInvocationIdentity::capture(
            &loaded.documents,
            &supplied,
            &plan,
            &execution,
            &snapshots,
            &ownership,
        )
        .unwrap();
        let captured = test_inputs::CapturedInputs::capture(
            &identity.inputs,
            &snapshots,
            plan.limits().memory_bytes(),
        )
        .unwrap();
        let input = WorkerInput::capture(&request, &plan, &snapshots, &entries, &captured).unwrap();
        let encoded = Arc::new(input.encode().unwrap());
        let revision = diagnostic_source_revision(&request);
        let context = DiagnosticWorkerContext {
            profiles: &BTreeSet::new(),
            run_id: "pinned-input-regression",
            source_revision: &revision,
            shard: "all",
            invocation: 0,
        };
        // Runtime filesystem providers belong to the coordinator, independently
        // of the source project that has already been compiled.
        let worker_project = root.with_extension("worker-project");
        fs::create_dir(&worker_project).unwrap();
        let temporary_filesystem = input.temporary_filesystem;
        // No worker can reconstruct the invocation from these paths now.
        fs::remove_dir_all(&root).unwrap();
        let phase_limits = test_deadline::Limits {
            body: 10_000,
            setup: 10_000,
            teardown: 10_000,
        };
        let result = spawn_test_worker(
            &worker_project,
            None,
            temporary_filesystem,
            encoded,
            &ids,
            Some(10_000),
            phase_limits,
            false,
            &context,
        )
        .unwrap();
        let response = &result.leaves[&ids[0]];
        assert_eq!(response.status, "passed", "{:?}", response.error);
        assert!(response.error.is_none());
        let report = EnvelopeReport::decode_process(&response.report).unwrap();
        assert!(matches!(
            report.snapshots()[0].outcome(),
            SnapshotOutcome::Matched { .. }
        ));

        // A newly hashed but invalid program still has to pass the VM verifier.
        let bytes = input.encode().unwrap();
        let hash = tondo_compiler::artifact::sha256(&bytes);
        let mut invalid = WorkerInput::read(bytes.as_slice(), &hash, bytes.len()).unwrap();
        invalid.program.functions[invalid.entry.index() as usize]
            .blocks
            .clear();
        let result = spawn_test_worker(
            &worker_project,
            None,
            temporary_filesystem,
            Arc::new(invalid.encode().unwrap()),
            &ids,
            Some(10_000),
            phase_limits,
            false,
            &context,
        )
        .unwrap();
        assert_eq!(result.leaves[&ids[0]].status, "infrastructure");
        assert!(
            result.leaves[&ids[0]]
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("bytecode")
        );

        let mut revision_context = context;
        revision_context.source_revision = "wrong-source-revision";
        let result = spawn_test_worker(
            &worker_project,
            None,
            temporary_filesystem,
            Arc::new(bytes),
            &ids,
            Some(10_000),
            phase_limits,
            false,
            &revision_context,
        )
        .unwrap();
        assert_eq!(
            result.leaves[&ids[0]].error.as_ref().unwrap().message,
            "closed worker source revision mismatch"
        );
        let context = DiagnosticWorkerContext {
            source_revision: &revision,
            ..revision_context
        };

        // Changing an expectation changes behavior, proving the transported
        // values reach the actual VM testing host.
        let mut changed = input;
        changed
            .expected
            .get_mut(&ids[0])
            .unwrap()
            .insert("golden".into(), "different".into());
        let result = spawn_test_worker(
            &worker_project,
            None,
            temporary_filesystem,
            Arc::new(changed.encode().unwrap()),
            &ids,
            Some(10_000),
            phase_limits,
            false,
            &context,
        )
        .unwrap();
        assert_ne!(result.leaves[&ids[0]].status, "passed");
        assert_eq!(
            fs::read_dir(worker_project.join("target/.tondo-test-root"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(worker_project).unwrap();
    }

    #[test]
    fn worker_transport_stops_serialization_and_pipe_reads_at_the_byte_limit() {
        struct Stream(std::cell::Cell<usize>);
        impl Serialize for Stream {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut sequence = serializer.serialize_seq(Some(10_000))?;
                for index in 0..10_000 {
                    self.0.set(index + 1);
                    sequence.serialize_element("entry")?;
                }
                sequence.end()
            }
        }
        let stream = Stream(std::cell::Cell::new(0));
        assert!(
            encode_bounded_worker_json(&stream, 64)
                .unwrap_err()
                .contains("transport limit")
        );
        assert!(
            stream.0.get() < 10,
            "serialization must stop without materializing the whole frame"
        );
        let value = "escaped\nvalue";
        let expected = serde_json::to_vec(value).unwrap();
        assert_eq!(
            encode_bounded_worker_json(&value, expected.len()).unwrap(),
            expected
        );
        assert!(encode_bounded_worker_json(&value, expected.len() - 1).is_err());
        let mut input = io::Cursor::new(vec![b'x'; 10_000]);
        assert!(
            read_bounded_worker_pipe(&mut input, 64)
                .unwrap_err()
                .to_string()
                .contains("transport limit")
        );
        assert_eq!(input.position(), 65);
        assert_eq!(
            read_bounded_worker_pipe(b"exact".as_slice(), 5).unwrap(),
            b"exact"
        );
        assert!(read_bounded_worker_pipe(io::empty(), 0).unwrap().is_empty());
    }

    #[test]
    fn closed_worker_input_rejects_drift_missing_identity_and_oversize() {
        let input = WorkerInput {
            source_files: BTreeMap::new(),
            format: WORKER_INPUT_FORMAT.into(),
            source_revision: "1".repeat(64),
            target: BuildTarget::vm_hosted().name().into(),
            temporary_filesystem: false,
            process_isolation: false,
            program: tondo_vm::bytecode::BytecodeProgram {
                reflection: Default::default(),
                types: Vec::new(),
                nominals: Vec::new(),
                callables: Vec::new(),
                constants: Vec::new(),
                functions: Vec::new(),
            },
            entry: tondo_vm::bytecode::BytecodeFunctionId::new(0),
            limits: tondo_vm::runtime::VmLimits::default(),
            output_bytes: 4096,
            artifact_bytes: 4096,
            snapshot_bytes: 4096,
            virtual_timers: 1024,
            entries: Vec::new(),
            expected: BTreeMap::new(),
            runtime_inputs: test_inputs::CapturedInputs::default(),
        };
        let bytes = input.encode().unwrap();
        let hash = tondo_compiler::artifact::sha256(&bytes);
        let read = WorkerInput::read(bytes.as_slice(), &hash, bytes.len()).unwrap();
        assert_eq!(read.encode().unwrap(), bytes);
        for (field, expected) in [
            ("max_steps", "invalid VM limit `max_steps`"),
            ("virtual_timers", "virtual-timers budget must be positive"),
            ("output_bytes", "output budget must be positive"),
            ("artifact_bytes", "artifact-bytes budget must be positive"),
            ("snapshot_bytes", "snapshot-bytes budget must be positive"),
        ] {
            let mut invalid = serde_json::to_value(&input).unwrap();
            if field == "max_steps" {
                invalid["limits"][field] = 0.into();
            } else {
                invalid[field] = 0.into();
            }
            let invalid = serde_json::to_vec(&invalid).unwrap();
            let hash = tondo_compiler::artifact::sha256(&invalid);
            let error = WorkerInput::read(invalid.as_slice(), &hash, invalid.len()).unwrap_err();
            assert_eq!(error, expected, "{field}");
        }
        assert!(
            WorkerInput::read(bytes.as_slice(), "", bytes.len())
                .unwrap_err()
                .contains("hash is required")
        );
        assert!(
            WorkerInput::read(bytes.as_slice(), &hash, bytes.len() - 1)
                .unwrap_err()
                .contains("transport limit")
        );
        for field in [
            "revision",
            "target",
            "code",
            "entry",
            "limits",
            "selection",
            "snapshots",
        ] {
            let mut changed = WorkerInput::read(bytes.as_slice(), &hash, bytes.len()).unwrap();
            match field {
                "revision" => changed.source_revision.push('0'),
                "target" => changed.target.push('0'),
                "code" => changed
                    .program
                    .types
                    .push(tondo_vm::bytecode::BytecodeType {
                        name: "Unit".into(),
                        kind: tondo_vm::bytecode::BytecodeTypeKind::Scalar(
                            tondo_vm::bytecode::BytecodeScalarType::Unit,
                        ),
                    }),
                "entry" => changed.entry = tondo_vm::bytecode::BytecodeFunctionId::new(1),
                "limits" => changed.limits.max_steps += 1,
                "selection" => changed.entries.push(WorkerTestEntry {
                    id: "leaf".into(),
                    suites: Vec::new(),
                }),
                "snapshots" => {
                    changed.expected.insert("leaf".into(), BTreeMap::new());
                }
                _ => unreachable!(),
            }
            let changed = changed.encode().unwrap();
            assert!(
                WorkerInput::read(changed.as_slice(), &hash, changed.len())
                    .unwrap_err()
                    .contains("hash mismatch"),
                "{field}"
            );
        }

        let nested_base = bytes.clone();
        std::thread::Builder::new()
            .stack_size(CLI_STACK_SIZE)
            .spawn(move || {
                use tondo_vm::bytecode::{
                    BytecodeConstantValue, BytecodeConstantValueKind, BytecodeNamedConstant,
                    BytecodeTypeId,
                };
                let hash = tondo_compiler::artifact::sha256(&nested_base);
                let mut nested =
                    WorkerInput::read(nested_base.as_slice(), &hash, nested_base.len()).unwrap();
                let mut value = BytecodeConstantValue {
                    ty: BytecodeTypeId::new(0),
                    kind: BytecodeConstantValueKind::Unit,
                };
                for _ in 0..256 {
                    value = BytecodeConstantValue {
                        ty: BytecodeTypeId::new(0),
                        kind: BytecodeConstantValueKind::OptionSome(Box::new(value)),
                    };
                }
                nested.program.constants.push(BytecodeNamedConstant {
                    name: "nested".into(),
                    value,
                });
                let bytes = nested.encode().unwrap();
                let hash = tondo_compiler::artifact::sha256(&bytes);
                let decoded = WorkerInput::read(bytes.as_slice(), &hash, bytes.len()).unwrap();
                assert_eq!(decoded.program, nested.program);
            })
            .unwrap()
            .join()
            .unwrap();

        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b" {}");
        for (bytes, message) in [
            (trailing, "invalid closed worker input"),
            (vec![b'['; MAX_WORKER_INPUT_DEPTH + 1], "nesting limit"),
            (b"{".to_vec(), "invalid closed worker input"),
            (
                {
                    let mut unknown = input;
                    unknown.format = "unsupported".into();
                    unknown.encode().unwrap()
                },
                "unsupported closed worker input format",
            ),
        ] {
            let hash = tondo_compiler::artifact::sha256(&bytes);
            assert!(
                WorkerInput::read(bytes.as_slice(), &hash, bytes.len())
                    .unwrap_err()
                    .contains(message)
            );
        }
    }

    #[test]
    fn worker_transport_depth_counts_only_containers_outside_json_strings() {
        let quoted = serde_json::to_vec("[[{{\\\"\\\\}}]]").unwrap();
        assert!(check_worker_input_depth(&quoted, 0).is_ok());
        for limit in [0, 1, MAX_WORKER_INPUT_DEPTH] {
            let mut bytes = vec![b'['; limit];
            bytes.extend(std::iter::repeat_n(b']', limit));
            assert!(check_worker_input_depth(&bytes, limit).is_ok());
            bytes.insert(0, b'[');
            assert!(
                check_worker_input_depth(&bytes, limit)
                    .unwrap_err()
                    .contains("nesting limit")
            );
        }
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
    fn parses_dump_analyzer_formats_and_requires_a_tdump_path() {
        let human = parse_dump_invocation(&arguments(&["dump", "analyze", "panic.tdump"])).unwrap();
        assert_eq!(human.format, DumpOutputFormat::Human);
        assert_eq!(human.path, PathBuf::from("panic.tdump"));

        let json = parse_dump_invocation(&arguments(&[
            "dump",
            "analyze",
            "panic.tdump",
            "--format=json",
        ]))
        .unwrap();
        assert_eq!(json.format, DumpOutputFormat::Json);

        assert!(
            parse_dump_invocation(&arguments(&["dump", "analyze", "panic.json"]))
                .unwrap_err()
                .contains("`.tdump`")
        );
        assert!(
            parse_dump_invocation(&arguments(&[
                "dump",
                "analyze",
                "panic.tdump",
                "--format",
                "xml",
            ]))
            .unwrap_err()
            .contains("unknown dump format")
        );
    }

    #[test]
    fn dump_analyzer_reads_a_valid_artifact_and_rejects_corruption() {
        let sections = vec![
            (
                "header",
                serde_json::json!({
                    "format": "tondo-dump/1",
                    "version": 1,
                    "content_address": "sha256",
                    "user_payloads": "omitted-by-default"
                }),
            ),
            (
                "termination",
                serde_json::json!({
                    "reason": "panic",
                    "program_exit_status": 101,
                    "command_exit_status": 101
                }),
            ),
            (
                "identity",
                serde_json::json!({
                    "run_id": "cli-test",
                    "attempt_id": "attempt-1",
                    "shard": "0/1",
                    "profile": "crash",
                    "target": "linux-x86_64",
                    "backend": "bytecode-vm",
                    "toolchain": "test",
                    "source_revision": "revision"
                }),
            ),
            ("stacks", serde_json::json!([])),
            (
                "heap_summary",
                serde_json::json!({
                    "object_count": 0,
                    "allocation_count": 0,
                    "replacement_count": 0,
                    "allocated_bytes": 0,
                    "objects": []
                }),
            ),
            ("resource_ledger", serde_json::json!([])),
            ("scheduler_tail", serde_json::json!([])),
            (
                "redaction",
                serde_json::json!({
                    "payloads": "omitted-by-default",
                    "secrets": "never-emitted-by-default",
                    "paths": "logical-only",
                    "network_upload": false,
                    "executes_dump_code": false
                }),
            ),
            (
                "limitations",
                serde_json::json!({
                    "truncated": false,
                    "unavailable": ["registers", "native-unwind", "physical-paths"],
                    "events_seen": 0
                }),
            ),
        ];
        let artifact = tondo_vm::runtime::DumpArtifact {
            format: tondo_vm::runtime::DUMP_SCHEMA.into(),
            version: 1,
            content_sha256: String::new(),
            sections: sections
                .into_iter()
                .map(|(name, value)| tondo_vm::runtime::DumpSection {
                    name: name.into(),
                    value,
                })
                .collect(),
        };
        let path = temp_root().join(format!("tondo-cli-dump-{}.tdump", std::process::id()));
        fs::write(&path, artifact.encode().unwrap()).unwrap();
        let valid = run_dump_command(&arguments(&[
            "dump",
            "analyze",
            path.to_str().unwrap(),
            "--format",
            "json",
        ]))
        .unwrap();
        assert_eq!(valid, ExitCode::SUCCESS);

        fs::write(&path, b"{}").unwrap();
        let invalid =
            run_dump_command(&arguments(&["dump", "analyze", path.to_str().unwrap()])).unwrap();
        assert_eq!(invalid, ExitCode::from(EXIT_USAGE));
        fs::remove_file(path).unwrap();
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
                &[
                    "run",
                    "--diagnostics",
                    "race",
                    "--diagnostics",
                    "leaks",
                    "main.to",
                ],
                "`--diagnostics` may appear only once",
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
            invocation: 0,
            status: RuntimeStatus::FailedError,
            report: report.clone(),
            error: Some(RunError::Error {
                code: Some("E-test".into()),
                error_type: "model.TestError".into(),
                source: None,
                message: "bad\nvalue".into(),
            }),
            snapshot_updates: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_artifacts: Vec::new(),
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
            invocation: 0,
            status: RuntimeStatus::Skipped,
            report: report.clone(),
            error: Some(RunError::Skip {
                reason: "not applicable".into(),
            }),
            snapshot_updates: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_artifacts: Vec::new(),
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
        let diagnostic_path = project.join("target/diagnostic-report.json");
        let diagnostic_plan = test_cli::parse(
            [
                OsString::from("test"),
                OsString::from("--project"),
                OsString::from(project.to_str().unwrap()),
                OsString::from("--diagnostics"),
                OsString::from("all"),
                OsString::from("--test-format"),
                OsString::from("json"),
                OsString::from("--report"),
                OsString::from(format!("json={}", diagnostic_path.display())),
            ]
            .as_slice(),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&diagnostic_plan, &project).unwrap(), 0);
        let diagnostic_report = TestReport::parse(&fs::read(&diagnostic_path).unwrap()).unwrap();
        assert!(
            diagnostic_report
                .tests()
                .iter()
                .flat_map(|node| node.attempts.iter())
                .all(|attempt| attempt.diagnostics.len() == 3)
        );
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
            load_test_project_plan(&project, None, Some(Path::new("tondo.test.json")), None)
                .unwrap_err();
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
                code: Some("T1".into()),
                error_type: "model.TestError".into(),
                source: None,
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
                    source: None,
                    error_type: None,
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
            invocation: 0,
            status: RuntimeStatus::Passed,
            report: report.clone(),
            error: None,
            snapshot_updates: vec![("new-value".into(), "value".into())],
            diagnostics: Vec::new(),
            diagnostic_artifacts: Vec::new(),
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
                invocation: 0,
                status: RuntimeStatus::Passed,
                report: report.clone(),
                error: None,
                snapshot_updates: Vec::new(),
                diagnostics: Vec::new(),
                diagnostic_artifacts: Vec::new(),
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
            invocation: 0,
            status: RuntimeStatus::Passed,
            report: report.clone(),
            error: None,
            snapshot_updates: vec![("known".into(), "new".into())],
            diagnostics: Vec::new(),
            diagnostic_artifacts: Vec::new(),
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
