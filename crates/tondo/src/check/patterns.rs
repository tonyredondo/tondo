//! `match`, `select`, patterns, `with`, updates and construction.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn path_text(path: &[ast::Ident]) -> String {
        path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
    }

    /// Resolves `Enum.Variant`. Returns `None` when the path does not name a
    /// type, and `Some(None)` after reporting a bad variant.
    pub(super) fn variant_path(&mut self, path: &[ast::Ident]) -> Option<Option<(Ty, u32)>> {
        let (variant, type_path) = path.split_last()?;
        let type_name = type_path.last()?;
        let ty = match type_path {
            [name] => match self.type_names.get(&name.name)? {
                TypeName::Nominal(ty) => *ty,
                TypeName::Alias(index) => {
                    let index = *index;
                    self.resolve_alias(index)
                }
                // The arguments of a generic enum are inferred at each use.
                TypeName::Template(template) => {
                    let template = *template;
                    let params = self.types.template(template).params.clone();
                    self.types.instantiate(template, params)
                }
            },
            [module, name] => self.module_type(&module.name, &name.name)?,
            _ => return None,
        };
        let found = match self.types.kind(ty) {
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Enum(variants) => Some(variants.iter().position(|v| v.name == variant.name)),
                _ => None,
            },
            _ => None,
        };
        match found {
            Some(Some(index)) => {
                if let Some(owner) = self.index_type_name(ty) {
                    self.index_refer(type_name, crate::index::Key::Type(type_name.name.clone()));
                    self.index_refer(variant, crate::index::Key::Variant(owner, variant.name.clone()));
                }
                Some(Some((ty, index as u32)))
            }
            Some(None) => {
                let mut message = format!("enum `{}` has no variant `{}`", type_name.name, variant.name);
                if let TyKind::Adt(id) = self.types.kind(ty)
                    && let AdtKind::Enum(variants) = &self.types.adt(*id).kind
                {
                    let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                    match closest(&variant.name, names.iter().map(String::as_str)) {
                        Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
                        None => message.push_str(&format!("; its variants are {}", listed(&names))),
                    }
                }
                self.diags.error("E1001", variant.span, message);
                self.offer_meant(variant.span);
                Some(None)
            }
            // A prelude name declared again is an error already (E1005).
            None if ty == Ty::ERROR || self.misnamed.contains(&type_name.name) => Some(None),
            None => {
                let message = format!("`{}` is not an enum", type_name.name);
                self.diags.error("E1102", type_name.span, message);
                Some(None)
            }
        }
    }

    pub(super) fn payload(&self, ty: Ty, variant: u32) -> Payload {
        let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
        let AdtKind::Enum(variants) = &self.types.adt(*id).kind else { unreachable!() };
        variants[variant as usize].payload.clone()
    }

    pub(super) fn match_expr(
        &mut self,
        cx: &mut FnCx,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let scrutinee = self.expr(cx, scrutinee, None);
        let mut result = expected;
        let mut out = Vec::new();
        // An arm whose value is an empty literal, `[]`, before any other with
        // a value: the others give it its type.
        let mut later = Vec::new();
        for arm in arms {
            cx.scopes.open();
            let outer = std::mem::replace(&mut cx.diverged, false);
            let pattern = self.pattern(
                cx,
                &arm.pattern,
                scrutinee.ty,
                PatternContext { in_match: true, mutable: false, loans: Loans::Shared },
            );
            let guard = arm.guard.as_ref().map(|guard| {
                let guard = self.expr(cx, guard, Some(Ty::BOOL));
                self.coerce(guard, Ty::BOOL)
            });
            let body = match &arm.body {
                ast::ArmBody::Expr(body) if result.is_none() && empty_literal(body) => {
                    later.push(out.len());
                    Self::error_expr(body.span)
                }
                _ => self.arm_body(cx, &arm.body, &mut result),
            };
            cx.diverged = outer;
            cx.scopes.close();
            out.push(hir::Arm { pattern, guard, body });
        }
        for index in later {
            out[index].body = self.arm_body(cx, &arms[index].body, &mut result);
        }
        let checked: Vec<(&hir::Pattern, bool)> = out.iter().map(|a| (&a.pattern, a.guard.is_some())).collect();
        if scrutinee.ty != Ty::ERROR && !checked.iter().any(|(p, _)| p.ty == Ty::ERROR) {
            let report = crate::exhaust::check_match(&self.types, scrutinee.ty, &checked);
            for index in report.unreachable {
                self.diags.error("E1203", out[index].pattern.span, "this arm is covered by earlier arms");
            }
            if let Some(missing) = report.missing {
                let message = format!("`match` is not exhaustive: `{missing}` is not covered");
                self.diags.error("E1204", span, message);
                // An arm for it before the closing brace, at the arms'
                // indentation, that says it is not written yet.
                let file = self.sources.file_of(span.start);
                let text = &file.text[(span.start - file.base) as usize..(span.end - file.base) as usize];
                if text.ends_with('}') {
                    let close = span.end - 1;
                    let line = file.text[..(close - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
                    let before = &file.text[line..(close - file.base) as usize];
                    let indent: String = file.text[line..].chars().take_while(|c| *c == ' ').collect();
                    let at = |arms: String| match before.trim().is_empty() {
                        true => (Span::new(file.base + line as u32, file.base + line as u32), arms),
                        false => (Span::new(close, close), format!("\n{arms}{indent}")),
                    };
                    let arm =
                        |missing: &str| format!("{indent}    {missing} => panic(\"`{missing}` is not handled yet\")\n");
                    self.diags.fix(format!("add an arm for `{missing}`"), false, vec![at(arm(&missing))]);
                    if report.every_missing.len() > 1 {
                        let arms: String = report.every_missing.iter().map(|m| arm(m)).collect();
                        let title = format!("add an arm for each of the {} cases missing", report.every_missing.len());
                        self.diags.fix(title, false, vec![at(arms)]);
                    }
                }
            }
        }
        let all_diverge = out.iter().all(|a| a.body.ty == Ty::NEVER);
        let ty = if all_diverge { Ty::NEVER } else { result.unwrap_or(Ty::UNIT) };
        Self::mk(hir::ExprKind::Match(Box::new(scrutinee), out), ty, span)
    }

    /// The body of a `match` or `select` arm, of the type the first arm
    /// with a value set in `result`.
    pub(super) fn arm_body(&mut self, cx: &mut FnCx, body: &ast::ArmBody, result: &mut Option<Ty>) -> hir::Expr {
        let body = match body {
            ast::ArmBody::Expr(body) => {
                let value = self.expr(cx, body, *result);
                match *result {
                    Some(expected) => self.coerce(value, expected),
                    None => value,
                }
            }
            ast::ArmBody::Statement(stmt) => {
                // An assignment or `_ =` gives `Unit`; a transfer leaves.
                let ty = match stmt.kind {
                    ast::StmtKind::Assign { .. } | ast::StmtKind::Discard(_) => Ty::UNIT,
                    _ => Ty::NEVER,
                };
                let stmts = self.statement(cx, stmt).into_iter().collect();
                let block = hir::Block { stmts, tail: None, ty };
                Self::mk(hir::ExprKind::Block(block), ty, stmt.span)
            }
        };
        if result.is_none() && body.ty != Ty::NEVER && body.ty != Ty::ERROR {
            *result = Some(body.ty);
        }
        body
    }

    /// `select { ... }` (11.14): waits for the first of its operations and
    /// runs that arm, or the `else` arm when none is ready.
    pub(super) fn select(
        &mut self,
        cx: &mut FnCx,
        arms: &[ast::SelectArm],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let mut result = expected;
        let mut out: Vec<hir::SelectArm> = Vec::new();
        let mut otherwise: Option<hir::Expr> = None;
        let mut taken: Vec<LocalId> = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            let outer = std::mem::replace(&mut cx.diverged, false);
            cx.scopes.open();
            match &arm.operation {
                None => {
                    if otherwise.is_some() {
                        self.diags.error("E1613", arm.span, "`select` has one `else` arm at most");
                    } else if index + 1 != arms.len() {
                        self.diags.error("E1613", arm.span, "the `else` arm of `select` comes last");
                    }
                    otherwise = Some(self.arm_body(cx, &arm.body, &mut result));
                }
                Some(operation) => {
                    let operation = self.expr(cx, operation, None);
                    let valid = matches!(
                        operation.kind,
                        hir::ExprKind::TaskOp(
                            hir::TaskOp::Receive | hir::TaskOp::Send | hir::TaskOp::Wait | hir::TaskOp::GroupNext,
                            _
                        ) | hir::ExprKind::Call(Callee::Std(StdFn::TimeSleep), ..)
                            | hir::ExprKind::Error
                    );
                    if let hir::ExprKind::TaskOp(hir::TaskOp::ReceiveTimeout, _) = operation.kind {
                        let message = "in `select`, a timeout is an arm of its own: `time.sleep(delay) => ...`";
                        self.diags.error("E1612", operation.span, message);
                    } else if !valid {
                        let message = "`select` waits for `receiver.receive()`, `sender.send(value)`, `job.wait()`, `group.next()` or `time.sleep(delay)`";
                        self.diags.error("E1612", operation.span, message);
                    }
                    // An affine value moves only in the arm whose operation
                    // completes, so no two arms may take the same one.
                    if let hir::ExprKind::TaskOp(op, operands) = &operation.kind {
                        for (position, operand) in operands.iter().enumerate() {
                            let Some(place) = hir::Place::of(operand.clone()) else { continue };
                            if !op.takes(position) || self.types.is_copy(operand.ty) {
                                continue;
                            }
                            if taken.contains(&place.local) {
                                let name = &cx.locals[place.local.0 as usize].name;
                                let message = format!(
                                    "`{name}` is taken by two arms of this `select`, but only one arm runs; give each arm its own value"
                                );
                                self.diags.error("E1615", operand.span, message);
                            }
                            taken.push(place.local);
                        }
                    }
                    let pattern = arm.pattern.as_ref().map(|p| {
                        let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
                        let pattern = self.pattern(cx, p, operation.ty, context);
                        if let Some(missing) = crate::exhaust::refutation(&self.types, operation.ty, &pattern) {
                            let message =
                                format!("this pattern can fail, for example on `{missing}`; match it in the body");
                            self.diags.error("E1201", pattern.span, message);
                        }
                        pattern
                    });
                    let body = self.arm_body(cx, &arm.body, &mut result);
                    out.push(hir::SelectArm { operation, pattern, body });
                }
            }
            cx.scopes.close();
            cx.diverged = outer;
        }
        if out.is_empty() {
            self.diags.error("E1613", span, "`select` needs at least one operation to wait for");
            return Self::error_expr(span);
        }
        let all_diverge = out.iter().map(|a| &a.body).chain(otherwise.as_ref()).all(|b| b.ty == Ty::NEVER);
        let ty = if all_diverge { Ty::NEVER } else { result.unwrap_or(Ty::UNIT) };
        Self::mk(hir::ExprKind::Select(out, otherwise.map(Box::new)), ty, span)
    }

    pub(super) fn pattern_error(&mut self, span: Span, message: String) -> hir::Pattern {
        self.diags.error("E1202", span, message);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    /// Checks a pattern against a value of type `ty`, declaring its bindings
    /// in the current scope. Those of a pattern that does not fit, or of a
    /// value already wrong, are declared as wrong too, so that their uses
    /// add no errors of their own.
    pub(super) fn pattern(&mut self, cx: &mut FnCx, p: &ast::Pattern, ty: Ty, context: PatternContext) -> hir::Pattern {
        let simple =
            matches!(p.kind, ast::PatternKind::Binding(_) | ast::PatternKind::Loan(..) | ast::PatternKind::Wildcard);
        if matches!(self.types.kind(ty), TyKind::Error) && !simple {
            self.declare_failed(cx, p);
            return Self::wild(ty, p.span);
        }
        let before = self.diags.list.len();
        let checked = self.checked_pattern(cx, p, ty, context);
        let failed = self.diags.list[before..].iter().any(|d| d.severity == crate::diag::Severity::Error);
        if failed && matches!(checked.kind, hir::PatKind::Wildcard) {
            self.declare_failed(cx, p);
        }
        checked
    }

    /// Declares the bindings of a pattern that failed, as wrong.
    pub(super) fn declare_failed(&mut self, cx: &mut FnCx, p: &ast::Pattern) {
        let mut names = Vec::new();
        bound_idents(p, &mut names);
        for name in names {
            if name.name != "_" && self.lookup(cx, &name.name).is_none() {
                self.declare(cx, name, Ty::ERROR, false);
            }
        }
    }

    pub(super) fn checked_pattern(
        &mut self,
        cx: &mut FnCx,
        p: &ast::Pattern,
        ty: Ty,
        context: PatternContext,
    ) -> hir::Pattern {
        let span = p.span;
        if let Some(members) = self.types.members(ty).map(<[Ty]>::to_vec)
            && !matches!(p.kind, ast::PatternKind::Wildcard)
            && let Some(pattern) = self.union_pattern(cx, p, ty, &members, context)
        {
            return pattern;
        }
        let mismatch = |this: &Self, what: &str| format!("{what} cannot match a value of type {}", this.display(ty));
        let kind = match &p.kind {
            ast::PatternKind::Wildcard => hir::PatKind::Wildcard,
            ast::PatternKind::Binding(name) => {
                let message = match name.name.as_str() {
                    "None" => Some("`None` is not a Tondo pattern; write `none`".to_string()),
                    "Some" | "Ok" | "Err" => Some(format!(
                        "`{}` is not a Tondo pattern; write `{}(...)`",
                        name.name,
                        name.name.to_lowercase()
                    )),
                    n if context.in_match && CONTEXTUAL_CONSTRUCTORS.contains(&n) => {
                        Some(format!("`{n}` without parentheses would match any value; write `{n}(...)` or rename it"))
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    return self.pattern_error(name.span, message);
                }
                hir::PatKind::Binding(self.declare(cx, name, ty, context.mutable))
            }
            ast::PatternKind::Loan(mode, name) => {
                let allowed = match context.loans {
                    Loans::Any => true,
                    Loans::Shared => *mode == ast::Mode::Ref,
                    Loans::None => false,
                };
                if !allowed {
                    let message = if context.loans == Loans::Shared {
                        format!(
                            "a `match` arm only observes its value, so it cannot bind `{} {}`: `mut` and `var` bindings belong in `for` headers",
                            mode.keyword(),
                            name.name
                        )
                    } else {
                        format!(
                            "`{} {}` borrows a part, which only `match` arms and `for` loops over collections do; bind it by value",
                            mode.keyword(),
                            name.name
                        )
                    };
                    return self.pattern_error(span, message);
                }
                let local = self.declare(cx, name, ty, *mode != ast::Mode::Ref);
                cx.params[local.0 as usize] = Some(*mode);
                cx.loan_bindings.insert(local);
                hir::PatKind::Binding(local)
            }
            ast::PatternKind::Literal(expr) => {
                let value = self.expr(cx, expr, Some(ty));
                let literal = match value.kind {
                    hir::ExprKind::Int(v) => hir::Literal::Int(v),
                    hir::ExprKind::Float(v) => hir::Literal::Float(v.to_bits()),
                    hir::ExprKind::Char(c) => hir::Literal::Char(c),
                    hir::ExprKind::Bool(b) => hir::Literal::Bool(b),
                    hir::ExprKind::Str(s) => hir::Literal::Str(s),
                    hir::ExprKind::Unit => hir::Literal::Unit,
                    _ => return self.unsupported_pattern(span, "this literal pattern"),
                };
                if value.ty != ty && ty != Ty::ERROR && value.ty != Ty::ERROR {
                    let message = mismatch(self, &format!("a {} literal", self.display(value.ty)));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Literal(literal)
            }
            ast::PatternKind::None if !context.in_match => {
                return self.pattern_error(span, "`none` is a reserved keyword and cannot name a binding".into());
            }
            ast::PatternKind::None => match self.types.kind(ty) {
                TyKind::Option(_) => hir::PatKind::Variant(NONE, Vec::new()),
                TyKind::Error => hir::PatKind::Wildcard,
                _ => return self.pattern_error(span, mismatch(self, "`none`")),
            },
            ast::PatternKind::Tuple(items) => match self.types.kind(ty) {
                TyKind::Tuple(types) if types.len() == items.len() => {
                    let types = types.clone();
                    let fields = items.iter().zip(types).map(|(item, t)| self.pattern(cx, item, t, context)).collect();
                    hir::PatKind::Fields(fields)
                }
                TyKind::Error => hir::PatKind::Wildcard,
                _ => {
                    let message = mismatch(self, &format!("a tuple pattern of {} elements", items.len()));
                    return self.pattern_error(span, message);
                }
            },
            ast::PatternKind::Constructor { path, args } => {
                if let [name] = path.as_slice()
                    && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str())
                {
                    let (variant, payload) = match (name.name.as_str(), self.types.kind(ty)) {
                        ("some", TyKind::Option(inner)) => (SOME, *inner),
                        ("ok", TyKind::Result(ok, _)) => (OK, *ok),
                        ("err", TyKind::Result(_, err)) => (ERR, *err),
                        (_, TyKind::Error) => return Self::wild(ty, span),
                        _ => return self.pattern_error(span, mismatch(self, &format!("`{}(...)`", name.name))),
                    };
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let inner = self.pattern(cx, arg, payload, context);
                    return hir::Pattern { kind: hir::PatKind::Variant(variant, vec![inner]), ty, span };
                }
                if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    if !self.same_nominal(enum_ty, ty) {
                        let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                        return self.pattern_error(span, message);
                    }
                    let Payload::Tuple(types) = self.payload(ty, variant) else {
                        let message = format!("`{}` has no positional payload", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    if types.len() != args.len() {
                        let message = format!("`{}` has {} payload values", Self::path_text(path), types.len());
                        return self.pattern_error(span, message);
                    }
                    let fields = args.iter().zip(types).map(|(arg, t)| self.pattern(cx, arg, t, context)).collect();
                    hir::PatKind::Variant(variant, fields)
                } else if let [name] = path.as_slice()
                    && let Some(&ModuleName::Newtype(newtype)) = self.names.get(&name.name)
                {
                    if newtype != ty {
                        let message = mismatch(self, &format!("a `{}` pattern", name.name));
                        return self.pattern_error(span, message);
                    }
                    let [arg] = args.as_slice() else {
                        return self.pattern_error(span, format!("`{}(...)` takes one pattern", name.name));
                    };
                    let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                    let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                    hir::PatKind::Newtype(Box::new(self.pattern(cx, arg, inner, context)))
                } else {
                    let message = format!("`{}` is not a constructor", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
            }
            ast::PatternKind::Record { path, fields, rest } => {
                let (record_ty, declared, variant) = if let Some(resolved) = self.variant_path(path) {
                    let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                    // A generic enum is matched as the scrutinee's instance.
                    let enum_ty = if self.same_nominal(enum_ty, ty) { ty } else { enum_ty };
                    let Payload::Record(fields) = self.payload(enum_ty, variant) else {
                        let message = format!("`{}` has no named fields", Self::path_text(path));
                        return self.pattern_error(span, message);
                    };
                    (enum_ty, fields.into_iter().map(|f| (f.name, f.ty)).collect(), Some(variant))
                } else if let [name] = path.as_slice()
                    && let Some(record) = self.type_named(&name.name)
                {
                    let record = if self.same_nominal(record, ty) { ty } else { record };
                    let Some(fields) = self.record_fields(record, name.span) else { return Self::wild(ty, span) };
                    if let Some(hidden) = self.hidden_fields(record).map(<[bool]>::to_vec) {
                        let ast::PatternKind::Record { fields: written, .. } = &p.kind else { unreachable!() };
                        let named = written.iter().find(|w| {
                            fields.iter().zip(&hidden).any(|((name, _), private)| *private && *name == w.name.name)
                        });
                        let message = match named {
                            Some(field) => Some(format!(
                                "field `{}` of {} is private to its module",
                                field.name.name,
                                self.display(record)
                            )),
                            None if !*rest => {
                                Some(format!("{} has private fields; end the pattern with `..`", self.display(record)))
                            }
                            None => None,
                        };
                        if let Some(message) = message {
                            self.diags.error("E1501", span, message);
                            return Self::wild(ty, span);
                        }
                    }
                    (record, fields, None)
                } else {
                    let message = format!("`{}` is not a record type or variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                if record_ty != ty {
                    let message = mismatch(self, &format!("a `{}` pattern", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                let owner = self.index_type_name(record_ty).map(|owner| match variant {
                    Some(_) => format!("{owner}.{}", path.last().unwrap().name),
                    None => owner,
                });
                if variant.is_none()
                    && let [name] = path.as_slice()
                {
                    self.index_refer(name, crate::index::Key::Type(name.name.clone()));
                }
                self.index_fields(owner, fields.iter().map(|f| (&f.name, f.pattern.is_none())));
                let patterns = self.field_patterns(cx, &declared, fields, *rest, context, span);
                match variant {
                    Some(variant) => hir::PatKind::Variant(variant, patterns),
                    None => hir::PatKind::Fields(patterns),
                }
            }
            ast::PatternKind::Member { .. } => {
                return self.pattern_error(span, mismatch(self, "a member pattern"));
            }
            ast::PatternKind::Array { items, rest } => {
                let element = match *self.types.kind(ty) {
                    TyKind::Array(element) => element,
                    TyKind::Error => return Self::wild(ty, span),
                    _ => return self.pattern_error(span, mismatch(self, "an array pattern")),
                };
                let items = items.iter().map(|item| self.pattern(cx, item, element, context)).collect();
                let mut bound = None;
                if let Some(Some((mode, name))) = rest {
                    if *mode == ast::Mode::Ref && context.loans == Loans::None {
                        let message = format!(
                            "`..ref {}` borrows the rest, which only `match` arms and `for` loops do; write `..{0}`",
                            name.name
                        );
                        return self.pattern_error(span, message);
                    }
                    let local = self.declare(cx, name, ty, context.mutable && *mode == ast::Mode::Value);
                    if *mode == ast::Mode::Ref {
                        cx.params[local.0 as usize] = Some(ast::Mode::Ref);
                        cx.loan_bindings.insert(local);
                    }
                    bound = Some(local);
                }
                let rest = rest.as_ref().map(|_| bound);
                hir::PatKind::Array { items, rest }
            }
            ast::PatternKind::Path(path) => {
                let Some(resolved) = self.variant_path(path) else {
                    let message = format!("`{}` is not a variant", Self::path_text(path));
                    return self.pattern_error(span, message);
                };
                let Some((enum_ty, variant)) = resolved else { return Self::wild(ty, span) };
                if !self.same_nominal(enum_ty, ty) {
                    let message = mismatch(self, &format!("`{}`", Self::path_text(path)));
                    return self.pattern_error(span, message);
                }
                if !matches!(self.payload(ty, variant), Payload::Unit) {
                    let message = format!("`{}` has a payload; match it explicitly", Self::path_text(path));
                    return self.pattern_error(span, message);
                }
                hir::PatKind::Variant(variant, Vec::new())
            }
        };
        hir::Pattern { kind, ty, span }
    }

    /// A pattern against a union (9.6): `Member(pattern)` tests the tag and
    /// matches the whole member, and a pattern of a member's own type (a
    /// record, variant, option or literal) selects that member. `None` lets
    /// a binding match the whole union.
    pub(super) fn union_pattern(
        &mut self,
        cx: &mut FnCx,
        p: &ast::Pattern,
        ty: Ty,
        members: &[Ty],
        context: PatternContext,
    ) -> Option<hir::Pattern> {
        let span = p.span;
        let explicit = match &p.kind {
            ast::PatternKind::Member { ty: member, inner } => Some((self.resolve_type(member), &**inner)),
            // `Int(n)`, or a type of a module, `time.DateTime(at)`; not a
            // variant, `Shape.Circle(r)`.
            ast::PatternKind::Constructor { path, args } if path.len() <= 2 => {
                let member = match path.as_slice() {
                    [name] => self.type_named(&name.name),
                    [module, name] if !self.type_names.contains_key(&module.name) => {
                        self.module_type(&module.name, &name.name)
                    }
                    _ => None,
                };
                match member {
                    Some(member) => {
                        let [inner] = args.as_slice() else {
                            let name = Self::path_text(path);
                            return Some(self.pattern_error(span, format!("`{name}(...)` takes one pattern")));
                        };
                        Some((member, inner))
                    }
                    None => None,
                }
            }
            ast::PatternKind::Binding(name) if self.type_named(&name.name).is_some() => {
                let message = format!(
                    "`{0}` alone would bind any value; test the member with `{0}(_)` or `{0}(name)`",
                    name.name
                );
                return Some(self.pattern_error(name.span, message));
            }
            _ => None,
        };
        let (member, inner) = match explicit {
            Some((member, inner)) => (member, Some(inner)),
            None => (self.pattern_member_type(p, ty, members)?, None),
        };
        if member == Ty::ERROR {
            return Some(Self::wild(ty, span));
        }
        let Some(index) = members.iter().position(|&m| m == member) else {
            let message = format!("{} is not a member of {}", self.display(member), self.display(ty));
            return Some(self.pattern_error(span, message));
        };
        let inner = self.pattern(cx, inner.unwrap_or(p), member, context);
        Some(hir::Pattern { kind: hir::PatKind::Variant(index as u32, vec![inner]), ty, span })
    }

    /// The member type a nominal, option or literal pattern belongs to.
    pub(super) fn pattern_member_type(&mut self, p: &ast::Pattern, ty: Ty, members: &[Ty]) -> Option<Ty> {
        let only = |this: &Self, test: fn(&TyKind) -> bool| {
            let found: Vec<Ty> = members.iter().copied().filter(|&m| test(this.types.kind(m))).collect();
            match found.as_slice() {
                [only] => Some(*only),
                _ => None,
            }
        };
        match &p.kind {
            ast::PatternKind::Record { path, .. } | ast::PatternKind::Path(path) => match self.variant_path(path) {
                Some(resolved) => Some(resolved.map_or(Ty::ERROR, |(enum_ty, _)| enum_ty)),
                None if path.len() == 1 => self.type_named(&path[0].name),
                None => None,
            },
            ast::PatternKind::Constructor { path, .. } => match path.as_slice() {
                [name] if matches!(name.name.as_str(), "some") => only(self, |k| matches!(k, TyKind::Option(_))),
                [name] if matches!(name.name.as_str(), "ok" | "err") => only(self, |k| matches!(k, TyKind::Result(..))),
                _ => self.variant_path(path).map(|resolved| resolved.map_or(Ty::ERROR, |(enum_ty, _)| enum_ty)),
            },
            ast::PatternKind::None => only(self, |k| matches!(k, TyKind::Option(_))),
            ast::PatternKind::Literal(expr) => self.literal_type(expr, ty),
            _ => None,
        }
    }

    /// The type a literal pattern has when matched against `ty`.
    pub(super) fn literal_type(&self, expr: &ast::Expr, ty: Ty) -> Option<Ty> {
        Some(match &expr.kind {
            ast::ExprKind::Int { suffix: Some(suffix), .. } => Ty::int(IntTy::from_suffix(suffix)),
            ast::ExprKind::Int { .. } if ty == Ty::BYTE => Ty::BYTE,
            ast::ExprKind::Int { .. } => self.expected_number(Some(ty), false).unwrap_or(Ty::INT),
            ast::ExprKind::Float { suffix: Some(suffix), .. } => {
                if *suffix == "f32" {
                    Ty::F32
                } else {
                    Ty::F64
                }
            }
            ast::ExprKind::Float { .. } => self.expected_number(Some(ty), true).unwrap_or(Ty::F64),
            ast::ExprKind::Unary(_, inner) => return self.literal_type(inner, ty),
            ast::ExprKind::Str(_) => Ty::STRING,
            ast::ExprKind::Char(_) => Ty::CHAR,
            ast::ExprKind::Bool(_) => Ty::BOOL,
            _ => return None,
        })
    }

    /// The type a single name denotes, without reporting unknown names.
    pub(super) fn type_named(&mut self, name: &str) -> Option<Ty> {
        // A type parameter in scope, as `T` in `T.max`.
        if let Some(&(_, param)) = self.type_params.iter().rev().find(|(n, _)| n == name) {
            return Some(param);
        }
        // A generic type stands for its instance with its own parameters.
        if let Some(&TypeName::Template(template)) = self.type_names.get(name) {
            let params = self.types.template(template).params.clone();
            return Some(self.types.instantiate(template, params));
        }
        let path = ast::TypeExpr {
            kind: ast::TypeKind::Path {
                segments: vec![ast::Ident { name: name.into(), span: Span::default() }],
                args: Vec::new(),
            },
            span: Span::default(),
        };
        let builtin = matches!(
            name,
            "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "Int128"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "UInt128"
                | "Bool"
                | "String"
                | "Float"
                | "Float32"
                | "Float64"
                | "Char"
                | "Byte"
        );
        (builtin || self.type_names.contains_key(name)).then(|| self.resolve_type(&path))
    }

    pub(super) fn wild(ty: Ty, span: Span) -> hir::Pattern {
        hir::Pattern { kind: hir::PatKind::Wildcard, ty, span }
    }

    pub(super) fn unsupported_pattern(&mut self, span: Span, what: &str) -> hir::Pattern {
        self.diags.unsupported(span, what);
        hir::Pattern { kind: hir::PatKind::Wildcard, ty: Ty::ERROR, span }
    }

    pub(super) fn field_patterns(
        &mut self,
        cx: &mut FnCx,
        declared: &[(String, Ty)],
        fields: &[ast::FieldPattern],
        rest: bool,
        context: PatternContext,
        span: Span,
    ) -> Vec<hir::Pattern> {
        let mut patterns: Vec<Option<hir::Pattern>> = (0..declared.len()).map(|_| None).collect();
        for field in fields {
            let Some(index) = declared.iter().position(|(n, _)| *n == field.name.name) else {
                let message = format!("there is no field `{}`", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            };
            if patterns[index].is_some() {
                let message = format!("field `{}` appears more than once", field.name.name);
                self.diags.error("E1202", field.name.span, message);
                continue;
            }
            let ty = declared[index].1;
            let pattern = match &field.pattern {
                Some(pattern) => self.pattern(cx, pattern, ty, context),
                None => {
                    let binding =
                        ast::Pattern { kind: ast::PatternKind::Binding(field.name.clone()), span: field.name.span };
                    self.pattern(cx, &binding, ty, context)
                }
            };
            patterns[index] = Some(pattern);
        }
        let missing: Vec<&str> =
            declared.iter().zip(&patterns).filter(|(_, p)| p.is_none()).map(|((n, _), _)| n.as_str()).collect();
        if !rest && !missing.is_empty() {
            let message = format!("pattern does not mention {}; add `..` to ignore them", missing.join(", "));
            self.diags.error("E1202", span, message);
        }
        patterns
            .into_iter()
            .zip(declared)
            .map(|(p, (_, ty))| p.unwrap_or(hir::Pattern { kind: hir::PatKind::Wildcard, ty: *ty, span }))
            .collect()
    }

    pub(super) fn with_expr(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        updates: &[ast::FieldInit],
        span: Span,
    ) -> hir::Expr {
        let base = self.expr(cx, base, None);
        self.update(cx, base, updates, true, span)
    }

    /// `base` with the fields of `updates` changed; with `shorthand`, a
    /// field may be named alone, as the binding of its name.
    pub(super) fn update(
        &mut self,
        cx: &mut FnCx,
        base: hir::Expr,
        updates: &[ast::FieldInit],
        shorthand: bool,
        span: Span,
    ) -> hir::Expr {
        let ty = base.ty;
        let Some(fields) = self.record_fields(ty, base.span) else {
            return Self::error_expr(span);
        };
        if updates.is_empty() {
            self.diags.error("E1102", span, "`with` must update at least one field");
        }
        if let Some(hidden) = self.hidden_fields(ty).map(<[bool]>::to_vec) {
            for update in updates {
                if let Some(index) = self.field_index(ty, &update.name.name)
                    && hidden[index]
                {
                    let message =
                        format!("field `{}` of {} is private to its module", update.name.name, self.display(ty));
                    self.diags.error("E1501", update.name.span, message);
                }
            }
        }
        let owner = self.index_type_name(ty);
        self.index_fields(owner, updates.iter().map(|u| (&u.name, false)));
        let (values, _) = self.field_values(cx, ty, &fields, updates, shorthand, None);
        if self.types.c_layout(ty).is_some_and(|l| l.union) {
            // The fields of a union overlap: a copy, then the fields written.
            let local = self.hidden_local(cx, ty);
            let mut stmts = vec![hir::Stmt::Let(local, base)];
            for (index, value) in values {
                stmts.push(hir::Stmt::Assign {
                    local,
                    path: vec![hir::PlaceStep::Field(index)],
                    op: None,
                    value,
                    span,
                });
            }
            let tail = Self::mk(hir::ExprKind::Local(local), ty, span);
            return Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span);
        }
        Self::mk(hir::ExprKind::With(Box::new(base), values), ty, span)
    }

    /// `some(value)`, `ok(value)` or `err(value)`.
    pub(super) fn construct(
        &mut self,
        cx: &mut FnCx,
        constructor: &str,
        value: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let member = self.union_member(expected, constructor);
        // `some(value)` where an option of a type that is not one itself is
        // expected, and not as a member of a union: the value alone
        // becomes it (9.7).
        let lifted = constructor == "some"
            && member == expected
            && expected.is_some_and(|t| {
                matches!(self.types.kind(t), TyKind::Option(inner) if !matches!(self.types.kind(*inner), TyKind::Option(_)))
            });
        let written = value;
        let expected = member;
        let expected_kind = expected.map(|t| self.types.kind(t).clone());
        let (variant, payload_ty, ty) = match (constructor, expected_kind) {
            ("some", Some(TyKind::Option(inner))) => (SOME, Some(inner), expected),
            ("some", _) => (SOME, None, None),
            ("ok", Some(TyKind::Result(ok, _))) => (OK, Some(ok), expected),
            ("err", Some(TyKind::Result(_, err))) => (ERR, Some(err), expected),
            _ => {
                let message = format!("`{constructor}(...)` needs an expected `T ! E` type");
                self.diags.error("E1304", span, message);
                self.expr(cx, value, None);
                return Self::error_expr(span);
            }
        };
        let value = self.expr(cx, value, payload_ty);
        if lifted
            && Some(value.ty) == payload_ty
            && value.ty != Ty::ERROR
            && value.ty != Ty::NEVER
            && self.inferring == 0
            && self.typed_alone(&value)
        {
            self.needless_some(written, value.ty, span);
        }
        let value = match payload_ty {
            Some(payload) => self.coerce(value, payload),
            None => value,
        };
        let ty = ty.unwrap_or_else(|| self.types.intern(TyKind::Option(value.ty)));
        Self::mk(hir::ExprKind::Variant(variant, vec![(0, value)]), ty, span)
    }

    /// Whether a value is typed the same where its option is expected, so
    /// that `some` around it can go: not what may take its type from the one
    /// expected, as the result of a generic call, `json.decode(text)?` or
    /// `value.to()?`, a branch, a block, a closure or a literal of a query.
    fn typed_alone(&self, value: &hir::Expr) -> bool {
        use hir::ExprKind as K;
        match &value.kind {
            K::Int(_) | K::Float(_) | K::Char(_) | K::Bool(_) | K::Unit | K::Local(_) => true,
            K::Str(_) | K::Interpolate(_) => value.ty == Ty::STRING,
            K::Field(base, _) | K::Try(base) | K::Length(base) | K::Unary(_, base) | K::Retype(base) => {
                self.typed_alone(base)
            }
            K::Convert(base) => self.typed_alone(base),
            K::Binary(_, left, right) => self.typed_alone(left) && self.typed_alone(right),
            K::Call(_, types, _) => types.is_empty(),
            K::ArrayOp(..) | K::StrOp(..) | K::MapOp(..) => true,
            K::Array(items) | K::Set(items) | K::Aggregate(items) => items.iter().all(|item| self.typed_alone(item)),
            K::Map(pairs) => pairs.iter().all(|(key, item)| self.typed_alone(key) && self.typed_alone(item)),
            K::Record(fields) | K::Variant(_, fields) => fields.iter().all(|(_, field)| self.typed_alone(field)),
            _ => false,
        }
    }

    /// W1021 for a `some(value)` that the value alone would be, with the
    /// value written in its place: in parentheses when it is an operation
    /// that is the operand of another, as `flag == some(a and b)`.
    fn needless_some(&mut self, value: &ast::Expr, ty: Ty, span: Span) {
        let option = self.types.intern(TyKind::Option(ty));
        let message = format!(
            "`some` is not needed here: where {} is expected, the value becomes one by itself; write it alone",
            self.display(option)
        );
        self.diags.warning("W1021", span, message);
        let source = self.sources.slice(value.span).to_string();
        let file = self.sources.file_of(span.start);
        let before = file.text[..(span.start - file.base) as usize].trim_end();
        // After an operator, not after `=` or `=>`.
        let symbol = ["==", "!=", "<=", ">="].iter().any(|op| before.ends_with(op))
            || (before.ends_with(['<', '>', '+', '-', '*', '/', '%', '&', '|', '^', '?']) && !before.ends_with("=>"));
        let word = |w: &str| {
            before.strip_suffix(w).is_some_and(|rest| !rest.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        };
        let operand = symbol || ["and", "or", "not", "in"].into_iter().any(word);
        let operation = matches!(value.kind, ast::ExprKind::Binary(..) | ast::ExprKind::Unary(..));
        let replacement = if operand && operation { format!("({source})") } else { source };
        self.diags.fix("remove `some`", true, vec![(span, replacement)]);
    }

    /// The type `none`, `some(...)`, `ok(...)` or `err(...)` builds: the
    /// expected type, or the only option or result member of an expected
    /// union, into which the value is then injected.
    pub(super) fn union_member(&self, expected: Option<Ty>, constructor: &str) -> Option<Ty> {
        let Some(members) = expected.and_then(|t| self.types.members(t)) else { return expected };
        let option = matches!(constructor, "none" | "some");
        let found: Vec<Ty> = members
            .iter()
            .copied()
            .filter(|&m| match self.types.kind(m) {
                TyKind::Option(_) => option,
                TyKind::Result(..) => !option,
                _ => false,
            })
            .collect();
        match found.as_slice() {
            [only] => Some(*only),
            _ => expected,
        }
    }
}
