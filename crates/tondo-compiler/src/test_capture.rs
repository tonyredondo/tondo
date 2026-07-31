//! Static suite-environment capture validation.
//!
//! A suite owns its setup environment.  A descendant may receive only a
//! logical snapshot of an immutable, copyable and shareable value; it never
//! receives a loan or the original affine owner.  This module is deliberately
//! independent from the test worker: the semantic checker supplies resolved
//! bindings and uses, and this boundary validates the closed capture contract
//! before lowering or scheduling exists.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::diagnostics::{
    Diagnostic, DiagnosticCode, DiagnosticError, PrimaryLocation, Related, Severity,
};
use crate::hir::{HirCapability, HirCapabilityStatus, HirProgram, HirTerminalStatus};
use crate::resolve::LocalId;
use crate::source::Span;
use crate::test_tree::{StaticTestTree, TestNodeIdentity, TestNodeKind};
use crate::types::TypeId;

const E2005: &str = "E2005";

/// The lexical declaration form that introduced a suite binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureBindingMode {
    /// An immutable owning binding that can be snapshotted when its type facts
    /// satisfy the capture contract.
    Let,
    /// A replaceable owning binding.  Its identity and contents may change,
    /// so it cannot cross a suite boundary.
    Var,
    /// A shared loan into another owner.
    Ref,
    /// An exclusive, fixed-extent loan.
    Mut,
}

impl CaptureBindingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Let => "let",
            Self::Var => "var",
            Self::Ref => "ref",
            Self::Mut => "mut",
        }
    }
}

impl fmt::Display for CaptureBindingMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a descendant observes a binding while constructing its snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureAccess {
    /// Ordinary value observation.  For a valid capture this is a `Copy`
    /// observation of the per-child snapshot, never a move from the suite.
    Observe,
    /// Shared loan syntax (`ref name`).
    SharedBorrow,
    /// Exclusive fixed-extent loan syntax (`mut name`).
    MutableBorrow,
    /// Replaceable loan syntax (`var name`).
    ReplaceBorrow,
    /// An owning use that consumes the binding.
    Move,
}

impl CaptureAccess {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::SharedBorrow => "ref",
            Self::MutableBorrow => "mut",
            Self::ReplaceBorrow => "var",
            Self::Move => "move",
        }
    }

    const fn is_snapshot_safe(self) -> bool {
        matches!(self, Self::Observe)
    }
}

impl fmt::Display for CaptureAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Capability/terminal facts for one already type-checked binding.
///
/// `from_hir` is the adapter used by the future test-body checker.  Keeping
/// the facts as a small value also lets this module be tested without
/// constructing a complete HIR program and makes the boundary explicit: no
/// capture path may invent a capability result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTypeFacts {
    copy: HirCapabilityStatus,
    send: HirCapabilityStatus,
    share: HirCapabilityStatus,
    terminal: HirTerminalStatus,
}

impl CaptureTypeFacts {
    pub const fn new(
        copy: HirCapabilityStatus,
        send: HirCapabilityStatus,
        share: HirCapabilityStatus,
        terminal: HirTerminalStatus,
    ) -> Self {
        Self {
            copy,
            send,
            share,
            terminal,
        }
    }

    /// Read the canonical facts computed by HIR capability and terminal
    /// analysis.  A missing fact means the caller supplied an incomplete HIR
    /// and is rejected rather than treated as safe.
    pub fn from_hir(program: &HirProgram, ty: TypeId) -> Result<Self, CaptureError> {
        let status = |capability| {
            program
                .capability_status(ty, capability)
                .ok_or(CaptureError::TypeFactsUnavailable { ty })
        };
        let terminal = program
            .terminal_status(ty)
            .ok_or(CaptureError::TypeFactsUnavailable { ty })?;
        Ok(Self::new(
            status(HirCapability::Copy)?,
            status(HirCapability::Send)?,
            status(HirCapability::Share)?,
            terminal,
        ))
    }

    pub const fn copy(self) -> HirCapabilityStatus {
        self.copy
    }

    pub const fn send(self) -> HirCapabilityStatus {
        self.send
    }

    pub const fn share(self) -> HirCapabilityStatus {
        self.share
    }

    pub const fn terminal(self) -> HirTerminalStatus {
        self.terminal
    }

    fn is_snapshot_safe(self) -> bool {
        self.copy == HirCapabilityStatus::Satisfied
            && self.send == HirCapabilityStatus::Satisfied
            && self.share == HirCapabilityStatus::Satisfied
            && self.terminal == HirTerminalStatus::Absent
    }

    fn missing_capabilities(self) -> Vec<&'static str> {
        [
            (self.copy, "Copy"),
            (self.send, "Send"),
            (self.share, "Share"),
        ]
        .into_iter()
        .filter_map(|(status, name)| (status != HirCapabilityStatus::Satisfied).then_some(name))
        .collect()
    }
}

/// A binding introduced by a suite setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureBinding {
    owner: TestNodeIdentity,
    local: LocalId,
    name: String,
    span: Span,
    mode: CaptureBindingMode,
    ty: TypeId,
    facts: CaptureTypeFacts,
}

impl CaptureBinding {
    pub fn new(
        owner: TestNodeIdentity,
        local: LocalId,
        name: impl Into<String>,
        span: Span,
        mode: CaptureBindingMode,
        ty: TypeId,
        facts: CaptureTypeFacts,
    ) -> Self {
        Self {
            owner,
            local,
            name: name.into(),
            span,
            mode,
            ty,
            facts,
        }
    }

    pub fn owner(&self) -> &TestNodeIdentity {
        &self.owner
    }

    pub fn local(&self) -> LocalId {
        self.local
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn mode(&self) -> CaptureBindingMode {
        self.mode
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub const fn facts(&self) -> CaptureTypeFacts {
        self.facts
    }
}

/// One semantic use of a suite binding from a descendant body/setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureUse {
    target: TestNodeIdentity,
    local: LocalId,
    span: Span,
    access: CaptureAccess,
}

impl CaptureUse {
    pub fn new(
        target: TestNodeIdentity,
        local: LocalId,
        span: Span,
        access: CaptureAccess,
    ) -> Self {
        Self {
            target,
            local,
            span,
            access,
        }
    }

    pub fn target(&self) -> &TestNodeIdentity {
        &self.target
    }

    pub fn local(&self) -> LocalId {
        self.local
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn access(&self) -> CaptureAccess {
        self.access
    }
}

/// One slot in a child's immutable snapshot environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCapture {
    target: TestNodeIdentity,
    source: TestNodeIdentity,
    local: LocalId,
    name: String,
    ty: TypeId,
    slot: u32,
}

impl SnapshotCapture {
    pub fn target(&self) -> &TestNodeIdentity {
        &self.target
    }

    pub fn source(&self) -> &TestNodeIdentity {
        &self.source
    }

    pub fn local(&self) -> LocalId {
        self.local
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn slot(&self) -> u32 {
        self.slot
    }
}

/// Deterministic capture environment descriptors consumed by later lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCapturePlan {
    captures: Vec<SnapshotCapture>,
    diagnostics: Vec<Diagnostic>,
}

impl TestCapturePlan {
    pub fn captures(&self) -> &[SnapshotCapture] {
        &self.captures
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn is_empty(&self) -> bool {
        self.captures.is_empty()
    }
}

/// Errors emitted while validating the closed capture boundary.
#[derive(Debug)]
pub enum CaptureError {
    Diagnostic(DiagnosticError),
    TypeFactsUnavailable { ty: TypeId },
    InvalidInput(String),
    Diagnostics(Vec<Diagnostic>),
}

impl CaptureError {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::Diagnostics(diagnostics) => diagnostics,
            Self::Diagnostic(_) | Self::TypeFactsUnavailable { .. } | Self::InvalidInput(_) => &[],
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::TypeFactsUnavailable { ty } => {
                write!(
                    formatter,
                    "capture type facts are unavailable for type {ty:?}"
                )
            }
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::Diagnostics(diagnostics) => write!(
                formatter,
                "suite capture rejected with {} diagnostic(s)",
                diagnostics.len()
            ),
        }
    }
}

impl Error for CaptureError {}

impl From<DiagnosticError> for CaptureError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

#[derive(Debug, Clone)]
struct NodeMeta {
    kind: TestNodeKind,
    parent: Option<TestNodeIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CaptureKey {
    target: TestNodeIdentity,
    source: TestNodeIdentity,
    local: LocalId,
}

#[derive(Debug, Clone)]
struct PendingDiagnostic {
    key: DiagnosticKey,
    message: String,
    primary: Span,
    related: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DiagnosticKey {
    file: u32,
    start: u32,
    end: u32,
    message: String,
}

/// Validate all semantic capture uses and produce one immutable snapshot slot
/// per `(target, source binding)` pair.
pub fn build(
    tree: &StaticTestTree,
    bindings: impl IntoIterator<Item = CaptureBinding>,
    uses: impl IntoIterator<Item = CaptureUse>,
) -> Result<TestCapturePlan, CaptureError> {
    let mut nodes = BTreeMap::<TestNodeIdentity, NodeMeta>::new();
    let mut diagnostics = tree.diagnostics().to_vec();
    for node in tree.nodes() {
        if nodes
            .insert(
                node.identity().clone(),
                NodeMeta {
                    kind: node.identity().kind(),
                    parent: node.parent().cloned(),
                },
            )
            .is_some()
        {
            return Err(CaptureError::InvalidInput(format!(
                "static test tree contains duplicate node identity `{}`",
                node.visible_id()
            )));
        }
    }

    let mut binding_by_local = BTreeMap::<LocalId, CaptureBinding>::new();
    for binding in bindings {
        let Some(owner) = nodes.get(binding.owner()) else {
            return Err(CaptureError::InvalidInput(format!(
                "capture binding `{}` refers to an unknown suite",
                binding.name
            )));
        };
        if owner.kind != TestNodeKind::Suite {
            return Err(CaptureError::InvalidInput(format!(
                "capture binding `{}` is not declared by a suite",
                binding.name
            )));
        }
        if binding_by_local.insert(binding.local, binding).is_some() {
            return Err(CaptureError::InvalidInput(
                "capture local is declared more than once".into(),
            ));
        }
    }

    let mut pending = Vec::new();
    let mut captures = BTreeMap::<CaptureKey, SnapshotCapture>::new();
    let mut seen_uses = BTreeSet::<(TestNodeIdentity, LocalId, Span, CaptureAccess)>::new();
    let mut uses = uses.into_iter().collect::<Vec<_>>();
    uses.sort_by(|left, right| {
        (
            left.target.clone(),
            left.local,
            left.span.file().index(),
            left.span.range(),
            left.access,
        )
            .cmp(&(
                right.target.clone(),
                right.local,
                right.span.file().index(),
                right.span.range(),
                right.access,
            ))
    });

    for use_site in uses {
        if !seen_uses.insert((
            use_site.target.clone(),
            use_site.local,
            use_site.span,
            use_site.access,
        )) {
            continue;
        }
        if !nodes.contains_key(&use_site.target) {
            return Err(CaptureError::InvalidInput(
                "capture use refers to an unknown test node".into(),
            ));
        }
        let Some(binding) = binding_by_local.get(&use_site.local) else {
            return Err(CaptureError::InvalidInput(format!(
                "capture use refers to unknown local {}",
                use_site.local.index()
            )));
        };
        let mut invalid = None;
        if !is_descendant(&nodes, &binding.owner, &use_site.target) {
            invalid = Some(format!(
                "binding `{}` is not an ancestor of the target node; nested suites cannot bypass the owning suite",
                binding.name
            ));
        } else if binding.mode != CaptureBindingMode::Let {
            invalid = Some(format!(
                "binding `{}` uses `{}` and cannot be copied into a suite snapshot",
                binding.name, binding.mode
            ));
        } else if !use_site.access.is_snapshot_safe() {
            invalid = Some(format!(
                "binding `{}` is accessed as `{}`; loans and moves cannot cross a suite boundary",
                binding.name, use_site.access
            ));
        } else if !binding.facts.is_snapshot_safe() {
            let missing = binding.facts.missing_capabilities();
            if !missing.is_empty() {
                invalid = Some(format!(
                    "binding `{}` requires Copy + Send + Share; missing or unresolved: {}",
                    binding.name,
                    missing.join(", ")
                ));
            } else {
                invalid = Some(format!(
                    "binding `{}` has a terminal ownership obligation and cannot be snapshotted",
                    binding.name
                ));
            }
        }
        if let Some(message) = invalid {
            pending.push(PendingDiagnostic {
                key: DiagnosticKey {
                    file: use_site.span.file().index(),
                    start: use_site.span.range().start(),
                    end: use_site.span.range().end(),
                    message: message.clone(),
                },
                message,
                primary: use_site.span,
                related: binding.span,
            });
            continue;
        }

        let source = binding.owner.clone();
        captures
            .entry(CaptureKey {
                target: use_site.target.clone(),
                source: source.clone(),
                local: binding.local,
            })
            .or_insert_with(|| SnapshotCapture {
                target: use_site.target,
                source,
                local: binding.local,
                name: binding.name.clone(),
                ty: binding.ty,
                slot: 0,
            });
    }

    pending.sort_by(|left, right| left.key.cmp(&right.key));
    for pending in pending {
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(E2005)?,
            pending.message,
            PrimaryLocation::Source(pending.primary),
        )?;
        diagnostic = diagnostic.with_related(Related::new(
            "suite binding declared here",
            pending.related,
        )?);
        diagnostics.push(diagnostic);
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == Severity::Error)
    {
        return Err(CaptureError::Diagnostics(diagnostics));
    }

    let mut captures = captures.into_values().collect::<Vec<_>>();
    let mut slots_by_target = BTreeMap::<TestNodeIdentity, u32>::new();
    for capture in &mut captures {
        let slot = slots_by_target.entry(capture.target.clone()).or_insert(0);
        capture.slot = *slot;
        *slot = slot.saturating_add(1);
    }
    Ok(TestCapturePlan {
        captures,
        diagnostics,
    })
}

fn is_descendant(
    nodes: &BTreeMap<TestNodeIdentity, NodeMeta>,
    ancestor: &TestNodeIdentity,
    descendant: &TestNodeIdentity,
) -> bool {
    let mut current = nodes.get(descendant).and_then(|node| node.parent.clone());
    let mut visited = BTreeSet::new();
    while let Some(identity) = current {
        if !visited.insert(identity.clone()) {
            return false;
        }
        if &identity == ancestor {
            return true;
        }
        current = nodes.get(&identity).and_then(|node| node.parent.clone());
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::hir::{ExpressionCheckLimits, TypeLoweringLimits, check_expressions, lower_types};
    use crate::package::{PackageGraph, PackageId};
    use crate::resolve::resolve;
    use crate::source::{
        LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, TextRange,
    };
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};
    use crate::test_plan::TestSourceClass;
    use crate::test_tree::{TestSourceInput, build as build_tree};

    fn package() -> PackageId {
        PackageId::new("workspace:app@1").unwrap()
    }

    fn parse_source_at(
        sources: &mut SourceDatabase,
        id: &str,
        module: &str,
        path: &str,
        text: &str,
    ) -> (crate::source::FileId, crate::syntax::Parsed) {
        let file = sources
            .add(SourceInput::new(
                SourceId::new(id).unwrap(),
                ModulePath::new(module).unwrap(),
                LogicalPath::new(path).unwrap(),
                crate::source::SourceOrigin::Virtual,
                Arc::<[u8]>::from(text.as_bytes()),
            ))
            .unwrap();
        let lexed = lex(sources, file, LexMode::Module).unwrap();
        let parsed = parse(
            sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(parsed.diagnostics().is_empty());
        (file, parsed)
    }

    fn parse_source(
        sources: &mut SourceDatabase,
        id: &str,
        text: &str,
    ) -> (crate::source::FileId, crate::syntax::Parsed) {
        parse_source_at(sources, id, "math", "src/math_test.to", text)
    }

    fn tree(
        sources: &SourceDatabase,
        file: crate::source::FileId,
        parsed: &crate::syntax::Parsed,
    ) -> StaticTestTree {
        let package = package();
        let module = ModulePath::new("math").unwrap();
        let path = LogicalPath::new("src/math_test.to").unwrap();
        build_tree(
            sources,
            [TestSourceInput::new(
                &package,
                "app",
                TestSourceClass::UnitTest,
                &module,
                &path,
                file,
                parsed.cst(),
            )],
        )
        .unwrap()
    }

    fn facts() -> CaptureTypeFacts {
        CaptureTypeFacts::new(
            HirCapabilityStatus::Satisfied,
            HirCapabilityStatus::Satisfied,
            HirCapabilityStatus::Satisfied,
            HirTerminalStatus::Absent,
        )
    }

    fn type_id() -> TypeId {
        TypeId::from_index(0)
    }

    fn binding(
        owner: &TestNodeIdentity,
        span: Span,
        local: u32,
        mode: CaptureBindingMode,
        facts: CaptureTypeFacts,
    ) -> CaptureBinding {
        CaptureBinding::new(
            owner.clone(),
            LocalId::from_index(local),
            "endpoint",
            span,
            mode,
            type_id(),
            facts,
        )
    }

    fn use_site(
        target: &TestNodeIdentity,
        span: Span,
        local: u32,
        access: CaptureAccess,
    ) -> CaptureUse {
        CaptureUse::new(target.clone(), LocalId::from_index(local), span, access)
    }

    fn error_codes(error: &CaptureError) -> Vec<&str> {
        error
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    fn hir_program(checked: bool) -> (crate::hir::HirProgram, TypeId) {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source_at(
            &mut sources,
            "src:hir-facts",
            "math",
            "src/hir_facts.to",
            "fn main(): Int {\n    1\n}\n",
        );
        let packages = PackageGraph::loose(&sources, file).unwrap();
        let resolved = resolve(&packages, &sources, [(file, &parsed)], 100).unwrap();
        let (resolved, diagnostics) = resolved.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let lowered = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        let (program, diagnostics) = lowered.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let (program, ty) = if checked {
            let checked = check_expressions(
                &sources,
                [(file, &parsed)],
                &resolved,
                program,
                ExpressionCheckLimits {
                    max_nodes: 100_000,
                    max_pattern_steps: 100_000,
                    max_trait_obligations: 100_000,
                    max_diagnostics: 100,
                },
            )
            .unwrap();
            let ty = checked.program().interner().ids().next().unwrap();
            (checked.into_parts().0, ty)
        } else {
            let ty = program.interner().ids().next().unwrap();
            (program, ty)
        };
        (program, ty)
    }

    #[test]
    fn valid_let_capture_creates_one_snapshot_slot_per_target() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:valid",
            "suite outer {\n    test child {\n        use(endpoint)\n        use(endpoint)\n    }\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        let suite = tree.nodes()[0].identity().clone();
        let child = tree.nodes()[1].identity().clone();
        let binding_span = sources.span(file, TextRange::new(14, 22).unwrap()).unwrap();
        let use_span = sources.span(file, TextRange::new(58, 66).unwrap()).unwrap();
        let source_binding = binding(&suite, binding_span, 1, CaptureBindingMode::Let, facts());
        let use_site = use_site(&child, use_span, 1, CaptureAccess::Observe);
        let plan = build(
            &tree,
            [source_binding.clone()],
            [use_site.clone(), use_site.clone()],
        )
        .unwrap();
        assert_eq!(plan.captures().len(), 1);
        assert_eq!(plan.captures()[0].slot(), 0);
        assert_eq!(plan.captures()[0].name(), "endpoint");
        assert_eq!(plan.captures()[0].source(), &suite);
        assert_eq!(plan.captures()[0].target(), &child);
        assert_eq!(plan.captures()[0].local(), LocalId::from_index(1));
        assert_eq!(plan.captures()[0].ty(), type_id());
        assert_eq!(source_binding.owner(), &suite);
        assert_eq!(source_binding.local(), LocalId::from_index(1));
        assert_eq!(source_binding.name(), "endpoint");
        assert_eq!(source_binding.span(), binding_span);
        assert_eq!(source_binding.mode(), CaptureBindingMode::Let);
        assert_eq!(source_binding.ty(), type_id());
        assert_eq!(source_binding.facts(), facts());
        assert_eq!(use_site.target(), &child);
        assert_eq!(use_site.local(), LocalId::from_index(1));
        assert_eq!(use_site.span(), use_span);
        assert_eq!(use_site.access(), CaptureAccess::Observe);
        assert!(!plan.is_empty());
        assert!(plan.diagnostics().is_empty());
    }

    #[test]
    fn nested_suite_captures_are_ancestor_checked_and_slot_order_is_stable() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:nested",
            "suite outer {\n    suite inner {\n        test child {}\n    }\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        let outer = tree.nodes()[0].identity().clone();
        let inner = tree.nodes()[1].identity().clone();
        let child = tree.nodes()[2].identity().clone();
        let span = sources.span(file, TextRange::empty(14)).unwrap();
        let plan = build(
            &tree,
            [
                binding(&outer, span, 2, CaptureBindingMode::Let, facts()),
                CaptureBinding::new(
                    inner.clone(),
                    LocalId::from_index(3),
                    "port",
                    span,
                    CaptureBindingMode::Let,
                    TypeId::from_index(1),
                    facts(),
                ),
            ],
            [
                use_site(&child, span, 3, CaptureAccess::Observe),
                use_site(&child, span, 2, CaptureAccess::Observe),
            ],
        )
        .unwrap();
        assert_eq!(plan.captures().len(), 2);
        assert_eq!(plan.captures()[0].name(), "endpoint");
        assert_eq!(plan.captures()[0].slot(), 0);
        assert_eq!(plan.captures()[1].name(), "port");
        assert_eq!(plan.captures()[1].slot(), 1);
    }

    #[test]
    fn var_ref_mut_and_move_accesses_are_all_e2005() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:modes",
            "suite outer {\n    test child {}\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        let suite = tree.nodes()[0].identity().clone();
        let child = tree.nodes()[1].identity().clone();
        let span = sources.span(file, TextRange::empty(10)).unwrap();
        for (index, (mode, access)) in [
            (CaptureBindingMode::Var, CaptureAccess::Observe),
            (CaptureBindingMode::Ref, CaptureAccess::Observe),
            (CaptureBindingMode::Mut, CaptureAccess::Observe),
            (CaptureBindingMode::Let, CaptureAccess::SharedBorrow),
            (CaptureBindingMode::Let, CaptureAccess::MutableBorrow),
            (CaptureBindingMode::Let, CaptureAccess::ReplaceBorrow),
            (CaptureBindingMode::Let, CaptureAccess::Move),
        ]
        .into_iter()
        .enumerate()
        {
            let error = build(
                &tree,
                [binding(&suite, span, index as u32, mode, facts())],
                [use_site(&child, span, index as u32, access)],
            )
            .unwrap_err();
            assert_eq!(error_codes(&error), [E2005]);
        }

        let error = build(
            &tree,
            [binding(&suite, span, 99, CaptureBindingMode::Let, facts())],
            [
                use_site(&child, span, 99, CaptureAccess::Observe),
                use_site(&child, span, 99, CaptureAccess::Move),
            ],
        )
        .unwrap_err();
        assert_eq!(error_codes(&error), [E2005]);
    }

    #[test]
    fn missing_capabilities_and_terminal_types_are_e2005() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:facts",
            "suite outer {\n    test child {}\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        let suite = tree.nodes()[0].identity().clone();
        let child = tree.nodes()[1].identity().clone();
        let span = sources.span(file, TextRange::empty(10)).unwrap();
        for facts in [
            CaptureTypeFacts::new(
                HirCapabilityStatus::Unsatisfied,
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Satisfied,
                HirTerminalStatus::Absent,
            ),
            CaptureTypeFacts::new(
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Deferred,
                HirCapabilityStatus::Satisfied,
                HirTerminalStatus::Absent,
            ),
            CaptureTypeFacts::new(
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Unsatisfied,
                HirTerminalStatus::Present,
            ),
            CaptureTypeFacts::new(
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Satisfied,
                HirCapabilityStatus::Satisfied,
                HirTerminalStatus::Potential,
            ),
        ] {
            let error = build(
                &tree,
                [binding(&suite, span, 1, CaptureBindingMode::Let, facts)],
                [use_site(&child, span, 1, CaptureAccess::Observe)],
            )
            .unwrap_err();
            assert_eq!(error_codes(&error), [E2005]);
        }
    }

    #[test]
    fn non_ancestor_and_invalid_node_inputs_are_rejected() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:invalid",
            "suite first {\n    test firstChild {}\n}\nsuite second {\n    test secondChild {}\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        let first = tree.nodes()[0].identity().clone();
        let first_child = tree.nodes()[1].identity().clone();
        let second_child = tree.nodes()[3].identity().clone();
        let span = sources.span(file, TextRange::empty(1)).unwrap();
        let error = build(
            &tree,
            [binding(&first, span, 1, CaptureBindingMode::Let, facts())],
            [use_site(&second_child, span, 1, CaptureAccess::Observe)],
        )
        .unwrap_err();
        assert_eq!(error_codes(&error), [E2005]);

        let (other_file, other_parsed) = parse_source_at(
            &mut sources,
            "src:other",
            "other",
            "src/other_test.to",
            "test elsewhere {}\n",
        );
        let other_tree = {
            let package = package();
            let module = ModulePath::new("other").unwrap();
            let path = LogicalPath::new("src/other_test.to").unwrap();
            build_tree(
                &sources,
                [TestSourceInput::new(
                    &package,
                    "app",
                    TestSourceClass::UnitTest,
                    &module,
                    &path,
                    other_file,
                    other_parsed.cst(),
                )],
            )
            .unwrap()
        };
        let unknown = other_tree.nodes()[0].identity().clone();
        let error = build(
            &tree,
            [binding(&first, span, 1, CaptureBindingMode::Let, facts())],
            [use_site(&unknown, span, 1, CaptureAccess::Observe)],
        )
        .unwrap_err();
        assert!(
            matches!(error, CaptureError::InvalidInput(message) if message.contains("unknown test node"))
        );

        let error = build(
            &tree,
            [binding(
                &first_child,
                span,
                1,
                CaptureBindingMode::Let,
                facts(),
            )],
            [use_site(&second_child, span, 1, CaptureAccess::Observe)],
        )
        .unwrap_err();
        assert!(
            matches!(error, CaptureError::InvalidInput(message) if message.contains("not declared by a suite"))
        );
    }

    #[test]
    fn diagnostics_preserve_tree_warnings_and_related_binding_spans() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:diagnostics",
            "suite not_camel {\n    test child {}\n}\n",
        );
        let tree = tree(&sources, file, &parsed);
        assert_eq!(tree.diagnostics().len(), 1);
        let suite = tree.nodes()[0].identity().clone();
        let child = tree.nodes()[1].identity().clone();
        let binding_span = sources.span(file, TextRange::new(6, 14).unwrap()).unwrap();
        let use_span = sources.span(file, TextRange::new(27, 32).unwrap()).unwrap();
        let error = build(
            &tree,
            [binding(
                &suite,
                binding_span,
                5,
                CaptureBindingMode::Var,
                facts(),
            )],
            [use_site(&child, use_span, 5, CaptureAccess::Observe)],
        )
        .unwrap_err();
        assert_eq!(error_codes(&error), ["W1004", E2005]);
        assert_eq!(
            error.diagnostics()[1].location(),
            &PrimaryLocation::Source(use_span)
        );
        assert!(
            error.diagnostics()[1]
                .message()
                .contains("cannot be copied into a suite snapshot")
        );
    }

    #[test]
    fn accessors_and_type_fact_adapter_boundaries_are_closed() {
        let facts = CaptureTypeFacts::new(
            HirCapabilityStatus::Satisfied,
            HirCapabilityStatus::Deferred,
            HirCapabilityStatus::Satisfied,
            HirTerminalStatus::Potential,
        );
        assert_eq!(facts.copy(), HirCapabilityStatus::Satisfied);
        assert_eq!(facts.send(), HirCapabilityStatus::Deferred);
        assert_eq!(facts.share(), HirCapabilityStatus::Satisfied);
        assert_eq!(facts.terminal(), HirTerminalStatus::Potential);
        assert_eq!(
            [
                CaptureBindingMode::Let,
                CaptureBindingMode::Var,
                CaptureBindingMode::Ref,
                CaptureBindingMode::Mut,
            ]
            .map(CaptureBindingMode::as_str),
            ["let", "var", "ref", "mut"]
        );
        assert_eq!(
            [
                CaptureAccess::Observe,
                CaptureAccess::SharedBorrow,
                CaptureAccess::MutableBorrow,
                CaptureAccess::ReplaceBorrow,
                CaptureAccess::Move,
            ]
            .map(CaptureAccess::as_str),
            ["observe", "ref", "mut", "var", "move"]
        );
        assert_eq!(CaptureBindingMode::Let.to_string(), "let");
        assert_eq!(CaptureAccess::Move.to_string(), "move");

        let error = CaptureError::TypeFactsUnavailable { ty: type_id() };
        assert!(error.to_string().contains("type facts are unavailable"));
        assert!(error.diagnostics().is_empty());
        let diagnostic = CaptureError::from(DiagnosticError::EmptyMessage);
        assert!(diagnostic.to_string().contains("diagnostic message"));
        assert!(diagnostic.diagnostics().is_empty());
    }

    #[test]
    fn hirs_type_facts_are_required_and_read_only() {
        let (checked, ty) = hir_program(true);
        let facts = CaptureTypeFacts::from_hir(&checked, ty).unwrap();
        assert_eq!(facts.copy(), HirCapabilityStatus::Satisfied);
        assert_eq!(facts.send(), HirCapabilityStatus::Satisfied);
        assert_eq!(facts.share(), HirCapabilityStatus::Satisfied);
        assert_eq!(facts.terminal(), HirTerminalStatus::Absent);

        let (unchecked, ty) = hir_program(false);
        assert!(matches!(
            CaptureTypeFacts::from_hir(&unchecked, ty),
            Err(CaptureError::TypeFactsUnavailable { .. })
        ));
    }

    #[test]
    fn empty_plans_and_input_duplicates_are_rejected() {
        let mut sources = SourceDatabase::new();
        let (file, parsed) = parse_source(
            &mut sources,
            "src:empty",
            "suite outer {\n    test child {}\n}\n",
        );
        let suite_tree = tree(&sources, file, &parsed);
        let plan = build(&suite_tree, [], []).unwrap();
        assert!(plan.is_empty());
        assert!(plan.captures().is_empty());
        assert!(plan.diagnostics().is_empty());

        let duplicate_tree = tree(&sources, file, &parsed);
        let suite = duplicate_tree.nodes()[0].identity().clone();
        let child = duplicate_tree.nodes()[1].identity().clone();
        let span = sources.span(file, TextRange::empty(5)).unwrap();
        let duplicate = binding(&suite, span, 1, CaptureBindingMode::Let, facts());
        let error = build(
            &duplicate_tree,
            [duplicate.clone(), duplicate],
            std::iter::empty::<CaptureUse>(),
        )
        .unwrap_err();
        assert!(
            matches!(error, CaptureError::InvalidInput(message) if message.contains("declared more than once"))
        );

        let error = build(
            &duplicate_tree,
            [binding(&suite, span, 1, CaptureBindingMode::Let, facts())],
            [use_site(&child, span, 99, CaptureAccess::Observe)],
        )
        .unwrap_err();
        assert!(
            matches!(error, CaptureError::InvalidInput(message) if message.contains("unknown local"))
        );
    }
}
