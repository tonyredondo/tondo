//! Rejection contracts for malformed MIR metadata and ownership proofs.
//! Keep these fixtures outside the verifier's measured production source.

use super::tests::{callable_named, checked_mir};
use super::*;
use crate::mir::MirConstant;

const CONTEXT: &str = "MIR boundary regression";
const SOURCE: &str = "type Record = { field: Int }\n\
    fn observe(value: ref Int): Int { value }\n\
    fn fixture(value: Int): Int {\n\
        let closure = (): Int { value }\n\
        _ = closure\n\
        observe(ref value)\n\
    }\n";

fn check_contract(source: &str, check: impl FnOnce(&Verifier<'_>, &MirProgram)) {
    let (resolved, hir, mir) = checked_mir(source);
    verify_mir(&resolved, &hir, &mir).unwrap();
    let capabilities = CapabilityAnalysis::new(&hir, &resolved).unwrap();
    let verifier = Verifier {
        resolved: &resolved,
        hir: &hir,
        capability_analysis: &capabilities,
        terminal_analysis: TerminalAnalysis::new(&hir, &resolved).unwrap(),
        capability_statuses: RefCell::new(BTreeMap::new()),
        terminal_statuses: RefCell::new(BTreeMap::new()),
        limits: MirVerificationLimits::default(),
        dataflow_steps: Cell::new(0),
    };
    check(&verifier, &mir);
}

fn rejects<T: fmt::Debug>(result: Result<T, MirInvariantError>, expected: &str) {
    let error = result.expect_err("malformed MIR must fail before bytecode lowering");
    assert!(error.message().contains(expected), "{error}");
    assert!(error.context().contains(CONTEXT), "{error}");
    assert!(!error.is_resource_limit(), "{error}");
}

fn fixture<'a>(verifier: &Verifier<'_>, mir: &'a MirProgram) -> &'a MirFunction {
    &mir.functions[&MirFunctionId::Callable(callable_named(verifier.resolved, "fixture"))]
}

fn root(function: &MirFunction) -> MirPlace {
    let local = function.parameters[0];
    MirPlace {
        local,
        ty: function.locals[local.0 as usize].ty,
        projections: Vec::new(),
        source_loan: None,
    }
}

#[test]
fn missing_metadata_reports_errors_at_each_verifier_boundary() {
    // An ID from another typed unit must never be accepted as local metadata.
    let (_, foreign_hir, _) = checked_mir("fn foreign() {\n _ = () {}\n _ = () {}\n}\n");
    let foreign_closure = foreign_hir.closures().last().unwrap().id();
    check_contract(SOURCE, |verifier, mir| {
        let function = fixture(verifier, mir);
        let int = verifier.hir.interner().scalar(ScalarType::Int);
        let invalid = TypeId::from_index(u32::MAX);
        rejects(verifier.kind(invalid, CONTEXT), "not interned");
        rejects(verifier.verify_type(invalid, CONTEXT), "not canonical");
        rejects(
            verifier.local(function, MirLocalId(u32::MAX), CONTEXT),
            "unknown MIR local",
        );
        rejects(
            verifier.loan(function, MirLoanId(u32::MAX), CONTEXT),
            "unknown MIR loan",
        );
        rejects(
            verifier.block(function, MirBlockId(u32::MAX), CONTEXT),
            "unknown MIR block",
        );
        rejects(
            verifier.normal_block(function, function.unwind, CONTEXT),
            "cleanup block",
        );
        rejects(
            verifier.intrinsic_arguments(int, IntrinsicType::Array, CONTEXT),
            "non-intrinsic",
        );
        rejects(
            verifier.nominal_instance(int, CONTEXT),
            "not a nominal type",
        );
        rejects(
            verifier.type_matches_substitution(invalid, int, &[], CONTEXT),
            "type#",
        );

        let record = verifier
            .resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Record")
            .unwrap()
            .id();
        let missing_callable = MirFunctionId::Callable(HirCallableId::Symbol(record));
        for id in [missing_callable, MirFunctionId::Closure(foreign_closure)] {
            let mut broken = function.clone();
            broken.id = id;
            rejects(verifier.function_is_async(&broken, CONTEXT), "metadata");
            rejects(
                verifier.capability_status(id, int, HirCapability::Copy, CONTEXT),
                "generics",
            );
            rejects(verifier.terminal_status(id, int, CONTEXT), "generics");
            rejects(
                verifier.generic_call_protocols(&broken, 0, int, CONTEXT),
                "metadata",
            );
            rejects(
                verifier.terminal_entry_owners(&broken, CONTEXT),
                "typed HIR",
            );
        }
        rejects(
            verifier.generic_call_protocols(function, u32::MAX, int, CONTEXT),
            "no function binder",
        );

        let mut broken = function.clone();
        broken.parameters.push(broken.parameters[0]);
        rejects(
            verifier.terminal_entry_owners(&broken, CONTEXT),
            "parameter index exceeds",
        );
        broken = function.clone();
        let parameter = broken.parameters[0];
        let MirLocalKind::Parameter { index, .. } = &mut broken.locals[parameter.0 as usize].kind
        else {
            unreachable!()
        };
        *index = u32::MAX;
        rejects(
            verifier.parameter_mode(&broken, parameter, CONTEXT),
            "no matching HIR parameter",
        );

        let closure = verifier.hir.closures().next().unwrap();
        let mut broken = mir.functions[&MirFunctionId::Closure(closure.id())].clone();
        broken.parameters.clear();
        rejects(
            verifier.terminal_entry_owners(&broken, CONTEXT),
            "hidden environment",
        );
        broken = mir.functions[&MirFunctionId::Closure(closure.id())].clone();
        broken.parameters.push(broken.parameters[0]);
        rejects(
            verifier.terminal_entry_owners(&broken, CONTEXT),
            "closure parameter index exceeds",
        );
        let environment = root(&broken);
        rejects(
            verifier.projection_result(
                &broken,
                environment.ty,
                &MirProjection {
                    ty: int,
                    kind: MirProjectionKind::ClosureCapture {
                        closure: foreign_closure,
                        index: 0,
                    },
                },
                CONTEXT,
            ),
            "no HIR metadata",
        );
    });
}

#[test]
fn operand_rejection_preserves_types_copy_modes_and_loan_identity() {
    check_contract(SOURCE, |verifier, mir| {
        let function = fixture(verifier, mir);
        let place = root(function);
        let int = place.ty;
        let string = verifier.hir.interner().scalar(ScalarType::String);
        let callable = callable_named(verifier.resolved, "fixture");
        for (kind, ty, message) in [
            (
                MirOperandKind::Constant(MirConstant::Named(match callable {
                    HirCallableId::Symbol(id) => id,
                    _ => unreachable!(),
                })),
                int,
                "unknown constant",
            ),
            (
                MirOperandKind::Copy(place.clone()),
                string,
                "changes its place type",
            ),
            (
                MirOperandKind::Borrow(place.clone()),
                string,
                "changes its place type",
            ),
            (MirOperandKind::Move(place.clone()), int, "Copy status"),
            (
                MirOperandKind::Loan(MirLoanId(u32::MAX)),
                int,
                "unknown MIR loan",
            ),
            (
                MirOperandKind::Function {
                    callable,
                    arguments: vec![int],
                },
                int,
                "specialization arity",
            ),
            (
                MirOperandKind::Function {
                    callable,
                    arguments: vec![],
                },
                int,
                "does not match its specialization",
            ),
            (
                MirOperandKind::PreludeTraitFunction {
                    method: HirPreludeTraitMethod::Display,
                    arguments: vec![],
                },
                int,
                "specialization arity",
            ),
            (
                MirOperandKind::PreludeTraitFunction {
                    method: HirPreludeTraitMethod::Display,
                    arguments: vec![int],
                },
                int,
                "closed contract",
            ),
        ] {
            rejects(
                verifier.verify_operand(function, &MirOperand { ty, kind }, CONTEXT),
                message,
            );
        }
        let record = verifier
            .resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Record")
            .unwrap()
            .id();
        rejects(
            verifier.verify_operand(
                function,
                &MirOperand {
                    ty: int,
                    kind: MirOperandKind::Function {
                        callable: HirCallableId::Symbol(record),
                        arguments: vec![],
                    },
                },
                CONTEXT,
            ),
            "no HIR signature",
        );
        rejects(
            verifier.verify_constant(&MirConstant::Named(record), int, CONTEXT),
            "literal constant validation",
        );

        let mut broken = function.clone();
        let loan = MirLoanId(0);
        broken.loans[0].kind = MirLoanKind::Region;
        rejects(
            verifier.verify_operand(
                &broken,
                &MirOperand {
                    ty: int,
                    kind: MirOperandKind::Loan(loan),
                },
                CONTEXT,
            ),
            "region loan",
        );
        broken.loans[0].kind = MirLoanKind::CallLocal;
        rejects(
            verifier.verify_operand(
                &broken,
                &MirOperand {
                    ty: string,
                    kind: MirOperandKind::Loan(loan),
                },
                CONTEXT,
            ),
            "reserved place type",
        );
        let mut escaped = place;
        escaped.source_loan = Some(loan);
        rejects(
            verifier.verify_place(&broken, &escaped, CONTEXT),
            "not a region loan",
        );
        broken.loans[0].kind = MirLoanKind::Region;
        broken.loans[0].place.local = broken.return_local;
        rejects(
            verifier.verify_place(&broken, &escaped, CONTEXT),
            "escapes the source region",
        );
    });
}

#[test]
fn region_access_rejects_cycles_inactive_sources_and_shared_mutation() {
    check_contract(SOURCE, |verifier, mir| {
        let mut function = fixture(verifier, mir).clone();
        let loan = MirLoanId(0);
        let place = root(&function);
        function.loans[0].kind = MirLoanKind::Region;
        function.loans[0].place = place.clone();
        let mut access = LocalAccess::from_place(&place);
        access.source_loan = Some(loan);
        let active = BTreeSet::from([loan]);
        rejects(
            verifier.verify_source_loan_access(&function, &active, &access, "move", CONTEXT),
            "transfers content",
        );
        rejects(
            verifier.verify_source_loan_access(
                &function,
                &BTreeSet::new(),
                &access,
                "read",
                CONTEXT,
            ),
            "inactive source",
        );
        rejects(
            verifier.verify_source_loan_access(&function, &active, &access, "write", CONTEXT),
            "shared region",
        );
        verifier
            .verify_source_loan_access(&function, &active, &access, "read", CONTEXT)
            .unwrap();
        function.loans[0].mode = ParameterMode::Mut;
        verifier
            .verify_source_loan_access(&function, &active, &access, "write", CONTEXT)
            .unwrap();
        function.loans[0].place.source_loan = Some(loan);
        rejects(
            verifier.verify_source_loan_access(&function, &active, &access, "read", CONTEXT),
            "cycle",
        );
        let mut cyclic = place.clone();
        cyclic.source_loan = Some(loan);
        rejects(
            verifier.place_source_chain(&function, &cyclic, CONTEXT),
            "cycle",
        );
        function.loans[0].place.source_loan = None;
        function.loans[0].kind = MirLoanKind::CallLocal;
        rejects(
            verifier.verify_source_loan_access(&function, &active, &access, "read", CONTEXT),
            "not a region loan",
        );

        let observed =
            &mir.functions[&MirFunctionId::Callable(callable_named(verifier.resolved, "observe"))];
        let observed_access = LocalAccess::from_place(&root(observed));
        for (kind, message) in [
            ("move", "borrowed parameter"),
            ("write", "shared `ref` parameter"),
        ] {
            verifier
                .verify_loan_local_access(
                    observed,
                    &BTreeMap::new(),
                    &BTreeSet::new(),
                    &LocalEvent::Read(observed_access.clone()),
                    None,
                    CONTEXT,
                )
                .unwrap();
            // Classify each actual access through the same entry used by flow verification.
            let event = if kind == "move" {
                LocalEvent::Move(observed_access.clone())
            } else {
                LocalEvent::WriteAccess(observed_access.clone())
            };
            rejects(
                verifier.verify_loan_local_access(
                    observed,
                    &BTreeMap::new(),
                    &BTreeSet::new(),
                    &event,
                    None,
                    CONTEXT,
                ),
                message,
            );
        }
    });
}

#[test]
fn runtime_overlap_proofs_expire_when_any_bound_input_changes() {
    check_contract(SOURCE, |verifier, mir| {
        let mut function = fixture(verifier, mir).clone();
        let input = function.parameters[0];
        let mut place = root(&function);
        place.projections.push(MirProjection {
            ty: place.ty,
            kind: MirProjectionKind::Index {
                index: input,
                access: HirIndexAccess::Array,
            },
        });
        let access = LocalAccess::from_place(&place);
        for proof_kind in ["loan", "read", "write"] {
            let mut state = LoanFlowState::default();
            if proof_kind == "loan" {
                function.loans[0].place = place.clone();
                state.validated.insert(MirLoanId(0), vec![]);
            } else {
                state.accesses.insert(
                    ValidatedAccess {
                        access: access.clone(),
                        for_write: proof_kind == "write",
                    },
                    vec![],
                );
            }
            for event in [
                LocalEvent::Move(LocalAccess::from_place(&root(&function))),
                LocalEvent::Write(LocalAccess::from_place(&root(&function))),
                LocalEvent::WriteAccess(LocalAccess::from_place(&root(&function))),
                LocalEvent::StorageLive(input),
                LocalEvent::StorageDead(input),
            ] {
                rejects(
                    verifier.verify_runtime_proof_inputs_stable(&function, &state, &event, CONTEXT),
                    "pending runtime-overlap proof",
                );
            }
            verifier
                .verify_runtime_proof_inputs_stable(
                    &function,
                    &state,
                    &LocalEvent::Read(access.clone()),
                    CONTEXT,
                )
                .unwrap();
            verifier
                .verify_runtime_proof_inputs_stable(
                    &function,
                    &state,
                    &LocalEvent::Resolve(access.clone()),
                    CONTEXT,
                )
                .unwrap();
        }
        let state = LoanFlowState {
            validated: BTreeMap::from([(MirLoanId(u32::MAX), vec![])]),
            ..LoanFlowState::default()
        };
        rejects(
            verifier.verify_runtime_proof_inputs_stable(
                &function,
                &state,
                &LocalEvent::StorageDead(input),
                CONTEXT,
            ),
            "unknown MIR loan",
        );
    });
}

#[test]
fn loan_events_require_single_reservation_and_ordered_release() {
    check_contract(SOURCE, |verifier, mir| {
        let mut function = fixture(verifier, mir).clone();
        let loan = MirLoanId(0);
        let apply = |state: &mut LoanFlowState, event: &LoanEvent| {
            verifier.apply_loan_event(&function, &BTreeMap::new(), state, event, CONTEXT)
        };
        let mut state = LoanFlowState::default();
        rejects(apply(&mut state, &LoanEvent::Release(loan)), "inactive");
        rejects(
            apply(&mut state, &LoanEvent::Consume(vec![loan])),
            "inactive",
        );
        rejects(
            apply(&mut state, &LoanEvent::Reserve(MirLoanId(u32::MAX))),
            "unknown MIR loan",
        );
        apply(&mut state, &LoanEvent::Reserve(loan)).unwrap();
        rejects(
            apply(&mut state, &LoanEvent::Reserve(loan)),
            "already-active",
        );
        let mut repeated = state.clone();
        rejects(
            apply(&mut repeated, &LoanEvent::Consume(vec![loan, loan])),
            "inactive",
        );
        apply(&mut state, &LoanEvent::Consume(vec![loan])).unwrap();
        assert!(state.active.is_empty());

        for pending_access in [false, true] {
            let mut state = LoanFlowState::default();
            if pending_access {
                state.accesses.insert(
                    ValidatedAccess {
                        access: LocalAccess::from_place(&root(&function)),
                        for_write: false,
                    },
                    vec![],
                );
            } else {
                state.validated.insert(loan, vec![]);
            }
            rejects(
                apply(&mut state, &LoanEvent::Release(loan)),
                "proof is pending",
            );
            rejects(
                apply(&mut state, &LoanEvent::Consume(vec![loan])),
                "proof is pending",
            );
            rejects(
                apply(&mut state, &LoanEvent::Reserve(loan)),
                if pending_access {
                    "access proof is pending"
                } else {
                    "required validation"
                },
            );
        }
        function.loans[0].kind = MirLoanKind::Region;
        let mut state = LoanFlowState {
            active: BTreeSet::from([loan]),
            ..LoanFlowState::default()
        };
        rejects(
            verifier.apply_loan_event(
                &function,
                &BTreeMap::new(),
                &mut state,
                &LoanEvent::Consume(vec![loan]),
                CONTEXT,
            ),
            "consumes region",
        );
        let mut dependent = function.loans[0].clone();
        dependent.place.source_loan = Some(loan);
        function.loans.push(dependent);
        state.active.insert(MirLoanId(1));
        rejects(
            verifier.apply_loan_event(
                &function,
                &BTreeMap::new(),
                &mut state,
                &LoanEvent::Release(loan),
                CONTEXT,
            ),
            "dependent loan",
        );
        verifier
            .apply_loan_event(
                &function,
                &BTreeMap::new(),
                &mut state,
                &LoanEvent::Release(MirLoanId(1)),
                CONTEXT,
            )
            .unwrap();
        verifier
            .apply_loan_event(
                &function,
                &BTreeMap::new(),
                &mut state,
                &LoanEvent::Release(loan),
                CONTEXT,
            )
            .unwrap();
        assert_eq!(state, LoanFlowState::default());
    });
}

#[test]
fn call_associations_reject_missing_duplicate_and_nonvariadic_arguments() {
    use crate::hir::HirCallArgumentTarget;
    use crate::mir::MirCallArgument;
    check_contract(SOURCE, |verifier, mir| {
        let function = fixture(verifier, mir);
        let callable = callable_named(verifier.resolved, "observe");
        let signature = verifier.hir.callable(callable).unwrap().function_type();
        let int = verifier.hir.interner().scalar(ScalarType::Int);
        let string = verifier.hir.interner().scalar(ScalarType::String);
        let callee = MirOperand {
            ty: signature,
            kind: MirOperandKind::Function {
                callable,
                arguments: vec![],
            },
        };
        let argument = MirCallArgument {
            mode: ParameterMode::Ref,
            target: HirCallArgumentTarget::Fixed(0),
            value: MirOperand {
                ty: int,
                kind: MirOperandKind::Loan(MirLoanId(0)),
            },
        };
        let check =
            |arguments: &[MirCallArgument], signature, outcome, context, protocol, unsafe_call| {
                verifier.verify_call(
                    function,
                    MirCallVerification {
                        callee: &callee,
                        arguments,
                        signature,
                        protocol,
                        unsafe_call,
                        outcome,
                    },
                    context,
                    CONTEXT,
                )
            };
        check(
            std::slice::from_ref(&argument),
            signature,
            int,
            MirOperationContext::Immediate,
            HirCallProtocol::Call,
            false,
        )
        .unwrap();
        rejects(
            check(
                &[],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "fixed parameters",
        );
        rejects(
            check(
                &[argument.clone(), argument.clone()],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "more than once",
        );
        rejects(
            check(
                &[],
                int,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "not a function",
        );
        rejects(
            check(
                &[],
                signature,
                string,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "outcome differs",
        );
        rejects(
            check(
                &[],
                signature,
                int,
                MirOperationContext::Await,
                HirCallProtocol::Call,
                false,
            ),
            "effects differ",
        );
        rejects(
            check(
                &[],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                true,
            ),
            "effects differ",
        );
        rejects(
            check(
                &[],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::CallOnce,
                false,
            ),
            "closed callee contract",
        );
        for target in [
            HirCallArgumentTarget::Receiver,
            HirCallArgumentTarget::Fixed(9),
            HirCallArgumentTarget::VariadicElement,
            HirCallArgumentTarget::Invalid,
            HirCallArgumentTarget::VariadicSpread,
        ] {
            let mut invalid = argument.clone();
            invalid.target = target;
            rejects(
                check(
                    &[invalid],
                    signature,
                    int,
                    MirOperationContext::Immediate,
                    HirCallProtocol::Call,
                    false,
                ),
                if target == HirCallArgumentTarget::VariadicSpread {
                    "non-variadic"
                } else {
                    "has no parameter"
                },
            );
        }
        for (mode, ty) in [(ParameterMode::Value, int), (ParameterMode::Ref, string)] {
            let mut invalid = argument.clone();
            invalid.mode = mode;
            invalid.value.ty = ty;
            rejects(
                check(
                    &[invalid],
                    signature,
                    int,
                    MirOperationContext::Immediate,
                    HirCallProtocol::Call,
                    false,
                ),
                "mode or type",
            );
        }
        let mut invalid = argument.clone();
        invalid.value.kind = MirOperandKind::Copy(root(function));
        rejects(
            check(
                &[invalid],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "loan access",
        );
        let mut spread = argument.clone();
        spread.target = HirCallArgumentTarget::VariadicSpread;
        rejects(
            check(
                &[spread, argument],
                signature,
                int,
                MirOperationContext::Immediate,
                HirCallProtocol::Call,
                false,
            ),
            "final argument",
        );
    });
}

#[test]
fn loan_flow_census_rejects_unknown_missing_and_repeated_protocol_steps() {
    check_contract(SOURCE, |verifier, mir| {
        let function = fixture(verifier, mir);
        for (event, message) in [
            (
                MirStatementKind::ReserveLoan(MirLoanId(u32::MAX)),
                "reserves an unknown",
            ),
            (MirStatementKind::ReserveLoan(MirLoanId(0)), "validations"),
        ] {
            let mut broken = function.clone();
            let span = broken.span;
            broken.blocks[broken.entry.0 as usize]
                .statements
                .insert(0, MirStatement { span, kind: event });
            rejects(
                verifier.verify_loan_flow(&broken, &vec![true; broken.blocks.len()], CONTEXT),
                message,
            );
        }
        let mut broken = function.clone();
        for block in &mut broken.blocks {
            block
                .statements
                .retain(|statement| !matches!(statement.kind, MirStatementKind::ReserveLoan(_)));
        }
        rejects(
            verifier.verify_loan_flow(&broken, &vec![true; broken.blocks.len()], CONTEXT),
            "reservations",
        );

        let mut broken = function.clone();
        let entry = broken.entry.0 as usize;
        broken.blocks[entry].terminator.kind = MirTerminatorKind::ValidateLoan {
            loan: MirLoanId(u32::MAX),
            against: vec![],
            target: broken.entry,
            unwind: broken.unwind,
        };
        rejects(
            verifier.verify_loan_flow(&broken, &vec![true; broken.blocks.len()], CONTEXT),
            "validates an unknown",
        );
    });
}

#[test]
fn defer_flow_requires_live_guards_and_immediate_owner_transitions() {
    use crate::mir::{MirRvalue, MirRvalueKind, MirStatement};
    const SOURCE: &str =
        "fn cleanup() {}\n fn fixture(value: Int): Int {\n defer cleanup()\n value\n}\n";
    check_contract(SOURCE, |verifier, mir| {
        let function = fixture(verifier, mir);
        let registration = function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| matches!(statement.kind, MirStatementKind::RegisterDefer { .. }))
            .unwrap()
            .clone();
        let mut guarded = registration.clone();
        let owner = root(function);
        let MirStatementKind::RegisterDefer { guard, .. } = &mut guarded.kind else {
            unreachable!()
        };
        *guard = Some(owner.clone());
        let test_flow = |statements: Vec<MirStatement>, terminator: MirTerminatorKind| {
            let mut broken = function.clone();
            for block in &mut broken.blocks {
                block.statements.clear();
                block.terminator.kind = MirTerminatorKind::Unreachable;
            }
            let entry = &mut broken.blocks[broken.entry.0 as usize];
            entry.statements = statements;
            entry.terminator.kind = terminator;
            verifier.verify_defer_flow(&broken, CONTEXT)
        };
        for end in [
            MirTerminatorKind::Return,
            MirTerminatorKind::ResumePanic,
            MirTerminatorKind::Goto {
                target: function.entry,
            },
        ] {
            let message = if matches!(
                end,
                MirTerminatorKind::Return | MirTerminatorKind::ResumePanic
            ) {
                "abandons"
            } else {
                "re-executed"
            };
            rejects(test_flow(vec![guarded.clone()], end), message);
        }
        rejects(
            test_flow(
                vec![guarded.clone(), guarded.clone()],
                MirTerminatorKind::Unreachable,
            ),
            "already-active guard",
        );
        let statement = |kind| MirStatement {
            span: function.span,
            kind,
        };
        for storage in [
            MirStatementKind::StorageLive(owner.local),
            MirStatementKind::StorageDead(owner.local),
        ] {
            rejects(
                test_flow(
                    vec![guarded.clone(), statement(storage)],
                    MirTerminatorKind::Unreachable,
                ),
                "storage lifetime",
            );
        }
        let retarget = MirStatementKind::RetargetCleanup {
            from: owner.clone(),
            to: owner.clone(),
        };
        rejects(
            test_flow(
                vec![guarded.clone(), statement(retarget)],
                MirTerminatorKind::Unreachable,
            ),
            "immediate move",
        );
        rejects(
            test_flow(
                vec![
                    guarded.clone(),
                    statement(MirStatementKind::DisarmCleanup(owner.clone())),
                ],
                MirTerminatorKind::Unreachable,
            ),
            "disarmed without",
        );
        let overwrite = MirStatementKind::Assign {
            destination: owner.clone(),
            value: MirRvalue {
                ty: owner.ty,
                kind: MirRvalueKind::Use(MirOperand {
                    ty: owner.ty,
                    kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                }),
            },
        };
        rejects(
            test_flow(
                vec![guarded.clone(), statement(overwrite)],
                MirTerminatorKind::Unreachable,
            ),
            "overwrites",
        );
        let MirStatementKind::RegisterDefer { scope, .. } = registration.kind else {
            unreachable!()
        };
        rejects(
            test_flow(
                vec![],
                MirTerminatorKind::DrainDefers {
                    scopes: vec![scope],
                    target: function.entry,
                    unwind: function.unwind,
                },
            ),
            "no registration",
        );
        // A fallback cannot claim ownership already held by an explicit guard.
        rejects(
            test_flow(
                vec![
                    guarded,
                    statement(MirStatementKind::RegisterFallback { scope, owner }),
                ],
                MirTerminatorKind::Unreachable,
            ),
            "overlaps an active explicit",
        );
        test_flow(vec![registration], MirTerminatorKind::Unreachable).unwrap();
    });
}
