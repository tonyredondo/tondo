use std::collections::BTreeSet;

use tondo_compiler::driver::{BuildTarget, CapabilityName, HostProfile};
use tondo_compiler::meta::{
    MetaContractError, MetaLimits, MetaOutputSpec, MetaRequest, MetaResponse, MetaSnapshot,
    MetaSourceMapEntry, MetaSpan,
};
use tondo_compiler::meta_vm::{MetaVmError, MetaVmLimits, MetaVmProgram};
use tondo_compiler::std_meta::{MetaRenderer, StdMetaPackage};
use tondo_vm::bytecode::{
    BytecodeFunctionId, BytecodeFunctionType, BytecodeProgram, BytecodeScalarType, BytecodeType,
    BytecodeTypeId, BytecodeTypeKind,
};
use tondo_vm::runtime::{
    RejectingHost, RuntimeValue, VmError, VmHost, VmTestNodeKind, VmTestNodeOutcome,
};

fn request(output_bytes: u64) -> MetaRequest {
    MetaRequest::new(
        MetaSnapshot::new(tondo_compiler::meta::MetaEnvironment::meta(), [], [], []).unwrap(),
        [],
        [MetaOutputSpec::new("generated/out.to", "generated.out").unwrap()],
        MetaLimits::new(10_000, 1024 * 1024, output_bytes).unwrap(),
    )
    .unwrap()
}

#[test]
fn public_companion_round_trips_mapped_source_canonically() {
    let source = "fn café() {}\n";
    let mapping = MetaSourceMapEntry::new(3, 8, MetaSpan::new(4, 20, 25).unwrap()).unwrap();
    let mut builder = request(1024).into_source_builder();
    builder
        .add_mapped_source(
            "generated/out.to",
            "generated.out",
            source.as_bytes(),
            [mapping],
        )
        .unwrap();
    let response = builder.finish().unwrap();
    let bytes = response.canonical_bytes().unwrap();
    let decoded = MetaResponse::decode(&bytes).unwrap();
    let output = decoded.output("generated/out.to").unwrap();
    assert_eq!(output.bytes(), source.as_bytes());
    assert_eq!(output.mappings(), &[mapping]);
    assert_eq!(output.mappings()[0].generated_start(), 3);
    assert_eq!(output.mappings()[0].generated_end(), 8);
    assert_eq!(
        output.mappings()[0].origin(),
        MetaSpan::new(4, 20, 25).unwrap()
    );
}

#[test]
fn source_maps_reject_overlap_utf8_splits_and_out_of_bounds() {
    let origin = MetaSpan::new(0, 0, 1).unwrap();
    assert!(matches!(
        MetaSourceMapEntry::new(2, 1, origin),
        Err(MetaContractError::InvalidSourceMap)
    ));
    for (source, mappings) in [
        ("é", vec![MetaSourceMapEntry::new(0, 1, origin).unwrap()]),
        (
            "abcd",
            vec![
                MetaSourceMapEntry::new(0, 2, origin).unwrap(),
                MetaSourceMapEntry::new(1, 3, origin).unwrap(),
            ],
        ),
        ("é", vec![MetaSourceMapEntry::new(0, 99, origin).unwrap()]),
    ] {
        let mut builder = request(1024).into_source_builder();
        assert!(matches!(
            builder.add_mapped_source("generated/out.to", "generated.out", source, mappings),
            Err(MetaContractError::InvalidSourceMap)
        ));
    }
}

#[test]
fn build_only_budgets_fail_without_partial_response() {
    let mut builder = request(3).into_source_builder();
    assert!(matches!(
        builder.add_source("generated/out.to", "generated.out", b"four"),
        Err(MetaContractError::OutputLimit { limit: 3 })
    ));
    assert!(matches!(
        builder.finish(),
        Err(MetaContractError::MissingOutput(path)) if path == "generated/out.to"
    ));
    assert!(MetaLimits::new(0, 1, 1).is_err());
    assert!(MetaLimits::new(1, 0, 1).is_err());
    assert!(MetaLimits::new(1, 1, 0).is_err());
}

#[test]
fn meta_target_admits_no_ambient_or_unsafe_surface() {
    let empty_program = BytecodeProgram {
        reflection: Default::default(),
        types: Vec::new(),
        nominals: Vec::new(),
        callables: Vec::new(),
        constants: Vec::new(),
        functions: Vec::new(),
    };
    for capability in [
        "filesystem",
        "environment",
        "process",
        "clock",
        "entropy",
        "network",
        "threads",
        "dynamic-linking",
        "console",
    ] {
        let capabilities = BTreeSet::from([CapabilityName::new(capability).unwrap()]);
        assert!(matches!(
            MetaVmProgram::load(
                &BuildTarget::tondo_meta(),
                HostProfile::Meta,
                &capabilities,
                empty_program.clone(),
                BytecodeFunctionId::new(0),
                MetaVmLimits::default(),
            ),
            Err(MetaVmError::Capability(actual)) if actual == capability
        ));
    }

    let unit = BytecodeTypeId::new(0);
    let unsafe_program = BytecodeProgram {
        reflection: Default::default(),
        types: vec![
            BytecodeType {
                name: "Unit".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
            },
            BytecodeType {
                name: "unsafe fn(): Unit".into(),
                kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                    is_async: false,
                    is_selectable: false,
                    is_unsafe: true,
                    parameters: Vec::new(),
                    variadic: None,
                    outcome: unit,
                }),
            },
        ],
        ..empty_program
    };
    assert!(matches!(
        MetaVmProgram::load(
            &BuildTarget::tondo_meta(),
            HostProfile::Meta,
            &BTreeSet::new(),
            unsafe_program,
            BytecodeFunctionId::new(0),
            MetaVmLimits::default(),
        ),
        Err(MetaVmError::ForbiddenType("unsafe function"))
    ));
}

#[test]
fn rejecting_host_defaults_are_closed_for_external_consumers() {
    let mut host = RejectingHost;
    assert!(matches!(
        host.start_async("work", &[]),
        Err(VmError::UnsupportedHostCall(name)) if name == "work"
    ));
    assert_eq!(host.poll_async(7).unwrap(), None);
    assert!(matches!(
        host.wait_async(&[]),
        Err(VmError::Invariant(message)) if message.contains("no calls")
    ));
    assert!(matches!(
        host.wait_async(&[7]),
        Err(VmError::UnsupportedHostCall(name)) if name == "async host call #7"
    ));
    host.set_execution_unit(7);
    host.cancel_async(7).unwrap();
    host.cleanup(&RuntimeValue::Unit).unwrap();
    assert!(matches!(
        host.begin_virtual_time(),
        Err(VmError::UnsupportedHostCall(name)) if name == "std.testing.withVirtualTime"
    ));
    assert!(matches!(
        host.finish_virtual_time(&RuntimeValue::Unit),
        Err(VmError::UnsupportedHostCall(name)) if name == "std.testing.withVirtualTime"
    ));
    assert!(!host.is_virtual_quiescence_call(7));
    assert!(matches!(
        host.begin_test_node(VmTestNodeKind::Leaf, "meta::leaf"),
        Err(VmError::UnsupportedHostCall(name)) if name == "test node `meta::leaf`"
    ));
    assert!(matches!(
        host.finish_test_node(
            VmTestNodeKind::Suite,
            "meta::suite",
            VmTestNodeOutcome::Passed,
        ),
        Err(VmError::UnsupportedHostCall(name)) if name == "test node `meta::suite`"
    ));
    assert!(matches!(
        host.begin_test_suite_cleanup(),
        Err(VmError::UnsupportedHostCall(name)) if name == "test suite cleanup"
    ));
}

#[test]
fn candidate_descriptor_and_rendering_are_reproducible() {
    let first = StdMetaPackage::load_candidate().unwrap();
    let second = StdMetaPackage::load_candidate().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(MetaRenderer::string("\nTondo🙂"), "\"\\nTondo🙂\"");
}

fn ordinary_provider(
    source: &str,
    entry: &str,
    kind: tondo_compiler::meta_vm::MetaEntryKind,
) -> tondo_compiler::meta_vm::MetaVmArtifact {
    use tondo_compiler::driver::{
        CompilationRequest, DiagnosticFormat, Operation, ResourceLimits, SourceForm,
    };
    use tondo_compiler::package::{Edition, PackageGraph};
    use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("owner:meta").unwrap(),
            ModulePath::new("companion").unwrap(),
            LogicalPath::new("src/companion.to").unwrap(),
            source.as_bytes(),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    let request = CompilationRequest::new(
        Operation::Check,
        Edition::V0_1,
        BuildTarget::tondo_meta(),
        HostProfile::Meta,
        BTreeSet::new(),
        DiagnosticFormat::Json,
        SourceForm::Module,
        ResourceLimits::default(),
        packages,
        sources,
        root,
    )
    .unwrap();
    tondo_compiler::meta_vm::MetaVmArtifact::compile_entry(request, entry, kind).unwrap()
}

fn ordinary_request() -> MetaRequest {
    use tondo_compiler::meta::{
        MetaDeclaration, MetaDeclarationKind, MetaEnvironment, MetaField, MetaInput, MetaModule,
        MetaRoot, MetaVisibility,
    };
    let snapshot = MetaSnapshot::new(
        MetaEnvironment::meta(),
        [MetaRoot::new("app", "model").unwrap()],
        [MetaModule::new("model", None::<String>).unwrap()],
        [MetaDeclaration::new(
            "Item",
            "model",
            MetaVisibility::Public,
            [],
            [],
            MetaSpan::new(0, 0, 4).unwrap(),
            None::<String>,
            MetaDeclarationKind::Record(vec![
                MetaField::new(
                    "value",
                    "Int",
                    MetaVisibility::Public,
                    0,
                    MetaSpan::new(0, 5, 10).unwrap(),
                    None::<String>,
                )
                .unwrap(),
            ]),
        )
        .unwrap()],
    )
    .unwrap();
    MetaRequest::new(
        snapshot,
        [MetaInput::new("schema", b"v1".as_slice()).unwrap()],
        [MetaOutputSpec::new("generated/out.to", "generated.out").unwrap()],
        MetaLimits::new(100_000, 1_048_576, 8192).unwrap(),
    )
    .unwrap()
}

const ORDINARY_COMPANION: &str = r#"
import std.meta
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    assert(meta.api() == "tondo-std-meta-0.1/1")
    assert(meta.target() == "tondo-meta")
    assert(meta.profile() == "meta")
    assert(request.inputs().length() == 1)
    let input = request.input("schema")?
    assert(input.bytes.length() == 2)
    assert(input.hash != "")
    assert(match request.input("missing") {
        err(meta.Error.UnknownInput(_)) => true
        _ => false
    })
    assert(request.limits().outputBytes == 8192u64)
    let declaration = request.snapshot().declarations[0]
    assert(declaration.identity == "Item")
    assert(match declaration.origin {
        meta.Origin.Source(span) => span.start == 0u32 and span.end == 4u32
        _ => false
    })
    let fields = match declaration.kind {
        meta.DeclarationKind.Record(fields) => fields
        _ => panic("expected record")
    }
    assert(fields[0].typeRef.identity() == "Int")
    var builder = request.sourceBuilder()
    assert(builder.outputs() == request.outputs())
    let path = builder.outputs()[0].path
    let ty = builder.renderType(path, fields[0].typeRef)?
    let indent = meta.indentation(1u32)?
    let literal = meta.stringLiteral("café\n")
    builder.add(path, "pub fn answer(): {ty} {{\n{indent}42\n}}\npub fn label(): String {{ {literal} }}\n")?
    builder.finish()
}
pub fn expand(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    assert(request.snapshot().declarations[0].identity == request.target())
    assert(request.module() == "model")
    assert(request.traitIdentity() == "Display")
    assert(request.bounds().length() == 0)
    assert(request.span().start == 0u32)
    assert(request.limits().outputBytes == 8192u64)
    var builder = request.sourceBuilder()
    let output = builder.outputs()[0]
    assert(output.module == request.module())
    builder.add(output.path, "impl {request.traitIdentity()} for {request.target()} {{\nfn display(self): String {{ \"Item\" }}\n}}\n")?
    builder.finishDerive()
}
"#;

#[test]
fn ordinary_companion_queries_and_builders_execute_with_fresh_owned_requests() {
    use tondo_compiler::meta_vm::MetaEntryKind;
    use tondo_compiler::std_meta::source_api;
    let request = ordinary_request();
    let generator = ordinary_provider(
        ORDINARY_COMPANION,
        "companion.generate",
        MetaEntryKind::Generate,
    )
    .load(MetaVmLimits::for_request(request.limits()))
    .unwrap();
    let run = || {
        generator
            .run_with_request(source_api::generate_request(&request), |outcome| {
                source_api::measure_generate_output(outcome, &request)
            })
            .unwrap()
    };
    let first = run();
    assert_eq!(first, run());
    let response = source_api::generate_response(&first.outcome, &request).unwrap();
    let source = std::str::from_utf8(response.outputs()[0].bytes()).unwrap();
    assert!(source.contains("pub fn answer(): Int"));
    assert!(source.contains("café\\n"));
    assert!(first.counters.steps > 0 && first.counters.peak_live_bytes > 0);
    assert!(first.counters.output_bytes > response.outputs()[0].bytes().len() as u64);
    let derive = ordinary_provider(
        ORDINARY_COMPANION,
        "companion.expand",
        MetaEntryKind::Derive,
    )
    .load(MetaVmLimits::for_request(request.limits()))
    .unwrap();
    let span = MetaSpan::new(0, 0, 4).unwrap();
    let execution = derive
        .run_with_request(
            source_api::derive_request(
                request.snapshot(),
                "Item",
                "model",
                "Display",
                &[],
                span,
                request.limits(),
            ),
            |outcome| {
                source_api::measure_derive_output(
                    outcome,
                    request.snapshot(),
                    request.limits(),
                    span,
                )
            },
        )
        .unwrap();
    let response = source_api::derive_response(
        &execution.outcome,
        request.snapshot(),
        request.limits(),
        span,
    )
    .unwrap();
    assert!(response.source.starts_with("impl Display for Item"));
    assert!(response.mappings.is_empty());
}
