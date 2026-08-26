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
use std::path::{Path, PathBuf};
use std::process::Command;
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
    functions: Vec<MirBackendFunction>,
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
    Unsupported {
        kind: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
enum MirBackendOperand {
    Constant(MirBackendConstant),
    Local { index: u32 },
    Borrow { index: u32 },
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
    native_runs: Vec<NativeRunReport>,
    native_managed_runs: Vec<NativeManagedRunReport>,
    native_runtime_runs: Vec<NativeRuntimeRunReport>,
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
struct NativeRuntimeRunReport {
    case: String,
    function_ordinal: u32,
    expected_result: i64,
    cranelift: &'static str,
    llvm: &'static str,
}

#[derive(Debug, Clone)]
struct RuntimeContractCase {
    name: &'static str,
    function_ordinal: u32,
    expected_result: i64,
}

#[derive(Debug)]
struct Options {
    probe: PathBuf,
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
    let mut native_runtime_runs = Vec::new();

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
        native_runtime_runs = run_native_runtime_probe(
            &options.llvm,
            cc,
            &options.target,
            &options.temp_dir,
        )?;
    }

    let report = EvaluationReport {
        format: "tondo-native-evaluation-candidates/1",
        phase: "NATIVE-001",
        status: "passed",
        target: options.target,
        adapter: AdapterReport {
            format: "tondo-mir-backend/1",
            supported_subset: "scalar-int-managed-result-checked-bounds-arithmetic-asserts-control-flow-tag-dispatch-direct-calls-host-calls-and-traps",
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
                "scalar-managed-and-runtime-native-executable-vs-vm-and-contract"
            },
        },
        native_runs,
        native_managed_runs,
        native_runtime_runs,
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
    let function_arities = program
        .functions
        .iter()
        .map(|function| (function.ordinal, function.parameters.len()))
        .collect::<BTreeMap<_, _>>();
    for function in &program.functions {
        if function.supported {
            for block in function.blocks.iter().filter(|block| block.kind == "cleanup") {
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
            let MirBackendTerminator::Invoke { operation, .. } = &block.terminator else {
                continue;
            };
            if let MirBackendOperation::Call {
                function: target,
                arguments,
            } = operation
            {
                if !function_ordinals.contains(target) {
                    return Err(format!(
                        "normalized MIR call target {target} is not present"
                    ));
                }
                if arguments.len() != function_arities[target] {
                    return Err(format!(
                        "normalized MIR call target {target} expects {} arguments, got {}",
                        function_arities[target],
                        arguments.len()
                    ));
                }
            }
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
    task_poll: FuncRef,
    task_wake: FuncRef,
    task_cancel: FuncRef,
    task_take: FuncRef,
    scope_cancel: FuncRef,
    scope_join: FuncRef,
    await_task: FuncRef,
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
        ("tondo_rt_task_poll", 1),
        ("tondo_rt_task_wake", 1),
        ("tondo_rt_task_cancel", 1),
        ("tondo_rt_task_take", 1),
        ("tondo_rt_scope_cancel", 1),
        ("tondo_rt_scope_join", 2),
        ("tondo_rt_await", 1),
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
        task_poll: get("tondo_rt_task_poll")?,
        task_wake: get("tondo_rt_task_wake")?,
        task_cancel: get("tondo_rt_task_cancel")?,
        task_take: get("tondo_rt_task_take")?,
        scope_cancel: get("tondo_rt_scope_cancel")?,
        scope_join: get("tondo_rt_scope_join")?,
        await_task: get("tondo_rt_await")?,
    })
}

fn aggregate_tag(kind: &str) -> Result<u32, String> {
    match kind {
        "option-none" => Ok(0),
        "option-some" => Ok(1),
        "result-ok" => Ok(2),
        "result-err" => Ok(3),
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
    match base {
        "retain" | "retain-value" => Ok(RuntimeCall { function: runtime.retain, arity: 1 }),
        "release" | "release-value" => Ok(RuntimeCall { function: runtime.release, arity: 1 }),
        "cow-clone" => Ok(RuntimeCall { function: runtime.cow_clone, arity: 1 }),
        "frame-enter" => Ok(RuntimeCall { function: runtime.frame_enter, arity: 0 }),
        "frame-publish-root" => Ok(RuntimeCall { function: runtime.frame_publish_root, arity: 2 }),
        "register-defer" => Ok(RuntimeCall { function: runtime.frame_register_defer, arity: 2 }),
        "disarm-defer" => Ok(RuntimeCall { function: runtime.frame_disarm_defer, arity: 2 }),
        "frame-cleanup" => Ok(RuntimeCall { function: runtime.frame_cleanup, arity: 2 }),
        "frame-leave" => Ok(RuntimeCall { function: runtime.frame_leave, arity: 2 }),
        "scope-enter" => Ok(RuntimeCall { function: runtime.scope_enter, arity: 0 }),
        "scope-spawn" => Ok(RuntimeCall { function: runtime.scope_spawn, arity: 3 }),
        "task-spawn" => Ok(RuntimeCall { function: runtime.task_spawn, arity: 2 }),
        "task-poll" => Ok(RuntimeCall { function: runtime.task_poll, arity: 1 }),
        "task-wake" => Ok(RuntimeCall { function: runtime.task_wake, arity: 1 }),
        "task-cancel" => Ok(RuntimeCall { function: runtime.task_cancel, arity: 1 }),
        "task-take" => Ok(RuntimeCall { function: runtime.task_take, arity: 1 }),
        "scope-cancel" => Ok(RuntimeCall { function: runtime.scope_cancel, arity: 1 }),
        "scope-join" => Ok(RuntimeCall { function: runtime.scope_join, arity: 2 }),
        "await" => Ok(RuntimeCall { function: runtime.await_task, arity: 1 }),
        other => Err(format!("native runtime operation is not supported: {other}")),
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
    if arguments.len() != call.arity {
        return Err(format!(
            "native runtime operation `{kind}` expects {} arguments, got {}",
            call.arity,
            arguments.len()
        ));
    }
    let arguments = arguments
        .iter()
        .map(|argument| lower_operand_cranelift(builder, argument, locals))
        .collect::<Result<Vec<_>, _>>()?;
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
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => {
            locals.insert(*index);
        }
        MirBackendOperand::Constant(_) | MirBackendOperand::Unsupported { .. } => {}
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
            next.extend(outgoing.into_iter().filter(|local| !definitions.contains(local)));
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
            locals
                .get(local)
                .copied()
                .ok_or_else(|| format!("MIR local {local} is not available on edge to block {target}"))
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
        MirBackendRvalue::Use(operand) => lower_operand_cranelift(builder, operand, locals),
        MirBackendRvalue::Tag { value } => Ok(builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I64, i64::from(*value))),
        MirBackendRvalue::Aggregate { kind, values } => {
            let tag = aggregate_tag(kind)?;
            let payload = values
                .first()
                .map(|operand| lower_operand_cranelift(builder, operand, locals))
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(cranelift_codegen::ir::types::I64, 0));
            let has_payload = builder.ins().iconst(
                cranelift_codegen::ir::types::I64,
                i64::from(!values.is_empty()),
            );
            let tag = builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, i64::from(tag));
            let call = builder.ins().call(runtime.result_new, &[tag, payload, has_payload]);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift result constructor did not return a handle".to_owned())
        }
        MirBackendRvalue::Prefix { operator, operand } => {
            let operand = lower_operand_cranelift(builder, operand, locals)?;
            match operator.as_str() {
                "negate" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
                    builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
                    Ok(value)
                }
                "bitwise-not" => Ok(builder.ins().bxor_imm(operand, -1)),
                other => Err(format!("Cranelift scalar prefix is not supported: {other}")),
            }
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => lower_checked_binary_cranelift(builder, operator, left, right, locals),
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
            let operand = lower_operand_cranelift(builder, operand, locals)?;
            match operator.as_str() {
                "negate" => {
                    let zero = builder.ins().iconst(cranelift_codegen::ir::types::I64, 0);
                    let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
                    builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
                    Ok(value)
                }
                "bitwise-not" => Ok(builder.ins().bxor_imm(operand, -1)),
                other => Err(format!("Cranelift scalar prefix is not supported: {other}")),
            }
        }
        MirBackendOperation::CheckedBinary {
            operator,
            left,
            right,
        } => lower_checked_binary_cranelift(builder, operator, left, right, locals),
        MirBackendOperation::BoundsCheck { index, length } => {
            let index = lower_operand_cranelift(builder, index, locals)?;
            let length = lower_operand_cranelift(builder, length, locals)?;
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
                .map(|argument| lower_operand_cranelift(builder, argument, locals))
                .collect::<Result<Vec<_>, _>>()?;
            let call = builder.ins().call(function_ref, &arguments);
            builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| "Cranelift scalar call did not return a value".to_owned())
        }
        MirBackendOperation::HostCall { kind, arguments } => {
            let kind_id = host_call_kind(kind)?;
            let argument = arguments
                .first()
                .map(|argument| lower_operand_cranelift(builder, argument, locals))
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(cranelift_codegen::ir::types::I64, 0));
            let kind_value = builder
                .ins()
                .iconst(cranelift_codegen::ir::types::I64, i64::from(kind_id));
            let call = builder.ins().call(runtime.host_call, &[kind_value, argument]);
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
            let condition = lower_operand_cranelift(builder, condition, locals)?;
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

fn lower_operand_cranelift(
    builder: &mut FunctionBuilder<'_>,
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, Value>,
) -> Result<Value, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => value
            .parse::<i64>()
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
        MirBackendOperand::Constant(other) => {
            let kind = match other {
                MirBackendConstant::Unit => "unit".to_owned(),
                MirBackendConstant::Float(value)
                | MirBackendConstant::Char(value) => value.clone(),
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
        "less" => {
            let value = builder.ins().icmp(IntCC::SignedLessThan, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
        }
        "less-equal" => {
            let value = builder
                .ins()
                .icmp(IntCC::SignedLessThanOrEqual, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
        }
        "greater" => {
            let value = builder
                .ins()
                .icmp(IntCC::SignedGreaterThan, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
        }
        "greater-equal" => {
            let value = builder
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
        }
        "equal" => {
            let value = builder.ins().icmp(IntCC::Equal, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
        }
        "not-equal" => {
            let value = builder.ins().icmp(IntCC::NotEqual, left, right);
            builder.ins().uextend(cranelift_codegen::ir::types::I64, value)
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
                        let value = locals.get(&function.return_local).copied().unwrap_or_else(|| {
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
                        let condition = lower_operand_cranelift(&mut builder, condition, &locals)?;
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
                        let value = lower_operand_cranelift(&mut builder, value, &locals)?;
                        let tag_call = builder.ins().call(runtime.result_tag, &[value]);
                        let value = builder
                            .inst_results(tag_call)
                            .first()
                            .copied()
                            .ok_or_else(|| "Cranelift tag helper did not return a value".to_owned())?;
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
                                let matches = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    value,
                                    i64::from(*tag),
                                );
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
                        let value =
                            lower_operation_cranelift(
                                &mut builder,
                                operation,
                                &locals,
                                &calls,
                                trap,
                                &runtime,
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
    let cranelift_object =
        temp_dir.join(format!("{}_managed.cranelift.o", safe_stem(&fixture.fixture)));
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
        let stem = format!("native_runtime_{}", case.name);
        let cranelift_source = temp_dir.join(format!("{stem}.cranelift.c"));
        fs::write(
            &cranelift_source,
            runtime_contract_c_runner_source(case.function_ordinal, case.expected_result),
        )
        .map_err(|error| format!("cannot write runtime Cranelift runner: {error}"))?;
        let cranelift_binary = temp_dir.join(format!("{stem}.cranelift.bin"));
        link_native_runner(cc, &cranelift_source, &object, &cranelift_binary)?;
        run_native_binary(&cranelift_binary, "Cranelift runtime", false)?;

        let llvm_ir = temp_dir.join(format!("{stem}.llvm.ll"));
        let llvm_object = temp_dir.join(format!("{stem}.llvm.o"));
        fs::write(
            &llvm_ir,
            llvm_module_with_runner(
                target,
                &program,
                program
                    .functions
                    .iter()
                    .find(|function| function.ordinal == case.function_ordinal)
                    .ok_or_else(|| format!("runtime function {} is missing", case.function_ordinal))?,
                &[],
                Some(case.expected_result),
            )?,
        )
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
            expected_result: case.expected_result,
            cranelift: "passed",
            llvm: "passed",
        });
    }
    Ok(reports)
}

fn native_cleanup_program() -> (MirBackendProgram, Vec<RuntimeContractCase>) {
    let runtime_operand = |index| MirBackendOperand::Local { index };
    let constant = |value: &str| {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value.to_owned()))
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
    (
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![cleanup_function, abort_function],
        },
        vec![
            RuntimeContractCase {
                name: "cleanup-exactly-once",
                function_ordinal: 100,
                expected_result: 5,
            },
            RuntimeContractCase {
                name: "cleanup-abort",
                function_ordinal: 101,
                expected_result: 0,
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
                other => Err(format!("scalar oracle prefix is not supported: {other}")),
            }
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => evaluate_binary(operator, left, right, locals),
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
        MirBackendOperation::Trap { kind } => {
            Err(format!("scalar oracle trap: {kind}"))
        }
        MirBackendOperation::Marker { kind } => {
            Err(format!("scalar oracle operation is not supported: {kind}"))
        }
    }
}

fn encode_oracle_managed(tag: i64, payload: i64, has_payload: bool) -> i64 {
    let encoded = ORACLE_MANAGED_BIT
        | ((tag as u64 & ORACLE_TAG_MASK) << ORACLE_TAG_SHIFT)
        | if has_payload {
            payload as u64 & ORACLE_PAYLOAD_MASK
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
    let payload = (tag != 0).then_some(encoded & ORACLE_PAYLOAD_MASK);
    Ok((tag, payload))
}

fn string_payload(value: &str) -> i64 {
    (value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        hash.wrapping_mul(0x100000001b3).wrapping_add(u64::from(byte))
    }) & ((1_u64 << 56) - 1)) as i64
}

fn evaluate_operand(
    operand: &MirBackendOperand,
    locals: &BTreeMap<u32, i64>,
) -> Result<i64, String> {
    match operand {
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => value
            .parse::<i64>()
            .map_err(|error| format!("invalid scalar oracle integer `{value}`: {error}")),
        MirBackendOperand::Constant(MirBackendConstant::Bool(value)) => Ok(i64::from(*value)),
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index } => locals
            .get(index)
            .copied()
            .ok_or_else(|| format!("scalar oracle local {index} is not available")),
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
        "{}\nextern uint64_t tondo_probe_{}({params});\nint main(void) {{ uint64_t result = tondo_probe_{}({args}); return tondo_rt_result_tag(result) == UINT64_C({expected_tag}) && ({payload_check}) ? 0 : 91; }}\n",
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
#define T_MAX 4096u
#define F_MAX 256u
#define D_MAX 64u
#define S_MAX 64u
#define HBIT (UINT64_C(1) << 63)
typedef struct { uint64_t tag, payload, has_payload, strong, kind, state, value; } t_entry;
typedef struct { uint64_t terminal, root_count, defer_count; uint64_t roots[D_MAX]; uint64_t defers[D_MAX]; } t_frame;
typedef struct { uint64_t handle, state, value, scope; } t_task;
static t_entry t_objects[T_MAX];
static t_frame t_frames[F_MAX];
static t_task t_tasks[S_MAX];
static uint64_t t_next = 1, t_next_frame = 1, t_last = 0;
static uint64_t t_alloc(uint64_t kind, uint64_t tag, uint64_t payload, uint64_t has_payload) {
    if (t_next >= T_MAX) { t_last = 8; return 0; }
    uint64_t id = t_next++;
    t_objects[id].kind = kind; t_objects[id].tag = tag; t_objects[id].payload = payload;
    t_objects[id].has_payload = has_payload; t_objects[id].strong = 1; t_objects[id].state = 0;
    return HBIT | id;
}
static uint64_t t_index(uint64_t handle) {
    if ((handle & HBIT) == 0) return 0;
    uint64_t id = handle & ~HBIT;
    return id < T_MAX && t_objects[id].kind != 0 ? id : 0;
}
static void t_reset(void) {
    for (uint64_t i = 0; i < T_MAX; ++i) t_objects[i].kind = 0;
    for (uint64_t i = 0; i < F_MAX; ++i) t_frames[i].terminal = 0;
    for (uint64_t i = 0; i < S_MAX; ++i) t_tasks[i].handle = 0;
    t_next = 1; t_next_frame = 1; t_last = 0;
}
uint64_t tondo_rt_result_new(uint64_t tag, uint64_t payload, uint64_t has_payload) {
    return tag <= 3 ? t_alloc(1, tag, payload, has_payload) : 0;
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
    uint64_t task = t_alloc(3, 0, 0, 0); uint64_t id = t_index(task); if (id == 0) return 0;
    t_objects[id].state = pending ? 0 : 1; t_objects[id].value = value; t_objects[id].payload = scope;
    return task;
}
uint64_t tondo_rt_task_spawn(uint64_t value, uint64_t pending) {
    uint64_t task = t_alloc(3, 0, 0, 0); uint64_t id = t_index(task); if (id == 0) return 0;
    t_objects[id].state = pending ? 0 : 1; t_objects[id].value = value; return task;
}
uint64_t tondo_rt_task_poll(uint64_t task) { uint64_t id = t_index(task); return id != 0 && t_objects[id].kind == 3 ? t_objects[id].state + 0 : 1; }
uint64_t tondo_rt_task_wake(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state >= 2) return 3; t_objects[id].state = 1; return 0; }
uint64_t tondo_rt_task_cancel(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state == 3) return 3; t_objects[id].state = 2; return 0; }
uint64_t tondo_rt_task_take(uint64_t task) { uint64_t id = t_index(task); if (id == 0 || t_objects[id].kind != 3 || t_objects[id].state != 1) { t_last = 6; return 0; } t_objects[id].state = 3; return t_objects[id].value; }
uint64_t tondo_rt_scope_cancel(uint64_t scope) { uint64_t id = t_index(scope); if (id == 0 || t_objects[id].kind != 2) return 1; t_objects[id].state = 1; return 0; }
uint64_t tondo_rt_scope_join(uint64_t scope, uint64_t task) { uint64_t sid = t_index(scope), tid = t_index(task); if (sid == 0 || tid == 0 || t_objects[sid].kind != 2 || t_objects[tid].kind != 3 || t_objects[tid].payload != scope) return 3; (void)tondo_rt_task_take(task); return 0; }
uint64_t tondo_rt_await(uint64_t task) { return tondo_rt_task_take(task); }
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
        writeln!(
            module,
            "  %payload_ok = icmp eq i64 %payload, {payload}"
        )
        .unwrap();
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

fn llvm_module(target: &str, program: &MirBackendProgram) -> Result<String, String> {
    let mut module = String::new();
    writeln!(module, "; tondo native evaluation normalized module").unwrap();
    writeln!(module, "target triple = \"{target}\"").unwrap();
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
                        let value_name =
                            llvm_rvalue(value, &slots, &mut module, &mut value_index)?;
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
                    writeln!(module, "  br label %{}", llvm_block_label(*target, entry_ordinal))
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
                    writeln!(
                        module,
                        "  {condition_value} = icmp ne i64 {condition}, 0"
                    )
                    .unwrap();
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
                    let value_name =
                        llvm_operation(operation, &slots, &mut module, &mut value_index)?;
                    if let Some(destination) = destination {
                        let slot = slots.get(destination).ok_or_else(|| {
                            format!("missing slot for destination local {destination}")
                        })?;
                        writeln!(module, "  store i64 {value_name}, ptr {slot}").unwrap();
                    }
                    writeln!(module, "  br label %{}", llvm_block_label(*target, entry_ordinal))
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
        "declare i64 @tondo_rt_task_poll(i64)",
        "declare i64 @tondo_rt_task_wake(i64)",
        "declare i64 @tondo_rt_task_cancel(i64)",
        "declare i64 @tondo_rt_task_take(i64)",
        "declare i64 @tondo_rt_scope_cancel(i64)",
        "declare i64 @tondo_rt_scope_join(i64, i64)",
        "declare i64 @tondo_rt_await(i64)",
    ] {
        writeln!(module, "{declaration}").unwrap();
    }
    writeln!(module, "define internal i64 @tondo_explicit_panic() {{").unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  call void @llvm.trap()").unwrap();
    writeln!(module, "  unreachable").unwrap();
    writeln!(module, "}}").unwrap();
    writeln!(module, "define internal i64 @tondo_checked_assert(i64 %condition) {{").unwrap();
    writeln!(module, "entry:").unwrap();
    writeln!(module, "  %valid = icmp ne i64 %condition, 0").unwrap();
    writeln!(module, "  br i1 %valid, label %assert_ok, label %assert_trap").unwrap();
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
}

fn llvm_checked_bounds_helper(module: &mut String) {
    writeln!(module, "define internal i64 @tondo_checked_bounds(i64 %index, i64 %length) {{")
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
                other => return Err(format!("LLVM scalar prefix is not supported: {other}")),
            }
            Ok(name)
        }
        MirBackendRvalue::Binary {
            operator,
            left,
            right,
        } => llvm_binary(operator, left, right, slots, module, value_index),
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
    let (function, arity) = match base {
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
        "task-poll" => ("tondo_rt_task_poll", 1),
        "task-wake" => ("tondo_rt_task_wake", 1),
        "task-cancel" => ("tondo_rt_task_cancel", 1),
        "task-take" => ("tondo_rt_task_take", 1),
        "scope-cancel" => ("tondo_rt_scope_cancel", 1),
        "scope-join" => ("tondo_rt_scope_join", 2),
        "await" => ("tondo_rt_await", 1),
        other => return Err(format!("native runtime operation is not supported: {other}")),
    };
    if arguments.len() != arity {
        return Err(format!(
            "native runtime operation `{kind}` expects {arity} arguments, got {}",
            arguments.len()
        ));
    }
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
        MirBackendOperand::Constant(MirBackendConstant::Integer(value)) => value
            .parse::<i64>()
            .map(|_| value.clone())
            .map_err(|error| format!("invalid scalar integer `{value}`: {error}")),
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
        MirBackendOperand::Constant(other) => {
            let kind = match other {
                MirBackendConstant::Unit => "unit".to_owned(),
                MirBackendConstant::Float(value)
                | MirBackendConstant::Char(value) => value.clone(),
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
                "--output" => output = Some(PathBuf::from(value()?)),
                "--llvm" => llvm = Some(PathBuf::from(value()?)),
                "--target" => target = Some(value()?),
                "--temp-dir" => temp_dir = Some(PathBuf::from(value()?)),
                "--cc" => cc = Some(PathBuf::from(value()?)),
                "--help" | "-h" => {
                    println!(
                        "usage: tondo-native-evaluation --probe FILE --output FILE --llvm ABSOLUTE --target TRIPLE --temp-dir DIR [--cc ABSOLUTE]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(Self {
            probe: probe.ok_or("--probe is required")?,
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

    fn simple_backend() -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
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
            }],
        }
    }

    fn branch_backend() -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
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
            }],
        }
    }

    fn tag_backend() -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
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
            }],
        }
    }

    fn loop_backend() -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
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
            }],
        }
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
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![callee, caller],
        }
    }

    fn trap_backend() -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
                ordinal: 0,
                parameters: vec![],
                parameter_types: Vec::new(),
                return_local: 0,
                return_type: "Int".to_owned(),
                supported: true,
                blocks: vec![MirBackendBlock {
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
                }, MirBackendBlock {
                    ordinal: 1,
                    kind: "normal".to_owned(),
                    statements: Vec::new(),
                    terminator: MirBackendTerminator::Return,
                }],
            }],
        }
    }

    fn assert_backend(condition: bool) -> MirBackendProgram {
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions: vec![MirBackendFunction {
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
                                condition: MirBackendOperand::Constant(
                                    MirBackendConstant::Bool(condition),
                                ),
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
            }],
        }
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
                operation: MirBackendOperation::CheckedBinary {
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
