//! What only the standard library may do, and the literals its functions check.

use super::*;

impl<'a> Checker<'a> {
    /// Whether a function is `name` of the standard module `module`.
    /// A value whose methods `std.mcp` serves, as `served`: each `pub`
    /// method reflection cannot call would be no tool, which is an error
    /// rather than one left out (E1912).
    pub(super) fn serving(&mut self, id: FnId, served: Ty) {
        let mcp = self.is_std_fn(id, "mcp", "mcp.serve") || self.is_std_fn(id, "mcp", "mcp.handler");
        if !mcp || !self.served.insert(served) {
            return;
        }
        let Some(owner) = self.owner(served) else { return };
        let (name, unreflected) = (self.types.display(served), owner.unreflected.clone());
        for (method, span, why) in unreflected {
            let message = format!(
                "`{name}.{method}` is `pub`, so `std.mcp` serves it as a tool, which it cannot be: {why}; make it private, or take and give values a tool can"
            );
            self.diags.error("E1912", span, message);
        }
    }

    pub(super) fn is_std_fn(&self, id: FnId, module: &str, name: &str) -> bool {
        matches!(self.names.get(name), Some(&ModuleName::Fn(f)) if f == id)
            && self.fn_scopes.get(id.0 as usize).is_some_and(|scope| {
                self.module_paths
                    .get(scope.module as usize)
                    .is_some_and(|p| p.len() == 2 && p[0] == "std" && p[1] == module)
            })
    }

    /// The function of `std.format` that applies a format to a value of
    /// `ty` as an integer or a float, as `formatted` does.
    pub(super) fn typed_apply(&mut self, ty: Ty) -> Option<FnId> {
        let integer = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte) || self.bound_by(ty, self.integer);
        let name = match self.types.kind(ty) {
            _ if integer => "format.applyInteger",
            TyKind::Float(_) => "format.applyFloat",
            _ => return None,
        };
        match self.names.get(name) {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        }
    }

    /// `format.apply(value, spec)` of an integer or a float.
    pub(super) fn apply_typed(
        &mut self,
        id: FnId,
        value: hir::Expr,
        spec: hir::Expr,
        result: Ty,
        span: Span,
    ) -> hir::Expr {
        let (ty, at) = (value.ty, value.span);
        let (type_args, args) = match self.types.kind(ty) {
            TyKind::Byte => (vec![Ty::INT], vec![Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::INT, at), spec]),
            TyKind::Float(_) => {
                let single = Self::mk(hir::ExprKind::Bool(ty == Ty::F32), Ty::BOOL, at);
                let value = match ty == Ty::F32 {
                    true => Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::F64, at),
                    false => value,
                };
                (Vec::new(), vec![value, single, spec])
            }
            _ => (vec![ty], vec![value, spec]),
        };
        Self::mk(hir::ExprKind::Call(Callee::Fn(id), type_args, args), result, span)
    }

    /// Writes each value of an interpolated markup through
    /// `cli.markupText`, which keeps its brackets as text.
    pub(super) fn markup_values(&mut self, text: &mut hir::Expr) {
        let Some(&ModuleName::Fn(escape)) = self.names.get("cli.markupText") else { return };
        let hir::ExprKind::Interpolate(segments) = &mut text.kind else { return };
        for segment in segments.iter_mut() {
            let hir::Segment::Value(value) = segment else { continue };
            let span = value.span;
            let placeholder = Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
            let inner = std::mem::replace(value, placeholder);
            let shown = Self::mk(hir::ExprKind::Interpolate(vec![hir::Segment::Value(inner)]), Ty::STRING, span);
            *value = Self::mk(hir::ExprKind::Call(Callee::Fn(escape), Vec::new(), vec![shown]), Ty::STRING, span);
        }
    }

    /// The templates a `template.Set` adds whose text is known when the
    /// program compiles, a string or the files of `embed.files("...")`: one
    /// that does not read is `E1906` (standard library 14.31).
    /// The declarations of a file of a system whose name a file of another
    /// system declares too, with another head: one signature for all of
    /// them, so that what common code calls is the same (language 6.8).
    pub(super) fn system_heads(&mut self, module: &ast::Module) {
        let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        for item in &module.items {
            let (name, span, end) = match item {
                ast::Item::Fn(f) if f.owner.is_none() => (&f.name, f.span, f.body.as_ref().map(|b| b.span.start)),
                ast::Item::Const(c) => (&c.name, c.span, None),
                ast::Item::Type(t) => (&t.name, t.span, None),
                ast::Item::Enum(e) => (&e.name, e.span, None),
                ast::Item::Alias(a) => (&a.name, a.span, None),
                _ => continue,
            };
            let file = self.sources.file_of(span.start);
            let Some(systems) = crate::project::systems_of(&file.path) else { continue };
            let text = self.sources.slice(Span::new(span.start, end.unwrap_or(span.end).max(span.start)));
            let head = text.lines().next().unwrap_or_default().trim().trim_end_matches('{').trim_end().to_string();
            let Some(other) = self
                .elsewhere
                .iter()
                .find(|e| e.module == file.module && e.name == name.name && flat(&e.head) != flat(&head))
            else {
                continue;
            };
            let here: Vec<&str> = systems.iter().map(|s| s.shown()).collect();
            let message = format!(
                "`{}` is `{head}` here, for {}, and `{}` in `{}`, for {}: give it one signature for every system, which common code calls",
                name.name,
                here.join(" and "),
                other.head,
                other.file,
                other.systems
            );
            self.diags.error("E1808", name.span, message);
        }
    }

    /// A literal pattern of `Regex.compile`, which must be in the dialect
    /// the runtime compiles (standard library 14.16): checked here, so that
    /// its `?` cannot fail.
    /// The name a call of `model.step`, `model.mark`, `model.weak` or
    /// `model.strong` gives, when it is written out.
    pub(super) fn note_model_name(&mut self, id: FnId, args: &[ast::Arg]) {
        let what =
            ["step", "mark", "weak", "strong"].into_iter().find(|f| self.is_std_fn(id, "model", &format!("model.{f}")));
        let Some(what) = what else { return };
        let Some(arg) = args.first().filter(|a| a.name.is_none()) else { return };
        let ast::ExprKind::Str(segments) = &arg.value.kind else { return };
        let mut text = String::new();
        let mut whole = true;
        for segment in segments {
            match segment {
                ast::StrSegment::Text(part) => text.push_str(part),
                ast::StrSegment::Expr(..) => {
                    whole = false;
                    break;
                }
            }
        }
        match what {
            "step" => self.model_steps.push((text, whole)),
            _ => self.model_names.push((what, text, whole, arg.value.span)),
        }
    }

    /// `W1020`: a step marked or made fair that no model of the program has,
    /// as its name was written wrong or changed on one side only.
    pub(super) fn check_model_names(&mut self) {
        if self.model_steps.is_empty() {
            return;
        }
        for (what, name, whole, span) in std::mem::take(&mut self.model_names) {
            // A step named in parts, as `"resume {t}"`, has every name that
            // starts as it does.
            let known = self.model_steps.iter().any(|(step, all)| match (all, whole) {
                (true, true) => *step == name,
                (true, false) => step.starts_with(&name),
                (false, _) => name.starts_with(step.as_str()) || step.starts_with(&name),
            });
            if known {
                continue;
            }
            let mut steps: Vec<String> = self
                .model_steps
                .iter()
                .map(|(step, all)| if *all { format!("`{step}`") } else { format!("`{step}...`") })
                .collect();
            steps.sort();
            steps.dedup();
            let near = closest(&name, self.model_steps.iter().map(|(step, _)| step.as_str()));
            let hint = near.map_or(String::new(), |near| format!("; did you mean `{near}`?"));
            let message = format!(
                "`model.{what}(\"{name}\")` names no step of a model of the program, whose steps are {}{hint}",
                steps.join(", ")
            );
            self.diags.warning("W1020", span, message);
            if let Some(near) = near.filter(|_| whole) {
                self.diags.fix(format!("write `{near}`"), false, vec![(span, format!("\"{near}\""))]);
            }
        }
    }

    pub(super) fn check_regex_literal(&mut self, id: FnId, args: &[ast::Arg]) {
        let Some(&TypeName::Nominal(regex)) = self.type_names.get("regex.Regex") else { return };
        if self.methods.get(&(regex, "compile".to_string())) != Some(&id) {
            return;
        }
        let named = args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == "pattern"));
        let Some(arg) = named.or_else(|| args.first().filter(|a| a.name.is_none())) else { return };
        let ast::ExprKind::Str(segments) = &arg.value.kind else { return };
        let Some(pattern) = segments
            .iter()
            .map(|s| match s {
                ast::StrSegment::Text(text) => Some(text.as_str()),
                ast::StrSegment::Expr(..) => None,
            })
            .collect::<Option<String>>()
        else {
            return;
        };
        if let Err((kind, offset)) = crate::regex_dialect::check(&pattern, false, 256, 256) {
            let message = format!("the pattern does not compile: regex.{kind:?} at byte {offset}");
            self.diags.error("E1909", arg.value.span, message);
        }
    }

    pub(super) fn check_template_literals(&mut self, id: FnId, args: &[ast::Arg]) {
        let is = |this: &Self, name: &str| match this.type_names.get("template.Set") {
            Some(&TypeName::Nominal(ty)) => this.methods.get(&(ty, name.to_string())).is_some_and(|&m| m == id),
            _ => false,
        };
        let literal = |expr: &ast::Expr| match &expr.kind {
            ast::ExprKind::Str(segments) => segments
                .iter()
                .map(|s| match s {
                    ast::StrSegment::Text(text) => Some(text.as_str()),
                    ast::StrSegment::Expr(..) => None,
                })
                .collect::<Option<String>>(),
            _ => None,
        };
        if is(self, "add")
            && let Some(arg) = args.get(1)
            && let Some(text) = literal(&arg.value)
            && let Err(error) = crate::template_syntax::load(&text, false)
        {
            let (line, column) = crate::template_syntax::line_column(&text, error.at);
            let message = format!("line {line}, column {column} of the template: {}", error.message);
            self.diags.error("E1906", arg.value.span, message);
        }
        // Every file is a template when no pattern chooses among them.
        if is(self, "addFiles")
            && args.len() == 1
            && let Some(arg) = args.first()
            && let ast::ExprKind::Call { callee, args: inner } = &arg.value.kind
            && let ast::ExprKind::Path(path) = &callee.kind
            && path.last().is_some_and(|p| p.name.ends_with("files"))
            && let [written] = inner.as_slice()
            && let Some(written) = literal(&written.value)
            && let Ok(crate::embed::Read::Files(files)) = crate::embed::read(
                &self.sources.file_of(arg.value.span.start).path.clone(),
                &written,
                crate::embed::Kind::Files,
            )
        {
            for (name, bytes) in files {
                let Ok(text) = String::from_utf8(bytes) else { continue };
                if let Err(error) = crate::template_syntax::load(&text, false) {
                    let (line, column) = crate::template_syntax::line_column(&text, error.at);
                    let message = format!("`{written}/{name}`, line {line}, column {column}: {}", error.message);
                    self.diags.error("E1906", arg.value.span, message);
                }
            }
        }
    }

    /// A path or method of `std.reflect` written as a literal, on a value of
    /// a type known here: one it cannot reach is `E2110` (27.10).
    pub(super) fn check_reflection(&mut self, name: &str, type_args: &[Ty], args: &[ast::Arg]) {
        let target = if name == "reflect.set" { type_args[0] } else { type_args[1] };
        let Some(arg) = args.get(1).filter(|a| a.name.is_none()) else { return };
        let ast::ExprKind::Str(segments) = &arg.value.kind else { return };
        let [ast::StrSegment::Text(text)] = segments.as_slice() else { return };
        if self.types.has_params(target) || self.types.kind(target) == &TyKind::Error {
            return;
        }
        let result = if name.starts_with("reflect.call") {
            self.reflect_method(target, text, name == "reflect.callMut")
        } else {
            self.types.reflect_path(target, text)
        };
        if let Err(why) = result {
            self.diags.error("E2110", arg.value.span, format!("`{name}` cannot reach `{text}`: {why}"));
        }
    }

    /// Whether reflection can call a method of the type by this name.
    pub(super) fn reflect_method(&mut self, ty: Ty, method: &str, mutating: bool) -> Result<(), String> {
        let key = self.owner_key(ty);
        let shown = self.types.display(ty);
        let public = |id: &FnId| self.method_visibility.get(id).is_some_and(|&(_, public)| public);
        let mut names: Vec<&str> = self
            .methods
            .iter()
            .filter(|((owner, _), id)| *owner == key && public(id) && self.signatures[id.0 as usize].receiver.is_some())
            .map(|((_, name), _)| name.as_str())
            .collect();
        names.sort();
        match self.methods.get(&(key, method.to_string())) {
            Some(id) if public(id) => match self.signatures[id.0 as usize].receiver {
                Some(ast::Mode::Mut) if !mutating => {
                    Err(format!("`{shown}.{method}` takes `mut self`: call it with `callMut`"))
                }
                Some(_) => Ok(()),
                None => Err(format!("`{shown}.{method}` has no receiver; call it as `{shown}.{method}(...)`")),
            },
            _ if names.is_empty() => Err(format!("`{shown}` has no `pub` methods, so it has no method `{method}`")),
            _ => {
                let listed: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                let listed = match listed.split_last() {
                    Some((last, [])) => format!("its method is {last}"),
                    Some((last, rest)) => format!("its methods are {} and {last}", rest.join(", ")),
                    None => String::new(),
                };
                Err(format!("`{shown}` has no `pub` method `{method}`; {listed}"))
            }
        }
    }

    /// The number with named operations a type is (18.9).
    pub(super) fn number_kind(&self, ty: Ty) -> Option<crate::numbers::Kind> {
        use crate::numbers::Kind;
        match self.types.kind(ty) {
            TyKind::Int(int) => Some(Kind::Int(*int)),
            TyKind::Float(float) => Some(Kind::Float(*float)),
            TyKind::Byte => Some(Kind::Byte),
            _ => None,
        }
    }

    /// The type a parameter or result of a named operation of `own` has.
    pub(super) fn number_shape(&mut self, shape: crate::numbers::Shape, own: Ty) -> Ty {
        use crate::numbers::Shape;
        match shape {
            Shape::Same | Shape::Integral | Shape::Numeric => own,
            Shape::Int => Ty::INT,
            Shape::Bool => Ty::BOOL,
            Shape::String => Ty::STRING,
            Shape::Bytes => self.types.intern(TyKind::Array(Ty::BYTE)),
            Shape::Order => match self.type_names.get("prelude.ByteOrder") {
                Some(&TypeName::Nominal(ty)) => ty,
                _ => Ty::ERROR,
            },
            Shape::Pattern if own == Ty::F32 => Ty::int(IntTy::U32),
            Shape::Pattern => Ty::int(IntTy::U64),
            Shape::Optional => self.types.intern(TyKind::Option(own)),
            Shape::Overflowing => self.types.intern(TyKind::Tuple(vec![own, Ty::BOOL])),
            Shape::Parsed => self.types.intern(TyKind::Result(own, self.text_error)),
            Shape::Rooted => {
                let error = match self.type_names.get("math.MathError") {
                    Some(&TypeName::Nominal(ty)) => ty,
                    _ => Ty::ERROR,
                };
                self.types.intern(TyKind::Result(own, error))
            }
        }
    }

    /// The signature of a named operation of `own`, its receiver first when
    /// it has one, with the values of the parameters a call may omit.
    pub(super) fn number_signature(&mut self, own: Ty, entry: &crate::numbers::Entry) -> Signature {
        let (mut params, mut names, mut defaults) = (Vec::new(), Vec::new(), Vec::new());
        if entry.receiver {
            params.push(own);
            names.push("self".to_string());
            defaults.push(None);
        }
        for &(name, shape, default) in entry.params {
            let ty = self.number_shape(shape, own);
            params.push(ty);
            names.push(name.to_string());
            defaults.push(default.map(|text| {
                let kind = match text.parse::<i128>() {
                    Ok(number) => hir::ExprKind::Int(number),
                    // `ByteOrder.Big`, its first variant.
                    Err(_) => hir::ExprKind::Variant(0, Vec::new()),
                };
                Self::mk(kind, ty, Span::default())
            }));
        }
        let result = self.number_shape(entry.result, own);
        Signature {
            modes: vec![ast::Mode::Value; params.len()],
            params,
            names,
            receiver: entry.receiver.then_some(ast::Mode::Value),
            variadic: false,
            result,
            generics: Vec::new(),
            name: format!("{}.{}", self.display(own), entry.name),
            defaults,
            is_unsafe: false,
            foreign: false,
        }
    }

    /// A named operation of the number `own` without a receiver:
    /// `UInt8.parse(text)`, `Float.fromBits(bits)`, `Int8.wrapping(value)`.
    pub(super) fn number_function(
        &mut self,
        cx: &mut FnCx,
        own: Ty,
        entry: &crate::numbers::Entry,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        use crate::numbers::{Kind, NumOp};
        let callee = Callee::Std(StdFn::Number(entry.op));
        if !matches!(entry.op, NumOp::Wrapping | NumOp::Saturating) {
            let signature = self.number_signature(own, entry);
            let target = CallTarget::new(callee, signature, expected);
            return self.call_signature(cx, target, None, args, span);
        }
        // Any integer or `Byte`, and for `saturating` any float too.
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let value = self.expr(cx, &arg.value, None);
        let accepted = match self.number_kind(value.ty) {
            Some(Kind::Float(_)) => entry.op == NumOp::Saturating,
            Some(_) => true,
            None => value.ty == Ty::ERROR || self.bound_by(value.ty, self.integer),
        };
        if !accepted {
            let what = if entry.op == NumOp::Saturating { "a number or a `Byte`" } else { "an integer or a `Byte`" };
            let message =
                format!("`{}.{}` takes {what}, and this is {}", self.display(own), entry.name, self.display(value.ty));
            self.diags.error("E1102", value.span, message);
            return Self::error_expr(span);
        }
        Self::mk(hir::ExprKind::Call(callee, Vec::new(), vec![value]), own, span)
    }

    /// A constant of an intrinsic numeric type: `max`, `min` and `bits` of
    /// the integers, and `pi`, `e`, `infinity`, `nan`, `epsilon`, `min` and
    /// `max` of the floats.
    pub(super) fn intrinsic_constant(&self, ty: Ty, name: &str) -> Option<hir::ExprKind> {
        use crate::types::FloatTy;
        match (self.types.kind(ty), name) {
            (TyKind::Int(int), "max") => Some(hir::ExprKind::Int(int.max() as i128)),
            (TyKind::Int(int), "min") => Some(hir::ExprKind::Int(int.min())),
            (TyKind::Int(int), "bits") => Some(hir::ExprKind::Int(int.bits() as i128)),
            (TyKind::Float(FloatTy::F64), "max") => Some(hir::ExprKind::Float(f64::MAX)),
            (TyKind::Float(FloatTy::F64), "min") => Some(hir::ExprKind::Float(f64::MIN)),
            (TyKind::Float(_), "max") => Some(hir::ExprKind::Float(f32::MAX as f64)),
            (TyKind::Float(_), "min") => Some(hir::ExprKind::Float(f32::MIN as f64)),
            (TyKind::Float(_), "pi") => Some(hir::ExprKind::Float(std::f64::consts::PI)),
            (TyKind::Float(_), "e") => Some(hir::ExprKind::Float(std::f64::consts::E)),
            (TyKind::Float(_), "infinity") => Some(hir::ExprKind::Float(f64::INFINITY)),
            (TyKind::Float(_), "nan") => Some(hir::ExprKind::Float(f64::NAN)),
            (TyKind::Float(crate::types::FloatTy::F64), "epsilon") => Some(hir::ExprKind::Float(f64::EPSILON)),
            (TyKind::Float(_), "epsilon") => Some(hir::ExprKind::Float(f32::EPSILON as f64)),
            _ => None,
        }
    }

    /// In the `meta` profile, an operation of the host is `E2108`: a
    /// provider or generator runs while compiling and sees only its request
    /// (27.7). Whether it was.
    pub(super) fn meta_denied(&mut self, span: Span, what: &str) -> bool {
        if self.meta {
            let message = format!(
                "{what} is not available to a provider or generator, which runs while the program compiles and works only on its request"
            );
            self.diags.error("E2108", span, message);
        }
        self.meta
    }

    /// A provider or generator gives the same output for the same request,
    /// so it cannot call what reads the clock or the system's entropy.
    /// The tests that check a model: those that call a function of
    /// `checks_models`, or one of theirs that does, however deep; and those
    /// that start tasks, so.
    pub(super) fn mark_models(&mut self) {
        let models = self.reaching(self.checks_models.clone());
        let tasks = self.reaching(self.spawning.clone());
        for (test, callees) in &self.test_calls {
            let node = &mut self.test_nodes[*test as usize];
            node.model |= callees.iter().any(|f| models.contains(f));
            node.tasks |= callees.iter().any(|f| tasks.contains(f));
        }
        for test in &self.test_spawns {
            self.test_nodes[*test as usize].tasks = true;
        }
    }

    /// The functions that call one of `reach`, however deep, with them.
    pub(super) fn reaching(&self, mut reach: HashSet<FnId>) -> HashSet<FnId> {
        loop {
            let more: Vec<FnId> = self
                .calls
                .iter()
                .filter(|(caller, callees)| !reach.contains(caller) && callees.iter().any(|f| reach.contains(f)))
                .map(|(caller, _)| *caller)
                .collect();
            if more.is_empty() {
                break;
            }
            reach.extend(more);
        }
        reach
    }

    pub(super) fn repeatable(&mut self, id: FnId, span: Span) {
        if let Some(name) = self.unrepeatable.get(&id) {
            let what = format!("{name}, which gives another value each time,");
            self.meta_denied(span, &what);
        }
    }

    /// `reflect.Value`, once `std.reflect` is imported.
    pub(super) fn reflect_value_ty(&self) -> Option<Ty> {
        match self.type_names.get("reflect.Value") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// `reflect.valueOf(value)`, which an argument for a `reflect.Value`
    /// parameter becomes.
    pub(super) fn reflect_value(&mut self, value: hir::Expr) -> hir::Expr {
        let (Some(&ModuleName::Fn(id)), Some(&reflect), Some(target)) =
            (self.names.get("reflect.valueOf"), self.trait_names.get("reflect.Reflect"), self.reflect_value_ty())
        else {
            return value;
        };
        let (ty, span) = (value.ty, value.span);
        if ty != Ty::ERROR && !self.satisfies(ty, reflect, &[]) {
            let message = format!(
                "{} does not implement `reflect.Reflect`, so it cannot become a `reflect.Value`{REFLECT_WHY}",
                self.display(ty)
            );
            self.diags.error("E1105", span, message);
            return Self::error_expr(span);
        }
        Self::mk(hir::ExprKind::Call(Callee::Fn(id), vec![ty], vec![value]), target, span)
    }

    /// `clock.settle()` and `clock.advance(duration)` (testing 7.9).
    pub(super) fn clock_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let (function, params) = match method.name.as_str() {
            "settle" => (StdFn::ClockSettle, Vec::new()),
            "advance" => {
                let Some(duration) = self.duration else {
                    let message = "`advance` takes a `time.Duration`: import std.time";
                    self.diags.error("E1001", method.span, message);
                    return Self::error_expr(span);
                };
                (StdFn::ClockAdvance, vec![duration])
            }
            _ => return self.no_method(receiver.ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, &param) in args.iter().zip(&params) {
            let value = self.expr(cx, &arg.value, Some(param));
            values.push(self.coerce(value, param));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(function), Vec::new(), values), Ty::UNIT, span)
    }

    /// The members of a suite, or of the root, each as a closure that runs
    /// it; `prefixes` gives the identity each member's name extends.
    pub(super) fn test_members(
        &mut self,
        cx: &mut FnCx,
        nodes: &[&ast::TestNode],
        parent: u32,
        prefixes: &[String],
        span: Span,
    ) -> hir::Expr {
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        for (&node, prefix) in nodes.iter().zip(prefixes) {
            let name = node.name();
            let id = format!("{prefix}::{}", name.name);
            if name.name == "_" {
                self.diags.error("E1115", name.span, "a test or a suite needs a name; `_` is not one");
                continue;
            }
            if !seen.insert(id.clone()) {
                let message =
                    format!("`{}` is declared twice at this level of the test tree; give one another name", name.name);
                self.diags.error("E2002", name.span, message);
                continue;
            }
            let index = self.test_nodes.len() as u32;
            let suite = matches!(node, ast::TestNode::Suite(_));
            let (span, whole) = (name.span, node.span());
            let info = hir::TestNodeInfo {
                id: id.clone(),
                parent,
                suite,
                bench: false,
                fuzz: false,
                model: false,
                tasks: false,
                span,
                whole,
            };
            self.test_nodes.push(info);
            let unit = self.types.intern(TyKind::Fn(Vec::new(), Ty::UNIT, false, Vec::new(), false));
            let kind = ClosureKind { deferred: false, is_unsafe: false, test: true, infer_error: false };
            self.closure_name = Some(format!("{} {}", if suite { "suite" } else { "test" }, name.name));
            // A test's own body may measure (`testing.bench`), and it is of
            // its file's module, whose private operations it calls.
            let outer = std::mem::replace(&mut self.measuring, (!suite).then_some(index));
            let module = match node {
                ast::TestNode::Test(t) => t.module,
                ast::TestNode::Suite(s) => s.module,
            };
            let outer_module = std::mem::replace(&mut self.module, module);
            let closure =
                self.closure_with(cx, (&[], None), Some(unit), kind, node.span(), |this, inner, _| match node {
                    ast::TestNode::Test(t) => this.block(inner, &t.body, Some(Ty::UNIT)),
                    ast::TestNode::Suite(s) => this.suite_body(inner, s, index, &id),
                });
            self.module = outer_module;
            self.measuring = outer;
            let Some((body, _, info)) = self.types.closure(closure.ty) else { continue };
            let erased = self.types.intern(TyKind::Fn(Vec::new(), info.result, false, Vec::new(), false));
            members.push((index, Self::mk(hir::ExprKind::Erase(Box::new(closure), FnId(body)), erased, name.span)));
        }
        Self::mk(hir::ExprKind::TestMembers(members), Ty::UNIT, span)
    }

    /// The body of a suite's closure: its setup, then its members, then,
    /// as it ends, its `defer`s.
    pub(super) fn suite_body(&mut self, cx: &mut FnCx, suite: &ast::SuiteDecl, index: u32, id: &str) -> hir::Block {
        cx.setup = true;
        let mut stmts = self.statements(cx, &suite.setup);
        cx.setup = false;
        if suite.members.is_empty() {
            let message = format!("the suite `{}` has no tests: add a `test` inside it, or remove it", suite.name.name);
            self.diags.error("E2004", suite.name.span, message);
        }
        let members: Vec<&ast::TestNode> = suite.members.iter().collect();
        let prefixes = vec![id.to_string(); members.len()];
        stmts.push(hir::Stmt::Expr(self.test_members(cx, &members, index, &prefixes, suite.span)));
        let ty = if cx.diverged { Ty::NEVER } else { Ty::UNIT };
        hir::Block { stmts, tail: None, ty }
    }

    /// The value of a constant, evaluated on first use (6.5): integer
    /// arithmetic is done now, so what would panic is an error (`E1903`).
    pub(super) fn const_value(&mut self, id: usize, span: Span) -> hir::Expr {
        if let Some(value) = &self.consts[id].value {
            let mut value = value.clone();
            value.span = span;
            if let hir::ExprKind::Int(_) = value.kind {
                self.const_uses.insert(span);
            }
            return value;
        }
        let decl = self.consts[id].decl;
        if self.consts[id].resolving {
            let message = format!("the constant `{}` depends on itself", decl.name.name);
            self.diags.error("E1902", decl.name.span, message);
            return Self::error_expr(span);
        }
        self.consts[id].resolving = true;
        let module = std::mem::replace(&mut self.module, decl.module);
        let outer_types = std::mem::take(&mut self.type_params);
        let outer_self = self.self_type.take();
        let ty = decl.ty.as_ref().map(|t| self.resolve_type(t));
        let mut cx = self.new_cx(Ty::UNIT);
        let value = self.expr(&mut cx, &decl.value, ty);
        if decl.public && ty.is_none() {
            let written =
                if self.types.has_error(value.ty) { "Type".to_string() } else { self.types.display(value.ty) };
            let name = decl.name.name.rsplit('.').next().unwrap_or(&decl.name.name);
            let message = format!(
                "a public constant declares its type, which is part of the module's API: `pub const {name}: {written} = ...`"
            );
            self.diags.error("E1115", decl.name.span, message);
        }
        let mut value = match ty {
            Some(ty) => self.coerce(value, ty),
            None => value,
        };
        if !self.types.has_error(value.ty) {
            match fold(&self.types, &value) {
                Some(Ok(number)) => value.kind = hir::ExprKind::Int(number),
                Some(Err(problem)) => {
                    let message = format!("evaluating the constant would panic: {problem}");
                    self.diags.error("E1903", decl.value.span, message);
                }
                None if !constant(&mut value) => {
                    let message = "a constant is evaluated in compilation: build it from literals, other constants, their operators and constructors";
                    self.diags.error("E1901", decl.value.span, message);
                }
                None => {}
            }
        }
        self.module = module;
        self.type_params = outer_types;
        self.self_type = outer_self;
        self.consts[id].value = Some(value.clone());
        value.span = span;
        if let hir::ExprKind::Int(_) = value.kind {
            self.const_uses.insert(span);
        }
        value
    }
}
