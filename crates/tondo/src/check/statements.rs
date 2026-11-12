//! Declarations, blocks, statements, assignment, places, loans and `for`.

use super::*;

impl<'a> Checker<'a> {
    /// A local that no name refers to, such as the value of `for _ in ...`.
    pub(super) fn hidden_local(&mut self, cx: &mut FnCx, ty: Ty) -> LocalId {
        cx.locals.push(hir::Local { name: "_".into(), ty, mutable: false, span: Span::default() });
        cx.mutable.push(false);
        cx.params.push(None);
        LocalId(cx.locals.len() as u32 - 1)
    }

    pub(super) fn declare(&mut self, cx: &mut FnCx, name: &ast::Ident, ty: Ty, mutable: bool) -> LocalId {
        self.declare_with(cx, name, ty, mutable, None)
    }

    /// Declares a binding, recorded for an editor under `key` when it is a
    /// parameter, which calls name too.
    pub(super) fn declare_with(
        &mut self,
        cx: &mut FnCx,
        name: &ast::Ident,
        ty: Ty,
        mutable: bool,
        key: Option<crate::index::Key>,
    ) -> LocalId {
        // `_` takes a value without binding it (5.4).
        let discard = name.name == "_";
        // Another module's bindings may reuse the root's names, which only
        // the root sees unprefixed; the linker checks each module's own.
        if !discard
            && (self.lookup(cx, &name.name).is_some()
                || self.visible_capture(cx, &name.name)
                || (self.module == 0 && self.names.contains_key(&name.name)))
        {
            let message = format!(
                "`{0}` would shadow a visible binding with the same name: choose another name, or assign `{0} = ...` if it is a `var`",
                name.name
            );
            self.diags.error("E1003", name.span, message);
            if let Some(earlier) = self.binding_span(cx, &name.name) {
                self.diags.related(earlier, format!("`{}` is declared here", name.name));
            }
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        }
        let id = LocalId(cx.locals.len() as u32);
        let kind = if key.is_some() { crate::index::DefKind::Parameter } else { crate::index::DefKind::Local };
        if let Some(def) = self.index_define(key, kind, name, name.span, Some(ty)) {
            cx.local_defs.insert(id.0, def);
        }
        cx.locals.push(hir::Local { name: name.name.clone(), ty, mutable, span: name.span });
        cx.mutable.push(mutable);
        cx.params.push(None);
        if !discard {
            cx.scopes.bind(name.name.clone(), id);
            cx.named.push(id);
        }
        id
    }

    /// Whether `param` is there only for its type: it alone gives a `T` of
    /// the function, and the body names that `T`.
    pub(super) fn gives_type(&self, f: &ast::FnDecl, param: &ast::Param) -> bool {
        let (Some(ty), Some(body)) = (&param.ty, &f.body) else { return false };
        let file = self.sources.file_of(ty.span.start);
        let name = file.slice(ty.span);
        f.generics.iter().any(|generic| generic.name.name == name)
            && crate::lints::names(file, body.span, name)
            && !f.params.iter().any(|other| {
                !std::ptr::eq(other, param)
                    && other.ty.as_ref().is_some_and(|ty| crate::lints::names(file, ty.span, name))
            })
    }

    /// Warns of the bindings and parameters a body never reads (`W1002`,
    /// `W1003`); `self` and names that start with `_` are exempt. A body
    /// with an error has none: the error may be where a binding is used,
    /// in code the checker could not follow.
    pub(super) fn unused(&mut self, cx: &FnCx, params: &[LocalId]) {
        if self.error_count() > cx.errors_before {
            return;
        }
        for &local in &cx.named {
            let hir::Local { name, span, .. } = &cx.locals[local.0 as usize];
            if cx.reads.contains(&local) || name == "self" || !name.starts_with(char::is_alphabetic) {
                continue;
            }
            if params.contains(&local) {
                let message = format!(
                    "the parameter `{name}` is never used: write `_` instead, or `_{name}` to hold its value until the function ends"
                );
                self.diags.warning("W1003", *span, message);
                self.diags.fix(format!("rename it `_{name}`"), false, vec![(*span, format!("_{name}"))]);
            } else {
                let message = format!(
                    "`{name}` is never used: write `_` instead, or `_{name}` to hold its value until the block ends"
                );
                self.diags.warning("W1002", *span, message);
                self.diags.fix(format!("rename it `_{name}`"), false, vec![(*span, format!("_{name}"))]);
            }
        }
    }

    // ---- statements ------------------------------------------------------

    pub(super) fn block(&mut self, cx: &mut FnCx, b: &ast::Block, expected: Option<Ty>) -> hir::Block {
        let body = std::mem::take(&mut cx.body);
        cx.scopes.open();
        let withs = cx.withs.len();
        let outer = std::mem::replace(&mut cx.diverged, false);
        let stmts = self.statements(cx, &b.stmts);
        let tail = b.tail.as_ref().map(|t| {
            if cx.diverged {
                self.diags.warning("W1006", t.span, "unreachable expression");
            }
            let value = self.expr(cx, t, expected);
            if expected == Some(Ty::UNIT) {
                self.dropped_value(t, &value);
            }
            Box::new(match expected {
                Some(expected) if body => self.returned(cx, value, expected),
                Some(expected) => self.coerce(value, expected),
                None => value,
            })
        });
        let ty = match &tail {
            _ if cx.diverged => Ty::NEVER,
            Some(tail) => tail.ty,
            None => match expected.filter(|&t| t != Ty::UNIT && t != Ty::ERROR) {
                Some(expected) => {
                    let mut message = match body {
                        true => format!(
                            "this function returns {}, but its body ends without a value: end it with one, or `return` one",
                            self.display(expected)
                        ),
                        false => format!("expected {}, found `Unit`", self.display(expected)),
                    };
                    // `{}` written for an empty collection.
                    let empty = match self.types.kind(expected) {
                        _ if !stmts.is_empty() => None,
                        TyKind::Map(..) => Some("an empty map is `[:]`"),
                        TyKind::Set(_) => Some("an empty set is `Set[]`"),
                        TyKind::Array(_) => Some("an empty array is `[]`"),
                        _ => None,
                    };
                    if let Some(empty) = empty {
                        message.push_str(&format!(": `{{}}` is an empty block; {empty}"));
                    }
                    let end = Span::new(b.span.end - 1, b.span.end);
                    self.diags.error("E1102", end, message);
                    // Reported once, here.
                    Ty::ERROR
                }
                None => Ty::UNIT,
            },
        };
        cx.diverged = outer;
        cx.scopes.close();
        cx.withs.truncate(withs);
        hir::Block { stmts, tail, ty }
    }

    pub(super) fn statements(&mut self, cx: &mut FnCx, stmts: &[ast::Stmt]) -> Vec<hir::Stmt> {
        let mut out = Vec::new();
        let mut warned = false;
        for stmt in stmts {
            let reachable = !cx.diverged;
            if !reachable && !warned {
                self.diags.warning("W1006", stmt.span, "unreachable statement");
                warned = true;
            }
            if let Some(stmt) = self.statement(cx, stmt)
                && reachable
            {
                out.push(stmt);
            }
        }
        out
    }

    pub(super) fn statement(&mut self, cx: &mut FnCx, stmt: &ast::Stmt) -> Option<hir::Stmt> {
        match &stmt.kind {
            ast::StmtKind::With { path, fields } => {
                let name = Self::path_text(path);
                let Some((ty, context)) = self.context_named(path) else {
                    let message = format!(
                        "`with {name} {{ ... }}` changes a context, but `{name}` is not one: a context is declared `context {name} = {{ field: Type = value }}`, and a record is updated with `value with {{ field: value }}`"
                    );
                    self.diags.error("E1617", stmt.span, message);
                    return None;
                };
                let reference = self.types.handle(Handle::Ref, ty);
                let base = Self::mk(hir::ExprKind::ContextRead(context, reference), ty, stmt.span);
                let value = self.update(cx, base, fields, true, stmt.span);
                let value = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![value]), reference, stmt.span);
                let names = self.record_fields(ty, stmt.span).unwrap_or_default();
                let changed = fields.iter().filter_map(|f| names.iter().position(|(name, _)| *name == f.name.name));
                let fields = changed.fold(0u64, |set, index| set | 1 << index.min(63));
                cx.withs.push((context, fields, stmt.span));
                Some(hir::Stmt::ContextSet { context, value, fields, span: stmt.span })
            }
            // `with captured` gives back the contexts of a `Contexts`.
            ast::StmtKind::Activate(captured) => {
                let contexts = match self.type_names.get("Contexts") {
                    Some(&TypeName::Nominal(ty)) => ty,
                    _ => return None,
                };
                let value = self.expr(cx, captured, Some(contexts));
                if value.ty != contexts {
                    if !self.types.has_error(value.ty) {
                        let message = format!(
                            "`with` takes a context's fields, as `with Trace {{ span }}`, or contexts that `Contexts.current()` captured, as `with captured`; this is {}",
                            self.display(value.ty)
                        );
                        self.diags.error("E1617", captured.span, message);
                    }
                    return None;
                }
                let Some(&ModuleName::Fn(activate)) = self.names.get("prelude.activateContexts") else { return None };
                let activate =
                    Self::mk(hir::ExprKind::Call(Callee::Fn(activate), Vec::new(), vec![value]), Ty::INT, stmt.span);
                Some(hir::Stmt::ContextsActivate { activate, span: stmt.span })
            }
            ast::StmtKind::Let { mutable, pattern, ty, value } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let Some(value) = value else {
                    self.diags.error("E1109", stmt.span, "a binding must be initialized where it is declared");
                    return None;
                };
                let value = self.expr(cx, value, declared);
                let value = match declared {
                    Some(declared) => self.coerce(value, declared),
                    None => value,
                };
                cx.diverged |= value.ty == Ty::NEVER;
                let ty = declared.unwrap_or(value.ty);
                match &pattern.kind {
                    ast::PatternKind::Wildcard => Some(hir::Stmt::Expr(value)),
                    ast::PatternKind::Binding(name) => {
                        if let Some(read) =
                            self.cell_uses.last_mut().filter(|used| used.read && used.span == value.span)
                        {
                            read.within = name.span;
                            read.statement = stmt.span;
                        }
                        let local = self.declare(cx, name, ty, *mutable);
                        Some(hir::Stmt::Let(local, value))
                    }
                    _ => {
                        let pattern = self.pattern(
                            cx,
                            pattern,
                            ty,
                            PatternContext { in_match: false, mutable: *mutable, loans: Loans::None },
                        );
                        if let Some(missing) = crate::exhaust::refutation(&self.types, ty, &pattern) {
                            let message = format!("this pattern can fail, for example on `{missing}`; use `match`");
                            self.diags.error("E1201", pattern.span, message);
                        }
                        Some(hir::Stmt::Destructure(pattern, value))
                    }
                }
            }
            ast::StmtKind::Assign { target, op, value } => self.assign(cx, target, *op, value, stmt.span),
            ast::StmtKind::Discard(value) => {
                let value = self.expr(cx, value, None);
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Expr(call @ ast::Expr { kind: ast::ExprKind::Spawn(_, false), .. }) => {
                Some(self.spawn_statement(cx, call))
            }
            ast::StmtKind::Expr(value) => {
                // A line that starts with `-` is a value of its own (5.2):
                // a subtraction split before its `-` becomes one.
                if let ast::ExprKind::Unary(ast::UnaryOp::Neg, _) = value.kind {
                    let message = "this line starts with `-`, so it is a negative value of its own that nothing uses, not a subtraction from the line before: to subtract, end that line with `-`";
                    self.diags.warning("W1014", value.span, message);
                }
                // The value of a block, `if` or `match` statement is dropped.
                let expected = matches!(
                    value.kind,
                    ast::ExprKind::Block(_) | ast::ExprKind::If { .. } | ast::ExprKind::Match { .. }
                )
                .then_some(Ty::UNIT);
                let written = value;
                let value = self.expr(cx, written, expected);
                self.dropped_value(written, &value);
                // Only a result must be used: it may be a failure the
                // program has to see (11.9).
                if self.must_use(value.ty) {
                    let message = format!(
                        "a value of type {} is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`",
                        self.display(value.ty)
                    );
                    self.diags.error("E1303", value.span, message);
                    let start = Span::new(value.span.start, value.span.start);
                    let end = Span::new(value.span.end, value.span.end);
                    if matches!(self.types.kind(value.ty), TyKind::Result(..)) && cx.error.is_some() {
                        self.diags.fix("pass the failure on with `?`", false, vec![(end, "?".into())]);
                    }
                    self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
                }
                cx.diverged |= value.ty == Ty::NEVER;
                Some(hir::Stmt::Expr(value))
            }
            ast::StmtKind::Return(_) | ast::StmtKind::Fail(_) if cx.deferred => {
                let message = "a defer block cannot leave its scope; handle the case inside the block";
                self.diags.error("E1410", stmt.span, message);
                None
            }
            ast::StmtKind::Defer(_) if cx.deferred => {
                self.diags.error("E1410", stmt.span, "a defer block cannot register another `defer`");
                None
            }
            ast::StmtKind::Defer(deferred) => self.defer(cx, deferred, stmt.span),
            ast::StmtKind::Return(_) if cx.setup => {
                let message = "the setup of a suite cannot `return`, or its tests would never run; use `fail` or `testing.skip(reason)`";
                self.diags.error("E1205", stmt.span, message);
                None
            }
            ast::StmtKind::Return(value) => {
                self.note_exit(cx, Span::new(stmt.span.start, stmt.span.start + 6));
                let result = cx.result;
                let value = match value {
                    // A closure without a declared result takes it from its first `return`.
                    Some(value) if cx.infer_result && result == Ty::ERROR => {
                        let value = self.expr(cx, value, None);
                        cx.result = value.ty;
                        Some(value)
                    }
                    // `return err(e)` in a function that fails: its error
                    // leaves by `fail`, as `return` gives its success.
                    Some(ast::Expr { kind: ast::ExprKind::Construct(constructor, error), .. })
                        if *constructor == "err"
                            && cx.error.is_some()
                            && !matches!(self.types.kind(result), TyKind::Result(..)) =>
                    {
                        let written = format!("fail {}", self.sources.slice(error.span));
                        let message = format!(
                            "`return` gives a function's success, also when it can fail; its error leaves with `fail`: \
                             write `{written}`"
                        );
                        self.diags.error("E1304", stmt.span, message);
                        self.diags.fix(format!("write `{written}`"), true, vec![(stmt.span, written)]);
                        self.expr(cx, error, None);
                        None
                    }
                    Some(value) => {
                        let value = self.expr(cx, value, Some(result));
                        if result == Ty::UNIT && ![Ty::UNIT, Ty::NEVER, Ty::ERROR].contains(&value.ty) {
                            let message = format!(
                                "this function has no result, so its `return` takes no value: declare one, as `: {}` after the parameters, or write `return` alone",
                                self.display(value.ty).trim_matches('`')
                            );
                            self.diags.error("E1102", value.span, message);
                        }
                        Some(self.returned(cx, value, result))
                    }
                    None => {
                        if result != Ty::UNIT && result != Ty::ERROR {
                            let message =
                                format!("`return` without a value in a function returning {}", self.display(result));
                            self.diags.error("E1102", stmt.span, message);
                        }
                        None
                    }
                };
                cx.diverged = true;
                Some(hir::Stmt::Return(value))
            }
            ast::StmtKind::Fail(value) => {
                let declared = cx.error;
                let value = self.expr(cx, value, declared);
                // It ends the function, even where it cannot.
                cx.diverged = true;
                let error = self.error_channel(cx, value.ty, stmt.span, "E1302")?;
                self.note_propagation(cx, value.ty, Span::new(stmt.span.start, stmt.span.start + 4), "fail");
                self.note_exit(cx, Span::new(stmt.span.start, stmt.span.start + 4));
                let value = self.coerce(value, error);
                Some(hir::Stmt::Fail(value))
            }
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                let is_break = matches!(stmt.kind, ast::StmtKind::Break);
                let Some(saw_break) = cx.loops.last_mut() else {
                    let keyword = if is_break { "break" } else { "continue" };
                    let message = format!("`{keyword}` outside of a loop");
                    self.diags.error("E1205", stmt.span, message);
                    return None;
                };
                *saw_break |= is_break;
                cx.diverged = true;
                Some(if is_break { hir::Stmt::Break } else { hir::Stmt::Continue })
            }
            ast::StmtKind::For { header, body } => self.for_loop(cx, header, body),
        }
    }

    pub(super) fn assign(
        &mut self,
        cx: &mut FnCx,
        target: &ast::Expr,
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        if let Some(at) = self.ref_value_target(cx, target) {
            self.diags.error("E1411", at, REF_UNCHANGED);
            return None;
        }
        if let ast::ExprKind::Tuple(targets) = &target.kind {
            return self.multiple_assign(cx, targets, op, value, span);
        }
        if let ast::ExprKind::Slice { base, start, end, step } = &target.kind {
            return self.assign_slice(cx, base, [start, end, step], op, value, span);
        }
        if let ast::ExprKind::Index { base, index } = &target.kind {
            let (local, path, ty) = self.place(cx, base)?;
            if let TyKind::Map(key, entry) = *self.types.kind(ty) {
                if path.is_empty() && cx.params[local.0 as usize] == Some(ast::Mode::Mut) {
                    let name = &cx.locals[local.0 as usize].name;
                    let message = format!(
                        "`{name}[key] = value` can add a key to `{name}`, a `mut` loan that keeps its keys; take it as `var`"
                    );
                    self.diags.error("E1411", span, message);
                }
                // `map[key] = value` inserts or replaces (10.11); there is no
                // compound form because the key may be absent.
                if op.is_some() {
                    self.diags.error("E1411", span, "a map entry has no compound assignment; read it with `get` first");
                    return None;
                }
                let key_value = self.expr(cx, index, Some(key));
                let key_value = self.coerce(key_value, key);
                let new = self.expr(cx, value, Some(entry));
                let new = self.coerce(new, entry);
                let result = self.types.intern(TyKind::Option(entry));
                self.check_lent_write(cx, local, &path, span);
                let place = hir::Place { local, path };
                let insert = hir::ExprKind::Mutate(hir::Mutation::Insert, place, vec![key_value, new]);
                return Some(hir::Stmt::Expr(Self::mk(insert, result, span)));
            }
            let (local, path, ty) = self.index_place(cx, (local, path, ty), index, base.span)?;
            return self.assign_to(cx, (local, path, ty), op, value, span);
        }
        let place = self.place(cx, target)?;
        self.assign_to(cx, place, op, value, span)
    }

    /// `(a, b) = value` (13.8): the places are resolved left to right, with
    /// their indices evaluated then; the value is evaluated whole into
    /// temporaries; and each place is written, left to right. `_` drops its
    /// component. Swapping values that are not `Copy` works, as writing a
    /// moved binding fills it again.
    pub(super) fn multiple_assign(
        &mut self,
        cx: &mut FnCx,
        targets: &[ast::Expr],
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        if let Some(op) = op {
            let message = format!("a tuple of places has no `{}=`; assign each place on its own", op.as_str());
            self.diags.error("E1411", span, message);
            return None;
        }
        let mut stmts = Vec::new();
        // Each place, and how it is reached, to find places assigned twice.
        let mut places: Vec<Option<(Target, Vec<PlaceKey>, Span)>> = Vec::new();
        for target in targets {
            if matches!(&target.kind, ast::ExprKind::Path(segments) if segments.len() == 1 && segments[0].name == "_") {
                places.push(None);
                continue;
            }
            if let Some(at) = self.ref_value_target(cx, target) {
                self.diags.error("E1411", at, REF_UNCHANGED);
                return None;
            }
            let (local, path, ty) = match &target.kind {
                ast::ExprKind::Index { base, index } => {
                    let base_place = self.place(cx, base)?;
                    if let TyKind::Map(..) = self.types.kind(base_place.2) {
                        let message =
                            "a map entry cannot be one of several places assigned at once; assign it on its own";
                        self.diags.error("E1411", target.span, message);
                        return None;
                    }
                    self.index_place(cx, base_place, index, base.span)?
                }
                ast::ExprKind::Slice { .. } => {
                    let message = "a slice cannot be one of several places assigned at once; assign it on its own";
                    self.diags.error("E1411", target.span, message);
                    return None;
                }
                _ => self.place(cx, target)?,
            };
            let keys = path
                .iter()
                .map(|step| match step {
                    hir::PlaceStep::Field(index) => PlaceKey::Field(*index),
                    hir::PlaceStep::Index(index) => match &index.kind {
                        hir::ExprKind::Int(value) => PlaceKey::Constant(*value),
                        hir::ExprKind::Local(local) => PlaceKey::Binding(*local),
                        _ => PlaceKey::Unknown,
                    },
                })
                .collect();
            // Indices are evaluated now, before the value.
            let path = path
                .into_iter()
                .map(|step| match step {
                    hir::PlaceStep::Index(index) => {
                        let held = self.hidden_local(cx, Ty::INT);
                        let read = Self::mk(hir::ExprKind::Local(held), Ty::INT, index.span);
                        stmts.push(hir::Stmt::Let(held, index));
                        hir::PlaceStep::Index(read)
                    }
                    step => step,
                })
                .collect();
            places.push(Some(((local, path, ty), keys, target.span)));
        }
        for (index, first) in places.iter().enumerate() {
            let Some(((local, _, _), keys, _)) = first else { continue };
            for second in places[index + 1..].iter().flatten() {
                let ((other, _, _), other_keys, at) = second;
                let same = keys.iter().zip(other_keys).all(|(a, b)| a == b && *a != PlaceKey::Unknown);
                if local == other && same {
                    let name = &cx.locals[local.0 as usize].name;
                    let message = format!(
                        "this place overlaps another one assigned in the same statement, inside `{name}`; give each component its own place"
                    );
                    self.diags.error("E1405", *at, message);
                    return None;
                }
            }
        }
        let wanted: Option<Vec<Ty>> = places.iter().map(|p| p.as_ref().map(|((_, _, ty), _, _)| *ty)).collect();
        let expected = wanted.map(|items| self.types.intern(TyKind::Tuple(items)));
        let value = self.expr(cx, value, expected);
        let value = match expected {
            Some(expected) if matches!(self.types.kind(value.ty), TyKind::Tuple(items) if items.len() == targets.len()) => {
                self.coerce(value, expected)
            }
            _ => value,
        };
        let items = match self.types.kind(value.ty).clone() {
            TyKind::Tuple(items) if items.len() == targets.len() => items,
            TyKind::Error => return None,
            _ => {
                let message = format!(
                    "{} places are assigned a value of type {}; the value must be a tuple of {} components",
                    targets.len(),
                    self.display(value.ty),
                    targets.len()
                );
                self.diags.error("E1102", value.span, message);
                return None;
            }
        };
        // `let (t0, t1, ...) = value`, then each place from its temporary.
        let mut parts = Vec::new();
        let mut writes = Vec::new();
        for (place, &ty) in places.into_iter().zip(&items) {
            let pattern_span = place.as_ref().map_or(value.span, |(_, _, at)| *at);
            let kind = match place {
                Some(((local, path, place_ty), _, at)) => {
                    let held = self.hidden_local(cx, ty);
                    let read = Self::mk(hir::ExprKind::Local(held), ty, at);
                    let read = self.coerce(read, place_ty);
                    self.check_lent_write(cx, local, &path, at);
                    writes.push(hir::Stmt::Assign { local, path, op: None, value: read, span: at });
                    hir::PatKind::Binding(held)
                }
                None => hir::PatKind::Wildcard,
            };
            parts.push(hir::Pattern { kind, ty, span: pattern_span });
        }
        let pattern = hir::Pattern { kind: hir::PatKind::Fields(parts), ty: value.ty, span: value.span };
        stmts.push(hir::Stmt::Destructure(pattern, value));
        stmts.extend(writes);
        let block = hir::Block { stmts, tail: None, ty: Ty::UNIT };
        Some(hir::Stmt::Expr(Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span)))
    }

    /// `place op= value` once the target is resolved.
    pub(super) fn assign_to(
        &mut self,
        cx: &mut FnCx,
        place: (LocalId, Vec<hir::PlaceStep>, Ty),
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        let (local, path, ty) = place;
        self.check_lent_write(cx, local, &path, span);
        // Replacing the root of a `mut` loan must keep its extent (16.3).
        if op.is_none()
            && path.is_empty()
            && cx.params[local.0 as usize] == Some(ast::Mode::Mut)
            && !self.fixed_extent(ty)
        {
            let name = &cx.locals[local.0 as usize].name;
            let message = format!(
                "assigning `{name}` may change its extent, but it is a `mut` loan; take it as `var`, or assign elements or a slice such as `{name}[:] = replacement`"
            );
            self.diags.error("E1411", span, message);
        }
        let value = match op {
            Some(BinOp::Shl | BinOp::Shr) => {
                if self.types.as_int(ty).is_none() && ty != Ty::ERROR && !self.bound_by(ty, self.integer) {
                    let message =
                        format!("`{}=` requires an integer target, found {}", op.unwrap().as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, None);
                self.require_int(&value);
                value
            }
            // `place = Trait.method(place, value)` on a type that takes the
            // operator by a trait (18.10).
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div))
                if let Some((trait_id, index)) = self.operator_trait(op, ty) =>
            {
                let value = self.expr(cx, value, Some(ty));
                let value = self.coerce(value, ty);
                let read = self.place_read(cx, &hir::Place { local, path: path.clone() }, span);
                let call = hir::ExprKind::Call(Callee::Trait(trait_id, index), vec![ty], vec![read, value]);
                let value = Self::mk(call, ty, span);
                cx.diverged |= value.ty == Ty::NEVER;
                return Some(hir::Stmt::Assign { local, path, op: None, value, span });
            }
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem)) => {
                self.compound_value(cx, op, ty, value, span)
            }
            Some(op) => {
                let bitwise = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte | TyKind::Error)
                    || self.bound_by(ty, self.integer);
                if !bitwise {
                    let message = format!("`{}=` cannot be applied to {}", op.as_str(), self.display(ty));
                    self.diags.error("E1102", span, message);
                }
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
            None => {
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
        };
        cx.diverged |= value.ty == Ty::NEVER;
        Some(hir::Stmt::Assign { local, path, op, value, span })
    }

    /// Where an assignment target reaches into the value of a `Ref`, which
    /// never changes (16.10).
    pub(super) fn ref_value_target(&self, cx: &FnCx, target: &ast::Expr) -> Option<Span> {
        match &target.kind {
            ast::ExprKind::Path(segments) => {
                let local = self.lookup(cx, &segments[0].name)?;
                let mut ty = cx.locals[local.0 as usize].ty;
                for segment in &segments[1..] {
                    if let TyKind::Handle(Handle::Ref, _) = self.types.kind(ty) {
                        return (segment.name == "value").then_some(segment.span);
                    }
                    ty = self.field_type(ty, &segment.name)?;
                }
                None
            }
            ast::ExprKind::Field(base, name) => {
                if name.name == "value"
                    && let Some(ty) = self.target_type(cx, base)
                    && let TyKind::Handle(Handle::Ref, _) = self.types.kind(ty)
                {
                    return Some(name.span);
                }
                self.ref_value_target(cx, base)
            }
            ast::ExprKind::Index { base, .. }
            | ast::ExprKind::Slice { base, .. }
            | ast::ExprKind::TupleSlot(base, _) => self.ref_value_target(cx, base),
            _ => None,
        }
    }

    /// The type of an assignment target, without reporting anything.
    pub(super) fn target_type(&self, cx: &FnCx, target: &ast::Expr) -> Option<Ty> {
        let ty = match &target.kind {
            ast::ExprKind::Path(segments) => {
                let local = self.lookup(cx, &segments[0].name)?;
                let mut ty = cx.locals[local.0 as usize].ty;
                for segment in &segments[1..] {
                    ty = self.field_type(ty, &segment.name)?;
                }
                return Some(ty);
            }
            ast::ExprKind::Field(base, name) => return self.field_type(self.target_type(cx, base)?, &name.name),
            ast::ExprKind::TupleSlot(base, slot) => match self.types.kind(self.target_type(cx, base)?) {
                TyKind::Tuple(items) => return items.get(*slot as usize).copied(),
                _ => return None,
            },
            ast::ExprKind::Index { base, .. } => self.target_type(cx, base)?,
            _ => return None,
        };
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Array(element) | TyKind::Deque(element) => Some(*element),
            TyKind::Map(_, value) => Some(*value),
            _ => None,
        }
    }

    /// The type of a field, of `value` of a newtype or a `Ref`.
    pub(super) fn field_type(&self, ty: Ty, name: &str) -> Option<Ty> {
        match self.types.kind(ty) {
            TyKind::Handle(Handle::Ref, value) if name == "value" => Some(*value),
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => fields.iter().find(|f| f.name == name).map(|f| f.ty),
                AdtKind::Newtype(inner) if name == "value" => Some(*inner),
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolves an assignment target to a `var` binding and a field path.
    pub(super) fn place(&mut self, cx: &mut FnCx, target: &ast::Expr) -> Option<(LocalId, Vec<hir::PlaceStep>, Ty)> {
        let (base, field) = match &target.kind {
            ast::ExprKind::Path(segments) => {
                let root = &segments[0];
                // For an editor, the binding written is a reference too.
                self.index_local(cx, root);
                if self.lookup(cx, &root.name).is_none()
                    && let Some(captured) = self.capture(cx, &root.name, root.span)
                {
                    // Assigning a capture changes the closure's own copy (11.8).
                    let hir::ExprKind::Field(_, index) = captured.kind else { unreachable!() };
                    if !cx.captures[index as usize].mutable {
                        let message = format!(
                            "cannot assign to `{0}`: it is not declared with `var`; declare it as `var {0}`",
                            root.name
                        );
                        self.diags.error("E1411", root.span, message);
                    }
                    cx.mutates = true;
                    let mut ty = captured.ty;
                    let mut path = vec![hir::PlaceStep::Field(index)];
                    for segment in &segments[1..] {
                        let (index, field_ty) = self.field_of(ty, FieldRef::Name(&segment.name), segment.span)?;
                        path.extend(index.map(hir::PlaceStep::Field));
                        ty = field_ty;
                    }
                    return Some((cx.env.unwrap(), path, ty));
                }
                let Some(local) = self.lookup(cx, &root.name) else {
                    let known = self.names.contains_key(&root.name);
                    let (code, message) = if known {
                        ("E1411", format!("`{}` is not a mutable binding", root.name))
                    } else {
                        ("E1001", format!("unknown name `{}`", root.name))
                    };
                    self.diags.error(code, root.span, message);
                    return None;
                };
                // Writing a loan is what it is for.
                if matches!(cx.params[local.0 as usize], Some(ast::Mode::Mut | ast::Mode::Var)) {
                    cx.reads.insert(local);
                }
                if !cx.mutable[local.0 as usize] {
                    let ty = self.types.display(cx.locals[local.0 as usize].ty);
                    let message = match cx.params[local.0 as usize] {
                        Some(ast::Mode::Ref) if cx.loan_bindings.contains(&local) => format!(
                            "cannot assign to `{0}`: it is a `ref` binding that only observes its part; borrow it with `mut {0}` in a `for` header",
                            root.name
                        ),
                        Some(ast::Mode::Ref) if root.name == "self" => {
                            "cannot assign to `self`: it is a shared loan; take it as `mut self` to change the caller's value".to_string()
                        }
                        Some(ast::Mode::Ref) => format!(
                            "cannot assign to `{0}`: it is a `ref` loan; take it as `{0}: mut {ty}` to change the caller's value",
                            root.name
                        ),
                        Some(_) => format!(
                            "cannot assign to `{0}`: parameters are immutable; take it as `{0}: mut {ty}` to change the caller's value, or copy it into a `var`",
                            root.name
                        ),
                        None => format!(
                            "cannot assign to `{0}`: it is not declared with `var`; declare it as `var {0}`",
                            root.name
                        ),
                    };
                    self.diags.error("E1411", root.span, message);
                }
                let mut ty = cx.locals[local.0 as usize].ty;
                let mut path = Vec::new();
                for segment in &segments[1..] {
                    let (index, field_ty) = self.field_of(ty, FieldRef::Name(&segment.name), segment.span)?;
                    path.extend(index.map(hir::PlaceStep::Field));
                    ty = field_ty;
                }
                return Some((local, path, ty));
            }
            ast::ExprKind::Index { base, index } => {
                let (local, path, ty) = self.place(cx, base)?;
                return self.index_place(cx, (local, path, ty), index, base.span);
            }
            ast::ExprKind::Field(base, name) => (base, (FieldRef::Name(&name.name), name.span)),
            ast::ExprKind::TupleSlot(base, slot) => (base, (FieldRef::Slot(*slot), target.span)),
            _ => {
                self.diags.error("E1411", target.span, "this expression cannot be assigned to");
                return None;
            }
        };
        let (local, mut path, ty) = self.place(cx, base)?;
        let (index, field_ty) = self.field_of(ty, field.0, field.1)?;
        path.extend(index.map(hir::PlaceStep::Field));
        Some((local, path, field_ty))
    }

    /// An exclusive loan of a call must not overlap its other loans (16.3,
    /// 16.4): a provable overlap is `E1403`; indices and regions known only
    /// at run time are checked then (`P0004`) between exclusive loans.
    /// `args` tells which arguments are shared `ref` loans.
    pub(super) fn check_loan_overlap<'e>(&mut self, args: impl Iterator<Item = (&'e hir::Expr, bool)>, span: Span) {
        // Each loan, whether it lends a region of its place, and whether it
        // is exclusive.
        type Bounds = [Option<Box<hir::Expr>>; 3];
        let loans: Vec<(hir::Place, bool, bool, Option<&Bounds>)> = args
            .filter_map(|(a, shared)| match &a.kind {
                hir::ExprKind::Loan(place) => Some((place.clone(), false, true, None)),
                hir::ExprKind::Region(place, bounds) => Some((place.clone(), true, true, Some(bounds))),
                hir::ExprKind::Slice(base, bounds) if shared => {
                    Some((hir::Place::of((**base).clone())?, true, false, Some(bounds)))
                }
                _ if shared => Some((hir::Place::of(a.clone())?, false, false, None)),
                _ => None,
            })
            .collect();
        for (index, (first, first_region, first_exclusive, first_bounds)) in loans.iter().enumerate() {
            for (second, second_region, second_exclusive, second_bounds) in &loans[index + 1..] {
                let (first_region, second_region) = (*first_region, *second_region);
                if first.local != second.local || !first_exclusive && !second_exclusive {
                    continue;
                }
                // The first step where the places part, if any, tells them
                // apart: different fields, or indices that are checked when
                // the call runs unless they are the same constant.
                let parted = first.path.iter().zip(&second.path).find_map(|steps| match steps {
                    (hir::PlaceStep::Field(x), hir::PlaceStep::Field(y)) => (x != y).then_some(false),
                    (hir::PlaceStep::Index(x), hir::PlaceStep::Index(y)) => match (&x.kind, &y.kind) {
                        (hir::ExprKind::Int(i), hir::ExprKind::Int(j)) if i == j => None,
                        _ => Some(false),
                    },
                    _ => None,
                });
                // Otherwise one place holds the other, which only a region
                // may leave disjoint, decided when the call runs.
                let overlap = parted.unwrap_or(match first.path.len().cmp(&second.path.len()) {
                    std::cmp::Ordering::Equal if first_region && second_region => {
                        first_bounds.zip(*second_bounds).is_some_and(|(a, b)| regions_overlap(a, b))
                    }
                    std::cmp::Ordering::Equal => true,
                    std::cmp::Ordering::Less => !first_region,
                    std::cmp::Ordering::Greater => !second_region,
                });
                if overlap {
                    let message = if *first_exclusive && *second_exclusive {
                        "two exclusive loans of one call overlap; lend disjoint parts, or one at a time"
                    } else {
                        "a `ref` loan overlaps an exclusive loan of the same call; copy the value it observes into a binding first"
                    };
                    self.diags.error("E1403", span, message);
                    return;
                }
            }
        }
    }

    /// The element `base[index]` of the array or deque place `base`.
    pub(super) fn index_place(
        &mut self,
        cx: &mut FnCx,
        base: (LocalId, Vec<hir::PlaceStep>, Ty),
        index: &ast::Expr,
        base_span: Span,
    ) -> Option<(LocalId, Vec<hir::PlaceStep>, Ty)> {
        let (local, mut path, ty) = base;
        let (TyKind::Array(element) | TyKind::Deque(element) | TyKind::CArray(element, _)) = *self.types.kind(ty)
        else {
            let message = if ty == Ty::STRING {
                "a `String` is immutable: build a new one, for example with interpolation".to_string()
            } else {
                format!("{} cannot be indexed", self.display(ty))
            };
            self.diags.error("E1102", base_span, message);
            return None;
        };
        let index = self.expr(cx, index, Some(Ty::INT));
        path.push(hir::PlaceStep::Index(self.coerce(index, Ty::INT)));
        Some((local, path, element))
    }

    /// Whether every value of `ty` has the same structural extent, so a
    /// `mut` loan may replace it whole (16.3).
    pub(super) fn fixed_extent(&self, ty: Ty) -> bool {
        !matches!(
            self.types.kind(self.types.repr(ty)),
            TyKind::Array(_)
                | TyKind::Deque(_)
                | TyKind::Map(..)
                | TyKind::Set(_)
                | TyKind::Param(_)
                | TyKind::Opaque(..)
        )
    }

    /// The value of `target op= value` for an arithmetic operator; the result
    /// of `target op value` must keep the target's type.
    pub(super) fn compound_value(
        &mut self,
        cx: &mut FnCx,
        op: BinOp,
        target: Ty,
        value: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let expected = self.arithmetic_expected(value, target).or(Some(target));
        let value = self.expr(cx, value, expected);
        let value = if self.number_leaf(target).1 == 0 && self.number_leaf(value.ty).1 == 0 {
            self.coerce(value, target)
        } else {
            value
        };
        let result = self.arithmetic(op, target, value.ty, span);
        if result != target && result != Ty::ERROR && target != Ty::ERROR {
            let message = format!(
                "`{0}=` would change the target from {1} to {2}; bind the result instead: `let result = target {0} value`",
                op.as_str(),
                self.display(target),
                self.display(result),
            );
            self.diags.error("E1102", span, message);
        }
        value
    }

    /// `values[start:end:step] = value` and its compound forms (10.5).
    pub(super) fn assign_slice(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        bounds: [&Option<Box<ast::Expr>>; 3],
        op: Option<BinOp>,
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        let (local, path, ty) = self.place(cx, base)?;
        self.check_lent_write(cx, local, &path, span);
        if !matches!(self.types.kind(ty), TyKind::Array(_)) {
            let message = if ty == Ty::STRING {
                "a `String` is immutable: build a new one, for example with interpolation".to_string()
            } else {
                format!("{} cannot be sliced", self.display(ty))
            };
            self.diags.error("E1102", base.span, message);
            return None;
        }
        let bounds = bounds.map(|bound| {
            bound.as_ref().map(|b| {
                let value = self.expr(cx, b, Some(Ty::INT));
                self.coerce(value, Ty::INT)
            })
        });
        let value = match op {
            None => {
                let value = self.expr(cx, value, Some(ty));
                self.coerce(value, ty)
            }
            Some(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem)) => {
                self.compound_value(cx, op, ty, value, span)
            }
            Some(op) => {
                let message = format!("`{}=` cannot be applied to a slice", op.as_str());
                self.diags.error("E1102", span, message);
                return None;
            }
        };
        Some(hir::Stmt::AssignSlice { local, path, bounds, op, value, span })
    }

    /// The position and type of a field. The position is `None` for a
    /// newtype's `.value`, which shares the newtype's representation.
    pub(super) fn field_of(&mut self, ty: Ty, field: FieldRef, span: Span) -> Option<(Option<u32>, Ty)> {
        if let (FieldRef::Name(name), Some(hidden)) = (field, self.hidden_fields(ty))
            && let Some(index) = self.types.components(ty).and(self.field_index(ty, name))
            && hidden[index]
        {
            let message = format!("field `{name}` of {} is private to its module", self.display(ty));
            self.diags.error("E1501", span, message);
        }
        if let FieldRef::Name(name) = field {
            self.index_field(ty, name, span);
        }
        let found = match (self.types.kind(ty), field) {
            (TyKind::Error, _) => return None,
            (TyKind::Tuple(items), FieldRef::Slot(slot)) => items.get(slot as usize).map(|&t| (Some(slot), t)),
            (TyKind::Adt(id), FieldRef::Name(name)) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => {
                    fields.iter().position(|f| f.name == name).map(|i| (Some(i as u32), fields[i].ty))
                }
                AdtKind::Newtype(inner) => (name == "value").then_some((None, *inner)),
                AdtKind::Enum(_) => None,
            },
            _ => None,
        };
        if found.is_none() {
            let what = match field {
                FieldRef::Name(name) => format!("field `{name}`"),
                FieldRef::Slot(slot) => format!("element {slot}"),
            };
            let mut message = format!("{} has no {what}", self.display(ty));
            if let FieldRef::Name(name) = field {
                let fields: Vec<String> = match self.types.kind(ty) {
                    TyKind::Adt(id) => match &self.types.adt(*id).kind {
                        AdtKind::Record(fields) => fields.iter().map(|f| f.name.clone()).collect(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                // `values.length`: a method read as a field.
                let intrinsic = ["length", "isEmpty", "byteLength", "keys", "values", "entries", "chars", "bytes"];
                let key = (self.owner_key(ty), name.to_string());
                if intrinsic.contains(&name) || self.methods.contains_key(&key) {
                    message.push_str(&format!("; `{name}` is a method: call it, `value.{name}()`"));
                } else if let TyKind::Option(inner) = *self.types.kind(ty) {
                    let inner = self.display(inner);
                    message.push_str(&format!(
                        "; it may be `none`: reach the {inner} inside with `match`, `?`, or `.map((value) {{ value.{name} }})`"
                    ));
                } else if let Some(near) = closest(name, fields.iter().map(String::as_str)) {
                    message.push_str(&format!("; did you mean `{near}`?"));
                } else if !fields.is_empty() {
                    message.push_str(&format!("; its fields are {}", listed(&fields)));
                }
            }
            self.diags.error("E1102", span, message);
            self.offer_meant(span);
        }
        found
    }

    /// Who declares a nominal type, if the program does.
    pub(super) fn owner(&self, ty: Ty) -> Option<&Owner> {
        let name = match self.types.instance_of(ty) {
            Some((template, _)) => &self.types.template(template).name,
            None => match self.types.kind(ty) {
                TyKind::Adt(id) => &self.types.adt(*id).name,
                _ => return None,
            },
        };
        self.owners.get(name)
    }

    /// Which fields of a record are private, when it is declared in another
    /// module and has some (7.6).
    pub(super) fn hidden_fields(&self, ty: Ty) -> Option<&[bool]> {
        let owner = self.owner(ty)?;
        (owner.module != self.module && owner.private_fields.contains(&true)).then_some(&owner.private_fields)
    }

    pub(super) fn field_index(&self, ty: Ty, name: &str) -> Option<usize> {
        let TyKind::Adt(id) = self.types.kind(ty) else { return None };
        match &self.types.adt(*id).kind {
            AdtKind::Record(fields) => fields.iter().position(|f| f.name == name),
            _ => None,
        }
    }

    /// A type that `ty` mentions and that is private to its module.
    pub(super) fn private_in(&self, ty: Ty) -> Option<Ty> {
        if self.owner(ty).is_some_and(|o| !o.public) {
            return Some(ty);
        }
        let parts: Vec<Ty> = match self.types.kind(ty) {
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => vec![*t],
            TyKind::Map(a, b) | TyKind::Result(a, b) => vec![*a, *b],
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            TyKind::Fn(params, result, ..) => params.iter().copied().chain([*result]).collect(),
            _ => self.types.instance_of(ty).map_or(Vec::new(), |(_, args)| args.to_vec()),
        };
        parts.into_iter().find_map(|t| self.private_in(t))
    }

    /// A `pub` function or operation exposes only `pub` types (7.9).
    pub(super) fn check_public_api(&mut self, params: &[Ty], result: Ty, f: &ast::FnDecl) {
        let exposed = params.iter().chain([&result]).find_map(|&t| self.private_in(t));
        if let Some(private) = exposed {
            let (shown, name) = (self.display(private), &f.name.name);
            let message = if f.owner.is_some() && params.first() == Some(&private) {
                format!(
                    "`pub` operation `{name}` belongs to {shown}, which is private; declare the type `pub`, or the operation private"
                )
            } else {
                format!("`pub fn {name}` exposes {shown}, which is private; declare it `pub`, or the function private")
            };
            self.diags.error("E1503", f.name.span, message);
        }
    }

    /// Reports a call of an inherent operation that is private to another
    /// module (7.8).
    pub(super) fn check_method_visible(&mut self, id: FnId, span: Span) {
        if let Some(&(module, public)) = self.method_visibility.get(&id)
            && module != self.module
            && !public
        {
            let name = self.signatures[id.0 as usize].name.clone();
            let message = format!("`{name}` is private to its module; declare it `pub` to use it here");
            self.diags.error("E1501", span, message);
        }
    }

    pub(super) fn for_loop(&mut self, cx: &mut FnCx, header: &ast::ForHeader, body: &ast::Block) -> Option<hir::Stmt> {
        match header {
            ast::ForHeader::Infinite => {
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                let saw_break = cx.loops.pop().unwrap();
                cx.diverged |= !saw_break;
                Some(hir::Stmt::Loop { condition: None, body })
            }
            ast::ForHeader::Condition(condition) => {
                let condition = self.expr(cx, condition, Some(Ty::BOOL));
                let condition = self.coerce(condition, Ty::BOOL);
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                Some(hir::Stmt::Loop { condition: Some(condition), body })
            }
            ast::ForHeader::In { pattern, source } => {
                let ast::ExprKind::Binary(op @ (BinOp::Range | BinOp::RangeInclusive), start, end) = &source.kind
                else {
                    let source = self.expr(cx, source, None);
                    // A shared collection is iterated through a snapshot.
                    let source = match *self.types.kind(source.ty) {
                        TyKind::Handle(handle, _) if handle.collection() => {
                            if loan_kind(pattern).is_some() {
                                let message = "another task may change a shared collection, so a `for` over it binds values: remove `ref`, `mut` or `var`";
                                self.diags.error("E1206", pattern.span, message);
                                return None;
                            }
                            let (span, state) = (source.span, self.types.handle_result(source.ty));
                            let snapshot = hir::TaskOp::Shared(hir::SharedOp::Snapshot);
                            Self::mk(hir::ExprKind::TaskOp(snapshot, vec![source]), state, span)
                        }
                        _ => source,
                    };
                    let element = match *self.types.kind(source.ty) {
                        TyKind::Handle(Handle::Receiver, element) => {
                            return Some(self.for_receiver(cx, pattern, source, element, body));
                        }
                        TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => element,
                        TyKind::String => Ty::CHAR,
                        // A map yields its entries as `(key, value)` (10.13).
                        TyKind::Map(key, value) => self.types.intern(TyKind::Tuple(vec![key, value])),
                        TyKind::Error => Ty::ERROR,
                        _ => match self.iterator_element(source.ty) {
                            Some(element) => return Some(self.for_cursor(cx, pattern, source, element, body)),
                            None => {
                                let message = format!(
                                    "{} cannot be iterated: implement `Iterator[T]` for it, or iterate a collection",
                                    self.display(source.ty)
                                );
                                self.diags.error("E1206", source.span, message);
                                return None;
                            }
                        },
                    };
                    if let Some(exclusive) = loan_kind(pattern) {
                        return self.borrowed_for(cx, pattern, source, element, body, exclusive);
                    }
                    cx.scopes.open();
                    let context = PatternContext { in_match: false, mutable: false, loans: Loans::None };
                    let pattern = self.pattern(cx, pattern, element, context);
                    if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
                        let message = format!("this pattern can fail, for example on `{missing}`");
                        self.diags.error("E1201", pattern.span, message);
                    }
                    cx.loops.push(false);
                    let body = self.block(cx, body, Some(Ty::UNIT));
                    cx.loops.pop();
                    cx.scopes.close();
                    return Some(hir::Stmt::ForEach { pattern, source, body });
                };
                let (start, end) = self.operands(cx, start, end, None, false);
                // Characters count through the prelude's `Range`.
                if start.ty == Ty::CHAR {
                    let range = self.range_value(start, end, *op == BinOp::RangeInclusive, source.span);
                    return Some(self.for_cursor(cx, pattern, range, Ty::CHAR, body));
                }
                self.require_int(&start);
                cx.scopes.open();
                let local = match &pattern.kind {
                    ast::PatternKind::Binding(binding) => self.declare(cx, binding, start.ty, false),
                    ast::PatternKind::Wildcard => self.hidden_local(cx, start.ty),
                    ast::PatternKind::Loan(..) => {
                        let message = "a range yields integers, which are not borrowed; bind them by value";
                        self.diags.error("E1206", pattern.span, message);
                        self.hidden_local(cx, start.ty)
                    }
                    _ => {
                        self.diags.error("E1206", pattern.span, "a range yields integers; bind them to a name");
                        self.hidden_local(cx, start.ty)
                    }
                };
                cx.loops.push(false);
                let body = self.block(cx, body, Some(Ty::UNIT));
                cx.loops.pop();
                cx.scopes.close();
                Some(hir::Stmt::ForRange { local, start, end, inclusive: *op == BinOp::RangeInclusive, body })
            }
        }
    }

    /// A `for` whose pattern borrows the elements of a collection (13.3):
    /// `ref` bindings observe them, and `mut` or `var` bindings change them
    /// in place. The collection must be a place, and stays lent for the
    /// whole loop: exclusively when anything is borrowed with `mut` or `var`.
    pub(super) fn borrowed_for(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
        exclusive: bool,
    ) -> Option<hir::Stmt> {
        let (ty, span) = (source.ty, source.span);
        let fits = match self.types.kind(ty) {
            TyKind::Array(_) | TyKind::Map(..) => true,
            TyKind::Set(_) => !exclusive,
            TyKind::Error => return None,
            _ => false,
        };
        if !fits {
            let message = match self.types.kind(ty) {
                TyKind::Set(_) => "the elements of a set are its keys and cannot change; borrow them with `ref`".into(),
                _ if exclusive => format!(
                    "`mut` and `var` bindings change the elements of an array or map in place, but {} has no such elements",
                    self.display(ty)
                ),
                _ => format!(
                    "`ref` bindings borrow the elements of an array, map or set, but {} produces values; bind them by value",
                    self.display(ty)
                ),
            };
            self.diags.error("E1206", span, message);
            return None;
        }
        // The collection is evaluated once: a place with fixed indices.
        let place = if exclusive {
            self.loan_place(cx, source, "lending its elements as `mut` or `var`", false)?
        } else {
            let Some(place) = hir::Place::of(source) else {
                let message = "a `for` with `ref` bindings borrows its collection, which must be a binding or part of one; bind it first";
                self.diags.error("E1206", span, message);
                return None;
            };
            place
        };
        let mut stmts = Vec::new();
        let mut path = Vec::new();
        for step in place.path {
            path.push(match step {
                hir::PlaceStep::Index(index)
                    if !matches!(index.kind, hir::ExprKind::Local(_) | hir::ExprKind::Int(_)) =>
                {
                    let local = self.hidden_local(cx, Ty::INT);
                    let read = Self::mk(hir::ExprKind::Local(local), Ty::INT, index.span);
                    stmts.push(hir::Stmt::Let(local, index));
                    hir::PlaceStep::Index(read)
                }
                step => step,
            });
        }
        let place = hir::Place { local: place.local, path };
        let read = self.place_read(cx, &place, span);

        cx.scopes.open();
        let context = PatternContext { in_match: false, mutable: false, loans: Loans::Any };
        let pattern = self.pattern(cx, pattern, element, context);
        if let Some(missing) = crate::exhaust::refutation(&self.types, element, &pattern) {
            let message = format!("this pattern can fail, for example on `{missing}`");
            self.diags.error("E1201", pattern.span, message);
        }
        let map = matches!(self.types.kind(ty), TyKind::Map(..));
        self.borrowed_bindings(cx, &pattern, map, false);
        cx.lent.push((place.clone(), exclusive));
        cx.loops.push(false);
        let mut body = self.block(cx, body, Some(Ty::UNIT));
        cx.loops.pop();
        cx.lent.pop();
        cx.scopes.close();
        if !exclusive {
            stmts.push(hir::Stmt::ForEach { pattern, source: read, body });
            return Some(hir::Stmt::Expr(Self::mk(
                hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }),
                Ty::UNIT,
                span,
            )));
        }
        // Each iteration binds the parts of the element at the next
        // position; the length cannot change while the loop runs.
        let (position, count) = (self.hidden_local(cx, Ty::INT), self.hidden_local(cx, Ty::INT));
        let at = Self::mk(hir::ExprKind::Local(position), Ty::INT, span);
        let mut element_place = place.clone();
        element_place.path.push(hir::PlaceStep::Index(at));
        let mut lends = Vec::new();
        self.lends(&pattern, element_place, &mut lends);
        lends.append(&mut body.stmts);
        body.stmts = lends;
        let length = Self::mk(hir::ExprKind::Length(Box::new(read)), Ty::INT, span);
        stmts.push(hir::Stmt::Let(count, length));
        stmts.push(hir::Stmt::ForRange {
            local: position,
            start: Self::mk(hir::ExprKind::Int(0), Ty::INT, span),
            end: Self::mk(hir::ExprKind::Local(count), Ty::INT, span),
            inclusive: false,
            body,
        });
        Some(hir::Stmt::Expr(Self::mk(
            hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }),
            Ty::UNIT,
            span,
        )))
    }

    /// Checks the bindings of a borrowed `for` header: one by value copies
    /// its part, which must be `Copy`, and only the value of a map entry
    /// (`entry.1`) can be borrowed with `mut` or `var`.
    pub(super) fn borrowed_bindings(&mut self, cx: &FnCx, pattern: &hir::Pattern, map: bool, in_value: bool) {
        match &pattern.kind {
            hir::PatKind::Binding(local) => {
                let name = &cx.locals[local.0 as usize].name;
                match cx.params[local.0 as usize] {
                    None if !self.types.is_copy(pattern.ty) => {
                        let message = format!(
                            "`{name}` would copy its part of type {}, which is not `Copy`; borrow it with `ref {name}`",
                            self.display(pattern.ty)
                        );
                        self.diags.error("E1406", pattern.span, message);
                    }
                    Some(ast::Mode::Mut | ast::Mode::Var) if map && !in_value => {
                        let message = "only the value of a map entry can change: write `(key, mut value)`";
                        self.diags.error("E1206", pattern.span, message);
                    }
                    _ => {}
                }
            }
            hir::PatKind::Fields(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.borrowed_bindings(cx, item, map, in_value || map && index == 1);
                }
            }
            hir::PatKind::Newtype(inner) => self.borrowed_bindings(cx, inner, map, in_value),
            hir::PatKind::Variant(_, items) | hir::PatKind::Array { items, .. } => {
                items.iter().for_each(|item| self.borrowed_bindings(cx, item, map, in_value))
            }
            hir::PatKind::Wildcard | hir::PatKind::Literal(_) => {}
        }
    }

    /// The statements that bind each binding of a borrowed `for` header to
    /// its part of the element at `place`.
    pub(super) fn lends(&mut self, pattern: &hir::Pattern, place: hir::Place, out: &mut Vec<hir::Stmt>) {
        match &pattern.kind {
            hir::PatKind::Binding(local) => out.push(hir::Stmt::Lend { local: *local, place }),
            hir::PatKind::Fields(items) => {
                for (index, item) in items.iter().enumerate() {
                    let mut part = place.clone();
                    part.path.push(hir::PlaceStep::Field(index as u32));
                    self.lends(item, part, out);
                }
            }
            hir::PatKind::Newtype(inner) => self.lends(inner, place, out),
            hir::PatKind::Wildcard | hir::PatKind::Literal(_) => {}
            hir::PatKind::Variant(..) | hir::PatKind::Array { .. } => {
                self.diags.unsupported(pattern.span, "borrowing parts of an enum or array element in a `for` header");
            }
        }
    }

    /// Reports a write to a place that overlaps a collection lent to the
    /// bindings of an enclosing `for`.
    pub(super) fn check_lent_write(&mut self, cx: &FnCx, local: LocalId, path: &[hir::PlaceStep], span: Span) {
        let place = hir::Place { local, path: path.to_vec() };
        if let Some((lent, exclusive)) = lent_conflict(cx, &place) {
            let lent = self.place_name(cx, lent);
            self.lent_error(&lent, exclusive, span);
        }
    }

    /// A place as written, such as `self.items`.
    pub(super) fn place_name(&self, cx: &FnCx, place: &hir::Place) -> String {
        let mut ty = cx.locals[place.local.0 as usize].ty;
        let mut name = cx.locals[place.local.0 as usize].name.clone();
        for step in &place.path {
            match step {
                hir::PlaceStep::Field(index) => {
                    let field = match self.types.kind(self.types.repr(ty)) {
                        TyKind::Adt(id) => match &self.types.adt(*id).kind {
                            AdtKind::Record(fields) => fields[*index as usize].name.clone(),
                            _ => index.to_string(),
                        },
                        _ => index.to_string(),
                    };
                    name = format!("{name}.{field}");
                    ty = self.types.components(ty).map_or(Ty::ERROR, |c| c[*index as usize]);
                }
                hir::PlaceStep::Index(_) => {
                    name.push_str("[..]");
                    ty = match *self.types.kind(self.types.repr(ty)) {
                        TyKind::Array(element) | TyKind::Deque(element) => element,
                        _ => Ty::ERROR,
                    };
                }
            }
        }
        name
    }

    /// Reports a use of a collection that the bindings of an enclosing
    /// `for` borrow (13.3): any use while they borrow it exclusively, and a
    /// change while they observe it.
    pub(super) fn lent_error(&mut self, name: &str, exclusive: bool, span: Span) {
        let message = if exclusive {
            format!(
                "`{name}` is lent to the `mut` or `var` bindings of the loop until it ends; reach its elements through them"
            )
        } else {
            format!("`{name}` is lent to the `ref` bindings of the loop until it ends, so it cannot change inside it")
        };
        self.diags.error("E1403", span, message);
    }

    /// An expression reading a place.
    pub(super) fn place_read(&mut self, cx: &FnCx, place: &hir::Place, span: Span) -> hir::Expr {
        let ty = cx.locals[place.local.0 as usize].ty;
        let mut value = Self::mk(hir::ExprKind::Local(place.local), ty, span);
        for step in &place.path {
            value = match step {
                hir::PlaceStep::Field(index) => {
                    let ty = self.types.components(value.ty).map_or(Ty::ERROR, |c| c[*index as usize]);
                    Self::mk(hir::ExprKind::Field(Box::new(value), *index), ty, span)
                }
                hir::PlaceStep::Index(index) => {
                    let ty = match *self.types.kind(value.ty) {
                        TyKind::Array(element) | TyKind::Deque(element) => element,
                        _ => Ty::ERROR,
                    };
                    Self::mk(hir::ExprKind::Index(Box::new(value), Box::new(index.clone())), ty, span)
                }
            };
        }
        value
    }

    // ---- expressions -----------------------------------------------------

    pub(super) fn mk(kind: hir::ExprKind, ty: Ty, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
    }

    /// The part of a type that is not `Send`, if any: a `Pointer`, or a
    /// value holding one.
    pub(super) fn not_send(&self, ty: Ty) -> Option<Ty> {
        if self.types.is_send(ty) {
            return None;
        }
        let repr = self.types.repr(ty);
        let parts = match self.types.kind(repr) {
            TyKind::Pointer(_) => return Some(ty),
            TyKind::Handle(_, value) => vec![*value],
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) | TyKind::CArray(t, _) => vec![*t],
            TyKind::Map(a, b) | TyKind::Result(a, b) => vec![*a, *b],
            TyKind::Tuple(items) | TyKind::Union(items) => items.clone(),
            _ => match (self.types.components(repr), self.types.variants(repr)) {
                (Some(items), _) => items,
                (None, Some(variants)) => variants.concat(),
                (None, None) => Vec::new(),
            },
        };
        parts.into_iter().find_map(|part| self.not_send(part)).or(Some(ty))
    }

    /// Whether a value may be a failure the program must see (11.9): a
    /// result that can fail.
    pub(super) fn must_use(&self, ty: Ty) -> bool {
        match self.types.kind(self.types.repr(ty)) {
            TyKind::Result(_, error) => *error != Ty::NEVER,
            _ => false,
        }
    }

    pub(super) fn error_expr(span: Span) -> hir::Expr {
        Self::mk(hir::ExprKind::Error, Ty::ERROR, span)
    }
}

/// Whether two regions of one array, `[start:end:step]`, provably share an
/// element: of constant bounds not below zero and no step but one, where an
/// end left out is the array's length. A region that ends at a constant is
/// valid only if the array reaches it, which the call needs.
fn regions_overlap(a: &[Option<Box<hir::Expr>>; 3], b: &[Option<Box<hir::Expr>>; 3]) -> bool {
    // `(start, end)`, `None` for the length; none when not constant.
    let range = |bounds: &[Option<Box<hir::Expr>>; 3]| -> Option<(i128, Option<i128>)> {
        let constant = |bound: &Option<Box<hir::Expr>>| -> Option<Option<i128>> {
            match bound.as_deref().map(|e| &e.kind) {
                None => Some(None),
                Some(hir::ExprKind::Int(value)) if *value >= 0 => Some(Some(*value)),
                _ => None,
            }
        };
        let step = constant(&bounds[2])?;
        if step.is_some_and(|step| step != 1) {
            return None;
        }
        Some((constant(&bounds[0])?.unwrap_or(0), constant(&bounds[1])?))
    };
    let (Some((a_start, a_end)), Some((b_start, b_end))) = (range(a), range(b)) else { return false };
    let from = a_start.max(b_start);
    match (a_end, b_end) {
        (Some(x), Some(y)) => from < x.min(y),
        (Some(end), None) | (None, Some(end)) => from < end,
        (None, None) => false,
    }
}
