//! Proves that a source selectable adapter has no externally visible prefix.
//! A selectable call or Join await is the first commit checkpoint. Branches
//! retain the uncommitted state if any reachable path can bypass that checkpoint.

use std::collections::BTreeSet;

use crate::resolve::LocalId;
use crate::source::Span;
use crate::types::TypeKind;

use super::{
    HirAssignmentTarget, HirAssignmentTargetKind, HirBootstrapHostFunction as Host, HirCallableId,
    HirExpressionId, HirExpressionKind as Expr, HirForKind, HirPatternKind, HirProgram,
    HirStatement as Stmt,
};

pub(super) fn findings(program: &HirProgram) -> Vec<Span> {
    let selectable = |ty| matches!(program.interner.kind(ty), Ok(TypeKind::Function(signature)) if signature.is_selectable());
    let roots = program
        .callables
        .iter()
        .filter(|callable| selectable(callable.function_type()))
        .filter_map(|callable| program.body(callable.id()).map(|body| body.root()))
        .chain(
            program
                .closures()
                .filter(|closure| selectable(closure.function_type()))
                .map(|closure| closure.body().root()),
        );
    roots
        .filter_map(|root| {
            let mut proof = Proof {
                program,
                steps: program.expressions.len().saturating_mul(4).max(64),
                calls: BTreeSet::new(),
                private: BTreeSet::new(),
            };
            proof.expression(root, true, true, 0).err()
        })
        .collect()
}

struct Proof<'a> {
    program: &'a HirProgram,
    steps: usize,
    calls: BTreeSet<HirCallableId>,
    private: BTreeSet<LocalId>,
}

impl Proof<'_> {
    fn expression(
        &mut self,
        id: HirExpressionId,
        mut pending: bool,
        checkpoints: bool,
        depth: usize,
    ) -> Result<bool, Span> {
        let expression = self.program.expression(id).expect("checked expression");
        let span = expression.span();
        if !pending {
            return Ok(false);
        }
        if self.steps == 0 || depth >= 256 {
            return Err(span);
        }
        self.steps -= 1;
        match expression.kind() {
            Expr::Block {
                statements, tail, ..
            } => {
                for statement in statements {
                    if !pending {
                        break;
                    }
                    match statement {
                        Stmt::Binding { value, pattern, .. } => {
                            pending = self.expression(*value, pending, checkpoints, depth + 1)?;
                            self.bind_private(*pattern);
                        }
                        Stmt::Expression { value, .. } | Stmt::Discard { value, .. } => {
                            pending = self.expression(*value, pending, checkpoints, depth + 1)?
                        }
                        Stmt::Assignment { target, value, .. } => {
                            if !self.private_target(target) {
                                return Err(statement.span());
                            }
                            pending = self.expression(*value, pending, checkpoints, depth + 1)?;
                        }
                        Stmt::Defer { action, .. } => {
                            self.expression(action.expression(), true, false, depth + 1)?;
                        }
                        Stmt::For { kind, body, .. } => {
                            match kind {
                                HirForKind::Conditional { condition } => {
                                    pending = self.expression(
                                        *condition,
                                        pending,
                                        checkpoints,
                                        depth + 1,
                                    )?;
                                }
                                HirForKind::Iterate {
                                    source, protocol, ..
                                } => {
                                    if matches!(protocol, super::HirIterationProtocol::Trait { .. })
                                    {
                                        return Err(statement.span());
                                    }
                                    pending =
                                        self.expression(*source, pending, checkpoints, depth + 1)?;
                                }
                                HirForKind::Infinite => {}
                            }
                            self.expression(*body, pending, checkpoints, depth + 1)?;
                            // The loop can execute zero iterations or break
                            // before the checkpoint; do not promote its body.
                        }
                    }
                }
                if let Some(tail) = tail {
                    pending = self.expression(*tail, pending, checkpoints, depth + 1)?;
                }
                Ok(pending)
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                pending = self.expression(*condition, pending, checkpoints, depth + 1)?;
                let left = self.expression(*then_branch, pending, checkpoints, depth + 1)?;
                let right = match else_branch {
                    Some(other) => self.expression(*other, pending, checkpoints, depth + 1)?,
                    None => pending,
                };
                Ok(left || right)
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                pending = self.expression(*scrutinee, pending, checkpoints, depth + 1)?;
                let mut remaining = false;
                for arm in arms {
                    let guarded = match arm.guard() {
                        Some(guard) => self.expression(guard, pending, checkpoints, depth + 1)?,
                        None => pending,
                    };
                    remaining |= self.expression(arm.body(), guarded, checkpoints, depth + 1)?;
                }
                Ok(remaining)
            }
            Expr::AsyncCall {
                callee,
                arguments,
                signature,
                ..
            }
            | Expr::Call {
                callee,
                arguments,
                signature,
                ..
            } => {
                pending = self.expression(*callee, pending, checkpoints, depth + 1)?;
                for argument in arguments {
                    pending = self.expression(argument.value(), pending, checkpoints, depth + 1)?;
                }
                if !pending {
                    return Ok(false);
                }
                let Ok(TypeKind::Function(signature)) = self.program.interner.kind(*signature)
                else {
                    return Err(span);
                };
                if signature.is_selectable() && checkpoints {
                    return Ok(false);
                }
                if signature.is_async() {
                    return Err(span);
                }
                let callee = self.program.expression(*callee).expect("checked callee");
                let callable = match callee.kind() {
                    Expr::Function(callable) | Expr::SpecializedFunction { callable, .. } => {
                        *callable
                    }
                    _ => return Err(span),
                };
                if let HirCallableId::Host(host) = callable {
                    return if pure_host(host) {
                        Ok(pending)
                    } else {
                        Err(span)
                    };
                }
                let Some(body) = self.program.body(callable) else {
                    return Err(span);
                };
                if !self.calls.insert(callable) {
                    return Err(span);
                }
                // An ordinary helper cannot commit its caller's selection.
                let result = self.expression(body.root(), true, false, depth + 1);
                self.calls.remove(&callable);
                result.map(|_| pending)
            }
            Expr::BootstrapHostCall {
                function,
                arguments,
            } => {
                for argument in arguments {
                    pending = self.expression(*argument, pending, checkpoints, depth + 1)?;
                }
                if !pending || pure_host(*function) {
                    Ok(pending)
                } else {
                    Err(span)
                }
            }
            Expr::InterpolatedString { values, .. } => {
                for value in values {
                    pending = self.expression(*value, pending, checkpoints, depth + 1)?;
                    if pending
                        && !matches!(
                            self.program.interner.kind(
                                self.program
                                    .expression(*value)
                                    .expect("checked interpolation")
                                    .ty()
                            ),
                            Ok(TypeKind::Scalar(_))
                        )
                    {
                        return Err(span);
                    }
                }
                Ok(pending)
            }
            Expr::Await { operation } => {
                self.expression(*operation, pending, checkpoints, depth + 1)?;
                if checkpoints { Ok(false) } else { Err(span) }
            }
            Expr::PreludePanic { message } => {
                self.expression(*message, pending, checkpoints, depth + 1)?;
                // Panic is an observable structured failure, never a losing
                // value arm. The selector gives ready panic precedence.
                Ok(false)
            }
            Expr::Spawn { .. }
            | Expr::Select { .. }
            | Expr::MapRemove { .. }
            | Expr::ArraySequence { .. } => Err(span),
            Expr::Return { value } => {
                if let Some(value) = value {
                    self.expression(*value, pending, checkpoints, depth + 1)?;
                }
                Ok(false)
            }
            Expr::Fail { error } => {
                self.expression(*error, pending, checkpoints, depth + 1)?;
                Ok(false)
            }
            Expr::Binary {
                operator: super::HirBinaryOperator::LogicalAnd | super::HirBinaryOperator::LogicalOr,
                left,
                right,
            } => {
                pending = self.expression(*left, pending, checkpoints, depth + 1)?;
                let other = self.expression(*right, pending, checkpoints, depth + 1)?;
                Ok(pending || other)
            }
            _ => {
                for child in super::verify::expression_children(expression) {
                    pending = self.expression(child, pending, checkpoints, depth + 1)?;
                }
                Ok(pending)
            }
        }
    }

    fn bind_private(&mut self, id: super::HirPatternId) {
        match self.program.pattern(id).expect("checked pattern").kind() {
            HirPatternKind::Binding(local) => {
                self.private.insert(*local);
            }
            HirPatternKind::Tuple(items) => {
                for item in items {
                    self.bind_private(*item);
                }
            }
            _ => {}
        }
    }

    fn private_target(&self, target: &HirAssignmentTarget) -> bool {
        match target.kind() {
            HirAssignmentTargetKind::Discard => true,
            HirAssignmentTargetKind::Tuple(items) => {
                items.iter().all(|item| self.private_target(item))
            }
            HirAssignmentTargetKind::Place { place, .. } => {
                // Only a local scalar replacement is proved here. Following a
                // projection might reach a borrowed or shared representation.
                matches!(self.program.expression(*place).map(|expression| expression.kind()), Some(Expr::Local(local)) if self.private.contains(local))
            }
        }
    }
}

fn pure_host(host: Host) -> bool {
    if let Host::Network(operation) = host {
        return operation.reversible_setup();
    }
    matches!(
        host,
        Host::UuidNil
            | Host::UuidMax
            | Host::UuidParse
            | Host::UuidFromBytes
            | Host::UuidToBytes
            | Host::UuidToString
            | Host::UuidVersion
            | Host::UuidVariant
            | Host::UuidIsNil
            | Host::UuidIsMax
            | Host::UuidCompare
            | Host::UuidV5
            | Host::IoLimitsDefault
            | Host::IoLimitsNew
    )
}
