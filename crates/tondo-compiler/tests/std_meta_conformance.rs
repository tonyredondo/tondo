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

fn request(output_bytes: u64) -> MetaRequest {
    MetaRequest::new(
        MetaSnapshot::new([], [], []).unwrap(),
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
        types: vec![
            BytecodeType {
                name: "Unit".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
            },
            BytecodeType {
                name: "unsafe fn(): Unit".into(),
                kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                    is_async: false,
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
fn candidate_descriptor_and_rendering_are_reproducible() {
    let first = StdMetaPackage::load_candidate().unwrap();
    let second = StdMetaPackage::load_candidate().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(MetaRenderer::string("\nTondo🙂"), "\"\\nTondo🙂\"");
}
