use std::collections::BTreeSet;
use std::sync::Arc;

use tondo_compiler::diagnostics::DiagnosticCode;
use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, HostProfile, Operation,
    ResourceLimits, SourceForm, execute,
};
use tondo_compiler::hir::{HirTerminalOperation, HirTerminalStatus, HirTerminalUnwindAction};
use tondo_compiler::package::{Edition, PackageGraph};
use tondo_compiler::resolve::{ResolveError, resolve};
use tondo_compiler::semantic::SemanticEntity;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, TextRange,
};
use tondo_compiler::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

#[test]
fn public_driver_output_supports_semantic_queries() {
    let source = "fn answer(): Int { 42 }\n\
                  fn inspect(value: ref Join[Int, Never]) {}\n\
                  fn process(left: mut Array[Int], right: mut Array[Int]) {}\n\
                  fn inspect_region(item: mut Int, region: mut Array[Int]) {}\n\
                  fn ranges(values: var Array[Int]) {\n\
                      process(mut values[0:2], mut values[2:4])\n\
                      inspect_region(mut values[0], mut values[2:4])\n\
                  }\n\
                  fn main() {\n    let value = answer()\n}\n";
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:public-semantic-test").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("main.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes().to_vec()),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    let output = execute(
        CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap(),
    )
    .unwrap();

    let model = output.semantic_model().expect("semantic phase completed");
    let call_start = u32::try_from(source.rfind("answer()").unwrap()).unwrap();
    let name = TextRange::new(call_start, call_start + 6).unwrap();
    let call = TextRange::new(call_start, call_start + 8).unwrap();
    let entity = model
        .entities_at(root, name)
        .into_iter()
        .find(|entity| matches!(entity, SemanticEntity::Name(_)))
        .expect("the function name resolves");
    assert!(model.signature(&entity).is_some());
    assert_eq!(
        model.closed_call_errors_at(root, call).unwrap(),
        Some(Vec::new())
    );

    let join = model
        .interner()
        .unwrap()
        .ids()
        .find(|ty| {
            model
                .canonical_type(*ty)
                .is_ok_and(|name| name.as_deref() == Some("Join[Int, Never]"))
        })
        .expect("the terminal parameter interns Join");
    assert_eq!(
        model.terminal_status(join),
        Some(HirTerminalStatus::Present)
    );
    let contract = model
        .direct_terminal_contract(join)
        .unwrap()
        .expect("Join has a direct terminal contract");
    assert_eq!(contract.operation(), HirTerminalOperation::JoinAwait);
    assert_eq!(contract.unwind(), HirTerminalUnwindAction::JoinTeardown);
    assert!(contract.unwind_may_suspend());
}

#[test]
fn public_driver_reproves_intrinsic_serialization_opaque_witnesses() {
    let source = "import std.serialization\n\
                  fn encoded(): impl Discard + serialization.Encode[Json] { 1 }\n\
                  fn decoded(): impl Discard + serialization.Decode[Json] { 1 }\n\
                  fn main() {}\n";
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:public-serialization-opaque-test").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("main.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes().to_vec()),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    let output = execute(
        CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(output.status(), CompilationStatus::Success);
    assert!(output.diagnostics().is_empty());
}

#[test]
fn public_driver_rejects_inherent_methods_on_intrinsic_types() {
    let source = "fn Int.invalid() {}\nfn main() {}\n";
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:public-intrinsic-method-test").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("main.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes().to_vec()),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    let output = execute(
        CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(output.status(), CompilationStatus::Rejected);
    assert!(output.diagnostics().json_lines().unwrap().contains("E1504"));
}

#[test]
fn public_resolve_output_exposes_the_resolved_program() {
    let source = "type Counter = { value: Int }\n\
                  fn Counter.read(self): Int { self.value }\n";
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:public-resolve-output-test").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("main.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes().to_vec()),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    let lexed = lex(&sources, root, LexMode::Module).unwrap();
    let parsed = parse(
        &sources,
        root,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .unwrap();
    let output = resolve(&packages, &sources, [(root, &parsed)], 10_000).unwrap();

    assert_eq!(output.program().modules().len(), 1);
    assert_eq!(output.program().file(root).unwrap().imports().len(), 0);
    assert!(!output.program().members().next().unwrap().is_synthetic());
    assert!(output.diagnostics().is_empty());

    let error = ResolveError::Diagnostic(DiagnosticCode::new("bad").unwrap_err());
    assert_eq!(error.to_string(), "invalid diagnostic code `bad`");
}
