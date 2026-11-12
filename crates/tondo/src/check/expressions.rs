//! Coercion, expressions, record and union literals, unification and inference.

use super::*;

impl<'a> Checker<'a> {
    /// Reports a mismatch unless `value` already has (or diverges into)
    /// `expected`. A `T` where `T?` is expected becomes `some(T)` (8.9).
    pub(super) fn coerce(&mut self, value: hir::Expr, expected: Ty) -> hir::Expr {
        if value.ty == expected {
            return value;
        }
        // Where nothing is expected, a value is dropped, unless it is a
        // result the program has to see (11.9).
        if expected == Ty::UNIT && value.ty != Ty::NEVER && !self.types.has_error(value.ty) {
            if self.must_use(value.ty) {
                let message = match value.kind {
                    hir::ExprKind::Recover(..) => format!(
                        "what `recover` gives, of type {}, is not used; read it with `match`, `?? fallback` or `.isErr()`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    ),
                    _ => format!(
                        "a value of type {} is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    ),
                };
                self.diags.error("E1303", value.span, message);
                let start = Span::new(value.span.start, value.span.start);
                self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
                return value;
            }
            let span = value.span;
            let block = hir::Block { stmts: vec![hir::Stmt::Expr(value)], tail: None, ty: Ty::UNIT };
            return Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span);
        }
        // A closure becomes a `fn(...)` value when it never changes its
        // captures (11.8).
        if let TyKind::Fn(..) = self.types.kind(expected)
            && let Some((body, _, info)) = self.types.closure(value.ty)
        {
            let found =
                self.types.intern(TyKind::Fn(info.params, info.result, info.variadic, info.modes, info.is_unsafe));
            // Any task may call a `fn(...)` value (16.13).
            if found == expected
                && !info.mutates
                && let Some(part) = self.not_send(value.ty)
            {
                let message = format!(
                    "this closure captures {}, which is not `Send`, so it cannot become {}, which any task may call; call the closure where it is instead",
                    self.display(part),
                    self.display(expected)
                );
                self.diags.error("E1605", value.span, message);
                return value;
            }
            if found == expected && info.once {
                let message = format!(
                    "this closure gives a capture away, so it can be called once and cannot become {}; take it as a type parameter bound by `CallOnce[{}]`",
                    self.display(expected),
                    self.types.display(expected)
                );
                self.diags.error("E1108", value.span, message);
                return value;
            }
            if found == expected && !info.mutates {
                let span = value.span;
                return Self::mk(hir::ExprKind::Erase(Box::new(value), FnId(body)), expected, span);
            }
            if found == expected {
                let message = format!(
                    "this closure changes its captures, so it cannot become {}; take it as a type parameter bound by `CallMut[{}]`",
                    self.display(expected),
                    self.types.display(expected)
                );
                self.diags.error("E1108", value.span, message);
                return value;
            }
        }
        // A `String` passes to C as a copy that ends with a zero byte.
        if self.c_arguments
            && value.ty == Ty::STRING
            && (expected == self.c_string || self.types.lookup(&TyKind::Option(self.c_string)) == Some(expected))
        {
            return value;
        }
        // An integer constant's value is checked like a literal's.
        if let hir::ExprKind::Int(number) = value.kind
            && self.const_uses.contains(&value.span)
            && let Some(int) = self.types.as_int(expected)
        {
            let from = self.types.as_int(self.types.repr(value.ty)).unwrap_or(IntTy::I64);
            if int.holds_from(from, number) {
                return Self::mk(hir::ExprKind::Int(number), expected, value.span);
            }
            let number = from.show(number);
            let message = format!("the constant's value {number} does not fit in {}", self.display(expected));
            self.diags.error("E1102", value.span, message);
            return value;
        }
        let value = match self.implicit(value, expected) {
            Ok(converted) => return converted,
            Err(value) => value,
        };
        if let TyKind::CFn(params, result, variadic) = self.types.kind(expected).clone() {
            return self.callback(value, (params, result, variadic), expected);
        }
        // A value of a member's exact type is injected into a union (9.5).
        if let Some(member) = self.types.members(expected).and_then(|m| m.iter().position(|&t| t == value.ty)) {
            let span = value.span;
            return Self::mk(hir::ExprKind::Variant(member as u32, vec![(0, value)]), expected, span);
        }
        if let TyKind::Option(inner) = *self.types.kind(expected)
            && (value.ty == inner
                || self.types.members(inner).is_some_and(|m| m.contains(&value.ty))
                || self.converts(value.ty, inner)
                || matches!(self.types.kind(inner), TyKind::CFn(..))
                    && matches!(self.types.kind(value.ty), TyKind::Fn(..) | TyKind::Adt(_))
                || matches!(self.types.kind(inner), TyKind::Fn(..)) && self.types.closure(value.ty).is_some())
        {
            let span = value.span;
            let value = self.coerce(value, inner);
            return Self::mk(hir::ExprKind::Variant(SOME, vec![(0, value)]), expected, span);
        }
        if value.ty != expected
            && value.ty != Ty::NEVER
            && !self.types.has_error(value.ty)
            && !self.types.has_error(expected)
        {
            let wrapped = match *self.types.kind(value.ty) {
                TyKind::Result(ok, _) | TyKind::Option(ok) => ok == expected,
                _ => false,
            };
            let narrowed = self.conversion_checked(value.ty, expected);
            let message = match value.kind {
                hir::ExprKind::Int(literal) if expected == Ty::BYTE && (0..256).contains(&literal) => {
                    format!("an integer literal is not a `Byte`: write `Byte({literal}u8)`")
                }
                // Each closure has a type of its own, which only it has.
                _ if self.types.is_closure(value.ty) && self.types.is_closure(expected) => {
                    let shown = self.types.display(expected);
                    let signature = shown.trim_start_matches("closure ");
                    format!(
                        "each closure is a type of its own, so this one is not the other: give the place the function type `{signature}`, which any closure of that signature fits"
                    )
                }
                _ if narrowed.is_some() && expected != Ty::BYTE && value.ty != Ty::BYTE => {
                    let text = self.sources.slice(value.span);
                    let target = self.types.display(expected);
                    match narrowed {
                        Some(true) => format!(
                            "expected {}, found {}, which may not fit: convert it with `{target}({text})?`, or handle the error with `match`",
                            self.display(expected),
                            self.display(value.ty)
                        ),
                        _ => format!(
                            "expected {}, found {}: convert it with `{target}({text})`",
                            self.display(expected),
                            self.display(value.ty)
                        ),
                    }
                }
                _ if wrapped && matches!(self.types.kind(value.ty), TyKind::Option(_)) => format!(
                    "expected {}, found {}, which may be `none`: give a fallback with `value ?? fallback`, handle it with `match`, or pass it on with `?` in a function that returns an option",
                    self.display(expected),
                    self.display(value.ty)
                ),
                _ if wrapped => format!(
                    "expected {}, found {}: add `?` to pass the failure on, give a fallback with `value ?? fallback`, or handle it with `match`",
                    self.display(expected),
                    self.display(value.ty)
                ),

                _ if self.comparing.is_some() => {
                    let (found, other) = (self.display(value.ty), self.display(expected));
                    let (left, right) = if self.comparing == Some(true) { (found, other) } else { (other, found) };
                    format!("cannot compare {left} with {right}: both sides of a comparison have one type")
                }
                _ if self.panic_datum == Some(expected) => format!(
                    "a panic's data holds an `Int`, a `Float`, a `Bool` or a `String`, not {}: write its text, as `\"{{value}}\"`",
                    self.display(value.ty)
                ),
                _ if expected == Ty::STRING && value.ty == Ty::CHAR => {
                    "expected `String`, found `Char`: interpolate it, as `\"{c}\"`, for a `String`".to_string()
                }
                _ if expected == Ty::CHAR && value.ty == Ty::STRING => {
                    "expected `Char`, found `String`: a `Char` literal is in single quotes, `'a'`".to_string()
                }
                _ if value.ty == Ty::STRING && self.query_type() == Some(expected) => {
                    "expected `sql.Query`, found `String`: a query is a string literal written where a `sql.Query` is expected, whose values are parameters; write the literal here, keep it as `let query: sql.Query = \"...\"`, or, for SQL made while the program runs, `sql.raw(text)`".to_string()
                }
                _ => format!("expected {}, found {}", self.display(expected), self.display(value.ty)),
            };
            self.diags.error("E1102", value.span, message);
        }
        value
    }

    /// The conversions that never lose information and so need no syntax:
    /// a number to a wider one of its kind (8.6), any pointer to a
    /// `Pointer[ffi.Void]`, and between `ffi.CString` and `Pointer[Byte]`.
    pub(super) fn implicit(&mut self, value: hir::Expr, expected: Ty) -> Result<hir::Expr, hir::Expr> {
        if !self.converts(value.ty, expected) {
            return Err(value);
        }
        let span = value.span;
        let kind = match self.types.kind(expected) {
            TyKind::Int(_) | TyKind::Float(_) => hir::ExprKind::Convert(Box::new(value)),
            TyKind::Array(_) | TyKind::CArray(..) => hir::ExprKind::Fixed(Box::new(value)),
            _ => hir::ExprKind::Retype(Box::new(value)),
        };
        Ok(Self::mk(kind, expected, span))
    }

    /// Whether a value of `from` becomes a `to` implicitly (`implicit`).
    pub(super) fn converts(&self, from: Ty, to: Ty) -> bool {
        if from == to {
            return false;
        }
        let byte_pointer = self.types.lookup(&TyKind::Pointer(Ty::BYTE));
        match (self.types.kind(from), self.types.kind(to)) {
            (TyKind::Int(a), TyKind::Int(b)) => a.min() >= b.min() && a.max() <= b.max(),
            (TyKind::Float(_), TyKind::Float(_)) => to == Ty::F64,
            (TyKind::Pointer(_), TyKind::Pointer(target)) => *target == self.void,
            (TyKind::Array(a), TyKind::CArray(b, _)) | (TyKind::CArray(a, _), TyKind::Array(b)) => a == b,
            _ if from == self.c_string => {
                Some(to) == byte_pointer || to == self.types.lookup(&TyKind::Pointer(self.void)).unwrap_or(Ty::ERROR)
            }
            _ if to == self.c_string => Some(from) == byte_pointer,
            _ => false,
        }
    }

    /// A named function, or a closure without captures, as the address of
    /// a C function that runs it (FFI): C passes no context, so a function
    /// that needs one takes it through a `Pointer[ffi.Void]`.
    pub(super) fn callback(
        &mut self,
        value: hir::Expr,
        (params, result, variadic): (Vec<Ty>, Ty, bool),
        expected: Ty,
    ) -> hir::Expr {
        let span = value.span;
        if self.types.has_error(value.ty) || value.ty == Ty::NEVER {
            return value;
        }
        let signature = match self.types.kind(value.ty).clone() {
            TyKind::Fn(p, r, v, modes, _) if matches!(value.kind, hir::ExprKind::FnValue(..)) => {
                Some((p, r, v, modes, None))
            }
            _ => self.types.closure(value.ty).map(|(_, _, info)| {
                let captures = self.types.components(value.ty).map_or(0, |c| c.len());
                (info.params, info.result, info.variadic, info.modes, Some(captures))
            }),
        };
        let Some((p, r, v, modes, captures)) = signature else {
            let message = format!(
                "{} cannot become {}: only a named function or a closure without captures can, since C keeps no captures",
                self.display(value.ty),
                self.display(expected)
            );
            self.diags.error("E1705", span, message);
            return Self::error_expr(span);
        };
        let found = self.types.intern(TyKind::Fn(p.clone(), r, v, modes.clone(), false));
        if variadic || v || p != params || r != result || modes.iter().any(|&m| m != ast::Mode::Value) {
            let message = format!(
                "expected {}, found {}: a function C calls takes and returns exactly its types",
                self.display(expected),
                self.types.display(found)
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        if captures.is_some_and(|c| c > 0) {
            let message = "a closure that C calls cannot capture values, since C keeps no captures; pass what it needs through a `Pointer[ffi.Void]` made with `ffi.context(value)`";
            self.diags.error("E1705", span, message);
            return Self::error_expr(span);
        }
        Self::mk(hir::ExprKind::Callback(Box::new(value), FnId(u32::MAX)), expected, span)
    }

    /// The prelude's `Range[T]` of integers or characters (10.16).
    pub(super) fn range_type(&mut self, element: Ty, span: Span) -> Ty {
        let Some(&TypeName::Template(range)) = self.type_names.get("prelude.Range") else { return Ty::ERROR };
        if !matches!(
            self.types.kind(element),
            TyKind::Int(_) | TyKind::Char | TyKind::Param(_) | TyKind::Error | TyKind::Never
        ) {
            let message = format!("a range counts through integers or characters, not {}", self.display(element));
            self.diags.error("E1102", span, message);
            return Ty::ERROR;
        }
        self.types.instantiate(range, vec![element])
    }

    /// What a `Range[T]` yields, if `ty` is one.
    pub(super) fn range_element(&self, ty: Ty) -> Option<Ty> {
        let Some(&TypeName::Template(range)) = self.type_names.get("prelude.Range") else { return None };
        match self.types.instance_of(ty) {
            Some((template, args)) if template == range => args.first().copied(),
            _ => None,
        }
    }

    /// `start..end` or `start..=end` as a value.
    pub(super) fn range_value(&mut self, start: hir::Expr, end: hir::Expr, inclusive: bool, span: Span) -> hir::Expr {
        let ty = self.range_type(start.ty, span);
        if ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let flag = |value| Self::mk(hir::ExprKind::Bool(value), Ty::BOOL, span);
        let fields = vec![(0, start), (1, end), (2, flag(inclusive)), (3, flag(false))];
        Self::mk(hir::ExprKind::Record(fields), ty, span)
    }

    pub(super) fn require_int(&mut self, value: &hir::Expr) {
        if self.types.as_int(value.ty).is_none()
            && ![Ty::NEVER, Ty::ERROR].contains(&value.ty)
            && !self.bound_by(value.ty, self.integer)
        {
            let message = format!("expected an integer, found {}", self.display(value.ty));
            self.diags.error("E1102", value.span, message);
        }
    }

    pub(super) fn expr(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
        // A value that takes its type from the one expected, and is never an
        // option itself, is typed as the `T` of an expected `T?` and then
        // becomes one (9.7): `[]`, `3` as a `Float?`, `ok(value)`.
        let expected = match expected.map(|t| self.types.kind(t)) {
            Some(&TyKind::Option(inner)) if Self::typed_by_expected(e) => Some(inner),
            _ => expected,
        };
        let entries = cx.entry_values.len();
        let value = self.expr_kind(cx, e, expected);
        self.index_expr(e.span, value.ty);
        // A map's entry that this expression did not lend changes nothing.
        if !matches!(e.kind, ast::ExprKind::Entry { .. }) {
            self.unchanged_entries(cx, entries);
        }
        value
    }

    /// `map[key, default: value]` read as values since the first `from`,
    /// which no `mut` or `var` loan took: they change nothing, so each is
    /// the read it stands for.
    pub(super) fn unchanged_entries(&mut self, cx: &mut FnCx, from: usize) {
        for (span, read) in cx.entry_values.split_off(from.min(cx.entry_values.len())) {
            let message = format!(
                "`[key, default: value]` is an entry to change, as `counts[word, default: 0] += 1`, and adds it when it is missing; to read it, write `{read}`"
            );
            self.diags.error("E1411", span, message);
            self.diags.fix(format!("read `{read}`"), true, vec![(span, read)]);
        }
    }

    /// `value ?? fallback` (17.11): the `T` that an option or a result holds,
    /// or else the fallback, evaluated only then, which may leave; after an
    /// option, a fallback that is a `T?` keeps the option, so that `??`
    /// chains. A `match` on the value, whose error goes unread.
    pub(super) fn coalesce(
        &mut self,
        cx: &mut FnCx,
        value: &ast::Expr,
        fallback: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        // A call learns the success type it should give, as with `?`.
        let hint = match &value.kind {
            ast::ExprKind::Call { .. } | ast::ExprKind::Navigate(..) => {
                expected.filter(|t| !matches!(self.types.kind(*t), TyKind::Option(_)))
            }
            _ => None,
        };
        let value = self.expr(cx, value, hint);
        let (present, absent, payload, error) = match self.types.kind(self.types.repr(value.ty)).clone() {
            TyKind::Option(payload) => (SOME, NONE, payload, None),
            TyKind::Result(payload, error) => (OK, ERR, payload, Some(error)),
            _ if value.ty == Ty::ERROR => {
                self.expr(cx, fallback, None);
                return Self::error_expr(span);
            }
            _ => {
                let message = format!(
                    "`??` gives the value of an option or a result, and this is {}: write it without `?? ...`",
                    self.display(value.ty)
                );
                self.diags.error("E1102", value.span, message);
                self.expr(cx, fallback, None);
                return Self::error_expr(span);
            }
        };
        let option = error.is_none();
        let optional = self.types.intern(TyKind::Option(payload));
        // The fallback runs only when there is no value: what it does to
        // the flow of the rest is not known.
        let diverged = cx.diverged;
        let hint = match expected {
            Some(expected) if option && expected == optional => optional,
            _ => payload,
        };
        let fallback = self.expr(cx, fallback, Some(hint));
        cx.diverged = diverged;
        let result = if option && fallback.ty == optional { optional } else { payload };
        let fallback = self.coerce(fallback, result);
        let local = self.hidden_local(cx, payload);
        let read = Self::mk(hir::ExprKind::Local(local), payload, span);
        let read = if result == optional {
            Self::mk(hir::ExprKind::Variant(SOME, vec![(0, read)]), optional, span)
        } else {
            read
        };
        let bound = hir::Pattern { kind: hir::PatKind::Binding(local), ty: payload, span };
        let some = hir::Pattern { kind: hir::PatKind::Variant(present, vec![bound]), ty: value.ty, span };
        let rest = error.map(|ty| hir::Pattern { kind: hir::PatKind::Wildcard, ty, span }).into_iter().collect();
        let none = hir::Pattern { kind: hir::PatKind::Variant(absent, rest), ty: value.ty, span };
        let arms = vec![
            hir::Arm { pattern: some, guard: None, body: read },
            hir::Arm { pattern: none, guard: None, body: fallback },
        ];
        Self::mk(hir::ExprKind::Match(Box::new(value), arms), result, span)
    }

    /// `base?.rest` (17.2): on an option, the rest of the chain runs on the
    /// value when there is one, and the whole is an option, `none` when there
    /// is not, a level flattened when the rest gives one already; on a
    /// result, `?` propagates the error and the rest runs on the value.
    pub(super) fn navigate(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        rest: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, base, None);
        let question = Span::new(base.span.end, rest.span.start + 1);
        match self.types.kind(self.types.repr(value.ty)).clone() {
            TyKind::Option(payload) => {
                let local = self.hidden_local(cx, payload);
                self.holes.push((local, payload));
                // What the rest gives is the value of the option expected.
                let hint = expected.and_then(|ty| match self.types.kind(ty) {
                    TyKind::Option(inner) => Some(*inner),
                    _ => None,
                });
                let then = self.expr(cx, rest, hint);
                self.holes.pop();
                if then.ty == Ty::ERROR {
                    return Self::error_expr(span);
                }
                let (result, then) = match self.types.kind(then.ty) {
                    TyKind::Option(_) => (then.ty, then),
                    _ => {
                        let result = self.types.intern(TyKind::Option(then.ty));
                        (result, Self::mk(hir::ExprKind::Variant(SOME, vec![(0, then)]), result, span))
                    }
                };
                let bound = hir::Pattern { kind: hir::PatKind::Binding(local), ty: payload, span };
                let present = hir::Pattern { kind: hir::PatKind::Variant(SOME, vec![bound]), ty: value.ty, span };
                let absent = hir::Pattern { kind: hir::PatKind::Variant(NONE, Vec::new()), ty: value.ty, span };
                let none = Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), result, span);
                let arms = vec![
                    hir::Arm { pattern: present, guard: None, body: then },
                    hir::Arm { pattern: absent, guard: None, body: none },
                ];
                Self::mk(hir::ExprKind::Match(Box::new(value), arms), result, span)
            }
            TyKind::Error => {
                let local = self.hidden_local(cx, Ty::ERROR);
                self.holes.push((local, Ty::ERROR));
                self.expr(cx, rest, None);
                self.holes.pop();
                Self::error_expr(span)
            }
            // A result, or what `?` reports it cannot apply to.
            _ => {
                let tried = self.try_value(cx, value, Span::new(base.span.start, question.end));
                let local = self.hidden_local(cx, tried.ty);
                self.holes.push((local, tried.ty));
                let then = self.expr(cx, rest, expected);
                self.holes.pop();
                let ty = then.ty;
                let block = hir::Block { stmts: vec![hir::Stmt::Let(local, tried)], tail: Some(Box::new(then)), ty };
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
        }
    }

    /// Whether an expression takes its type from the one expected and is
    /// never an option: a number, a literal of a collection or a tuple, or
    /// `ok(...)` and `err(...)`.
    fn typed_by_expected(e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Int { .. }
            | ast::ExprKind::Float { .. }
            | ast::ExprKind::BigInt(_)
            | ast::ExprKind::Array(_)
            | ast::ExprKind::Map(_)
            | ast::ExprKind::Set(_)
            | ast::ExprKind::Tuple(_) => true,
            ast::ExprKind::Construct(constructor, _) => *constructor != "some",
            _ => false,
        }
    }

    /// For an editor: the type of an expression of the program's own files.
    pub(super) fn index_expr(&mut self, span: Span, ty: Ty) {
        if self.index.is_some() && ty != Ty::ERROR && !self.sources.file_of(span.start).logical.starts_with("<std>/") {
            self.index_exprs.push((span, ty));
        }
    }

    pub(super) fn expr_kind(&mut self, cx: &mut FnCx, e: &ast::Expr, expected: Option<Ty>) -> hir::Expr {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::Int { value, suffix } => self.int_literal(false, *value, *suffix, expected, span),
            ast::ExprKind::Bool(b) => Self::mk(hir::ExprKind::Bool(*b), Ty::BOOL, span),
            ast::ExprKind::Unit => Self::mk(hir::ExprKind::Unit, Ty::UNIT, span),
            ast::ExprKind::Command(stages) => self.command_literal(cx, stages, span),
            ast::ExprKind::Str(segments) => {
                match expected.and_then(|ty| self.query_type().filter(|&query| query == ty)) {
                    Some(query) => self.query_literal(cx, segments, query, span),
                    None => self.string(cx, segments, span),
                }
            }
            // A name given where a function is expected (11.7).
            ast::ExprKind::Path(_)
                if let Some(expected) = expected
                    && self.wants_function(expected)
                    && let Some(form) = self.function_form(cx, e) =>
            {
                self.expr_kind(cx, &form, Some(expected))
            }
            ast::ExprKind::Path(segments) => self.path(cx, segments, expected, span),
            ast::ExprKind::Call { callee, args } => self.call(cx, callee, args, expected, span),
            ast::ExprKind::Unary(op, operand) => self.unary(cx, *op, operand, expected, span),
            ast::ExprKind::Binary(op, left, right) => self.binary(cx, *op, left, right, expected, span),
            ast::ExprKind::If { condition, then, otherwise } => {
                self.if_expr(cx, condition, then, otherwise.as_deref(), expected, span)
            }
            ast::ExprKind::Block(block) => {
                let block = self.block(cx, block, expected);
                let ty = block.ty;
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            ast::ExprKind::Float { text, suffix } => self.float_literal(text, *suffix, false, expected, span),
            ast::ExprKind::BigInt(digits) => self.big_literal(digits, expected, span),
            ast::ExprKind::Char(c) => Self::mk(hir::ExprKind::Char(*c), Ty::CHAR, span),
            ast::ExprKind::Field(base, name) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Name(&name.name), name.span, span)
            }
            ast::ExprKind::TupleSlot(base, slot) => {
                let base = self.expr(cx, base, None);
                self.field_access(base, FieldRef::Slot(*slot), span, span)
            }
            ast::ExprKind::Tuple(items) => self.tuple(cx, items, expected, span),
            ast::ExprKind::Array(items) => self.array_literal(cx, items, expected, span),
            ast::ExprKind::Map(pairs) => self.map_literal(cx, pairs, expected, span),
            ast::ExprKind::Set(items) => self.set_literal(cx, items, expected, span),
            ast::ExprKind::Index { base: function, index: argument } if self.specialization(cx, function).is_some() => {
                // `identity[Int]` used as a value (11.7).
                let id = self.specialization(cx, function).unwrap();
                match Self::type_of_expr(argument) {
                    Some(ty) => {
                        let ty = self.resolve_type(&ty);
                        self.fn_value(id, vec![ty], expected, span)
                    }
                    None => {
                        self.diags.error("E1104", argument.span, "type arguments are types, such as `identity[Int]`");
                        Self::error_expr(span)
                    }
                }
            }
            // A map's entry as a value, which a `mut` receiver or argument
            // takes as a place.
            ast::ExprKind::Entry { base: map, key, default } => {
                let Some((local, path, ty)) = self.place(cx, e) else { return Self::error_expr(span) };
                let text = |e: &ast::Expr| self.sources.slice(e.span).to_string();
                let read = format!("{}[{}] ?? {}", text(map), text(key), text(default));
                cx.entry_values.push((span, read));
                Self::mk(hir::ExprKind::PlaceValue(hir::Place { local, path }), ty, span)
            }
            ast::ExprKind::Index { base, index } => {
                let base = self.expr(cx, base, None);
                // `document["key"]` and `document[index]` of a `json.Value`
                // read a member or an element, or `Null` (14.10).
                if let Some(&TypeName::Nominal(json)) = self.type_names.get("json.Value")
                    && base.ty == json
                {
                    let index = self.expr(cx, index, None);
                    let (method, ty) = if index.ty == Ty::STRING { ("field", Ty::STRING) } else { ("item", Ty::INT) };
                    let index = self.coerce(index, ty);
                    let key = self.owner_key(json);
                    let id = self.methods[&(key, method.to_string())];
                    return Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![base, index]), json, span);
                }
                if let TyKind::Map(key, value) = *self.types.kind(base.ty) {
                    // Reading a map entry yields `V?`: absence is normal (10.10).
                    let index = self.expr(cx, index, Some(key));
                    let index = self.coerce(index, key);
                    let ty = self.types.intern(TyKind::Option(value));
                    return Self::mk(hir::ExprKind::MapOp(hir::MapOp::Get, vec![base, index]), ty, span);
                }
                if base.ty == Ty::STRING {
                    let index = self.expr(cx, index, Some(Ty::INT));
                    let index = self.coerce(index, Ty::INT);
                    return Self::mk(hir::ExprKind::StrOp(hir::StrOp::At, vec![base, index]), Ty::CHAR, span);
                }
                let (TyKind::Array(element) | TyKind::Deque(element) | TyKind::CArray(element, _)) =
                    *self.types.kind(base.ty)
                else {
                    return self.not_indexable(base, span);
                };
                let index = self.expr(cx, index, Some(Ty::INT));
                let index = self.coerce(index, Ty::INT);
                Self::mk(hir::ExprKind::Index(Box::new(base), Box::new(index)), element, span)
            }
            ast::ExprKind::Slice { base, start, end, step } => {
                let base = self.expr(cx, base, None);
                if !matches!(self.types.kind(base.ty), TyKind::Array(_) | TyKind::String) {
                    return self.not_indexable(base, span);
                }
                let mut bound = |this: &mut Self, part: &Option<Box<ast::Expr>>| {
                    part.as_ref().map(|p| {
                        let value = this.expr(cx, p, Some(Ty::INT));
                        Box::new(this.coerce(value, Ty::INT))
                    })
                };
                let parts = [bound(self, start), bound(self, end), bound(self, step)];
                let ty = base.ty;
                Self::mk(hir::ExprKind::Slice(Box::new(base), parts), ty, span)
            }
            ast::ExprKind::Generic { base, args } => match &base.kind {
                // `identity[Int]`: a specialized generic function as a value (11.7).
                ast::ExprKind::Path(segments)
                    if segments.len() == 1 && self.lookup(cx, &segments[0].name).is_none() =>
                {
                    match self.names.get(&segments[0].name).copied() {
                        Some(ModuleName::Fn(id)) => {
                            let explicit = args.iter().map(|a| self.resolve_type(a)).collect();
                            self.fn_value(id, explicit, expected, span)
                        }
                        _ => {
                            self.diags.error("E1102", span, "type arguments must be followed by a call");
                            Self::error_expr(span)
                        }
                    }
                }
                _ => {
                    self.diags.error("E1102", span, "type arguments must be followed by a call");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Match { scrutinee, arms } => self.match_expr(cx, scrutinee, arms, expected, span),
            ast::ExprKind::Select(arms) => self.select(cx, arms, expected, span),
            ast::ExprKind::SyncLiteral(path, literal) => self.sync_literal(cx, path, literal, expected, span),
            ast::ExprKind::Record { path, type_args, fields } => {
                self.record_literal(cx, path, type_args, fields, expected, span)
            }
            ast::ExprKind::With { base, updates } => self.with_expr(cx, base, updates, span),
            ast::ExprKind::Coalesce(value, fallback) => self.coalesce(cx, value, fallback, expected, span),
            ast::ExprKind::Navigate(base, rest) => self.navigate(cx, base, rest, expected, span),
            ast::ExprKind::Hole => {
                let (local, ty) = *self.holes.last().expect("a hole is inside a `Navigate`");
                Self::mk(hir::ExprKind::Local(local), ty, span)
            }
            ast::ExprKind::Try(inner) => self.try_expr(cx, inner, expected, span),
            ast::ExprKind::Closure(closure) => self.closure(cx, closure, expected, span),
            ast::ExprKind::Recover(block) => self.recover(cx, block, expected, span),
            ast::ExprKind::Within(limit, block) => self.within(cx, limit, block, span),
            // The text of `template.html` or `template.text`, which the call
            // compiles (standard library 14.31).
            ast::ExprKind::Template(_) => Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span),
            ast::ExprKind::TemplateWrite { value, format, place } => {
                self.template_write(cx, value, format.as_ref(), *place, span)
            }
            ast::ExprKind::TemplateInclude { data, named, body } => {
                self.template_include(cx, data.as_deref(), named, body, span)
            }
            ast::ExprKind::Unsafe(block) => {
                self.meta_denied(span, "`unsafe`");
                let outer = std::mem::replace(&mut cx.unsafe_region, true);
                let block = self.block(cx, block, expected);
                cx.unsafe_region = outer;
                let ty = block.ty;
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            ast::ExprKind::None => match self.union_member(expected, "none").map(|t| (t, self.types.kind(t))) {
                Some((ty, TyKind::Option(_))) => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), ty, span),
                _ => {
                    self.diags.error("E1304", span, "`none` needs an expected `T?` type");
                    Self::error_expr(span)
                }
            },
            ast::ExprKind::Construct(constructor, value) => self.construct(cx, constructor, value, expected, span),
            // A task whose `Join` nobody uses belongs to the enclosing block.
            ast::ExprKind::Spawn(_, false) if expected == Some(Ty::UNIT) => {
                let stmts = vec![self.spawn_statement(cx, e)];
                Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }), Ty::UNIT, span)
            }
            ast::ExprKind::Spawn(call, false) => self.spawn(cx, call, span),
            ast::ExprKind::Spawn(call, true) => {
                let join = self.spawn(cx, call, span);
                if join.ty == Ty::ERROR {
                    return join;
                }
                Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Detach, vec![join]), Ty::UNIT, span)
            }
            ast::ExprKind::Error => Self::error_expr(span),
        }
    }

    pub(super) fn field_access(&mut self, base: hir::Expr, field: FieldRef, field_span: Span, span: Span) -> hir::Expr {
        if let TyKind::Handle(Handle::Ref, value) = *self.types.kind(base.ty) {
            if let FieldRef::Name("value") = field {
                return Self::mk(hir::ExprKind::RefValue(Box::new(base)), value, span);
            }
            let message = format!("{} has one field, `value`, the value it identifies", self.display(base.ty));
            self.diags.error("E1102", field_span, message);
            return Self::error_expr(span);
        }
        let Some((index, ty)) = self.field_of(base.ty, field, field_span) else {
            return Self::error_expr(span);
        };
        let kind = match index {
            Some(index) => hir::ExprKind::Field(Box::new(base), index),
            None => hir::ExprKind::Retype(Box::new(base)),
        };
        Self::mk(kind, ty, span)
    }

    pub(super) fn tuple(&mut self, cx: &mut FnCx, items: &[ast::Expr], expected: Option<Ty>, span: Span) -> hir::Expr {
        let expected_items = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Tuple(types)) if types.len() == items.len() => types.clone(),
            _ => Vec::new(),
        };
        let items: Vec<hir::Expr> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let expected = expected_items.get(i).copied();
                let value = self.expr(cx, item, expected);
                match expected {
                    Some(expected) => self.coerce(value, expected),
                    None => value,
                }
            })
            .collect();
        let ty = self.types.intern(TyKind::Tuple(items.iter().map(|i| i.ty).collect()));
        Self::mk(hir::ExprKind::Aggregate(items), ty, span)
    }

    /// The fields of a record type, or an error at `span`.
    pub(super) fn record_fields(&mut self, ty: Ty, span: Span) -> Option<Vec<(String, Ty)>> {
        if let TyKind::Adt(id) = self.types.kind(ty)
            && let AdtKind::Record(fields) = &self.types.adt(*id).kind
        {
            return Some(fields.iter().map(|f| (f.name.clone(), f.ty)).collect());
        }
        if ty != Ty::ERROR {
            let message = format!("{} is not a record type", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        None
    }

    /// Checks field initializers against `fields`, in source order.
    /// The values of `inits` for `fields`. With an inference, field types
    /// may mention parameters being inferred and values are not coerced yet.
    pub(super) fn field_values(
        &mut self,
        cx: &mut FnCx,
        record: Ty,
        fields: &[(String, Ty)],
        inits: &[ast::FieldInit],
        allow_shorthand: bool,
        mut inference: Option<&mut Inference>,
    ) -> (Vec<(u32, hir::Expr)>, Vec<bool>) {
        let mut seen = vec![false; fields.len()];
        let mut values = Vec::new();
        for init in inits {
            let index = fields.iter().position(|(name, _)| *name == init.name.name);
            let expected = match (index, inference.as_deref()) {
                (Some(i), Some(inference)) => self.inferred(inference, fields[i].1),
                (Some(i), None) => Some(fields[i].1),
                (None, _) => None,
            };
            let inferring = u32::from(inference.is_some());
            self.inferring += inferring;
            let value = match &init.value {
                Some(value) => self.expr(cx, value, expected),
                None if allow_shorthand && crate::lexer::Kw::lookup(&init.name.name).is_none() => {
                    self.path(cx, std::slice::from_ref(&init.name), None, init.name.span)
                }
                None => {
                    let message =
                        format!("field `{}` needs an explicit value: `{}: value`", init.name.name, init.name.name);
                    self.diags.error("E1102", init.name.span, message);
                    Self::error_expr(init.name.span)
                }
            };
            self.inferring -= inferring;
            let Some(index) = index else {
                let mut message = format!("{} has no field `{}`", self.display(record), init.name.name);
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                match closest(&init.name.name, names.iter().map(String::as_str)) {
                    Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
                    None => message.push_str(&format!("; its fields are {}", listed(&names))),
                }
                self.diags.error("E1102", init.name.span, message);
                self.offer_meant(init.name.span);
                continue;
            };
            if std::mem::replace(&mut seen[index], true) {
                let message = format!("field `{}` is initialized more than once", init.name.name);
                self.diags.error("E1102", init.name.span, message);
            }
            let value = match inference.as_deref_mut() {
                Some(inference) => {
                    self.unify(inference, fields[index].1, value.ty);
                    value
                }
                None => self.coerce(value, fields[index].1),
            };
            values.push((index as u32, value));
        }
        (values, seen)
    }

    /// `ty` with the inferred arguments, unless it still mentions an
    /// unknown parameter.
    pub(super) fn inferred(&mut self, inference: &Inference, ty: Ty) -> Option<Ty> {
        let mut params = Vec::new();
        self.types.params_in(ty, &mut params);
        let unknown = params.iter().any(|p| inference.vars.contains(p) && !inference.map.contains_key(p));
        (!unknown).then(|| self.types.subst(ty, &inference.map))
    }

    /// Binds the parameters in `pattern` so that it matches `actual`.
    pub(super) fn unify(&mut self, inference: &mut Inference, pattern: Ty, actual: Ty) {
        if actual == Ty::ERROR || actual == Ty::NEVER {
            return;
        }
        if inference.vars.contains(&pattern) {
            inference.map.entry(pattern).or_insert(actual);
            return;
        }
        let (expected, found) = (self.types.kind(pattern).clone(), self.types.kind(actual).clone());
        match (expected, found) {
            (TyKind::Tuple(items), TyKind::Tuple(values)) if items.len() == values.len() => {
                for (item, value) in items.into_iter().zip(values) {
                    self.unify(inference, item, value);
                }
            }
            (TyKind::Option(a), TyKind::Option(b))
            | (TyKind::Array(a), TyKind::Array(b))
            | (TyKind::Pointer(a), TyKind::Pointer(b))
            | (TyKind::Deque(a), TyKind::Deque(b))
            | (TyKind::Set(a), TyKind::Set(b)) => self.unify(inference, a, b),
            (TyKind::Handle(h, a), TyKind::Handle(k, b)) if h == k => self.unify_argument(inference, a, b),
            // A value lifts into an option.
            (TyKind::Option(a), _) => self.unify(inference, a, actual),
            // An error type of `Never` is one: what cannot fail.
            (TyKind::Result(a, b), TyKind::Result(c, Ty::NEVER)) if inference.vars.contains(&b) => {
                self.unify(inference, a, c);
                inference.map.entry(b).or_insert(Ty::NEVER);
            }
            // A value of `Never ! E` is one, as `recover { panic(...) }`.
            (TyKind::Result(a, b), TyKind::Result(c, d)) => {
                self.unify_argument(inference, a, c);
                self.unify_argument(inference, b, d);
            }
            (TyKind::Map(a, b), TyKind::Map(c, d)) => {
                self.unify(inference, a, c);
                self.unify(inference, b, d);
            }
            (TyKind::Fn(params, result, ..), TyKind::Fn(values, value, ..))
            | (TyKind::CFn(params, result, _), TyKind::CFn(values, value, _))
                if params.len() == values.len() =>
            {
                for (param, value) in params.into_iter().zip(values) {
                    self.unify(inference, param, value);
                }
                self.unify(inference, result, value);
            }
            (TyKind::Adt(_), TyKind::Adt(_)) => {
                if let (Some((template, args)), Some((other, values))) =
                    (self.types.instance_of(pattern), self.types.instance_of(actual))
                    && template == other
                {
                    let pairs: Vec<(Ty, Ty)> = args.iter().copied().zip(values.iter().copied()).collect();
                    for (arg, value) in pairs {
                        self.unify_argument(inference, arg, value);
                    }
                }
            }
            _ => {}
        }
    }

    /// `unify` for a type argument, where `Never` is a type like any other
    /// rather than the type of an expression that does not finish.
    pub(super) fn unify_argument(&mut self, inference: &mut Inference, pattern: Ty, actual: Ty) {
        if actual == Ty::NEVER && inference.vars.contains(&pattern) {
            inference.map.entry(pattern).or_insert(Ty::NEVER);
        } else {
            self.unify(inference, pattern, actual);
        }
    }

    /// For a generic record or enum named without type arguments (its
    /// instance with its own parameters), the inference of its arguments,
    /// seeded from the expected type.
    pub(super) fn generic_inference(&mut self, ty: Ty, expected: Option<Ty>) -> Option<Inference> {
        let (template, args) = self.types.instance_of(ty)?;
        let params = self.types.template(template).params.clone();
        if args != params.as_slice() {
            return None;
        }
        let mut inference = Inference::new(params);
        let expected = expected.map(|t| match self.types.kind(t) {
            TyKind::Option(inner) => *inner,
            _ => t,
        });
        if let Some(expected) = expected {
            self.unify(&mut inference, ty, expected);
        }
        Some(inference)
    }

    /// The instance of a generic type once every argument is inferred.
    pub(super) fn finish_inference(&mut self, inference: &Inference, generic: Ty, span: Span) -> Ty {
        let unknown: Vec<String> =
            inference.vars.iter().filter(|v| !inference.map.contains_key(v)).map(|&v| self.types.display(v)).collect();
        if !unknown.is_empty() {
            let (template, _) = self.types.instance_of(generic).unwrap();
            let name = self.types.template(template).name.clone();
            let message = format!(
                "cannot infer {} of `{name}`; annotate the expected type, such as `let value: {} = ...`",
                unknown.join(", "),
                self.types.display(generic)
            );
            self.diags.error("E1101", span, message);
            return Ty::ERROR;
        }
        self.types.subst(generic, &inference.map)
    }

    /// Whether two nominal types are the same, or instances of one generic type.
    pub(super) fn same_nominal(&self, a: Ty, b: Ty) -> bool {
        a == b
            || matches!((self.types.instance_of(a), self.types.instance_of(b)), (Some((x, _)), Some((y, _))) if x == y)
    }

    pub(super) fn record_literal(
        &mut self,
        cx: &mut FnCx,
        path: &[ast::Ident],
        type_args: &[ast::TypeExpr],
        inits: &[ast::FieldInit],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if let Some(resolved) = self.variant_path(path) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            if !type_args.is_empty() {
                return self.unsupported_expr(span, "type arguments on a variant");
            }
            let Payload::Record(fields) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take named fields", Self::path_text(path));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            let fields: Vec<(String, Ty)> = fields.into_iter().map(|f| (f.name, f.ty)).collect();
            let owner = self.index_type_name(ty).map(|e| format!("{e}.{}", path.last().unwrap().name));
            self.index_fields(owner, inits.iter().map(|i| (&i.name, i.value.is_none())));
            let (ty, values, seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
            self.report_missing(&fields, &seen, inits, span);
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        let name = path.last().expect("a path has a name");
        let ty = match self.type_names.get(&name.name) {
            // A type of another module, as `time.DateTime { ... }`.
            _ if path.len() > 1 => {
                let explicit = ast::TypeExpr {
                    kind: ast::TypeKind::Path { segments: path.to_vec(), args: type_args.to_vec() },
                    span: name.span,
                };
                match self.resolve_type(&explicit) {
                    Ty::ERROR => return Self::error_expr(span),
                    ty => ty,
                }
            }
            Some(TypeName::Nominal(ty)) if type_args.is_empty() => *ty,
            Some(TypeName::Alias(index)) if type_args.is_empty() => {
                let index = *index;
                self.resolve_alias(index)
            }
            Some(TypeName::Template(_)) if !type_args.is_empty() => {
                let explicit = ast::TypeExpr {
                    kind: ast::TypeKind::Path { segments: path.to_vec(), args: type_args.to_vec() },
                    span: name.span,
                };
                self.resolve_type(&explicit)
            }
            Some(TypeName::Template(template)) => {
                let template = *template;
                let params = self.types.template(template).params.clone();
                self.types.instantiate(template, params)
            }
            Some(_) => {
                let message = format!("`{}` is not generic: write `{} {{ ... }}`", name.name, name.name);
                self.diags.error("E1104", name.span, message);
                return Self::error_expr(span);
            }
            None => {
                self.diags.error("E1001", name.span, unknown_type(&name.name));
                return Self::error_expr(span);
            }
        };
        if path.len() == 1 {
            self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        }
        if self.types.c_layout(ty).is_some_and(|l| l.opaque) {
            let message =
                format!("{} is opaque: only C makes one, and Tondo holds a `Pointer` to it", self.display(ty));
            self.diags.error("E1703", name.span, message);
            return Self::error_expr(span);
        }
        let Some(fields) = self.record_fields(ty, name.span) else {
            return Self::error_expr(span);
        };
        // Outside its module, a literal leaves out the private fields, which
        // then need defaults; one that cannot is refused, and nothing more
        // is said of its fields.
        let mut refused = false;
        if let Some(hidden) = self.hidden_fields(ty).map(<[bool]>::to_vec) {
            let defaults = self.defaults_of(ty).unwrap_or_default();
            let written = inits.iter().find(|i| self.field_index(ty, &i.name.name).is_some_and(|k| hidden[k]));
            if hidden.iter().enumerate().any(|(k, &private)| private && !defaults.get(k).is_some_and(Option::is_some)) {
                let message = format!(
                    "{} has private fields, so only its module can build it with a literal; call a function of that module",
                    self.display(ty)
                );
                self.diags.error("E1502", name.span, message);
                refused = true;
            } else if let Some(init) = written {
                let message = format!(
                    "field `{}` of {} is private to its module: leave it out, and it has its default value",
                    init.name.name,
                    self.display(ty)
                );
                self.diags.error("E1501", init.name.span, message);
            }
        }
        self.index_fields(Some(name.name.clone()), inits.iter().map(|i| (&i.name, i.value.is_none())));
        let (ty, mut values, mut seen) = self.generic_fields(cx, ty, &fields, inits, expected, span);
        if self.types.c_layout(ty).is_some_and(|l| l.union) {
            return self.union_literal(cx, ty, values, span);
        }
        // A field a literal leaves out has its default: an annotation's
        // (27.13), or a record's.
        let defaults = match self.annotation_defaults.get(&ty) {
            Some(defaults) => Some(defaults.clone()),
            None => self.defaults_of(ty),
        };
        if let Some(defaults) = defaults {
            for (index, default) in defaults.into_iter().enumerate() {
                if let (false, Some(mut default)) = (seen[index], default) {
                    default.span = span;
                    values.push((index as u32, default));
                    seen[index] = true;
                }
            }
        }
        if !refused {
            self.report_missing(&fields, &seen, inits, span);
        }
        Self::mk(hir::ExprKind::Record(values), ty, span)
    }

    /// A C union from one of its fields, the rest of its bytes zero.
    pub(super) fn union_literal(
        &mut self,
        cx: &mut FnCx,
        ty: Ty,
        mut values: Vec<(u32, hir::Expr)>,
        span: Span,
    ) -> hir::Expr {
        if values.len() != 1 {
            let message = format!(
                "{} is a union: give one of its fields, which all share its memory, as in `{} {{ field: value }}`",
                self.display(ty),
                self.types.display(ty)
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let (index, value) = values.pop().unwrap();
        let local = self.hidden_local(cx, ty);
        let zeroed = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), ty, span);
        let stmts = vec![
            hir::Stmt::Let(local, zeroed),
            hir::Stmt::Assign { local, path: vec![hir::PlaceStep::Field(index)], op: None, value, span },
        ];
        let tail = Self::mk(hir::ExprKind::Local(local), ty, span);
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span)
    }

    /// The field values of a record or record variant of type `ty`. For a
    /// generic type named without arguments, they are inferred from the
    /// expected type and the values, which are then coerced.
    pub(super) fn generic_fields(
        &mut self,
        cx: &mut FnCx,
        ty: Ty,
        fields: &[(String, Ty)],
        inits: &[ast::FieldInit],
        expected: Option<Ty>,
        span: Span,
    ) -> (Ty, Vec<(u32, hir::Expr)>, Vec<bool>) {
        let Some(mut inference) = self.generic_inference(ty, expected) else {
            let (values, seen) = self.field_values(cx, ty, fields, inits, true, None);
            return (ty, values, seen);
        };
        let (values, seen) = self.field_values(cx, ty, fields, inits, true, Some(&mut inference));
        let concrete = self.finish_inference(&inference, ty, span);
        let values = values
            .into_iter()
            .map(|(index, value)| {
                let field = self.types.subst(fields[index as usize].1, &inference.map);
                (index, self.coerce(value, field))
            })
            .collect();
        (concrete, values, seen)
    }

    /// The fields a literal leaves out, but for one that a misspelled field
    /// of it was already told to be, as `emial` for `email`.
    pub(super) fn report_missing(
        &mut self,
        fields: &[(String, Ty)],
        seen: &[bool],
        inits: &[ast::FieldInit],
        span: Span,
    ) {
        let names = || fields.iter().map(|(n, _)| n.as_str());
        let meant: Vec<&str> = inits
            .iter()
            .filter(|init| !names().any(|n| n == init.name.name))
            .filter_map(|init| closest(&init.name.name, names()))
            .collect();
        let missing: Vec<(&String, &Ty)> = fields
            .iter()
            .zip(seen)
            .filter(|((n, _), s)| !**s && !meant.contains(&n.as_str()))
            .map(|((n, t), _)| (n, t))
            .collect();
        if !missing.is_empty() {
            let typed: Vec<String> = missing.iter().map(|(n, t)| format!("{n}: {}", self.types.display(**t))).collect();
            let what = if missing.len() == 1 { "the field" } else { "the fields" };
            let message = format!("this value is missing {what} {}", listed(&typed));
            self.diags.error("E1102", span, message);
            // Each with the value a new one starts from, when every one has
            // such a value.
            let values: Option<Vec<String>> =
                missing.iter().map(|(n, t)| Some(format!("{n}: {}", self.empty_value(**t)?))).collect();
            if let Some(values) = values
                && let Some(edit) = self.field_edit(span, &values)
            {
                let title =
                    format!("add {} {}", if values.len() == 1 { "the field" } else { "the fields" }, listed(&typed));
                self.diags.fix(title, false, vec![edit]);
            }
        }
    }

    /// The value a field of a type starts from when nothing says another:
    /// a zero, an empty text or collection, `none`, `false`.
    pub(super) fn empty_value(&self, ty: Ty) -> Option<&'static str> {
        Some(match self.types.kind(ty) {
            TyKind::Int(_) => "0",
            TyKind::Float(_) => "0.0",
            TyKind::Bool => "false",
            TyKind::String => "\"\"",
            TyKind::Option(_) => "none",
            TyKind::Array(_) => "[]",
            TyKind::Map(..) => "[:]",
            _ => return None,
        })
    }

    /// The edit that adds fields to a record literal before its closing
    /// brace: on their own lines in a literal of several lines, else after
    /// a comma.
    pub(super) fn field_edit(&self, literal: Span, fields: &[String]) -> Option<(Span, String)> {
        let file = self.sources.file_of(literal.start);
        let start = (literal.start - file.base) as usize;
        let text = &file.text[start..(literal.end - file.base) as usize];
        let open = text.find('{')?;
        if !text.ends_with('}') {
            return None;
        }
        let inside = &text[open + 1..text.len() - 1];
        let close = literal.end - 1;
        if inside.trim().is_empty() {
            let span = Span::new(literal.start + open as u32 + 1, close);
            return Some((span, format!(" {} ", fields.join(", "))));
        }
        if inside.contains('\n') {
            let line = file.text[..(close - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
            let indent: String = file.text[line..].chars().take_while(|c| *c == ' ').collect();
            let lines: String = fields.iter().map(|field| format!("{indent}    {field}\n")).collect();
            return Some((Span::new(file.base + line as u32, file.base + line as u32), lines));
        }
        let end = literal.start + (open + 1 + inside.trim_end().len()) as u32;
        let separator = if inside.trim_end().ends_with(',') { " " } else { ", " };
        Some((Span::new(end, end), format!("{separator}{}", fields.join(", "))))
    }
}
