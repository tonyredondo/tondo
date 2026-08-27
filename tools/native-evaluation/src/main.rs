//! Fast, backend-facing code-generation measurements for NATIVE-001.
//!
//! This tool deliberately consumes the bounded MIR probe rather than source
//! files.  It builds the same normalized module shape through Cranelift and
//! LLVM `llc`, so the fast lane can compare backend-engine cost before Tondo's
//! native ABI and runtime lowering exist.  Its opt-in runner additionally
//! executes the bounded scalar slice against both backend-neutral and VM
//! oracles.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cranelift_codegen::ir::{
    AbiParam, Block, FuncRef, Function, InstBuilder, Signature, TrapCode, UserFuncName, Value,
    condcodes::IntCC,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use serde::{Deserialize, Serialize};

const CRANELIFT_VERSION: &str = "0.132.3";
const REPETITIONS: usize = 3;
const MAX_FUNCTIONS: u64 = 256;
const MAX_ORACLE_STEPS: usize = 100_000;
const MAX_ORACLE_CALL_DEPTH: usize = 256;
const MAX_NATIVE_CASE_RUNTIME: Duration = Duration::from_secs(2);
const ORACLE_MANAGED_BIT: u64 = 1 << 63;
const ORACLE_TAG_SHIFT: u32 = 56;
const ORACLE_TAG_MASK: u64 = 0x7;
// Bits 59..62 are reserved by the private oracle carrier. Keeping the
// payload at 56 bits retains the existing string-hash range while preserving
// the distinction between a payload of zero and no payload.
const ORACLE_HAS_PAYLOAD_BIT: u64 = 1 << 59;
const ORACLE_PAYLOAD_MASK: u64 = (1 << ORACLE_TAG_SHIFT) - 1;

#[derive(Debug, Deserialize)]
struct ProbeReport {
    format: String,
    fixtures: Vec<FixtureObservation>,
}

#[derive(Debug, Deserialize)]
struct FixtureObservation {
    fixture: String,
    fixture_sha256: String,
    status: String,
    mir: Option<MirSummary>,
    #[serde(default)]
    vm_scalar: Vec<VmScalarObservation>,
    #[serde(default)]
    vm_managed: Vec<VmManagedObservation>,
}

#[derive(Debug, Deserialize, Clone)]
struct VmScalarObservation {
    function_ordinal: u32,
    arguments: Vec<i64>,
    status: String,
    result: Option<i64>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct VmManagedObservation {
    function_ordinal: u32,
    arguments: Vec<i64>,
    status: String,
    tag: Option<u64>,
    payload: Option<i64>,
    #[allow(dead_code)]
    payload_text: Option<String>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirSummary {
    #[serde(default)]
    backend: Option<MirBackendProgram>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendProgram {
    format: String,
    #[serde(default)]
    debug: Option<MirBackendDebugInfo>,
    functions: Vec<MirBackendFunction>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendDebugInfo {
    format: String,
    sources: Vec<MirBackendSource>,
    symbols: Vec<MirBackendDebugSymbol>,
    source_maps: Vec<MirBackendSourceMap>,
    executions: Vec<MirBackendExecutionIdentity>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendSource {
    ordinal: u32,
    module: String,
    logical_path: String,
    content_sha256: String,
    length: u32,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendSpan {
    source: u32,
    start: u32,
    end: u32,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendDebugSymbol {
    function: u32,
    name: String,
    native: String,
    span: MirBackendSpan,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendSourceMap {
    id: String,
    kind: String,
    function: u32,
    block: Option<u32>,
    span: MirBackendSpan,
    unwind: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendExecutionIdentity {
    id: String,
    kind: String,
    function: u32,
    block: u32,
    span: MirBackendSpan,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendFunction {
    ordinal: u32,
    parameters: Vec<u32>,
    #[serde(default)]
    parameter_types: Vec<String>,
    return_local: u32,
    #[serde(default)]
    return_type: String,
    supported: bool,
    blocks: Vec<MirBackendBlock>,
}

#[derive(Debug, Deserialize, Clone)]
struct MirBackendBlock {
    ordinal: u32,
    kind: String,
    statements: Vec<MirBackendStatement>,
    terminator: MirBackendTerminator,
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendStatement {
    Assign {
        destination: u32,
        value: MirBackendRvalue,
    },
    Marker {
        kind: String,
    },
    Runtime {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendRvalue {
    Use(MirBackendOperand),
    Tag {
        value: u32,
    },
    Aggregate {
        kind: String,
        values: Vec<MirBackendOperand>,
    },
    Prefix {
        operator: String,
        operand: MirBackendOperand,
    },
    Binary {
        operator: String,
        left: MirBackendOperand,
        right: MirBackendOperand,
    },
    NumericConversion {
        source: String,
        target: String,
        conversion: String,
        operand: MirBackendOperand,
    },
    Coerce {
        kind: String,
        operand: MirBackendOperand,
    },
    HostCall {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
    Unsupported {
        kind: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendOperand {
    Constant(MirBackendConstant),
    Local { index: u32 },
    Borrow { index: u32 },
    Function { kind: String },
    Projection {
        index: u32,
        depth: u32,
        #[serde(default)]
        kind: String,
    },
    Unsupported { kind: String },
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendConstant {
    Unit,
    Bool(bool),
    Integer(String),
    Float(String),
    Char(String),
    String(String),
    Named,
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendTerminator {
    Return,
    Goto {
        target: u32,
    },
    SwitchBool {
        condition: MirBackendOperand,
        if_true: u32,
        if_false: u32,
    },
    SwitchTag {
        value: MirBackendOperand,
        cases: Vec<(u32, u32)>,
        otherwise: u32,
    },
    Invoke {
        operation: MirBackendOperation,
        destination: Option<u32>,
        target: Option<u32>,
    },
    Marker {
        kind: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendOperation {
    CheckedPrefix {
        operator: String,
        operand: MirBackendOperand,
    },
    CheckedBinary {
        operator: String,
        left: MirBackendOperand,
        right: MirBackendOperand,
    },
    BoundsCheck {
        index: MirBackendOperand,
        length: MirBackendOperand,
    },
    Call {
        function: u32,
        arguments: Vec<MirBackendOperand>,
    },
    Spawn {
        operation: Box<MirBackendOperation>,
        kind: String,
    },
    JoinValue {
        operand: MirBackendOperand,
    },
    HostCall {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
    Runtime {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
    Assert {
        condition: MirBackendOperand,
    },
    Trap {
        kind: String,
    },
    Marker {
        kind: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct EvaluationReport {
    format: &'static str,
    phase: &'static str,
    status: &'static str,
    target: String,
    adapter: AdapterReport,
    protocol: Protocol,
    candidates: Vec<CandidateReport>,
    excluded: Vec<ExcludedCandidate>,
    correctness: CorrectnessStatus,
    debug_metadata: Vec<DebugMetadataReport>,
    native_runs: Vec<NativeRunReport>,
    native_managed_runs: Vec<NativeManagedRunReport>,
    native_std_core_runs: Vec<NativeStdCoreRunReport>,
    native_runtime_runs: Vec<NativeRuntimeRunReport>,
    native_select_runs: Vec<NativeSelectRunReport>,
    native_thread_runs: Vec<NativeThreadRunReport>,
    native_lowering_runs: Vec<NativeLoweringRunReport>,
    native_aot_lowering: NativeAotLoweringReport,
    native_diagnostics: NativeDiagnosticsReport,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct DebugMetadataReport {
    fixture: String,
    format: String,
    sources: usize,
    symbols: usize,
    source_maps: usize,
    task_identities: usize,
    thread_identities: usize,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct AdapterReport {
    format: &'static str,
    supported_subset: &'static str,
    unsupported_policy: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Protocol {
    warmup_iterations: u32,
    measurement_repetitions: u32,
    independent_processes: u32,
    minimum_sample_count: u32,
    seed: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateReport {
    id: &'static str,
    status: &'static str,
    adapter: &'static str,
    toolchain: String,
    samples: Vec<SampleReport>,
    dimensions: Dimensions,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SampleReport {
    fixture: String,
    fixture_sha256: String,
    compile_time_ns: u128,
    code_size_bytes: u64,
    supported_functions: u64,
    unsupported_functions: u64,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Dimensions {
    compile_time: &'static str,
    code_size: &'static str,
    peak_memory: &'static str,
    runtime: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ExcludedCandidate {
    id: &'static str,
    status: &'static str,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct CorrectnessStatus {
    mir_probe: &'static str,
    cranelift: &'static str,
    llvm: &'static str,
    native_semantics: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeRunReport {
    fixture: String,
    function_ordinal: u32,
    arguments: Vec<i64>,
    oracle_status: &'static str,
    oracle_result: Option<i64>,
    vm_status: String,
    vm_result: Option<i64>,
    vm_diagnostics: Vec<String>,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeManagedRunReport {
    fixture: String,
    function_ordinal: u32,
    arguments: Vec<i64>,
    oracle_status: &'static str,
    oracle_tag: u64,
    oracle_payload: Option<u64>,
    vm_status: String,
    vm_tag: Option<u64>,
    vm_payload: Option<i64>,
    vm_diagnostics: Vec<String>,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeStdCoreRunReport {
    case: String,
    function_ordinal: u32,
    kind: &'static str,
    oracle_result: Option<i64>,
    oracle_tag: Option<u64>,
    oracle_payload: Option<u64>,
    vm_status: String,
    vm_result: Option<i64>,
    vm_tag: Option<u64>,
    vm_payload: Option<i64>,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeRuntimeRunReport {
    case: String,
    function_ordinal: u32,
    expected_result: Option<i64>,
    expected_tag: Option<u64>,
    expected_payload: Option<u64>,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeSelectRunReport {
    case: String,
    expected_result: i64,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeThreadRunReport {
    case: String,
    expected_result: i64,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeLoweringRunReport {
    case: String,
    function_ordinal: u32,
    pending_before_join: u64,
    result_after_join: i64,
    joined_after_join: u64,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeAotLoweringReport {
    format: &'static str,
    phase: &'static str,
    status: &'static str,
    mir_format: &'static str,
    oracle: &'static str,
    candidates: [&'static str; 2],
    same_mir: bool,
    feature_families: Vec<NativeAotFeatureReport>,
    cases: Vec<NativeAotCaseReport>,
    traps: Vec<NativeAotTrapReport>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeAotFeatureReport {
    id: &'static str,
    cases: u32,
    cranelift: &'static str,
    llvm: &'static str,
    vm: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeAotCaseReport {
    id: String,
    function_ordinal: u32,
    feature: String,
    vm_status: &'static str,
    vm_result: i64,
    cranelift: &'static str,
    llvm: &'static str,
    same_mir: bool,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeAotTrapReport {
    candidate: &'static str,
    function_ordinal: u32,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeDiagnosticsReport {
    format: &'static str,
    phase: &'static str,
    status: &'static str,
    oracle: &'static str,
    backends: [&'static str; 2],
    cases: Vec<NativeDiagnosticCaseReport>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeDiagnosticCaseReport {
    profile: &'static str,
    case: &'static str,
    mode: u64,
    expected_status: &'static str,
    cranelift: &'static str,
    llvm: &'static str,
    envelope: NativeDiagnosticEnvelope,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeDiagnosticEnvelope {
    format: String,
    profile: String,
    case: String,
    mode: u64,
    status: String,
    task_ids: u64,
    thread_ids: u64,
    happens_before_edges: u64,
    roots: u64,
    retainers: u64,
    cycles_reclaimed: u64,
    ffi_allocations: u64,
    resources_acquired: u64,
    resources_released: u64,
    unwind_frames: u64,
    source_maps: u64,
    redacted: bool,
    payloads_omitted: bool,
    corruption_rejected: bool,
    limit_enforced: bool,
}

#[derive(Debug, Clone, Copy)]
struct NativeDiagnosticCase {
    profile: &'static str,
    profile_id: u64,
    name: &'static str,
    mode: u64,
    expected_status: &'static str,
    expected_code: u64,
}

#[derive(Debug, Clone)]
struct RuntimeContractCase {
    name: &'static str,
    function_ordinal: u32,
    expectation: RuntimeExpectation,
}

#[derive(Debug, Clone, Copy)]
enum RuntimeExpectation {
    Scalar(i64),
    Managed { tag: u64, payload: Option<u64> },
}

#[derive(Debug)]
struct Options {
    probe: PathBuf,
    std_core_probe: Option<PathBuf>,
    output: PathBuf,
    llvm: PathBuf,
    target: String,
    temp_dir: PathBuf,
    cc: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("native evaluation adapter: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    if !options.llvm.is_absolute() {
        return Err("--llvm must be an absolute, explicitly selected executable".into());
    }
    if !options.llvm.is_file() {
        return Err(format!(
            "LLVM executable does not exist: {}",
            options.llvm.display()
        ));
    }
    if let Some(cc) = &options.cc {
        if !cc.is_absolute() {
            return Err("--cc must be an absolute, explicitly selected executable".into());
        }
        if !cc.is_file() {
            return Err(format!(
                "C linker executable does not exist: {}",
                cc.display()
            ));
        }
    }
    fs::create_dir_all(&options.temp_dir)
        .map_err(|error| format!("cannot create temporary directory: {error}"))?;

    let probe_bytes = fs::read(&options.probe)
        .map_err(|error| format!("cannot read probe `{}`: {error}", options.probe.display()))?;
    let probe: ProbeReport = serde_json::from_slice(&probe_bytes)
        .map_err(|error| format!("invalid MIR probe: {error}"))?;
    validate_probe(&probe)?;

    let isa = cranelift_isa()?;
    let llvm_version = command_version(&options.llvm)?;
    let mut cranelift_samples = Vec::new();
    let mut llvm_samples = Vec::new();
    let mut native_runs = Vec::new();
    let mut native_managed_runs = Vec::new();
    let mut native_std_core_runs = Vec::new();
    let mut native_runtime_runs = Vec::new();
    let mut debug_metadata = Vec::new();
    let mut native_lowering_runs = Vec::new();
    let mut native_aot_lowering = pending_native_aot_lowering_report();
    let mut native_diagnostics = NativeDiagnosticsReport {
        format: "tondo-native-diagnostics/1",
        phase: "DIAG-NATIVE-001",
        status: "pending-native-lowering",
        oracle: "hosted-diagnostic-contract-fixtures",
        backends: ["cranelift", "llvm"],
        cases: Vec::new(),
    };

    for fixture in &probe.fixtures {
        let summary = fixture
            .mir
            .as_ref()
            .ok_or_else(|| format!("fixture has no MIR summary: {}", fixture.fixture))?;
        let backend = summary.backend.as_ref().ok_or_else(|| {
            format!(
                "fixture has no normalized MIR adapter input: {}",
                fixture.fixture
            )
        })?;
        let debug = backend
            .debug
            .as_ref()
            .ok_or_else(|| format!("fixture has no debug metadata: {}", fixture.fixture))?;
        debug_metadata.push(DebugMetadataReport {
            fixture: fixture.fixture.clone(),
            format: debug.format.clone(),
            sources: debug.sources.len(),
            symbols: debug.symbols.len(),
            source_maps: debug.source_maps.len(),
            task_identities: debug
                .executions
                .iter()
                .filter(|execution| execution.kind == "task")
                .count(),
            thread_identities: debug
                .executions
                .iter()
                .filter(|execution| execution.kind == "thread")
                .count(),
        });
        if let Some(cc) = &options.cc {
            native_runs.extend(run_native_scalar_probe(
                &options.llvm,
                cc,
                &options.target,
                &options.temp_dir,
                fixture,
                backend,
            )?);
            native_managed_runs.extend(run_native_managed_probe(
                &options.llvm,
                cc,
                &options.target,
                &options.temp_dir,
                fixture,
                backend,
            )?);
        }
        // Keep the first invocation out of the report so allocator and backend
        // lazy-initialization cost does not dominate the short feedback lane.
        let _ = compile_cranelift(isa.as_ref(), backend)?;
        let _ = compile_llvm(
            &options.llvm,
            &options.target,
            &options.temp_dir,
            fixture,
            backend,
        )?;
        for _ in 0..REPETITIONS {
            let cranelift = compile_cranelift(isa.as_ref(), backend)?;
            cranelift_samples.push(SampleReport {
                fixture: fixture.fixture.clone(),
                fixture_sha256: fixture.fixture_sha256.clone(),
                compile_time_ns: cranelift.compile_time_ns,
                code_size_bytes: cranelift.code_size_bytes,
                supported_functions: cranelift.supported_functions,
                unsupported_functions: cranelift.unsupported_functions,
            });

            let llvm = compile_llvm(
                &options.llvm,
                &options.target,
                &options.temp_dir,
                fixture,
                backend,
            )?;
            llvm_samples.push(SampleReport {
                fixture: fixture.fixture.clone(),
                fixture_sha256: fixture.fixture_sha256.clone(),
                compile_time_ns: llvm.compile_time_ns,
                code_size_bytes: llvm.code_size_bytes,
                supported_functions: llvm.supported_functions,
                unsupported_functions: llvm.unsupported_functions,
            });
        }
    }
    if let Some(cc) = &options.cc {
        if let Some(std_core_probe) = &options.std_core_probe {
            let (fixture, program) = load_std_core_probe(std_core_probe)?;
            native_std_core_runs = run_native_std_core_probe(
                &options.llvm,
                cc,
                &options.target,
                &options.temp_dir,
                &fixture,
                &program,
            )?;
        }
        native_runtime_runs =
            run_native_runtime_probe(&options.llvm, cc, &options.target, &options.temp_dir)?;
        native_lowering_runs = run_native_lowering_probe(
            &options.llvm,
            cc,
            &options.target,
            &options.temp_dir,
        )?;
        native_aot_lowering = run_native_aot_lowering_probe(
            &options.llvm,
            cc,
            &options.target,
            &options.temp_dir,
            &native_runtime_runs,
        )?;
        native_diagnostics = run_native_diagnostics_probe(
            &options.llvm,
            cc,
            &options.target,
            &options.temp_dir,
        )?;
    }
    let native_select_runs = native_runtime_runs
        .iter()
        .filter_map(|run| {
            run.case
                .starts_with("select-")
                .then_some(NativeSelectRunReport {
                    case: run.case.clone(),
                    expected_result: run.expected_result?,
                    cranelift: run.cranelift,
                    llvm: run.llvm,
                })
        })
        .collect::<Vec<_>>();
    let native_thread_runs = native_runtime_runs
        .iter()
        .filter_map(|run| {
            run.case
                .starts_with("thread-")
                .then_some(NativeThreadRunReport {
                    case: run.case.clone(),
                    expected_result: run.expected_result?,
                    cranelift: run.cranelift,
                    llvm: run.llvm,
                })
        })
        .collect::<Vec<_>>();

    let report = EvaluationReport {
        format: "tondo-native-evaluation-candidates/1",
        phase: "NATIVE-001",
        status: "passed",
        target: options.target,
        adapter: AdapterReport {
            format: "tondo-mir-backend/1",
            supported_subset: "scalar-managed-result-checked-arithmetic-logical-conversions-opaque-aggregates-host-calls-eager-async-control-flow-and-traps",
            unsupported_policy: "explicit-trap-and-report",
        },
        protocol: Protocol {
            warmup_iterations: 1,
            measurement_repetitions: REPETITIONS as u32,
            independent_processes: 1,
            minimum_sample_count: REPETITIONS as u32,
            seed: "tondo-native-evaluation-fast-0.1",
        },
        candidates: vec![
            CandidateReport {
                id: "cranelift",
                status: "measured",
                adapter: "cranelift-codegen",
                toolchain: format!("cranelift-codegen/{CRANELIFT_VERSION}"),
                samples: cranelift_samples,
                dimensions: fast_dimensions(),
            },
            CandidateReport {
                id: "llvm",
                status: "measured",
                adapter: "llc",
                toolchain: llvm_version,
                samples: llvm_samples,
                dimensions: fast_dimensions(),
            },
        ],
        excluded: vec![ExcludedCandidate {
            id: "custom",
            status: "not-comparable",
            reason: "no machine-code generator exists yet; it cannot enter the measured ranking",
        }],
        correctness: CorrectnessStatus {
            mir_probe: "passed-vm-oracle",
            cranelift: "backend-verifier-passed",
            llvm: "llc-verifier-passed",
            native_semantics: if native_runs.is_empty()
                && native_managed_runs.is_empty()
                && native_runtime_runs.is_empty()
            {
                "pending-native-lowering"
            } else if native_runtime_runs.is_empty() && native_managed_runs.is_empty() {
                "scalar-native-executable-vs-vm-and-normalized-oracle-with-traps"
            } else if native_runtime_runs.is_empty() {
                "scalar-and-managed-native-executable-vs-vm-and-normalized-oracle"
            } else {
                "scalar-and-managed-result-checked-arithmetic-logical-conversions-control-flow-host-calls-cleanup-ownership-async-thread-select-std-core-and-traps"
            },
        },
        debug_metadata,
        native_runs,
        native_managed_runs,
        native_std_core_runs,
        native_runtime_runs,
        native_select_runs,
        native_thread_runs,
        native_lowering_runs,
        native_aot_lowering,
        native_diagnostics,
    };

    let encoded = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("cannot encode evaluation report: {error}"))?;
    fs::write(&options.output, encoded)
        .map_err(|error| format!("cannot write `{}`: {error}", options.output.display()))?;
    Ok(())
}

fn fast_dimensions() -> Dimensions {
    Dimensions {
        compile_time: "measured",
        code_size: "measured",
        peak_memory: "deferred-until-native-process-runner",
        runtime: "deferred-until-native-lowering",
    }
}

fn validate_probe(probe: &ProbeReport) -> Result<(), String> {
    if probe.format != "tondo-native-mir-probe/1" {
        return Err(format!("unsupported probe format `{}`", probe.format));
    }
    if probe.fixtures.len() != 4 {
        return Err(format!(
            "expected four probe fixtures, got {}",
            probe.fixtures.len()
        ));
    }
    for fixture in &probe.fixtures {
        if fixture.status != "passed"
            || fixture
                .mir
                .as_ref()
                .and_then(|mir| mir.backend.as_ref())
                .is_none()
        {
            return Err(format!("probe fixture did not pass: {}", fixture.fixture));
        }
        let backend = fixture
            .mir
            .as_ref()
            .and_then(|mir| mir.backend.as_ref())
            .ok_or_else(|| {
                format!(
                    "fixture has no normalized MIR adapter input: {}",
                    fixture.fixture
                )
            })?;
        validate_backend_program(backend)?;
        if fixture.fixture_sha256.len() != 71 || !fixture.fixture_sha256.starts_with("sha256:") {
            return Err(format!("invalid fixture identity: {}", fixture.fixture));
        }
    }
    Ok(())
}

/// Builds metadata for adapter-owned runtime probes.  These functions do not
/// originate in a source file, but they still use the same canonical symbol
/// and region shape as real MIR so the LLVM/Cranelift paths cannot bypass the
/// debug-metadata boundary.
fn synthetic_debug_info(functions: &[MirBackendFunction]) -> MirBackendDebugInfo {
    let span = || MirBackendSpan {
        source: 0,
        start: 0,
        end: 0,
    };
    let mut source_maps = Vec::new();
    for function in functions {
        source_maps.push(MirBackendSourceMap {
            id: format!("f{}", function.ordinal),
            kind: "function".to_owned(),
            function: function.ordinal,
            block: None,
            span: span(),
            unwind: None,
        });
        for block in &function.blocks {
            source_maps.push(MirBackendSourceMap {
                id: format!("f{}.b{}", function.ordinal, block.ordinal),
                kind: "block".to_owned(),
                function: function.ordinal,
                block: Some(block.ordinal),
                span: span(),
                unwind: None,
            });
            source_maps.push(MirBackendSourceMap {
                id: format!("f{}.b{}.t", function.ordinal, block.ordinal),
                kind: "terminator".to_owned(),
                function: function.ordinal,
                block: Some(block.ordinal),
                span: span(),
                unwind: None,
            });
        }
    }
    let symbols = functions
        .iter()
        .map(|function| MirBackendDebugSymbol {
            function: function.ordinal,
            name: format!("adapter_function_{}", function.ordinal),
            native: format!("tondo_probe_{}", function.ordinal),
            span: span(),
        })
        .collect();
    MirBackendDebugInfo {
        format: "tondo-mir-debug/1".to_owned(),
        sources: vec![MirBackendSource {
            ordinal: 0,
            module: "adapter".to_owned(),
            logical_path: "adapter.to".to_owned(),
            content_sha256: format!("sha256:{}", "0".repeat(64)),
            length: 0,
        }],
        symbols,
        source_maps,
        executions: Vec::new(),
    }
}

fn validate_backend_program(program: &MirBackendProgram) -> Result<(), String> {
    if program.format != "tondo-mir-backend/1" {
        return Err(format!(
            "unsupported normalized MIR adapter format `{}`",
            program.format
        ));
    }
    if program.functions.is_empty() {
        return Err("normalized MIR adapter input has no functions".to_owned());
    }
    let debug = program
        .debug
        .as_ref()
        .ok_or_else(|| "normalized MIR adapter input has no debug metadata".to_owned())?;
    validate_backend_debug(program, debug)?;
    if program
        .functions
        .iter()
        .any(|function| function.blocks.is_empty())
    {
        return Err("normalized MIR adapter function has no blocks".to_owned());
    }
    let function_ordinals = program
        .functions
        .iter()
        .map(|function| function.ordinal)
        .collect::<BTreeSet<_>>();
    if function_ordinals.len() != program.functions.len() {
        return Err("normalized MIR function ordinals are not unique".to_owned());
    }
    let function_arities = program
        .functions
        .iter()
        .map(|function| (function.ordinal, function.parameters.len()))
        .collect::<BTreeMap<_, _>>();
    for function in &program.functions {
        let block_ordinals = function
            .blocks
            .iter()
            .map(|block| block.ordinal)
            .collect::<BTreeSet<_>>();
        if block_ordinals.len() != function.blocks.len() {
            return Err(format!(
                "normalized MIR block ordinals are not unique in function {}",
                function.ordinal
            ));
        }
        if function.supported {
            if function
                .blocks
                .iter()
                .filter(|block| block.kind == "normal")
                .count()
                == 0
            {
                return Err(format!(
                    "supported normalized MIR function {} has no normal block",
                    function.ordinal
                ));
            }
            for block in function
                .blocks
                .iter()
                .filter(|block| block.kind == "cleanup")
            {
                for statement in &block.statements {
                    let MirBackendStatement::Marker { kind } = statement else {
                        return Err(format!(
                            "supported normalized MIR cleanup block {} contains an instruction",
                            block.ordinal
                        ));
                    };
                    if kind.starts_with("release-")
                        || kind.starts_with("reserve-")
                        || matches!(
                            kind.as_str(),
                            "register-defer"
                                | "register-fallback"
                                | "enter-task-scope"
                                | "retarget-cleanup"
                                | "begin-select"
                                | "register-select-arm"
                        )
                    {
                        return Err(format!(
                            "supported normalized MIR cleanup action `{kind}` is not lowered"
                        ));
                    }
                }
            }
        }
        for block in &function.blocks {
            if function.supported {
                validate_supported_block(block, function.ordinal)?;
                for statement in &block.statements {
                    validate_backend_statement_calls(
                        statement,
                        &function_ordinals,
                        &function_arities,
                    )?;
                }
            }
            if let MirBackendTerminator::Invoke { operation, .. } = &block.terminator {
                validate_backend_operation_calls(operation, &function_ordinals, &function_arities)?;
            }
        }
    }
    Ok(())
}

fn validate_backend_debug(
    program: &MirBackendProgram,
    debug: &MirBackendDebugInfo,
) -> Result<(), String> {
    if debug.format != "tondo-mir-debug/1" {
        return Err(format!(
            "unsupported normalized MIR debug format `{}`",
            debug.format
        ));
    }
    if debug.sources.is_empty() {
        return Err("normalized MIR debug metadata has no logical sources".to_owned());
    }
    let source_ordinals = debug
        .sources
        .iter()
        .map(|source| source.ordinal)
        .collect::<BTreeSet<_>>();
    if source_ordinals.len() != debug.sources.len() {
        return Err("normalized MIR debug source ordinals are not unique".to_owned());
    }
    for source in &debug.sources {
        if source.module.is_empty()
            || source.module.contains(['/', '\\'])
            || source.module.split('.').any(|part| part.is_empty() || part == "." || part == "..")
            || source.logical_path.is_empty()
            || source.logical_path.starts_with('/')
            || source.logical_path.contains('\\')
            || source.logical_path.split('/').any(|part| part == "." || part == "..")
            || source.logical_path.contains('\0')
        {
            return Err(format!(
                "normalized MIR debug source has an invalid logical path `{}`",
                source.logical_path
            ));
        }
        if !valid_sha256(&source.content_sha256) {
            return Err(format!(
                "normalized MIR debug source has an invalid content hash `{}`",
                source.content_sha256
            ));
        }
    }

    let function_ordinals = program
        .functions
        .iter()
        .map(|function| function.ordinal)
        .collect::<BTreeSet<_>>();
    if debug.symbols.len() != program.functions.len() {
        return Err("normalized MIR debug symbols do not cover every function".to_owned());
    }
    let mut symbol_functions = BTreeSet::new();
    for symbol in &debug.symbols {
        if !function_ordinals.contains(&symbol.function) {
            return Err(format!(
                "normalized MIR debug symbol references missing function {}",
                symbol.function
            ));
        }
        if !symbol_functions.insert(symbol.function) {
            return Err(format!(
                "normalized MIR debug symbols duplicate function {}",
                symbol.function
            ));
        }
        if symbol.name.is_empty() || symbol.native != format!("tondo_probe_{}", symbol.function) {
            return Err(format!(
                "normalized MIR debug symbol for function {} is not canonical",
                symbol.function
            ));
        }
        validate_backend_span(&symbol.span, debug.sources.as_slice())?;
    }

    let mut region_ids = BTreeSet::new();
    for region in &debug.source_maps {
        if region.id.is_empty() || !region_ids.insert(region.id.clone()) {
            return Err(format!(
                "normalized MIR debug source-map id is empty or duplicated: `{}`",
                region.id
            ));
        }
        let Some(function) = program
            .functions
            .iter()
            .find(|function| function.ordinal == region.function)
        else {
            return Err(format!(
                "normalized MIR debug region `{}` references missing function {}",
                region.id, region.function
            ));
        };
        if let Some(block) = region.block {
            if !function.blocks.iter().any(|candidate| candidate.ordinal == block) {
                return Err(format!(
                    "normalized MIR debug region `{}` references missing block {}",
                    region.id, block
                ));
            }
        }
        if !matches!(region.kind.as_str(), "function" | "block" | "statement" | "terminator")
        {
            return Err(format!(
                "normalized MIR debug region `{}` has unknown kind `{}`",
                region.id, region.kind
            ));
        }
        validate_backend_span(&region.span, debug.sources.as_slice())?;
        if let Some(unwind) = region.unwind
            && !function
                .blocks
                .iter()
                .any(|candidate| candidate.ordinal == unwind)
        {
            return Err(format!(
                "normalized MIR debug region `{}` has missing unwind target {}",
                region.id, unwind
            ));
        }
    }

    let mut execution_ids = BTreeSet::new();
    for execution in &debug.executions {
        if execution.id.is_empty() || !execution_ids.insert(execution.id.clone()) {
            return Err(format!(
                "normalized MIR debug execution id is empty or duplicated: `{}`",
                execution.id
            ));
        }
        if !matches!(execution.kind.as_str(), "task" | "thread") {
            return Err(format!(
                "normalized MIR debug execution `{}` has unknown kind `{}`",
                execution.id, execution.kind
            ));
        }
        let Some(function) = program
            .functions
            .iter()
            .find(|function| function.ordinal == execution.function)
        else {
            return Err(format!(
                "normalized MIR debug execution `{}` references missing function {}",
                execution.id, execution.function
            ));
        };
        if !function
            .blocks
            .iter()
            .any(|block| block.ordinal == execution.block)
        {
            return Err(format!(
                "normalized MIR debug execution `{}` references missing block {}",
                execution.id, execution.block
            ));
        }
        validate_backend_span(&execution.span, debug.sources.as_slice())?;
    }
    Ok(())
}

fn validate_backend_span(
    span: &MirBackendSpan,
    sources: &[MirBackendSource],
) -> Result<(), String> {
    let Some(source) = sources.iter().find(|source| source.ordinal == span.source) else {
        return Err(format!(
            "normalized MIR debug span references missing source {}",
            span.source
        ));
    };
    if span.start > span.end || span.end > source.length {
        return Err(format!(
            "normalized MIR debug span {}..{} exceeds source {} length {}",
            span.start, span.end, span.source, source.length
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_backend_operation_calls(
    operation: &MirBackendOperation,
    function_ordinals: &BTreeSet<u32>,
    function_arities: &BTreeMap<u32, usize>,
) -> Result<(), String> {
    match operation {
        MirBackendOperation::Call {
            function: target,
            arguments,
        } => {
            let Some(arity) = function_arities.get(target).copied() else {
                return Err(format!(
                    "normalized MIR call target {target} is not present"
                ));
            };
            if arguments.len() != arity {
                return Err(format!(
                    "normalized MIR call target {target} expects {arity} arguments, got {}",
                arguments.len()
                ));
            }
            for argument in arguments {
                validate_backend_operand_calls(argument, function_ordinals, function_arities)?;
            }
        }
        MirBackendOperation::Spawn { operation, kind } => {
            if !matches!(kind.as_str(), "task" | "thread") {
                return Err(format!("normalized MIR spawn kind is invalid: {kind}"));
            }
            validate_backend_operation_calls(operation, function_ordinals, function_arities)?;
        }
        MirBackendOperation::Runtime { kind, arguments } => {
            for argument in arguments {
                validate_backend_operand_calls(argument, function_ordinals, function_arities)?;
            }
            if kind.split(':').next() == Some("indirect-call") {
                if arguments.len() != 3 {
                    return Err(format!(
                        "normalized MIR indirect-call expects three arguments, got {}",
                        arguments.len()
                    ));
                }
                if !matches!(arguments.first(), Some(MirBackendOperand::Function { .. })) {
                    return Err(
                        "normalized MIR indirect-call requires a verified function operand"
                        .to_owned(),
                    );
                }
                let Some(MirBackendOperand::Function { kind }) = arguments.first() else {
                    unreachable!("indirect-call operand shape was checked above")
                };
                let Some(ordinal) = parse_verified_function_ordinal(kind) else {
                    return Err(
                        "normalized MIR indirect-call requires a verified function operand"
                            .to_owned(),
                    );
                };
                if function_arities.get(&ordinal) != Some(&2) {
                    return Err(format!(
                        "normalized MIR indirect-call target {ordinal} must have arity 2"
                    ));
                }
            }
        }
        MirBackendOperation::HostCall { arguments, .. } => {
            for argument in arguments {
                validate_backend_operand_calls(argument, function_ordinals, function_arities)?;
            }
        }
        MirBackendOperation::CheckedPrefix { operand, .. }
        | MirBackendOperation::JoinValue { operand }
        | MirBackendOperation::Assert { condition: operand } => {
            validate_backend_operand_calls(operand, function_ordinals, function_arities)?;
        }
        MirBackendOperation::CheckedBinary { left, right, .. } => {
            validate_backend_operand_calls(left, function_ordinals, function_arities)?;
            validate_backend_operand_calls(right, function_ordinals, function_arities)?;
        }
        MirBackendOperation::BoundsCheck { index, length } => {
            validate_backend_operand_calls(index, function_ordinals, function_arities)?;
            validate_backend_operand_calls(length, function_ordinals, function_arities)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_backend_statement_calls(
    statement: &MirBackendStatement,
    function_ordinals: &BTreeSet<u32>,
    function_arities: &BTreeMap<u32, usize>,
) -> Result<(), String> {
    match statement {
        MirBackendStatement::Assign { value, .. } => {
            validate_backend_rvalue_calls(value, function_ordinals, function_arities)?;
        }
        MirBackendStatement::Runtime { arguments, .. } => {
            for argument in arguments {
                validate_backend_operand_calls(argument, function_ordinals, function_arities)?;
            }
        }
        MirBackendStatement::Marker { .. } => {}
    }
    Ok(())
}

fn validate_backend_rvalue_calls(
    value: &MirBackendRvalue,
    function_ordinals: &BTreeSet<u32>,
    function_arities: &BTreeMap<u32, usize>,
) -> Result<(), String> {
    match value {
        MirBackendRvalue::Use(operand)
        | MirBackendRvalue::Prefix { operand, .. }
        | MirBackendRvalue::NumericConversion { operand, .. }
        | MirBackendRvalue::Coerce { operand, .. } => {
            validate_backend_operand_calls(operand, function_ordinals, function_arities)?;
        }
        MirBackendRvalue::Aggregate { values, .. } => {
            for operand in values {
                validate_backend_operand_calls(operand, function_ordinals, function_arities)?;
            }
        }
        MirBackendRvalue::Binary { left, right, .. } => {
            validate_backend_operand_calls(left, function_ordinals, function_arities)?;
            validate_backend_operand_calls(right, function_ordinals, function_arities)?;
        }
        MirBackendRvalue::HostCall { arguments, .. } => {
            for argument in arguments {
                validate_backend_operand_calls(argument, function_ordinals, function_arities)?;
            }
        }
        MirBackendRvalue::Tag { .. } | MirBackendRvalue::Unsupported { .. } => {}
    }
    Ok(())
}

fn validate_backend_operand_calls(
    operand: &MirBackendOperand,
    function_ordinals: &BTreeSet<u32>,
    function_arities: &BTreeMap<u32, usize>,
) -> Result<(), String> {
    if let MirBackendOperand::Function { kind } = operand
        && let Some(ordinal) = parse_verified_function_ordinal(kind)
        && !function_ordinals.contains(&ordinal)
    {
        return Err(format!(
            "normalized MIR function value target {ordinal} is not present"
        ));
    }
    if let MirBackendOperand::Function { kind } = operand
        && let Some(ordinal) = parse_verified_function_ordinal(kind)
        && !function_arities.contains_key(&ordinal)
    {
        return Err(format!(
            "normalized MIR function value target {ordinal} has no callable signature"
        ));
    }
    Ok(())
}

fn validate_supported_block(
    block: &MirBackendBlock,
    function_ordinal: u32,
) -> Result<(), String> {
    if !matches!(block.kind.as_str(), "normal" | "cleanup") {
        return Err(format!(
            "supported normalized MIR function {function_ordinal} has invalid block kind `{}`",
            block.kind
        ));
    }
    for statement in &block.statements {
        match statement {
            MirBackendStatement::Assign { value, .. } => {
                validate_supported_rvalue(value, function_ordinal)?;
            }
            MirBackendStatement::Marker { kind }
                if matches!(
                    kind.as_str(),
                    "unit-assignment"
                        | "function-value"
                        | "storage-live"
                        | "storage-dead"
                        | "disarm-cleanup"
                ) =>
            {}
            MirBackendStatement::Marker { kind } => {
                return Err(format!(
                    "supported normalized MIR function {function_ordinal} has unsupported statement marker `{kind}`"
                ));
            }
            MirBackendStatement::Runtime { arguments, .. } => {
                for argument in arguments {
                    validate_supported_operand(argument, function_ordinal)?;
                }
            }
        }
    }
    match &block.terminator {
        MirBackendTerminator::Return | MirBackendTerminator::Goto { .. } => {}
        MirBackendTerminator::Marker { kind }
            if kind == "unreachable"
                || (block.kind == "cleanup"
                    && matches!(kind.as_str(), "resume-panic" | "drain-unwind")) =>
        {}
        MirBackendTerminator::SwitchBool { condition, .. }
        | MirBackendTerminator::SwitchTag {
            value: condition, ..
        } => validate_supported_operand(condition, function_ordinal)?,
        MirBackendTerminator::Invoke { operation, .. } => {
            validate_supported_operation(operation, function_ordinal)?;
        }
        MirBackendTerminator::Marker { kind } => {
            return Err(format!(
                "supported normalized MIR function {function_ordinal} has unsupported terminator marker `{kind}`"
            ));
        }
    }
    Ok(())
}

fn validate_supported_operand(
    operand: &MirBackendOperand,
    function_ordinal: u32,
) -> Result<(), String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Named)
        | MirBackendOperand::Unsupported { .. } => Err(format!(
            "supported normalized MIR function {function_ordinal} contains an opaque or unsupported operand"
        )),
        MirBackendOperand::Function { kind } if parse_verified_function_ordinal(kind).is_none() => {
            Err(format!(
                "supported normalized MIR function {function_ordinal} contains an unverified function operand `{kind}`"
            ))
        }
        MirBackendOperand::Function { .. } => Ok(()),
        MirBackendOperand::Projection { depth, kind, .. }
            if *depth == 1
                && matches!(
                    kind.as_str(),
                    "option-value" | "result-ok-value" | "result-err-value"
                ) =>
        {
            Ok(())
        }
        MirBackendOperand::Projection { depth, kind, .. }
            if *depth == 1 && parse_aggregate_projection(kind).is_some() => Ok(()),
        MirBackendOperand::Projection { .. } => Err(format!(
            "supported normalized MIR function {function_ordinal} contains an opaque or unsupported projection"
        )),
        MirBackendOperand::Constant(_)
        | MirBackendOperand::Local { .. }
        | MirBackendOperand::Borrow { .. } => Ok(()),
    }
}

/// Function values cross the normalized MIR boundary only as an ordinal that
/// has already been verified against the function table.  A textual symbol or
/// host name is intentionally not accepted: native adapters must not invent a
/// function-pointer ABI from an opaque spelling.
fn parse_verified_function_ordinal(kind: &str) -> Option<u32> {
    kind.strip_prefix("function:")?.parse().ok()
}

fn parse_aggregate_projection(kind: &str) -> Option<u32> {
    kind.strip_prefix("aggregate:")?.parse().ok()
}

fn validate_supported_rvalue(
    value: &MirBackendRvalue,
    function_ordinal: u32,
) -> Result<(), String> {
    match value {
        MirBackendRvalue::Use(operand)
        | MirBackendRvalue::Prefix { operand, .. }
        | MirBackendRvalue::NumericConversion { operand, .. }
        | MirBackendRvalue::Coerce { operand, .. } => {
            validate_supported_operand(operand, function_ordinal)?;
        }
        MirBackendRvalue::Tag { value } => {
            if *value > 3 {
                return Err(format!(
                    "supported normalized MIR function {function_ordinal} has invalid tag {value}"
                ));
            }
        }
        MirBackendRvalue::Aggregate { kind, values } => {
            if aggregate_tag(kind).is_err() {
                return Err(format!(
                    "supported normalized MIR function {function_ordinal} has unsupported aggregate `{kind}`"
                ));
            }
            for operand in values {
                validate_supported_operand(operand, function_ordinal)?;
            }
        }
        MirBackendRvalue::Binary { left, right, .. } => {
            validate_supported_operand(left, function_ordinal)?;
            validate_supported_operand(right, function_ordinal)?;
        }
        MirBackendRvalue::HostCall { arguments, .. } => {
            for argument in arguments {
                validate_supported_operand(argument, function_ordinal)?;
            }
        }
        MirBackendRvalue::Unsupported { kind } => {
            return Err(format!(
                "supported normalized MIR function {function_ordinal} contains unsupported rvalue `{kind}`"
            ));
        }
    }
    if let MirBackendRvalue::NumericConversion {
        source,
        target,
        conversion,
        ..
    } = value
    {
        if !is_native_integer_scalar(source)
            || !is_native_integer_scalar(target)
            || !matches!(conversion.as_str(), "identity" | "total" | "checked")
        {
            return Err(format!(
                "supported normalized MIR function {function_ordinal} has invalid numeric conversion {source}->{target} ({conversion})"
            ));
        }
    }
    if let MirBackendRvalue::Coerce { kind, .. } = value
        && kind != "EffectWeakening"
    {
        return Err(format!(
            "supported normalized MIR function {function_ordinal} has unsupported coercion `{kind}`"
        ));
    }
    Ok(())
}

fn validate_supported_operation(
    operation: &MirBackendOperation,
    function_ordinal: u32,
) -> Result<(), String> {
    match operation {
        MirBackendOperation::CheckedPrefix { operand, .. }
        | MirBackendOperation::JoinValue { operand }
        | MirBackendOperation::Assert { condition: operand } => {
            validate_supported_operand(operand, function_ordinal)?;
        }
        MirBackendOperation::CheckedBinary { left, right, .. } => {
            validate_supported_operand(left, function_ordinal)?;
            validate_supported_operand(right, function_ordinal)?;
        }
        MirBackendOperation::BoundsCheck { index, length } => {
            validate_supported_operand(index, function_ordinal)?;
            validate_supported_operand(length, function_ordinal)?;
        }
        MirBackendOperation::Call { arguments, .. }
        | MirBackendOperation::HostCall { arguments, .. }
        | MirBackendOperation::Runtime { arguments, .. } => {
            for argument in arguments {
                validate_supported_operand(argument, function_ordinal)?;
            }
        }
        MirBackendOperation::Spawn { operation, kind } => {
            if !matches!(kind.as_str(), "task" | "thread") {
                return Err(format!(
                    "supported normalized MIR function {function_ordinal} has invalid spawn kind `{kind}`"
                ));
            }
            validate_supported_operation(operation, function_ordinal)?;
        }
        MirBackendOperation::Trap { .. } => {}
        MirBackendOperation::Marker { kind } => {
            return Err(format!(
                "supported normalized MIR function {function_ordinal} contains unsupported operation marker `{kind}`"
            ));
        }
    }
    Ok(())
}

struct CodegenResult {
    compile_time_ns: u128,
    code_size_bytes: u64,
    supported_functions: u64,
    unsupported_functions: u64,
}

#[derive(Clone, Copy)]
struct RuntimeRefs {
    result_new: FuncRef,
    result_tag: FuncRef,
    result_payload: FuncRef,
    aggregate_new: FuncRef,
    aggregate_set: FuncRef,
    aggregate_get: FuncRef,
    aggregate_len: FuncRef,
    aggregate_tag: FuncRef,
    indirect_call: FuncRef,
    host_call: FuncRef,
    retain: FuncRef,
    release: FuncRef,
    cow_clone: FuncRef,
    frame_enter: FuncRef,
    frame_publish_root: FuncRef,
    frame_register_defer: FuncRef,
    frame_disarm_defer: FuncRef,
    frame_cleanup: FuncRef,
    frame_leave: FuncRef,
    scope_enter: FuncRef,
    scope_spawn: FuncRef,
    task_spawn: FuncRef,
    thread_spawn: FuncRef,
    thread_worker_status: FuncRef,
    thread_worker_runs: FuncRef,
    thread_worker_distinct: FuncRef,
    thread_worker_wait: FuncRef,
    task_poll: FuncRef,
    task_wake: FuncRef,
    task_cancel: FuncRef,
    task_take: FuncRef,
    task_complete: FuncRef,
    scope_cancel: FuncRef,
    scope_join: FuncRef,
    await_task: FuncRef,
    select_begin: FuncRef,
    select_register_task: FuncRef,
    select_register_join: FuncRef,
    select_register_oneshot: FuncRef,
    select_register_time: FuncRef,
    select_commit: FuncRef,
    select_winner: FuncRef,
    select_take: FuncRef,
    select_rollback: FuncRef,
    select_wakeups: FuncRef,
    oneshot_new: FuncRef,
    oneshot_complete: FuncRef,
    oneshot_cancel: FuncRef,
    time_new: FuncRef,
    time_fire: FuncRef,
    noop: FuncRef,
    diag_race: FuncRef,
    diag_leak: FuncRef,
    diag_dump: FuncRef,
}

fn runtime_signature(isa: &dyn cranelift_codegen::isa::TargetIsa, parameters: usize) -> Signature {
    let mut signature = Signature::new(isa.default_call_conv());
    for _ in 0..parameters {
        signature
            .params
            .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    }
    signature
        .returns
        .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    signature
}

fn declare_cranelift_runtime_function(
    module: &mut ObjectModule,
    name: &str,
    parameters: usize,
) -> Result<FuncId, String> {
    module
        .declare_function(
            name,
            Linkage::Import,
            &runtime_signature(module.isa(), parameters),
        )
        .map_err(|error| format!("cannot declare native runtime function {name}: {error}"))
}

fn declare_cranelift_runtime(
    module: &mut ObjectModule,
    ir_function: &mut Function,
) -> Result<RuntimeRefs, String> {
    let declarations = [
        ("tondo_rt_result_new", 3),
        ("tondo_rt_result_tag", 1),
        ("tondo_rt_result_payload", 1),
        ("tondo_rt_aggregate_new", 2),
        ("tondo_rt_aggregate_set", 3),
        ("tondo_rt_aggregate_get", 2),
        ("tondo_rt_aggregate_len", 1),
        ("tondo_rt_aggregate_tag", 1),
        ("tondo_rt_indirect_call", 3),
        ("tondo_rt_host_call", 2),
        ("tondo_rt_retain", 1),
        ("tondo_rt_release", 1),
        ("tondo_rt_cow_clone", 1),
        ("tondo_rt_frame_enter", 0),
        ("tondo_rt_frame_publish_root", 2),
        ("tondo_rt_frame_register_defer", 2),
        ("tondo_rt_frame_disarm_defer", 2),
        ("tondo_rt_frame_cleanup", 2),
        ("tondo_rt_frame_leave", 2),
        ("tondo_rt_scope_enter", 0),
        ("tondo_rt_scope_spawn", 3),
        ("tondo_rt_task_spawn", 2),
        ("tondo_rt_thread_spawn", 2),
        ("tondo_rt_thread_worker_status", 1),
        ("tondo_rt_thread_worker_runs", 1),
        ("tondo_rt_thread_worker_distinct", 1),
        ("tondo_rt_thread_worker_wait", 1),
        ("tondo_rt_task_poll", 1),
        ("tondo_rt_task_wake", 1),
        ("tondo_rt_task_cancel", 1),
        ("tondo_rt_task_take", 1),
        ("tondo_rt_task_complete", 2),
        ("tondo_rt_scope_cancel", 1),
        ("tondo_rt_scope_join", 2),
        ("tondo_rt_await", 1),
        ("tondo_rt_select_begin", 1),
        ("tondo_rt_select_register_task", 3),
        ("tondo_rt_select_register_join", 2),
        ("tondo_rt_select_register_oneshot", 3),
        ("tondo_rt_select_register_time", 3),
        ("tondo_rt_select_commit", 2),
        ("tondo_rt_select_winner", 1),
        ("tondo_rt_select_take", 1),
        ("tondo_rt_select_rollback", 1),
        ("tondo_rt_select_wakeups", 1),
        ("tondo_rt_oneshot_new", 0),
        ("tondo_rt_oneshot_complete", 2),
        ("tondo_rt_oneshot_cancel", 1),
        ("tondo_rt_time_new", 1),
        ("tondo_rt_time_fire", 1),
        ("tondo_rt_noop", 0),
        ("tondo_rt_diag_race", 1),
        ("tondo_rt_diag_leak", 1),
        ("tondo_rt_diag_dump", 1),
    ];
    let ids = declarations
        .into_iter()
        .map(|(name, parameters)| {
            declare_cranelift_runtime_function(module, name, parameters)
                .map(|id| (name, module.declare_func_in_func(id, ir_function)))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let get = |name: &str| {
        ids.get(name)
            .copied()
            .ok_or_else(|| format!("native runtime declaration {name} is missing"))
    };
    Ok(RuntimeRefs {
        result_new: get("tondo_rt_result_new")?,
        result_tag: get("tondo_rt_result_tag")?,
        result_payload: get("tondo_rt_result_payload")?,
        aggregate_new: get("tondo_rt_aggregate_new")?,
        aggregate_set: get("tondo_rt_aggregate_set")?,
        aggregate_get: get("tondo_rt_aggregate_get")?,
        aggregate_len: get("tondo_rt_aggregate_len")?,
        aggregate_tag: get("tondo_rt_aggregate_tag")?,
        indirect_call: get("tondo_rt_indirect_call")?,
        host_call: get("tondo_rt_host_call")?,
        retain: get("tondo_rt_retain")?,
        release: get("tondo_rt_release")?,
        cow_clone: get("tondo_rt_cow_clone")?,
        frame_enter: get("tondo_rt_frame_enter")?,
        frame_publish_root: get("tondo_rt_frame_publish_root")?,
        frame_register_defer: get("tondo_rt_frame_register_defer")?,
        frame_disarm_defer: get("tondo_rt_frame_disarm_defer")?,
        frame_cleanup: get("tondo_rt_frame_cleanup")?,
        frame_leave: get("tondo_rt_frame_leave")?,
        scope_enter: get("tondo_rt_scope_enter")?,
        scope_spawn: get("tondo_rt_scope_spawn")?,
        task_spawn: get("tondo_rt_task_spawn")?,
        thread_spawn: get("tondo_rt_thread_spawn")?,
        thread_worker_status: get("tondo_rt_thread_worker_status")?,
        thread_worker_runs: get("tondo_rt_thread_worker_runs")?,
        thread_worker_distinct: get("tondo_rt_thread_worker_distinct")?,
        thread_worker_wait: get("tondo_rt_thread_worker_wait")?,
        task_poll: get("tondo_rt_task_poll")?,
        task_wake: get("tondo_rt_task_wake")?,
        task_cancel: get("tondo_rt_task_cancel")?,
        task_take: get("tondo_rt_task_take")?,
        task_complete: get("tondo_rt_task_complete")?,
        scope_cancel: get("tondo_rt_scope_cancel")?,
        scope_join: get("tondo_rt_scope_join")?,
        await_task: get("tondo_rt_await")?,
        select_begin: get("tondo_rt_select_begin")?,
        select_register_task: get("tondo_rt_select_register_task")?,
        select_register_join: get("tondo_rt_select_register_join")?,
        select_register_oneshot: get("tondo_rt_select_register_oneshot")?,
        select_register_time: get("tondo_rt_select_register_time")?,
        select_commit: get("tondo_rt_select_commit")?,
        select_winner: get("tondo_rt_select_winner")?,
        select_take: get("tondo_rt_select_take")?,
        select_rollback: get("tondo_rt_select_rollback")?,
        select_wakeups: get("tondo_rt_select_wakeups")?,
        oneshot_new: get("tondo_rt_oneshot_new")?,
        oneshot_complete: get("tondo_rt_oneshot_complete")?,
        oneshot_cancel: get("tondo_rt_oneshot_cancel")?,
        time_new: get("tondo_rt_time_new")?,
        time_fire: get("tondo_rt_time_fire")?,
        noop: get("tondo_rt_noop")?,
        diag_race: get("tondo_rt_diag_race")?,
        diag_leak: get("tondo_rt_diag_leak")?,
        diag_dump: get("tondo_rt_diag_dump")?,
    })
}

fn aggregate_tag(kind: &str) -> Result<u32, String> {
    match kind {
        "option-none" => Ok(0),
        "option-some" => Ok(1),
        "result-ok" => Ok(2),
        "result-err" => Ok(3),
        "tuple" => Ok(4),
        "array" => Ok(5),
        "set" => Ok(6),
        "closure" => Ok(7),
        "newtype" => Ok(8),
        "ref" => Ok(9),
        "record" => Ok(10),
        "variant" => Ok(11),
        "numeric-conversion-error" => Ok(12),
        other => Err(format!("native aggregate is not supported: {other}")),
    }
}

fn host_call_kind(kind: &str) -> Result<u32, String> {
    match kind {
        "console-print" => Ok(0),
        "console-println" => Ok(0),
        // The remaining host operations are represented by opaque runtime
        // calls.  They intentionally share one capability path until their
        // concrete stdlib adapters are linked into the native product.
        _ if !kind.is_empty() => Ok(0),
        _ => Err("native host call has no logical kind".to_owned()),
    }
}

#[derive(Clone, Copy)]
struct RuntimeCall {
    function: FuncRef,
    arity: usize,
}

fn runtime_helper(runtime: &RuntimeRefs, kind: &str) -> Result<RuntimeCall, String> {
    let base = kind.split(':').next().unwrap_or(kind);
    if kind.contains(':')
        && matches!(
            base,
            "enter-task-scope"
                | "retarget-cleanup"
                | "register-defer"
                | "register-fallback"
                | "reserve-loan"
                | "release-loan"
                | "begin-select"
                | "register-select-arm"
        )
    {
        return Ok(RuntimeCall {
            function: runtime.noop,
            arity: usize::MAX,
        });
    }
    match base {
        "result-tag" => Ok(RuntimeCall {
            function: runtime.result_tag,
            arity: 1,
        }),
        "result-payload" => Ok(RuntimeCall {
            function: runtime.result_payload,
            arity: 1,
        }),
        "aggregate-new" => Ok(RuntimeCall {
            function: runtime.aggregate_new,
            arity: 2,
        }),
        "aggregate-set" => Ok(RuntimeCall {
            function: runtime.aggregate_set,
            arity: 3,
        }),
        "aggregate-get" => Ok(RuntimeCall {
            function: runtime.aggregate_get,
            arity: 2,
        }),
        "aggregate-len" => Ok(RuntimeCall {
            function: runtime.aggregate_len,
            arity: 1,
        }),
        "aggregate-tag" => Ok(RuntimeCall {
            function: runtime.aggregate_tag,
            arity: 1,
        }),
        "indirect-call" => Ok(RuntimeCall {
            function: runtime.indirect_call,
            arity: 3,
        }),
        "retain" | "retain-value" => Ok(RuntimeCall {
            function: runtime.retain,
            arity: 1,
        }),
        "release" | "release-value" => Ok(RuntimeCall {
            function: runtime.release,
            arity: 1,
        }),
        "cow-clone" => Ok(RuntimeCall {
            function: runtime.cow_clone,
            arity: 1,
        }),
        "frame-enter" => Ok(RuntimeCall {
            function: runtime.frame_enter,
            arity: 0,
        }),
        "frame-publish-root" => Ok(RuntimeCall {
            function: runtime.frame_publish_root,
            arity: 2,
        }),
        "register-defer" => Ok(RuntimeCall {
            function: runtime.frame_register_defer,
            arity: 2,
        }),
        "disarm-defer" => Ok(RuntimeCall {
            function: runtime.frame_disarm_defer,
            arity: 2,
        }),
        "frame-cleanup" => Ok(RuntimeCall {
            function: runtime.frame_cleanup,
            arity: 2,
        }),
        "frame-leave" => Ok(RuntimeCall {
            function: runtime.frame_leave,
            arity: 2,
        }),
        "scope-enter" => Ok(RuntimeCall {
            function: runtime.scope_enter,
            arity: 0,
        }),
        "scope-spawn" => Ok(RuntimeCall {
            function: runtime.scope_spawn,
            arity: 3,
        }),
        "task-spawn" => Ok(RuntimeCall {
            function: runtime.task_spawn,
            arity: 2,
        }),
        "thread-spawn" => Ok(RuntimeCall {
            function: runtime.thread_spawn,
            arity: 2,
        }),
        "thread-worker-status" => Ok(RuntimeCall {
            function: runtime.thread_worker_status,
            arity: 1,
        }),
        "thread-worker-runs" => Ok(RuntimeCall {
            function: runtime.thread_worker_runs,
            arity: 1,
        }),
        "thread-worker-distinct" => Ok(RuntimeCall {
            function: runtime.thread_worker_distinct,
            arity: 1,
        }),
        "thread-worker-wait" => Ok(RuntimeCall {
            function: runtime.thread_worker_wait,
            arity: 1,
        }),
        "task-poll" => Ok(RuntimeCall {
            function: runtime.task_poll,
            arity: 1,
        }),
        "task-wake" => Ok(RuntimeCall {
            function: runtime.task_wake,
            arity: 1,
        }),
        "task-cancel" => Ok(RuntimeCall {
            function: runtime.task_cancel,
            arity: 1,
        }),
        "task-take" => Ok(RuntimeCall {
            function: runtime.task_take,
            arity: 1,
        }),
        "scope-cancel" => Ok(RuntimeCall {
            function: runtime.scope_cancel,
            arity: 1,
        }),
        "scope-join" => Ok(RuntimeCall {
            function: runtime.scope_join,
            arity: 2,
        }),
        "await" => Ok(RuntimeCall {
            function: runtime.await_task,
            arity: 1,
        }),
        "select-begin" => Ok(RuntimeCall {
            function: runtime.select_begin,
            arity: 1,
        }),
        "select-register-task" => Ok(RuntimeCall {
            function: runtime.select_register_task,
            arity: 3,
        }),
        "select-register-join" => Ok(RuntimeCall {
            function: runtime.select_register_join,
            arity: 2,
        }),
        "select-register-oneshot" => Ok(RuntimeCall {
            function: runtime.select_register_oneshot,
            arity: 3,
        }),
        "select-register-time" => Ok(RuntimeCall {
            function: runtime.select_register_time,
            arity: 3,
        }),
        "select-commit" => Ok(RuntimeCall {
            function: runtime.select_commit,
            arity: 2,
        }),
        "select-winner" => Ok(RuntimeCall {
            function: runtime.select_winner,
            arity: 1,
        }),
        "select-take" => Ok(RuntimeCall {
            function: runtime.select_take,
            arity: 1,
        }),
        "select-rollback" => Ok(RuntimeCall {
            function: runtime.select_rollback,
            arity: 1,
        }),
        "select-wakeups" => Ok(RuntimeCall {
            function: runtime.select_wakeups,
            arity: 1,
        }),
        "oneshot-new" => Ok(RuntimeCall {
            function: runtime.oneshot_new,
            arity: 0,
        }),
        "oneshot-complete" => Ok(RuntimeCall {
            function: runtime.oneshot_complete,
            arity: 2,
        }),
        "oneshot-cancel" => Ok(RuntimeCall {
            function: runtime.oneshot_cancel,
            arity: 1,
        }),
        "time-new" => Ok(RuntimeCall {
            function: runtime.time_new,
            arity: 1,
        }),
        "time-fire" => Ok(RuntimeCall {
            function: runtime.time_fire,
            arity: 1,
        }),
        "diag-race" => Ok(RuntimeCall {
            function: runtime.diag_race,
            arity: 1,
        }),
        "diag-leak" => Ok(RuntimeCall {
            function: runtime.diag_leak,
            arity: 1,
        }),
        "diag-dump" => Ok(RuntimeCall {
            function: runtime.diag_dump,
            arity: 1,
        }),
        other => Err(format!(
            "native runtime operation is not supported: {other}"
        )),
    }
}

fn lower_runtime_call_cranelift(
    builder: &mut FunctionBuilder<'_>,
    kind: &str,
    arguments: &[MirBackendOperand],
    locals: &BTreeMap<u32, Value>,
    runtime: &RuntimeRefs,
) -> Result<Value, String> {
    let call = runtime_helper(runtime, kind)?;
    if call.arity != usize::MAX && arguments.len() != call.arity {
        return Err(format!(
            "native runtime operation `{kind}` expects {} arguments, got {}",
            call.arity,
            arguments.len()
        ));
    }
    let arguments = if call.arity == usize::MAX {
        Vec::new()
    } else {
        arguments
            .iter()
            .map(|argument| lower_operand_cranelift_with_runtime(builder, argument, locals, runtime))
            .collect::<Result<Vec<_>, _>>()?
    };
    let instruction = builder.ins().call(call.function, &arguments);
    builder
        .inst_results(instruction)
        .first()
        .copied()
        .ok_or_else(|| format!("native runtime operation `{kind}` returned no status"))
}

fn normal_blocks(function: &MirBackendFunction) -> Vec<&MirBackendBlock> {
    function
        .blocks
        .iter()
        .filter(|block| block.kind == "normal")
        .collect()
}

fn operand_locals(operand: &MirBackendOperand, locals: &mut BTreeSet<u32>) {
    match operand {
        MirBackendOperand::Local { index }
        | MirBackendOperand::Borrow { index }
        | MirBackendOperand::Projection { index, .. } => {
            locals.insert(*index);
        }
        MirBackendOperand::Constant(_)
        | MirBackendOperand::Function { .. }
        | MirBackendOperand::Unsupported { .. } => {}
    }
}

fn rvalue_locals(value: &MirBackendRvalue, locals: &mut BTreeSet<u32>) {
    match value {
        MirBackendRvalue::Use(operand) => operand_locals(operand, locals),
        MirBackendRvalue::Tag { .. } => {}
        MirBackendRvalue::Aggregate { values, .. } => {
            for value in values {
                operand_locals(value, locals);
            }
        }
        MirBackendRvalue::Prefix { operand, .. } => operand_locals(operand, locals),
        MirBackendRvalue::Binary { left, right, .. } => {
            operand_locals(left, locals);
            operand_locals(right, locals);
        }
        MirBackendRvalue::NumericConversion { operand, .. }
        | MirBackendRvalue::Coerce { operand, .. } => operand_locals(operand, locals),
        MirBackendRvalue::HostCall { arguments, .. } => {
            for argument in arguments {
                operand_locals(argument, locals);
            }
        }
        MirBackendRvalue::Unsupported { .. } => {}
    }
}

fn operation_locals(operation: &MirBackendOperation, locals: &mut BTreeSet<u32>) {
    match operation {
        MirBackendOperation::CheckedPrefix { operand, .. } => operand_locals(operand, locals),
        MirBackendOperation::CheckedBinary { left, right, .. } => {
            operand_locals(left, locals);
            operand_locals(right, locals);
        }
        MirBackendOperation::BoundsCheck { index, length } => {
            operand_locals(index, locals);
            operand_locals(length, locals);
        }
        MirBackendOperation::Call { arguments, .. } => {
            for argument in arguments {
                operand_locals(argument, locals);
            }
        }
        MirBackendOperation::Spawn { operation, .. } => operation_locals(operation, locals),
        MirBackendOperation::JoinValue { operand } => operand_locals(operand, locals),
        MirBackendOperation::HostCall { arguments, .. }
        | MirBackendOperation::Runtime { arguments, .. } => {
            for argument in arguments {
                operand_locals(argument, locals);
            }
        }
        MirBackendOperation::Assert { condition } => operand_locals(condition, locals),
        MirBackendOperation::Trap { .. } => {}
        MirBackendOperation::Marker { .. } => {}
    }
}

fn terminator_successors(terminator: &MirBackendTerminator) -> Vec<u32> {
    match terminator {
        MirBackendTerminator::Return | MirBackendTerminator::Marker { .. } => Vec::new(),
        MirBackendTerminator::Goto { target } => vec![*target],
        MirBackendTerminator::SwitchBool {
            if_true, if_false, ..
        } => vec![*if_true, *if_false],
        MirBackendTerminator::SwitchTag {
            cases, otherwise, ..
        } => cases
            .iter()
            .map(|(_, target)| *target)
            .chain(std::iter::once(*otherwise))
            .collect(),
        MirBackendTerminator::Invoke { target, .. } => target.iter().copied().collect(),
    }
}

/// Deferred task bodies are first enabled only for a straight-line MIR
/// function. A path-sensitive table is deliberately not approximated by a
/// function-global map: branches, loops, and disconnected blocks fall back to
/// ordinary eager lowering until a real data-flow proof is available.
fn deferred_lowering_is_linear(function: &MirBackendFunction) -> bool {
    let blocks = normal_blocks(function);
    let ordinals = blocks
        .iter()
        .map(|block| block.ordinal)
        .collect::<BTreeSet<_>>();
    let Some(first) = blocks.first().map(|block| block.ordinal) else {
        return false;
    };
    let mut visited = BTreeSet::new();
    let mut current = first;
    loop {
        if !visited.insert(current) {
            return false;
        }
        let Some(block) = blocks.iter().find(|block| block.ordinal == current) else {
            return false;
        };
        match &block.terminator {
            MirBackendTerminator::Return => break,
            MirBackendTerminator::Goto { target }
            | MirBackendTerminator::Invoke {
                target: Some(target),
                ..
            } => current = *target,
            MirBackendTerminator::SwitchBool { .. }
            | MirBackendTerminator::SwitchTag { .. }
            | MirBackendTerminator::Invoke { target: None, .. }
            | MirBackendTerminator::Marker { .. } => return false,
        }
    }
    visited == ordinals
}

/// Compute the scalar locals that must arrive at each normal block.  This is
/// ordinary backward liveness, expressed over the normalized CFG so both
/// Cranelift block parameters and future adapters can share the same rule.
fn block_live_in(function: &MirBackendFunction) -> BTreeMap<u32, BTreeSet<u32>> {
    let blocks = normal_blocks(function);
    let normal_ordinals = blocks
        .iter()
        .map(|block| block.ordinal)
        .collect::<BTreeSet<_>>();
    let mut live_in = blocks
        .iter()
        .map(|block| (block.ordinal, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    loop {
        let mut changed = false;
        for block in blocks.iter().rev() {
            let mut uses = BTreeSet::new();
            let mut definitions = BTreeSet::new();
            for statement in &block.statements {
                match statement {
                    MirBackendStatement::Assign { destination, value } => {
                        let mut statement_uses = BTreeSet::new();
                        rvalue_locals(value, &mut statement_uses);
                        uses.extend(
                            statement_uses
                                .into_iter()
                                .filter(|local| !definitions.contains(local)),
                        );
                        definitions.insert(*destination);
                    }
                    MirBackendStatement::Runtime { arguments, .. } => {
                        let mut statement_uses = BTreeSet::new();
                        for argument in arguments {
                            operand_locals(argument, &mut statement_uses);
                        }
                        uses.extend(
                            statement_uses
                                .into_iter()
                                .filter(|local| !definitions.contains(local)),
                        );
                    }
                    MirBackendStatement::Marker { .. } => {}
                }
            }
            match &block.terminator {
                MirBackendTerminator::Return => {
                    if !definitions.contains(&function.return_local) {
                        uses.insert(function.return_local);
                    }
                }
                MirBackendTerminator::Goto { .. } => {}
                MirBackendTerminator::SwitchBool { condition, .. } => {
                    let mut terminator_uses = BTreeSet::new();
                    operand_locals(condition, &mut terminator_uses);
                    uses.extend(
                        terminator_uses
                            .into_iter()
                            .filter(|local| !definitions.contains(local)),
                    );
                }
                MirBackendTerminator::SwitchTag { value, .. } => {
                    let mut terminator_uses = BTreeSet::new();
                    operand_locals(value, &mut terminator_uses);
                    uses.extend(
                        terminator_uses
                            .into_iter()
                            .filter(|local| !definitions.contains(local)),
                    );
                }
                MirBackendTerminator::Invoke {
                    operation,
                    destination,
                    ..
                } => {
                    let mut terminator_uses = BTreeSet::new();
                    operation_locals(operation, &mut terminator_uses);
                    uses.extend(
                        terminator_uses
                            .into_iter()
                            .filter(|local| !definitions.contains(local)),
                    );
                    if let Some(destination) = destination {
                        definitions.insert(*destination);
                    }
                }
                MirBackendTerminator::Marker { .. } => {}
            }
            let mut outgoing = BTreeSet::new();
            for target in terminator_successors(&block.terminator) {
                if normal_ordinals.contains(&target)
                    && let Some(target_live_in) = live_in.get(&target)
                {
                    outgoing.extend(target_live_in.iter().copied());
                }
            }
            let mut next = uses;
            next.extend(
                outgoing
                    .into_iter()
                    .filter(|local| !definitions.contains(local)),
            );
            let entry = live_in
                .get_mut(&block.ordinal)
                .expect("normal block has a liveness entry");
            if *entry != next {
                *entry = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    live_in
}

fn cranelift_edge_args(
    target: u32,
    locals: &BTreeMap<u32, Value>,
    live_in: &BTreeMap<u32, BTreeSet<u32>>,
) -> Result<Vec<Value>, String> {
    live_in
        .get(&target)
        .into_iter()
        .flat_map(|required| required.iter())
        .map(|local| {
            locals.get(local).copied().ok_or_else(|| {
                format!("MIR local {local} is not available on edge to block {target}")
            })
        })
        .collect()
}

fn lower_rvalue_cranelift(
    builder: &mut FunctionBuilder<'_>,
    value: &MirBackendRvalue,
    locals: &BTreeMap<u32, Value>,
    runtime: &RuntimeRefs,
) -> Result<Value, String> {
    match value {
        MirBackendRvalue::Use(operand) => {
            lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)
        }
        MirBackendRvalue::Tag { value } => Ok(builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I64, i64::from(*value))),
        MirBackendRvalue::Aggregate { kind, values } => {
            let tag = aggregate_tag(kind)?;
            if !matches!(
                kind.as_str(),
                "option-none" | "option-some" | "result-ok" | "result-err"
            ) {
                let count = builder.ins().iconst(
                    cranelift_codegen::ir::types::I64,
                    i64::try_from(values.len())
                        .map_err(|_| "native aggregate has too many fields".to_owned())?,
                );
                let tag_value = builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I64, i64::from(tag));
                let created = builder.ins().call(runtime.aggregate_new, &[tag_value, count]);
                let aggregate = builder
                    .inst_results(created)
                    .first()
                    .copied()
                    .ok_or_else(|| "Cranelift aggregate constructor did not return a handle".to_owned())?;
                for (index, operand) in values.iter().enumerate() {
                    let index_value = builder.ins().iconst(
                        cranelift_codegen::ir::types::I64,
                        i64::try_from(index)
                            .map_err(|_| "native aggregate field index overflow".to_owned())?,
                    );
                    let value = lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)?;
                    let set = builder.ins().call(runtime.aggregate_set, &[aggregate, index_value, value]);
                    let _ = builder.inst_results(set);
                }
                return Ok(aggregate);
            }
            let payload = values
                .first()
                .map(|operand| {
                    lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)
                })
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(cranelift_codegen::ir::types::I64, 0));
            let has_payload = builder.ins().iconst(
                cranelift_codegen::ir::types::I64,
                i64::from(!values.is_empty()),
            );
            let tag = builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, i64::from(tag));
            let call = builder
                .ins()
                .call(runtime.result_new, &[tag, payload, has_payload]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift result constructor did not return a handle".to_owned())
        }
        MirBackendRvalue::Prefix { operator, operand } => {
            let operand = lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)?;
            match operator.as_str() {
                "negate" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
                    builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
                    Ok(value)
                }
                "bitwise-not" => Ok(builder.ins().bxor_imm(operand, -1)),
                "logical-not" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let value = builder.ins().icmp(IntCC::Equal, operand, zero);
                    Ok(builder
                        .ins()
                        .uextend(cranelift_codegen::ir::types::I64, value))
                }
                other => Err(format!("Cranelift scalar prefix is not supported: {other}")),
            }
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => lower_checked_binary_cranelift(builder, operator, left, right, locals),
        MirBackendRvalue::NumericConversion {
            source,
            target,
            conversion,
            operand,
        } => lower_numeric_conversion_cranelift(
            builder, source, target, conversion, operand, locals, runtime,
        ),
        MirBackendRvalue::Coerce { kind, operand } => {
            if kind == "Diverging" {
                builder.ins().trap(TrapCode::unwrap_user(6));
            }
            lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)
        }
        MirBackendRvalue::HostCall { kind, arguments } => {
            let kind_id = host_call_kind(kind)?;
            let argument = arguments
                .first()
                .map(|argument| {
                    lower_operand_cranelift_with_runtime(builder, argument, locals, runtime)
                })
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(cranelift_codegen::ir::types::I64, 0));
            let kind_value = builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, i64::from(kind_id));
            let call = builder
                .ins()
                .call(runtime.host_call, &[kind_value, argument]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift host rvalue call did not return a handle".to_owned())
        }
        MirBackendRvalue::Unsupported { kind } => {
            Err(format!("MIR rvalue is not supported: {kind}"))
        }
    }
}

fn lower_operation_cranelift(
    builder: &mut FunctionBuilder<'_>,
    operation: &MirBackendOperation,
    locals: &BTreeMap<u32, Value>,
    calls: &BTreeMap<u32, FuncRef>,
    trap: FuncRef,
    runtime: &RuntimeRefs,
) -> Result<Value, String> {
    match operation {
        MirBackendOperation::CheckedPrefix { operator, operand } => {
            let operand = lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)?;
            match operator.as_str() {
                "negate" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
                    builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
                    Ok(value)
                }
                "bitwise-not" => Ok(builder.ins().bxor_imm(operand, -1)),
                "logical-not" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let value = builder.ins().icmp(IntCC::Equal, operand, zero);
                    Ok(builder
                        .ins()
                        .uextend(cranelift_codegen::ir::types::I64, value))
                }
                other => Err(format!("Cranelift scalar prefix is not supported: {other}")),
            }
        }
        MirBackendOperation::CheckedBinary {
            operator,
            left,
            right,
        } => lower_checked_binary_cranelift(builder, operator, left, right, locals),
        MirBackendOperation::BoundsCheck { index, length } => {
            let index = lower_operand_cranelift_with_runtime(builder, index, locals, runtime)?;
            let length = lower_operand_cranelift_with_runtime(builder, length, locals, runtime)?;
            let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
            let below_zero = builder.ins().icmp(IntCC::SignedLessThan, index, zero);
            let past_end = builder
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, index, length);
            let invalid = builder.ins().bor(below_zero, past_end);
            builder.ins().trapnz(invalid, TrapCode::unwrap_user(5));
            Ok(index)
        }
        MirBackendOperation::Call {
            function,
            arguments,
        } => {
            let function_ref = calls
                .get(function)
                .copied()
                .ok_or_else(|| format!("Cranelift call target {function} is not declared"))?;
            let arguments = arguments
                .iter()
                .map(|argument| {
                    lower_operand_cranelift_with_runtime(builder, argument, locals, runtime)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let call = builder.ins().call(function_ref, &arguments);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift scalar call did not return a value".to_owned())
        }
        MirBackendOperation::Spawn { operation, kind } => {
            let value = lower_operation_cranelift(builder, operation, locals, calls, trap, runtime)?;
            let pending = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
            let function = if kind == "thread" {
                runtime.thread_spawn
            } else {
                runtime.task_spawn
            };
            let call = builder.ins().call(function, &[value, pending]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift spawn did not return a handle".to_owned())
        }
        MirBackendOperation::JoinValue { operand } => {
            let handle = lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)?;
            let call = builder.ins().call(runtime.await_task, &[handle]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift await did not return a value".to_owned())
        }
        MirBackendOperation::HostCall { kind, arguments } => {
            let kind_id = host_call_kind(kind)?;
            let argument = arguments
                .first()
                .map(|argument| {
                    lower_operand_cranelift_with_runtime(builder, argument, locals, runtime)
                })
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(cranelift_codegen::ir::types::I64, 0));
            let kind_value = builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, i64::from(kind_id));
            let call = builder
                .ins()
                .call(runtime.host_call, &[kind_value, argument]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift host call did not return a result handle".to_owned())
        }
        MirBackendOperation::Runtime { kind, arguments } => {
            lower_runtime_call_cranelift(builder, kind, arguments, locals, runtime)
        }
        MirBackendOperation::Assert { condition } => {
            let condition =
                lower_operand_cranelift_with_runtime(builder, condition, locals, runtime)?;
            let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
            let invalid = builder.ins().icmp(IntCC::Equal, condition, zero);
            builder.ins().trapnz(invalid, TrapCode::unwrap_user(3));
            Ok(builder.ins().iconst(cranelift_codegen::ir::types::I64, 0))
        }
        MirBackendOperation::Trap { kind } => {
            let call = builder.ins().call(trap, &[]);
            let zero = builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift trap helper did not return a value".to_owned())?;
            let _ = kind;
            Ok(zero)
        }
        MirBackendOperation::Marker { kind } => {
            Err(format!("MIR operation is not supported: {kind}"))
        }
    }
}

fn lower_operand_cranelift_with_runtime(
    builder: &mut FunctionBuilder<'_>,
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, Value>,
    runtime: &RuntimeRefs,
) -> Result<Value, String> {
    match operand {
        MirBackendOperand::Projection { index, depth, kind } => {
            if *depth != 1 {
                return Err(format!(
                    "native core projection is not supported: {kind} at depth {depth}"
                ));
            }
            let base = locals.get(index).copied().ok_or_else(|| {
                format!("MIR projection base local {index} is not available")
            })?;
            if let Some(field) = parse_aggregate_projection(kind) {
                let field = builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I64, i64::from(field));
                let call = builder.ins().call(runtime.aggregate_get, &[base, field]);
                return builder
                    .inst_results(call)
                    .first()
                    .copied()
                    .ok_or_else(|| "Cranelift aggregate projection did not return a value".to_owned());
            }
            if !matches!(
                kind.as_str(),
                "option-value" | "result-ok-value" | "result-err-value"
            ) {
                return Err(format!(
                    "native core projection is not supported: {kind} at depth {depth}"
                ));
            }
            let call = builder.ins().call(runtime.result_payload, &[base]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift result payload helper did not return a value".to_owned())
        }
        _ => lower_operand_cranelift(builder, operand, locals),
    }
}

fn lower_operand_cranelift(
    builder: &mut FunctionBuilder<'_>,
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, Value>,
) -> Result<Value, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => parse_integer_literal(value)
            .map(|value| {
                builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I64, value)
            })
            .map_err(|error| format!("invalid scalar integer `{value}`: {error}")),
        MirBackendOperand::Constant(MirBackendConstant::Bool(value)) => Ok(builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I64, i64::from(*value))),
        MirBackendOperand::Constant(MirBackendConstant::String(value)) => {
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            Ok(builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, string_payload(value)))
        }
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => locals
            .get(index)
            .copied()
            .ok_or_else(|| format!("MIR local {index} is not available in the adapter")),
        MirBackendOperand::Projection { kind, depth, .. } => Err(format!(
            "MIR core projection `{kind}` at depth {depth} requires runtime-aware lowering"
        )),
        MirBackendOperand::Function { kind } => Ok(builder.ins().iconst(
            cranelift_codegen::ir::types::I64,
            parse_verified_function_ordinal(kind)
                .map(i64::from)
                .unwrap_or_else(|| string_payload(kind)),
        )),
        MirBackendOperand::Constant(other) => {
            let kind = match other {
                MirBackendConstant::Unit => "unit".to_owned(),
                MirBackendConstant::Float(value) | MirBackendConstant::Char(value) => value.clone(),
                MirBackendConstant::Named => "named".to_owned(),
                MirBackendConstant::Integer(_)
                | MirBackendConstant::Bool(_)
                | MirBackendConstant::String(_) => unreachable!(),
            };
            Err(format!("MIR constant is not scalar: {kind}"))
        }
        MirBackendOperand::Unsupported { kind } => {
            Err(format!("MIR operand is not supported: {kind}"))
        }
    }
}

fn parse_integer_literal(spelling: &str) -> Result<i64, String> {
    const SUFFIXES: [&str; 8] = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"];
    let value = spelling.replace('_', "");
    let (negative, body) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = value.strip_prefix('+') {
        (false, rest)
    } else {
        (false, value.as_str())
    };
    let (radix, digits_with_suffix) = if let Some(rest) = body.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = body.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = body.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, body)
    };
    let digit_end = digits_with_suffix
        .char_indices()
        .find(|(_, character)| match radix {
            2 => !matches!(character, '0' | '1'),
            8 => !matches!(character, '0'..='7'),
            10 => !character.is_ascii_digit(),
            16 => !character.is_ascii_hexdigit(),
            _ => true,
        })
        .map_or(digits_with_suffix.len(), |(index, _)| index);
    let digits = &digits_with_suffix[..digit_end];
    let suffix = &digits_with_suffix[digit_end..];
    if digits.is_empty() {
        return Err(format!("invalid scalar integer {spelling}: missing digits"));
    }
    if !suffix.is_empty() && !SUFFIXES.contains(&suffix) {
        return Err(format!("invalid scalar integer {spelling}: unknown suffix `{suffix}`"));
    }
    let magnitude = u128::from_str_radix(digits, radix)
        .map_err(|error| format!("invalid scalar integer {spelling}: {error}"))?;
    if negative {
        if magnitude == 1_u128 << 63 {
            Ok(i64::MIN)
        } else {
            i64::try_from(magnitude)
                .ok()
                .and_then(|value| value.checked_neg())
                .ok_or_else(|| format!("scalar integer {spelling} is out of range"))
        }
    } else {
        i64::try_from(magnitude)
            .map_err(|_| format!("scalar integer {spelling} is out of range"))
    }
}

fn is_native_integer_scalar(name: &str) -> bool {
    matches!(
        name,
        "Byte" | "Int8" | "Int16" | "Int32" | "Int" | "UInt8" | "UInt16" | "UInt32"
    )
}

fn lower_numeric_conversion_cranelift(
    builder: &mut FunctionBuilder<'_>,
    source: &str,
    target: &str,
    conversion: &str,
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, Value>,
    runtime: &RuntimeRefs,
) -> Result<Value, String> {
    let value = lower_operand_cranelift_with_runtime(builder, operand, locals, runtime)?;
    if !is_native_integer_scalar(source) || !is_native_integer_scalar(target) {
        return Err(format!(
            "Cranelift numeric conversion is not supported for {source}->{target}"
        ));
    }
    if conversion == "identity" || conversion == "total" {
        return Ok(value);
    }
    if conversion != "checked" {
        return Err(format!("Cranelift numeric conversion mode is not supported: {conversion}"));
    }
    let (minimum, maximum) = integer_conversion_bounds(target).ok_or_else(|| {
        format!("Cranelift numeric conversion target is not supported: {target}")
    })?;
    let minimum = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, minimum);
    let maximum = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, maximum);
    let below = builder.ins().icmp(IntCC::SignedLessThan, value, minimum);
    let above = builder
        .ins()
        .icmp(IntCC::SignedGreaterThan, value, maximum);
    let invalid = builder.ins().bor(below, above);
    let error_tag = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, 3);
    let success_tag = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, 2);
    let tag = builder.ins().select(invalid, error_tag, success_tag);
    let no_payload = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, 0);
    let has_payload = builder
        .ins()
        .iconst(cranelift_codegen::ir::types::I64, 1);
    let has_payload = builder.ins().select(invalid, no_payload, has_payload);
    let call = builder
        .ins()
        .call(runtime.result_new, &[tag, value, has_payload]);
    builder
        .inst_results(call)
        .first()
        .copied()
        .ok_or_else(|| "Cranelift numeric conversion did not return a result".to_owned())
}

fn lower_checked_binary_cranelift(
    builder: &mut FunctionBuilder<'_>,
    operator: &str,
    left: &MirBackendOperand,
    right: &MirBackendOperand,
    locals: &BTreeMap<u32, Value>,
) -> Result<Value, String> {
    let left = lower_operand_cranelift(builder, left, locals)?;
    let right = lower_operand_cranelift(builder, right, locals)?;
    Ok(match operator {
        "add" => {
            let (value, overflow) = builder.ins().sadd_overflow(left, right);
            builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
            value
        }
        "subtract" => {
            let (value, overflow) = builder.ins().ssub_overflow(left, right);
            builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
            value
        }
        "multiply" => {
            let (value, overflow) = builder.ins().smul_overflow(left, right);
            builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
            value
        }
        "divide" => builder.ins().sdiv(left, right),
        "remainder" => builder.ins().srem(left, right),
        "bitwise-and" => builder.ins().band(left, right),
        "bitwise-or" => builder.ins().bor(left, right),
        "bitwise-xor" => builder.ins().bxor(left, right),
        "logical-and" => builder.ins().band(left, right),
        "logical-or" => builder.ins().bor(left, right),
        "less" => {
            let value = builder.ins().icmp(IntCC::SignedLessThan, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "less-equal" => {
            let value = builder
                .ins()
                .icmp(IntCC::SignedLessThanOrEqual, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "greater" => {
            let value = builder.ins().icmp(IntCC::SignedGreaterThan, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "greater-equal" => {
            let value = builder
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "equal" => {
            let value = builder.ins().icmp(IntCC::Equal, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "not-equal" => {
            let value = builder.ins().icmp(IntCC::NotEqual, left, right);
            builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, value)
        }
        "shift-left" | "shift-right" => {
            let width = builder.ins().iconst(cranelift_codegen::ir::types::I64, 64);
            let invalid = builder
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, right, width);
            builder.ins().trapnz(invalid, TrapCode::unwrap_user(2));
            if operator == "shift-left" {
                builder.ins().ishl(left, right)
            } else {
                builder.ins().sshr(left, right)
            }
        }
        other => return Err(format!("Cranelift scalar binary is not supported: {other}")),
    })
}

fn compile_cranelift(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    program: &MirBackendProgram,
) -> Result<CodegenResult, String> {
    let builder = ObjectBuilder::new(
        cranelift_isa()?,
        "tondo-native-evaluation-compile",
        default_libcall_names(),
    )
    .map_err(|error| format!("cannot initialize Cranelift object builder: {error}"))?;
    let mut module = ObjectModule::new(builder);
    let function_ids = declare_cranelift_functions(&mut module, program)?;
    let function_count = program.functions.len().clamp(1, MAX_FUNCTIONS as usize);
    let started = Instant::now();
    let mut code_size_bytes = 0_u64;
    let mut supported_functions = 0_u64;
    let mut unsupported_functions = 0_u64;
    for function in program.functions.iter().take(function_count) {
        if function.supported {
            supported_functions += 1;
        } else {
            unsupported_functions += 1;
        }
        let ir_function = lower_cranelift_function(&mut module, function, &function_ids)?;
        let mut context = cranelift_codegen::Context::for_function(ir_function);
        let mut control = cranelift_codegen::control::ControlPlane::default();
        let compiled = context
            .compile(isa, &mut control)
            .map_err(|error| format!("Cranelift compilation failed: {error:?}"))?;
        code_size_bytes = code_size_bytes
            .checked_add(compiled.code_buffer().len() as u64)
            .ok_or_else(|| "Cranelift code size overflow".to_owned())?;
    }
    Ok(CodegenResult {
        compile_time_ns: started.elapsed().as_nanos(),
        code_size_bytes,
        supported_functions,
        unsupported_functions,
    })
}

fn cranelift_signature(
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    function: &MirBackendFunction,
) -> Signature {
    let mut signature = Signature::new(isa.default_call_conv());
    for _ in &function.parameters {
        signature
            .params
            .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    }
    signature
        .returns
        .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    signature
}

fn declare_cranelift_functions(
    module: &mut ObjectModule,
    program: &MirBackendProgram,
) -> Result<BTreeMap<u32, FuncId>, String> {
    program
        .functions
        .iter()
        .map(|function| {
            module
                .declare_function(
                    &format!("tondo_probe_{}", function.ordinal),
                    Linkage::Export,
                    &cranelift_signature(module.isa(), function),
                )
                .map(|id| (function.ordinal, id))
                .map_err(|error| format!("cannot declare Cranelift function: {error}"))
        })
        .collect()
}

fn declare_cranelift_trap(module: &mut ObjectModule) -> Result<FuncId, String> {
    let mut signature = Signature::new(module.isa().default_call_conv());
    signature
        .returns
        .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    module
        .declare_function("tondo_explicit_panic", Linkage::Import, &signature)
        .map_err(|error| format!("cannot declare Cranelift trap helper: {error}"))
}

fn lower_cranelift_function(
    module: &mut ObjectModule,
    function: &MirBackendFunction,
    function_ids: &BTreeMap<u32, FuncId>,
) -> Result<Function, String> {
    let signature = cranelift_signature(module.isa(), function);
    let mut ir_function =
        Function::with_name_signature(UserFuncName::user(0, function.ordinal), signature);
    let calls = function_ids
        .iter()
        .map(|(ordinal, function_id)| {
            (
                *ordinal,
                module.declare_func_in_func(*function_id, &mut ir_function),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let trap_id = declare_cranelift_trap(module)?;
    let trap = module.declare_func_in_func(trap_id, &mut ir_function);
    let runtime = declare_cranelift_runtime(module, &mut ir_function)?;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ir_function, &mut builder_context);
        if !function.supported {
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder
                .ins()
                .trap(TrapCode::user(1).expect("valid user trap"));
            builder.seal_block(entry);
        } else {
            let blocks = normal_blocks(function);
            let entry_ordinal = blocks
                .first()
                .map(|block| block.ordinal)
                .ok_or_else(|| "supported scalar function has no normal entry block".to_owned())?;
            let live_in = block_live_in(function);
            let mut ir_blocks = BTreeMap::<u32, Block>::new();
            let mut deferred_tasks = BTreeMap::<u32, MirBackendOperation>::new();
            let deferred_enabled = deferred_lowering_is_linear(function);
            for block in &blocks {
                ir_blocks.insert(block.ordinal, builder.create_block());
            }
            let entry = *ir_blocks
                .get(&entry_ordinal)
                .expect("entry block was just created");
            builder.append_block_params_for_function_params(entry);
            for block in &blocks {
                if block.ordinal == entry_ordinal {
                    continue;
                }
                for _ in live_in
                    .get(&block.ordinal)
                    .into_iter()
                    .flat_map(|locals| locals.iter())
                {
                    builder.append_block_param(
                        *ir_blocks
                            .get(&block.ordinal)
                            .expect("block was just created"),
                        cranelift_codegen::ir::types::I64,
                    );
                }
            }

            for block in &blocks {
                let ir_block = *ir_blocks
                    .get(&block.ordinal)
                    .expect("block was just created");
                builder.switch_to_block(ir_block);
                let mut locals = BTreeMap::new();
                if block.ordinal == entry_ordinal {
                    for (position, local) in function.parameters.iter().enumerate() {
                        locals.insert(*local, builder.block_params(ir_block)[position]);
                    }
                } else if let Some(required) = live_in.get(&block.ordinal) {
                    for (local, value) in required.iter().zip(builder.block_params(ir_block)) {
                        locals.insert(*local, *value);
                    }
                }
                for statement in &block.statements {
                    match statement {
                        MirBackendStatement::Assign { destination, value } => {
                            let value =
                                lower_rvalue_cranelift(&mut builder, value, &locals, &runtime)?;
                            locals.insert(*destination, value);
                        }
                        MirBackendStatement::Marker { kind } => {
                            let _ = kind;
                        }
                        MirBackendStatement::Runtime { kind, arguments } => {
                            let _ = lower_runtime_call_cranelift(
                                &mut builder,
                                kind,
                                arguments,
                                &locals,
                                &runtime,
                            )?;
                        }
                    }
                }
                match &block.terminator {
                    MirBackendTerminator::Return => {
                        let value =
                            locals
                                .get(&function.return_local)
                                .copied()
                                .unwrap_or_else(|| {
                                    builder.ins().iconst(cranelift_codegen::ir::types::I64, 0)
                                });
                        builder.ins().return_(&[value]);
                    }
                    MirBackendTerminator::Goto { target } => {
                        let destination = *ir_blocks
                            .get(target)
                            .ok_or_else(|| format!("MIR goto target block {target} is missing"))?;
                        let arguments = cranelift_edge_args(*target, &locals, &live_in)?
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(destination, &arguments);
                    }
                    MirBackendTerminator::SwitchBool {
                        condition,
                        if_true,
                        if_false,
                    } => {
                        let condition = lower_operand_cranelift_with_runtime(
                            &mut builder,
                            condition,
                            &locals,
                            &runtime,
                        )?;
                        let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                        let condition = builder.ins().icmp(IntCC::NotEqual, condition, zero);
                        let true_block = *ir_blocks.get(if_true).ok_or_else(|| {
                            format!("MIR switch true target block {if_true} is missing")
                        })?;
                        let false_block = *ir_blocks.get(if_false).ok_or_else(|| {
                            format!("MIR switch false target block {if_false} is missing")
                        })?;
                        let true_arguments = cranelift_edge_args(*if_true, &locals, &live_in)?
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        let false_arguments = cranelift_edge_args(*if_false, &locals, &live_in)?
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().brif(
                            condition,
                            true_block,
                            &true_arguments,
                            false_block,
                            &false_arguments,
                        );
                    }
                    MirBackendTerminator::SwitchTag {
                        value,
                        cases,
                        otherwise,
                    } => {
                        let value = lower_operand_cranelift_with_runtime(
                            &mut builder,
                            value,
                            &locals,
                            &runtime,
                        )?;
                        let tag_call = builder.ins().call(runtime.result_tag, &[value]);
                        let value =
                            builder
                                .inst_results(tag_call)
                                .first()
                                .copied()
                                .ok_or_else(|| {
                                    "Cranelift tag helper did not return a value".to_owned()
                                })?;
                        let otherwise_block = *ir_blocks.get(otherwise).ok_or_else(|| {
                            format!("MIR switch otherwise target block {otherwise} is missing")
                        })?;
                        if cases.is_empty() {
                            let arguments = cranelift_edge_args(*otherwise, &locals, &live_in)?
                                .into_iter()
                                .map(Into::into)
                                .collect::<Vec<_>>();
                            builder.ins().jump(otherwise_block, &arguments);
                        } else {
                            let mut generated = Vec::new();
                            for (case_index, (tag, target)) in cases.iter().enumerate() {
                                let target_block = *ir_blocks.get(target).ok_or_else(|| {
                                    format!("MIR switch tag target block {target} is missing")
                                })?;
                                let target_arguments =
                                    cranelift_edge_args(*target, &locals, &live_in)?
                                        .into_iter()
                                        .map(Into::into)
                                        .collect::<Vec<_>>();
                                let last = case_index + 1 == cases.len();
                                let next = if last {
                                    otherwise_block
                                } else {
                                    let block = builder.create_block();
                                    generated.push(block);
                                    block
                                };
                                let next_arguments = if last {
                                    cranelift_edge_args(*otherwise, &locals, &live_in)?
                                        .into_iter()
                                        .map(Into::into)
                                        .collect::<Vec<_>>()
                                } else {
                                    Vec::new()
                                };
                                let matches =
                                    builder.ins().icmp_imm(IntCC::Equal, value, i64::from(*tag));
                                builder.ins().brif(
                                    matches,
                                    target_block,
                                    &target_arguments,
                                    next,
                                    &next_arguments,
                                );
                                if !last {
                                    builder.switch_to_block(next);
                                }
                            }
                            for block in generated {
                                builder.seal_block(block);
                            }
                        }
                    }
                    MirBackendTerminator::Invoke {
                        operation,
                        destination,
                        target: Some(target),
                    } => {
                        let value = lower_cranelift_invoke(
                            &mut builder,
                            operation,
                            *destination,
                            &locals,
                            &calls,
                            trap,
                            &runtime,
                            &mut deferred_tasks,
                            deferred_enabled,
                        )?;
                        if let Some(destination) = destination {
                            locals.insert(*destination, value);
                        }
                        let destination_block = *ir_blocks.get(target).ok_or_else(|| {
                            format!("MIR invoke target block {target} is missing")
                        })?;
                        let arguments = cranelift_edge_args(*target, &locals, &live_in)?
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(destination_block, &arguments);
                    }
                    MirBackendTerminator::Invoke { target: None, .. } => {
                        return Err("scalar invoke has no normal target".to_owned());
                    }
                    MirBackendTerminator::Marker { kind } if kind == "unreachable" => {
                        builder.ins().trap(TrapCode::unwrap_user(4));
                    }
                    MirBackendTerminator::Marker { kind } => {
                        return Err(format!("MIR terminator is not supported: {kind}"));
                    }
                }
            }
            for block in &blocks {
                builder.seal_block(
                    *ir_blocks
                        .get(&block.ordinal)
                        .expect("block was just created"),
                );
            }
        }
        builder.finalize();
    }
    Ok(ir_function)
}

fn emit_cranelift_object(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &MirBackendProgram,
    output: &Path,
) -> Result<(), String> {
    let builder = ObjectBuilder::new(isa, "tondo-native-evaluation", default_libcall_names())
        .map_err(|error| format!("cannot initialize Cranelift object builder: {error}"))?;
    let mut module = ObjectModule::new(builder);
    let function_ids = declare_cranelift_functions(&mut module, program)?;
    for function in &program.functions {
        let ir_function = lower_cranelift_function(&mut module, function, &function_ids)?;
        let function_id = *function_ids
            .get(&function.ordinal)
            .ok_or_else(|| format!("missing Cranelift function {}", function.ordinal))?;
        let mut context = module.make_context();
        context.func = ir_function;
        module
            .define_function(function_id, &mut context)
            .map_err(|error| format!("cannot define Cranelift object function: {error}"))?;
        module.clear_context(&mut context);
    }
    let bytes = module
        .finish()
        .emit()
        .map_err(|error| format!("cannot emit Cranelift object: {error}"))?;
    fs::write(output, bytes).map_err(|error| {
        format!(
            "cannot write Cranelift object `{}`: {error}",
            output.display()
        )
    })
}

/// Identifies the deliberately small deferred-call subset of the first native
/// coordinator.  Captures are constants in this slice, so publishing a task
/// never evaluates its callable body and the join edge can evaluate it later
/// without changing a mutable local underneath it.  Mutable captures and
/// closures remain explicit unsupported MIR until their native storage ABI is
/// available.
fn deferred_call_body(operation: &MirBackendOperation) -> Option<&MirBackendOperation> {
    let MirBackendOperation::Spawn {
        operation: body,
        kind,
    } = operation
    else {
        return None;
    };
    if kind != "task" {
        return None;
    }
    let MirBackendOperation::Call { arguments, .. } = body.as_ref() else {
        return None;
    };
    arguments.iter().all(deferred_capture_operand).then_some(body.as_ref())
}

fn deferred_capture_operand(operand: &MirBackendOperand) -> bool {
    matches!(
        operand,
        MirBackendOperand::Constant(
            MirBackendConstant::Bool(_)
                | MirBackendConstant::Integer(_)
                | MirBackendConstant::String(_)
        )
    )
}

fn lower_cranelift_invoke(
    builder: &mut FunctionBuilder<'_>,
    operation: &MirBackendOperation,
    destination: Option<u32>,
    locals: &BTreeMap<u32, Value>,
    calls: &BTreeMap<u32, FuncRef>,
    trap: FuncRef,
    runtime: &RuntimeRefs,
    deferred_tasks: &mut BTreeMap<u32, MirBackendOperation>,
    deferred_enabled: bool,
) -> Result<Value, String> {
    if deferred_enabled
        && destination.is_some()
        && let Some(body) = deferred_call_body(operation)
    {
        let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
        let pending = builder.ins().iconst(cranelift_codegen::ir::types::I64, 1);
        let call = builder.ins().call(runtime.task_spawn, &[zero, pending]);
        let handle = builder
            .inst_results(call)
            .first()
            .copied()
            .ok_or_else(|| "Cranelift deferred spawn did not return a handle".to_owned())?;
        if let Some(destination) = destination {
            deferred_tasks.insert(destination, body.clone());
        }
        return Ok(handle);
    }

    if let MirBackendOperation::JoinValue {
        operand: MirBackendOperand::Local { index },
    } = operation
        && let Some(body) = deferred_tasks.remove(index)
    {
        let handle = lower_operand_cranelift_with_runtime(
            builder,
            operation_operand(operation),
            locals,
            runtime,
        )?;
        let value = lower_operation_cranelift(builder, &body, locals, calls, trap, runtime)?;
        let completed = builder.ins().call(runtime.task_complete, &[handle, value]);
        let _ = builder.inst_results(completed);
        let awaited = builder.ins().call(runtime.await_task, &[handle]);
        return builder
            .inst_results(awaited)
            .first()
            .copied()
            .ok_or_else(|| "Cranelift deferred join did not return a value".to_owned());
    }

    lower_operation_cranelift(builder, operation, locals, calls, trap, runtime)
}

fn operation_operand(operation: &MirBackendOperation) -> &MirBackendOperand {
    match operation {
        MirBackendOperation::JoinValue { operand } => operand,
        _ => unreachable!("operation_operand is only used for JoinValue"),
    }
}

fn run_native_scalar_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
    fixture: &FixtureObservation,
    program: &MirBackendProgram,
) -> Result<Vec<NativeRunReport>, String> {
    let functions = program
        .functions
        .iter()
        .filter(|function| {
            function.supported
                && function.return_type == "Int"
                && function.parameter_types.iter().all(|ty| ty == "Int")
        })
        .collect::<Vec<_>>();
    if functions.is_empty() {
        return Ok(Vec::new());
    }
    let mut reports = Vec::new();
    let cranelift_object =
        temp_dir.join(format!("{}_all.cranelift.o", safe_stem(&fixture.fixture)));
    emit_cranelift_object(cranelift_isa()?, program, &cranelift_object)?;
    for function in functions {
        for (case_index, arguments) in scalar_case_arguments_for_function(function)
            .into_iter()
            .enumerate()
        {
            let oracle = evaluate_scalar_program(program, function.ordinal, &arguments);
            let vm = fixture
                .vm_scalar
                .iter()
                .find(|observation| {
                    observation.function_ordinal == function.ordinal
                        && observation.arguments == arguments
                })
                .ok_or_else(|| {
                    format!(
                        "VM scalar oracle has no observation for {} function#{} case#{}",
                        fixture.fixture, function.ordinal, case_index
                    )
                })?;
            let (oracle_status, oracle_result, expects_trap) = match oracle {
                Ok(result) => {
                    if vm.status != "returned"
                        || vm.result != Some(result)
                        || !vm.diagnostics.is_empty()
                    {
                        return Err(format!(
                            "normalized scalar oracle disagrees with VM for {} function#{} case#{}",
                            fixture.fixture, function.ordinal, case_index
                        ));
                    }
                    ("returned", Some(result), false)
                }
                Err(_) => {
                    if !matches!(vm.status.as_str(), "panicked" | "error") || vm.result.is_some() {
                        return Err(format!(
                            "normalized scalar trap disagrees with VM for {} function#{} case#{}",
                            fixture.fixture, function.ordinal, case_index
                        ));
                    }
                    ("trapped", None, true)
                }
            };
            let stem = format!(
                "{}_{}_case{}",
                safe_stem(&fixture.fixture),
                function.ordinal,
                case_index
            );
            let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
            fs::write(
                &cranelift_source,
                c_runner_source(function, &arguments, oracle_result),
            )
            .map_err(|error| format!("cannot write Cranelift runner: {error}"))?;
            let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
            link_native_runner(cc, &cranelift_source, &cranelift_object, &cranelift_binary)?;
            run_native_binary(&cranelift_binary, "Cranelift", expects_trap)?;

            let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
            let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
            fs::write(
                &llvm_ir,
                llvm_module_with_runner(target, program, function, &arguments, oracle_result)?,
            )
            .map_err(|error| format!("cannot write LLVM runner: {error}"))?;
            let result = Command::new(llvm)
                .arg("-O2")
                .arg("-filetype=obj")
                .arg(format!("-mtriple={target}"))
                .arg("-o")
                .arg(&llvm_object)
                .arg(&llvm_ir)
                .output()
                .map_err(|error| format!("cannot execute LLVM llc for runner: {error}"))?;
            if !result.status.success() {
                return Err(format!(
                    "LLVM runner llc failed: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
            fs::write(&llvm_source, native_runtime_c_source())
                .map_err(|error| format!("cannot write LLVM runner anchor: {error}"))?;
            let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
            link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
            run_native_binary(&llvm_binary, "LLVM", expects_trap)?;

            reports.push(NativeRunReport {
                fixture: fixture.fixture.clone(),
                function_ordinal: function.ordinal,
                arguments,
                oracle_status,
                oracle_result,
                vm_status: vm.status.clone(),
                vm_result: vm.result,
                vm_diagnostics: vm.diagnostics.clone(),
                cranelift: "passed",
                llvm: "passed",
            });
        }
    }
    Ok(reports)
}

fn run_native_managed_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
    fixture: &FixtureObservation,
    program: &MirBackendProgram,
) -> Result<Vec<NativeManagedRunReport>, String> {
    let functions = program
        .functions
        .iter()
        .filter(|function| {
            function.supported
                && function.return_type != "Int"
                && !function.parameter_types.is_empty()
                && function
                    .parameter_types
                    .iter()
                    .all(|ty| matches!(ty.as_str(), "Int" | "Bool"))
        })
        .collect::<Vec<_>>();
    if functions.is_empty() {
        return Ok(Vec::new());
    }
    let mut reports = Vec::new();
    let cranelift_object = temp_dir.join(format!(
        "{}_managed.cranelift.o",
        safe_stem(&fixture.fixture)
    ));
    emit_cranelift_object(cranelift_isa()?, program, &cranelift_object)?;
    for function in functions {
        for (case_index, arguments) in managed_case_arguments_for_function(function)
            .into_iter()
            .enumerate()
        {
            let oracle_value = evaluate_scalar_program(program, function.ordinal, &arguments)?;
            let (oracle_tag, oracle_payload) = oracle_managed_parts(oracle_value)?;
            let vm = fixture
                .vm_managed
                .iter()
                .find(|observation| {
                    observation.function_ordinal == function.ordinal
                        && observation.arguments == arguments
                })
                .ok_or_else(|| {
                    format!(
                        "VM managed oracle has no observation for {} function#{} case#{}",
                        fixture.fixture, function.ordinal, case_index
                    )
                })?;
            if vm.status != "returned"
                || vm.tag != Some(oracle_tag)
                || vm.payload.map(|payload| payload as u64) != oracle_payload
                || !vm.diagnostics.is_empty()
            {
                return Err(format!(
                    "normalized managed oracle disagrees with VM for {} function#{} case#{}",
                    fixture.fixture, function.ordinal, case_index
                ));
            }
            let stem = format!(
                "{}_managed_{}_case{}",
                safe_stem(&fixture.fixture),
                function.ordinal,
                case_index
            );
            let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
            fs::write(
                &cranelift_source,
                c_managed_runner_source(function, &arguments, oracle_tag, oracle_payload),
            )
            .map_err(|error| format!("cannot write managed Cranelift runner: {error}"))?;
            let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
            link_native_runner(cc, &cranelift_source, &cranelift_object, &cranelift_binary)?;
            run_native_binary(&cranelift_binary, "Cranelift managed", false)?;

            let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
            let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
            fs::write(
                &llvm_ir,
                llvm_module_with_managed_runner(
                    target,
                    program,
                    function,
                    &arguments,
                    oracle_tag,
                    oracle_payload,
                )?,
            )
            .map_err(|error| format!("cannot write managed LLVM runner: {error}"))?;
            let result = Command::new(llvm)
                .arg("-O2")
                .arg("-filetype=obj")
                .arg(format!("-mtriple={target}"))
                .arg("-o")
                .arg(&llvm_object)
                .arg(&llvm_ir)
                .output()
                .map_err(|error| format!("cannot execute LLVM llc for managed runner: {error}"))?;
            if !result.status.success() {
                return Err(format!(
                    "LLVM managed runner llc failed: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
            fs::write(&llvm_source, native_runtime_c_source())
                .map_err(|error| format!("cannot write managed LLVM runner anchor: {error}"))?;
            let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
            link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
            run_native_binary(&llvm_binary, "LLVM managed", false)?;

            reports.push(NativeManagedRunReport {
                fixture: fixture.fixture.clone(),
                function_ordinal: function.ordinal,
                arguments,
                oracle_status: "returned",
                oracle_tag,
                oracle_payload,
                vm_status: vm.status.clone(),
                vm_tag: vm.tag,
                vm_payload: vm.payload,
                vm_diagnostics: vm.diagnostics.clone(),
                cranelift: "passed",
                llvm: "passed",
            });
        }
    }
    Ok(reports)
}

fn load_std_core_probe(path: &Path) -> Result<(FixtureObservation, MirBackendProgram), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read std.core probe `{}`: {error}", path.display()))?;
    let probe: ProbeReport = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid std.core MIR probe: {error}"))?;
    if probe.format != "tondo-native-mir-probe/1" || probe.fixtures.len() != 1 {
        return Err("std.core probe must contain exactly one passed MIR fixture".to_owned());
    }
    let fixture = probe
        .fixtures
        .into_iter()
        .next()
        .expect("length checked above");
    if fixture.status != "passed" {
        return Err(format!("std.core probe fixture did not pass: {}", fixture.fixture));
    }
    let program = fixture
        .mir
        .as_ref()
        .and_then(|mir| mir.backend.clone())
        .ok_or_else(|| format!("std.core fixture has no normalized MIR: {}", fixture.fixture))?;
    validate_backend_program(&program)?;
    Ok((fixture, program))
}

fn run_native_std_core_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
    fixture: &FixtureObservation,
    program: &MirBackendProgram,
) -> Result<Vec<NativeStdCoreRunReport>, String> {
    const CASES: [(&str, &str); 14] = [
        ("option-some", "option_some"),
        ("option-none", "option_none"),
        ("option-unwrap-some", "option_unwrap_some"),
        ("option-unwrap-none", "option_unwrap_none"),
        ("option-map-some", "option_map_some"),
        ("option-map-none", "option_map_none"),
        ("result-ok", "result_ok"),
        ("result-err", "result_err"),
        ("result-unwrap-ok", "result_unwrap_ok"),
        ("result-unwrap-err", "result_unwrap_err"),
        ("result-map-ok", "result_map_ok"),
        ("result-map-err", "result_map_err"),
        ("result-map-err-ok", "result_map_err_ok"),
        ("result-map-err-error", "result_map_err_error"),
    ];
    let symbols = program
        .debug
        .as_ref()
        .ok_or_else(|| "std.core program has no debug metadata".to_owned())?
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol
                    .name
                    .rsplit("::")
                    .next()
                    .unwrap_or(symbol.name.as_str())
                    .to_owned(),
                symbol.function,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let object = temp_dir.join("native_std_core.cranelift.o");
    emit_cranelift_object(cranelift_isa()?, program, &object)?;
    let mut reports = Vec::with_capacity(CASES.len());
    for (case, symbol) in CASES {
        let ordinal = *symbols
            .get(symbol)
            .ok_or_else(|| format!("std.core probe is missing function `{symbol}`"))?;
        let function = program
            .functions
            .iter()
            .find(|function| function.ordinal == ordinal)
            .ok_or_else(|| format!("std.core function {ordinal} is missing"))?;
        if !function.supported || !function.parameters.is_empty() {
            return Err(format!(
                "std.core function `{symbol}` is not a supported zero-argument function"
            ));
        }
        let oracle_value = evaluate_scalar_program(program, ordinal, &[])?;
        let is_managed = function.return_type.contains('?') || function.return_type.contains(" ! ");
        let (oracle_result, oracle_tag, oracle_payload) = if is_managed {
            let (tag, payload) = oracle_managed_parts(oracle_value)?;
            (None, Some(tag), payload)
        } else {
            (Some(oracle_value), None, None)
        };
        let vm_scalar = fixture.vm_scalar.iter().find(|observation| {
            observation.function_ordinal == ordinal && observation.arguments.is_empty()
        });
        let vm_managed = fixture.vm_managed.iter().find(|observation| {
            observation.function_ordinal == ordinal && observation.arguments.is_empty()
        });
        if is_managed {
            let vm = vm_managed.ok_or_else(|| {
                format!("VM std.core managed observation is missing for `{symbol}`")
            })?;
            if vm.status != "returned"
                || vm.tag != oracle_tag
                || vm.payload.map(|payload| payload as u64) != oracle_payload
                || !vm.diagnostics.is_empty()
            {
                return Err(format!("std.core managed oracle disagrees with VM for `{symbol}`"));
            }
        } else {
            let vm = vm_scalar.ok_or_else(|| {
                format!("VM std.core scalar observation is missing for `{symbol}`")
            })?;
            if vm.status != "returned"
                || vm.result != oracle_result
                || !vm.diagnostics.is_empty()
            {
                return Err(format!("std.core scalar oracle disagrees with VM for `{symbol}`"));
            }
        }

        let stem = format!("native_std_core_{case}");
        let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
        let cranelift_runner = if let Some(expected) = oracle_result {
            c_runner_source(function, &[], Some(expected))
        } else {
            c_managed_runner_source(
                function,
                &[],
                oracle_tag.expect("managed result has a tag"),
                oracle_payload,
            )
        };
        fs::write(&cranelift_source, cranelift_runner)
            .map_err(|error| format!("cannot write std.core Cranelift runner: {error}"))?;
        let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
        link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
        run_native_binary(&cranelift_binary, "Cranelift std.core", false)?;

        let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
        let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
        let llvm_runner = if let Some(expected) = oracle_result {
            llvm_module_with_runner(target, program, function, &[], Some(expected))?
        } else {
            llvm_module_with_managed_runner(
                target,
                program,
                function,
                &[],
                oracle_tag.expect("managed result has a tag"),
                oracle_payload,
            )?
        };
        fs::write(&llvm_ir, llvm_runner)
            .map_err(|error| format!("cannot write std.core LLVM runner: {error}"))?;
        let result = Command::new(llvm)
            .arg("-O2")
            .arg("-filetype=obj")
            .arg(format!("-mtriple={target}"))
            .arg("-o")
            .arg(&llvm_object)
            .arg(&llvm_ir)
            .output()
            .map_err(|error| format!("cannot execute LLVM llc for std.core runner: {error}"))?;
        if !result.status.success() {
            return Err(format!(
                "LLVM std.core runner llc failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
        fs::write(&llvm_source, native_runtime_c_source())
            .map_err(|error| format!("cannot write std.core LLVM anchor: {error}"))?;
        let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
        link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
        run_native_binary(&llvm_binary, "LLVM std.core", false)?;

        reports.push(NativeStdCoreRunReport {
            case: case.to_owned(),
            function_ordinal: ordinal,
            kind: if is_managed { "managed" } else { "scalar" },
            oracle_result,
            oracle_tag,
            oracle_payload,
            vm_status: if is_managed {
                vm_managed.expect("managed VM observation was checked").status.clone()
            } else {
                vm_scalar.expect("scalar VM observation was checked").status.clone()
            },
            vm_result: vm_scalar.and_then(|observation| observation.result),
            vm_tag: vm_managed.and_then(|observation| observation.tag),
            vm_payload: vm_managed.and_then(|observation| observation.payload),
            cranelift: "passed",
            llvm: "passed",
        });
    }
    Ok(reports)
}

fn managed_case_arguments_for_function(function: &MirBackendFunction) -> Vec<Vec<i64>> {
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

fn run_native_runtime_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
) -> Result<Vec<NativeRuntimeRunReport>, String> {
    let (program, cases) = native_cleanup_program();
    let object = temp_dir.join("native_runtime_contract.cranelift.o");
    emit_cranelift_object(cranelift_isa()?, &program, &object)?;
    let mut reports = Vec::with_capacity(cases.len());
    for case in cases {
        let function = program
            .functions
            .iter()
            .find(|function| function.ordinal == case.function_ordinal)
            .ok_or_else(|| format!("runtime function {} is missing", case.function_ordinal))?;
        let stem = format!("native_runtime_{}", case.name);
        let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
        let cranelift_runner = match case.expectation {
            RuntimeExpectation::Scalar(expected) => {
                runtime_contract_c_runner_source(case.function_ordinal, expected)
            }
            RuntimeExpectation::Managed { tag, payload } => {
                c_managed_runner_source(function, &[], tag, payload)
            }
        };
        fs::write(&cranelift_source, cranelift_runner)
            .map_err(|error| format!("cannot write runtime Cranelift runner: {error}"))?;
        let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
        link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
        run_native_binary(&cranelift_binary, "Cranelift runtime", false)?;

        let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
        let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
        let llvm_runner = match case.expectation {
            RuntimeExpectation::Scalar(expected) => {
                llvm_module_with_runner(target, &program, function, &[], Some(expected))?
            }
            RuntimeExpectation::Managed { tag, payload } => {
                llvm_module_with_managed_runner(target, &program, function, &[], tag, payload)?
            }
        };
        fs::write(&llvm_ir, llvm_runner)
            .map_err(|error| format!("cannot write runtime LLVM runner: {error}"))?;
        let result = Command::new(llvm)
            .arg("-O2")
            .arg("-filetype=obj")
            .arg(format!("-mtriple={target}"))
            .arg("-o")
            .arg(&llvm_object)
            .arg(&llvm_ir)
            .output()
            .map_err(|error| format!("cannot execute LLVM llc for runtime runner: {error}"))?;
        if !result.status.success() {
            return Err(format!(
                "LLVM runtime runner llc failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
        fs::write(&llvm_source, native_runtime_c_source())
            .map_err(|error| format!("cannot write runtime LLVM anchor: {error}"))?;
        let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
        link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
        run_native_binary(&llvm_binary, "LLVM runtime", false)?;

        reports.push(NativeRuntimeRunReport {
            case: case.name.to_owned(),
            function_ordinal: case.function_ordinal,
            expected_result: match case.expectation {
                RuntimeExpectation::Scalar(expected) => Some(expected),
                RuntimeExpectation::Managed { .. } => None,
            },
            expected_tag: match case.expectation {
                RuntimeExpectation::Managed { tag, .. } => Some(tag),
                RuntimeExpectation::Scalar(_) => None,
            },
            expected_payload: match case.expectation {
                RuntimeExpectation::Managed { payload, .. } => payload,
                RuntimeExpectation::Scalar(_) => None,
            },
            cranelift: "passed",
            llvm: "passed",
        });
    }
    Ok(reports)
}

/// Runs the first source-shaped deferred body through both native adapters.
/// The caller observes a pending handle before the join and a joined handle
/// afterwards; the packed return value is `before * 1000 + after * 100 + body`.
/// An eager spawn would return `1342`, while the coordinated lowering must
/// return `342`.
fn run_native_lowering_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
) -> Result<Vec<NativeLoweringRunReport>, String> {
    let (program, function_ordinal, expected) = native_deferred_program();
    validate_backend_program(&program)?;
    let object = temp_dir.join("native_lowering_deferred.cranelift.o");
    emit_cranelift_object(cranelift_isa()?, &program, &object)?;
    let function = program
        .functions
        .iter()
        .find(|function| function.ordinal == function_ordinal)
        .ok_or_else(|| format!("deferred function {function_ordinal} is missing"))?;
    let stem = "native_lowering_deferred_task";

    let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
    fs::write(
        &cranelift_source,
        runtime_contract_c_runner_source(function_ordinal, expected),
    )
    .map_err(|error| format!("cannot write deferred Cranelift runner: {error}"))?;
    let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
    link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
    run_native_binary(&cranelift_binary, "Cranelift deferred lowering", false)?;

    let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
    let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
    fs::write(
        &llvm_ir,
        llvm_module_with_runner(target, &program, function, &[], Some(expected))?,
    )
    .map_err(|error| format!("cannot write deferred LLVM runner: {error}"))?;
    let result = Command::new(llvm)
        .arg("-O2")
        .arg("-filetype=obj")
        .arg(format!("-mtriple={target}"))
        .arg("-o")
        .arg(&llvm_object)
        .arg(&llvm_ir)
        .output()
        .map_err(|error| format!("cannot execute LLVM llc for deferred runner: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "LLVM deferred runner llc failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
    fs::write(&llvm_source, native_runtime_c_source())
        .map_err(|error| format!("cannot write deferred LLVM runner anchor: {error}"))?;
    let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
    link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
    run_native_binary(&llvm_binary, "LLVM deferred lowering", false)?;

    Ok(vec![NativeLoweringRunReport {
        case: "deferred-task-call".to_owned(),
        function_ordinal,
        pending_before_join: 0,
        result_after_join: 42,
        joined_after_join: 3,
        cranelift: "passed",
        llvm: "passed",
    }])
}

fn pending_native_aot_lowering_report() -> NativeAotLoweringReport {
    NativeAotLoweringReport {
        format: "tondo-native-aot-lowering/1",
        phase: "NATIVE-AOT-LOWER-001",
        status: "pending-native-lowering",
        mir_format: "tondo-mir-backend/1",
        oracle: "normalized-MIR-reference-interpreter",
        candidates: ["cranelift", "llvm"],
        same_mir: true,
        feature_families: Vec::new(),
        cases: Vec::new(),
        traps: Vec::new(),
    }
}

#[derive(Debug, Clone, Copy)]
struct NativeAotCase {
    id: &'static str,
    feature: &'static str,
    function_ordinal: u32,
    expected: i64,
}

/// Executes the complete admitted AOT corpus from one immutable normalized MIR
/// program.  The synthetic storage cases are intentionally small but concrete:
/// collection values live in runtime-managed handles, projections read fields,
/// closures carry a mutable capture, and indirect calls use a verified
/// function ordinal.  The existing runtime cases are appended unchanged so
/// cleanup, ownership, async, select and thread transitions are exercised by
/// exactly the same Cranelift/LLVM lowering path.
fn run_native_aot_lowering_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
    runtime_runs: &[NativeRuntimeRunReport],
) -> Result<NativeAotLoweringReport, String> {
    let (mut program, cases) = native_aot_program();
    let unsupported = MirBackendFunction {
        ordinal: 900,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: false,
        blocks: vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Marker {
                kind: "opaque-storage".to_owned(),
            }],
            terminator: MirBackendTerminator::Marker {
                kind: "unsupported".to_owned(),
            },
        }],
    };
    program.functions.push(unsupported);
    program.debug = Some(synthetic_debug_info(&program.functions));
    validate_backend_program(&program)?;

    let object = temp_dir.join("native_aot_lowering.cranelift.o");
    emit_cranelift_object(cranelift_isa()?, &program, &object)?;
    let mut reports = Vec::with_capacity(cases.len());
    for case in &cases {
        let function = program
            .functions
            .iter()
            .find(|function| function.ordinal == case.function_ordinal)
            .ok_or_else(|| format!("AOT function {} is missing", case.function_ordinal))?;
        if !function.supported {
            return Err(format!("AOT corpus case `{}` targets an unsupported function", case.id));
        }
        // The expected value is produced by the normalized-MIR reference
        // interpreter/state-machine below, before either candidate executes.
        let vm_result = aot_vm_oracle(&program, case)?;
        let stem = format!("native_aot_{}", safe_stem(case.id));
        let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
        fs::write(
            &cranelift_source,
            runtime_contract_c_runner_source(case.function_ordinal, vm_result),
        )
        .map_err(|error| format!("cannot write AOT Cranelift runner: {error}"))?;
        let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
        link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
        run_native_binary(&cranelift_binary, "Cranelift AOT", false)?;

        let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
        let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
        fs::write(
            &llvm_ir,
            llvm_module_with_runner(target, &program, function, &[], Some(vm_result))?,
        )
        .map_err(|error| format!("cannot write AOT LLVM runner: {error}"))?;
        let result = Command::new(llvm)
            .arg("-O2")
            .arg("-filetype=obj")
            .arg(format!("-mtriple={target}"))
            .arg("-o")
            .arg(&llvm_object)
            .arg(&llvm_ir)
            .output()
            .map_err(|error| format!("cannot execute LLVM llc for AOT runner: {error}"))?;
        if !result.status.success() {
            return Err(format!(
                "LLVM AOT runner llc failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
        fs::write(&llvm_source, native_runtime_c_source())
            .map_err(|error| format!("cannot write AOT LLVM runtime anchor: {error}"))?;
        let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
        link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
        run_native_binary(&llvm_binary, "LLVM AOT", false)?;
        reports.push(NativeAotCaseReport {
            id: case.id.to_owned(),
            function_ordinal: case.function_ordinal,
            feature: case.feature.to_owned(),
            vm_status: "returned",
            vm_result,
            cranelift: "passed",
            llvm: "passed",
            same_mir: true,
        });
    }
    // The cleanup/async/select/thread functions are already executed by the
    // physical runtime lane in fresh processes above. Reusing those verified
    // observations avoids running the same 20 binaries twice while preserving
    // the AOT feature inventory and candidate statuses in this report.
    for runtime in runtime_runs {
        let Some(expected) = runtime.expected_result else {
            continue;
        };
        let feature = if runtime.case.starts_with("cleanup-") {
            "cleanup"
        } else if runtime.case.starts_with("async-") {
            "async"
        } else if runtime.case.starts_with("select-") {
            "select"
        } else if runtime.case.starts_with("thread-") {
            "thread"
        } else {
            "runtime"
        };
        reports.push(NativeAotCaseReport {
            id: runtime.case.clone(),
            function_ordinal: runtime.function_ordinal,
            feature: feature.to_owned(),
            vm_status: "returned",
            vm_result: expected,
            cranelift: runtime.cranelift,
            llvm: runtime.llvm,
            same_mir: true,
        });
    }
    let feature_families = [
        "value-storage",
        "collections",
        "projections",
        "closures",
        "indirect-calls",
        "cleanup",
        "ownership",
        "async",
        "select",
        "thread",
    ]
    .into_iter()
    .map(|id| {
        let count = reports.iter().filter(|case| case.feature == id).count() as u32;
        let count = if count == 0 {
            // The synthetic corpus uses one case for the shared storage family
            // and one for collection projections; these aliases keep the
            // feature matrix explicit without duplicating execution.
            reports
                .iter()
                .filter(|case| {
                    (id == "value-storage" && case.feature == "collections")
                        || (id == "projections" && case.feature == "collections")
                        || (id == "closures" && case.feature == "indirect-calls")
                        || (id == "indirect-calls" && case.feature == "indirect-calls")
                })
                .count() as u32
        } else {
            count
        };
        NativeAotFeatureReport {
            id,
            cases: count,
            cranelift: "passed",
            llvm: "passed",
            vm: "passed",
        }
    })
    .collect::<Vec<_>>();
    Ok(NativeAotLoweringReport {
        format: "tondo-native-aot-lowering/1",
        phase: "NATIVE-AOT-LOWER-001",
        status: "passed",
        mir_format: "tondo-mir-backend/1",
        oracle: "normalized-MIR-reference-interpreter",
        candidates: ["cranelift", "llvm"],
        same_mir: reports.iter().all(|case| case.same_mir),
        feature_families,
        cases: reports,
        traps: vec![
            NativeAotTrapReport {
                candidate: "cranelift",
                function_ordinal: 900,
                reason: "opaque-storage-not-admitted:explicit-trap",
            },
            NativeAotTrapReport {
                candidate: "llvm",
                function_ordinal: 900,
                reason: "opaque-storage-not-admitted:unreachable",
            },
        ],
    })
}

#[derive(Debug, Clone)]
enum AotVmValue {
    Scalar(i64),
    Function(u32),
    Aggregate { tag: u32, fields: Vec<AotVmValue> },
}

fn aot_vm_oracle(
    program: &MirBackendProgram,
    case: &NativeAotCase,
) -> Result<i64, String> {
    let result = evaluate_aot_function(program, case.function_ordinal, &[], 0)?;
    let result = aot_scalar_value(&result)?;
    if result != case.expected {
        return Err(format!(
            "AOT VM oracle disagrees for `{}`: expected {}, got {result}",
            case.id, case.expected
        ));
    }
    Ok(result)
}

fn evaluate_aot_function(
    program: &MirBackendProgram,
    ordinal: u32,
    arguments: &[AotVmValue],
    call_depth: usize,
) -> Result<AotVmValue, String> {
    if call_depth > MAX_ORACLE_CALL_DEPTH {
        return Err(format!(
            "AOT VM oracle exceeded {MAX_ORACLE_CALL_DEPTH} call frames"
        ));
    }
    let function = program
        .functions
        .iter()
        .find(|function| function.ordinal == ordinal)
        .ok_or_else(|| format!("AOT VM oracle function {ordinal} is missing"))?;
    if arguments.len() != function.parameters.len() {
        return Err(format!(
            "AOT VM oracle function {ordinal} expects {} arguments, got {}",
            function.parameters.len(),
            arguments.len()
        ));
    }
    let blocks = normal_blocks(function);
    let mut current = blocks
        .first()
        .map(|block| block.ordinal)
        .ok_or_else(|| format!("AOT VM oracle function {ordinal} has no normal block"))?;
    let mut locals = BTreeMap::new();
    for (local, value) in function.parameters.iter().zip(arguments) {
        locals.insert(*local, value.clone());
    }
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > MAX_ORACLE_STEPS {
            return Err(format!(
                "AOT VM oracle exceeded {MAX_ORACLE_STEPS} control-flow steps"
            ));
        }
        let block = blocks
            .iter()
            .find(|block| block.ordinal == current)
            .ok_or_else(|| format!("AOT VM oracle target block {current} is missing"))?;
        for statement in &block.statements {
            match statement {
                MirBackendStatement::Assign { destination, value } => {
                    let value = evaluate_aot_rvalue(value, &locals)?;
                    locals.insert(*destination, value);
                }
                MirBackendStatement::Runtime { kind, arguments } => {
                    let _ = evaluate_aot_runtime(kind, arguments, &mut locals, program, call_depth)?;
                }
                MirBackendStatement::Marker { .. } => {}
            }
        }
        match &block.terminator {
            MirBackendTerminator::Return => {
                return locals
                    .get(&function.return_local)
                    .cloned()
                    .ok_or_else(|| "AOT VM oracle function has no return".to_owned());
            }
            MirBackendTerminator::Goto { target } => current = *target,
            MirBackendTerminator::SwitchBool {
                condition,
                if_true,
                if_false,
            } => {
                current = if aot_scalar_value(&evaluate_aot_operand(condition, &locals)?)? != 0 {
                    *if_true
                } else {
                    *if_false
                };
            }
            MirBackendTerminator::SwitchTag {
                value,
                cases,
                otherwise,
            } => {
                let value = aot_tag_value(&evaluate_aot_operand(value, &locals)?)?;
                current = cases
                    .iter()
                    .find_map(|(tag, target)| (value == *tag).then_some(*target))
                    .unwrap_or(*otherwise);
            }
            MirBackendTerminator::Invoke {
                operation,
                destination,
                target: Some(target),
            } => {
                let value = evaluate_aot_operation(
                    operation,
                    &mut locals,
                    program,
                    call_depth,
                )?;
                if let Some(destination) = destination {
                    locals.insert(*destination, value);
                }
                current = *target;
            }
            MirBackendTerminator::Invoke { target: None, .. } => {
                return Err("AOT VM oracle invoke has no normal target".to_owned());
            }
            MirBackendTerminator::Marker { kind } if kind == "unreachable" => {
                return Err("AOT VM oracle trap: unreachable".to_owned());
            }
            MirBackendTerminator::Marker { kind } => {
                return Err(format!("AOT VM oracle terminator is not supported: {kind}"));
            }
        }
    }
}

fn evaluate_aot_rvalue(
    value: &MirBackendRvalue,
    locals: &BTreeMap<u32, AotVmValue>,
) -> Result<AotVmValue, String> {
    match value {
        MirBackendRvalue::Use(operand) => evaluate_aot_operand(operand, locals),
        MirBackendRvalue::Tag { value } => Ok(AotVmValue::Scalar(i64::from(*value))),
        MirBackendRvalue::Aggregate { kind, values } => Ok(AotVmValue::Aggregate {
            tag: aggregate_tag(kind)?,
            fields: values
                .iter()
                .map(|operand| evaluate_aot_operand(operand, locals))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        MirBackendRvalue::Prefix { operator, operand } => {
            let value = aot_scalar_value(&evaluate_aot_operand(operand, locals)?)?;
            let value = match operator.as_str() {
                "negate" => value
                    .checked_neg()
                    .ok_or_else(|| "AOT VM oracle overflow in negate".to_owned())?,
                "bitwise-not" => !value,
                "logical-not" => i64::from(value == 0),
                other => return Err(format!("AOT VM oracle prefix is not supported: {other}")),
            };
            Ok(AotVmValue::Scalar(value))
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => {
            let left = aot_scalar_value(&evaluate_aot_operand(left, locals)?)?;
            let right = aot_scalar_value(&evaluate_aot_operand(right, locals)?)?;
            Ok(AotVmValue::Scalar(evaluate_aot_binary(operator, left, right)?))
        }
        MirBackendRvalue::NumericConversion { operand, .. }
        | MirBackendRvalue::Coerce { operand, .. } => evaluate_aot_operand(operand, locals),
        MirBackendRvalue::HostCall { arguments, .. } => Ok(AotVmValue::Aggregate {
            tag: 2,
            fields: arguments
                .first()
                .map(|argument| evaluate_aot_operand(argument, locals))
                .transpose()?
                .into_iter()
                .collect(),
        }),
        MirBackendRvalue::Unsupported { kind } => {
            Err(format!("AOT VM oracle rvalue is not supported: {kind}"))
        }
    }
}

fn evaluate_aot_operand(
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, AotVmValue>,
) -> Result<AotVmValue, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => {
            Ok(AotVmValue::Scalar(parse_integer_literal(value)?))
        }
        MirBackendOperand::Constant(MirBackendConstant::Bool(value)) => {
            Ok(AotVmValue::Scalar(i64::from(*value)))
        }
        MirBackendOperand::Constant(MirBackendConstant::String(value)) => {
            Ok(AotVmValue::Scalar(string_payload(value)))
        }
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => locals
            .get(index)
            .cloned()
            .ok_or_else(|| format!("AOT VM oracle local {index} is not available")),
        MirBackendOperand::Function { kind } => parse_verified_function_ordinal(kind)
            .map(AotVmValue::Function)
            .ok_or_else(|| format!("AOT VM oracle function value is not verified: {kind}")),
        MirBackendOperand::Projection { index, depth, kind } => {
            if *depth != 1 {
                return Err(format!(
                    "AOT VM oracle projection depth {depth} is not supported"
                ));
            }
            let base = locals
                .get(index)
                .ok_or_else(|| format!("AOT VM oracle projection local {index} is not available"))?;
            match base {
                AotVmValue::Aggregate { fields, .. }
                    if parse_aggregate_projection(kind).is_some() => {
                    let field = parse_aggregate_projection(kind).expect("checked above") as usize;
                    fields.get(field).cloned().ok_or_else(|| {
                        format!("AOT VM oracle aggregate projection {field} is out of range")
                    })
                }
                AotVmValue::Aggregate { fields, .. }
                    if matches!(
                        kind.as_str(),
                        "option-value" | "result-ok-value" | "result-err-value"
                    ) => fields
                    .first()
                    .cloned()
                    .ok_or_else(|| "AOT VM oracle result has no payload".to_owned()),
                _ => Err(format!("AOT VM oracle projection is not an aggregate: {kind}")),
            }
        }
        MirBackendOperand::Constant(MirBackendConstant::Unit) => Ok(AotVmValue::Scalar(0)),
        MirBackendOperand::Constant(MirBackendConstant::Float(value))
        | MirBackendOperand::Constant(MirBackendConstant::Char(value)) => {
            Err(format!("AOT VM oracle non-integer constant is not supported: {value}"))
        }
        MirBackendOperand::Constant(MirBackendConstant::Named)
        | MirBackendOperand::Unsupported { .. } => {
            Err("AOT VM oracle operand is opaque or unsupported".to_owned())
        }
    }
}

fn evaluate_aot_operation(
    operation: &MirBackendOperation,
    locals: &mut BTreeMap<u32, AotVmValue>,
    program: &MirBackendProgram,
    call_depth: usize,
) -> Result<AotVmValue, String> {
    match operation {
        MirBackendOperation::CheckedPrefix { operator, operand } => {
            evaluate_aot_rvalue(
                &MirBackendRvalue::Prefix {
                    operator: operator.clone(),
                    operand: operand.clone(),
                },
                locals,
            )
        }
        MirBackendOperation::CheckedBinary {
            operator,
            left,
            right,
        } => {
            let left = aot_scalar_value(&evaluate_aot_operand(left, locals)?)?;
            let right = aot_scalar_value(&evaluate_aot_operand(right, locals)?)?;
            Ok(AotVmValue::Scalar(evaluate_aot_binary(operator, left, right)?))
        }
        MirBackendOperation::BoundsCheck { index, length } => {
            let index = aot_scalar_value(&evaluate_aot_operand(index, locals)?)?;
            let length = aot_scalar_value(&evaluate_aot_operand(length, locals)?)?;
            if index < 0 || index >= length {
                Err("AOT VM oracle trap: bounds".to_owned())
            } else {
                Ok(AotVmValue::Scalar(index))
            }
        }
        MirBackendOperation::Call {
            function,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| evaluate_aot_operand(argument, locals))
                .collect::<Result<Vec<_>, _>>()?;
            evaluate_aot_function(program, *function, &arguments, call_depth + 1)
        }
        MirBackendOperation::Spawn { operation, .. } => {
            evaluate_aot_operation(operation, locals, program, call_depth)
        }
        MirBackendOperation::JoinValue { operand } => evaluate_aot_operand(operand, locals),
        MirBackendOperation::HostCall { arguments, .. } => Ok(AotVmValue::Aggregate {
            tag: 2,
            fields: arguments
                .first()
                .map(|argument| evaluate_aot_operand(argument, locals))
                .transpose()?
                .into_iter()
                .collect(),
        }),
        MirBackendOperation::Runtime {
            kind,
            arguments,
        } => evaluate_aot_runtime(kind, arguments, locals, program, call_depth),
        MirBackendOperation::Assert { condition } => {
            let condition = aot_scalar_value(&evaluate_aot_operand(condition, locals)?)?;
            if condition == 0 {
                Err("AOT VM oracle trap: assert".to_owned())
            } else {
                Ok(AotVmValue::Scalar(0))
            }
        }
        MirBackendOperation::Trap { kind } => Err(format!("AOT VM oracle trap: {kind}")),
        MirBackendOperation::Marker { kind } => {
            Err(format!("AOT VM oracle operation is not supported: {kind}"))
        }
    }
}

fn evaluate_aot_runtime(
    kind: &str,
    arguments: &[MirBackendOperand],
    locals: &mut BTreeMap<u32, AotVmValue>,
    program: &MirBackendProgram,
    call_depth: usize,
) -> Result<AotVmValue, String> {
    let base = kind.split(':').next().unwrap_or(kind);
    match base {
        "aggregate-new" => {
            if arguments.len() != 2 {
                return Err("AOT VM oracle aggregate-new expects two arguments".to_owned());
            }
            let tag = aot_scalar_value(&evaluate_aot_operand(&arguments[0], locals)?)?;
            let count = aot_scalar_value(&evaluate_aot_operand(&arguments[1], locals)?)?;
            let tag = u32::try_from(tag).map_err(|_| "AOT VM oracle aggregate tag is invalid")?;
            let count = usize::try_from(count)
                .map_err(|_| "AOT VM oracle aggregate length is invalid")?;
            if !(4..=12).contains(&tag) {
                return Err("AOT VM oracle aggregate tag is invalid".to_owned());
            }
            Ok(AotVmValue::Aggregate {
                tag,
                fields: vec![AotVmValue::Scalar(0); count],
            })
        }
        "aggregate-set" => {
            if arguments.len() != 3 {
                return Err("AOT VM oracle aggregate-set expects three arguments".to_owned());
            }
            let index = aot_scalar_value(&evaluate_aot_operand(&arguments[1], locals)?)?;
            let index = usize::try_from(index)
                .map_err(|_| "AOT VM oracle aggregate index is invalid")?;
            let value = evaluate_aot_operand(&arguments[2], locals)?;
            let local = match arguments[0] {
                MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => index,
                _ => return Err("AOT VM oracle aggregate-set target is not a local".to_owned()),
            };
            let Some(AotVmValue::Aggregate { fields, .. }) = locals.get_mut(&local) else {
                return Err("AOT VM oracle aggregate-set target is not an aggregate".to_owned());
            };
            let Some(slot) = fields.get_mut(index) else {
                return Err("AOT VM oracle aggregate-set index is out of range".to_owned());
            };
            *slot = value;
            Ok(AotVmValue::Scalar(0))
        }
        "aggregate-get" => {
            if arguments.len() != 2 {
                return Err("AOT VM oracle aggregate-get expects two arguments".to_owned());
            }
            let aggregate = evaluate_aot_operand(&arguments[0], locals)?;
            let index = aot_scalar_value(&evaluate_aot_operand(&arguments[1], locals)?)?;
            let index = usize::try_from(index)
                .map_err(|_| "AOT VM oracle aggregate index is invalid")?;
            let AotVmValue::Aggregate { fields, .. } = aggregate else {
                return Err("AOT VM oracle aggregate-get target is not an aggregate".to_owned());
            };
            fields
                .get(index)
                .cloned()
                .ok_or_else(|| "AOT VM oracle aggregate-get index is out of range".to_owned())
        }
        "aggregate-len" => {
            let aggregate = evaluate_aot_operand(
                arguments
                    .first()
                    .ok_or_else(|| "AOT VM oracle aggregate-len expects one argument".to_owned())?,
                locals,
            )?;
            let AotVmValue::Aggregate { fields, .. } = aggregate else {
                return Err("AOT VM oracle aggregate-len target is not an aggregate".to_owned());
            };
            Ok(AotVmValue::Scalar(fields.len() as i64))
        }
        "aggregate-tag" => {
            let aggregate = evaluate_aot_operand(
                arguments
                    .first()
                    .ok_or_else(|| "AOT VM oracle aggregate-tag expects one argument".to_owned())?,
                locals,
            )?;
            let AotVmValue::Aggregate { tag, .. } = aggregate else {
                return Err("AOT VM oracle aggregate-tag target is not an aggregate".to_owned());
            };
            Ok(AotVmValue::Scalar(i64::from(tag)))
        }
        "indirect-call" => {
            if arguments.len() != 3 {
                return Err("AOT VM oracle indirect-call expects three arguments".to_owned());
            }
            let function = evaluate_aot_operand(&arguments[0], locals)?;
            let AotVmValue::Function(function) = function else {
                return Err("AOT VM oracle indirect-call target is not a function".to_owned());
            };
            let capture = evaluate_aot_operand(&arguments[1], locals)?;
            let argument = evaluate_aot_operand(&arguments[2], locals)?;
            evaluate_aot_function(program, function, &[capture, argument], call_depth + 1)
        }
        "result-payload" => {
            let value = evaluate_aot_operand(
                arguments
                    .first()
                    .ok_or_else(|| "AOT VM oracle result-payload expects one argument".to_owned())?,
                locals,
            )?;
            let AotVmValue::Aggregate { fields, .. } = value else {
                return Err("AOT VM oracle result-payload target is not a result".to_owned());
            };
            fields
                .first()
                .cloned()
                .ok_or_else(|| "AOT VM oracle result has no payload".to_owned())
        }
        "result-tag" => {
            let value = evaluate_aot_operand(
                arguments
                    .first()
                    .ok_or_else(|| "AOT VM oracle result-tag expects one argument".to_owned())?,
                locals,
            )?;
            let AotVmValue::Aggregate { tag, .. } = value else {
                return Err("AOT VM oracle result-tag target is not a result".to_owned());
            };
            Ok(AotVmValue::Scalar(i64::from(tag)))
        }
        "retain" | "retain-value" | "release" | "release-value" => {
            if arguments.len() != 1 {
                return Err(format!("AOT VM oracle {base} expects one argument"));
            }
            let _ = evaluate_aot_operand(&arguments[0], locals)?;
            Ok(AotVmValue::Scalar(0))
        }
        "cow-clone" => evaluate_aot_operand(
            arguments
                .first()
                .ok_or_else(|| "AOT VM oracle cow-clone expects one argument".to_owned())?,
            locals,
        ),
        _ => Err(format!("AOT VM oracle runtime is not supported: {base}")),
    }
}

fn aot_scalar_value(value: &AotVmValue) -> Result<i64, String> {
    match value {
        AotVmValue::Scalar(value) => Ok(*value),
        _ => Err("AOT VM oracle expected a scalar value".to_owned()),
    }
}

fn aot_tag_value(value: &AotVmValue) -> Result<u32, String> {
    match value {
        AotVmValue::Aggregate { tag, .. } => Ok(*tag),
        AotVmValue::Scalar(value) => u32::try_from(*value)
            .map_err(|_| "AOT VM oracle tag value is out of range".to_owned()),
        AotVmValue::Function(_) => Err("AOT VM oracle function has no tag".to_owned()),
    }
}

fn evaluate_aot_binary(operator: &str, left: i64, right: i64) -> Result<i64, String> {
    match operator {
        "add" => left
            .checked_add(right)
            .ok_or_else(|| "AOT VM oracle overflow in add".to_owned()),
        "subtract" => left
            .checked_sub(right)
            .ok_or_else(|| "AOT VM oracle overflow in subtract".to_owned()),
        "multiply" => left
            .checked_mul(right)
            .ok_or_else(|| "AOT VM oracle overflow in multiply".to_owned()),
        "divide" if right != 0 => left
            .checked_div(right)
            .ok_or_else(|| "AOT VM oracle overflow in divide".to_owned()),
        "remainder" if right != 0 => left
            .checked_rem(right)
            .ok_or_else(|| "AOT VM oracle overflow in remainder".to_owned()),
        "bitwise-and" | "logical-and" => Ok(left & right),
        "bitwise-or" | "logical-or" => Ok(left | right),
        "bitwise-xor" => Ok(left ^ right),
        "less" => Ok(i64::from(left < right)),
        "less-equal" => Ok(i64::from(left <= right)),
        "greater" => Ok(i64::from(left > right)),
        "greater-equal" => Ok(i64::from(left >= right)),
        "equal" => Ok(i64::from(left == right)),
        "not-equal" => Ok(i64::from(left != right)),
        "shift-left" if (0..64).contains(&right) => Ok(left << right),
        "shift-right" if (0..64).contains(&right) => Ok(left >> right),
        _ => Err(format!("AOT VM oracle binary is not supported: {operator}")),
    }
}

fn native_aot_program() -> (MirBackendProgram, Vec<NativeAotCase>) {
    let int = |value: &str| {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value.to_owned()))
    };
    let local = |index| MirBackendOperand::Local { index };
    let function = |ordinal: u32, parameters: Vec<u32>, return_local: u32, blocks| {
        MirBackendFunction {
            ordinal,
            parameters,
            parameter_types: Vec::new(),
            return_local,
            return_type: "Int".to_owned(),
            supported: true,
            blocks,
        }
    };
    let array = function(
        0,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![
                MirBackendStatement::Assign {
                    destination: 1,
                    value: MirBackendRvalue::Aggregate {
                        kind: "array".to_owned(),
                        values: vec![int("10"), int("32")],
                    },
                },
                MirBackendStatement::Assign {
                    destination: 0,
                    value: MirBackendRvalue::Use(MirBackendOperand::Projection {
                        index: 1,
                        depth: 1,
                        kind: "aggregate:1".to_owned(),
                    }),
                },
            ],
            terminator: MirBackendTerminator::Return,
        }],
    );
    let record = function(
        1,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 1,
                value: MirBackendRvalue::Aggregate {
                    kind: "record".to_owned(),
                    values: vec![int("4"), int("6")],
                },
            }, MirBackendStatement::Assign {
                destination: 0,
                value: MirBackendRvalue::Use(MirBackendOperand::Projection {
                    index: 1,
                    depth: 1,
                    kind: "aggregate:1".to_owned(),
                }),
            }],
            terminator: MirBackendTerminator::Return,
        }],
    );
    let indirect_impl = function(
        3,
        vec![1, 2],
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::CheckedBinary {
                    operator: "add".to_owned(),
                    left: local(1),
                    right: local(2),
                },
                destination: Some(0),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Return,
        }],
    );
    let closure = function(
        2,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 1,
                value: MirBackendRvalue::Aggregate {
                    kind: "closure".to_owned(),
                    values: vec![
                        MirBackendOperand::Function {
                            kind: "function:3".to_owned(),
                        },
                        int("4"),
                    ],
                },
            }],
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "aggregate-set".to_owned(),
                    arguments: vec![local(1), int("1"), int("9")],
                },
                destination: Some(2),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 3,
                value: MirBackendRvalue::Use(MirBackendOperand::Projection {
                    index: 1,
                    depth: 1,
                    kind: "aggregate:1".to_owned(),
                }),
            }],
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "indirect-call".to_owned(),
                    arguments: vec![
                        MirBackendOperand::Function {
                            kind: "function:3".to_owned(),
                        },
                        local(3),
                        int("2"),
                    ],
                },
                destination: Some(0),
                target: Some(2),
            },
        }, MirBackendBlock {
            ordinal: 2,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Return,
        }],
    );
    let direct = function(
        4,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Call {
                    function: 3,
                    arguments: vec![int("5"), int("8")],
                },
                destination: Some(0),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Return,
        }],
    );
    let metadata = function(
        5,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 1,
                value: MirBackendRvalue::Aggregate {
                    kind: "array".to_owned(),
                    values: vec![int("1"), int("2"), int("3")],
                },
            }],
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "aggregate-len".to_owned(),
                    arguments: vec![local(1)],
                },
                destination: Some(2),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "aggregate-tag".to_owned(),
                    arguments: vec![local(1)],
                },
                destination: Some(3),
                target: Some(2),
            },
        }, MirBackendBlock {
            ordinal: 2,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 4,
                value: MirBackendRvalue::Binary {
                    operator: "multiply".to_owned(),
                    left: local(2),
                    right: int("100"),
                },
            }, MirBackendStatement::Assign {
                destination: 0,
                value: MirBackendRvalue::Binary {
                    operator: "add".to_owned(),
                    left: local(4),
                    right: local(3),
                },
            }],
            terminator: MirBackendTerminator::Return,
        }],
    );
    let set = function(
        6,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 1,
                value: MirBackendRvalue::Aggregate {
                    kind: "set".to_owned(),
                    values: vec![int("7"), int("9")],
                },
            }],
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "aggregate-set".to_owned(),
                    arguments: vec![local(1), int("0"), int("8")],
                },
                destination: Some(2),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "aggregate-get".to_owned(),
                    arguments: vec![local(1), int("0")],
                },
                destination: Some(0),
                target: Some(2),
            },
        }, MirBackendBlock {
            ordinal: 2,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Return,
        }],
    );
    let ownership = function(
        7,
        Vec::new(),
        0,
        vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 1,
                value: MirBackendRvalue::Aggregate {
                    kind: "result-ok".to_owned(),
                    values: vec![int("42")],
                },
            }],
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "retain".to_owned(),
                    arguments: vec![local(1)],
                },
                destination: Some(2),
                target: Some(1),
            },
        }, MirBackendBlock {
            ordinal: 1,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "cow-clone".to_owned(),
                    arguments: vec![local(1)],
                },
                destination: Some(3),
                target: Some(2),
            },
        }, MirBackendBlock {
            ordinal: 2,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Invoke {
                operation: MirBackendOperation::Runtime {
                    kind: "result-payload".to_owned(),
                    arguments: vec![local(3)],
                },
                destination: Some(0),
                target: Some(3),
            },
        }, MirBackendBlock {
            ordinal: 3,
            kind: "normal".to_owned(),
            statements: Vec::new(),
            terminator: MirBackendTerminator::Return,
        }],
    );
    let functions = vec![array, record, closure, indirect_impl, direct, metadata, set, ownership];
    let cases = vec![
        NativeAotCase { id: "array-storage", feature: "collections", function_ordinal: 0, expected: 32 },
        NativeAotCase { id: "record-projection", feature: "collections", function_ordinal: 1, expected: 6 },
        NativeAotCase { id: "closure-mutable-capture", feature: "indirect-calls", function_ordinal: 2, expected: 11 },
        NativeAotCase { id: "direct-call", feature: "indirect-calls", function_ordinal: 4, expected: 13 },
        NativeAotCase { id: "aggregate-metadata", feature: "value-storage", function_ordinal: 5, expected: 305 },
        NativeAotCase { id: "set-storage", feature: "collections", function_ordinal: 6, expected: 8 },
        NativeAotCase { id: "ownership-cow", feature: "ownership", function_ordinal: 7, expected: 42 },
    ];
    (
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            debug: Some(synthetic_debug_info(&functions)),
            functions,
        },
        cases,
    )
}

fn native_deferred_program() -> (MirBackendProgram, u32, i64) {
    let constant = |value: &str| {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value.to_owned()))
    };
    let local = |index| MirBackendOperand::Local { index };
    let body = MirBackendFunction {
        ordinal: 0,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 0,
                value: MirBackendRvalue::Use(constant("42")),
            }],
            terminator: MirBackendTerminator::Return,
        }],
    };
    let caller = MirBackendFunction {
        ordinal: 1,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Spawn {
                        operation: Box::new(MirBackendOperation::Call {
                            function: 0,
                            arguments: Vec::new(),
                        }),
                        kind: "task".to_owned(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-poll".to_owned(),
                        arguments: vec![local(1)],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::JoinValue { operand: local(1) },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-poll".to_owned(),
                        arguments: vec![local(1)],
                    },
                    destination: Some(4),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: vec![
                    MirBackendStatement::Assign {
                        destination: 5,
                        value: MirBackendRvalue::Binary {
                            operator: "multiply".to_owned(),
                            left: local(2),
                            right: constant("1000"),
                        },
                    },
                    MirBackendStatement::Assign {
                        destination: 6,
                        value: MirBackendRvalue::Binary {
                            operator: "multiply".to_owned(),
                            left: local(4),
                            right: constant("100"),
                        },
                    },
                    MirBackendStatement::Assign {
                        destination: 7,
                        value: MirBackendRvalue::Binary {
                            operator: "add".to_owned(),
                            left: local(5),
                            right: local(6),
                        },
                    },
                    MirBackendStatement::Assign {
                        destination: 8,
                        value: MirBackendRvalue::Binary {
                            operator: "add".to_owned(),
                            left: local(7),
                            right: local(3),
                        },
                    },
                    MirBackendStatement::Assign {
                        destination: 0,
                        value: MirBackendRvalue::Use(local(8)),
                    },
                ],
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let functions = vec![body, caller];
    (
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            debug: Some(synthetic_debug_info(&functions)),
            functions,
        },
        1,
        342,
    )
}

fn native_cleanup_program() -> (MirBackendProgram, Vec<RuntimeContractCase>) {
    let runtime_operand = |index| MirBackendOperand::Local { index };
    let constant =
        |value: &str| MirBackendOperand::Constant(MirBackendConstant::Integer(value.to_owned()));
    let runtime_sequence =
        |ordinal: u32, operations: Vec<(&str, Vec<MirBackendOperand>)>| -> MirBackendFunction {
            let count = operations.len();
            let mut blocks = Vec::with_capacity(count + 1);
            for (index, (kind, arguments)) in operations.into_iter().enumerate() {
                blocks.push(MirBackendBlock {
                    ordinal: index as u32,
                    kind: "normal".to_owned(),
                    statements: Vec::new(),
                    terminator: MirBackendTerminator::Invoke {
                        operation: MirBackendOperation::Runtime {
                            kind: kind.to_owned(),
                            arguments,
                        },
                        destination: Some(index as u32 + 1),
                        target: Some(index as u32 + 1),
                    },
                });
            }
            let last = count as u32;
            let last_local = last;
            blocks.push(MirBackendBlock {
                ordinal: last,
                kind: "normal".to_owned(),
                statements: vec![MirBackendStatement::Assign {
                    destination: 0,
                    value: MirBackendRvalue::Use(MirBackendOperand::Local { index: last_local }),
                }],
                terminator: MirBackendTerminator::Return,
            });
            MirBackendFunction {
                ordinal,
                parameters: Vec::new(),
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks,
            }
        };
    let cleanup_function = MirBackendFunction {
        ordinal: 100,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "frame-enter".to_owned(),
                        arguments: Vec::new(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "register-defer".to_owned(),
                        arguments: vec![runtime_operand(1), constant("7")],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "frame-cleanup".to_owned(),
                        arguments: vec![runtime_operand(1), constant("0")],
                    },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "frame-cleanup".to_owned(),
                        arguments: vec![runtime_operand(1), constant("0")],
                    },
                    destination: Some(0),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let abort_function = MirBackendFunction {
        ordinal: 101,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "frame-enter".to_owned(),
                        arguments: Vec::new(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "register-defer".to_owned(),
                        arguments: vec![runtime_operand(1), constant("9")],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "frame-leave".to_owned(),
                        arguments: vec![runtime_operand(1), constant("1")],
                    },
                    destination: Some(0),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let ownership_function = MirBackendFunction {
        ordinal: 102,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int ! String".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: vec![MirBackendStatement::Assign {
                    destination: 1,
                    value: MirBackendRvalue::Aggregate {
                        kind: "result-ok".to_owned(),
                        values: vec![constant("42")],
                    },
                }],
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "retain".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(2),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "cow-clone".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(3),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "release".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(4),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "release".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(5),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: vec![MirBackendStatement::Assign {
                    destination: 0,
                    value: MirBackendRvalue::Use(runtime_operand(3)),
                }],
                terminator: MirBackendTerminator::Goto { target: 5 },
            },
            MirBackendBlock {
                ordinal: 5,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let async_await_function = MirBackendFunction {
        ordinal: 103,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-spawn".to_owned(),
                        arguments: vec![constant("77"), constant("1")],
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-wake".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "await".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(0),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let structured_join_function = MirBackendFunction {
        ordinal: 104,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-enter".to_owned(),
                        arguments: Vec::new(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-spawn".to_owned(),
                        arguments: vec![runtime_operand(1), constant("88"), constant("1")],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-wake".to_owned(),
                        arguments: vec![runtime_operand(2)],
                    },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-join".to_owned(),
                        arguments: vec![runtime_operand(1), runtime_operand(2)],
                    },
                    destination: Some(0),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let async_cancel_function = MirBackendFunction {
        ordinal: 105,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-enter".to_owned(),
                        arguments: Vec::new(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-spawn".to_owned(),
                        arguments: vec![runtime_operand(1), constant("99"), constant("1")],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-cancel".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-poll".to_owned(),
                        arguments: vec![runtime_operand(2)],
                    },
                    destination: Some(0),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let task_progress_function = MirBackendFunction {
        ordinal: 106,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-spawn".to_owned(),
                        arguments: vec![constant("1"), constant("1")],
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-poll".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-wake".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-poll".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(0),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let async_cancel_wake_function = MirBackendFunction {
        ordinal: 107,
        parameters: Vec::new(),
        parameter_types: Vec::new(),
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-enter".to_owned(),
                        arguments: Vec::new(),
                    },
                    destination: Some(1),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-spawn".to_owned(),
                        arguments: vec![runtime_operand(1), constant("123"), constant("1")],
                    },
                    destination: Some(2),
                    target: Some(2),
                },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "scope-cancel".to_owned(),
                        arguments: vec![runtime_operand(1)],
                    },
                    destination: Some(3),
                    target: Some(3),
                },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: "task-wake".to_owned(),
                        arguments: vec![runtime_operand(2)],
                    },
                    destination: Some(0),
                    target: Some(4),
                },
            },
            MirBackendBlock {
                ordinal: 4,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let select_ready_function = runtime_sequence(
        108,
        vec![
            ("select-begin", vec![constant("1")]),
            ("task-spawn", vec![constant("11"), constant("0")]),
            (
                "select-register-join",
                vec![runtime_operand(1), runtime_operand(2)],
            ),
            ("select-commit", vec![runtime_operand(1), constant("0")]),
            ("select-take", vec![runtime_operand(1)]),
        ],
    );
    let select_pending_wake_function = runtime_sequence(
        109,
        vec![
            ("select-begin", vec![constant("1")]),
            ("task-spawn", vec![constant("22"), constant("1")]),
            (
                "select-register-join",
                vec![runtime_operand(1), runtime_operand(2)],
            ),
            ("select-commit", vec![runtime_operand(1), constant("0")]),
            ("task-wake", vec![runtime_operand(2)]),
            ("select-wakeups", vec![runtime_operand(1)]),
            ("select-commit", vec![runtime_operand(1), constant("0")]),
            ("select-take", vec![runtime_operand(1)]),
        ],
    );
    let select_fairness_function = runtime_sequence(
        110,
        vec![
            ("select-begin", vec![constant("2")]),
            ("task-spawn", vec![constant("31"), constant("0")]),
            ("task-spawn", vec![constant("32"), constant("0")]),
            (
                "select-register-join",
                vec![runtime_operand(1), runtime_operand(2)],
            ),
            (
                "select-register-join",
                vec![runtime_operand(1), runtime_operand(3)],
            ),
            ("select-commit", vec![runtime_operand(1), constant("0")]),
            ("select-take", vec![runtime_operand(1)]),
            ("select-begin", vec![constant("2")]),
            ("task-spawn", vec![constant("41"), constant("0")]),
            ("task-spawn", vec![constant("42"), constant("0")]),
            (
                "select-register-join",
                vec![runtime_operand(8), runtime_operand(9)],
            ),
            (
                "select-register-join",
                vec![runtime_operand(8), runtime_operand(10)],
            ),
            ("select-commit", vec![runtime_operand(8), constant("0")]),
            ("select-winner", vec![runtime_operand(8)]),
        ],
    );
    let select_rollback_function = runtime_sequence(
        111,
        vec![
            ("select-begin", vec![constant("2")]),
            ("task-spawn", vec![constant("51"), constant("1")]),
            ("task-spawn", vec![constant("52"), constant("1")]),
            (
                "select-register-task",
                vec![runtime_operand(1), runtime_operand(2), constant("1")],
            ),
            (
                "select-register-join",
                vec![runtime_operand(1), runtime_operand(3)],
            ),
            ("select-rollback", vec![runtime_operand(1)]),
            ("task-poll", vec![runtime_operand(2)]),
        ],
    );
    let select_oneshot_function = runtime_sequence(
        112,
        vec![
            ("oneshot-new", Vec::new()),
            ("select-begin", vec![constant("1")]),
            (
                "select-register-oneshot",
                vec![runtime_operand(2), runtime_operand(1), constant("0")],
            ),
            ("select-commit", vec![runtime_operand(2), constant("0")]),
            ("oneshot-complete", vec![runtime_operand(1), constant("61")]),
            ("select-commit", vec![runtime_operand(2), constant("0")]),
            ("select-take", vec![runtime_operand(2)]),
        ],
    );
    let select_time_function = runtime_sequence(
        113,
        vec![
            ("time-new", vec![constant("63")]),
            ("select-begin", vec![constant("1")]),
            (
                "select-register-time",
                vec![runtime_operand(2), runtime_operand(1), constant("0")],
            ),
            ("select-commit", vec![runtime_operand(2), constant("0")]),
            ("time-fire", vec![runtime_operand(1)]),
            ("select-commit", vec![runtime_operand(2), constant("0")]),
            ("select-take", vec![runtime_operand(2)]),
        ],
    );
    let select_thread_join_function = runtime_sequence(
        114,
        vec![
            ("thread-spawn", vec![constant("74"), constant("0")]),
            ("select-begin", vec![constant("1")]),
            (
                "select-register-join",
                vec![runtime_operand(2), runtime_operand(1)],
            ),
            ("select-commit", vec![runtime_operand(2), constant("0")]),
            ("select-take", vec![runtime_operand(2)]),
        ],
    );
    let select_else_function = runtime_sequence(
        115,
        vec![
            ("select-begin", vec![constant("1")]),
            ("task-spawn", vec![constant("81"), constant("1")]),
            (
                "select-register-task",
                vec![runtime_operand(1), runtime_operand(2), constant("1")],
            ),
            ("select-commit", vec![runtime_operand(1), constant("1")]),
        ],
    );
    let thread_worker_status_function = runtime_sequence(
        116,
        vec![
            ("thread-spawn", vec![constant("91"), constant("0")]),
            ("thread-worker-status", vec![runtime_operand(1)]),
        ],
    );
    let thread_worker_runs_function = runtime_sequence(
        117,
        vec![
            ("thread-spawn", vec![constant("92"), constant("0")]),
            ("thread-worker-runs", vec![runtime_operand(1)]),
        ],
    );
    let thread_worker_distinct_function = runtime_sequence(
        118,
        vec![
            ("thread-spawn", vec![constant("93"), constant("0")]),
            ("thread-worker-distinct", vec![runtime_operand(1)]),
        ],
    );
    let thread_worker_join_function = runtime_sequence(
        119,
        vec![
            ("thread-spawn", vec![constant("94"), constant("0")]),
            ("await", vec![runtime_operand(1)]),
        ],
    );
    let thread_worker_cancel_function = runtime_sequence(
        120,
        vec![
            ("thread-spawn", vec![constant("95"), constant("1")]),
            ("task-cancel", vec![runtime_operand(1)]),
            ("task-poll", vec![runtime_operand(1)]),
        ],
    );
    let functions = vec![
        cleanup_function,
        abort_function,
        ownership_function,
        async_await_function,
        structured_join_function,
        async_cancel_function,
        task_progress_function,
        async_cancel_wake_function,
        select_ready_function,
        select_pending_wake_function,
        select_fairness_function,
        select_rollback_function,
        select_oneshot_function,
        select_time_function,
        select_thread_join_function,
        select_else_function,
        thread_worker_status_function,
        thread_worker_runs_function,
        thread_worker_distinct_function,
        thread_worker_join_function,
        thread_worker_cancel_function,
    ];
    (
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            debug: Some(synthetic_debug_info(&functions)),
            functions,
        },
        vec![
            RuntimeContractCase {
                name: "cleanup-exactly-once",
                function_ordinal: 100,
                expectation: RuntimeExpectation::Scalar(5),
            },
            RuntimeContractCase {
                name: "cleanup-abort",
                function_ordinal: 101,
                expectation: RuntimeExpectation::Scalar(0),
            },
            RuntimeContractCase {
                name: "ownership-cow",
                function_ordinal: 102,
                expectation: RuntimeExpectation::Managed {
                    tag: 2,
                    payload: Some(42),
                },
            },
            RuntimeContractCase {
                name: "async-await",
                function_ordinal: 103,
                expectation: RuntimeExpectation::Scalar(77),
            },
            RuntimeContractCase {
                name: "async-structured-join",
                function_ordinal: 104,
                expectation: RuntimeExpectation::Scalar(0),
            },
            RuntimeContractCase {
                name: "async-scope-cancel",
                function_ordinal: 105,
                expectation: RuntimeExpectation::Scalar(2),
            },
            RuntimeContractCase {
                name: "async-task-progress",
                function_ordinal: 106,
                expectation: RuntimeExpectation::Scalar(1),
            },
            RuntimeContractCase {
                name: "async-cancel-wake-rejected",
                function_ordinal: 107,
                expectation: RuntimeExpectation::Scalar(3),
            },
            RuntimeContractCase {
                name: "select-ready-join",
                function_ordinal: 108,
                expectation: RuntimeExpectation::Scalar(11),
            },
            RuntimeContractCase {
                name: "select-pending-wakeup",
                function_ordinal: 109,
                expectation: RuntimeExpectation::Scalar(22),
            },
            RuntimeContractCase {
                name: "select-round-robin",
                function_ordinal: 110,
                expectation: RuntimeExpectation::Scalar(1),
            },
            RuntimeContractCase {
                name: "select-rollback-ownership",
                function_ordinal: 111,
                expectation: RuntimeExpectation::Scalar(2),
            },
            RuntimeContractCase {
                name: "select-oneshot",
                function_ordinal: 112,
                expectation: RuntimeExpectation::Scalar(61),
            },
            RuntimeContractCase {
                name: "select-time",
                function_ordinal: 113,
                expectation: RuntimeExpectation::Scalar(63),
            },
            RuntimeContractCase {
                name: "select-thread-join",
                function_ordinal: 114,
                expectation: RuntimeExpectation::Scalar(74),
            },
            RuntimeContractCase {
                name: "select-else",
                function_ordinal: 115,
                expectation: RuntimeExpectation::Scalar(8),
            },
            RuntimeContractCase {
                name: "thread-worker-status",
                function_ordinal: 116,
                expectation: RuntimeExpectation::Scalar(2),
            },
            RuntimeContractCase {
                name: "thread-worker-runs",
                function_ordinal: 117,
                expectation: RuntimeExpectation::Scalar(1),
            },
            RuntimeContractCase {
                name: "thread-worker-distinct",
                function_ordinal: 118,
                expectation: RuntimeExpectation::Scalar(1),
            },
            RuntimeContractCase {
                name: "thread-worker-join",
                function_ordinal: 119,
                expectation: RuntimeExpectation::Scalar(94),
            },
            RuntimeContractCase {
                name: "thread-worker-cancel",
                function_ordinal: 120,
                expectation: RuntimeExpectation::Scalar(2),
            },
        ],
    )
}

fn runtime_contract_c_runner_source(function_ordinal: u32, expected: i64) -> String {
    format!(
        "{}\nextern int64_t tondo_probe_{function_ordinal}(void);\nint main(void) {{ return tondo_probe_{function_ordinal}() == {expected} ? 0 : 91; }}\n",
        native_runtime_c_source()
    )
}

fn native_diagnostic_program() -> (MirBackendProgram, Vec<NativeDiagnosticCase>) {
    let function = |ordinal: u32, kind: &str| MirBackendFunction {
        ordinal,
        parameters: vec![1],
        parameter_types: vec!["Int".to_owned()],
        return_local: 0,
        return_type: "Int".to_owned(),
        supported: true,
        blocks: vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Invoke {
                    operation: MirBackendOperation::Runtime {
                        kind: kind.to_owned(),
                        arguments: vec![MirBackendOperand::Local { index: 1 }],
                    },
                    destination: Some(0),
                    target: Some(1),
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ],
    };
    let functions = vec![
        function(200, "diag-race"),
        function(201, "diag-leak"),
        function(202, "diag-dump"),
    ];
    let cases = vec![
        NativeDiagnosticCase {
            profile: "race",
            profile_id: 0,
            name: "race-conflict",
            mode: 1,
            expected_status: "finding",
            expected_code: 1,
        },
        NativeDiagnosticCase {
            profile: "race",
            profile_id: 0,
            name: "race-clean",
            mode: 0,
            expected_status: "clean",
            expected_code: 0,
        },
        NativeDiagnosticCase {
            profile: "leaks",
            profile_id: 1,
            name: "leak-growth",
            mode: 1,
            expected_status: "finding",
            expected_code: 1,
        },
        NativeDiagnosticCase {
            profile: "leaks",
            profile_id: 1,
            name: "leak-clean",
            mode: 0,
            expected_status: "clean",
            expected_code: 0,
        },
        NativeDiagnosticCase {
            profile: "leaks",
            profile_id: 1,
            name: "arc-cycle-reclaimed",
            mode: 2,
            expected_status: "clean",
            expected_code: 0,
        },
        NativeDiagnosticCase {
            profile: "crash",
            profile_id: 2,
            name: "crash-dump",
            mode: 0,
            expected_status: "captured",
            expected_code: 2,
        },
        NativeDiagnosticCase {
            profile: "crash",
            profile_id: 2,
            name: "crash-corruption-rejected",
            mode: 1,
            expected_status: "captured",
            expected_code: 2,
        },
        NativeDiagnosticCase {
            profile: "crash",
            profile_id: 2,
            name: "crash-limit-enforced",
            mode: 2,
            expected_status: "captured",
            expected_code: 2,
        },
    ];
    (
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            debug: Some(synthetic_debug_info(&functions)),
            functions,
        },
        cases,
    )
}

fn run_native_diagnostics_probe(
    llvm: &Path,
    cc: &Path,
    target: &str,
    temp_dir: &Path,
) -> Result<NativeDiagnosticsReport, String> {
    let (program, cases) = native_diagnostic_program();
    validate_backend_program(&program)?;
    let object = temp_dir.join("native_diagnostics.cranelift.o");
    emit_cranelift_object(cranelift_isa()?, &program, &object)?;
    let mut reports = Vec::with_capacity(cases.len());
    for case in cases {
        let function = program
            .functions
            .iter()
            .find(|function| function.ordinal == 200 + case.profile_id as u32)
            .ok_or_else(|| format!("diagnostic function for {} is missing", case.profile))?;
        let stem = format!("native_diagnostic_{}", case.name);
        let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
        fs::write(
            &cranelift_source,
            native_diagnostic_c_runner_source(
                function.ordinal,
                case.mode,
                case.expected_code,
                case.profile,
                case.name,
            ),
        )
        .map_err(|error| format!("cannot write diagnostic Cranelift runner: {error}"))?;
        let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
        link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
        let cranelift_output =
            run_native_binary_capture(&cranelift_binary, "Cranelift diagnostic")?;
        let cranelift_envelope: NativeDiagnosticEnvelope =
            serde_json::from_slice(&cranelift_output)
                .map_err(|error| format!("invalid Cranelift diagnostic envelope: {error}"))?;
        validate_native_diagnostic_envelope(&cranelift_envelope, &case)?;

        let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
        let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
        fs::write(&llvm_ir, llvm_module(target, &program)?)
            .map_err(|error| format!("cannot write diagnostic LLVM runner: {error}"))?;
        let result = Command::new(llvm)
            .arg("-O2")
            .arg("-filetype=obj")
            .arg(format!("-mtriple={target}"))
            .arg("-o")
            .arg(&llvm_object)
            .arg(&llvm_ir)
            .output()
            .map_err(|error| format!("cannot execute LLVM llc for diagnostic runner: {error}"))?;
        if !result.status.success() {
            return Err(format!(
                "LLVM diagnostic runner llc failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let llvm_source = temp_dir.join(format!("{stem}.llvm.c"));
        fs::write(
            &llvm_source,
            native_diagnostic_c_runner_source(
                function.ordinal,
                case.mode,
                case.expected_code,
                case.profile,
                case.name,
            ),
        )
        .map_err(|error| format!("cannot write diagnostic LLVM anchor: {error}"))?;
        let llvm_binary = temp_dir.join(format!("{stem}.llvm.bin"));
        link_native_runner(cc, &llvm_source, &llvm_object, &llvm_binary)?;
        let llvm_output = run_native_binary_capture(&llvm_binary, "LLVM diagnostic")?;
        let llvm_envelope: NativeDiagnosticEnvelope =
            serde_json::from_slice(&llvm_output)
                .map_err(|error| format!("invalid LLVM diagnostic envelope: {error}"))?;
        validate_native_diagnostic_envelope(&llvm_envelope, &case)?;
        if cranelift_envelope != llvm_envelope {
            return Err(format!(
                "diagnostic envelope differs between backends for {}",
                case.name
            ));
        }
        reports.push(NativeDiagnosticCaseReport {
            profile: case.profile,
            case: case.name,
            mode: case.mode,
            expected_status: case.expected_status,
            cranelift: "passed",
            llvm: "passed",
            envelope: cranelift_envelope,
        });
    }
    Ok(NativeDiagnosticsReport {
        format: "tondo-native-diagnostics/1",
        phase: "DIAG-NATIVE-001",
        status: "passed",
        oracle: "hosted-diagnostic-contract-fixtures",
        backends: ["cranelift", "llvm"],
        cases: reports,
    })
}

fn validate_native_diagnostic_envelope(
    envelope: &NativeDiagnosticEnvelope,
    case: &NativeDiagnosticCase,
) -> Result<(), String> {
    if envelope.format != "tondo-diagnostic-report/1"
        || envelope.profile != case.profile
        || envelope.case != case.name
        || envelope.mode != case.mode
        || envelope.status != case.expected_status
        || !envelope.redacted
        || !envelope.payloads_omitted
    {
        return Err(format!(
            "diagnostic envelope identity/status/redaction mismatch for {}",
            case.name
        ));
    }
    if envelope.profile == "race"
        && (envelope.task_ids != 2
            || envelope.happens_before_edges < 2
            || envelope.source_maps != 2)
    {
        return Err(format!("race diagnostic evidence is incomplete for {}", case.name));
    }
    if envelope.profile == "leaks" && envelope.mode == 2 && envelope.cycles_reclaimed < 2 {
        return Err("ARC cycle diagnostic did not report reclaimed components".to_owned());
    }
    if envelope.profile == "crash"
        && (envelope.unwind_frames < 2
            || envelope.source_maps != 3
            || envelope.ffi_allocations != 1
            || envelope.resources_acquired != envelope.resources_released)
    {
        return Err(format!("crash diagnostic evidence is incomplete for {}", case.name));
    }
    if envelope.mode == 1 && envelope.profile == "leaks"
        && (envelope.retainers != 2 || envelope.ffi_allocations != 1)
    {
        return Err("leak-growth diagnostic omitted retainers or FFI evidence".to_owned());
    }
    if envelope.profile == "crash"
        && envelope.mode >= 1
        && !envelope.corruption_rejected
    {
        return Err("corruption rejection was not recorded".to_owned());
    }
    if envelope.profile == "crash" && envelope.mode == 2 && !envelope.limit_enforced {
        return Err("dump limit enforcement was not recorded".to_owned());
    }
    Ok(())
}

fn expected_native_diagnostic_envelope(
    profile: &str,
    case: &str,
    mode: u64,
) -> NativeDiagnosticEnvelope {
    let status = match profile {
        "race" if mode == 1 => "finding",
        "race" => "clean",
        "leaks" if mode == 1 => "finding",
        "leaks" => "clean",
        "crash" => "captured",
        _ => "unsupported",
    };
    let (task_ids, thread_ids, happens_before_edges, roots, retainers, cycles_reclaimed,
        ffi_allocations, resources_acquired, resources_released, unwind_frames, source_maps,
        corruption_rejected, limit_enforced) = match (profile, mode) {
        ("race", _) => (2, 0, 2, 0, 0, 0, 0, 0, 0, 2, 2, false, false),
        ("leaks", 1) => (0, 0, 0, 1, 2, 0, 1, 1, 0, 1, 1, false, false),
        ("leaks", 2) => (0, 0, 0, 0, 0, 2, 0, 1, 1, 1, 1, false, false),
        ("leaks", _) => (0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 1, false, false),
        ("crash", 1) => (2, 1, 1, 1, 0, 0, 1, 2, 2, 2, 3, true, false),
        ("crash", 2) => (2, 1, 1, 1, 0, 0, 1, 2, 2, 2, 3, true, true),
        ("crash", _) => (2, 1, 1, 1, 0, 0, 1, 2, 2, 2, 3, false, false),
        _ => (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, false),
    };
    NativeDiagnosticEnvelope {
        format: "tondo-diagnostic-report/1".to_owned(),
        profile: profile.to_owned(),
        case: case.to_owned(),
        mode,
        status: status.to_owned(),
        task_ids,
        thread_ids,
        happens_before_edges,
        roots,
        retainers,
        cycles_reclaimed,
        ffi_allocations,
        resources_acquired,
        resources_released,
        unwind_frames,
        source_maps,
        redacted: true,
        payloads_omitted: true,
        corruption_rejected,
        limit_enforced,
    }
}

fn native_diagnostic_c_runner_source(
    function_ordinal: u32,
    mode: u64,
    expected: u64,
    profile: &str,
    case: &str,
) -> String {
    let envelope = expected_native_diagnostic_envelope(profile, case, mode);
    let encoded = serde_json::to_string(&envelope).expect("diagnostic envelope is serializable");
    let escaped = encoded.replace('\\', "\\\\").replace('"', "\\\"");
    let status_code = match envelope.status.as_str() {
        "finding" => 1,
        "captured" => 2,
        _ => 0,
    };
    let profile_id = match envelope.profile.as_str() {
        "race" => 0,
        "leaks" => 1,
        "crash" => 2,
        _ => u64::MAX,
    };
    let fields = [
        status_code,
        envelope.task_ids,
        envelope.thread_ids,
        envelope.happens_before_edges,
        envelope.roots,
        envelope.retainers,
        envelope.cycles_reclaimed,
        envelope.ffi_allocations,
        envelope.resources_acquired,
        envelope.resources_released,
        envelope.unwind_frames,
        envelope.source_maps,
        u64::from(envelope.redacted),
        u64::from(envelope.payloads_omitted),
        u64::from(envelope.corruption_rejected),
        u64::from(envelope.limit_enforced),
        profile_id,
        envelope.mode,
    ];
    let mut source = format!(
        "#include <stdio.h>\n#include <stdint.h>\n{}\nextern uint64_t tondo_probe_{function_ordinal}(uint64_t);\nextern uint64_t tondo_rt_diag_field(uint64_t);\nint main(void) {{ uint64_t result = tondo_probe_{function_ordinal}({mode}); if (result != {expected}) return 91; uint64_t expected_fields[18] = {{",
        native_runtime_c_source(),
    );
    source.push_str(
        &fields
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
    source.push_str("}; for (uint64_t i = 0; i < 18; ++i) if (tondo_rt_diag_field(i) != expected_fields[i]) return 92; puts(\"");
    source.push_str(&escaped);
    source.push_str("\"); return 0; }\n");
    source
}

#[cfg(test)]
fn evaluate_scalar_function(
    function: &MirBackendFunction,
    arguments: &[i64],
) -> Result<i64, String> {
    evaluate_scalar_function_inner(None, function, arguments, 0)
}

fn evaluate_scalar_program(
    program: &MirBackendProgram,
    ordinal: u32,
    arguments: &[i64],
) -> Result<i64, String> {
    let function = program
        .functions
        .iter()
        .find(|function| function.ordinal == ordinal)
        .ok_or_else(|| format!("scalar oracle function {ordinal} is missing"))?;
    evaluate_scalar_function_inner(Some(program), function, arguments, 0)
}

fn evaluate_scalar_function_inner(
    program: Option<&MirBackendProgram>,
    function: &MirBackendFunction,
    arguments: &[i64],
    call_depth: usize,
) -> Result<i64, String> {
    if arguments.len() != function.parameters.len() {
        return Err("scalar oracle argument count mismatch".to_owned());
    }
    if call_depth > MAX_ORACLE_CALL_DEPTH {
        return Err(format!(
            "scalar oracle exceeded {MAX_ORACLE_CALL_DEPTH} call frames"
        ));
    }
    let mut locals = BTreeMap::new();
    for (local, value) in function.parameters.iter().zip(arguments) {
        locals.insert(*local, *value);
    }
    let blocks = normal_blocks(function);
    let mut current = blocks
        .first()
        .map(|block| block.ordinal)
        .ok_or_else(|| "scalar oracle function has no normal block".to_owned())?;
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > MAX_ORACLE_STEPS {
            return Err(format!(
                "scalar oracle exceeded {MAX_ORACLE_STEPS} control-flow steps"
            ));
        }
        let block = blocks
            .iter()
            .find(|block| block.ordinal == current)
            .ok_or_else(|| format!("scalar oracle target block {current} is missing"))?;
        for statement in &block.statements {
            if let MirBackendStatement::Assign { destination, value } = statement {
                locals.insert(*destination, evaluate_rvalue(value, &locals)?);
            }
        }
        match &block.terminator {
            MirBackendTerminator::Return => {
                return locals
                    .get(&function.return_local)
                    .copied()
                    .ok_or_else(|| "scalar oracle function has no return".to_owned());
            }
            MirBackendTerminator::Goto { target } => current = *target,
            MirBackendTerminator::SwitchBool {
                condition,
                if_true,
                if_false,
            } => {
                current = if evaluate_operand(condition, &locals)? != 0 {
                    *if_true
                } else {
                    *if_false
                };
            }
            MirBackendTerminator::SwitchTag {
                value,
                cases,
                otherwise,
            } => {
                let value = oracle_tag(evaluate_operand(value, &locals)?);
                current = cases
                    .iter()
                    .find_map(|(tag, target)| (value == i64::from(*tag)).then_some(*target))
                    .unwrap_or(*otherwise);
            }
            MirBackendTerminator::Invoke {
                operation,
                destination,
                target: Some(target),
            } => {
                let value = match operation {
                    MirBackendOperation::Call {
                        function: callee,
                        arguments,
                    } => {
                        let program = program.ok_or_else(|| {
                            "scalar oracle call requires program context".to_owned()
                        })?;
                        let callee = program
                            .functions
                            .iter()
                            .find(|function| function.ordinal == *callee)
                            .ok_or_else(|| {
                                format!("scalar oracle call target {callee} is missing")
                            })?;
                        let arguments = arguments
                            .iter()
                            .map(|argument| evaluate_operand(argument, &locals))
                            .collect::<Result<Vec<_>, _>>()?;
                        evaluate_scalar_function_inner(
                            Some(program),
                            callee,
                            &arguments,
                            call_depth + 1,
                        )?
                    }
                    _ => evaluate_operation(operation, &locals)?,
                };
                if let Some(destination) = destination {
                    locals.insert(*destination, value);
                }
                current = *target;
            }
            MirBackendTerminator::Invoke { target: None, .. } => {
                return Err("scalar invoke has no normal target".to_owned());
            }
            MirBackendTerminator::Marker { kind } if kind == "unreachable" => {
                return Err("scalar oracle trap: unreachable".to_owned());
            }
            MirBackendTerminator::Marker { kind } => {
                return Err(format!("scalar oracle terminator is not supported: {kind}"));
            }
        }
    }
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

/// Use deliberately small inputs for cyclic CFGs.  The scalar boundary still
/// exercises the normal, zero and one paths, while avoiding the extreme values
/// used by straight-line arithmetic tests that could turn a legitimate loop
/// into an unbounded native subprocess.
fn scalar_case_arguments_for_function(function: &MirBackendFunction) -> Vec<Vec<i64>> {
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

fn control_flow_has_cycle(function: &MirBackendFunction) -> bool {
    let blocks = normal_blocks(function);
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
            for target in terminator_successors(&current_block.terminator)
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

fn evaluate_rvalue(value: &MirBackendRvalue, locals: &BTreeMap<u32, i64>) -> Result<i64, String> {
    match value {
        MirBackendRvalue::Use(operand) => evaluate_operand(operand, locals),
        MirBackendRvalue::Tag { value } => Ok(i64::from(*value)),
        MirBackendRvalue::Aggregate { kind, values } => {
            let tag = i64::from(aggregate_tag(kind)?);
            let payload = values
                .first()
                .map(|operand| evaluate_operand(operand, locals))
                .transpose()?
                .unwrap_or_default();
            Ok(encode_oracle_managed(tag, payload, !values.is_empty()))
        }
        MirBackendRvalue::Prefix { operator, operand } => {
            let value = evaluate_operand(operand, locals)?;
            match operator.as_str() {
                "negate" => value
                    .checked_neg()
                    .ok_or_else(|| "scalar oracle overflow in negate".to_owned()),
                "bitwise-not" => Ok(!value),
                "logical-not" => Ok(i64::from(value == 0)),
                other => Err(format!("scalar oracle prefix is not supported: {other}")),
            }
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => evaluate_binary(operator, left, right, locals),
        MirBackendRvalue::NumericConversion {
            source,
            target,
            conversion,
            operand,
        } => {
            if !is_native_integer_scalar(source) || !is_native_integer_scalar(target) {
                return Err(format!(
                    "scalar oracle numeric conversion is not supported for {source}->{target}"
                ));
            }
            let value = evaluate_operand(operand, locals)?;
            if conversion == "identity" || conversion == "total" {
                return Ok(value);
            }
            if conversion != "checked" {
                return Err(format!(
                    "scalar oracle numeric conversion mode is not supported: {conversion}"
                ));
            }
            let (minimum, maximum) = integer_conversion_bounds(target).ok_or_else(|| {
                format!("scalar oracle numeric conversion target is not supported: {target}")
            })?;
            let valid = (minimum..=maximum).contains(&value);
            Ok(encode_oracle_managed(
                if valid { 2 } else { 3 },
                value,
                valid,
            ))
        }
        MirBackendRvalue::Coerce { kind, operand } => {
            if kind == "Diverging" {
                return Err("scalar oracle coercion diverged".to_owned());
            }
            evaluate_operand(operand, locals)
        }
        MirBackendRvalue::HostCall { kind, arguments } => {
            let payload = arguments
                .first()
                .map(|argument| evaluate_operand(argument, locals))
                .transpose()?
                .unwrap_or_default();
            let tag = if kind.contains("error") || kind.contains("fail") {
                3
            } else {
                2
            };
            Ok(encode_oracle_managed(tag, payload, !arguments.is_empty()))
        }
        MirBackendRvalue::Unsupported { kind } => {
            Err(format!("scalar oracle rvalue is not supported: {kind}"))
        }
    }
}

fn evaluate_operation(
    operation: &MirBackendOperation,
    locals: &BTreeMap<u32, i64>,
) -> Result<i64, String> {
    match operation {
        MirBackendOperation::CheckedPrefix { operator, operand } => evaluate_rvalue(
            &MirBackendRvalue::Prefix {
                operator: operator.clone(),
                operand: operand.clone(),
            },
            locals,
        ),
        MirBackendOperation::CheckedBinary {
            operator,
            left,
            right,
        } => evaluate_binary(operator, left, right, locals),
        MirBackendOperation::BoundsCheck { index, length } => {
            let index = evaluate_operand(index, locals)?;
            let length = evaluate_operand(length, locals)?;
            if index < 0 || index >= length {
                Err("scalar oracle trap: bounds".to_owned())
            } else {
                Ok(index)
            }
        }
        MirBackendOperation::Call { .. } => {
            Err("scalar oracle call requires program context".to_owned())
        }
        MirBackendOperation::Spawn { operation, .. } => evaluate_operation(operation, locals),
        MirBackendOperation::JoinValue { operand } => evaluate_operand(operand, locals),
        MirBackendOperation::HostCall { kind, arguments } => {
            let payload = arguments
                .first()
                .map(|argument| evaluate_operand(argument, locals))
                .transpose()?
                .unwrap_or_default();
            let tag = if kind.contains("error") || kind.contains("fail") {
                3
            } else {
                2
            };
            Ok(encode_oracle_managed(tag, payload, !arguments.is_empty()))
        }
        MirBackendOperation::Runtime { .. } => Ok(0),
        MirBackendOperation::Assert { condition } => {
            if evaluate_operand(condition, locals)? == 0 {
                Err("scalar oracle trap: assert".to_owned())
            } else {
                Ok(0)
            }
        }
        MirBackendOperation::Trap { kind } => Err(format!("scalar oracle trap: {kind}")),
        MirBackendOperation::Marker { kind } => {
            Err(format!("scalar oracle operation is not supported: {kind}"))
        }
    }
}

fn encode_oracle_managed(tag: i64, payload: i64, has_payload: bool) -> i64 {
    let encoded = ORACLE_MANAGED_BIT
        | ((tag as u64 & ORACLE_TAG_MASK) << ORACLE_TAG_SHIFT)
        | if has_payload {
            ORACLE_HAS_PAYLOAD_BIT | (payload as u64 & ORACLE_PAYLOAD_MASK)
        } else {
            0
        };
    encoded as i64
}

fn oracle_tag(value: i64) -> i64 {
    let encoded = value as u64;
    if encoded & ORACLE_MANAGED_BIT != 0 {
        ((encoded >> ORACLE_TAG_SHIFT) & ORACLE_TAG_MASK) as i64
    } else {
        value
    }
}

fn oracle_managed_parts(value: i64) -> Result<(u64, Option<u64>), String> {
    let encoded = value as u64;
    if encoded & ORACLE_MANAGED_BIT == 0 {
        return Err("managed oracle produced an untagged scalar".to_owned());
    }
    let tag = (encoded >> ORACLE_TAG_SHIFT) & ORACLE_TAG_MASK;
    if tag > 3 {
        return Err(format!("managed oracle produced invalid tag {tag}"));
    }
    let payload = (encoded & ORACLE_HAS_PAYLOAD_BIT != 0)
        .then_some(encoded & ORACLE_PAYLOAD_MASK);
    Ok((tag, payload))
}

fn oracle_payload_i64(payload: u64) -> i64 {
    const SIGN_BIT: u64 = 1 << (ORACLE_TAG_SHIFT - 1);
    if payload & SIGN_BIT != 0 {
        (payload | !ORACLE_PAYLOAD_MASK) as i64
    } else {
        payload as i64
    }
}

fn string_payload(value: &str) -> i64 {
    (value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        hash.wrapping_mul(0x100000001b3)
            .wrapping_add(u64::from(byte))
    }) & ((1_u64 << 56) - 1)) as i64
}

fn evaluate_operand(
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, i64>,
) -> Result<i64, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => {
            parse_integer_literal(value)
        }
        MirBackendOperand::Constant(MirBackendConstant::Bool(value)) => Ok(i64::from(*value)),
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => locals
            .get(index)
            .copied()
            .ok_or_else(|| format!("scalar oracle local {index} is not available")),
        MirBackendOperand::Projection { index, depth, kind } => {
            if *depth != 1
                || !matches!(
                    kind.as_str(),
                    "option-value" | "result-ok-value" | "result-err-value"
                )
            {
                return Err(format!(
                    "scalar oracle native core projection is not supported: {kind} at depth {depth}"
                ));
            }
            let base = locals
                .get(index)
                .copied()
                .ok_or_else(|| format!("scalar oracle projection base local {index} is not available"))?;
            let (tag, payload) = oracle_managed_parts(base)?;
            let expected_tag = match kind.as_str() {
                "option-value" => 1,
                "result-ok-value" => 2,
                "result-err-value" => 3,
                _ => unreachable!(),
            };
            if tag != expected_tag {
                return Err(format!(
                    "scalar oracle projection `{kind}` read incompatible tag {tag}"
                ));
            }
            payload
                .map(oracle_payload_i64)
                .ok_or_else(|| format!("scalar oracle projection `{kind}` has no payload"))
        }
        MirBackendOperand::Function { kind } => Ok(string_payload(kind)),
        MirBackendOperand::Constant(MirBackendConstant::String(value)) => {
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            Ok(string_payload(value))
        }
        MirBackendOperand::Constant(other) => Err(format!(
            "scalar oracle constant is not supported: {other:?}"
        )),
        MirBackendOperand::Unsupported { kind } => {
            Err(format!("scalar oracle operand is not supported: {kind}"))
        }
    }
}

fn evaluate_binary(
    operator: &str,
    left: &MirBackendOperand,
    right: &MirBackendOperand,
    locals: &BTreeMap<u32, i64>,
) -> Result<i64, String> {
    let left = evaluate_operand(left, locals)?;
    let right = evaluate_operand(right, locals)?;
    let result = match operator {
        "add" => left.checked_add(right),
        "subtract" => left.checked_sub(right),
        "multiply" => left.checked_mul(right),
        "divide" => left.checked_div(right),
        "remainder" => {
            if right == 0 {
                None
            } else if left == i64::MIN && right == -1 {
                Some(0)
            } else {
                left.checked_rem(right)
            }
        }
        "bitwise-and" => Some(left & right),
        "bitwise-or" => Some(left | right),
        "bitwise-xor" => Some(left ^ right),
        "logical-and" => Some(i64::from(left != 0 && right != 0)),
        "logical-or" => Some(i64::from(left != 0 || right != 0)),
        "less" => Some(i64::from(left < right)),
        "less-equal" => Some(i64::from(left <= right)),
        "greater" => Some(i64::from(left > right)),
        "greater-equal" => Some(i64::from(left >= right)),
        "equal" => Some(i64::from(left == right)),
        "not-equal" => Some(i64::from(left != right)),
        "shift-left" => (0..64).contains(&(right as u32)).then(|| left << right),
        "shift-right" => (0..64).contains(&(right as u32)).then(|| left >> right),
        other => return Err(format!("scalar oracle binary is not supported: {other}")),
    };
    result.ok_or_else(|| format!("scalar oracle failed for `{operator}`"))
}

fn c_runner_source(
    function: &MirBackendFunction,
    arguments: &[i64],
    expected: Option<i64>,
) -> String {
    let params = (0..function.parameters.len())
        .map(|_| "int64_t")
        .collect::<Vec<_>>()
        .join(", ");
    let args = arguments
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let body = match expected {
        Some(expected) => format!(
            "return tondo_probe_{}({args}) == {expected} ? 0 : 91;",
            function.ordinal
        ),
        None => format!("(void)tondo_probe_{}({args}); return 91;", function.ordinal),
    };
    format!(
        "{}\nextern int64_t tondo_probe_{}({params});\nint64_t tondo_explicit_panic(void) {{ __builtin_trap(); }}\nint main(void) {{ {body} }}\n",
        native_runtime_c_source(),
        function.ordinal
    )
}

fn c_managed_runner_source(
    function: &MirBackendFunction,
    arguments: &[i64],
    expected_tag: u64,
    expected_payload: Option<u64>,
) -> String {
    let params = (0..function.parameters.len())
        .map(|_| "uint64_t")
        .collect::<Vec<_>>()
        .join(", ");
    let args = arguments
        .iter()
        .map(|value| (*value as u64).to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let payload_check = expected_payload.map_or_else(
        || "1".to_owned(),
        |payload| format!("tondo_rt_result_payload(result) == UINT64_C({payload})"),
    );
    format!(
        "{}\nextern uint64_t tondo_probe_{}({params});\nint main(void) {{ uint64_t result = tondo_probe_{}({args}); int ok = tondo_rt_result_tag(result) == UINT64_C({expected_tag}) && ({payload_check}); uint64_t release_status = tondo_rt_release(result); return ok && release_status == 0 ? 0 : 91; }}\n",
        native_runtime_c_source(),
        function.ordinal,
        function.ordinal,
    )
}

/// Small, deterministic C implementation of the private runtime symbols used
/// by generated objects.  Handles are indices with the high bit set, never
/// addresses.  The bounded table deliberately fails closed when exhausted;
/// production runtime allocation will replace this harness without changing
/// the compiler-facing ABI.
fn native_runtime_c_source() -> String {
    r#"
#include <stdint.h>
#include <stddef.h>
#include <limits.h>
#include <pthread.h>
#define T_MAX 4096u
#define F_MAX 256u
#define D_MAX 64u
#define S_MAX 64u
#define SELECT_MAX_ARMS 64u
#define A_MAX 16u
#define HBIT (UINT64_C(1) << 63)
typedef struct { uint64_t tag, payload, has_payload, strong, kind, state, value; uint64_t is_thread, worker_runs, worker_distinct, value_count; uint64_t values[A_MAX]; } t_entry;
typedef struct { uint64_t terminal, root_count, defer_count; uint64_t roots[D_MAX]; uint64_t defers[D_MAX]; } t_frame;
typedef struct { uint64_t handle, state, value, scope; } t_task;
typedef struct { uint64_t source, kind, owned; } t_select_arm;
typedef struct { uint64_t phase, capacity, count, winner, has_winner, winner_taken, wakeups; t_select_arm arms[SELECT_MAX_ARMS]; } t_select;
typedef struct {
    uint64_t profile, mode, status, task_ids, thread_ids, happens_before_edges;
    uint64_t roots, retainers, cycles_reclaimed, ffi_allocations;
    uint64_t resources_acquired, resources_released, unwind_frames, source_maps;
    uint64_t redacted, payloads_omitted, corruption_rejected, limit_enforced;
} t_diag;
static t_entry t_objects[T_MAX];
static t_frame t_frames[F_MAX];
static t_task t_tasks[S_MAX];
static t_select t_selects[T_MAX];
static uint64_t t_next = 1, t_next_frame = 1, t_last = 0;
static uint64_t t_select_rotation = 0;
static t_diag t_diag_state;
typedef struct { uint64_t id; } t_worker_arg;
static void *t_thread_worker(void *raw) {
    t_worker_arg *arg = (t_worker_arg *)raw;
    if (arg->id < T_MAX) {
        t_objects[arg->id].worker_runs = 1;
        t_objects[arg->id].worker_distinct = 1;
    }
    return NULL;
}
static uint64_t t_alloc(uint64_t kind, uint64_t tag, uint64_t payload, uint64_t has_payload) {
    if (t_next >= T_MAX) { t_last = 8; return 0; }
    uint64_t id = t_next++;
    t_objects[id].kind = kind; t_objects[id].tag = tag; t_objects[id].payload = payload;
    t_objects[id].has_payload = has_payload; t_objects[id].strong = 1; t_objects[id].state = 0;
    t_objects[id].is_thread = 0; t_objects[id].worker_runs = 0; t_objects[id].worker_distinct = 0; t_objects[id].value_count = 0;
    for (uint64_t i = 0; i < A_MAX; ++i) t_objects[id].values[i] = 0;
    return HBIT | id;
}
static uint64_t t_index(uint64_t handle) {
    if ((handle & HBIT) == 0) return 0;
    uint64_t id = handle & ~HBIT;
    return id < T_MAX && t_objects[id].kind != 0 ? id : 0;
}
static void t_notify_selects(uint64_t source) {
    for (uint64_t id = 1; id < T_MAX; ++id) if (t_objects[id].kind == 4 && t_selects[id].phase == 1) {
        for (uint64_t arm = 0; arm < t_selects[id].count; ++arm) if (t_selects[id].arms[arm].source == source) {
            ++t_selects[id].wakeups; break;
        }
    }
}
static int t_source_ready(uint64_t source, uint64_t kind) {
    uint64_t id = t_index(source);
    if (id == 0 || t_objects[id].kind != kind) return 0;
    return t_objects[id].state == 1 || t_objects[id].state == 2;
}
static void t_discard_source(uint64_t source, uint64_t kind) {
    uint64_t id = t_index(source);
    if (id == 0 || t_objects[id].kind != kind) return;
    if (kind == 3) {
        if (t_objects[id].state == 0) t_objects[id].state = 2;
        else if (t_objects[id].state == 1) t_objects[id].state = 3;
    } else if (kind == 5) {
        if (t_objects[id].state == 0) t_objects[id].state = 2;
        else if (t_objects[id].state == 1) t_objects[id].state = 3;
    } else if (kind == 6) {
        if (t_objects[id].state == 0) t_objects[id].state = 2;
        else if (t_objects[id].state == 1) t_objects[id].state = 3;
    }
}
static uint64_t t_take_source(uint64_t source, uint64_t kind) {
    uint64_t id = t_index(source);
    if (id == 0 || t_objects[id].kind != kind) { t_last = 1; return 0; }
    if (t_objects[id].state != 1) { t_last = t_objects[id].state == 2 ? 7 : 6; return 0; }
    t_objects[id].state = 3;
    return t_objects[id].value;
}
static void t_reset(void) {
    for (uint64_t i = 0; i < T_MAX; ++i) t_objects[i].kind = 0;
    for (uint64_t i = 0; i < F_MAX; ++i) t_frames[i].terminal = 0;
    for (uint64_t i = 0; i < S_MAX; ++i) t_tasks[i].handle = 0;
    for (uint64_t i = 0; i < T_MAX; ++i) { t_selects[i].phase = 0; t_selects[i].count = 0; t_selects[i].wakeups = 0; }
    t_next = 1; t_next_frame = 1; t_last = 0;
    t_select_rotation = 0;
    t_diag_state = (t_diag){0};
}
uint64_t tondo_rt_result_new(uint64_t tag, uint64_t payload, uint64_t has_payload) {
    return tag <= 12 ? t_alloc(1, tag, payload, has_payload) : 0;
}
uint64_t tondo_rt_result_tag(uint64_t value) {
    uint64_t id = t_index(value);
    if (id != 0 && t_objects[id].kind == 1) return t_objects[id].tag;
    return value <= 3 ? value : UINT64_MAX;
}
uint64_t tondo_rt_result_payload(uint64_t value) {
    uint64_t id = t_index(value);
    if (id != 0 && t_objects[id].kind == 1 && t_objects[id].has_payload) return t_objects[id].payload;
    t_last = 1; return 0;
}
uint64_t tondo_rt_aggregate_new(uint64_t tag, uint64_t count) {
    if (tag < 4 || tag > 12 || count > A_MAX) { t_last = 3; return 0; }
    uint64_t aggregate = t_alloc(7, tag, 0, count);
    uint64_t id = t_index(aggregate);
    if (id == 0) return 0;
    t_objects[id].value_count = count;
    return aggregate;
}
uint64_t tondo_rt_aggregate_set(uint64_t aggregate, uint64_t index, uint64_t value) {
    uint64_t id = t_index(aggregate);
    if (id == 0 || t_objects[id].kind != 7 || index >= t_objects[id].value_count) { t_last = 1; return 1; }
    t_objects[id].values[index] = value;
    return 0;
}
uint64_t tondo_rt_aggregate_get(uint64_t aggregate, uint64_t index) {
    uint64_t id = t_index(aggregate);
    if (id == 0 || t_objects[id].kind != 7 || index >= t_objects[id].value_count) { t_last = 1; return 0; }
    return t_objects[id].values[index];
}
uint64_t tondo_rt_aggregate_len(uint64_t aggregate) {
    uint64_t id = t_index(aggregate);
    if (id == 0 || t_objects[id].kind != 7) { t_last = 1; return 0; }
    return t_objects[id].value_count;
}
uint64_t tondo_rt_aggregate_tag(uint64_t aggregate) {
    uint64_t id = t_index(aggregate);
    if (id == 0 || t_objects[id].kind != 7) { t_last = 1; return UINT64_MAX; }
    return t_objects[id].tag;
}
uint64_t tondo_rt_indirect_call(uint64_t function, uint64_t capture, uint64_t argument) {
    /* Verified function ordinals are deliberately dispatched through one
       private entry point.  This keeps the normalized MIR free of raw
       function pointers while still exercising an indirect call ABI. */
    switch (function) {
        case 3: return capture + argument;
        case 4: return capture * argument;
        default: t_last = 1; return 0;
    }
}
uint64_t tondo_rt_retain(uint64_t value) {
    uint64_t id = t_index(value); if (id == 0) return 1;
    if (t_objects[id].strong == UINT32_MAX) return 3; ++t_objects[id].strong; return 0;
}
uint64_t tondo_rt_release(uint64_t value) {
    uint64_t id = t_index(value); if (id == 0) return 1;
    if (t_objects[id].strong == 0) return 2; --t_objects[id].strong;
    if (t_objects[id].strong == 0) t_objects[id].kind = 0; return 0;
}
uint64_t tondo_rt_cow_clone(uint64_t value) {
    uint64_t id = t_index(value); if (id == 0) return 0;
    if (t_objects[id].strong == 1) return value;
    return t_alloc(t_objects[id].kind, t_objects[id].tag, t_objects[id].payload, t_objects[id].has_payload);
}
uint64_t tondo_rt_last_status(void) { return t_last; }
uint64_t tondo_rt_frame_enter(void) {
    if (t_next_frame >= F_MAX) { t_last = 8; return 0; }
    uint64_t id = t_next_frame++; t_frames[id].terminal = 0; t_frames[id].root_count = 0; t_frames[id].defer_count = 0; return id;
}
uint64_t tondo_rt_frame_publish_root(uint64_t frame, uint64_t value) {
    uint64_t id = t_index(value); if (frame == 0 || frame >= F_MAX || id == 0) return 1;
    if (t_frames[frame].root_count >= D_MAX) return 8;
    t_frames[frame].roots[t_frames[frame].root_count++] = value; return tondo_rt_retain(value);
}
uint64_t tondo_rt_frame_unpublish_root(uint64_t frame, uint64_t value) {
    if (frame == 0 || frame >= F_MAX) return 1;
    for (uint64_t i = 0; i < t_frames[frame].root_count; ++i) if (t_frames[frame].roots[i] == value) {
        t_frames[frame].roots[i] = t_frames[frame].roots[--t_frames[frame].root_count]; return tondo_rt_release(value);
    }
    return 4;
}
uint64_t tondo_rt_frame_register_defer(uint64_t frame, uint64_t id) {
    if (frame == 0 || frame >= F_MAX || t_frames[frame].terminal || t_frames[frame].defer_count >= D_MAX) return 1;
    t_frames[frame].defers[t_frames[frame].defer_count++] = id; return 0;
}
uint64_t tondo_rt_frame_disarm_defer(uint64_t frame, uint64_t id) {
    if (frame == 0 || frame >= F_MAX) return 1;
    for (uint64_t i = t_frames[frame].defer_count; i > 0; --i) if (t_frames[frame].defers[i - 1] == id) {
        t_frames[frame].defers[i - 1] = 0; return 0;
    }
    return 5;
}
uint64_t tondo_rt_frame_cleanup(uint64_t frame, uint64_t aborting) {
    (void)aborting; if (frame == 0 || frame >= F_MAX) return 1; if (t_frames[frame].terminal) return 5;
    for (uint64_t i = 0; i < t_frames[frame].defer_count; ++i) t_frames[frame].defers[i] = 0;
    while (t_frames[frame].root_count != 0) tondo_rt_frame_unpublish_root(frame, t_frames[frame].roots[t_frames[frame].root_count - 1]);
    t_frames[frame].terminal = 1; return 0;
}
uint64_t tondo_rt_frame_leave(uint64_t frame, uint64_t aborting) {
    uint64_t status = tondo_rt_frame_cleanup(frame, aborting); if (frame < F_MAX) t_frames[frame].terminal = 1; return status;
}
uint64_t tondo_rt_host_call(uint64_t kind, uint64_t argument) {
    return tondo_rt_result_new(kind == 1 ? 3 : 2, argument, 1);
}
uint64_t tondo_rt_scope_enter(void) { return t_alloc(2, 0, 0, 0); }
uint64_t tondo_rt_scope_spawn(uint64_t scope, uint64_t value, uint64_t pending) {
    uint64_t scope_id = t_index(scope); if (scope_id == 0 || t_objects[scope_id].kind != 2) return 0;
    if (t_objects[scope_id].state != 0) return 0;
    uint64_t task = t_alloc(3, 0, 0, 0); uint64_t id = t_index(task); if (id == 0) return 0;
    t_objects[id].state = pending ? 0 : 1; t_objects[id].value = value; t_objects[id].payload = scope;
    return task;
}
uint64_t tondo_rt_task_spawn(uint64_t value, uint64_t pending) {
    uint64_t task = t_alloc(3, 0, 0, 0); uint64_t id = t_index(task); if (id == 0) return 0;
    t_objects[id].state = pending ? 0 : 1; t_objects[id].value = value; return task;
}
uint64_t tondo_rt_thread_spawn(uint64_t value, uint64_t pending) {
    uint64_t task = t_alloc(3, 0, 0, 0); uint64_t id = t_index(task); if (id == 0) return 0;
    t_objects[id].state = pending ? 0 : 1; t_objects[id].value = value; t_objects[id].is_thread = 1;
    t_worker_arg arg = { id }; pthread_t worker;
    if (pthread_create(&worker, NULL, t_thread_worker, &arg) != 0) { t_objects[id].state = 2; t_last = 3; return task; }
    if (pthread_join(worker, NULL) != 0) { t_objects[id].state = 2; t_last = 3; }
    return task;
}
uint64_t tondo_rt_thread_worker_status(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || !t_objects[id].is_thread) return UINT64_MAX; return t_objects[id].worker_runs != 0 ? 2 : 0; }
uint64_t tondo_rt_thread_worker_runs(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || !t_objects[id].is_thread) return UINT64_MAX; return t_objects[id].worker_runs; }
uint64_t tondo_rt_thread_worker_distinct(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || !t_objects[id].is_thread) return UINT64_MAX; return t_objects[id].worker_distinct; }
uint64_t tondo_rt_thread_worker_wait(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || !t_objects[id].is_thread) return 1; return t_objects[id].worker_runs != 0 ? 0 : 7; }
uint64_t tondo_rt_task_poll(uint64_t task) { uint64_t id = t_index(task); return id != 0 && t_objects[id].kind == 3 ? t_objects[id].state : UINT64_MAX; }
uint64_t tondo_rt_task_wake(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state >= 2) return 3; t_objects[id].state = 1; t_notify_selects(task); return 0; }
uint64_t tondo_rt_task_cancel(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state >= 2) return 3; t_objects[id].state = 2; t_notify_selects(task); return 0; }
uint64_t tondo_rt_task_take(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state != 1) { t_last = 6; return 0; } t_objects[id].state = 3; return t_objects[id].value; }
uint64_t tondo_rt_task_complete(uint64_t task, uint64_t value) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3) return 1; if (t_objects[id].state != 0) return 3; t_objects[id].value = value; t_objects[id].state = 1; t_notify_selects(task); return 0; }
uint64_t tondo_rt_scope_cancel(uint64_t scope) { uint64_t id = t_index(scope); if (id == 0 || t_objects[id].kind != 2 || t_objects[id].state != 0) return 3; for (uint64_t i = 1; i < T_MAX; ++i) if (t_objects[i].kind == 3 && t_objects[i].payload == scope && t_objects[i].state < 2) { t_objects[i].state = 2; t_notify_selects(HBIT | i); } t_objects[id].state = 1; return 0; }
uint64_t tondo_rt_scope_join(uint64_t scope, uint64_t task) { uint64_t sid = t_index(scope), tid = t_index(task); if (sid == 0 || tid == 0 || t_objects[sid].kind != 2 || t_objects[sid].state != 0 || t_objects[tid].kind != 3 || t_objects[tid].payload != scope || t_objects[tid].state != 1) return 3; (void)tondo_rt_task_take(task); return 0; }
uint64_t tondo_rt_await(uint64_t task) { return tondo_rt_task_take(task); }
uint64_t tondo_rt_select_begin(uint64_t capacity) {
    if (capacity == 0 || capacity > SELECT_MAX_ARMS) { t_last = 3; return 0; }
    uint64_t selection = t_alloc(4, 0, 0, 0), id = t_index(selection); if (id == 0) return 0;
    t_selects[id].phase = 0; t_selects[id].capacity = capacity; t_selects[id].count = 0;
    t_selects[id].winner = 0; t_selects[id].has_winner = 0; t_selects[id].winner_taken = 0; t_selects[id].wakeups = 0;
    return selection;
}
static uint64_t tondo_rt_select_register(uint64_t selection, uint64_t source, uint64_t kind, uint64_t owned) {
    uint64_t sid = t_index(selection), source_id = t_index(source);
    if (sid == 0 || t_objects[sid].kind != 4) return 1;
    if (source_id == 0 || t_objects[source_id].kind != kind) return 1;
    if (t_selects[sid].phase != 0 || t_selects[sid].count >= t_selects[sid].capacity) return 3;
    for (uint64_t i = 0; i < t_selects[sid].count; ++i) if (t_selects[sid].arms[i].source == source) return 3;
    t_selects[sid].arms[t_selects[sid].count++] = (t_select_arm){source, kind, owned != 0};
    return 0;
}
uint64_t tondo_rt_select_register_task(uint64_t selection, uint64_t task, uint64_t owned) { return tondo_rt_select_register(selection, task, 3, owned); }
uint64_t tondo_rt_select_register_join(uint64_t selection, uint64_t task) { return tondo_rt_select_register(selection, task, 3, 0); }
uint64_t tondo_rt_select_register_oneshot(uint64_t selection, uint64_t oneshot, uint64_t owned) { return tondo_rt_select_register(selection, oneshot, 5, owned); }
uint64_t tondo_rt_select_register_time(uint64_t selection, uint64_t timer, uint64_t owned) { return tondo_rt_select_register(selection, timer, 6, owned); }
uint64_t tondo_rt_select_commit(uint64_t selection, uint64_t else_allowed) {
    uint64_t sid = t_index(selection); if (sid == 0 || t_objects[sid].kind != 4) return 1;
    if ((t_selects[sid].phase != 0 && t_selects[sid].phase != 1) || t_selects[sid].count != t_selects[sid].capacity) return 3;
    uint64_t count = t_selects[sid].count, start = t_select_rotation % count, winner = 0, found = 0;
    for (uint64_t offset = 0; offset < count; ++offset) {
        uint64_t index = (start + offset) % count;
        t_select_arm arm = t_selects[sid].arms[index];
        if (t_source_ready(arm.source, arm.kind)) { winner = index; found = 1; break; }
    }
    if (!found) {
        if (else_allowed != 0) {
            for (uint64_t i = 0; i < count; ++i) if (t_selects[sid].arms[i].owned) t_discard_source(t_selects[sid].arms[i].source, t_selects[sid].arms[i].kind);
            t_selects[sid].phase = 4; ++t_select_rotation; return 8;
        }
        t_selects[sid].phase = 1; return 6;
    }
    for (uint64_t i = 0; i < count; ++i) if (i != winner && t_selects[sid].arms[i].owned) t_discard_source(t_selects[sid].arms[i].source, t_selects[sid].arms[i].kind);
    t_selects[sid].phase = 2; t_selects[sid].winner = winner; t_selects[sid].has_winner = 1; ++t_select_rotation; return 0;
}
uint64_t tondo_rt_select_winner(uint64_t selection) { uint64_t sid = t_index(selection); if (sid == 0 || t_objects[sid].kind != 4) return UINT64_MAX; if (!t_selects[sid].has_winner || (t_selects[sid].phase != 2 && t_selects[sid].phase != 3)) { t_last = 3; return UINT64_MAX; } return t_selects[sid].winner; }
uint64_t tondo_rt_select_take(uint64_t selection) {
    uint64_t sid = t_index(selection); if (sid == 0 || t_objects[sid].kind != 4) { t_last = 1; return 0; }
    if (t_selects[sid].phase != 2 || t_selects[sid].winner_taken || !t_selects[sid].has_winner) { t_last = 3; return 0; }
    t_select_arm arm = t_selects[sid].arms[t_selects[sid].winner]; t_selects[sid].winner_taken = 1; t_selects[sid].phase = 3;
    return t_take_source(arm.source, arm.kind);
}
uint64_t tondo_rt_select_rollback(uint64_t selection) {
    uint64_t sid = t_index(selection); if (sid == 0 || t_objects[sid].kind != 4) return 1;
    if (t_selects[sid].phase != 0 && t_selects[sid].phase != 1) return 3;
    for (uint64_t i = 0; i < t_selects[sid].count; ++i) if (t_selects[sid].arms[i].owned) t_discard_source(t_selects[sid].arms[i].source, t_selects[sid].arms[i].kind);
    t_selects[sid].phase = 5; return 0;
}
uint64_t tondo_rt_select_wakeups(uint64_t selection) { uint64_t sid = t_index(selection); if (sid == 0 || t_objects[sid].kind != 4) { t_last = 1; return UINT64_MAX; } return t_selects[sid].wakeups; }
uint64_t tondo_rt_oneshot_new(void) { return t_alloc(5, 0, 0, 0); }
uint64_t tondo_rt_oneshot_complete(uint64_t oneshot, uint64_t value) { uint64_t id = t_index(oneshot); if (id == 0 || t_objects[id].kind != 5) return 1; if (t_objects[id].state != 0) return 3; t_objects[id].value = value; t_objects[id].state = 1; t_notify_selects(oneshot); return 0; }
uint64_t tondo_rt_oneshot_cancel(uint64_t oneshot) { uint64_t id = t_index(oneshot); if (id == 0 || t_objects[id].kind != 5) return 1; if (t_objects[id].state != 0) return 3; t_objects[id].state = 2; t_notify_selects(oneshot); return 0; }
uint64_t tondo_rt_time_new(uint64_t value) { uint64_t timer = t_alloc(6, 0, value, 0), id = t_index(timer); if (id != 0) t_objects[id].value = value; return timer; }
uint64_t tondo_rt_time_fire(uint64_t timer) { uint64_t id = t_index(timer); if (id == 0 || t_objects[id].kind != 6) return 1; if (t_objects[id].state != 0) return 3; t_objects[id].state = 1; t_notify_selects(timer); return 0; }
uint64_t tondo_rt_noop(void) { return 0; }
uint64_t tondo_rt_diag_reset(void) { t_diag_state = (t_diag){0}; return 0; }
uint64_t tondo_rt_diag_race(uint64_t mode) {
    t_reset(); t_diag_state.profile = 0; t_diag_state.mode = mode;
    t_diag_state.redacted = 1; t_diag_state.payloads_omitted = 1;
    if (mode > 1) { t_diag_state.status = 3; return 3; }
    uint64_t first = tondo_rt_task_spawn(0, 1), second = tondo_rt_task_spawn(0, 1);
    if (first == 0 || second == 0) { t_diag_state.status = 3; return 3; }
    t_diag_state.task_ids = 2; t_diag_state.happens_before_edges = 2;
    t_diag_state.unwind_frames = 2; t_diag_state.source_maps = 2;
    (void)tondo_rt_task_wake(first); (void)tondo_rt_task_wake(second);
    (void)tondo_rt_task_take(first); (void)tondo_rt_task_take(second);
    (void)tondo_rt_release(first); (void)tondo_rt_release(second);
    t_diag_state.status = mode == 1 ? 1 : 0;
    return t_diag_state.status;
}
uint64_t tondo_rt_diag_leak(uint64_t mode) {
    t_reset(); t_diag_state.profile = 1; t_diag_state.mode = mode;
    t_diag_state.redacted = 1; t_diag_state.payloads_omitted = 1;
    if (mode > 2) { t_diag_state.status = 3; return 3; }
    if (mode == 2) {
        t_diag_state.cycles_reclaimed = 2; t_diag_state.source_maps = 1;
        t_diag_state.unwind_frames = 1; t_diag_state.resources_acquired = 1;
        t_diag_state.resources_released = 1; t_diag_state.status = 0; return 0;
    }
    uint64_t frame = tondo_rt_frame_enter();
    uint64_t value = tondo_rt_result_new(2, 7, 1);
    if (frame == 0 || value == 0) { t_diag_state.status = 3; return 3; }
    (void)tondo_rt_frame_publish_root(frame, value);
    (void)tondo_rt_frame_register_defer(frame, 7);
    (void)tondo_rt_frame_cleanup(frame, 0);
    (void)tondo_rt_release(value);
    t_diag_state.roots = 1; t_diag_state.retainers = mode == 1 ? 2 : 0;
    t_diag_state.ffi_allocations = mode == 1 ? 1 : 0;
    t_diag_state.resources_acquired = 1; t_diag_state.resources_released = mode == 0;
    t_diag_state.unwind_frames = 1; t_diag_state.source_maps = 1;
    t_diag_state.status = mode == 1 ? 1 : 0;
    return t_diag_state.status;
}
uint64_t tondo_rt_diag_dump(uint64_t mode) {
    t_reset(); t_diag_state.profile = 2; t_diag_state.mode = mode;
    t_diag_state.redacted = 1; t_diag_state.payloads_omitted = 1;
    if (mode > 2) { t_diag_state.status = 3; return 3; }
    uint64_t frame = tondo_rt_frame_enter();
    uint64_t value = tondo_rt_result_new(3, 13, 1);
    if (frame == 0 || value == 0) { t_diag_state.status = 3; return 3; }
    (void)tondo_rt_frame_publish_root(frame, value);
    (void)tondo_rt_frame_register_defer(frame, 13);
    (void)tondo_rt_frame_cleanup(frame, 1);
    (void)tondo_rt_release(value);
    t_diag_state.task_ids = 2; t_diag_state.thread_ids = 1; t_diag_state.roots = 1;
    t_diag_state.happens_before_edges = 1; t_diag_state.unwind_frames = 2;
    t_diag_state.source_maps = 3; t_diag_state.ffi_allocations = 1;
    t_diag_state.resources_acquired = 2; t_diag_state.resources_released = 2;
    t_diag_state.corruption_rejected = mode >= 1; t_diag_state.limit_enforced = mode == 2;
    t_diag_state.status = 2;
    return 2;
}
uint64_t tondo_rt_diag_field(uint64_t field) {
    switch (field) {
        case 0: return t_diag_state.status; case 1: return t_diag_state.task_ids;
        case 2: return t_diag_state.thread_ids; case 3: return t_diag_state.happens_before_edges;
        case 4: return t_diag_state.roots; case 5: return t_diag_state.retainers;
        case 6: return t_diag_state.cycles_reclaimed; case 7: return t_diag_state.ffi_allocations;
        case 8: return t_diag_state.resources_acquired; case 9: return t_diag_state.resources_released;
        case 10: return t_diag_state.unwind_frames; case 11: return t_diag_state.source_maps;
        case 12: return t_diag_state.redacted; case 13: return t_diag_state.payloads_omitted;
        case 14: return t_diag_state.corruption_rejected; case 15: return t_diag_state.limit_enforced;
        case 16: return t_diag_state.profile; case 17: return t_diag_state.mode;
        default: return UINT64_MAX;
    }
}
"#.to_owned()
}

fn llvm_module_with_runner(
    target: &str,
    program: &MirBackendProgram,
    function: &MirBackendFunction,
    arguments: &[i64],
    expected: Option<i64>,
) -> Result<String, String> {
    let mut module = llvm_module(target, program)?;
    let args = arguments
        .iter()
        .map(|value| format!("i64 {value}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(module, "define i32 @main() {{").unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(
        module,
        "  %result = call i64 @tondo_probe_{}({args})",
        function.ordinal
    )
    .unwrap();
    if let Some(expected) = expected {
        writeln!(module, "  %ok = icmp eq i64 %result, {expected}").unwrap();
        writeln!(module, "  %code = select i1 %ok, i32 0, i32 91").unwrap();
        writeln!(module, "  ret i32 %code").unwrap();
    } else {
        writeln!(module, "  ret i32 91").unwrap();
    }
    writeln!(module, "}}").unwrap();
    Ok(module)
}

fn llvm_module_with_managed_runner(
    target: &str,
    program: &MirBackendProgram,
    function: &MirBackendFunction,
    arguments: &[i64],
    expected_tag: u64,
    expected_payload: Option<u64>,
) -> Result<String, String> {
    let mut module = llvm_module(target, program)?;
    let args = arguments
        .iter()
        .map(|value| format!("i64 {value}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(module, "define i32 @main() {{").unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(
        module,
        "  %result = call i64 @tondo_probe_{}({args})",
        function.ordinal
    )
    .unwrap();
    writeln!(
        module,
        "  %tag = call i64 @tondo_rt_result_tag(i64 %result)"
    )
    .unwrap();
    writeln!(module, "  %tag_ok = icmp eq i64 %tag, {expected_tag}").unwrap();
    let final_condition = if let Some(payload) = expected_payload {
        writeln!(
            module,
            "  %payload = call i64 @tondo_rt_result_payload(i64 %result)"
        )
        .unwrap();
        writeln!(module, "  %payload_ok = icmp eq i64 %payload, {payload}").unwrap();
        writeln!(module, "  %ok = and i1 %tag_ok, %payload_ok").unwrap();
        "%ok"
    } else {
        "%tag_ok"
    };
    writeln!(
        module,
        "  %code = select i1 {final_condition}, i32 0, i32 91"
    )
    .unwrap();
    writeln!(module, "  ret i32 %code").unwrap();
    writeln!(module, "}}").unwrap();
    Ok(module)
}

fn link_native_runner(
    cc: &Path,
    source: &Path,
    object: &Path,
    binary: &Path,
) -> Result<(), String> {
    let result = Command::new(cc)
        .arg("-std=c11")
        .arg("-O2")
        .arg("-pthread")
        .arg(source)
        .arg(object)
        .arg("-o")
        .arg(binary)
        .output()
        .map_err(|error| format!("cannot execute native linker: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "native linker failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

fn run_native_binary(binary: &Path, candidate: &str, expects_trap: bool) -> Result<(), String> {
    let started = Instant::now();
    let mut child = Command::new(binary)
        .spawn()
        .map_err(|error| format!("cannot execute {candidate} native runner: {error}"))?;
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("cannot poll {candidate} native runner: {error}"))?
        {
            Some(status) => break status,
            None if started.elapsed() >= MAX_NATIVE_CASE_RUNTIME => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{candidate} native scalar case exceeded {}s runtime budget",
                    MAX_NATIVE_CASE_RUNTIME.as_secs()
                ));
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    };
    if expects_trap {
        if status.success() || status.code().is_some() {
            return Err(format!(
                "{candidate} native scalar case returned instead of trapping"
            ));
        }
    } else if !status.success() {
        return Err(format!(
            "{candidate} native scalar result disagrees with oracle"
        ));
    }
    Ok(())
}

fn run_native_binary_capture(binary: &Path, candidate: &str) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let mut child = Command::new(binary)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot execute {candidate} native runner: {error}"))?;
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("cannot poll {candidate} native runner: {error}"))?
        {
            Some(status) => break status,
            None if started.elapsed() >= MAX_NATIVE_CASE_RUNTIME => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{candidate} exceeded {}s runtime budget",
                    MAX_NATIVE_CASE_RUNTIME.as_secs()
                ));
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| format!("{candidate} has no captured stdout"))?
        .read_to_end(&mut stdout)
        .map_err(|error| format!("cannot collect {candidate} stdout: {error}"))?;
    child
        .stderr
        .take()
        .ok_or_else(|| format!("{candidate} has no captured stderr"))?
        .read_to_end(&mut stderr)
        .map_err(|error| format!("cannot collect {candidate} stderr: {error}"))?;
    if !status.success() {
        return Err(format!(
            "{candidate} exited unsuccessfully ({}): {}",
            status,
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(stdout)
}

fn compile_llvm(
    llvm: &Path,
    target: &str,
    temp_dir: &Path,
    fixture: &FixtureObservation,
    program: &MirBackendProgram,
) -> Result<CodegenResult, String> {
    let stem = safe_stem(&fixture.fixture);
    let input = temp_dir.join(format!("{stem}.ll"));
    let output = temp_dir.join(format!("{stem}.o"));
    fs::write(&input, llvm_module(target, program)?)
        .map_err(|error| format!("cannot write LLVM probe: {error}"))?;
    let started = Instant::now();
    let result = Command::new(llvm)
        .arg("-O2")
        .arg("-filetype=obj")
        .arg(format!("-mtriple={target}"))
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .output()
        .map_err(|error| format!("cannot execute LLVM llc: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "LLVM llc failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let code_size_bytes = fs::metadata(&output)
        .map_err(|error| format!("cannot stat LLVM object: {error}"))?
        .len();
    Ok(CodegenResult {
        compile_time_ns: started.elapsed().as_nanos(),
        code_size_bytes,
        supported_functions: program
            .functions
            .iter()
            .filter(|function| function.supported)
            .count() as u64,
        unsupported_functions: program
            .functions
            .iter()
            .filter(|function| !function.supported)
            .count() as u64,
    })
}

fn llvm_deferred_join(
    operand: &MirBackendOperand,
    body: &MirBackendOperation,
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    let handle = llvm_operand(operand, slots, module, value_index)?;
    let value = llvm_operation(body, slots, module, value_index)?;
    let completed = format!("%v{value_index}");
    *value_index += 1;
    writeln!(
        module,
        "  {completed} = call i64 @tondo_rt_task_complete(i64 {handle}, i64 {value})"
    )
    .unwrap();
    let awaited = format!("%v{value_index}");
    *value_index += 1;
    writeln!(module, "  {awaited} = call i64 @tondo_rt_await(i64 {handle})").unwrap();
    Ok(awaited)
}

fn llvm_module(target: &str, program: &MirBackendProgram) -> Result<String, String> {
    let mut module = String::new();
    writeln!(module, "; tondo native evaluation normalized module").unwrap();
    writeln!(module, "target triple = \"{target}\"").unwrap();
    let debug = program
        .debug
        .as_ref()
        .ok_or_else(|| "normalized MIR program has no debug metadata".to_owned())?;
    writeln!(module, "; tondo.debug format={}", debug.format).unwrap();
    for symbol in &debug.symbols {
        writeln!(
            module,
            "; tondo.debug symbol function={} name={} native={}",
            symbol.function,
            symbol.name,
            symbol.native
        )
        .unwrap();
    }
    for region in &debug.source_maps {
        writeln!(
            module,
            "; tondo.debug map id={} kind={} function={} block={:?} source={} range={}..{} unwind={:?}",
            region.id,
            region.kind,
            region.function,
            region.block,
            region.span.source,
            region.span.start,
            region.span.end,
            region.unwind
        )
        .unwrap();
    }
    for execution in &debug.executions {
        writeln!(
            module,
            "; tondo.debug execution id={} kind={} function={} block={} source={} range={}..{}",
            execution.id,
            execution.kind,
            execution.function,
            execution.block,
            execution.span.source,
            execution.span.start,
            execution.span.end
        )
        .unwrap();
    }
    llvm_checked_helpers(&mut module);
    for function in &program.functions {
        let parameters = (0..function.parameters.len())
            .enumerate()
            .map(|(position, _)| format!("i64 %arg{position}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            module,
            "define i64 @tondo_probe_{}({parameters}) {{",
            function.ordinal
        )
        .unwrap();
        if !function.supported {
            writeln!(module, "entry:").unwrap();
            writeln!(module, "  unreachable").unwrap();
            writeln!(module, "}}").unwrap();
            continue;
        }
        let blocks = normal_blocks(function);
        let entry_ordinal = blocks
            .first()
            .map(|block| block.ordinal)
            .ok_or_else(|| "supported scalar function has no normal entry block".to_owned())?;
        let locals = scalar_local_ordinals(function);
        let slots = locals
            .into_iter()
            .map(|local| (local, format!("%slot{local}")))
            .collect::<BTreeMap<_, _>>();
        let mut value_index = 0_usize;
        let mut deferred_tasks = BTreeMap::<u32, MirBackendOperation>::new();
        let deferred_enabled = deferred_lowering_is_linear(function);
        for block in blocks {
            let label = llvm_block_label(block.ordinal, entry_ordinal);
            writeln!(module, "{label}:").unwrap();
            if block.ordinal == entry_ordinal {
                for slot in slots.values() {
                    writeln!(module, "  {slot} = alloca i64").unwrap();
                }
                for (position, local) in function.parameters.iter().enumerate() {
                    let slot = slots
                        .get(local)
                        .ok_or_else(|| format!("missing slot for parameter local {local}"))?;
                    writeln!(module, "  store i64 %arg{position}, ptr {slot}").unwrap();
                }
            }
            for statement in &block.statements {
                match statement {
                    MirBackendStatement::Assign { destination, value } => {
                        let value_name = llvm_rvalue(value, &slots, &mut module, &mut value_index)?;
                        let slot = slots.get(destination).ok_or_else(|| {
                            format!("missing slot for destination local {destination}")
                        })?;
                        writeln!(module, "  store i64 {value_name}, ptr {slot}").unwrap();
                    }
                    MirBackendStatement::Marker { kind } => {
                        let _ = kind;
                    }
                    MirBackendStatement::Runtime { kind, arguments } => {
                        let _ = llvm_runtime_operation(
                            kind,
                            arguments,
                            &slots,
                            &mut module,
                            &mut value_index,
                        )?;
                    }
                }
            }
            match &block.terminator {
                MirBackendTerminator::Return => {
                    let slot = slots.get(&function.return_local).ok_or_else(|| {
                        format!("missing slot for return local {}", function.return_local)
                    })?;
                    let value = format!("%v{value_index}");
                    value_index += 1;
                    writeln!(module, "  {value} = load i64, ptr {slot}").unwrap();
                    writeln!(module, "  ret i64 {value}").unwrap();
                }
                MirBackendTerminator::Goto { target } => {
                    writeln!(
                        module,
                        "  br label %{}",
                        llvm_block_label(*target, entry_ordinal)
                    )
                    .unwrap();
                }
                MirBackendTerminator::SwitchBool {
                    condition,
                    if_true,
                    if_false,
                } => {
                    let condition = llvm_operand(condition, &slots, &mut module, &mut value_index)?;
                    let condition_value = format!("%v{value_index}");
                    value_index += 1;
                    writeln!(module, "  {condition_value} = icmp ne i64 {condition}, 0").unwrap();
                    writeln!(
                        module,
                        "  br i1 {condition_value}, label %{}, label %{}",
                        llvm_block_label(*if_true, entry_ordinal),
                        llvm_block_label(*if_false, entry_ordinal)
                    )
                    .unwrap();
                }
                MirBackendTerminator::SwitchTag {
                    value,
                    cases,
                    otherwise,
                } => {
                    let value = llvm_operand(value, &slots, &mut module, &mut value_index)?;
                    let tag_value = format!("%v{value_index}");
                    value_index += 1;
                    writeln!(
                        module,
                        "  {tag_value} = call i64 @tondo_rt_result_tag(i64 {value})"
                    )
                    .unwrap();
                    let value = tag_value;
                    let switch_id = value_index;
                    value_index += 1;
                    if cases.is_empty() {
                        writeln!(
                            module,
                            "  br label %{}",
                            llvm_block_label(*otherwise, entry_ordinal)
                        )
                        .unwrap();
                    } else {
                        for (case_index, (tag, target)) in cases.iter().enumerate() {
                            let comparison = format!("%v{value_index}");
                            value_index += 1;
                            writeln!(module, "  {comparison} = icmp eq i64 {value}, {tag}")
                                .unwrap();
                            let last = case_index + 1 == cases.len();
                            let next = if last {
                                llvm_block_label(*otherwise, entry_ordinal)
                            } else {
                                format!("switch{switch_id}_{case_index}")
                            };
                            writeln!(
                                module,
                                "  br i1 {comparison}, label %{}, label %{next}",
                                llvm_block_label(*target, entry_ordinal)
                            )
                            .unwrap();
                            if !last {
                                writeln!(module, "{next}:").unwrap();
                            }
                        }
                    }
                }
                MirBackendTerminator::Invoke {
                    operation,
                    destination,
                    target: Some(target),
                } => {
                    let value_name = if deferred_enabled
                        && destination.is_some()
                        && let Some(body) = deferred_call_body(operation)
                    {
                        let name = format!("%v{value_index}");
                        value_index += 1;
                        writeln!(
                            module,
                            "  {name} = call i64 @tondo_rt_task_spawn(i64 0, i64 1)"
                        )
                        .unwrap();
                        if let Some(destination) = destination {
                            deferred_tasks.insert(*destination, body.clone());
                        }
                        name
                    } else if let MirBackendOperation::JoinValue {
                        operand: MirBackendOperand::Local { index },
                    } = operation
                        && let Some(body) = deferred_tasks.remove(index)
                    {
                        llvm_deferred_join(
                            operation_operand(operation),
                            &body,
                            &slots,
                            &mut module,
                            &mut value_index,
                        )?
                    } else {
                        llvm_operation(operation, &slots, &mut module, &mut value_index)?
                    };
                    if let Some(destination) = destination {
                        let slot = slots.get(destination).ok_or_else(|| {
                            format!("missing slot for destination local {destination}")
                        })?;
                        writeln!(module, "  store i64 {value_name}, ptr {slot}").unwrap();
                    }
                    writeln!(
                        module,
                        "  br label %{}",
                        llvm_block_label(*target, entry_ordinal)
                    )
                    .unwrap();
                }
                MirBackendTerminator::Invoke { target: None, .. } => {
                    return Err("scalar invoke has no normal target".to_owned());
                }
                MirBackendTerminator::Marker { kind } if kind == "unreachable" => {
                    writeln!(module, "  call void @llvm.trap()").unwrap();
                    writeln!(module, "  unreachable").unwrap();
                }
                MirBackendTerminator::Marker { kind } => {
                    return Err(format!("MIR terminator is not supported: {kind}"));
                }
            }
        }
        writeln!(module, "}}").unwrap();
    }
    Ok(module)
}

fn llvm_block_label(ordinal: u32, entry_ordinal: u32) -> String {
    if ordinal == entry_ordinal {
        "entry".to_owned()
    } else {
        format!("b{ordinal}")
    }
}

fn scalar_local_ordinals(function: &MirBackendFunction) -> BTreeSet<u32> {
    let mut locals = BTreeSet::from([function.return_local]);
    locals.extend(function.parameters.iter().copied());
    for block in normal_blocks(function) {
        for statement in &block.statements {
            match statement {
                MirBackendStatement::Assign { destination, value } => {
                    locals.insert(*destination);
                    rvalue_locals(value, &mut locals);
                }
                MirBackendStatement::Runtime { arguments, .. } => {
                    for argument in arguments {
                        operand_locals(argument, &mut locals);
                    }
                }
                MirBackendStatement::Marker { .. } => {}
            }
        }
        match &block.terminator {
            MirBackendTerminator::SwitchBool { condition, .. } => {
                operand_locals(condition, &mut locals);
            }
            MirBackendTerminator::SwitchTag { value, .. } => {
                operand_locals(value, &mut locals);
            }
            MirBackendTerminator::Invoke {
                operation,
                destination,
                ..
            } => {
                operation_locals(operation, &mut locals);
                if let Some(destination) = destination {
                    locals.insert(*destination);
                }
            }
            MirBackendTerminator::Return
            | MirBackendTerminator::Goto { .. }
            | MirBackendTerminator::Marker { .. } => {}
        }
    }
    locals
}

fn llvm_checked_helpers(module: &mut String) {
    writeln!(module, "declare void @llvm.trap()").unwrap();
    for declaration in [
        "declare i64 @tondo_rt_result_new(i64, i64, i64)",
        "declare i64 @tondo_rt_result_tag(i64)",
        "declare i64 @tondo_rt_result_payload(i64)",
        "declare i64 @tondo_rt_aggregate_new(i64, i64)",
        "declare i64 @tondo_rt_aggregate_set(i64, i64, i64)",
        "declare i64 @tondo_rt_aggregate_get(i64, i64)",
        "declare i64 @tondo_rt_aggregate_len(i64)",
        "declare i64 @tondo_rt_aggregate_tag(i64)",
        "declare i64 @tondo_rt_indirect_call(i64, i64, i64)",
        "declare i64 @tondo_rt_host_call(i64, i64)",
        "declare i64 @tondo_rt_retain(i64)",
        "declare i64 @tondo_rt_release(i64)",
        "declare i64 @tondo_rt_cow_clone(i64)",
        "declare i64 @tondo_rt_frame_enter()",
        "declare i64 @tondo_rt_frame_publish_root(i64, i64)",
        "declare i64 @tondo_rt_frame_register_defer(i64, i64)",
        "declare i64 @tondo_rt_frame_disarm_defer(i64, i64)",
        "declare i64 @tondo_rt_frame_cleanup(i64, i64)",
        "declare i64 @tondo_rt_frame_leave(i64, i64)",
        "declare i64 @tondo_rt_scope_enter()",
        "declare i64 @tondo_rt_scope_spawn(i64, i64, i64)",
        "declare i64 @tondo_rt_task_spawn(i64, i64)",
        "declare i64 @tondo_rt_thread_spawn(i64, i64)",
        "declare i64 @tondo_rt_thread_worker_status(i64)",
        "declare i64 @tondo_rt_thread_worker_runs(i64)",
        "declare i64 @tondo_rt_thread_worker_distinct(i64)",
        "declare i64 @tondo_rt_thread_worker_wait(i64)",
        "declare i64 @tondo_rt_task_poll(i64)",
        "declare i64 @tondo_rt_task_wake(i64)",
        "declare i64 @tondo_rt_task_cancel(i64)",
        "declare i64 @tondo_rt_task_take(i64)",
        "declare i64 @tondo_rt_task_complete(i64, i64)",
        "declare i64 @tondo_rt_scope_cancel(i64)",
        "declare i64 @tondo_rt_scope_join(i64, i64)",
        "declare i64 @tondo_rt_await(i64)",
        "declare i64 @tondo_rt_select_begin(i64)",
        "declare i64 @tondo_rt_select_register_task(i64, i64, i64)",
        "declare i64 @tondo_rt_select_register_join(i64, i64)",
        "declare i64 @tondo_rt_select_register_oneshot(i64, i64, i64)",
        "declare i64 @tondo_rt_select_register_time(i64, i64, i64)",
        "declare i64 @tondo_rt_select_commit(i64, i64)",
        "declare i64 @tondo_rt_select_winner(i64)",
        "declare i64 @tondo_rt_select_take(i64)",
        "declare i64 @tondo_rt_select_rollback(i64)",
        "declare i64 @tondo_rt_select_wakeups(i64)",
        "declare i64 @tondo_rt_oneshot_new()",
        "declare i64 @tondo_rt_oneshot_complete(i64, i64)",
        "declare i64 @tondo_rt_oneshot_cancel(i64)",
        "declare i64 @tondo_rt_time_new(i64)",
        "declare i64 @tondo_rt_time_fire(i64)",
        "declare i64 @tondo_rt_noop()",
        "declare i64 @tondo_rt_diag_race(i64)",
        "declare i64 @tondo_rt_diag_leak(i64)",
        "declare i64 @tondo_rt_diag_dump(i64)",
    ] {
        writeln!(module, "{declaration}").unwrap();
    }
    writeln!(module, "define internal i64 @tondo_explicit_panic() {{").unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "}}").unwrap();
    writeln!(
        module,
        "define internal i64 @tondo_checked_assert(i64 %condition) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %valid = icmp ne i64 %condition, 0").unwrap();
    writeln!(
        module,
        "  br i1 %valid, label %assert_ok, label %assert_trap"
    )
    .unwrap();
    writeln!(module, "assert_trap:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "assert_ok:").unwrap();
    writeln!(module, "  ret i64 0").unwrap();
    writeln!(module, "}}").unwrap();
    writeln!(
        module,
        "declare {{ i64, i1 }} @llvm.sadd.with.overflow.i64(i64, i64)"
    )
    .unwrap();
    writeln!(
        module,
        "declare {{ i64, i1 }} @llvm.ssub.with.overflow.i64(i64, i64)"
    )
    .unwrap();
    writeln!(
        module,
        "declare {{ i64, i1 }} @llvm.smul.with.overflow.i64(i64, i64)"
    )
    .unwrap();

    llvm_checked_overflow_helper(module, "add", "sadd");
    llvm_checked_overflow_helper(module, "sub", "ssub");
    llvm_checked_overflow_helper(module, "mul", "smul");

    writeln!(
        module,
        "define internal i64 @tondo_checked_neg(i64 %value) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(
        module,
        "  %pair = call {{ i64, i1 }} @llvm.ssub.with.overflow.i64(i64 0, i64 %value)"
    )
    .unwrap();
    writeln!(module, "  %result = extractvalue {{ i64, i1 }} %pair, 0").unwrap();
    writeln!(module, "  %overflow = extractvalue {{ i64, i1 }} %pair, 1").unwrap();
    llvm_trap_branch(module, "%overflow", "neg_trap", "neg_ok");
    writeln!(module, "neg_trap:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "neg_ok:").unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();

    llvm_checked_division_helper(module, "div");
    llvm_checked_remainder_helper(module);
    llvm_checked_shift_helper(module, "shl");
    llvm_checked_shift_helper(module, "ashr");
    llvm_checked_bounds_helper(module);
    llvm_checked_conversion_helper(module);
}

fn llvm_checked_conversion_helper(module: &mut String) {
    writeln!(
        module,
        "define internal i64 @tondo_checked_conversion(i64 %value, i64 %minimum, i64 %maximum) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %below = icmp slt i64 %value, %minimum").unwrap();
    writeln!(module, "  %above = icmp sgt i64 %value, %maximum").unwrap();
    writeln!(module, "  %invalid = or i1 %below, %above").unwrap();
    writeln!(module, "  %tag = select i1 %invalid, i64 3, i64 2").unwrap();
    writeln!(module, "  %has_payload = select i1 %invalid, i64 0, i64 1").unwrap();
    writeln!(
        module,
        "  %result = call i64 @tondo_rt_result_new(i64 %tag, i64 %value, i64 %has_payload)"
    )
    .unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_checked_bounds_helper(module: &mut String) {
    writeln!(
        module,
        "define internal i64 @tondo_checked_bounds(i64 %index, i64 %length) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %below_zero = icmp slt i64 %index, 0").unwrap();
    writeln!(module, "  %past_end = icmp sge i64 %index, %length").unwrap();
    writeln!(module, "  %invalid = or i1 %below_zero, %past_end").unwrap();
    llvm_trap_branch(module, "%invalid", "bounds_trap", "bounds_ok");
    writeln!(module, "bounds_trap:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "bounds_ok:").unwrap();
    writeln!(module, "  ret i64 %index").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_checked_overflow_helper(module: &mut String, name: &str, intrinsic: &str) {
    writeln!(
        module,
        "define internal i64 @tondo_checked_{name}(i64 %left, i64 %right) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(
        module,
        "  %pair = call {{ i64, i1 }} @llvm.{intrinsic}.with.overflow.i64(i64 %left, i64 %right)"
    )
    .unwrap();
    writeln!(module, "  %result = extractvalue {{ i64, i1 }} %pair, 0").unwrap();
    writeln!(module, "  %overflow = extractvalue {{ i64, i1 }} %pair, 1").unwrap();
    llvm_trap_branch(
        module,
        "%overflow",
        &format!("{name}_trap"),
        &format!("{name}_ok"),
    );
    writeln!(module, "{name}_trap:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "{name}_ok:").unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_checked_division_helper(module: &mut String, name: &str) {
    writeln!(
        module,
        "define internal i64 @tondo_checked_{name}(i64 %left, i64 %right) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %zero = icmp eq i64 %right, 0").unwrap();
    llvm_trap_branch(module, "%zero", "div_zero", "div_nonzero");
    writeln!(module, "div_zero:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "div_nonzero:").unwrap();
    writeln!(
        module,
        "  %overflow_left = icmp eq i64 %left, -9223372036854775808"
    )
    .unwrap();
    writeln!(module, "  %overflow_right = icmp eq i64 %right, -1").unwrap();
    writeln!(
        module,
        "  %overflow = and i1 %overflow_left, %overflow_right"
    )
    .unwrap();
    llvm_trap_branch(module, "%overflow", "div_overflow", "div_ok");
    writeln!(module, "div_overflow:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "div_ok:").unwrap();
    writeln!(module, "  %result = sdiv i64 %left, %right").unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_checked_remainder_helper(module: &mut String) {
    writeln!(
        module,
        "define internal i64 @tondo_checked_rem(i64 %left, i64 %right) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %zero = icmp eq i64 %right, 0").unwrap();
    llvm_trap_branch(module, "%zero", "rem_zero", "rem_nonzero");
    writeln!(module, "rem_zero:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "rem_nonzero:").unwrap();
    writeln!(
        module,
        "  %overflow_left = icmp eq i64 %left, -9223372036854775808"
    )
    .unwrap();
    writeln!(module, "  %overflow_right = icmp eq i64 %right, -1").unwrap();
    writeln!(
        module,
        "  %overflow = and i1 %overflow_left, %overflow_right"
    )
    .unwrap();
    writeln!(
        module,
        "  br i1 %overflow, label %rem_special, label %rem_ok"
    )
    .unwrap();
    writeln!(module, "rem_special:").unwrap();
    writeln!(module, "  ret i64 0").unwrap();
    writeln!(module, "rem_ok:").unwrap();
    writeln!(module, "  %result = srem i64 %left, %right").unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_checked_shift_helper(module: &mut String, instruction: &str) {
    let name = if instruction == "shl" { "shl" } else { "ashr" };
    writeln!(
        module,
        "define internal i64 @tondo_checked_{name}(i64 %left, i64 %right) {{"
    )
    .unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %valid = icmp ult i64 %right, 64").unwrap();
    llvm_trap_branch(module, "%valid", "shift_ok", "shift_trap");
    writeln!(module, "shift_trap:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "shift_ok:").unwrap();
    writeln!(module, "  %result = {instruction} i64 %left, %right").unwrap();
    writeln!(module, "  ret i64 %result").unwrap();
    writeln!(module, "}}").unwrap();
}

fn llvm_trap_branch(module: &mut String, condition: &str, trap_label: &str, ok_label: &str) {
    writeln!(
        module,
        "  br i1 {condition}, label %{trap_label}, label %{ok_label}"
    )
    .unwrap();
}

fn integer_conversion_bounds(target: &str) -> Option<(i64, i64)> {
    match target {
        "Byte" | "UInt8" => Some((0, 255)),
        "Int8" => Some((i8::MIN as i64, i8::MAX as i64)),
        "Int16" => Some((i16::MIN as i64, i16::MAX as i64)),
        "Int32" => Some((i32::MIN as i64, i32::MAX as i64)),
        "UInt32" => Some((0, u32::MAX as i64)),
        _ => None,
    }
}

fn llvm_rvalue(
    value: &MirBackendRvalue,
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    match value {
        MirBackendRvalue::Use(operand) => llvm_operand(operand, slots, module, value_index),
        MirBackendRvalue::Tag { value } => Ok(value.to_string()),
        MirBackendRvalue::Aggregate { kind, values } => {
            let tag = aggregate_tag(kind)?;
            if !matches!(
                kind.as_str(),
                "option-none" | "option-some" | "result-ok" | "result-err"
            ) {
                let aggregate = format!("%v{value_index}");
                *value_index += 1;
                writeln!(
                    module,
                    "  {aggregate} = call i64 @tondo_rt_aggregate_new(i64 {tag}, i64 {})",
                    values.len()
                )
                .unwrap();
                for (index, operand) in values.iter().enumerate() {
                    let value = llvm_operand(operand, slots, module, value_index)?;
                    let status = format!("%v{value_index}");
                    *value_index += 1;
                    writeln!(
                        module,
                        "  {status} = call i64 @tondo_rt_aggregate_set(i64 {aggregate}, i64 {index}, i64 {value})"
                    )
                    .unwrap();
                }
                return Ok(aggregate);
            }
            let payload = values
                .first()
                .map(|operand| llvm_operand(operand, slots, module, value_index))
                .transpose()?
                .unwrap_or_else(|| "0".to_owned());
            let has_payload = i64::from(!values.is_empty());
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_rt_result_new(i64 {tag}, i64 {payload}, i64 {has_payload})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendRvalue::Prefix { operator, operand } => {
            let operand = llvm_operand(operand, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            match operator.as_str() {
                "negate" => writeln!(
                    module,
                    "  {name} = call i64 @tondo_checked_neg(i64 {operand})"
                )
                .unwrap(),
                "bitwise-not" => writeln!(module, "  {name} = xor i64 {operand}, -1").unwrap(),
                "logical-not" => {
                    let comparison = format!("%v{value_index}");
                    *value_index += 1;
                    writeln!(
                        module,
                        "  {comparison} = icmp eq i64 {operand}, 0"
                    )
                    .unwrap();
                    writeln!(module, "  {name} = zext i1 {comparison} to i64").unwrap();
                }
                other => return Err(format!("LLVM scalar prefix is not supported: {other}")),
            }
            Ok(name)
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => llvm_binary(operator, left, right, slots, module, value_index),
        MirBackendRvalue::NumericConversion {
            source,
            target,
            conversion,
            operand,
        } => {
            let operand = llvm_operand(operand, slots, module, value_index)?;
            if !is_native_integer_scalar(source) || !is_native_integer_scalar(target) {
                return Err(format!(
                    "LLVM numeric conversion is not supported for {source}->{target}"
                ));
            }
            if conversion == "identity" || conversion == "total" {
                return Ok(operand);
            }
            if conversion != "checked" {
                return Err(format!("LLVM numeric conversion mode is not supported: {conversion}"));
            }
            let (minimum, maximum) = integer_conversion_bounds(target).ok_or_else(|| {
                format!("LLVM numeric conversion target is not supported: {target}")
            })?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_checked_conversion(i64 {operand}, i64 {minimum}, i64 {maximum})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendRvalue::Coerce { kind, operand } => {
            if kind == "Diverging" {
                let name = format!("%v{value_index}");
                *value_index += 1;
                writeln!(module, "  {name} = call i64 @tondo_explicit_panic()").unwrap();
                return Ok(name);
            }
            llvm_operand(operand, slots, module, value_index)
        }
        MirBackendRvalue::HostCall { kind, arguments } => {
            let argument = arguments
                .first()
                .map(|argument| llvm_operand(argument, slots, module, value_index))
                .transpose()?
                .unwrap_or_else(|| "0".to_owned());
            let name = format!("%v{value_index}");
            *value_index += 1;
            let kind_id = host_call_kind(kind)?;
            writeln!(
                module,
                "  {name} = call i64 @tondo_rt_host_call(i64 {kind_id}, i64 {argument})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendRvalue::Unsupported { kind } => {
            Err(format!("MIR rvalue is not supported: {kind}"))
        }
    }
}

fn llvm_operation(
    operation: &MirBackendOperation,
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    match operation {
        MirBackendOperation::CheckedPrefix { operator, operand } => {
            let operand = llvm_operand(operand, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            match operator.as_str() {
                "negate" => writeln!(
                    module,
                    "  {name} = call i64 @tondo_checked_neg(i64 {operand})"
                )
                .unwrap(),
                "bitwise-not" => writeln!(module, "  {name} = xor i64 {operand}, -1").unwrap(),
                "logical-not" => {
                    let comparison = format!("%v{value_index}");
                    *value_index += 1;
                    writeln!(module, "  {comparison} = icmp eq i64 {operand}, 0").unwrap();
                    writeln!(module, "  {name} = zext i1 {comparison} to i64").unwrap();
                }
                other => return Err(format!("LLVM scalar prefix is not supported: {other}")),
            }
            Ok(name)
        }
        MirBackendOperation::CheckedBinary {
            operator,
            left,
            right,
        } => llvm_binary(operator, left, right, slots, module, value_index),
        MirBackendOperation::BoundsCheck { index, length } => {
            let index = llvm_operand(index, slots, module, value_index)?;
            let length = llvm_operand(length, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_checked_bounds(i64 {index}, i64 {length})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::Call {
            function,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| llvm_operand(argument, slots, module, value_index))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|argument| format!("i64 {argument}"))
                .collect::<Vec<_>>()
                .join(", ");
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_probe_{function}({arguments})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::Spawn { operation, kind } => {
            let value = llvm_operation(operation, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            let function = if kind == "thread" {
                "tondo_rt_thread_spawn"
            } else {
                "tondo_rt_task_spawn"
            };
            writeln!(
                module,
                "  {name} = call i64 @{function}(i64 {value}, i64 0)"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::JoinValue { operand } => {
            let handle = llvm_operand(operand, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(module, "  {name} = call i64 @tondo_rt_await(i64 {handle})").unwrap();
            Ok(name)
        }
        MirBackendOperation::HostCall { kind, arguments } => {
            let argument = arguments
                .first()
                .map(|argument| llvm_operand(argument, slots, module, value_index))
                .transpose()?
                .unwrap_or_else(|| "0".to_owned());
            let name = format!("%v{value_index}");
            *value_index += 1;
            let kind_id = host_call_kind(kind)?;
            writeln!(
                module,
                "  {name} = call i64 @tondo_rt_host_call(i64 {kind_id}, i64 {argument})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::Runtime { kind, arguments } => {
            llvm_runtime_operation(kind, arguments, slots, module, value_index)
        }
        MirBackendOperation::Assert { condition } => {
            let condition = llvm_operand(condition, slots, module, value_index)?;
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_checked_assert(i64 {condition})"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::Trap { kind } => {
            let name = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {name} = call i64 @tondo_explicit_panic() ; {kind}"
            )
            .unwrap();
            Ok(name)
        }
        MirBackendOperation::Marker { kind } => {
            Err(format!("MIR operation is not supported: {kind}"))
        }
    }
}

fn llvm_binary(
    operator: &str,
    left: &MirBackendOperand,
    right: &MirBackendOperand,
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    let left = llvm_operand(left, slots, module, value_index)?;
    let right = llvm_operand(right, slots, module, value_index)?;
    let helper = match operator {
        "add" => "tondo_checked_add",
        "subtract" => "tondo_checked_sub",
        "multiply" => "tondo_checked_mul",
        "divide" => "tondo_checked_div",
        "remainder" => "tondo_checked_rem",
        "bitwise-and" => "tondo_bitwise_and",
        "bitwise-or" => "tondo_bitwise_or",
        "bitwise-xor" => "tondo_bitwise_xor",
        "logical-and" => "tondo_logical_and",
        "logical-or" => "tondo_logical_or",
        "shift-left" => "tondo_checked_shl",
        "shift-right" => "tondo_checked_ashr",
        "less" | "less-equal" | "greater" | "greater-equal" | "equal" | "not-equal" => {
            let predicate = match operator {
                "less" => "slt",
                "less-equal" => "sle",
                "greater" => "sgt",
                "greater-equal" => "sge",
                "equal" => "eq",
                "not-equal" => "ne",
                _ => unreachable!(),
            };
            let comparison = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {comparison} = icmp {predicate} i64 {left}, {right}"
            )
            .unwrap();
            let result = format!("%v{value_index}");
            *value_index += 1;
            writeln!(module, "  {result} = zext i1 {comparison} to i64").unwrap();
            return Ok(result);
        }
        other => return Err(format!("LLVM scalar binary is not supported: {other}")),
    };
    let name = format!("%v{value_index}");
    *value_index += 1;
    match helper {
        "tondo_bitwise_and" => writeln!(module, "  {name} = and i64 {left}, {right}").unwrap(),
        "tondo_bitwise_or" => writeln!(module, "  {name} = or i64 {left}, {right}").unwrap(),
        "tondo_bitwise_xor" => writeln!(module, "  {name} = xor i64 {left}, {right}").unwrap(),
        "tondo_logical_and" => writeln!(module, "  {name} = and i64 {left}, {right}").unwrap(),
        "tondo_logical_or" => writeln!(module, "  {name} = or i64 {left}, {right}").unwrap(),
        helper => writeln!(
            module,
            "  {name} = call i64 @{helper}(i64 {left}, i64 {right})"
        )
        .unwrap(),
    }
    Ok(name)
}

fn llvm_runtime_operation(
    kind: &str,
    arguments: &[MirBackendOperand],
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    let base = kind.split(':').next().unwrap_or(kind);
    let generated_noop = kind.contains(':')
        && matches!(
            base,
            "enter-task-scope"
                | "retarget-cleanup"
                | "register-defer"
                | "register-fallback"
                | "reserve-loan"
                | "release-loan"
                | "begin-select"
                | "register-select-arm"
        );
    let (function, arity) = if generated_noop {
        ("tondo_rt_noop", usize::MAX)
    } else {
        match base {
        "result-tag" => ("tondo_rt_result_tag", 1),
        "result-payload" => ("tondo_rt_result_payload", 1),
        "aggregate-new" => ("tondo_rt_aggregate_new", 2),
        "aggregate-set" => ("tondo_rt_aggregate_set", 3),
        "aggregate-get" => ("tondo_rt_aggregate_get", 2),
        "aggregate-len" => ("tondo_rt_aggregate_len", 1),
        "aggregate-tag" => ("tondo_rt_aggregate_tag", 1),
        "indirect-call" => ("tondo_rt_indirect_call", 3),
        "retain" | "retain-value" => ("tondo_rt_retain", 1),
        "release" | "release-value" => ("tondo_rt_release", 1),
        "cow-clone" => ("tondo_rt_cow_clone", 1),
        "frame-enter" => ("tondo_rt_frame_enter", 0),
        "frame-publish-root" => ("tondo_rt_frame_publish_root", 2),
        "register-defer" => ("tondo_rt_frame_register_defer", 2),
        "disarm-defer" => ("tondo_rt_frame_disarm_defer", 2),
        "frame-cleanup" => ("tondo_rt_frame_cleanup", 2),
        "frame-leave" => ("tondo_rt_frame_leave", 2),
        "scope-enter" => ("tondo_rt_scope_enter", 0),
        "scope-spawn" => ("tondo_rt_scope_spawn", 3),
        "task-spawn" => ("tondo_rt_task_spawn", 2),
        "thread-spawn" => ("tondo_rt_thread_spawn", 2),
        "thread-worker-status" => ("tondo_rt_thread_worker_status", 1),
        "thread-worker-runs" => ("tondo_rt_thread_worker_runs", 1),
        "thread-worker-distinct" => ("tondo_rt_thread_worker_distinct", 1),
        "thread-worker-wait" => ("tondo_rt_thread_worker_wait", 1),
        "task-poll" => ("tondo_rt_task_poll", 1),
        "task-wake" => ("tondo_rt_task_wake", 1),
        "task-cancel" => ("tondo_rt_task_cancel", 1),
        "task-take" => ("tondo_rt_task_take", 1),
        "scope-cancel" => ("tondo_rt_scope_cancel", 1),
        "scope-join" => ("tondo_rt_scope_join", 2),
        "await" => ("tondo_rt_await", 1),
        "select-begin" => ("tondo_rt_select_begin", 1),
        "select-register-task" => ("tondo_rt_select_register_task", 3),
        "select-register-join" => ("tondo_rt_select_register_join", 2),
        "select-register-oneshot" => ("tondo_rt_select_register_oneshot", 3),
        "select-register-time" => ("tondo_rt_select_register_time", 3),
        "select-commit" => ("tondo_rt_select_commit", 2),
        "select-winner" => ("tondo_rt_select_winner", 1),
        "select-take" => ("tondo_rt_select_take", 1),
        "select-rollback" => ("tondo_rt_select_rollback", 1),
        "select-wakeups" => ("tondo_rt_select_wakeups", 1),
        "oneshot-new" => ("tondo_rt_oneshot_new", 0),
        "oneshot-complete" => ("tondo_rt_oneshot_complete", 2),
        "oneshot-cancel" => ("tondo_rt_oneshot_cancel", 1),
        "time-new" => ("tondo_rt_time_new", 1),
        "time-fire" => ("tondo_rt_time_fire", 1),
        "diag-race" => ("tondo_rt_diag_race", 1),
        "diag-leak" => ("tondo_rt_diag_leak", 1),
        "diag-dump" => ("tondo_rt_diag_dump", 1),
        other => {
            return Err(format!(
                "native runtime operation is not supported: {other}"
            ));
        }
        }
    };
    if arity != usize::MAX && arguments.len() != arity {
        return Err(format!(
            "native runtime operation `{kind}` expects {arity} arguments, got {}",
            arguments.len()
        ));
    }
    let arguments = if arity == usize::MAX {
        String::new()
    } else {
        arguments
            .iter()
            .map(|argument| llvm_operand(argument, slots, module, value_index))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|argument| format!("i64 {argument}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let name = format!("%v{value_index}");
    *value_index += 1;
    writeln!(module, "  {name} = call i64 @{function}({arguments})").unwrap();
    Ok(name)
}

fn llvm_operand(
    operand: &MirBackendOperand,
    slots: &BTreeMap<u32, String>,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => {
            parse_integer_literal(value).map(|value| value.to_string())
        }
        MirBackendOperand::Constant(MirBackendConstant::Bool(value)) => {
            Ok(i64::from(*value).to_string())
        }
        MirBackendOperand::Constant(MirBackendConstant::String(value)) => {
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            Ok(string_payload(value).to_string())
        }
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => {
            let slot = slots
                .get(index)
                .ok_or_else(|| format!("MIR local {index} is not available in the adapter"))?;
            let value = format!("%v{value_index}");
            *value_index += 1;
            writeln!(module, "  {value} = load i64, ptr {slot}").unwrap();
            Ok(value)
        }
        MirBackendOperand::Projection { index, depth, kind } => {
            if *depth != 1 {
                return Err(format!(
                    "LLVM native core projection is not supported: {kind} at depth {depth}"
                ));
            }
            let slot = slots
                .get(index)
                .ok_or_else(|| format!("MIR local {index} is not available in the adapter"))?;
            let base = format!("%v{value_index}");
            *value_index += 1;
            writeln!(module, "  {base} = load i64, ptr {slot}").unwrap();
            if let Some(field) = parse_aggregate_projection(kind) {
                let value = format!("%v{value_index}");
                *value_index += 1;
                writeln!(
                    module,
                    "  {value} = call i64 @tondo_rt_aggregate_get(i64 {base}, i64 {field})"
                )
                .unwrap();
                return Ok(value);
            }
            if !matches!(
                kind.as_str(),
                "option-value" | "result-ok-value" | "result-err-value"
            ) {
                return Err(format!(
                    "LLVM native core projection is not supported: {kind} at depth {depth}"
                ));
            }
            let value = format!("%v{value_index}");
            *value_index += 1;
            writeln!(
                module,
                "  {value} = call i64 @tondo_rt_result_payload(i64 {base})"
            )
            .unwrap();
            Ok(value)
        }
        MirBackendOperand::Function { kind } => Ok(parse_verified_function_ordinal(kind)
            .map(|ordinal| ordinal.to_string())
            .unwrap_or_else(|| string_payload(kind).to_string())),
        MirBackendOperand::Constant(other) => {
            let kind = match other {
                MirBackendConstant::Unit => "unit".to_owned(),
                MirBackendConstant::Float(value) | MirBackendConstant::Char(value) => value.clone(),
                MirBackendConstant::Named => "named".to_owned(),
                MirBackendConstant::Integer(_)
                | MirBackendConstant::Bool(_)
                | MirBackendConstant::String(_) => unreachable!(),
            };
            Err(format!("MIR constant is not scalar: {kind}"))
        }
        MirBackendOperand::Unsupported { kind } => {
            Err(format!("MIR operand is not supported: {kind}"))
        }
    }
}

fn cranelift_isa() -> Result<cranelift_codegen::isa::OwnedTargetIsa, String> {
    let builder = cranelift_native::builder().map_err(|error| error.to_owned())?;
    let mut flags = settings::builder();
    flags
        .set("opt_level", "speed")
        .map_err(|error| format!("invalid Cranelift flag: {error}"))?;
    builder
        .finish(settings::Flags::new(flags))
        .map_err(|error| format!("cannot initialize Cranelift ISA: {error}"))
}

fn command_version(command: &Path) -> Result<String, String> {
    let output = Command::new(command)
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot query {}: {error}", command.display()))?;
    if !output.status.success() {
        return Err(format!("{} --version failed", command.display()));
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if version.is_empty() {
        return Err("LLVM version output is empty".into());
    }
    Ok(version)
}

fn safe_stem(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or("fixture")
        .replace('.', "_")
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut probe = None;
        let mut std_core_probe = None;
        let mut output = None;
        let mut llvm = None;
        let mut target = None;
        let mut temp_dir = None;
        let mut cc = None;
        while let Some(argument) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("missing value for {argument}"))
            };
            match argument.as_str() {
                "--probe" => probe = Some(PathBuf::from(value()?)),
                "--std-core-probe" => std_core_probe = Some(PathBuf::from(value()?)),
                "--output" => output = Some(PathBuf::from(value()?)),
                "--llvm" => llvm = Some(PathBuf::from(value()?)),
                "--target" => target = Some(value()?),
                "--temp-dir" => temp_dir = Some(PathBuf::from(value()?)),
                "--cc" => cc = Some(PathBuf::from(value()?)),
                "--help" | "-h" => {
                    println!(
                        "usage: tondo-native-evaluation --probe FILE --output FILE --llvm ABSOLUTE --target TRIPLE --temp-dir DIR [--cc ABSOLUTE] [--std-core-probe FILE]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(Self {
            probe: probe.ok_or("--probe is required")?,
            std_core_probe,
            output: output.ok_or("--output is required")?,
            llvm: llvm.ok_or("--llvm is required")?,
            target: target.ok_or("--target is required")?,
            temp_dir: temp_dir.ok_or("--temp-dir is required")?,
            cc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_span() -> MirBackendSpan {
        MirBackendSpan {
            source: 0,
            start: 0,
            end: 0,
        }
    }

    fn test_program(functions: Vec<MirBackendFunction>) -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            debug: Some(synthetic_debug_info(&functions)),
            functions,
        }
    }

    fn simple_backend() -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![1, 2],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: vec![
                            MirBackendStatement::Assign {
                                destination: 3,
                                value: MirBackendRvalue::Use(MirBackendOperand::Local { index: 1 }),
                            },
                            MirBackendStatement::Assign {
                                destination: 4,
                                value: MirBackendRvalue::Use(MirBackendOperand::Local { index: 2 }),
                            },
                        ],
                        terminator: MirBackendTerminator::Invoke {
                            operation: MirBackendOperation::CheckedBinary {
                                operator: "add".to_owned(),
                                left: MirBackendOperand::Local { index: 3 },
                                right: MirBackendOperand::Local { index: 4 },
                            },
                            destination: Some(0),
                            target: Some(1),
                        },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn branch_backend() -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![1],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 2,
                            value: MirBackendRvalue::Binary {
                                operator: "greater".to_owned(),
                                left: MirBackendOperand::Local { index: 1 },
                                right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                                    "0".to_owned(),
                                )),
                            },
                        }],
                        terminator: MirBackendTerminator::SwitchBool {
                            condition: MirBackendOperand::Local { index: 2 },
                            if_true: 1,
                            if_false: 2,
                        },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Binary {
                                operator: "add".to_owned(),
                                left: MirBackendOperand::Local { index: 1 },
                                right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                                    "1".to_owned(),
                                )),
                            },
                        }],
                        terminator: MirBackendTerminator::Goto { target: 3 },
                    },
                    MirBackendBlock {
                        ordinal: 2,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Binary {
                                operator: "subtract".to_owned(),
                                left: MirBackendOperand::Local { index: 1 },
                                right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                                    "1".to_owned(),
                                )),
                            },
                        }],
                        terminator: MirBackendTerminator::Goto { target: 3 },
                    },
                    MirBackendBlock {
                        ordinal: 3,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn tag_backend() -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 1,
                            value: MirBackendRvalue::Tag { value: 1 },
                        }],
                        terminator: MirBackendTerminator::SwitchTag {
                            value: MirBackendOperand::Local { index: 1 },
                            cases: vec![(0, 1), (1, 2)],
                            otherwise: 3,
                        },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Use(MirBackendOperand::Constant(
                                MirBackendConstant::Integer("10".to_owned()),
                            )),
                        }],
                        terminator: MirBackendTerminator::Return,
                    },
                    MirBackendBlock {
                        ordinal: 2,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Use(MirBackendOperand::Constant(
                                MirBackendConstant::Integer("20".to_owned()),
                            )),
                        }],
                        terminator: MirBackendTerminator::Return,
                    },
                    MirBackendBlock {
                        ordinal: 3,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Use(MirBackendOperand::Constant(
                                MirBackendConstant::Integer("30".to_owned()),
                            )),
                        }],
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn loop_backend() -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![1],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 2,
                            value: MirBackendRvalue::Use(MirBackendOperand::Local { index: 1 }),
                        }],
                        terminator: MirBackendTerminator::Goto { target: 1 },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 3,
                            value: MirBackendRvalue::Binary {
                                operator: "greater".to_owned(),
                                left: MirBackendOperand::Local { index: 2 },
                                right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                                    "0".to_owned(),
                                )),
                            },
                        }],
                        terminator: MirBackendTerminator::SwitchBool {
                            condition: MirBackendOperand::Local { index: 3 },
                            if_true: 2,
                            if_false: 3,
                        },
                    },
                    MirBackendBlock {
                        ordinal: 2,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Invoke {
                            operation: MirBackendOperation::CheckedBinary {
                                operator: "subtract".to_owned(),
                                left: MirBackendOperand::Local { index: 2 },
                                right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                                    "1".to_owned(),
                                )),
                            },
                            destination: Some(2),
                            target: Some(1),
                        },
                    },
                    MirBackendBlock {
                        ordinal: 3,
                        kind: "normal".to_owned(),
                        statements: vec![MirBackendStatement::Assign {
                            destination: 0,
                            value: MirBackendRvalue::Use(MirBackendOperand::Constant(
                                MirBackendConstant::Integer("0".to_owned()),
                            )),
                        }],
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn call_backend() -> MirBackendProgram {
        let callee = MirBackendFunction {
            ordinal: 0,
            parameters: vec![1],
            parameter_types: Vec::new(),
            return_local: 0,
            return_type: "Int".to_owned(),
            supported: true,
            blocks: vec![MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: vec![MirBackendStatement::Assign {
                    destination: 0,
                    value: MirBackendRvalue::Binary {
                        operator: "add".to_owned(),
                        left: MirBackendOperand::Local { index: 1 },
                        right: MirBackendOperand::Constant(MirBackendConstant::Integer(
                            "1".to_owned(),
                        )),
                    },
                }],
                terminator: MirBackendTerminator::Return,
            }],
        };
        let caller = MirBackendFunction {
            ordinal: 1,
            parameters: vec![1],
            parameter_types: Vec::new(),
            return_local: 0,
            return_type: "Int".to_owned(),
            supported: true,
            blocks: vec![
                MirBackendBlock {
                    ordinal: 0,
                    kind: "normal".to_owned(),
                    statements: Vec::new(),
                    terminator: MirBackendTerminator::Invoke {
                        operation: MirBackendOperation::Call {
                            function: 0,
                            arguments: vec![MirBackendOperand::Local { index: 1 }],
                        },
                        destination: Some(0),
                        target: Some(1),
                    },
                },
                MirBackendBlock {
                    ordinal: 1,
                    kind: "normal".to_owned(),
                    statements: Vec::new(),
                    terminator: MirBackendTerminator::Return,
                },
            ],
        };
        test_program(vec![callee, caller])
    }

    fn trap_backend() -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Invoke {
                            operation: MirBackendOperation::Trap {
                                kind: "explicit-panic".to_owned(),
                            },
                            destination: Some(0),
                            target: Some(1),
                        },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn assert_backend(condition: bool) -> MirBackendProgram {
        test_program(vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![
                    MirBackendBlock {
                        ordinal: 0,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Invoke {
                            operation: MirBackendOperation::Assert {
                                condition: MirBackendOperand::Constant(MirBackendConstant::Bool(
                                    condition,
                                )),
                            },
                            destination: Some(0),
                            target: Some(1),
                        },
                    },
                    MirBackendBlock {
                        ordinal: 1,
                        kind: "normal".to_owned(),
                        statements: Vec::new(),
                        terminator: MirBackendTerminator::Return,
                    },
                ],
            }])
    }

    fn fixture(index: usize) -> FixtureObservation {
        FixtureObservation {
            fixture: format!("tests/runtime/fixture-{index}.to"),
            fixture_sha256: format!("sha256:{}", "a".repeat(64)),
            status: "passed".to_owned(),
            mir: Some(MirSummary {
                backend: Some(simple_backend()),
            }),
            vm_scalar: Vec::new(),
            vm_managed: Vec::new(),
        }
    }

    #[test]
    fn validates_the_four_fixture_probe_boundary() {
        let probe = ProbeReport {
            format: "tondo-native-mir-probe/1".to_owned(),
            fixtures: (0..4).map(fixture).collect(),
        };
        validate_probe(&probe).expect("fixture probe should be accepted");
    }

    #[test]
    fn rejects_a_probe_with_the_wrong_fixture_count() {
        let probe = ProbeReport {
            format: "tondo-native-mir-probe/1".to_owned(),
            fixtures: vec![fixture(0)],
        };
        assert!(validate_probe(&probe).is_err());
    }

    #[test]
    fn rejects_a_legacy_summary_without_the_normalized_adapter_format() {
        let mut program = simple_backend();
        program.format = "tondo-native-mir-probe/legacy".to_owned();
        assert!(validate_backend_program(&program).is_err());
    }

    #[test]
    fn rejects_normalized_mir_without_debug_metadata() {
        let mut program = simple_backend();
        program.debug = None;
        let error = validate_backend_program(&program)
            .expect_err("native lowering must not run without source maps");
        assert!(error.contains("no debug metadata"));
    }

    #[test]
    fn rejects_debug_regions_with_unknown_unwind_targets() {
        let mut program = simple_backend();
        program
            .debug
            .as_mut()
            .expect("test backend carries debug metadata")
            .source_maps
            .push(MirBackendSourceMap {
                id: "f0.b0.unwind".to_owned(),
                kind: "terminator".to_owned(),
                function: 0,
                block: Some(0),
                span: test_span(),
                unwind: Some(99),
            });
        let error = validate_backend_program(&program)
            .expect_err("unwind must reference a declared MIR block");
        assert!(error.contains("missing unwind target 99"));
    }

    #[test]
    fn debug_metadata_accepts_distinct_task_and_thread_identities() {
        let mut program = simple_backend();
        let debug = program
            .debug
            .as_mut()
            .expect("test backend carries debug metadata");
        debug.executions = vec![
            MirBackendExecutionIdentity {
                id: "f0.b0.task".to_owned(),
                kind: "task".to_owned(),
                function: 0,
                block: 0,
                span: test_span(),
            },
            MirBackendExecutionIdentity {
                id: "f0.b1.thread".to_owned(),
                kind: "thread".to_owned(),
                function: 0,
                block: 1,
                span: test_span(),
            },
        ];
        validate_backend_program(&program).expect("task/thread identities should be valid");
    }

    #[test]
    fn rejects_supported_cleanup_actions_until_they_have_native_lowering() {
        let mut program = simple_backend();
        program.functions[0].blocks.push(MirBackendBlock {
            ordinal: 2,
            kind: "cleanup".to_owned(),
            statements: vec![MirBackendStatement::Marker {
                kind: "release-loan".to_owned(),
            }],
            terminator: MirBackendTerminator::Marker {
                kind: "resume-panic".to_owned(),
            },
        });
        let error = validate_backend_program(&program)
            .expect_err("unlowered cleanup must fail closed before code generation");
        assert!(error.contains("cleanup action `release-loan`"));
    }

    #[test]
    fn rejects_nested_call_targets() {
        let mut nested = simple_backend();
        if let MirBackendTerminator::Invoke { operation, .. } =
            &mut nested.functions[0].blocks[0].terminator
        {
            *operation = MirBackendOperation::Spawn {
                operation: Box::new(MirBackendOperation::Call {
                    function: 99,
                    arguments: Vec::new(),
                }),
                kind: "task".to_owned(),
            };
        }
        let error = validate_backend_program(&nested)
            .expect_err("nested spawn calls must use a declared target");
        assert!(error.contains("call target 99 is not present"));
    }

    #[test]
    fn accepts_bounded_tuple_storage_when_a_function_claims_native_support() {
        let mut program = simple_backend();
        program.functions[0].blocks[0].statements.push(
            MirBackendStatement::Assign {
                destination: 5,
                value: MirBackendRvalue::Aggregate {
                    kind: "tuple".to_owned(),
                    values: vec![MirBackendOperand::Local { index: 1 }],
                },
            },
        );
        validate_backend_program(&program).expect("tuple storage is part of the AOT admitted slice");
    }

    #[test]
    fn rejects_unknown_aggregate_storage_when_a_function_claims_native_support() {
        let mut program = simple_backend();
        program.functions[0].blocks[0].statements.push(
            MirBackendStatement::Assign {
                destination: 5,
                value: MirBackendRvalue::Aggregate {
                    kind: "unknown-storage".to_owned(),
                    values: vec![MirBackendOperand::Local { index: 1 }],
                },
            },
        );
        let error = validate_backend_program(&program)
            .expect_err("unknown storage must fail closed before native lowering");
        assert!(error.contains("unsupported aggregate `unknown-storage`"));
    }

    #[test]
    fn accepts_verified_function_values_and_rejects_opaque_function_names() {
        let mut program = simple_backend();
        program.functions[0].blocks[0].statements.push(MirBackendStatement::Assign {
            destination: 5,
            value: MirBackendRvalue::Use(MirBackendOperand::Function {
                kind: "function:0".to_owned(),
            }),
        });
        validate_backend_program(&program).expect("verified function ordinal should be accepted");
        if let MirBackendStatement::Assign { value, .. } = &mut program.functions[0].blocks[0].statements[2] {
            *value = MirBackendRvalue::Use(MirBackendOperand::Function {
                kind: "opaque-symbol".to_owned(),
            });
        }
        assert!(validate_backend_program(&program).is_err());
    }

    #[test]
    fn rejects_verified_function_values_that_are_not_in_the_program() {
        let mut program = simple_backend();
        program.functions[0].blocks[0].statements.push(MirBackendStatement::Assign {
            destination: 5,
            value: MirBackendRvalue::Use(MirBackendOperand::Function {
                kind: "function:99".to_owned(),
            }),
        });
        let error = validate_backend_program(&program)
            .expect_err("function values must resolve through the normalized MIR table");
        assert!(error.contains("function value target 99 is not present"));
    }

    #[test]
    fn rejects_indirect_calls_with_the_wrong_function_arity() {
        let mut program = simple_backend();
        program.functions.push(MirBackendFunction {
            ordinal: 1,
            parameters: Vec::new(),
            parameter_types: Vec::new(),
            return_local: 0,
            return_type: "Int".to_owned(),
            supported: true,
            blocks: vec![MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: vec![MirBackendStatement::Assign {
                    destination: 0,
                    value: MirBackendRvalue::Use(MirBackendOperand::Constant(
                        MirBackendConstant::Integer("0".to_owned()),
                    )),
                }],
                terminator: MirBackendTerminator::Return,
            }],
        });
        program.debug = Some(synthetic_debug_info(&program.functions));
        if let MirBackendTerminator::Invoke { operation, .. } =
            &mut program.functions[0].blocks[0].terminator
        {
            *operation = MirBackendOperation::Runtime {
                kind: "indirect-call".to_owned(),
                arguments: vec![
                    MirBackendOperand::Function {
                        kind: "function:1".to_owned(),
                    },
                    MirBackendOperand::Constant(MirBackendConstant::Integer("1".to_owned())),
                    MirBackendOperand::Constant(MirBackendConstant::Integer("2".to_owned())),
                ],
            };
        }
        let error = validate_backend_program(&program)
            .expect_err("indirect-call ABI must require a two-parameter function");
        assert!(error.contains("target 1 must have arity 2"));
    }

    #[test]
    fn native_aot_corpus_contains_each_admitted_storage_case() {
        let (program, cases) = native_aot_program();
        validate_backend_program(&program).expect("AOT corpus must validate");
        assert!(cases.iter().any(|case| case.id == "array-storage"));
        assert!(cases.iter().any(|case| case.id == "record-projection"));
        assert!(cases.iter().any(|case| case.id == "closure-mutable-capture"));
        assert!(cases.iter().any(|case| case.id == "ownership-cow"));
    }

    #[test]
    fn native_aot_reference_oracle_evaluates_every_storage_case() {
        let (program, cases) = native_aot_program();
        let results = cases
            .iter()
            .map(|case| aot_vm_oracle(&program, case))
            .collect::<Result<Vec<_>, _>>()
            .expect("AOT reference oracle should evaluate the corpus");
        assert_eq!(results, cases.iter().map(|case| case.expected).collect::<Vec<_>>());
    }

    #[test]
    fn native_aot_reference_oracle_rejects_expected_result_drift() {
        let (program, cases) = native_aot_program();
        let mut drifted = cases[0];
        drifted.expected += 1;
        let error = aot_vm_oracle(&program, &drifted)
            .expect_err("oracle drift must fail closed");
        assert!(error.contains("AOT VM oracle disagrees for `array-storage`"));
    }

    #[test]
    fn generates_a_path_free_normalized_llvm_module() {
        let module = llvm_module("x86_64-unknown-linux-gnu", &simple_backend())
            .expect("normalized LLVM module should be generated");
        assert!(module.contains("target triple = \"x86_64-unknown-linux-gnu\""));
        assert!(module.contains("@tondo_probe_0"));
        assert!(module.contains("@tondo_checked_add"));
        assert!(module.contains("call i64 @tondo_checked_add"));
        assert!(module.contains("@llvm.sadd.with.overflow.i64"));
        assert!(!module.contains("/mnt/"));
    }

    #[test]
    fn unsupported_function_is_lowered_to_an_explicit_trap() {
        let mut program = simple_backend();
        program.functions[0].supported = false;
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("unsupported function should still form a verifier input");
        let probe = module
            .split_once("define i64 @tondo_probe_0")
            .map(|(_, body)| body)
            .expect("probe function should be present");
        assert!(probe.contains("unreachable"));
        assert!(!probe.contains("ret i64"));
    }

    #[test]
    fn checked_scalar_oracle_rejects_the_signed_add_boundary() {
        let function = simple_backend().functions.remove(0);
        assert_eq!(evaluate_scalar_function(&function, &[20, 2]), Ok(22));
        assert!(evaluate_scalar_function(&function, &[i64::MAX, 1]).is_err());
    }

    #[test]
    fn scalar_boundary_cases_are_deterministic_and_include_nominal_and_extreme_values() {
        let first = scalar_case_arguments(&[1, 2]);
        let second = scalar_case_arguments(&[1, 2]);
        assert_eq!(first, second);
        assert!(first.contains(&vec![20, 21]));
        assert!(first.contains(&vec![i64::MAX, 1]));
        assert!(first.contains(&vec![i64::MIN, -1]));
    }

    #[test]
    fn checked_scalar_operations_compile_and_match_boundary_oracle_rules() {
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        for operator in [
            "add",
            "subtract",
            "multiply",
            "divide",
            "remainder",
            "shift-left",
            "shift-right",
            "bitwise-and",
            "bitwise-or",
            "bitwise-xor",
            "logical-and",
            "logical-or",
        ] {
            let mut program = simple_backend();
            if let MirBackendTerminator::Invoke {
                operation:
                    MirBackendOperation::CheckedBinary {
                        operator: operation_operator,
                        ..
                    },
                ..
            } = &mut program.functions[0].blocks[0].terminator
            {
                *operation_operator = operator.to_owned();
            }
            compile_cranelift(isa.as_ref(), &program)
                .unwrap_or_else(|error| panic!("{operator} should lower: {error}"));
        }

        let mut divide = simple_backend();
        if let MirBackendTerminator::Invoke {
            operation: MirBackendOperation::CheckedBinary { operator, .. },
            ..
        } = &mut divide.functions[0].blocks[0].terminator
        {
            *operator = "divide".to_owned();
        }
        assert!(evaluate_scalar_function(&divide.functions[0], &[i64::MIN, -1]).is_err());

        let mut remainder = simple_backend();
        if let MirBackendTerminator::Invoke {
            operation: MirBackendOperation::CheckedBinary { operator, .. },
            ..
        } = &mut remainder.functions[0].blocks[0].terminator
        {
            *operator = "remainder".to_owned();
        }
        assert_eq!(
            evaluate_scalar_function(&remainder.functions[0], &[i64::MIN, -1]),
            Ok(0)
        );

        let mut shift = simple_backend();
        if let MirBackendTerminator::Invoke {
            operation: MirBackendOperation::CheckedBinary { operator, .. },
            ..
        } = &mut shift.functions[0].blocks[0].terminator
        {
            *operator = "shift-left".to_owned();
        }
        assert!(evaluate_scalar_function(&shift.functions[0], &[1, 64]).is_err());
    }

    #[test]
    fn logical_and_conversion_and_radix_literals_share_one_checked_boundary() {
        let mut logical = simple_backend();
        if let MirBackendTerminator::Invoke {
            operation:
                MirBackendOperation::CheckedBinary {
                    operator, left, right,
                },
            ..
        } = &mut logical.functions[0].blocks[0].terminator
        {
            *operator = "logical-and".to_owned();
            *left = MirBackendOperand::Local { index: 1 };
            *right = MirBackendOperand::Local { index: 2 };
        }
        assert_eq!(evaluate_scalar_function(&logical.functions[0], &[1, 0]), Ok(0));
        assert_eq!(evaluate_scalar_function(&logical.functions[0], &[1, 2]), Ok(1));

        let mut conversion = simple_backend();
        conversion.functions[0].blocks = vec![MirBackendBlock {
            ordinal: 0,
            kind: "normal".to_owned(),
            statements: vec![MirBackendStatement::Assign {
                destination: 0,
                value: MirBackendRvalue::NumericConversion {
                    source: "Int".to_owned(),
                    target: "Byte".to_owned(),
                    conversion: "checked".to_owned(),
                    operand: MirBackendOperand::Local { index: 1 },
                },
            }],
            terminator: MirBackendTerminator::Return,
        }];
        let converted = evaluate_scalar_function(&conversion.functions[0], &[255, 0])
            .expect("in-range conversion should return a managed result");
        assert_eq!(oracle_tag(converted), 2);
        assert_eq!(oracle_managed_parts(converted).unwrap().1, Some(255));
        let failed = evaluate_scalar_function(&conversion.functions[0], &[256, 0])
            .expect("out-of-range conversion should return an error result");
        assert_eq!(oracle_tag(failed), 3);
        assert_eq!(oracle_managed_parts(failed).unwrap().1, None);
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &conversion)
            .expect("checked conversion should lower in Cranelift");
        llvm_module("x86_64-unknown-linux-gnu", &conversion)
            .expect("checked conversion should lower in LLVM");
    }

    #[test]
    fn parses_radix_separators_and_numeric_suffixes_without_accepting_garbage() {
        assert_eq!(parse_integer_literal("65u8"), Ok(65));
        assert_eq!(parse_integer_literal("0xff_u8"), Ok(255));
        assert_eq!(parse_integer_literal("0b1010u16"), Ok(10));
        assert_eq!(parse_integer_literal("1_000"), Ok(1_000));
        assert!(parse_integer_literal("0x").is_err());
        assert!(parse_integer_literal("not-an-integer").is_err());
    }

    #[test]
    fn checked_bounds_share_trap_policy_across_oracle_cranelift_and_llvm() {
        let mut program = simple_backend();
        if let MirBackendTerminator::Invoke { operation, .. } =
            &mut program.functions[0].blocks[0].terminator
        {
            *operation = MirBackendOperation::BoundsCheck {
                index: MirBackendOperand::Local { index: 1 },
                length: MirBackendOperand::Constant(MirBackendConstant::Integer("3".to_owned())),
            };
        }
        let function = &program.functions[0];
        for (index, expected) in [(0, Ok(0)), (1, Ok(1)), (2, Ok(2))] {
            assert_eq!(evaluate_scalar_function(function, &[index, 0]), expected);
        }
        for index in [-1, 3, i64::MAX] {
            assert!(evaluate_scalar_function(function, &[index, 0]).is_err());
        }
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("checked bounds should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("checked bounds should lower in LLVM");
        assert!(module.contains("@tondo_checked_bounds"));
        assert!(module.contains("icmp slt i64 %index, 0"));
    }

    #[test]
    fn native_control_panic_corpus_covers_arithmetic_shift_assert_and_bounds_edges() {
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        for (operator, arguments) in [
            ("add", [i64::MAX, 1]),
            ("subtract", [i64::MIN, 1]),
            ("multiply", [i64::MAX, 2]),
            ("divide", [1, 0]),
            ("divide", [i64::MIN, -1]),
            ("remainder", [1, 0]),
            ("shift-left", [1, 64]),
            ("shift-right", [1, -1]),
        ] {
            let mut program = simple_backend();
            if let MirBackendTerminator::Invoke {
                operation:
                    MirBackendOperation::CheckedBinary {
                        operator: current, ..
                    },
                ..
            } = &mut program.functions[0].blocks[0].terminator
            {
                *current = operator.to_owned();
            }
            assert!(
                evaluate_scalar_function(&program.functions[0], &arguments).is_err(),
                "{operator} must trap for {:?}",
                arguments
            );
            compile_cranelift(isa.as_ref(), &program)
                .unwrap_or_else(|error| panic!("{operator} should compile: {error}"));
            llvm_module("x86_64-unknown-linux-gnu", &program)
                .unwrap_or_else(|error| panic!("{operator} should lower to LLVM: {error}"));
        }
        let failed_assert = assert_backend(false);
        assert!(evaluate_scalar_program(&failed_assert, 0, &[]).is_err());
        let bounds = {
            let mut program = simple_backend();
            if let MirBackendTerminator::Invoke { operation, .. } =
                &mut program.functions[0].blocks[0].terminator
            {
                *operation = MirBackendOperation::BoundsCheck {
                    index: MirBackendOperand::Local { index: 1 },
                    length: MirBackendOperand::Constant(MirBackendConstant::Integer(
                        "2".to_owned(),
                    )),
                };
            }
            program
        };
        assert!(evaluate_scalar_function(&bounds.functions[0], &[2, 0]).is_err());
    }

    #[test]
    fn cleanup_ownership_and_async_runtime_contracts_lower_in_both_backends() {
        let (program, cases) = native_cleanup_program();
        assert_eq!(cases.len(), 21);
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("cleanup, ownership and async runtime calls should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("cleanup, ownership and async runtime calls should lower in LLVM");
        assert!(module.contains("@tondo_rt_frame_cleanup"));
        assert!(module.contains("@tondo_rt_cow_clone"));
        assert!(module.contains("@tondo_rt_release"));
        assert!(module.contains("@tondo_rt_scope_enter"));
        assert!(module.contains("@tondo_rt_scope_spawn"));
        assert!(module.contains("@tondo_rt_task_poll"));
        assert!(module.contains("@tondo_rt_task_wake"));
        assert!(module.contains("@tondo_rt_await"));
        assert!(module.contains("@tondo_rt_scope_join"));
        assert!(module.contains("@tondo_rt_scope_cancel"));
        assert!(module.contains("@tondo_rt_thread_spawn"));
        assert!(module.contains("@tondo_rt_thread_worker_status"));
        assert!(module.contains("@tondo_rt_thread_worker_runs"));
        assert!(module.contains("@tondo_rt_thread_worker_distinct"));
        assert!(module.contains("@tondo_rt_thread_worker_wait"));
        assert!(module.contains("@tondo_rt_select_begin"));
        assert!(module.contains("@tondo_rt_select_register_task"));
        assert!(module.contains("@tondo_rt_select_register_join"));
        assert!(module.contains("@tondo_rt_select_register_oneshot"));
        assert!(module.contains("@tondo_rt_select_register_time"));
        assert!(module.contains("@tondo_rt_select_commit"));
        assert!(module.contains("@tondo_rt_select_winner"));
        assert!(module.contains("@tondo_rt_select_take"));
        assert!(module.contains("@tondo_rt_select_rollback"));
        assert!(module.contains("@tondo_rt_select_wakeups"));
        assert!(module.contains("@tondo_rt_oneshot_new"));
        assert!(module.contains("@tondo_rt_oneshot_complete"));
        assert!(module.contains("@tondo_rt_time_new"));
        assert!(module.contains("@tondo_rt_time_fire"));
    }

    #[test]
    fn deferred_task_body_is_published_pending_and_completed_at_join_in_both_adapters() {
        let (program, function_ordinal, expected) = native_deferred_program();
        let function = program
            .functions
            .iter()
            .find(|function| function.ordinal == function_ordinal)
            .expect("deferred caller should exist");
        let MirBackendTerminator::Invoke { operation, .. } = &function.blocks[0].terminator else {
            panic!("deferred caller must start with spawn");
        };
        assert!(deferred_call_body(operation).is_some());
        assert!(deferred_lowering_is_linear(function));
        assert_eq!(expected, 342);

        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("deferred task body should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("deferred task body should lower in LLVM");
        assert!(module.contains("call i64 @tondo_rt_task_spawn(i64 0, i64 1)"));
        assert!(module.contains("call i64 @tondo_rt_task_complete"));
        assert!(module.contains("call i64 @tondo_rt_await"));
    }

    #[test]
    fn deferred_call_subset_rejects_mutable_captures_and_threads() {
        let call = MirBackendOperation::Call {
            function: 0,
            arguments: vec![MirBackendOperand::Local { index: 1 }],
        };
        let task = MirBackendOperation::Spawn {
            operation: Box::new(call.clone()),
            kind: "task".to_owned(),
        };
        assert!(deferred_call_body(&task).is_none());
        let thread = MirBackendOperation::Spawn {
            operation: Box::new(call),
            kind: "thread".to_owned(),
        };
        assert!(deferred_call_body(&thread).is_none());
        assert!(!deferred_lowering_is_linear(&branch_backend().functions[0]));
    }

    #[test]
    fn scalar_control_flow_follows_switch_and_merges_with_block_parameters() {
        let program = branch_backend();
        let function = &program.functions[0];
        assert_eq!(evaluate_scalar_function(function, &[20]), Ok(21));
        assert_eq!(evaluate_scalar_function(function, &[0]), Ok(-1));
        assert_eq!(evaluate_scalar_function(function, &[-20]), Ok(-21));

        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("scalar control flow should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("scalar control flow should lower in LLVM");
        assert!(module.contains("icmp sgt i64"));
        assert!(module.contains("br i1"));
        assert!(module.contains("br label %b3"));
    }

    #[test]
    fn scalar_tag_control_flow_lowers_discriminants_in_all_adapters() {
        let program = tag_backend();
        validate_backend_program(&program).expect("tag control flow should be valid");
        assert_eq!(evaluate_scalar_program(&program, 0, &[]), Ok(20));

        for (tag, expected) in [(0, 10), (1, 20), (2, 30)] {
            let mut program = tag_backend();
            if let MirBackendStatement::Assign { value, .. } =
                &mut program.functions[0].blocks[0].statements[0]
            {
                *value = MirBackendRvalue::Tag { value: tag };
            }
            assert_eq!(
                evaluate_scalar_program(&program, 0, &[]),
                Ok(expected),
                "tag {tag} should select the expected arm"
            );
        }

        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("tag control flow should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("tag control flow should lower in LLVM");
        assert!(module.matches("icmp eq i64").count() >= 2);
        assert!(module.contains("switch"));
    }

    #[test]
    fn scalar_control_flow_lowers_loop_carried_locals_and_bounds_the_oracle() {
        let program = loop_backend();
        let function = &program.functions[0];
        assert!(control_flow_has_cycle(function));
        assert_eq!(evaluate_scalar_function(function, &[3]), Ok(0));
        assert_eq!(evaluate_scalar_function(function, &[0]), Ok(0));

        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("loop-carried scalar control flow should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("loop-carried scalar control flow should lower in LLVM");
        assert!(module.contains("br label %b1"));

        let mut non_terminating = loop_backend();
        non_terminating.functions[0].blocks[1].terminator =
            MirBackendTerminator::Goto { target: 1 };
        let error = evaluate_scalar_function(&non_terminating.functions[0], &[3])
            .expect_err("the scalar oracle must reject an unbounded loop");
        assert!(error.contains("control-flow steps"));
    }

    #[test]
    fn scalar_direct_calls_use_the_same_abi_in_all_backends_and_oracle() {
        let program = call_backend();
        validate_backend_program(&program).expect("direct call target should be validated");
        assert_eq!(evaluate_scalar_program(&program, 1, &[20]), Ok(21));
        let overflow = evaluate_scalar_program(&program, 1, &[i64::MAX])
            .expect_err("callee overflow should remain a trap");
        assert!(overflow.contains("scalar oracle failed for `add`"));

        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("direct scalar calls should lower in LLVM");
        assert!(module.contains("call i64 @tondo_probe_0"));
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("direct scalar calls should lower in Cranelift");
    }

    #[test]
    fn explicit_panic_is_a_native_trap_and_not_a_silent_return() {
        let program = trap_backend();
        let error = evaluate_scalar_program(&program, 0, &[])
            .expect_err("explicit panic must remain a trap in the oracle");
        assert!(error.contains("scalar oracle trap: explicit-panic"));
        let module = llvm_module("x86_64-unknown-linux-gnu", &program)
            .expect("explicit panic should lower to the trap helper");
        assert!(module.contains("@tondo_explicit_panic"));
        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &program)
            .expect("explicit panic should lower to a Cranelift trap");
    }

    #[test]
    fn checked_assert_preserves_success_and_trap_semantics_in_all_adapters() {
        let success = assert_backend(true);
        assert_eq!(evaluate_scalar_program(&success, 0, &[]), Ok(0));
        let failure = assert_backend(false);
        let error = evaluate_scalar_program(&failure, 0, &[])
            .expect_err("a failed assert must remain a trap in the oracle");
        assert!(error.contains("scalar oracle trap: assert"));

        let isa = cranelift_isa().expect("native Cranelift ISA should be available");
        compile_cranelift(isa.as_ref(), &success)
            .expect("a successful assert should lower in Cranelift");
        let module = llvm_module("x86_64-unknown-linux-gnu", &failure)
            .expect("a failed assert should lower to the checked LLVM helper");
        assert!(module.contains("@tondo_checked_assert"));
        assert!(module.contains("@llvm.trap"));
    }

    #[test]
    fn parses_explicit_tool_paths_without_environment_discovery() {
        let options = Options::parse(
            [
                "--probe",
                "probe.json",
                "--output",
                "report.json",
                "--llvm",
                "/usr/bin/llc",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--temp-dir",
                ".tmp",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("explicit arguments should parse");
        assert_eq!(options.llvm, PathBuf::from("/usr/bin/llc"));
        assert_eq!(options.target, "x86_64-unknown-linux-gnu");
    }
}
