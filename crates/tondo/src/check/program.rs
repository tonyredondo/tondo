//! The program as a whole: tests, fuzzing, benchmarks, snapshots, `recover`, `within`, panics.

use super::reach::Reach;
use super::*;

impl<'a> Checker<'a> {
    pub(super) fn program(&mut self, module: &'a ast::Module) -> Option<hir::Program> {
        self.prelude = module.module_paths.iter().position(|p| p == &["std", "prelude"]).map(|i| i as u32);
        self.module_paths = module.module_paths.clone();
        self.built = module.built.clone();
        self.elsewhere = module.elsewhere.clone();
        self.system_heads(module);
        self.datetime_module = module.module_paths.iter().position(|p| p == &["std", "datetime"]).map(|i| i as u32);
        self.pacing_module = module.module_paths.iter().position(|p| p == &["std", "pacing"]).map(|i| i as u32);
        for import in &module.imports {
            self.import(import);
        }
        // The network, through a module written in Tondo (27.7); `std.http`
        // brings `std.net`, so it is named first.
        let network = ["http", "net"].into_iter().find(|name| module.module_paths.iter().any(|p| p == &["std", name]));
        if let Some(name) = network.filter(|_| self.meta) {
            let message = format!(
                "`std.{name}` is not available to a provider or generator, which runs while the program compiles and works only on its request"
            );
            self.diags.error_without_span("E2108", message);
        }

        self.declare_types(&module.items);
        // The prelude's `trait Iterator[T]` adds methods to the intrinsic one.
        let (extensions, traits): (Vec<&ast::TraitDecl>, Vec<&ast::TraitDecl>) = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Trait(t) => Some(t),
                _ => None,
            })
            .partition(|t| Some(t.module) == self.prelude && t.name.name.rsplit('.').next() == Some("Iterator"));
        for extension in &extensions {
            // Its module names it by its own path.
            self.trait_names.insert(extension.name.name.clone(), ITERATOR);
        }
        self.declare_traits(&traits);
        for item in &module.items {
            if let ast::Item::Const(decl) = item {
                let key = Some(crate::index::Key::Value(decl.name.name.clone()));
                self.index_define(key, crate::index::DefKind::Const, &decl.name, decl.span, None);
                self.consts.push(ConstEntry { decl, value: None, resolving: false });
                self.declare_module_name(&decl.name, ModuleName::Const(self.consts.len() - 1));
            }
        }
        let (externs, mut fns): (Vec<&ast::FnDecl>, Vec<&ast::FnDecl>) = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Fn(f) => Some(f),
                _ => None,
            })
            .partition(|f| f.foreign.is_some() && f.body.is_none());
        // A fixture's operations come last, after the toolchain's own.
        fns.sort_by_key(|f| self.fixture(f.span));
        for f in externs {
            self.declare_extern(f);
        }
        // Every function with a body, in `FnId` order.
        let mut bodies: Vec<&ast::FnDecl> = Vec::new();
        for f in &fns {
            let id = FnId(bodies.len() as u32);
            // What reads the clock or the system's entropy (27.7).
            let bare = |name: &str| name.rsplit('.').next().unwrap_or_default().to_string();
            let qualified = (
                module.module_paths[f.module as usize].join("."),
                f.owner.as_ref().map(|o| bare(&o.name)),
                bare(&f.name.name),
            );
            if matches!(&qualified, (m, None, n) if m == "std.model" && matches!(n.as_str(), "check" | "machine" | "follows" | "watch"))
            {
                self.checks_models.insert(id);
            }
            let unrepeatable = match qualified {
                (m, Some(o), n) if m == "std.uuid" && o == "Uuid" && n == "v4" => Some("`Uuid.v4`"),
                (m, Some(o), n) if m == "std.uuid" && o == "Uuid" && n == "v7" => Some("`Uuid.v7`"),
                (m, None, n) if m == "std.random" && n == "Random" => Some("`Random()`"),
                (m, None, n) if m == "std.random" && n == "entropy" => Some("`random.entropy`"),
                _ => None,
            };
            if let Some(name) = unrepeatable {
                self.unrepeatable.insert(id, name);
            }
            let owner = f.owner.as_ref().map(|owner| self.method_owner(owner, f.module));
            let signature = self.signature(f, owner, FnScope::default());
            let receiver = signature.receiver;
            self.signatures.push(signature);
            if f.foreign.is_some() {
                self.declare_export(f, id, &module.module_paths);
            }
            bodies.push(f);
            match owner {
                Some(owner) => self.declare_method(f, owner, receiver.is_some(), id),
                None => {
                    if f.public {
                        self.public_fns.insert(id);
                    }
                    self.declare_module_name(&f.name, ModuleName::Fn(id))
                }
            }
        }
        self.trait_methods(&traits, &mut bodies);
        for extension in extensions {
            self.iterator_defaults(extension, &mut bodies);
        }
        for item in &module.items {
            if let ast::Item::Impl(decl) = item {
                self.implementation(decl, &mut bodies);
            }
        }
        self.index_functions(&bodies);
        for (index, f) in fns.iter().enumerate() {
            self.defaults(f, FnId(index as u32));
        }
        self.check_required();
        self.check_termination();
        // Every constant is checked, used or not.
        for id in 0..self.consts.len() {
            let span = self.consts[id].decl.span;
            self.const_value(id, span);
        }
        self.annotation_declarations();
        self.record_defaults();
        self.annotation_uses(&module.items, &bodies);

        for item in &module.items {
            if let ast::Item::Test(node) = item
                && Self::test_class(node) == ast::SourceClass::Production
            {
                let message = format!(
                    "a {} belongs in a test file: move it to a `_test.to` file next to this code, or to a file under `tests/`",
                    if matches!(node, ast::TestNode::Suite(_)) { "`suite`" } else { "`test`" }
                );
                self.diags.error("E2001", node.span(), message);
            }
        }

        // Closure bodies are numbered after every declared function, the
        // script's `main` and the root of a test build's tree.
        let testing = module.test_package.is_some();
        for statement in &module.statements {
            if let ast::StmtKind::Let { pattern, .. } = &statement.kind {
                let mut names = Vec::new();
                bound_idents(pattern, &mut names);
                self.script_bindings.extend(names.into_iter().map(|name| name.name.clone()));
            }
        }
        self.closure_base = bodies.len() as u32 + u32::from(!module.statements.is_empty()) + u32::from(testing);
        // The program's bodies, then those of the standard library they reach.
        let mut reach = Reach::new(bodies.len());
        for (f, i) in bodies.iter().zip(0..) {
            if !self.reached_later(f) {
                reach.add(FnId(i));
            }
        }
        self.check_reached(&bodies, &mut reach);
        for (id, span) in std::mem::take(&mut self.dropped_calls) {
            if self.only_computes(&bodies, id, &mut HashSet::new()) {
                self.result_unused(span);
            }
        }

        let explicit_main = match self.names.get("main") {
            Some(ModuleName::Fn(id)) => Some(*id),
            _ => None,
        };
        // A script's `main` and a test build's root, after the declared functions.
        let mut made = Vec::new();
        let main = if !module.statements.is_empty() {
            if let Some(id) = explicit_main {
                let span = fns[id.0 as usize].name.span;
                self.diags.error("E1802", span, "a script with top-level statements cannot also declare `main`");
            }
            made.push(self.script_main(&module.statements));
            Some(FnId((bodies.len() + made.len()) as u32 - 1))
        } else if let Some(id) = explicit_main {
            self.validate_main(fns[id.0 as usize], id);
            Some(id)
        } else if testing {
            Some(FnId(0))
        } else if module.library {
            // A library runs what C calls.
            None
        } else {
            let message = "nothing to run: the program has no `main` and no top-level statements; add `fn main() { ... }`, \
                or statements at the top of the file; a library for C builds with `--library`";
            self.diags.error_without_span("E1806", message);
            return None;
        };
        // A test build runs its tests instead of `main`.
        let main = match &module.test_package {
            Some(package) => {
                made.push(self.tests(module, package));
                Some(FnId((bodies.len() + made.len()) as u32 - 1))
            }
            None => main,
        };
        for function in &made {
            self.reach_from(function, &mut reach);
        }
        // A failing `main` shows its error with `Display` (20.10).
        let result = main.and_then(|main| match reach.functions.get(main.0 as usize) {
            Some(function) => function.as_ref().map(|f| f.result),
            None => made.last().map(|f| f.result),
        });
        if let Some(result) = result
            && let TyKind::Result(_, error) = *self.types.kind(result)
        {
            self.reach_display(error, &mut reach);
        }
        self.check_reached(&bodies, &mut reach);
        if testing {
            self.mark_models();
        }
        self.check_model_names();
        let makers = self.context_makers();
        for context in &makers {
            let initials = context.initials.iter().map(|initial| &initial.function);
            for function in [&context.maker].into_iter().chain(&context.log).chain(&context.label).chain(initials) {
                self.reach_from(function, &mut reach);
            }
            // What fails to compute an initial value is shown in its panic.
            for error in context.initials.iter().filter_map(|initial| initial.error) {
                self.reach_display(error, &mut reach);
            }
        }
        self.check_reached(&bodies, &mut reach);
        let mut unchecked = HashSet::new();
        let mut functions: Vec<hir::Function> = Vec::with_capacity(bodies.len());
        for ((function, f), i) in reach.functions.into_iter().zip(&bodies).zip(0..) {
            functions.push(function.unwrap_or_else(|| {
                unchecked.insert(FnId(i));
                let signature = &self.signatures[i as usize];
                hir::Function {
                    name: self.traced_name(f, FnId(i)),
                    generics: signature.generics.clone(),
                    params: Vec::new(),
                    loans: Vec::new(),
                    fallible: false,
                    locals: Vec::new(),
                    result: signature.result,
                    body: hir::Block { stmts: Vec::new(), tail: None, ty: Ty::UNIT },
                    moves: HashSet::new(),
                    test: false,
                    span: f.name.span,
                }
            }));
        }
        functions.extend(made);
        functions.extend(std::mem::take(&mut self.closure_functions).into_iter().map(|f| f.expect("checked closure")));
        let mut contexts = Vec::new();
        for ContextMade { maker, log, label, initials } in makers {
            let mut add = |function: hir::Function| {
                functions.push(function);
                FnId(functions.len() as u32 - 1)
            };
            let maker = add(maker);
            let (log, label) = (log.map(&mut add), label.map(&mut add));
            let mut computed = Vec::new();
            for initial in initials {
                let (field, reference, named) = (initial.field, initial.function.result, initial.named.clone());
                let computes = add(initial.function);
                let id = match initial.error {
                    None => computes,
                    Some(error) => {
                        let TyKind::Result(reference, _) = *self.types.kind(reference) else { unreachable!() };
                        add(self.initial_failing(computes, reference, error, &named))
                    }
                };
                computed.push((field, id, named));
            }
            contexts.push(hir::ContextFns { maker, log, label, computed });
        }

        if module.c_library && self.c_functions.is_empty() {
            let message = "the library publishes no function: C calls a `pub extern fn name(...) { ... }` of the package by its name";
            self.diags.error_without_span("E1807", message);
        }
        if self.diags.has_errors() {
            return None;
        }
        let traits = self
            .traits
            .iter()
            .enumerate()
            .map(|(id, t)| hir::TraitInfo {
                name: t.name.clone(),
                defaults: t.methods.iter().map(|m| m.default).collect(),
                param_count: t.params.len(),
                methods: t.methods.iter().map(|m| m.name.clone()).collect(),
                arithmetic: [self.additive, self.number, self.integer].contains(&Some(TraitId(id as u32))),
            })
            .collect();
        let impls = self
            .impls
            .iter()
            .map(|i| hir::ImplInfo {
                intrinsic: i.intrinsic,
                trait_id: i.trait_id,
                trait_args: i.trait_args.clone(),
                target: i.target,
                generics: i.generics.clone(),
                methods: i.methods.clone(),
            })
            .collect();
        // Before the program takes the types.
        self.index_finish();
        let c_api = hir::CApi { structs: self.c_structs(), functions: std::mem::take(&mut self.c_functions) };
        Some(hir::Program {
            views: HashMap::new(),
            types: std::mem::take(&mut self.types),
            functions,
            main,
            traits,
            impls,
            display: HashMap::new(),
            externs: std::mem::take(&mut self.externs),
            exports: std::mem::take(&mut self.exports),
            c_api,
            tests: std::mem::take(&mut self.test_nodes),
            contexts,
            field_defaults: self.exported_defaults(),
            unchecked,
        })
    }

    /// The field defaults of the program's records, for live builds to
    /// give a new field of a value alive.
    pub(super) fn exported_defaults(&mut self) -> hir::FieldDefaults {
        let mut exported = hir::FieldDefaults::default();
        for (key, defaults) in std::mem::take(&mut self.field_defaults) {
            match key {
                Defaulted::Record(ty) => exported.records.insert(ty, defaults),
                Defaulted::Template(template) => exported.templates.insert(template, defaults),
            };
        }
        exported
    }

    pub(super) fn test_class(node: &ast::TestNode) -> ast::SourceClass {
        match node {
            ast::TestNode::Test(t) => t.class,
            ast::TestNode::Suite(s) => s.class,
        }
    }

    /// The root of a test build's tree (testing 3.2): a function that runs
    /// the top-level suites and tests of the test files.
    pub(super) fn tests(&mut self, module: &ast::Module, package: &str) -> hir::Function {
        let root = hir::TestNodeInfo {
            id: String::new(),
            parent: 0,
            suite: true,
            bench: false,
            fuzz: false,
            model: false,
            tasks: false,
            span: Span::new(0, 0),
            whole: Span::new(0, 0),
        };
        self.test_nodes.push(root);
        let mut nodes = Vec::new();
        let mut prefixes = Vec::new();
        for item in &module.items {
            let ast::Item::Test(node) = item else { continue };
            let (module_index, class) = match node {
                ast::TestNode::Test(t) => (t.module, t.class),
                ast::TestNode::Suite(s) => (s.module, s.class),
            };
            // `package::unit::module.path` or `package::integration::file.path`.
            let path = &module.module_paths[module_index as usize];
            let (kind, logical) = match class {
                ast::SourceClass::Production => continue,
                ast::SourceClass::Unit if path.len() == 1 => ("unit", path[0].clone()),
                ast::SourceClass::Unit => ("unit", path[1..].join(".")),
                ast::SourceClass::Integration => ("integration", path[1..].join(".")),
            };
            nodes.push(node);
            prefixes.push(format!("{package}::{kind}::{logical}"));
        }
        let mut cx = self.new_cx(Ty::UNIT);
        let run = self.test_members(&mut cx, &nodes, 0, &prefixes, Span::new(0, 0));
        let body = hir::Block { stmts: vec![hir::Stmt::Expr(run)], tail: None, ty: Ty::UNIT };
        let mut function = hir::Function {
            name: "tests".into(),
            generics: Vec::new(),
            params: Vec::new(),
            loans: Vec::new(),
            fallible: false,
            locals: cx.locals,
            result: Ty::UNIT,
            body,
            moves: HashSet::new(),
            test: false,
            span: Span::default(),
        };
        self.ownership(&mut function, &cx.params);
        function
    }

    /// `testing.withVirtualTime((clock) { ... })` (testing 7.9): the closure
    /// runs on a new virtual clock, which ends as it returns; it infers its
    /// errors, which the call passes on.
    pub(super) fn with_virtual_time(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let [arg] = args else {
            return self.arity_error(span, 1, args.len());
        };
        let ast::ExprKind::Closure(closure) = &arg.value.kind else {
            let message = "`withVirtualTime` takes a closure: `testing.withVirtualTime((clock) { ... })`";
            self.diags.error("E1102", arg.value.span, message);
            return Self::error_expr(span);
        };
        let clock = self.types.handle(Handle::Clock, Ty::UNIT);
        let expected = self.types.intern(TyKind::Fn(vec![clock], Ty::UNIT, false, vec![ast::Mode::Value], false));
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let body = &closure.body;
        let value =
            self.closure_with(cx, parts, Some(expected), kind, span, |this, cx, success| this.block(cx, body, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        // `let clock = begin(); defer end(); body(clock)`
        let local = |this: &mut Self, cx: &mut FnCx, ty: Ty| {
            let local = this.hidden_local(cx, ty);
            cx.mutable[local.0 as usize] = true;
            local
        };
        let clock_local = local(self, cx, clock);
        let begin = Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::ClockBegin), Vec::new(), Vec::new()), clock, span);
        let end = Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::ClockEnd), Vec::new(), Vec::new()), Ty::UNIT, span);
        let Some(cleanup) = self.deferred_call(end, span) else { return Self::error_expr(span) };
        let cleanup_local = local(self, cx, cleanup.ty);
        let read = Self::mk(hir::ExprKind::Local(cleanup_local), cleanup.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), Ty::UNIT, span);
        let body_ty = value.ty;
        let body_local = local(self, cx, body_ty);
        let stmts = vec![
            hir::Stmt::Let(clock_local, begin),
            hir::Stmt::Defer { local: cleanup_local, closure: cleanup, call },
            hir::Stmt::Let(body_local, value),
        ];
        let callee = Self::mk(hir::ExprKind::Local(body_local), body_ty, span);
        let argument = Self::mk(hir::ExprKind::Local(clock_local), clock, span);
        let run = Self::mk(hir::ExprKind::CallValue(Box::new(callee), vec![argument]), info.result, span);
        // Always a result, so that `?` follows the call as the canonical
        // form writes it; one that cannot fail fails with `Never`.
        let (run, ty) = match self.types.kind(info.result) {
            TyKind::Result(..) => (run, info.result),
            _ => {
                let ty = self.types.intern(TyKind::Result(info.result, Ty::NEVER));
                (Self::mk(hir::ExprKind::Variant(OK, vec![(0, run)]), ty, span), ty)
            }
        };
        let block = hir::Block { stmts, tail: Some(Box::new(run)), ty };
        Self::mk(hir::ExprKind::Block(block), ty, span)
    }

    /// `testing.fuzz(name, seeds, (input: T) { ... })` (testing 10.6), in a
    /// test's own body, which makes the test one that fuzzes: a call of the
    /// loop the standard library writes for it, `fuzz.run`, with the input's
    /// type, the body's error, `Never` if it does not fail, and the body's.
    pub(super) fn fuzz(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        const EXAMPLE: &str = "testing.fuzz(\"parse\", [\"[1]\"], (text: String) { _ = json.parse(text) })";
        match self.measuring {
            Some(test) if cx.test_member => self.test_nodes[test as usize].fuzz = true,
            _ => {
                let message = format!(
                    "`testing.fuzz` tries inputs in the body of a `test`, which it makes a fuzz test: call it there, \
                     as `test parsesAnything {{ {EXAMPLE} }}`, not in a closure, a function or a suite's setup"
                );
                self.diags.error("E2007", span, message);
            }
        }
        let [name, seeds, body] = args else {
            return self.arity_error(span, 3, args.len());
        };
        let ast::ExprKind::Closure(closure) = &body.value.kind else {
            let message = format!("`fuzz` takes a name, the seeds and a closure that tries an input: `{EXAMPLE}`");
            self.diags.error("E1102", body.value.span, message);
            return Self::error_expr(span);
        };
        let name = self.expr(cx, &name.value, Some(Ty::STRING));
        let name = self.coerce(name, Ty::STRING);
        // The input's type: the closure's parameter's, or the seeds'.
        let written = closure.params.first().and_then(|p| p.ty.as_ref()).map(|t| self.resolve_type(t));
        let seeds_ty = written.map(|t| self.types.intern(TyKind::Array(t)));
        let seeds = self.expr(cx, &seeds.value, seeds_ty);
        let seeds = match seeds_ty {
            Some(ty) => self.coerce(seeds, ty),
            None => seeds,
        };
        let input = match *self.types.kind(seeds.ty) {
            TyKind::Array(input) if !self.types.has_params(input) => input,
            TyKind::Error => return Self::error_expr(span),
            _ => {
                let message = "the seeds of `testing.fuzz` are an array of inputs; with none, write the input's \
                               type on the closure's parameter: `testing.fuzz(\"parse\", [], (text: String) { ... })`";
                self.diags.error("E1101", seeds.span, message);
                return Self::error_expr(span);
            }
        };
        // Inputs are written and read as JSON.
        for path in ["serialization.Encode", "serialization.Decode"] {
            let Some(&id) = self.trait_names.get(path) else { continue };
            if input != Ty::ERROR && !self.satisfies(input, id, &[]) {
                let message = format!(
                    "{} does not implement `{path}`, which the input of `testing.fuzz` needs, as it is written and \
                     read as JSON; the compiler implements it for records, enums and newtypes whose parts all \
                     implement it",
                    self.display(input)
                );
                self.diags.error("E1105", seeds.span, message);
                return Self::error_expr(span);
            }
        }
        let expected = self.types.intern(TyKind::Fn(vec![input], Ty::UNIT, false, Vec::new(), false));
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let block = &closure.body;
        let value = self
            .closure_with(cx, parts, Some(expected), kind, span, |this, cx, success| this.block(cx, block, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        let error = match *self.types.kind(info.result) {
            TyKind::Result(_, error) => error,
            _ => Ty::NEVER,
        };
        let Some(&ModuleName::Fn(run)) = self.names.get("fuzz.run") else {
            unreachable!("std.testing brings the loop of `testing.fuzz`")
        };
        let ty = self.types.intern(TyKind::Result(Ty::UNIT, error));
        let mut defs = Vec::new();
        let root = self.fuzz_shape(input, &mut defs, &mut HashMap::new());
        let shape = Self::mk(
            hir::ExprKind::Str(format!("{{\"root\":{root},\"defs\":[{}]}}", defs.join(","))),
            Ty::STRING,
            span,
        );
        let types = vec![input, error, value.ty];
        Self::mk(hir::ExprKind::Call(Callee::Fn(run), types, vec![name, shape, seeds, value]), ty, span)
    }

    /// The shape of a record, or of a variant's fields, `key` naming their
    /// keys in `wires`: each written field's key and shape.
    pub(super) fn fuzz_record(
        &mut self,
        key: &str,
        fields: &[Field],
        defs: &mut Vec<String>,
        named: &mut HashMap<Ty, usize>,
    ) -> String {
        let wires = self.wires.get(key).cloned().unwrap_or_default();
        let mut members = Vec::new();
        for (at, field) in fields.iter().enumerate() {
            let wire = match wires.get(at) {
                Some(None) => continue,
                Some(Some(wire)) => wire.clone(),
                None => field.name.clone(),
            };
            let shape = self.fuzz_shape(field.ty, defs, named);
            members.push(format!("[{},{shape}]", serde_json::Value::String(wire)));
        }
        format!("{{\"record\":[{}]}}", members.join(","))
    }

    /// What `json.encode` writes for a value of `ty`, as the fuzzer reads it
    /// (testing 10.6), in JSON: `"int"`, `"byte"`, `"float"`, `"bool"`,
    /// `"text"`, `"bytes"` and `"datetime"`; `{"option": s}`, `{"list": s}` and
    /// `{"map": s}`; and for a record, enum or newtype, `{"ref": n}`, its
    /// index in `defs`, whose entry is `{"record": [[key, s]...]}`,
    /// `{"enum": [[name, payload]...]}` or its value's shape, so that one
    /// that holds itself ends. What the fuzzer cannot tell is `"any"`.
    pub(super) fn fuzz_shape(&mut self, ty: Ty, defs: &mut Vec<String>, named: &mut HashMap<Ty, usize>) -> String {
        let quoted = |text: &str| serde_json::Value::String(text.to_string()).to_string();
        match self.types.kind(ty).clone() {
            _ if ty == Ty::INT => "\"int\"".into(),
            _ if ty == Ty::BYTE => "\"byte\"".into(),
            _ if ty == Ty::F64 => "\"float\"".into(),
            _ if ty == Ty::BOOL => "\"bool\"".into(),
            _ if ty == Ty::STRING => "\"text\"".into(),
            _ if Some(ty) == self.datetime => "\"datetime\"".into(),
            TyKind::Option(inner) => format!("{{\"option\":{}}}", self.fuzz_shape(inner, defs, named)),
            TyKind::Array(inner) => format!("{{\"list\":{}}}", self.fuzz_shape(inner, defs, named)),
            TyKind::Map(key, value) if key == Ty::STRING => {
                format!("{{\"map\":{}}}", self.fuzz_shape(value, defs, named))
            }
            TyKind::Adt(id) => {
                if let Some(&index) = named.get(&ty) {
                    return format!("{{\"ref\":{index}}}");
                }
                let (kind, instance, own) = {
                    let adt = self.types.adt(id);
                    (adt.kind.clone(), adt.instance.clone(), adt.name.clone())
                };
                let name = match &instance {
                    Some((template, _)) => self.types.template(*template).name.clone(),
                    None => own,
                };
                if name == "serialization.Bytes" {
                    return "\"bytes\"".into();
                }
                let index = defs.len();
                named.insert(ty, index);
                defs.push(String::new());
                let def = match &kind {
                    AdtKind::Newtype(inner) => self.fuzz_shape(*inner, defs, named),
                    AdtKind::Record(fields) => self.fuzz_record(&name, fields, defs, named),
                    AdtKind::Enum(variants) => {
                        let mut shown = Vec::new();
                        for variant in variants {
                            let payload = match &variant.payload {
                                Payload::Unit => "null".to_string(),
                                Payload::Tuple(items) if items.len() == 1 => self.fuzz_shape(items[0], defs, named),
                                Payload::Tuple(items) => {
                                    let items: Vec<String> =
                                        items.iter().map(|&t| self.fuzz_shape(t, defs, named)).collect();
                                    format!("{{\"tuple\":[{}]}}", items.join(","))
                                }
                                Payload::Record(fields) => {
                                    self.fuzz_record(&format!("{name}.{}", variant.name), fields, defs, named)
                                }
                            };
                            shown.push(format!("[{},{payload}]", quoted(&variant.name)));
                        }
                        format!("{{\"enum\":[{}]}}", shown.join(","))
                    }
                };
                defs[index] = def;
                format!("{{\"ref\":{index}}}")
            }
            _ => "\"any\"".into(),
        }
    }

    /// `panic(message, code:, data:)` (15.7): the message, and the
    /// program's own code and data, which the standard library hands the
    /// runtime just before the panic.
    pub(super) fn panic_call(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let mut given: [Option<&ast::Expr>; 3] = [None; 3];
        for (index, arg) in args.iter().enumerate() {
            let slot = match (arg.name.as_ref().map(|n| n.name.as_str()), index) {
                (Some("message"), _) | (None, 0) => 0,
                (Some("code"), _) | (None, 1) => 1,
                (Some("data"), _) | (None, 2) => 2,
                (Some(other), _) => {
                    let message = format!("`panic` takes `message`, `code` and `data`, not `{other}`");
                    self.diags.error("E1102", arg.value.span, message);
                    continue;
                }
                (None, _) => return self.arity_error(span, 3, args.len()),
            };
            given[slot] = Some(&arg.value);
        }
        let [Some(message), code, data] = given else {
            return self.arity_error(span, 1, 0);
        };
        let message = self.expr(cx, message, Some(Ty::STRING));
        let message = self.coerce(message, Ty::STRING);
        let panic = Self::mk(hir::ExprKind::Panic(Box::new(message), 8), Ty::NEVER, span);
        if code.is_none() && data.is_none() {
            return panic;
        }
        let mut stmts = Vec::new();
        if let Some(code) = code {
            if let ast::ExprKind::Str(segments) = &code.kind
                && let [ast::StrSegment::Text(text)] = segments.as_slice()
                && !own_panic_code(text)
            {
                let message = format!(
                    "`{text}` is not a code of the program's own: write segments of a lowercase letter followed by letters and digits, separated by `.`, as `cache.corrupt`; codes `P` with digits are the language's"
                );
                self.diags.error("E1305", code.span, message);
            }
            let code = self.expr(cx, code, Some(Ty::STRING));
            let code = self.coerce(code, Ty::STRING);
            stmts.push(hir::Stmt::Expr(self.panics_call("panics.stageCode", code, span)));
        }
        if let Some(data) = data {
            let datum = self.types.union(&[Ty::INT, Ty::F64, Ty::BOOL, Ty::STRING]);
            let map = self.map_type(Ty::STRING, datum);
            let outer = self.panic_datum.replace(datum);
            let data = self.expr(cx, data, Some(map));
            let data = self.coerce(data, map);
            self.panic_datum = outer;
            stmts.push(hir::Stmt::Expr(self.panics_call("panics.stageData", data, span)));
        }
        let block = hir::Block { stmts, tail: Some(Box::new(panic)), ty: Ty::NEVER };
        Self::mk(hir::ExprKind::Block(block), Ty::NEVER, span)
    }

    /// `recover { ... }` (15.9): the block's value, or the panic that
    /// stopped it, as `T ! Panic`.
    pub(super) fn recover(&mut self, cx: &mut FnCx, block: &ast::Block, expected: Option<Ty>, span: Span) -> hir::Expr {
        let Some(panic) = self.type_named("Panic") else { unreachable!("the prelude names `Panic`") };
        let inner = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Result(ok, error)) if *error == panic => Some(*ok),
            _ => None,
        };
        let block = self.block(cx, block, inner);
        let ty = self.types.intern(TyKind::Result(block.ty, panic));
        let Some(&ModuleName::Fn(caught)) = self.names.get("panics.caught") else {
            unreachable!("the prelude brings `std.panics`")
        };
        let caught = Self::mk(hir::ExprKind::Call(Callee::Fn(caught), Vec::new(), Vec::new()), panic, span);
        Self::mk(hir::ExprKind::Recover(block, Box::new(caught)), ty, span)
    }

    /// `within limit { ... }` (11.17): the block's value, as a result whose
    /// error also takes `time.TimedOut`, which it is when the deadline
    /// passes first. The limit is evaluated first, into a binding that the
    /// error reads.
    pub(super) fn within(&mut self, cx: &mut FnCx, limit: &ast::Expr, block: &ast::Block, span: Span) -> hir::Expr {
        let (Some(duration), Some(timed_out)) = (self.duration, self.timed_out()) else {
            let message =
                "`within` takes a `time.Duration`: `import std.time`, and write `within time.seconds(2) { ... }`";
            self.diags.error("E1001", limit.span, message);
            self.block(cx, block, None);
            return Self::error_expr(span);
        };
        let limit = self.expr(cx, limit, Some(duration));
        let limit = self.coerce(limit, duration);
        let held = self.hidden_local(cx, duration);
        let block = self.block(cx, block, None);
        let (ty, error) = match *self.types.kind(block.ty) {
            TyKind::Result(ok, error) => {
                let error = self.types.union(&[error, timed_out]);
                (self.types.intern(TyKind::Result(ok, error)), error)
            }
            _ => (self.types.intern(TyKind::Result(block.ty, timed_out)), timed_out),
        };
        let read = Self::mk(hir::ExprKind::Local(held), duration, span);
        let failure = Self::mk(hir::ExprKind::Record(vec![(0, read.clone())]), timed_out, span);
        let failure = self.coerce(failure, error);
        let within = Self::mk(hir::ExprKind::Within(Box::new(read), block, Box::new(failure)), ty, span);
        let stmts = vec![hir::Stmt::Let(held, limit)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(within)), ty }), ty, span)
    }

    /// `time.TimedOut`, which `std.time` writes in Tondo (11.17).
    /// The modules written in Tondo with the rest of a module the compiler
    /// implements (`crate::stdlib::COMPANIONS`).
    pub(super) fn companions(module: StdModule) -> &'static [&'static str] {
        crate::stdlib::companions(match module {
            StdModule::Time => "time",
            StdModule::Fs => "fs",
            StdModule::Path => "path",
            StdModule::Io => "io",
            StdModule::Text => "text",
            StdModule::Env => "env",
            StdModule::Process => "process",
            StdModule::Sync => "sync",
            _ => return &[],
        })
    }

    /// A `pub` function of a module's companion, which is the module's own:
    /// `time.parseDateTime` for `datetime.parseDateTime`.
    pub(super) fn companion_fn(&self, module: StdModule, member: &str) -> Option<FnId> {
        Self::companions(module).iter().find_map(|companion| match self.names.get(&format!("{companion}.{member}")) {
            Some(&ModuleName::Fn(id)) if self.public_fns.contains(&id) => Some(id),
            _ => None,
        })
    }

    pub(super) fn timed_out(&self) -> Option<Ty> {
        match self.type_names.get("datetime.TimedOut") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A call of a function of the standard library's `std.panics`.
    pub(super) fn panics_call(&mut self, name: &str, value: hir::Expr, span: Span) -> hir::Expr {
        let Some(&ModuleName::Fn(id)) = self.names.get(name) else { unreachable!("the prelude brings `std.panics`") };
        Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![value]), Ty::UNIT, span)
    }

    /// `testing.bench(name, () { ... })` (testing 10.4), in a test's own
    /// body, which makes the test a benchmark: runs the body as many times
    /// as the runtime asks, keeping what it gives, until it asks for none
    /// or the body fails; the call fails as the body does.
    pub(super) fn bench(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let [name, body] = args else {
            return self.arity_error(span, 2, args.len());
        };
        let ast::ExprKind::Closure(closure) = &body.value.kind else {
            let message = "`bench` takes a name and a closure: `testing.bench(\"parse\", () { json.parse(text) })?`";
            self.diags.error("E1102", body.value.span, message);
            return Self::error_expr(span);
        };
        match self.measuring {
            Some(test) if cx.test_member => self.test_nodes[test as usize].bench = true,
            _ => {
                let message = "`testing.bench` measures in the body of a `test`, which it makes a benchmark: call it \
                               there, as `test parsing { testing.bench(\"parse\", () { json.parse(text) })? }`, not \
                               in a closure, a function or a suite's setup";
                self.diags.error("E2006", span, message);
            }
        }
        let name = self.expr(cx, &name.value, Some(Ty::STRING));
        let name = self.coerce(name, Ty::STRING);
        let kind = ClosureKind { deferred: false, is_unsafe: closure.is_unsafe, test: false, infer_error: true };
        let parts = (&closure.params[..], closure.result.as_ref());
        let block = &closure.body;
        let value = self.closure_with(cx, parts, None, kind, span, |this, cx, success| this.block(cx, block, success));
        let Some((_, _, info)) = self.types.closure(value.ty) else { return Self::error_expr(span) };
        let (success, error) = match *self.types.kind(info.result) {
            TyKind::Result(success, error) => (success, Some(error)),
            _ => (info.result, None),
        };
        let mk = |kind, ty| Self::mk(kind, ty, span);
        let std = |function, values, ty| mk(hir::ExprKind::Call(Callee::Std(function), Vec::new(), values), ty);
        let local = |this: &mut Self, cx: &mut FnCx, ty: Ty| {
            let local = this.hidden_local(cx, ty);
            cx.mutable[local.0 as usize] = true;
            local
        };
        let read = |local: LocalId, ty: Ty| mk(hir::ExprKind::Local(local), ty);
        let set = |local: LocalId, op: Option<BinOp>, value: hir::Expr| hir::Stmt::Assign {
            local,
            path: Vec::new(),
            op,
            value,
            span,
        };
        let int = |n: i128| mk(hir::ExprKind::Int(n), Ty::INT);
        let ty = self.types.intern(TyKind::Result(Ty::UNIT, error.unwrap_or(Ty::NEVER)));
        let (body, left, completed, outcome) =
            (local(self, cx, value.ty), local(self, cx, Ty::INT), local(self, cx, Ty::BOOL), local(self, cx, ty));
        let unit = || mk(hir::ExprKind::Unit, Ty::UNIT);
        let ok = mk(hir::ExprKind::Variant(OK, vec![(0, unit())]), ty);
        // What a run gives is kept, so that its work is not optimized away.
        let keep = |value: hir::Expr| match success {
            Ty::UNIT | Ty::NEVER => hir::Stmt::Expr(value),
            _ => {
                hir::Stmt::Expr(mk(hir::ExprKind::Call(Callee::Std(StdFn::Keep), vec![success], vec![value]), Ty::UNIT))
            }
        };
        let run = mk(hir::ExprKind::CallValue(Box::new(read(body, value.ty)), Vec::new()), info.result);
        let run = match error {
            None => keep(run),
            Some(error) => {
                // `match body() { ok(v) => keep(v), err(e) => { outcome = err(e); completed = false; break } }`
                let (kept, failure) = (local(self, cx, success), local(self, cx, error));
                let pattern = |variant, local, ty| hir::Pattern {
                    kind: hir::PatKind::Variant(
                        variant,
                        vec![hir::Pattern { kind: hir::PatKind::Binding(local), ty, span }],
                    ),
                    ty: info.result,
                    span,
                };
                let kept_body = hir::Block { stmts: vec![keep(read(kept, success))], tail: None, ty: Ty::UNIT };
                let failed = mk(hir::ExprKind::Variant(ERR, vec![(0, read(failure, error))]), ty);
                let stop = hir::Block {
                    stmts: vec![
                        set(outcome, None, failed),
                        set(completed, None, mk(hir::ExprKind::Bool(false), Ty::BOOL)),
                        hir::Stmt::Break,
                    ],
                    tail: None,
                    ty: Ty::NEVER,
                };
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(OK, kept, success),
                        guard: None,
                        body: mk(hir::ExprKind::Block(kept_body), Ty::UNIT),
                    },
                    hir::Arm {
                        pattern: pattern(ERR, failure, error),
                        guard: None,
                        body: mk(hir::ExprKind::Block(stop), Ty::NEVER),
                    },
                ];
                hir::Stmt::Expr(mk(hir::ExprKind::Match(Box::new(run), arms), Ty::UNIT))
            }
        };
        // `if left == 0 { left = next(); if left == 0 { break } }`
        let none_left =
            || mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(read(left, Ty::INT)), Box::new(int(0))), Ty::BOOL);
        let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
        let next = hir::Block {
            stmts: vec![
                set(left, None, std(StdFn::BenchNext, Vec::new(), Ty::INT)),
                hir::Stmt::Expr(mk(hir::ExprKind::If(Box::new(none_left()), stop, None), Ty::UNIT)),
            ],
            tail: None,
            ty: Ty::UNIT,
        };
        let refill = hir::Stmt::Expr(mk(hir::ExprKind::If(Box::new(none_left()), next, None), Ty::UNIT));
        let turn =
            hir::Block { stmts: vec![refill, set(left, Some(BinOp::Sub), int(1)), run], tail: None, ty: Ty::UNIT };
        let stmts = vec![
            hir::Stmt::Expr(std(StdFn::BenchStart, vec![name], Ty::UNIT)),
            hir::Stmt::Let(body, value),
            hir::Stmt::Let(left, int(0)),
            hir::Stmt::Let(completed, mk(hir::ExprKind::Bool(true), Ty::BOOL)),
            hir::Stmt::Let(outcome, ok),
            hir::Stmt::Loop { condition: None, body: turn },
            hir::Stmt::Expr(std(StdFn::BenchEnd, vec![read(completed, Ty::BOOL)], Ty::UNIT)),
        ];
        let block = hir::Block { stmts, tail: Some(Box::new(read(outcome, ty))), ty };
        Self::mk(hir::ExprKind::Block(block), ty, span)
    }

    /// `testing.snapshot(name, value, ignore:, matches:, replace:)` (testing
    /// 9.7): a `String` is compared as it is, any other value as the JSON
    /// `json.encode` writes for it.
    pub(super) fn snapshot(&mut self, cx: &mut FnCx, args: &[ast::Arg], span: Span) -> hir::Expr {
        let Some(&ModuleName::Fn(encode)) = self.names.get("json.encode") else {
            unreachable!("std.json comes with std.testing")
        };
        let value = self.signatures[encode.0 as usize].generics[0];
        let strings = self.types.intern(TyKind::Array(Ty::STRING));
        let map = self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING));
        let mut signature = Signature::simple(vec![Ty::STRING, value, strings, map, map], Ty::UNIT);
        signature.names = ["name", "value", "ignore", "matches", "replace"].map(String::from).to_vec();
        signature.generics = vec![value];
        signature.name = "testing.snapshot".into();
        signature.defaults = vec![
            None,
            None,
            Some(Self::mk(hir::ExprKind::Array(Vec::new()), strings, span)),
            Some(Self::mk(hir::ExprKind::Map(Vec::new()), map, span)),
            Some(Self::mk(hir::ExprKind::Map(Vec::new()), map, span)),
        ];
        let target = CallTarget::new(Callee::Std(StdFn::TestSnapshot), signature, None);
        let mut call = self.call_signature(cx, target, None, args, span);
        // The call, or the block that evaluates its named arguments first.
        fn inner(expr: &mut hir::Expr) -> Option<(Ty, &mut Vec<hir::Expr>)> {
            match &mut expr.kind {
                hir::ExprKind::Call(_, types, values) => Some((types[0], values)),
                hir::ExprKind::Block(block) => block.tail.as_deref_mut().and_then(inner),
                _ => None,
            }
        }
        let Some((ty, values)) = inner(&mut call) else { return call };
        let json = ty != Ty::STRING;
        if json {
            // The value, then each option of `json.encode` as it is when
            // not given.
            let raw = values.remove(1);
            let encoding = &self.signatures[encode.0 as usize];
            let options = (1..encoding.params.len()).map(|at| {
                encoding.defaults.get(at).cloned().flatten().expect("every option of `json.encode` has a default")
            });
            let arguments = std::iter::once(raw).chain(options).collect();
            let text = hir::ExprKind::Call(Callee::Fn(encode), vec![ty], arguments);
            values.insert(1, Self::mk(text, Ty::STRING, span));
        } else if let Some(arg) =
            args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == "ignore" || n.name == "matches"))
        {
            let message = "`ignore` and `matches` look into the JSON of a value; a `String` is compared as it is, so change its varying parts with `replace: [pattern: text]`";
            self.diags.error("E1102", arg.span, message);
        }
        values.insert(2, Self::mk(hir::ExprKind::Bool(json), Ty::BOOL, span));
        call
    }

    /// The literal pattern and locale of a call of `format` or of the
    /// parsing of `std.time`: a pattern with a directive that does not exist,
    /// one to parse that reads no whole date, or a locale without names, is
    /// `E1905` (standard library 14.3.5).
    pub(super) fn check_time_literals(&mut self, id: FnId, signature: &Signature, first: usize, args: &[ast::Arg]) {
        let named = |name: &str| matches!(self.names.get(name), Some(&ModuleName::Fn(f)) if f == id);
        let zoned_format = self
            .type_names
            .get("datetime.ZonedDateTime")
            .and_then(|t| match t {
                TypeName::Nominal(ty) => self.methods.get(&(*ty, "format".to_string())),
                _ => None,
            })
            .is_some_and(|&method| method == id);
        let parsing = match () {
            _ if named("datetime.format") || zoned_format => false,
            _ if named("datetime.parseDateTime") || named("zoned.parseZoned") => true,
            _ => return,
        };
        for param in ["pattern", "locale"] {
            let Some(position) = signature.names.iter().position(|n| n == param) else { continue };
            let arg = args
                .iter()
                .find(|a| a.name.as_ref().is_some_and(|n| n.name == param))
                .or_else(|| args.get(position - first).filter(|a| a.name.is_none()));
            let Some(arg) = arg else { continue };
            let ast::ExprKind::Str(segments) = &arg.value.kind else { continue };
            let text = match segments.as_slice() {
                [] => "",
                [ast::StrSegment::Text(text)] => text.as_str(),
                _ => continue,
            };
            let problem = if param == "pattern" {
                crate::time_pattern::pieces(text)
                    .and_then(|pieces| if parsing { crate::time_pattern::parsing(&pieces) } else { Ok(()) })
                    .err()
            } else {
                let tag = text.replace('_', "-").to_ascii_lowercase();
                let language = tag.split('-').next().unwrap_or_default().to_string();
                let known = include_str!("../../../tondo-runtime/data/names.txt")
                    .lines()
                    .filter_map(|line| line.split('\t').next())
                    .any(|known| known.eq_ignore_ascii_case(&tag) || known.eq_ignore_ascii_case(&language));
                (!known).then(|| {
                    format!("`{text}` is not a language with names of months and days: write a BCP 47 tag of CLDR, such as `es`, `de` or `pt-BR`")
                })
            };
            if let Some(problem) = problem {
                self.diags.error("E1905", arg.value.span, problem);
            }
        }
    }
}
