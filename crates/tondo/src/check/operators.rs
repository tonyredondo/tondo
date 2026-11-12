//! Unary and binary operators, arithmetic and `if`.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn unary(
        &mut self,
        cx: &mut FnCx,
        op: UnaryOp,
        operand: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        match op {
            UnaryOp::Neg => {
                if let ast::ExprKind::Int { value, suffix } = operand.kind {
                    let literal = self.int_literal(true, value, suffix, expected, span);
                    if self.types.as_int(literal.ty).is_some_and(|i| !i.signed()) {
                        let message = format!("negation is not defined for {}", self.display(literal.ty));
                        self.diags.error("E1102", span, message);
                    }
                    return literal;
                }
                if let ast::ExprKind::Float { text, suffix } = &operand.kind {
                    return self.float_literal(text, *suffix, true, expected, span);
                }
                if let ast::ExprKind::BigInt(digits) = &operand.kind {
                    return self.big_literal(&format!("-{digits}"), expected, span);
                }
                let value = self.expr(cx, operand, expected);
                let negatable = match self.types.kind(value.ty) {
                    TyKind::Int(int) => int.signed(),
                    TyKind::Float(_) | TyKind::Error | TyKind::Never => true,
                    _ => false,
                };
                if !negatable {
                    let shown = self.types.display(value.ty);
                    let mut message = format!("negation is not defined for {}", self.display(value.ty));
                    if self.number.is_some_and(|n| self.satisfies(value.ty, n, &[])) {
                        message.push_str(": write `0 - value`");
                    } else if self.additive.is_some_and(|a| self.satisfies(value.ty, a, &[])) {
                        message.push_str(&format!(": write `Additive.zero[{shown}]() - value`"));
                    }
                    self.diags.error("E1102", span, message);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
            UnaryOp::Not => {
                let value = self.expr(cx, operand, Some(Ty::BOOL));
                let value = self.coerce(value, Ty::BOOL);
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), Ty::BOOL, span)
            }
            UnaryOp::BitNot => {
                let value = self.expr(cx, operand, expected);
                if value.ty != Ty::BYTE && !self.bound_by(value.ty, self.integer) {
                    self.require_int(&value);
                }
                let ty = value.ty;
                Self::mk(hir::ExprKind::Unary(op, Box::new(value)), ty, span)
            }
        }
    }

    /// Whether an expression takes its type from the context: a literal
    /// without suffix, `none`, an empty collection or a construction of a
    /// generic enum. Inference checks such arguments last.
    pub(super) fn needs_context(&self, e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::None | ast::ExprKind::Construct(..) | ast::ExprKind::Map(_) => true,
            ast::ExprKind::Array(items) => items.is_empty() || items.iter().all(|item| self.needs_context(item)),
            ast::ExprKind::Path(segments) | ast::ExprKind::Record { path: segments, .. } => {
                matches!(self.type_names.get(&segments[0].name), Some(TypeName::Template(_)))
            }
            ast::ExprKind::Call { callee, .. } => self.needs_context(callee),
            // A closure whose parameters take their types from the call.
            ast::ExprKind::Closure(closure) => closure.params.iter().any(|p| p.ty.is_none()),
            _ => Self::is_untyped_literal(e),
        }
    }

    pub(super) fn is_untyped_literal(e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Int { suffix: None, .. } | ast::ExprKind::Float { suffix: None, .. } => true,
            ast::ExprKind::Unary(UnaryOp::Neg, inner) => Self::is_untyped_literal(inner),
            _ => false,
        }
    }

    /// The numbers inside nested arrays, and how many arrays deep they are.
    pub(super) fn number_leaf(&self, ty: Ty) -> (Ty, u32) {
        match *self.types.kind(ty) {
            TyKind::Array(element) => {
                let (leaf, depth) = self.number_leaf(element);
                (leaf, depth + 1)
            }
            _ => (ty, 0),
        }
    }

    /// What an operand of arithmetic is expected to be given its peer: the
    /// peer's number type in the operand's own literal shape (10.6). Only
    /// literals take an expectation; other operands have their own type.
    pub(super) fn arithmetic_expected(&mut self, operand: &ast::Expr, peer: Ty) -> Option<Ty> {
        match &operand.kind {
            ast::ExprKind::Array(items) => match items.first() {
                Some(first) => {
                    let inner = self.arithmetic_expected(first, peer)?;
                    Some(self.types.intern(TyKind::Array(inner)))
                }
                None => Some(peer),
            },
            _ if Self::is_untyped_literal(operand) => Some(self.number_leaf(peer).0),
            _ => None,
        }
    }

    /// Types the operands of an arithmetic operator. Without arrays they
    /// share a type, as in `operands`; with arrays only their numbers must.
    pub(super) fn arithmetic_operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
    ) -> (hir::Expr, hir::Expr) {
        let (left, right) = if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let shape = self.arithmetic_expected(left, right.ty);
            (self.expr(cx, left, shape), right)
        } else {
            let left = self.expr(cx, left, expected);
            let shape = self.arithmetic_expected(right, left.ty).or(Some(left.ty));
            let right = self.expr(cx, right, shape);
            (left, right)
        };
        if self.number_leaf(left.ty).1 > 0 || self.number_leaf(right.ty).1 > 0 || left.ty == Ty::NEVER {
            return (left, right);
        }
        // A left operand that is not a number: the operator's error says
        // so, without another for the right operand. A duration and a
        // `T: Integer` compute as numbers do (18.10).
        let computes = matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Float(_) | TyKind::Error)
            || Some(left.ty) == self.duration
            || self.bound_by(left.ty, self.integer);
        if !computes {
            return (left, right);
        }
        self.common(left, right)
    }

    /// Two operands of one type: the right one coerced to the left's, or
    /// the narrower number widened to the other's (8.6).
    pub(super) fn common(&mut self, left: hir::Expr, right: hir::Expr) -> (hir::Expr, hir::Expr) {
        if matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Float(_)) && self.converts(left.ty, right.ty) {
            let ty = right.ty;
            return (self.coerce(left, ty), right);
        }
        let ty = left.ty;
        (left, self.coerce(right, ty))
    }

    /// The type of `left op right` for `+ - * / %`, lifted over arrays
    /// (10.6): arrays pair element by element and a number pairs with every
    /// element. Reports operands that do not combine.
    pub(super) fn arithmetic(&mut self, op: BinOp, left: Ty, right: Ty, span: Span) -> Ty {
        let ((number, left_depth), (right_number, right_depth)) = (self.number_leaf(left), self.number_leaf(right));
        if [number, right_number].iter().any(|&t| matches!(self.types.kind(t), TyKind::Error | TyKind::Never)) {
            return if left_depth >= right_depth { left } else { right };
        }
        // Floats have no `%` operator; their remainder is a named operation (17.4).
        // A duration adds and subtracts, and a `T: Integer` computes as an
        // integer does (18.10).
        let numeric = match self.types.kind(number) {
            TyKind::Int(_) => true,
            TyKind::Float(_) => op != BinOp::Rem,
            _ if Some(number) == self.duration => {
                matches!(op, BinOp::Add | BinOp::Sub) && left_depth + right_depth == 0
            }
            _ => self.bound_by(number, self.integer),
        };
        if !numeric {
            let mut message = format!("operator `{}` cannot be applied to {}", op.as_str(), self.display(left));
            let takes = match op {
                BinOp::Add | BinOp::Sub => Some("Additive"),
                BinOp::Mul | BinOp::Div => Some("Number"),
                _ => None,
            };
            if let Some(takes) = takes
                && left_depth == 0
            {
                if self.is_abstract(left) {
                    let shown = self.types.display(left);
                    message.push_str(&format!(": add the bound `{shown}: {takes}`, which gives it `{}`", op.as_str()));
                } else if matches!(self.types.kind(left), TyKind::Adt(_)) {
                    message
                        .push_str(&format!(": implement `{takes}` for it, which gives it `{}` (18.10)", op.as_str()));
                }
            }
            match (op, left_depth, right_depth) {
                (BinOp::Add, _, _) if left == Ty::STRING => {
                    message.push_str(": use interpolation, `\"{a}{b}\"`, or `a.concat(b)`")
                }
                (BinOp::Add, 1.., 1..) => message.push_str(": to join arrays use `a.concat(b)`"),
                (BinOp::Mul, 1.., 0) if right == Ty::INT => message.push_str(": to repeat an array use `a.repeat(n)`"),
                (BinOp::Mul, 0, 0) if left == Ty::STRING => message.push_str(": to repeat a string use `text.repeat(n)`"),
                (BinOp::Rem, 0, 0) if matches!(self.types.kind(left), TyKind::Float(_)) => message.push_str(
                    ": floats have no `%`, since remainders follow several conventions; the truncated one is `a - b * (a / b).truncate()`",
                ),
                _ if matches!(self.types.kind(left), TyKind::Result(..) | TyKind::Option(_)) => message.push_str(
                    ": the value may be missing or an error; get the number first with `?`, `?? fallback` or `match`",
                ),
                _ => {}
            }
            self.diags.error("E1102", span, message);
            return Ty::ERROR;
        }
        if number != right_number {
            // Without arrays, `arithmetic_operands` already reported this.
            if left_depth > 0 || right_depth > 0 {
                let message = format!(
                    "operator `{}` cannot combine {} and {}: their numbers must have the same type",
                    op.as_str(),
                    self.display(left),
                    self.display(right)
                );
                self.diags.error("E1102", span, message);
            }
            return Ty::ERROR;
        }
        self.lift(left, right)
    }

    pub(super) fn lift(&mut self, left: Ty, right: Ty) -> Ty {
        let element = |ty: Ty| match self.types.kind(ty) {
            TyKind::Array(element) => Some(*element),
            _ => None,
        };
        let inner = match (element(left), element(right)) {
            (Some(a), Some(b)) => self.lift(a, b),
            (Some(a), None) => self.lift(a, right),
            (None, Some(b)) => self.lift(left, b),
            (None, None) => return left,
        };
        self.types.intern(TyKind::Array(inner))
    }

    /// Types two operands that must share a type. An unsuffixed literal on
    /// the left adopts the right operand's type; evaluation order is unchanged.
    /// Two operands of one type; `comparison` says that a mismatch is one.
    pub(super) fn operands(
        &mut self,
        cx: &mut FnCx,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
        comparison: bool,
    ) -> (hir::Expr, hir::Expr) {
        if Self::is_untyped_literal(left) && !Self::is_untyped_literal(right) {
            let right = self.expr(cx, right, expected);
            let left = self.expr(cx, left, Some(right.ty));
            let outer = std::mem::replace(&mut self.comparing, comparison.then_some(true));
            let left = self.coerce(left, right.ty);
            self.comparing = outer;
            (left, right)
        } else {
            let left = self.expr(cx, left, expected);
            let right = self.expr(cx, right, Some(left.ty));
            if left.ty == Ty::NEVER {
                return (left, right);
            }
            let outer = std::mem::replace(&mut self.comparing, comparison.then_some(false));
            let operands = self.common(left, right);
            self.comparing = outer;
            operands
        }
    }

    /// Warns of a comparison with a constant that is NaN (`W1008`).
    pub(super) fn nan_comparison(&mut self, left: &hir::Expr, right: &hir::Expr, span: Span) {
        if [left, right].into_iter().any(|side| fold_float(&self.types, side).is_some_and(f64::is_nan)) {
            let message = "NaN is not equal to any value, itself included, nor less or greater than one: test it with `value.isNaN()`";
            self.diags.warning("W1008", span, message);
        }
    }

    pub(super) fn binary(
        &mut self,
        cx: &mut FnCx,
        op: BinOp,
        left: &ast::Expr,
        right: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let operator_error = |this: &mut Self, ty: Ty| {
            let mut message = format!("operator `{}` cannot be applied to {}", op.as_str(), this.display(ty));
            if let (BinOp::Eq | BinOp::Ne, TyKind::Handle(Handle::Weak, _)) = (op, this.types.kind(ty)) {
                message.push_str(": compare the references they give, `left.get() == right.get()`");
            }
            this.diags.error("E1102", span, message);
        };
        let (left, right, ty) = match op {
            BinOp::And | BinOp::Or => {
                let left = self.expr(cx, left, Some(Ty::BOOL));
                let left = self.coerce(left, Ty::BOOL);
                let right = self.expr(cx, right, Some(Ty::BOOL));
                let right = self.coerce(right, Ty::BOOL);
                (left, right, Ty::BOOL)
            }
            BinOp::Shl | BinOp::Shr => {
                let left = self.expr(cx, left, expected);
                if left.ty != Ty::BYTE && !self.bound_by(left.ty, self.integer) {
                    self.require_int(&left);
                }
                let right = self.expr(cx, right, None);
                self.require_int(&right);
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let (left, right) = self.arithmetic_operands(cx, left, right, expected);
                // The trait's method, on a type that takes the operator by one (18.10).
                if let Some((trait_id, index)) = self.operator_trait(op, left.ty) {
                    let ty = left.ty;
                    let right = self.coerce(right, ty);
                    let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![left, right]);
                    return Self::mk(call, ty, span);
                }
                let ty = self.arithmetic(op, left.ty, right.ty, span);
                (left, right, ty)
            }
            // `Command | Pipeline` composes process plans (17.6).
            BinOp::BitOr if !Self::is_untyped_literal(left) => {
                let plan = |this: &Self, ty: Ty| {
                    matches!(this.types.kind(ty), TyKind::Handle(Handle::Command | Handle::Pipeline, _))
                };
                let hint = expected.filter(|&ty| !plan(self, ty));
                let left = self.expr(cx, left, hint);
                if plan(self, left.ty) {
                    let right = self.expr(cx, right, None);
                    if !plan(self, right.ty) && !self.types.has_error(right.ty) {
                        let message = format!(
                            "`|` joins a `Command` or `Pipeline` with another, not with {}",
                            self.display(right.ty)
                        );
                        self.diags.error("E1102", right.span, message);
                    }
                    let pipeline = self.types.handle(Handle::Pipeline, Ty::UNIT);
                    let callee = Callee::Std(StdFn::Process(hir::ProcessFn::Pipe));
                    return Self::mk(hir::ExprKind::Call(callee, Vec::new(), vec![left, right]), pipeline, span);
                }
                let right = self.expr(cx, right, Some(left.ty));
                let (left, right) = if left.ty == Ty::NEVER { (left, right) } else { self.common(left, right) };
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never)
                        || self.bound_by(left.ty, self.integer);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let (left, right) = self.operands(cx, left, right, expected, false);
                let allowed =
                    matches!(self.types.kind(left.ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error | TyKind::Never)
                        || self.bound_by(left.ty, self.integer);
                if !allowed {
                    operator_error(self, left.ty);
                }
                let ty = left.ty;
                (left, right, ty)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.operands(cx, left, right, None, true);
                if matches!(self.types.kind(left.ty), TyKind::Pointer(_)) {
                    let message =
                        "raw pointers have no `==`: compare `left.address() == right.address()` inside `unsafe`";
                    self.diags.error("E1102", span, message);
                } else if !self.types.is_equatable(left.ty) {
                    operator_error(self, left.ty);
                }
                self.nan_comparison(&left, &right, span);
                (left, right, Ty::BOOL)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.operands(cx, left, right, None, true);
                let ordered = matches!(
                    self.types.kind(left.ty),
                    TyKind::Int(_)
                        | TyKind::Float(_)
                        | TyKind::Char
                        | TyKind::Byte
                        | TyKind::String
                        | TyKind::Error
                        | TyKind::Never
                ) || self.bound_by(left.ty, self.integer);
                if !ordered {
                    // `a < b` is `Ordered.compare(a, b) < 0`.
                    if let Some((trait_id, index)) = self.ordering(left.ty) {
                        let ty = left.ty;
                        let right = self.coerce(right, ty);
                        let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![left, right]);
                        let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                        let compared =
                            hir::ExprKind::Binary(op, Box::new(Self::mk(call, Ty::INT, span)), Box::new(zero));
                        return Self::mk(compared, Ty::BOOL, span);
                    }
                    // Only the module that declares a type implements it there.
                    let local = self.owner(left.ty).is_some_and(|owner| owner.module == self.module);
                    if local && matches!(self.types.kind(left.ty), TyKind::Adt(_)) {
                        let name = self.types.display(left.ty);
                        let message = format!(
                            "operator `{}` cannot be applied to {}: implement `Ordered` for it, `impl Ordered for {name} {{ fn compare(self, other: {name}): Int {{ ... }} }}`, and the operators compare by it",
                            op.as_str(),
                            self.display(left.ty),
                        );
                        self.diags.error("E1102", span, message);
                    } else {
                        operator_error(self, left.ty);
                    }
                }
                self.nan_comparison(&left, &right, span);
                (left, right, Ty::BOOL)
            }
            BinOp::In => {
                let container = self.expr(cx, right, None);
                // A range or a string asks the prelude, the element first.
                let asked = match self.range_element(container.ty) {
                    Some(element) => Some(("prelude.rangeHas", element, vec![element])),
                    None if container.ty == Ty::STRING => Some(("prelude.textHas", Ty::CHAR, Vec::new())),
                    None => None,
                };
                if let Some((name, element, generics)) = asked
                    && let Some(&ModuleName::Fn(has)) = self.names.get(name)
                {
                    let item = self.expr(cx, left, Some(element));
                    let item = self.coerce(item, element);
                    let call = hir::ExprKind::Call(Callee::Fn(has), generics, vec![item, container]);
                    return Self::mk(call, Ty::BOOL, span);
                }
                if let TyKind::Map(element, _) | TyKind::Set(element) = *self.types.kind(container.ty) {
                    let item = self.expr(cx, left, Some(element));
                    let item = self.coerce(item, element);
                    let op = hir::MapOp::Contains;
                    return Self::mk(hir::ExprKind::MapOp(op, vec![container, item]), Ty::BOOL, span);
                }
                let (TyKind::Array(element) | TyKind::Deque(element)) = *self.types.kind(container.ty) else {
                    if container.ty != Ty::ERROR {
                        let message = format!("`in` is not defined for {}", self.display(container.ty));
                        self.diags.error("E1102", container.span, message);
                    }
                    return Self::error_expr(span);
                };
                if !self.types.is_equatable(element) {
                    operator_error(self, element);
                }
                // Membership observes the element first (5.12 orders operands
                // textually; the left operand is the element).
                let item = self.expr(cx, left, Some(element));
                let item = self.coerce(item, element);
                return Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Contains, vec![item, container]), Ty::BOOL, span);
            }
            BinOp::Range | BinOp::RangeInclusive => {
                let hint = expected.and_then(|ty| self.range_element(ty));
                let (start, end) = self.operands(cx, left, right, hint, false);
                return self.range_value(start, end, op == BinOp::RangeInclusive, span);
            }
        };
        Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span)
    }

    pub(super) fn if_expr(
        &mut self,
        cx: &mut FnCx,
        condition: &ast::Expr,
        then: &ast::Block,
        otherwise: Option<&ast::Expr>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let condition = self.expr(cx, condition, Some(Ty::BOOL));
        let condition = Box::new(self.coerce(condition, Ty::BOOL));
        let Some(otherwise) = otherwise else {
            // Its value is used, but there is none when the condition is false.
            let used = expected != Some(Ty::UNIT);
            let then = self.block(cx, then, if used { None } else { Some(Ty::UNIT) });
            if used && ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&then.ty) {
                let message = "an `if` without `else` has no value when its condition is false: add `else { ... }` with a value of the same type";
                self.diags.error("E1102", span, message);
            }
            return Self::mk(hir::ExprKind::If(condition, then, None), Ty::UNIT, span);
        };
        // `if c { [] } else { items }`: the other branch gives the type an
        // empty literal needs, as the first does the other way round.
        if expected.is_none() && then.tail.as_deref().is_some_and(empty_literal) {
            let other = self.expr(cx, otherwise, None);
            let then_expected = (![Ty::NEVER, Ty::ERROR].contains(&other.ty)).then_some(other.ty);
            let then = self.block(cx, then, then_expected);
            let ty = if then.ty == Ty::NEVER { other.ty } else { then.ty };
            return Self::mk(hir::ExprKind::If(condition, then, Some(Box::new(other))), ty, span);
        }
        let then = self.block(cx, then, expected);
        let other_expected = expected.or((then.ty != Ty::NEVER).then_some(then.ty));
        let other = self.expr(cx, otherwise, other_expected);
        let ty = if then.ty == Ty::NEVER {
            other.ty
        } else if other.ty == Ty::NEVER {
            then.ty
        } else {
            if expected.is_none() && other.ty != then.ty && ![then.ty, other.ty].contains(&Ty::ERROR) {
                let message = format!(
                    "`if` branches have different types: {} and {}",
                    self.display(then.ty),
                    self.display(other.ty)
                );
                self.diags.error("E1102", other.span, message);
            }
            then.ty
        };
        Self::mk(hir::ExprKind::If(condition, then, Some(Box::new(other))), ty, span)
    }
}
