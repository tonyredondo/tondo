use std::collections::BTreeSet;
use std::sync::Arc;

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, DiagnosticFormat, HostProfile, Operation, ResourceLimits,
    SourceForm, execute,
};
use tondo_compiler::hir::{HirTerminalOperation, HirTerminalStatus, HirTerminalUnwindAction};
use tondo_compiler::package::{Edition, PackageGraph};
use tondo_compiler::semantic::SemanticEntity;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, TextRange,
};

#[test]
fn public_driver_output_supports_semantic_queries() {
    let source = "fn answer(): Int { 42 }\n\
                  fn inspect(value: ref Join[Int, Never]) {}\n\
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
