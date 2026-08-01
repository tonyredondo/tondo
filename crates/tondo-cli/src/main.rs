use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

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
use tondo_compiler::test_repeat::{RepeatCampaign, RepeatContext, RepeatPolicy};
use tondo_compiler::test_report::{
    OrderMode as ReportOrderMode, ReportMetadata, ReportOrder, ReportSelection, ReportShard,
    SelectionKind, SnapshotMode, SnapshotStoreIdentity, TestList, TestReport,
};
use tondo_compiler::test_result::{
    AggregateStatus, ArtifactRecord, AttemptStatus, FailureRecord, ResultNodeKind, RetryUnit,
    RetryUnitKind, SkipRecord, SnapshotRecord, SnapshotStatus, TestAttempt, TestNode,
    VirtualTimeRecord,
};
use tondo_compiler::test_retry::{RetryCampaign, RetryContext, RetryPolicy};
use tondo_compiler::test_runtime::{
    LeafProgram, RunError, RuntimeConfig, RuntimeRunner, RuntimeStatus,
};
use tondo_compiler::test_schedule::{OrderMode, ScheduleNode, SchedulePlan, Seed};
use tondo_compiler::test_shard::ShardSpec;

mod test_cli;

const EXIT_DIAGNOSTIC: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_INTERNAL: u8 = 3;

type PreparedCompilation = (CompilationRequest, Option<Arc<[u8]>>);

const USAGE: &str = "\
Tondo bootstrap toolchain

Usage:
  tondo <command> [--diagnostic-format <human|json>] [--warnings core] <source.to>
  tondo <check|run> [--diagnostic-format <human|json>] [--warnings core] --manifest <tondo.json>
  tondo run [--diagnostic-format <human|json>] [--warnings core] <source.to> -- [argument ...]

Commands:
  fmt      Format one Tondo source file
  check    Analyze one Tondo source file
  run      Compile and run one Tondo script
  test     Discover, compile and run project tests

Options:
  --diagnostic-format <human|json>  Select diagnostic output
  --warnings <core>                 Enable a closed warning profile
  --check                           Verify formatting without writing output (fmt only)
  --manifest <path>                 Build a closed project manifest (check/run only)
  --lockfile <path>                 Use this lockfile (default: tondo.lock.json)
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
    let Some(manifest_path) = plan.manifest.as_ref() else {
        eprintln!("tondo: `tondo test` requires `--manifest <tondo.json>`\n\n{USAGE}");
        return Ok(ExitCode::from(EXIT_USAGE));
    };
    match execute_test_plan(&plan, manifest_path) {
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

#[derive(Debug)]
struct OwnershipInfo {
    mode: tondo_compiler::test_report::OwnershipMode,
    source: Option<String>,
    sha256: Option<String>,
    resolution: tondo_compiler::test_owners::OwnershipResolution,
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

fn execute_test_plan(
    plan: &test_cli::TestCliPlan,
    manifest_path: &Path,
) -> Result<u8, TestCommandError> {
    let base = manifest_path.parent().unwrap_or_else(|| Path::new(""));
    let lockfile = base.join("tondo.lock.json");
    let manifest = read_input(manifest_path, "manifest").map_err(TestCommandError::Usage)?;
    let lockfile_bytes = read_input(&lockfile, "lockfile").map_err(TestCommandError::Usage)?;
    let project = ProjectPlan::parse(&manifest, &lockfile_bytes)
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
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
            plan.diagnostic_format,
            ResourceLimits::default(),
        )
        .map_err(|error| TestCommandError::Usage(error.to_string()))?;
    let request = Arc::new(request);
    let ownership = resolve_ownership(plan, base)?;
    let entries = discover_tests(&request)?;
    let selected = select_test_entries(entries, plan)?;
    if selected.is_empty() {
        if plan.allow_empty {
            if plan.list {
                return Ok(0);
            }
            eprintln!("tondo: no tests selected");
            return Ok(0);
        }
        return Err(TestCommandError::Diagnostic(
            "tondo: no tests matched the selection".into(),
        ));
    }
    if plan.list {
        if plan.test_format == test_cli::TestFormat::Json {
            let list = build_test_list(&request, plan, &selected, &ownership)?;
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

    let ordered = order_test_entries(selected, plan)?;
    let programs = ordered
        .iter()
        .map(|entry| {
            let id = entry.id().to_owned();
            let base_request = Arc::clone(&request);
            let entry = entry.clone();
            Ok(LeafProgram::new(id, move |context| {
                let request = base_request.for_test_entry(&entry).map_err(|error| {
                    RunError::Infrastructure {
                        message: error.to_string(),
                    }
                })?;
                let output = execute(request).map_err(|error| RunError::Infrastructure {
                    message: error.to_string(),
                })?;
                if !output.stdout().is_empty() {
                    context.stdout(String::from_utf8_lossy(output.stdout()).into_owned())?;
                }
                if output.status() == CompilationStatus::Success {
                    Ok(())
                } else {
                    let diagnostic = output.diagnostics().human();
                    let message = if diagnostic.is_empty() {
                        "test body failed".to_owned()
                    } else {
                        diagnostic
                    };
                    context.stderr(message.clone())?;
                    Err(RunError::Error {
                        code: "T3001".into(),
                        message,
                    })
                }
            }))
        })
        .collect::<Result<Vec<_>, TestCommandError>>()?;
    let runtime = RuntimeRunner::new(
        RuntimeConfig::new(
            plan.jobs as usize,
            EnvelopeLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 16 * 1024 * 1024),
        )
        .map_err(|error| TestCommandError::Internal(error.to_string()))?,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let attempts = execute_campaign(&request, plan, &ordered, programs, runtime)?;
    publish_attempt_artifacts(base, plan, &attempts)?;
    let report = build_test_report(&request, plan, &ordered, &ownership, &attempts)?;
    publish_test_outputs(plan, &report)?;
    if plan.test_format == test_cli::TestFormat::Json {
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
            if plan.show_output {
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
    let nodes = entries
        .iter()
        .map(|entry| ScheduleNode::test(entry.id().to_owned(), None::<String>))
        .collect::<Vec<_>>();
    let schedule = SchedulePlan::new(nodes, mode, plan.jobs)
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

#[derive(Debug)]
struct CliAttempt {
    id: String,
    iteration: u32,
    round: u32,
    unit: Option<u32>,
    status: RuntimeStatus,
    report: EnvelopeReport,
    error: Option<RunError>,
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
) -> Result<TestList, TestCommandError> {
    let metadata = report_metadata(request, plan, ownership)?;
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
        empty_snapshot_identity(request)?,
        entries.iter().map(|entry| entry.id().to_owned()).collect(),
        Vec::new(),
        tests,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))
}

fn empty_snapshot_identity(
    request: &CompilationRequest,
) -> Result<SnapshotStoreIdentity, TestCommandError> {
    let package = request.packages().root().as_str();
    let store = tondo_compiler::test_snapshots::SnapshotStore::empty(package)
        .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let hash = store
        .content_hash()
        .map_err(|error| TestCommandError::Internal(error.to_string()))?
        .strip_prefix("sha256:")
        .unwrap_or_default()
        .to_owned();
    Ok(SnapshotStoreIdentity {
        format: tondo_compiler::test_report::TEST_SNAPSHOT_FORMAT.into(),
        sha256: hash,
    })
}

fn report_metadata(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    ownership: &OwnershipInfo,
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

fn build_test_report(
    request: &CompilationRequest,
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    ownership: &OwnershipInfo,
    attempts: &[CliAttempt],
) -> Result<TestReport, TestCommandError> {
    let mut metadata = report_metadata(request, plan, ownership)?;
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
    TestReport::assemble(
        metadata,
        entries.iter().map(|entry| entry.id().to_owned()).collect(),
        Vec::new(),
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
        let units = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                attempts
                    .iter()
                    .any(|attempt| attempt.id == entry.id() && attempt.round == round)
                    .then(|| RetryUnit {
                        kind: RetryUnitKind::Test,
                        id: entry.id().to_owned(),
                        execution_plan: vec![entry.id().to_owned()],
                    })
                    .map(|unit| (index, unit))
            })
            .collect::<Vec<_>>();
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
            automatic_advances: 0,
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
                SnapshotOutcome::Missing { actual_sha256 } => {
                    (SnapshotStatus::Missing, None, actual_sha256)
                }
                SnapshotOutcome::Mismatched {
                    expected_sha256,
                    actual_sha256,
                } => (
                    SnapshotStatus::Mismatched,
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
    attempts: &[CliAttempt],
) -> Result<(), TestCommandError> {
    let root = plan.artifacts.as_ref().map_or_else(
        || base.join("target/test-artifacts"),
        |path| base.join(path),
    );
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
            tondo_compiler::test_artifacts::ArtifactLimits::new(64 * 1024 * 1024, 64),
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
    manifest: Option<PathBuf>,
    lockfile: Option<PathBuf>,
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
    let mut manifest: Option<PathBuf> = None;
    let mut lockfile: Option<PathBuf> = None;
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
            if source.is_none() && manifest.is_none() {
                return Err("the source file or manifest must appear before `--`".into());
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
        } else if argument == "--manifest" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--manifest` requires a path".into());
            };
            if manifest.replace(PathBuf::from(value)).is_some() {
                return Err("`--manifest` may appear only once".into());
            }
        } else if argument == "--lockfile" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--lockfile` requires a path".into());
            };
            if lockfile.replace(PathBuf::from(value)).is_some() {
                return Err("`--lockfile` may appear only once".into());
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

    if source.is_some() && manifest.is_some() {
        return Err("choose either one source file or `--manifest`, not both".into());
    }
    if source.is_none() && manifest.is_none() {
        return Err("a source file is required (or use `--manifest` for a project)".into());
    }
    if operation == Operation::Format && manifest.is_some() {
        return Err("`tondo fmt` accepts a source file, not a project manifest".into());
    }
    if operation == Operation::Format && (emit_interface.is_some() || emit_artifact.is_some()) {
        return Err("build products are only available from `check` or `run`".into());
    }
    if operation == Operation::Format && !warning_profiles.is_empty() {
        return Err("warning profiles are only available from `check` or `run`".into());
    }
    if lockfile.is_some() && manifest.is_none() {
        return Err("`--lockfile` requires `--manifest`".into());
    }
    if let Some(source) = &source {
        validate_source_extension(source)?;
        if source.file_name().and_then(OsStr::to_str).is_none() {
            return Err("source filename is not valid UTF-8".into());
        }
    }
    if let Some(manifest_path) = &manifest {
        let resolved_lockfile = lockfile.get_or_insert_with(|| {
            manifest_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join("tondo.lock.json")
        });
        for output in [&emit_interface, &emit_artifact].into_iter().flatten() {
            if paths_refer_to_same_location(output, manifest_path)
                || paths_refer_to_same_location(output, resolved_lockfile)
            {
                return Err(
                    "an emitted product must not overwrite the manifest or lockfile".into(),
                );
            }
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
        manifest,
        lockfile,
        emit_interface,
        emit_artifact,
        program_arguments,
    })
}

fn compilation_request(invocation: &Invocation) -> Result<PreparedCompilation, String> {
    if let Some(manifest_path) = &invocation.manifest {
        let lockfile_path = invocation
            .lockfile
            .as_ref()
            .expect("parse_invocation resolves the default lockfile");
        let manifest = read_input(manifest_path, "manifest")?;
        let lockfile = read_input(lockfile_path, "lockfile")?;
        let plan = ProjectPlan::parse(&manifest, &lockfile).map_err(|error| error.to_string())?;
        let base = manifest_path.parent().unwrap_or_else(|| Path::new(""));
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
        .expect("parse_invocation requires a source or manifest");
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
    use tondo_compiler::artifact::CAPABILITY_REGISTRY;
    use tondo_compiler::project::MANIFEST_FORMAT;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation_error(values: &[&str]) -> String {
        parse_invocation(&arguments(values)).unwrap_err()
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
            "--manifest",
            "project/tondo.json",
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
            (&["check", "--manifest"], "`--manifest` requires"),
            (
                &["check", "--manifest", "one.json", "--manifest", "two.json"],
                "`--manifest` may appear only once",
            ),
            (&["check", "--lockfile"], "`--lockfile` requires"),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--lockfile",
                    "one.lock",
                    "--lockfile",
                    "two.lock",
                ],
                "`--lockfile` may appear only once",
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
                &["check", "main.to", "--manifest", "tondo.json"],
                "choose either",
            ),
            (
                &["fmt", "--manifest", "tondo.json"],
                "accepts a source file",
            ),
            (
                &["fmt", "main.to", "--emit-interface", "main.ti"],
                "build products",
            ),
            (&["fmt", "--warnings=core", "main.to"], "warning profiles"),
            (
                &["check", "--lockfile", "tondo.lock.json", "main.to"],
                "requires `--manifest`",
            ),
            (&["check", "main.tondo"], "`.to` extension"),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--emit-interface",
                    "tondo.json",
                ],
                "must not overwrite the manifest or lockfile",
            ),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--lockfile",
                    "custom.lock",
                    "--emit-artifact",
                    "custom.lock",
                ],
                "must not overwrite the manifest or lockfile",
            ),
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
        };
        assert_eq!(skip_reason(&skipped), "not applicable");
        assert!(make_test_attempt(2, &skipped).unwrap().skip.is_some());

        let base = std::env::temp_dir().join(format!(
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
        publish_attempt_artifacts(&base, &artifacts_plan, &[failed]).unwrap();
        assert!(base.join("artifacts").exists());
        fs::remove_dir_all(base).unwrap();

        let project = std::env::temp_dir().join(format!(
            "tondo-cli-backend-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(project.join("tests")).unwrap();
        let source = b"test smoke { assert(true) }\n";
        fs::write(project.join("tests/smoke.to"), source).unwrap();
        let package_id = "workspace:cli-helper@1";
        let source_hash = tondo_compiler::artifact::sha256(source);
        let standard_package = tondo_compiler::project::BOOTSTRAP_STANDARD_PACKAGE;
        let lockfile_format = tondo_compiler::project::LOCKFILE_FORMAT;
        let manifest = format!(
            "{{\"format\":\"{MANIFEST_FORMAT}\",\"target\":{{\"name\":\"tondo-vm-hosted\",\"profile\":\"hosted\",\"capability_registry\":\"{CAPABILITY_REGISTRY}\",\"capabilities\":[],\"features\":[]}},\"root\":{{\"package\":\"{package_id}\",\"source\":\"tests/smoke.to\",\"form\":\"module\"}},\"standard\":\"{standard_package}\",\"packages\":[{{\"id\":\"{package_id}\",\"local_name\":\"cli\",\"edition\":\"0.1\",\"dependencies\":[],\"source_sets\":[{{\"id\":\"common\",\"sources\":[{{\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\"}}]}}]}}],\"generator_inputs\":[],\"privileged_units\":[]}}"
        );
        fs::write(project.join("tondo.json"), &manifest).unwrap();
        let package_fingerprint = format!(
            "{{\"package_id\":\"{package_id}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\",\"sha256\":\"{source_hash}\"}}],\"interface_hash\":null}}"
        );
        let lockfile = format!(
            "{{\"format\":\"{lockfile_format}\",\"manifest_hash\":\"{}\",\"standard\":{{\"package_id\":\"{standard_package}\",\"content_hash\":\"{}\"}},\"packages\":[{{\"id\":\"{package_id}\",\"content_hash\":\"{}\",\"dependencies\":[],\"sources\":[{{\"source_set\":\"common\",\"physical_path\":\"tests/smoke.to\",\"logical_path\":\"tests/smoke.to\",\"module\":\"smoke\",\"sha256\":\"{source_hash}\"}}],\"interface\":null}}],\"generator_inputs\":[],\"privileged_units\":[]}}",
            tondo_compiler::artifact::sha256(manifest.as_bytes()),
            tondo_compiler::project::bootstrap_standard_hash(),
            tondo_compiler::artifact::sha256(package_fingerprint.as_bytes()),
        );
        fs::write(project.join("tondo.lock.json"), lockfile).unwrap();
        let manifest_path = project.join("tondo.json");
        let run_plan = test_cli::parse(
            &[
                "test",
                "--manifest",
                "tondo.json",
                "--order",
                "random",
                "--seed",
                "a",
                "--report",
                "json=target/helper-report.json",
                "--test-format",
                "json",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&run_plan, &manifest_path).unwrap(), 0);
        let list_plan = test_cli::parse(
            &[
                "test",
                "--manifest",
                "tondo.json",
                "--list",
                "--test-format",
                "json",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&list_plan, &manifest_path).unwrap(), 0);
        let repeat_plan = test_cli::parse(
            &["test", "--manifest", "tondo.json", "--repeat", "2"].map(OsString::from),
        )
        .unwrap();
        assert_eq!(execute_test_plan(&repeat_plan, &manifest_path).unwrap(), 0);
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
                OsString::from("--manifest"),
                manifest_path.clone().into(),
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
}
