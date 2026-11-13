//! Contexts and their initial values.

use super::*;

impl<'a> Checker<'a> {
    /// Whether a name is a context's (11.16).
    pub(super) fn is_context(&self, name: &str) -> bool {
        matches!(self.type_names.get(name), Some(TypeName::Nominal(ty)) if self.context_of.contains_key(ty))
    }

    /// The context a path names, and its index (11.16).
    pub(super) fn context_named(&mut self, path: &[ast::Ident]) -> Option<(Ty, u32)> {
        let [name] = path else { return None };
        let Some(&TypeName::Nominal(ty)) = self.type_names.get(&name.name) else { return None };
        let context = *self.context_of.get(&ty)?;
        self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        Some((ty, context))
    }

    /// Each context's maker (11.16): the function that gives its initial
    /// value in a `Ref`, which the runtime calls when a task first reads the
    /// context, with its constant initial values and zeros for the computed
    /// ones, each of which a function of its own computes when it is first
    /// read. The fields are `Send` and `Copy`, since tasks share them and
    /// reading one copies it, and those shown as text have `Display`.
    pub(super) fn context_makers(&mut self) -> Vec<ContextMade> {
        let mut makers = Vec::new();
        let mut reading = Vec::new();
        for (ty, decl) in self.contexts.clone() {
            let module = std::mem::replace(&mut self.module, decl.module);
            let ast::TypeBody::Record(declared) = &decl.body else { unreachable!("a context is a record") };
            let fields = self.record_fields(ty, decl.span).unwrap_or_default();
            let types: Vec<Ty> = fields.iter().map(|&(_, ty)| ty).collect();
            let reference = self.types.handle(Handle::Ref, ty);
            let context = self.context_of[&ty];
            let mut values = Vec::new();
            let mut initials = Vec::new();
            for (index, (field, (name, field_ty))) in declared.iter().zip(fields).enumerate() {
                let what = format!("the field `{name}` of the context `{}`", decl.name.name);
                if let Some(part) = self.not_send(field_ty) {
                    let message = format!("tasks share {what}, but {} is not `Send`", self.display(part));
                    self.diags.error("E1616", field.ty.span, message);
                }
                if !self.types.is_copy(field_ty) {
                    let message = format!(
                        "reading {what} copies it, but {} is not `Copy`: hold it in a `Ref`, or keep a handle to it",
                        self.display(field_ty)
                    );
                    self.diags.error("E1616", field.ty.span, message);
                }
                let shown = match *self.types.kind(field_ty) {
                    TyKind::Option(inner) => inner,
                    _ => field_ty,
                };
                let told = |a: &&ast::Annotation| a.name.name != "secret";
                let hidden = field.annotations.iter().any(|a| a.name.name == "secret") || self.types.secret(shown);
                for annotation in field.annotations.iter().filter(told) {
                    if hidden {
                        let message = format!(
                            "`@{}` writes {what} where anyone reads it, and it is `@secret`: leave one of the two",
                            annotation.name.name
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                    if !self.types.has_error(shown) && !self.satisfies(shown, DISPLAY, &[]) {
                        let message = format!(
                            "`@{}` shows {what} as text, but {} has no `Display`",
                            annotation.name.name,
                            self.display(shown)
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                }
                let Some(default) = &field.default else { continue };
                // Its own function's, in case it is computed, where `?` may
                // fail with what it propagates.
                let mut own = self.new_cx(field_ty);
                own.infer_error = true;
                self.initial_reads = Some(Vec::new());
                let value = self.expr(&mut own, default, Some(field_ty));
                let reads = self.initial_reads.take().unwrap_or_default();
                let mut value = self.coerce(value, field_ty);
                if self.types.has_error(value.ty) || (own.error.is_none() && constant(&mut value)) {
                    values.push((index as u32, value));
                    continue;
                }
                if let Some(annotation) = field.annotations.iter().find(told) {
                    let message = format!(
                        "`@{}` shows {what} when it is not its initial value, which must then be a constant, and this one is computed: give it a constant, or leave out `@{}`",
                        annotation.name.name, annotation.name.name
                    );
                    self.diags.error("E1616", annotation.span, message);
                }
                let zeroed = |ty: Ty| Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), ty, decl.span);
                values.push((index as u32, zeroed(field_ty)));
                let mut alone = Some(value);
                let parts = (0..types.len())
                    .map(|i| (i as u32, if i == index { alone.take().expect("once") } else { zeroed(types[i]) }))
                    .collect();
                let record = Self::mk(hir::ExprKind::Record(parts), ty, default.span);
                let made = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![record]), reference, default.span);
                let named = format!("{}.{name}", decl.name.name);
                let mut function = hir::Function {
                    name: format!("context {named}"),
                    generics: Vec::new(),
                    params: Vec::new(),
                    loans: Vec::new(),
                    fallible: own.error.is_some(),
                    locals: own.locals,
                    result: match own.error {
                        Some(error) => self.types.intern(TyKind::Result(reference, error)),
                        None => reference,
                    },
                    body: hir::Block { stmts: Vec::new(), tail: Some(Box::new(made)), ty: reference },
                    moves: HashSet::new(),
                    test: false,
                    needs: 0,
                    span: default.span,
                };
                self.ownership(&mut function, &own.params);
                reading.push(Reading { key: (context, index as u32), span: default.span, named: named.clone(), reads });
                initials.push(Initial { field: index as u32, function, error: own.error, named });
            }
            let record = Self::mk(hir::ExprKind::Record(values), ty, decl.span);
            let made = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![record]), reference, decl.span);
            let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(made)), ty: reference };
            let mut maker = hir::Function {
                name: format!("context {}", decl.name.name),
                generics: Vec::new(),
                params: Vec::new(),
                loans: Vec::new(),
                fallible: false,
                locals: Vec::new(),
                result: reference,
                body,
                moves: HashSet::new(),
                test: false,
                needs: 0,
                span: Span::default(),
            };
            self.ownership(&mut maker, &[]);
            let log = self.context_shown(decl, "log");
            let label = self.context_shown(decl, "label");
            self.module = module;
            makers.push(ContextMade { maker, log, label, initials });
        }
        self.initial_cycles(&reading);
        makers
    }

    /// Computed initial values that read each other, which no order can
    /// compute: `E1616` with the cycle, once (11.16).
    pub(super) fn initial_cycles(&mut self, reading: &[Reading]) {
        let at: HashMap<(u32, u32), usize> = reading.iter().enumerate().map(|(i, r)| (r.key, i)).collect();
        let next = |i: usize| reading[i].reads.iter().filter_map(|key| at.get(key).copied()).collect::<Vec<_>>();
        // 0 unseen, 1 on the path, 2 done.
        let mut state = vec![0u8; reading.len()];
        for start in 0..reading.len() {
            if state[start] != 0 {
                continue;
            }
            let mut path = vec![start];
            let mut pending = vec![next(start)];
            state[start] = 1;
            while let Some(children) = pending.last_mut() {
                let Some(child) = children.pop() else {
                    state[path.pop().expect("as deep as pending")] = 2;
                    pending.pop();
                    continue;
                };
                match state[child] {
                    0 => {
                        state[child] = 1;
                        path.push(child);
                        pending.push(next(child));
                    }
                    1 => {
                        let from = path.iter().position(|&i| i == child).expect("on the path");
                        let cycle: Vec<String> =
                            path[from..].iter().chain([&child]).map(|&i| format!("`{}`", reading[i].named)).collect();
                        let message = format!(
                            "the initial value of `{}` reads itself: {}; compute one of them from what the others are computed from",
                            reading[child].named,
                            cycle.join(" reads ")
                        );
                        self.diags.error("E1616", reading[child].span, message);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The function that gives a computed initial value whose `?` may fail
    /// (11.16): the record `computes` makes, or a panic that names the field
    /// and describes the error.
    pub(super) fn initial_failing(&mut self, computes: FnId, reference: Ty, error: Ty, named: &str) -> hir::Function {
        let span = Span::default();
        let mut cx = self.new_cx(reference);
        let (made, failure) = (self.hidden_local(&mut cx, reference), self.hidden_local(&mut cx, error));
        let result = self.types.intern(TyKind::Result(reference, error));
        let mk = |kind, ty| Self::mk(kind, ty, span);
        let pattern = |variant, local, ty| hir::Pattern {
            kind: hir::PatKind::Variant(variant, vec![hir::Pattern { kind: hir::PatKind::Binding(local), ty, span }]),
            ty: result,
            span,
        };
        let described = mk(
            hir::ExprKind::Reflect(hir::ReflectOp::Describe, error, vec![mk(hir::ExprKind::Local(failure), error)]),
            Ty::STRING,
        );
        let message = mk(
            hir::ExprKind::Interpolate(vec![
                hir::Segment::Text(format!("the initial value of `{named}` failed: ")),
                hir::Segment::Value(described),
            ]),
            Ty::STRING,
        );
        let arms = vec![
            hir::Arm {
                pattern: pattern(OK, made, reference),
                guard: None,
                body: mk(hir::ExprKind::Local(made), reference),
            },
            hir::Arm {
                pattern: pattern(ERR, failure, error),
                guard: None,
                body: mk(hir::ExprKind::Panic(Box::new(message), 8), Ty::NEVER),
            },
        ];
        let call = mk(hir::ExprKind::Call(Callee::Fn(computes), Vec::new(), Vec::new()), result);
        let value = mk(hir::ExprKind::Match(Box::new(call), arms), reference);
        let mut function = hir::Function {
            name: format!("context {named}"),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: reference,
            body: hir::Block { stmts: Vec::new(), tail: Some(Box::new(value)), ty: reference },
            moves: HashSet::new(),
            test: false,
            needs: 0,
            span,
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// The function that shows a context's fields marked `@log` or
    /// `@label` (11.16): for each that does not have its initial value, its
    /// name and its text, each followed by a zero character. A `T?` counts
    /// when it is `some`, with the text of what it holds.
    pub(super) fn context_shown(&mut self, decl: &'a ast::TypeDecl, kind: &str) -> Option<hir::Function> {
        let ast::TypeBody::Record(fields) = &decl.body else { return None };
        let span = decl.span;
        let ident = |name: &str| ast::Ident { name: name.to_string(), span };
        let expr = |kind: ast::ExprKind| ast::Expr { kind, span };
        let mut parts = Vec::new();
        for field in fields {
            let Some(annotation) = field.annotations.iter().find(|a| a.name.name == kind) else { continue };
            let key = match &annotation.argument {
                Some(ast::AnnotationArg::Text(text)) => text.clone(),
                _ => field.name.name.clone(),
            };
            let read = expr(ast::ExprKind::Path(vec![decl.name.clone(), field.name.clone()]));
            let shown = |value: ast::Expr| {
                let segments = vec![
                    ast::StrSegment::Text(format!("{key}\u{0}")),
                    ast::StrSegment::Expr(value, None),
                    ast::StrSegment::Text("\u{0}".into()),
                ];
                expr(ast::ExprKind::Str(segments))
            };
            let empty = || expr(ast::ExprKind::Str(Vec::new()));
            let ty = self.resolve_type(&field.ty);
            let part = if matches!(self.types.kind(ty), TyKind::Option(_)) {
                // `match field { some(value) => "name\0{value}\0", none => "" }`
                let value = ident("value");
                let pattern = |kind: ast::PatternKind| ast::Pattern { kind, span };
                let arm = |pattern: ast::Pattern, body: ast::Expr| ast::MatchArm {
                    pattern,
                    guard: None,
                    body: ast::ArmBody::Expr(body),
                    span,
                };
                let some = pattern(ast::PatternKind::Constructor {
                    path: vec![ident("some")],
                    args: vec![pattern(ast::PatternKind::Binding(value.clone()))],
                });
                let arms = vec![
                    arm(some, shown(expr(ast::ExprKind::Path(vec![value])))),
                    arm(pattern(ast::PatternKind::None), empty()),
                ];
                expr(ast::ExprKind::Match { scrutinee: Box::new(read), arms })
            } else if let (true, Some(initial)) = (self.types.is_equatable(ty), &field.default) {
                // `if field != initial { "name\0{field}\0" } else { "" }`
                let differs =
                    expr(ast::ExprKind::Binary(ast::BinOp::Ne, Box::new(read.clone()), Box::new(initial.clone())));
                let then = ast::Block { stmts: Vec::new(), tail: Some(Box::new(shown(read))), span };
                let otherwise = ast::Block { stmts: Vec::new(), tail: Some(Box::new(empty())), span };
                expr(ast::ExprKind::If {
                    condition: Box::new(differs),
                    then,
                    otherwise: Some(Box::new(expr(ast::ExprKind::Block(otherwise)))),
                })
            } else {
                shown(read)
            };
            parts.push(ast::StrSegment::Expr(part, None));
        }
        if parts.is_empty() {
            return None;
        }
        let mut cx = self.new_cx(Ty::STRING);
        let value = self.expr(&mut cx, &expr(ast::ExprKind::Str(parts)), Some(Ty::STRING));
        let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(value)), ty: Ty::STRING };
        let mut function = hir::Function {
            name: format!("context {} {kind}", decl.name.name),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: Ty::STRING,
            body,
            moves: HashSet::new(),
            test: false,
            needs: 0,
            span: Span::default(),
        };
        self.ownership(&mut function, &cx.params);
        Some(function)
    }
}
