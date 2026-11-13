//! Functions: their context, ownership, scripts, closures, `defer` and `spawn`.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn new_cx(&self, result: Ty) -> FnCx {
        let (success, error) = match self.types.kind(result) {
            TyKind::Result(ok, err) => (*ok, Some(*err)),
            _ => (result, None),
        };
        FnCx {
            errors_before: self.error_count(),
            entry_values: Vec::new(),
            withs: Vec::new(),
            locals: Vec::new(),
            local_defs: HashMap::new(),
            result_span: None,
            mutable: Vec::new(),
            scopes: Scopes::opened(),
            result: success,
            error,
            infer_error: false,
            undeclared: None,
            gathers: false,
            loops: Vec::new(),
            diverged: false,
            parent: None,
            env: None,
            captures: Vec::new(),
            loans_kept: Vec::new(),
            mutates: false,
            infer_result: false,
            params: Vec::new(),
            deferred: false,
            body: false,
            unsafe_region: false,
            unsafe_body: false,
            lent: Vec::new(),
            loan_bindings: HashSet::new(),
            test_member: false,
            setup: false,
            named: Vec::new(),
            reads: HashSet::new(),
            template: false,
        }
    }

    pub(super) fn function(&mut self, f: &ast::FnDecl, id: FnId) -> hir::Function {
        self.leaving = Some(f.name.span);
        self.calling = Some(id);
        let signature = self.signatures[id.0 as usize].clone();
        let scope = self.fn_scopes[id.0 as usize].clone();
        let trait_default = scope.self_type.is_some_and(|ty| matches!(self.types.kind(ty), TyKind::Param(_)));
        self.naming = self.traced_name(f, id);
        let outer_types = std::mem::replace(&mut self.type_params, scope.types);
        let outer_self = std::mem::replace(&mut self.self_type, scope.self_type);
        let outer_implementing = std::mem::replace(&mut self.implementing, scope.implementing);
        self.module = scope.module;
        // An opaque result's witness is inferred from the body (12.8).
        let opaque = match self.types.kind(signature.result) {
            TyKind::Opaque(..) => Some(signature.result),
            TyKind::Result(ok, _) if matches!(self.types.kind(*ok), TyKind::Opaque(..)) => Some(*ok),
            _ => None,
        };
        let mut cx = match opaque {
            Some(opaque) => {
                let open = match self.types.kind(signature.result) {
                    TyKind::Result(_, error) => self.types.intern(TyKind::Result(Ty::ERROR, *error)),
                    _ => Ty::ERROR,
                };
                let mut cx = self.new_cx(open);
                cx.infer_result = true;
                let _ = opaque;
                cx
            }
            None => self.new_cx(signature.result),
        };
        let mut params = Vec::new();
        let mut loans = Vec::new();
        for (param, ty) in f.params.iter().zip(signature.params) {
            // `mut` and `var` parameters, receivers included, borrow the
            // caller's place (16.3); `ref` and values are read-only values.
            let loan = matches!(param.mode, ast::Mode::Mut | ast::Mode::Var);
            let key = crate::index::Key::Param(id.0, param.name.name.clone());
            let local = self.declare_with(&mut cx, &param.name, ty, loan, Some(key));
            cx.params[local.0 as usize] = Some(param.mode);
            params.push(local);
            loans.push(loan);
        }
        let success = (!cx.infer_result).then_some(cx.result);
        cx.result_span = f.result.as_ref().map(|t| (t.span, f.public));
        // `main` without a declared error infers it, as a script does (20.2).
        let main = f.result.is_none() && Some(id) == self.main_fn();
        cx.infer_error = main;
        cx.gathers = true;
        cx.body = true;
        cx.unsafe_region = signature.is_unsafe;
        cx.unsafe_body = signature.is_unsafe;
        // Only a trait's required methods end at their signature.
        let nothing = ast::Block { stmts: Vec::new(), tail: None, span: f.span };
        let (written, success) = match &f.body {
            Some(written) => (written, success),
            None => {
                let name = &f.name.name;
                let message =
                    format!("`{name}` has no body: write what it does in a block after its signature, `{{ }}`");
                self.diags.error("E0004", f.name.span, message);
                (&nothing, None)
            }
        };
        let body = self.block(&mut cx, written, success);
        if let Some(opaque) = opaque {
            self.record_witness(&cx, opaque, &body, f);
        }
        // What a signature promises is not this body's to use: a trait's default
        // speaks for every implementation, and a parameter may only give a `T`.
        for (param, local) in f.params.iter().zip(&params) {
            if trait_default || self.gives_type(f, param) {
                cx.reads.insert(*local);
            }
        }
        self.unused(&cx, &params);
        if let Some((error, at, code)) = cx.undeclared {
            self.undeclared_error(f, signature.result, error, at, code);
        }
        self.type_params = outer_types;
        self.self_type = outer_self;
        self.implementing = outer_implementing;
        self.calling = None;
        let result = match cx.error {
            Some(error) if main => {
                let result = self.types.intern(TyKind::Result(Ty::UNIT, error));
                self.signatures[id.0 as usize].result = result;
                result
            }
            _ => signature.result,
        };
        let mut function = hir::Function {
            name: self.naming.clone(),
            generics: signature.generics,
            params,
            loans,
            fallible: cx.error.is_some(),
            locals: cx.locals,
            result,
            body,
            moves: HashSet::new(),
            test: false,
            needs: 0,
            span: f.name.span,
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// The name a trace gives a function: a trait's method by the type,
    /// `Cell.display`, and `Box.display[Int]` once monomorphized.
    pub(super) fn traced_name(&self, f: &ast::FnDecl, id: FnId) -> String {
        let scope = &self.fn_scopes[id.0 as usize];
        match (&scope.implementing, scope.self_type) {
            (Some(_), Some(target)) => {
                let target = self.types.display(target);
                format!("{}.{}", target.split('[').next().unwrap_or(&target), f.name.name)
            }
            _ => self.signatures[id.0 as usize].name.clone(),
        }
    }

    /// Checks moves and obligations (8.10) once the body is typed; whether
    /// a closure body gives a capture away.
    pub(super) fn ownership(&mut self, function: &mut hir::Function, params: &[Option<ast::Mode>]) -> bool {
        let (signatures, traits, externs) = (&self.signatures, &self.traits, &self.externs);
        let modes = |callee: Callee| match callee {
            Callee::Fn(id) => signatures.get(id.0 as usize).map(|s| s.modes.clone()).unwrap_or_default(),
            Callee::Trait(id, index) => traits[id.0 as usize].methods[index as usize].signature.modes.clone(),
            Callee::Std(function) => function.modes(),
            // C borrows what it receives.
            Callee::Extern(id) => vec![ast::Mode::Ref; externs[id as usize].params.len()],
        };
        // The parameter modes of the type parameters bound by `Call...`.
        let bound_modes: HashMap<Ty, Vec<ast::Mode>> = self
            .call_bounds
            .iter()
            .filter_map(|(&param, &(_, signature))| match self.types.kind(signature) {
                TyKind::Fn(_, _, _, modes, _) => Some((param, modes.clone())),
                _ => None,
            })
            .collect();
        // The type parameters a call consumes.
        let once: HashSet<Ty> =
            self.call_bounds.iter().filter(|(_, (p, _))| *p == Protocol::CallOnce).map(|(&t, _)| t).collect();
        let context = crate::ownership::Context { params, modes: &modes, bound_modes: &bound_modes, once: &once };
        let gives;
        (function.moves, gives) = crate::ownership::check(&mut self.types, function, context, self.diags);
        gives
    }

    /// The implicit `main` of a root script (spec 20.3). Its error type is
    /// inferred from the `?` and `fail` in its statements.
    pub(super) fn script_main(&mut self, statements: &[ast::Stmt]) -> hir::Function {
        self.naming = "main".into();
        self.leaving = None;
        // The statements are the root module's, as its functions are.
        self.module = 0;
        let mut cx = self.new_cx(Ty::UNIT);
        cx.infer_error = true;
        self.scripting = true;
        let stmts = self.statements(&mut cx, statements);
        self.scripting = false;
        self.unused(&cx, &[]);
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        let body = hir::Block { stmts, tail: None, ty };
        let result = match cx.error {
            Some(error) => self.types.intern(TyKind::Result(Ty::UNIT, error)),
            None => Ty::UNIT,
        };
        let mut function = hir::Function {
            name: "main".into(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: cx.error.is_some(),
            locals: cx.locals,
            result,
            body,
            moves: HashSet::new(),
            test: false,
            needs: 0,
            span: statements.first().map_or(Span::default(), |s| s.span),
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// The error type a `fail` or `?` may produce here, inferring it in a
    /// root script; `None` in an infallible function.
    pub(super) fn error_channel(&mut self, cx: &mut FnCx, produced: Ty, span: Span, code: &'static str) -> Option<Ty> {
        match cx.error {
            // What infers its errors fails with the union of what it
            // propagates (20.3).
            _ if cx.infer_error => {
                let channel = match cx.error {
                    Some(error) => self.types.union(&[error, produced]),
                    None => produced,
                };
                cx.error = Some(channel);
                Some(channel)
            }
            Some(error) => Some(error),
            None if cx.gathers => {
                cx.undeclared = Some(match cx.undeclared {
                    Some((before, at, code)) => (self.types.union(&[before, produced]), at, code),
                    None => (produced, span, code),
                });
                None
            }
            None => {
                self.diags.error(code, span, "this function cannot fail; declare its result as `T ! E`");
                None
            }
        }
    }

    /// `?` or `fail` in a function that declares no error: what it
    /// propagates, with the result that declares it and the imports that
    /// name it.
    pub(super) fn undeclared_error(&mut self, f: &ast::FnDecl, success: Ty, error: Ty, at: Span, code: &'static str) {
        if self.types.has_error(error) {
            return;
        }
        let result = self.types.intern(TyKind::Result(success, error));
        let declared = self.types.display(result);
        let message = format!(
            "`{}` cannot fail, but this propagates {}: declare its result as `{declared}`",
            f.name.name,
            self.display(error)
        );
        self.diags.error(code, at, message);
        let mut edits = match &f.result {
            Some(written) => vec![(written.span, declared.clone())],
            None => {
                // After the parameters' `)`.
                let body = f.body.as_ref().map_or(f.span.end, |b| b.span.start);
                let between = self.sources.slice(Span::new(f.name.span.end, body));
                let Some(close) = between.rfind(')') else { return };
                let place = f.name.span.end + close as u32 + 1;
                vec![(Span::new(place, place), format!(": {declared}"))]
            }
        };
        edits.extend(self.imports_naming(error, at));
        // Callers of a function that now fails must handle it.
        self.diags.fix(format!("declare the result as `{declared}`"), false, edits);
    }

    /// The imports a file lacks to name a type as it is shown, as
    /// `text.TextError` needs `import std.text`.
    pub(super) fn imports_naming(&self, ty: Ty, at: Span) -> Vec<(Span, String)> {
        let file = self.sources.file_of(at.start);
        let imported: HashSet<&str> = file
            .text
            .lines()
            .filter_map(|line| {
                let path = line.strip_prefix("import ")?.trim();
                Some(match path.split_once(" as ") {
                    Some((_, alias)) => alias.trim(),
                    None => path.rsplit('.').next().unwrap_or(path),
                })
            })
            .collect();
        let shown = self.types.display(ty);
        let mut modules: Vec<&str> = shown
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .filter_map(|name| name.rsplit_once('.').map(|(module, _)| module))
            .filter(|module| !module.contains('.') && !imported.contains(module))
            .collect();
        modules.dedup();
        let standard = crate::api::standard_modules();
        modules
            .into_iter()
            .filter_map(|module| {
                let path = format!("std.{module}");
                let path = match standard.contains(&path) {
                    true => path,
                    false => self.module_paths.iter().find(|p| p.last().is_some_and(|l| l == module))?.join("."),
                };
                Some((self.import_place(at, &path), format!("import {path}\n")))
            })
            .collect()
    }

    /// Where an import of `path` goes in the file of `at`: before the first
    /// import that sorts after it, as the formatter orders them, or after
    /// the last one, or at the file's start.
    /// The standard modules that declare `name`, those the file of `at`
    /// imports first.
    pub(super) fn declaring(&self, at: Span, name: &str) -> Vec<String> {
        let mut owners = crate::api::declaring(name);
        owners.sort_by_key(|owner| !self.imported(at, owner));
        owners
    }

    /// Whether an expression only makes a value, with no call in it.
    pub(super) fn only_a_value(written: &ast::Expr) -> bool {
        use ast::ExprKind as E;
        match &written.kind {
            E::Int { .. } | E::Float { .. } | E::BigInt(_) | E::Bool(_) | E::Char(_) | E::Path(_) => true,
            E::Str(segments) => segments.iter().all(|segment| match segment {
                ast::StrSegment::Text(_) => true,
                ast::StrSegment::Expr(part, _) => Self::only_a_value(part),
            }),
            E::Field(base, _) | E::TupleSlot(base, _) => Self::only_a_value(base),
            E::Binary(_, left, right) => Self::only_a_value(left) && Self::only_a_value(right),
            E::Tuple(parts) | E::Array(parts) => parts.iter().all(Self::only_a_value),
            _ => false,
        }
    }

    /// What a statement or the end of a block with no value leaves (5.2, 11.9):
    /// a call abandons its result, but a value with no call does nothing, and
    /// W1018 a call whose `String` nothing uses, made for nothing or meant to
    /// be written.
    pub(super) fn dropped_value(&mut self, written: &ast::Expr, value: &hir::Expr) {
        let plain = !matches!(value.ty, Ty::UNIT | Ty::NEVER) && !self.types.has_error(value.ty);
        if plain && Self::only_a_value(written) && self.types.is_copy(value.ty) {
            let message = "this value does nothing: use it, return it from a function that declares its type, \
                or discard it with `_ = ...`";
            self.diags.error("E1306", value.span, message);
            let start = Span::new(value.span.start, value.span.start);
            self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
            return;
        }
        let call = match &written.kind {
            ast::ExprKind::Try(inner) => &inner.kind,
            kind => kind,
        };
        if !matches!(call, ast::ExprKind::Call { .. }) {
            return;
        }
        if value.ty != Ty::STRING {
            if plain && !self.must_use(value.ty) {
                self.dropped_result(call, value);
            }
            return;
        }
        let message = "nothing uses the `String` of this call, and making it has a cost: write it with \
            `console.println(...)`, or discard it with `_ = ...`";
        self.diags.warning("W1018", value.span, message);
        let (start, end) = (Span::new(value.span.start, value.span.start), Span::new(value.span.end, value.span.end));
        let console = self.imported_as(value.span, "std.console");
        let mut edits = vec![(start, format!("{console}.println(")), (end, ")".to_string())];
        if !self.imported(value.span, "std.console") {
            edits.push((self.import_place(value.span, "std.console"), "import std.console\n".to_string()));
        }
        self.diags.fix("write it with `console.println`", false, edits);
        self.diags.fix("discard it with `_ = ...`", false, vec![(start, "_ = ".into())]);
    }

    /// W1018 of a result that is not a text, when the call has no other effect:
    /// a method of the language that only reads, or a function of the program
    /// that only computes, which is known once all are declared.
    pub(super) fn dropped_result(&mut self, call: &ast::ExprKind, value: &hir::Expr) {
        if let hir::ExprKind::Call(Callee::Fn(id), ..) = &value.kind {
            self.dropped_calls.push((*id, value.span));
        } else if let ast::ExprKind::Call { callee, .. } = call
            && let Some(method) = match &callee.kind {
                ast::ExprKind::Field(_, method) => Some(method),
                // `names.length()` on a binding is a path.
                ast::ExprKind::Path(segments) if segments.len() > 1 => segments.last(),
                _ => None,
            }
            && let Some(receiver) = Self::receiver(value)
            && self.only_reads(receiver.ty, &method.name)
        {
            self.result_unused(value.span);
        }
    }

    /// The value a method of the language is called on, in what its call
    /// became: `map.get(key)` an operation of the map, `text.isEmpty()` a
    /// comparison of its length.
    fn receiver(value: &hir::Expr) -> Option<&hir::Expr> {
        match &value.kind {
            hir::ExprKind::Length(receiver) => Some(receiver),
            hir::ExprKind::Binary(BinOp::Eq, length, _) => match &length.kind {
                hir::ExprKind::Length(_) | hir::ExprKind::StrOp(hir::StrOp::ByteLength, _) => Self::receiver(length),
                _ => None,
            },
            hir::ExprKind::MapOp(_, values) | hir::ExprKind::ArrayOp(_, values) | hir::ExprKind::StrOp(_, values) => {
                values.first()
            }
            hir::ExprKind::Call(Callee::Std(_), _, args) => args.first(),
            _ => None,
        }
    }

    pub(super) fn result_unused(&mut self, span: Span) {
        let message =
            "nothing uses what this call gives, and it does nothing else: use it, or discard it with `_ = ...`";
        self.diags.warning("W1018", span, message);
        self.diags.fix("discard it with `_ = ...`", false, vec![(Span::new(span.start, span.start), "_ = ".into())]);
    }

    /// Whether a method of a collection or a text leaves its receiver as it is.
    pub(super) fn only_reads(&self, ty: Ty, name: &str) -> bool {
        let owner = match self.types.kind(ty) {
            TyKind::String => "String",
            TyKind::Array(_) => "Array",
            TyKind::Deque(_) => "Deque",
            TyKind::Map(..) => "Map",
            TyKind::Set(_) => "Set",
            _ => return false,
        };
        crate::api::builtin_doc(owner, name).is_some_and(|(declaration, _)| declaration.contains("(self"))
    }

    /// Whether a function of the program only computes: what it does is
    /// arithmetic, values and calls of others that do. Anything else counts as
    /// an effect, a panic included, so that this only says what it knows.
    pub(super) fn only_computes(&self, bodies: &[&ast::FnDecl], id: FnId, seen: &mut HashSet<u32>) -> bool {
        if !seen.insert(id.0) {
            return true;
        }
        let Some(f) = bodies.get(id.0 as usize) else { return false };
        let plain = f.params.iter().all(|param| matches!(param.mode, ast::Mode::Value | ast::Mode::Ref));
        plain && f.foreign.is_none() && f.body.as_ref().is_some_and(|body| self.block_computes(bodies, body, seen))
    }

    pub(super) fn block_computes(&self, bodies: &[&ast::FnDecl], block: &ast::Block, seen: &mut HashSet<u32>) -> bool {
        let stmts = block.stmts.iter().all(|stmt| match &stmt.kind {
            ast::StmtKind::Let { value: Some(value), .. } | ast::StmtKind::Return(Some(value)) => {
                self.computes(bodies, value, seen)
            }
            ast::StmtKind::Expr(value) => self.computes(bodies, value, seen),
            _ => false,
        });
        stmts && block.tail.iter().all(|tail| self.computes(bodies, tail, seen))
    }

    pub(super) fn computes(&self, bodies: &[&ast::FnDecl], value: &ast::Expr, seen: &mut HashSet<u32>) -> bool {
        use ast::ExprKind as E;
        match &value.kind {
            E::Int { .. } | E::Float { .. } | E::BigInt(_) | E::Bool(_) | E::Char(_) | E::Unit | E::Path(_) => true,
            E::Str(segments) => segments.iter().all(|segment| match segment {
                ast::StrSegment::Text(_) => true,
                ast::StrSegment::Expr(part, _) => self.computes(bodies, part, seen),
            }),
            E::Field(base, _) | E::TupleSlot(base, _) | E::Unary(_, base) => self.computes(bodies, base, seen),
            E::Binary(_, left, right) => self.computes(bodies, left, seen) && self.computes(bodies, right, seen),
            E::Tuple(parts) | E::Array(parts) => parts.iter().all(|part| self.computes(bodies, part, seen)),
            E::Block(block) => self.block_computes(bodies, block, seen),
            E::If { condition, then, otherwise } => {
                self.computes(bodies, condition, seen)
                    && self.block_computes(bodies, then, seen)
                    && otherwise.iter().all(|part| self.computes(bodies, part, seen))
            }
            E::Call { callee, args } => {
                let E::Path(path) = &callee.kind else { return false };
                let [name] = path.as_slice() else { return false };
                let Some(&ModuleName::Fn(id)) = self.names.get(&name.name) else { return false };
                args.iter().all(|arg| self.computes(bodies, &arg.value, seen)) && self.only_computes(bodies, id, seen)
            }
            _ => false,
        }
    }

    /// Whether the file of `at` imports the module `path`.
    pub(super) fn imported(&self, at: Span, path: &str) -> bool {
        let file = self.sources.file_of(at.start);
        file.text
            .lines()
            .any(|line| line.strip_prefix("import ").and_then(|w| w.split_whitespace().next()) == Some(path))
    }

    /// The name the file of `at` gives the module `path`: its alias, or its
    /// last segment.
    pub(super) fn imported_as(&self, at: Span, path: &str) -> String {
        let file = self.sources.file_of(at.start);
        let alias = file.text.lines().find_map(|line| {
            let mut words = line.strip_prefix("import ")?.split_whitespace();
            (words.next()? == path).then(|| (words.next() == Some("as")).then(|| words.next()).flatten()).flatten()
        });
        alias.map(str::to_string).unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string())
    }

    pub(super) fn import_place(&self, at: Span, path: &str) -> Span {
        let file = self.sources.file_of(at.start);
        let mut place = None;
        let mut offset = 0;
        for line in file.text.split_inclusive('\n') {
            if let Some(written) = line.strip_prefix("import ") {
                if written.trim() > path {
                    place = Some(offset);
                    break;
                }
                place = Some(offset + line.len());
            }
            offset += line.len();
        }
        let place = file.base + place.unwrap_or(0) as u32;
        Span::new(place, place)
    }

    /// `main` used as a value or called: where the program starts, whose
    /// error its body infers (20.2), is not.
    pub(super) fn main_called(&mut self, span: Span) -> hir::Expr {
        let message = "`main` is where the program starts, and nothing calls it: move what it does to a function, \
                       and call that";
        self.diags.error("E1803", span, message);
        Self::error_expr(span)
    }

    /// The root module's `main`.
    pub(super) fn main_fn(&self) -> Option<FnId> {
        match self.names.get("main") {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        }
    }

    pub(super) fn lookup(&self, cx: &FnCx, name: &str) -> Option<LocalId> {
        cx.scopes.find(name)
    }

    /// Whether `name` is a binding of an enclosing function, which a
    /// closure can capture.
    pub(super) fn visible_capture(&self, cx: &FnCx, name: &str) -> bool {
        !cx.template
            && cx
                .parent
                .as_deref()
                .is_some_and(|parent| self.lookup(parent, name).is_some() || self.visible_capture(parent, name))
    }

    /// Reads the binding `name` of an enclosing function from inside a
    /// closure, capturing it on first use (11.8).
    pub(super) fn capture(&mut self, cx: &mut FnCx, name: &str, span: Span) -> Option<hir::Expr> {
        if cx.template {
            return None;
        }
        let env = cx.env?;
        let test_member = cx.test_member;
        if let Some(index) = cx.captures.iter().position(|c| c.name == name)
            && let Some(&(_, lent)) = cx.loans_kept.iter().find(|(i, _)| *i == index as u32)
        {
            return Some(Self::mk(hir::ExprKind::Local(lent), cx.captures[index].ty, span));
        }
        // A `defer` block uses a `ref` parameter of its function as the loan
        // it is, which outlives the block (13.7): the closure holds a copy
        // of it, which its body lends as a `ref` parameter.
        let deferred_loan = cx.deferred
            && cx.parent.as_deref().is_some_and(|parent| {
                self.lookup(parent, name).is_some_and(|local| {
                    parent.params[local.0 as usize] == Some(ast::Mode::Ref) && Some(local) != parent.env
                })
            });
        if deferred_loan && !cx.captures.iter().any(|c| c.name == name) {
            let parent = cx.parent.as_deref_mut()?;
            let local = self.lookup(parent, name)?;
            parent.reads.insert(local);
            let ty = parent.locals[local.0 as usize].ty;
            let source = Self::mk(hir::ExprKind::Loan(hir::Place { local, path: Vec::new() }), ty, span);
            cx.captures.push(Capture { name: name.into(), ty, source, mutable: false });
            let index = cx.captures.len() as u32 - 1;
            let lent = self.hidden_local(cx, ty);
            cx.params[lent.0 as usize] = Some(ast::Mode::Ref);
            cx.loans_kept.push((index, lent));
            return Some(Self::mk(hir::ExprKind::Local(lent), ty, span));
        }
        let index = match cx.captures.iter().position(|c| c.name == name) {
            Some(index) => index,
            None => {
                let parent = cx.parent.as_deref_mut()?;
                if let Some(local) = self.lookup(parent, name) {
                    parent.reads.insert(local);
                }
                if test_member && let Some(local) = self.lookup(parent, name) {
                    let index = local.0 as usize;
                    let ty = parent.locals[index].ty;
                    let why = if parent.params[index].is_some_and(|m| m != ast::Mode::Value) {
                        Some("a loan".to_string())
                    } else if parent.mutable[index] {
                        Some("a `var`".to_string())
                    } else if !self.types.is_copy(ty) {
                        Some(format!("of type {}, which is not `Copy`", self.display(ty)))
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        let message = format!(
                            "a test cannot use `{name}` of its suite, {why}: a suite shares only `let` bindings of `Copy` types; make what the test needs inside it"
                        );
                        self.diags.error("E2005", span, message);
                        return Some(Self::error_expr(span));
                    }
                }
                let (source, mutable) = match self.lookup(parent, name) {
                    Some(local)
                        if matches!(
                            parent.params[local.0 as usize],
                            Some(ast::Mode::Ref | ast::Mode::Mut | ast::Mode::Var)
                        ) =>
                    {
                        // A loan never escapes into a closure (16.5).
                        let message = if name == "self" {
                            "a closure cannot capture the receiver `self`, a loan; copy what it needs first, as in `let offset = self.offset`".to_string()
                        } else {
                            format!(
                                "a closure cannot capture `{name}`, a loan; copy what it needs into a binding first, or pass it as a closure parameter"
                            )
                        };
                        self.diags.error("E1402", span, message);
                        return Some(Self::error_expr(span));
                    }
                    Some(local) => {
                        if let Some((lent, _)) = parent.lent.iter().find(|(place, e)| place.local == local && *e) {
                            let lent = self.place_name(parent, lent);
                            self.lent_error(&lent, true, span);
                        }
                        let ty = parent.locals[local.0 as usize].ty;
                        let mutable = parent.mutable[local.0 as usize] && Some(local) != parent.env;
                        (Self::mk(hir::ExprKind::Local(local), ty, span), mutable)
                    }
                    None => {
                        let source = self.capture(parent, name, span)?;
                        let mutable = parent.captures.iter().any(|c| c.name == name && c.mutable);
                        (source, mutable)
                    }
                };
                // A defer block is no value that outlives its scope.
                if !cx.unsafe_body && !cx.deferred && self.holds_pointer(source.ty) {
                    let message = format!(
                        "a closure that captures `{name}`, which holds a raw pointer, is `unsafe`: write `unsafe (...) {{ ... }}`"
                    );
                    self.diags.error("E1702", span, message);
                }
                cx.captures.push(Capture { name: name.into(), ty: source.ty, source, mutable });
                cx.captures.len() - 1
            }
        };
        let closure = Self::mk(hir::ExprKind::Local(env), cx.locals[env.0 as usize].ty, span);
        Some(Self::mk(hir::ExprKind::Field(Box::new(closure), index as u32), cx.captures[index].ty, span))
    }

    /// A closure expression (11.8). Its type is an anonymous record of the
    /// bindings it captures; its body is a function whose first parameter
    /// is the closure, generic over the enclosing type parameters.
    pub(super) fn closure(
        &mut self,
        cx: &mut FnCx,
        closure: &ast::Closure,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        // Where an optional function is expected, a closure is that
        // function, which the coercion of its value makes `some`.
        let expected = match expected.map(|ty| self.types.kind(ty)) {
            Some(&TyKind::Option(inner)) if matches!(self.types.kind(inner), TyKind::Fn(..)) => Some(inner),
            _ => expected,
        };
        let parts = (&closure.params[..], closure.result.as_ref(), &closure.body);
        self.closure_parts(cx, parts, expected, false, closure.is_unsafe, span)
    }

    /// A closure from its parts; a `defer` block is a closure without
    /// parameters whose body cannot leave its scope (13.7).
    pub(super) fn closure_parts(
        &mut self,
        cx: &mut FnCx,
        (closure_params, closure_result, closure_body): (&[ast::ClosureParam], Option<&ast::TypeExpr>, &ast::Block),
        expected: Option<Ty>,
        deferred: bool,
        is_unsafe: bool,
        span: Span,
    ) -> hir::Expr {
        let kind = ClosureKind { deferred, is_unsafe, test: false, infer_error: false };
        let body = |this: &mut Self, cx: &mut FnCx, success| this.block(cx, closure_body, success);
        self.closure_with(cx, (closure_params, closure_result), expected, kind, span, body)
    }

    /// A closure whose body `check_body` checks in the closure's context,
    /// given the success type it must produce, if known. The closure of a
    /// test or a suite infers its errors, as a script does.
    pub(super) fn closure_with(
        &mut self,
        cx: &mut FnCx,
        (closure_params, closure_result): (&[ast::ClosureParam], Option<&ast::TypeExpr>),
        expected: Option<Ty>,
        ClosureKind { deferred, is_unsafe, test, infer_error }: ClosureKind,
        span: Span,
        check_body: impl FnOnce(&mut Self, &mut FnCx, Option<Ty>) -> hir::Block,
    ) -> hir::Expr {
        // A closure that C calls takes its parameter types from the
        // `extern fn(...)` expected.
        let expected = expected.and_then(|t| match self.types.kind(t).clone() {
            TyKind::CFn(params, result, _) => Some(Callable {
                modes: vec![ast::Mode::Value; params.len()],
                params,
                result,
                variadic: false,
                mutates: false,
                once: false,
                is_unsafe: false,
            }),
            _ => self.callable(t),
        });
        let mut params = Vec::new();
        let mut modes = Vec::new();
        let mut variadic = false;
        for (index, param) in closure_params.iter().enumerate() {
            // A parameter without a type takes its mode from the expected
            // signature too.
            let mode = match (&param.ty, &expected) {
                (None, Some(callable)) => callable.modes.get(index).copied().unwrap_or(ast::Mode::Value),
                _ => param.mode,
            };
            modes.push(mode);
            let ty = match (&param.ty, &expected) {
                (Some(ty), _) => {
                    let ty = self.resolve_type(ty);
                    if param.variadic {
                        variadic = true;
                        self.types.intern(TyKind::Array(ty))
                    } else {
                        ty
                    }
                }
                (None, Some(callable)) if index < callable.params.len() => {
                    variadic = callable.variadic && index + 1 == callable.params.len();
                    callable.params[index]
                }
                (None, _) => {
                    let message =
                        format!("closure parameter `{0}` needs a type: `({0}: Int) {{ ... }}`", param.name.name);
                    self.diags.error("E1101", param.name.span, message);
                    Ty::ERROR
                }
            };
            params.push(ty);
        }
        // Declaring no result where no function type is expected, it
        // infers what it fails with, as `main` does (11.8).
        let infer_error = infer_error || (closure_result.is_none() && expected.is_none());
        let declared = closure_result
            .map(|t| self.resolve_type(t))
            .or(expected.as_ref().map(|e| e.result).filter(|&r| r != Ty::ERROR));
        // Expected to give `T ! E` with `E` not known yet, it infers its
        // errors, none when it cannot fail.
        let infer_failure =
            declared.is_some_and(|d| matches!(self.types.kind(d), TyKind::Result(_, e) if *e == Ty::ERROR));
        let generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let template = self.types.declare_template("closure".into(), generics.clone());
        let env_ty = self.types.instantiate(template, generics.clone());
        let index = self.closure_functions.len();
        self.closure_functions.push(None);
        let body_id = FnId(self.closure_base + index as u32);
        let info = ClosureInfo {
            body: body_id.0,
            params: params.clone(),
            result: declared.unwrap_or(Ty::ERROR),
            variadic,
            modes: modes.clone(),
            mutates: false,
            once: false,
            is_unsafe,
        };
        self.types.declare_closure(template, info);

        let name = match self.closure_name.take() {
            Some(name) => name,
            None if deferred => format!("defer in {}", self.naming),
            None => format!("closure in {}", self.naming),
        };
        // The closures of a test are in the test.
        let outer_naming = test.then(|| std::mem::replace(&mut self.naming, name.clone()));
        // The body is checked in its own context, nested in the current one.
        let mut inner = self.new_cx(declared.unwrap_or(Ty::ERROR));
        inner.infer_result = declared.is_none();
        inner.infer_error = test || infer_error || infer_failure;
        if infer_failure {
            inner.error = None;
        }
        inner.test_member = test;
        inner.deferred = deferred;
        inner.template = std::mem::take(&mut self.templating);
        inner.unsafe_region = is_unsafe;
        inner.unsafe_body = is_unsafe;
        let outer = std::mem::replace(cx, inner);
        cx.parent = Some(Box::new(outer));
        let env = self.hidden_local(cx, env_ty);
        cx.mutable[env.0 as usize] = true;
        cx.env = Some(env);
        let mut locals = vec![env];
        for ((param, &ty), &mode) in closure_params.iter().zip(&params).zip(&modes) {
            let loan = matches!(mode, ast::Mode::Mut | ast::Mode::Var);
            let local = self.declare(cx, &param.name, ty, loan);
            cx.params[local.0 as usize] = Some(mode);
            locals.push(local);
        }
        let success = (!cx.infer_result).then_some(cx.result);
        let mut body = check_body(self, cx, success);
        // The loans a `defer` block keeps, lent from its captures.
        for &(index, lent) in cx.loans_kept.iter().rev() {
            let place = hir::Place { local: env, path: vec![hir::PlaceStep::Field(index)] };
            body.stmts.insert(0, hir::Stmt::Lend { local: lent, place });
        }
        if let Some(outer) = outer_naming {
            self.naming = outer;
        }
        let result = match declared {
            Some(declared) => declared,
            None if cx.result != Ty::ERROR => cx.result,
            None => body.ty,
        };
        let result = match cx.error {
            _ if infer_failure => {
                let success = cx.result;
                self.types.intern(TyKind::Result(success, cx.error.unwrap_or(Ty::NEVER)))
            }
            Some(error) if test || infer_error => self.types.intern(TyKind::Result(result, error)),
            _ => result,
        };
        let parent = cx.parent.take().expect("a closure context has a parent");
        let inner = std::mem::replace(cx, *parent);
        self.unused(&inner, &locals);

        let fields = inner
            .captures
            .iter()
            .map(|c| Field { name: c.name.clone(), ty: c.ty, private: false, secret: false })
            .collect();
        self.types.define_template(template, AdtKind::Record(fields));
        let info = self.types.closure_info_mut(template);
        info.result = result;
        info.mutates = inner.mutates;
        // The closure itself and its `mut` and `var` parameters are loans.
        let loans =
            [true].into_iter().chain(modes.iter().map(|m| matches!(m, ast::Mode::Mut | ast::Mode::Var))).collect();
        let mut params = inner.params;
        params[locals[0].0 as usize] = Some(ast::Mode::Mut);
        let mut function = hir::Function {
            name,
            generics,
            params: locals,
            loans,
            fallible: inner.error.is_some() || infer_failure,
            locals: inner.locals,
            result,
            body,
            moves: HashSet::new(),
            test,
            needs: 0,
            span,
        };
        // A cleanup runs once, so it may give its captures away, but not a
        // loan it keeps; another closure that gives one away is called once
        // (`CallOnce`).
        if self.ownership(&mut function, &params) && !deferred {
            self.types.closure_info_mut(template).once = true;
        }
        self.closure_functions[index] = Some(function);
        // The closure value holds the captured values, read now.
        let values = inner.captures.into_iter().enumerate().map(|(i, c)| (i as u32, c.source)).collect();
        Self::mk(hir::ExprKind::Record(values), env_ty, span)
    }

    /// `defer call(args)` or `defer { block }` (13.7): a closure created
    /// now, holding the values the cleanup uses, and called when the scope
    /// is left in any way, in LIFO order, or while a panic unwinds.
    pub(super) fn defer(&mut self, cx: &mut FnCx, deferred: &ast::Defer, span: Span) -> Option<hir::Stmt> {
        let closure = match deferred {
            ast::Defer::Block(block) => {
                // A cleanup gives nothing back: its final value is dropped (11.9).
                let unit = self.types.intern(TyKind::Fn(Vec::new(), Ty::UNIT, false, Vec::new(), false));
                self.closure_parts(cx, (&[], None, block), Some(unit), true, false, span)
            }
            ast::Defer::Call(call) => {
                if !matches!(call.kind, ast::ExprKind::Call { .. }) {
                    self.diags.error(
                        "E1410",
                        call.span,
                        "`defer` takes a call, such as `defer close(file)`, or a block",
                    );
                    return None;
                }
                let value = self.expr(cx, call, Some(Ty::UNIT));
                if value.ty != Ty::UNIT && !self.types.has_error(value.ty) {
                    let message = format!(
                        "a deferred call must return `Unit` and cannot fail, but this one returns {}; handle it in `defer {{ ... }}`",
                        self.display(value.ty)
                    );
                    self.diags.error("E1410", call.span, message);
                    return None;
                }
                self.deferred_call(value, span)?
            }
        };
        let local = self.hidden_local(cx, closure.ty);
        cx.mutable[local.0 as usize] = true;
        let read = Self::mk(hir::ExprKind::Local(local), closure.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), Ty::UNIT, span);
        Some(hir::Stmt::Defer { local, closure, call })
    }

    /// The closure of `defer call(args)`: its operands are evaluated now
    /// and the call is made at the end of the scope.
    pub(super) fn deferred_call(&mut self, call: hir::Expr, span: Span) -> Option<hir::Expr> {
        let args = match &call.kind {
            hir::ExprKind::Call(_, _, args) | hir::ExprKind::CallValue(_, args) => args,
            hir::ExprKind::Error => return None,
            _ => {
                self.diags.error("E1410", span, "this call cannot be deferred; write it in a `defer { ... }` block");
                return None;
            }
        };
        if args.iter().any(|a| matches!(a.kind, hir::ExprKind::Loan(_))) {
            self.diags.error("E1410", span, "a deferred call cannot borrow: loans end before the scope does");
            return None;
        }
        let name = format!("defer in {}", self.naming);
        Some(self.call_closure(call, &name, span))
    }

    /// A closure that makes `call` later with its callee and arguments,
    /// which are evaluated now and become its captures: the cleanup of
    /// `defer call(...)` and the body of `spawn call(...)`.
    pub(super) fn call_closure(&mut self, call: hir::Expr, name: &str, span: Span) -> hir::Expr {
        let result = call.ty;
        let (callee, values) = match call.kind {
            hir::ExprKind::Call(callee, type_args, args) => (Err((callee, type_args)), args),
            hir::ExprKind::CallValue(callee, args) => {
                let mut values = vec![*callee];
                values.extend(args);
                (Ok(()), values)
            }
            _ => unreachable!("only calls become closures"),
        };
        let generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let template = self.types.declare_template(name.into(), generics.clone());
        let fields = values
            .iter()
            .enumerate()
            .map(|(i, a)| Field { name: format!("argument{i}"), ty: a.ty, private: false, secret: false })
            .collect();
        self.types.define_template(template, AdtKind::Record(fields));
        let env_ty = self.types.instantiate(template, generics.clone());
        let index = self.closure_functions.len();
        let body_id = FnId(self.closure_base + index as u32);
        let info = ClosureInfo {
            body: body_id.0,
            params: Vec::new(),
            result,
            variadic: false,
            modes: Vec::new(),
            mutates: false,
            once: false,
            is_unsafe: false,
        };
        self.types.declare_closure(template, info);
        let env = Self::mk(hir::ExprKind::Local(LocalId(0)), env_ty, span);
        let mut fields: Vec<hir::Expr> = values
            .iter()
            .enumerate()
            .map(|(i, a)| Self::mk(hir::ExprKind::Field(Box::new(env.clone()), i as u32), a.ty, a.span))
            .collect();
        let invoke = match callee {
            Err((callee, type_args)) => hir::ExprKind::Call(callee, type_args, fields),
            Ok(()) => {
                let callee = fields.remove(0);
                hir::ExprKind::CallValue(Box::new(callee), fields)
            }
        };
        let invoke = Self::mk(invoke, result, span);
        let body = hir::Block { stmts: Vec::new(), tail: Some(Box::new(invoke)), ty: result };
        self.closure_functions.push(Some(hir::Function {
            name: name.into(),
            generics,
            params: vec![LocalId(0)],
            loans: vec![true],
            fallible: false,
            locals: vec![hir::Local { name: format!("<{name}>"), ty: env_ty, mutable: false, span }],
            result,
            body,
            moves: HashSet::new(),
            test: false,
            needs: 0,
            span,
        }));
        let values = values.into_iter().enumerate().map(|(i, a)| (i as u32, a)).collect();
        Self::mk(hir::ExprKind::Record(values), env_ty, span)
    }

    /// `spawn call(...)` as a statement: the task belongs to the block,
    /// which waits for it at its end.
    pub(super) fn spawn_statement(&mut self, cx: &mut FnCx, call: &ast::Expr) -> hir::Stmt {
        // In a loop the block is the iteration, so the tasks would run one
        // at a time: almost never what a loop that spawns wants.
        if !cx.loops.is_empty() {
            let message = "each iteration waits for this task at its end, so the tasks run one at a time; `spawn detached` lets them run at once, or add them to a `Group` and wait for all of them";
            self.diags.warning("W1013", call.span, message);
            let start = Span::new(call.span.start, call.span.start);
            self.diags.fix("launch it with `spawn detached`", false, vec![(start, "detached ".into())]);
        }
        let join = self.expr(cx, call, None);
        let local = self.hidden_local(cx, join.ty);
        hir::Stmt::Let(local, join)
    }

    /// `spawn call(arguments)` (11.11): the callee and arguments are
    /// evaluated now, by value, and a new task makes the call.
    pub(super) fn spawn(&mut self, cx: &mut FnCx, call: &ast::Expr, span: Span) -> hir::Expr {
        self.meta_denied(span, "`spawn`");
        // `std.model`'s own tasks share out its search, and are none of the
        // program's.
        let searching = self.module_paths.get(self.module as usize).is_some_and(|path| path == &["std", "model"]);
        match (self.measuring, self.calling) {
            _ if searching => {}
            (Some(test), _) => _ = self.test_spawns.insert(test),
            (None, Some(caller)) => _ = self.spawning.insert(caller),
            (None, None) => {}
        }
        let ast::ExprKind::Call { args, .. } = &call.kind else {
            let message =
                "`spawn` takes a call, such as `spawn fetch(url)`; put other work in a function and spawn its call";
            self.diags.error("E1611", call.span, message);
            return Self::error_expr(span);
        };
        if let Some(arg) = args.iter().find(|a| a.mode != ast::Mode::Value) {
            let message = format!(
                "a task cannot borrow: remove `{}` to move or copy the value into the task",
                arg.mode.keyword()
            );
            self.diags.error("E1609", arg.span, message);
            return Self::error_expr(span);
        }
        let before = self.error_count();
        let checked = self.expr(cx, call, None);
        // A method the compiler implements in place, as `lock.get()`, is no
        // call it can hand to a task: a closure around it is, made here and
        // called in the task, as the receiver and arguments are given now.
        fn spawnable(expr: &hir::Expr) -> bool {
            match &expr.kind {
                hir::ExprKind::Call(..) | hir::ExprKind::CallValue(..) | hir::ExprKind::Error => true,
                hir::ExprKind::Block(block) => block.tail.as_deref().is_some_and(spawnable),
                _ => false,
            }
        }
        let in_place = !spawnable(&checked);
        if in_place
            && self.error_count() == before
            && let ast::ExprKind::Call { callee, .. } = &call.kind
            && matches!(callee.kind, ast::ExprKind::Field(..) | ast::ExprKind::Path(_))
        {
            let body = ast::Block { stmts: Vec::new(), tail: Some(Box::new(call.clone())), span: call.span };
            let closure = ast::Closure { is_unsafe: false, params: Vec::new(), result: None, body };
            let wrapped = ast::Expr { kind: ast::ExprKind::Closure(Box::new(closure)), span: call.span };
            let closure = self.expr(cx, &wrapped, None);
            if self.types.closure_calls(closure.ty).is_some_and(|(mutates, _)| mutates) {
                let message = "a task cannot borrow: this call changes its receiver in place; make the task return the new value instead";
                self.diags.error("E1609", call.span, message);
                return Self::error_expr(span);
            }
            let called = Self::mk(hir::ExprKind::CallValue(Box::new(closure), Vec::new()), checked.ty, call.span);
            return self.spawn_call(called, span);
        }
        self.spawn_call(checked, span)
    }

    pub(super) fn spawn_call(&mut self, call: hir::Expr, span: Span) -> hir::Expr {
        match call.kind {
            // Named arguments are evaluated in order before the call.
            hir::ExprKind::Block(mut block) if block.tail.is_some() => {
                let tail = self.spawn_call(*block.tail.take().unwrap(), span);
                block.ty = tail.ty;
                let ty = tail.ty;
                block.tail = Some(Box::new(tail));
                Self::mk(hir::ExprKind::Block(block), ty, span)
            }
            hir::ExprKind::Call(_, _, ref args) | hir::ExprKind::CallValue(_, ref args) => {
                if args.iter().any(|a| matches!(a.kind, hir::ExprKind::Loan(_) | hir::ExprKind::Region(..))) {
                    let message = "a task cannot borrow: this call lends a place with `mut` or `var`; make the task return the new value instead";
                    self.diags.error("E1609", span, message);
                    return Self::error_expr(span);
                }
                // What enters the task, and its result that comes back, cross
                // between tasks (16.13).
                let crossing = match &call.kind {
                    hir::ExprKind::CallValue(callee, args) => std::iter::once(&**callee).chain(args).collect(),
                    _ => args.iter().collect::<Vec<_>>(),
                };
                for value in crossing {
                    if let Some(part) = self.not_send(value.ty) {
                        let message = format!(
                            "`spawn` moves this value into another task, but it holds {}, which is not `Send`: it stays in the task that has it",
                            self.display(part)
                        );
                        self.diags.error("E1605", value.span, message);
                        return Self::error_expr(span);
                    }
                }
                if let Some(part) = self.not_send(call.ty) {
                    let message = format!(
                        "the task's result goes back to its `Join`, but it holds {}, which is not `Send`",
                        self.display(part)
                    );
                    self.diags.error("E1605", span, message);
                    return Self::error_expr(span);
                }
                let result = call.ty;
                // A trace names the task by the function it runs.
                let name = match &call.kind {
                    hir::ExprKind::Call(hir::Callee::Fn(id), ..) if (id.0 as usize) < self.signatures.len() => {
                        format!("spawn {}", self.signatures[id.0 as usize].name)
                    }
                    _ => "spawn".to_string(),
                };
                let closure = self.call_closure(call, &name, span);
                let (body, ..) = self.types.closure(closure.ty).expect("a call closure");
                let erased = self.types.intern(TyKind::Fn(Vec::new(), result, false, Vec::new(), false));
                let erased = Self::mk(hir::ExprKind::Erase(Box::new(closure), FnId(body)), erased, span);
                let join = self.types.handle(Handle::Join, result);
                Self::mk(hir::ExprKind::Spawn(Box::new(erased)), join, span)
            }
            hir::ExprKind::Error => Self::error_expr(span),
            _ => {
                let message = "`spawn` takes a call of a function, method or closure; put this work in a function and spawn its call";
                self.diags.error("E1611", span, message);
                Self::error_expr(span)
            }
        }
    }
}
