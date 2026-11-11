//! `try`, collection literals, numbers, templates, display, strings and paths.

use super::*;

impl<'a> Checker<'a> {
    /// `value?` propagates `none` or an error out of the enclosing function
    /// and otherwise yields the payload (15.2).
    /// `try { ... }` (15.2): a closure called there, whose `?` ends the block
    /// and not the function. What it gives comes from where it goes.
    pub(super) fn try_block(
        &mut self,
        cx: &mut FnCx,
        block: &ast::Block,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let gives = expected.filter(|&ty| matches!(self.types.kind(ty), TyKind::Option(_) | TyKind::Result(..)));
        let Some(gives) = gives else {
            let message = "a `try` block gives an option or a result: say which, as `let city: String? = try { ... }`";
            self.diags.error("E1301", span, message);
            return Self::error_expr(span);
        };
        let function = self.types.intern(TyKind::Fn(Vec::new(), gives, false, Vec::new(), false));
        let closure = ast::Closure { is_unsafe: false, params: Vec::new(), result: None, body: block.clone() };
        self.try_body = true;
        let value = self.closure(cx, &closure, Some(function), span);
        let local = self.hidden_local(cx, value.ty);
        let read = Self::mk(hir::ExprKind::Local(local), value.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), Vec::new()), gives, span);
        let stmts = vec![hir::Stmt::Let(local, value)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(call)), ty: gives }), gives, span)
    }

    pub(super) fn try_expr(&mut self, cx: &mut FnCx, inner: &ast::Expr, expected: Option<Ty>, span: Span) -> hir::Expr {
        if cx.deferred {
            self.diags.error("E1410", span, "a defer block cannot propagate an error; handle it inside the block");
        }
        // A call learns the success type it should give: `decode(text)?`.
        let hint = match &inner.kind {
            ast::ExprKind::Call { .. } => expected,
            _ => None,
        };
        let value = self.expr(cx, inner, hint);
        self.note_propagation(cx, value.ty, Span::new(span.end - 1, span.end), "?");
        self.note_exit(cx, Span::new(span.end - 1, span.end));
        match self.types.kind(value.ty).clone() {
            TyKind::Option(payload) => {
                if !matches!(self.types.kind(cx.result), TyKind::Option(_)) {
                    // A function that fails with `Failure` says what is missing.
                    let message = match cx.error {
                        Some(error) if Some(error) == self.failure_type() => {
                            "`?` on an option needs the function to return an option; this one fails with `Failure`, so say what is missing with `.context(\"...\")?`".to_string()
                        }
                        _ => format!(
                            "`?` on an option needs the function to return an option, but it returns {}",
                            self.display(cx.result)
                        ),
                    };
                    self.diags.error("E1301", span, message);
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), payload, span)
            }
            // What cannot fail propagates nothing: its success comes out.
            TyKind::Result(ok, Ty::NEVER) => Self::mk(hir::ExprKind::Try(Box::new(value)), ok, span),
            TyKind::Result(ok, error) => {
                if let Some(channel) = self.error_channel(cx, error, span, "E1301")
                    && channel != error
                    && channel != Ty::ERROR
                    && !self.types.has_error(error)
                {
                    // Widening into an error union is a closed set inclusion (15.3).
                    let produced = self.types.members(error).map_or(vec![error], <[Ty]>::to_vec);
                    let declared = self.types.members(channel).map_or(vec![channel], <[Ty]>::to_vec);
                    let missing: Vec<Ty> = produced.iter().copied().filter(|t| !declared.contains(t)).collect();
                    if !missing.is_empty() && Some(channel) == self.failure_type() {
                        let message = format!(
                            "`?` would propagate {} but the function fails with `Failure`: say what it was doing with `.context(\"...\")?`, which keeps the error as the cause",
                            self.display(error)
                        );
                        self.diags.error("E1301", span, message);
                    } else if !missing.is_empty() {
                        let missing_text: Vec<String> = missing.iter().map(|&t| self.display(t)).collect();
                        let widened = self.types.union(&[channel, error]);
                        let result = self.types.intern(TyKind::Result(cx.result, widened));
                        let message = format!(
                            "`?` would propagate {} but the function fails with {}; {} is missing: declare the result as {}",
                            self.display(error),
                            self.display(channel),
                            missing_text.join(", "),
                            self.display(result)
                        );
                        self.diags.error("E1301", span, message);
                        // Widening a public function's errors changes its interface.
                        if let Some((written, public)) = cx.result_span {
                            let widened = self.types.display(result);
                            let title = format!("declare the result as `{widened}`");
                            let mut edits = vec![(written, widened)];
                            edits.extend(self.imports_naming(error, span));
                            self.diags.fix(title, !public, edits);
                        }
                    }
                }
                Self::mk(hir::ExprKind::Try(Box::new(value)), ok, span)
            }
            TyKind::Error => Self::error_expr(span),
            _ => {
                let message = format!("`?` applies to `T?` or `T ! E`, not {}", self.display(value.ty));
                self.diags.error("E1301", span, message);
                Self::error_expr(span)
            }
        }
    }

    /// For an editor: a place where the function being checked may leave,
    /// but not a closure in it, which leaves only itself.
    pub(super) fn note_exit(&mut self, cx: &FnCx, at: Span) {
        if cx.parent.is_none()
            && let (Some(function), Some(index)) = (self.leaving, self.index.as_mut())
        {
            index.exits.push((function, at));
        }
    }

    /// For an editor's hover: what a `?` or a `fail` does with a value of
    /// type `ty` where it is written.
    pub(super) fn note_propagation(&mut self, cx: &FnCx, ty: Ty, at: Span, what: &str) {
        if self.index.is_none() || ty == Ty::ERROR {
            return;
        }
        let from = match (cx.parent.is_some(), cx.infer_error) {
            (true, _) => "the closure".to_string(),
            (false, true) if self.naming == "main" => "`main`".to_string(),
            _ => format!("`{}`", self.naming),
        };
        // What the error does once out of the function.
        let error = match self.types.kind(ty).clone() {
            TyKind::Result(_, error) => Some(error),
            TyKind::Option(_) => None,
            _ => Some(ty),
        };
        let then = match cx.error {
            _ if cx.infer_error && cx.parent.is_none() && self.naming == "main" => {
                ", which ends the program: it prints `error: ` and the error's `Display`, and exits with 1".to_string()
            }
            Some(channel) if Some(channel) != error => format!(", which fails with `{}`", self.types.display(channel)),
            _ => String::new(),
        };
        let note = match (what, self.types.kind(ty).clone()) {
            ("fail", _) => format!("`fail` ends {from} with the `{}`{then}", self.types.display(ty)),
            (_, TyKind::Option(payload)) => {
                format!("`?` gives the `{}`, or returns `none` from {from}", self.types.display(payload))
            }
            (_, TyKind::Result(ok, Ty::NEVER)) => {
                format!("`?` gives the `{}`: what it applies to cannot fail", self.types.display(ok))
            }
            (_, TyKind::Result(ok, error)) => format!(
                "`?` gives the `{}`, or returns the `{}` from {from}{then}",
                self.types.display(ok),
                self.types.display(error)
            ),
            _ => return,
        };
        if let Some(index) = self.index.as_mut() {
            index.notes.push((at, note));
        }
    }

    /// The prelude's `Failure` (15.10).
    pub(super) fn failure_type(&self) -> Option<Ty> {
        match self.type_names.get("Failure") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A map type; its entry tuple `(K, V)` is interned too, for iteration.
    pub(super) fn map_type(&mut self, key: Ty, value: Ty) -> Ty {
        self.types.intern(TyKind::Tuple(vec![key, value]));
        self.types.intern(TyKind::Option(value));
        self.types.intern(TyKind::Map(key, value))
    }

    pub(super) fn require_key(&mut self, ty: Ty, span: Span) {
        if !self.types.is_key(ty) {
            let message = format!("{} cannot be a map key or set element: it is not `Key`", self.display(ty));
            self.diags.error("E1105", span, message);
        }
    }

    pub(super) fn map_literal(
        &mut self,
        cx: &mut FnCx,
        pairs: &[(ast::Expr, ast::Expr)],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let (mut key, mut value) = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Map(key, value)) => (Some(*key), Some(*value)),
            _ => (None, None),
        };
        let mut entries = Vec::new();
        let mut constants: Vec<(hir::Literal, Span)> = Vec::new();
        for (k, v) in pairs {
            let k = self.expr(cx, k, key);
            let k = match key {
                Some(key) => self.coerce(k, key),
                None => {
                    key = Some(k.ty);
                    k
                }
            };
            if let Some(constant) = literal(&k) {
                if constants.iter().any(|(c, _)| *c == constant) {
                    self.diags.error("E1116", k.span, "this key appears more than once in the literal");
                }
                constants.push((constant, k.span));
            }
            let v = self.expr(cx, v, value);
            let v = match value {
                Some(value) => self.coerce(v, value),
                None => {
                    value = Some(v.ty);
                    v
                }
            };
            entries.push((k, v));
        }
        let (Some(key), Some(value)) = (key, value) else {
            self.diags.error("E1101", span, "an empty map literal needs an expected `Map[K, V]` type");
            return Self::error_expr(span);
        };
        self.require_key(key, span);
        let ty = self.map_type(key, value);
        Self::mk(hir::ExprKind::Map(entries), ty, span)
    }

    pub(super) fn set_literal(
        &mut self,
        cx: &mut FnCx,
        items: &[ast::Expr],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Set(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        let mut constants = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            if let Some(constant) = literal(&value) {
                if constants.contains(&constant) {
                    self.diags.warning("W1011", value.span, "this entry appears earlier in the set: remove it");
                }
                constants.push(constant);
            }
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty set literal needs an expected `Set[T]` type");
            return Self::error_expr(span);
        };
        self.require_key(element, span);
        let ty = self.types.intern(TyKind::Set(element));
        Self::mk(hir::ExprKind::Set(values), ty, span)
    }

    pub(super) fn not_indexable(&mut self, base: hir::Expr, span: Span) -> hir::Expr {
        if base.ty != Ty::ERROR {
            let message = format!("{} cannot be indexed", self.display(base.ty));
            self.diags.error("E1102", base.span, message);
        }
        Self::error_expr(span)
    }

    pub(super) fn array_literal(
        &mut self,
        cx: &mut FnCx,
        items: &[ast::Expr],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let mut element = match expected.map(|t| self.types.kind(t)) {
            Some(TyKind::Array(element)) => Some(*element),
            _ => None,
        };
        let mut values = Vec::new();
        for item in items {
            let value = self.expr(cx, item, element);
            let value = match element {
                Some(element) => self.coerce(value, element),
                None => {
                    element = Some(value.ty);
                    value
                }
            };
            values.push(value);
        }
        let Some(element) = element else {
            self.diags.error("E1101", span, "an empty array literal needs an expected `Array[T]` type");
            return Self::error_expr(span);
        };
        let ty = self.types.intern(TyKind::Array(element));
        Self::mk(hir::ExprKind::Array(values), ty, span)
    }

    pub(super) fn unsupported_expr(&mut self, span: Span, what: &str) -> hir::Expr {
        self.diags.unsupported(span, what);
        Self::error_expr(span)
    }

    /// A literal, `-` before it when `negative`: the least value of a
    /// signed type is written so (18.8).
    pub(super) fn int_literal(
        &mut self,
        negative: bool,
        magnitude: u128,
        suffix: Option<&str>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(exact) =
                self.exact_literal(&format!("{}{magnitude}", if negative { "-" } else { "" }), target, span)
        {
            return exact;
        }
        // A literal where a program's `Number` or a `T: Number` is expected is
        // `Number.fromInt[T](literal)` (18.10).
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(number) = self.number
            && self.number_kind(target).is_none()
            && !matches!(self.types.kind(target), TyKind::Error | TyKind::Never)
            && self.satisfies(target, number, &[])
            && let Some(index) = self.traits[number.0 as usize].methods.iter().position(|m| m.name == "fromInt")
        {
            let literal = self.int_literal(negative, magnitude, None, Some(Ty::INT), span);
            let call = hir::ExprKind::Call(Callee::Trait(number, index as u32), vec![target], vec![literal]);
            return Self::mk(call, target, span);
        }
        // Where a `Byte` is expected, a literal from 0 to 255 is one (5.7).
        let unwrapped = expected.map(|t| match *self.types.kind(t) {
            TyKind::Option(inner) => inner,
            _ => t,
        });
        if suffix.is_none() && unwrapped == Some(Ty::BYTE) {
            if negative && magnitude != 0 || magnitude > 255 {
                let sign = if negative { "-" } else { "" };
                let message = format!("literal `{sign}{magnitude}` is not a `Byte`, which holds 0 to 255");
                self.diags.error("E1102", span, message);
            }
            return Self::mk(hir::ExprKind::Int(magnitude.min(255) as i128), Ty::BYTE, span);
        }
        let int = match suffix {
            Some(suffix) => IntTy::from_suffix(suffix),
            None => self.expected_number(expected, false).and_then(|t| self.types.as_int(t)).unwrap_or(IntTy::I64),
        };
        let fits = match negative {
            true => magnitude == 0 || (int.signed() && magnitude <= int.min().unsigned_abs()),
            false => magnitude <= int.max(),
        };
        if !fits {
            let sign = if negative { "-" } else { "" };
            let message = format!("literal `{sign}{magnitude}` does not fit in `{}`", int.name());
            self.diags.error("E1102", span, message);
        }
        let value = if negative { (magnitude as i128).wrapping_neg() } else { magnitude as i128 };
        Self::mk(hir::ExprKind::Int(value), Ty::int(int), span)
    }

    /// A literal where a `math.BigInt` or a `math.Decimal` is expected: that
    /// number, exact, from the literal's text (standard library 14.5).
    pub(super) fn exact_literal(&mut self, text: &str, target: Ty, span: Span) -> Option<hir::Expr> {
        let named =
            |this: &Self, name: &str| matches!(this.type_names.get(name), Some(&TypeName::Nominal(ty)) if ty == target);
        let maker = if named(self, "math.BigInt") && !text.contains(['.', 'e', 'E']) {
            "math.bigIntOf"
        } else if named(self, "math.Decimal") {
            "math.decimalOf"
        } else {
            return None;
        };
        let Some(&ModuleName::Fn(id)) = self.names.get(maker) else { return None };
        let literal = Self::mk(hir::ExprKind::Str(text.to_string()), Ty::STRING, span);
        Some(Self::mk(hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![literal]), target, span))
    }

    /// An integer literal wider than `u128`, which only a `math.BigInt` or a
    /// `math.Decimal` holds.
    pub(super) fn big_literal(&mut self, digits: &str, expected: Option<Ty>, span: Span) -> hir::Expr {
        if let Some(target) = expected
            && let Some(exact) = self.exact_literal(digits, target, span)
        {
            return exact;
        }
        let message = "integer literal is too large: a `UInt128` (`u128` suffix), the widest integer, holds up to 340282366920938463463374607431768211455, and a `math.BigInt` any integer";
        self.diags.error("E0003", span, message);
        Self::error_expr(span)
    }

    /// The integer or float type an unsuffixed literal takes from its
    /// context: the expected type, or the only such member of an expected
    /// union or option.
    pub(super) fn expected_number(&self, expected: Option<Ty>, float: bool) -> Option<Ty> {
        let fits = |t: &Ty| match self.types.kind(*t) {
            TyKind::Int(_) => !float,
            TyKind::Float(_) => float,
            _ => false,
        };
        let mut ty = expected?;
        if let TyKind::Option(inner) = self.types.kind(ty) {
            ty = *inner;
        }
        match self.types.members(ty) {
            Some(members) => match members.iter().filter(|t| fits(t)).collect::<Vec<_>>().as_slice() {
                [only] => Some(**only),
                _ => None,
            },
            None => fits(&ty).then_some(ty),
        }
    }

    pub(super) fn float_literal(
        &mut self,
        text: &str,
        suffix: Option<&str>,
        negative: bool,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if suffix.is_none()
            && let Some(target) = expected
            && let Some(exact) = self.exact_literal(&format!("{}{text}", if negative { "-" } else { "" }), target, span)
        {
            return exact;
        }
        let ty = match suffix {
            Some("f32") => Ty::F32,
            Some(_) => Ty::F64,
            None if self.expected_number(expected, true) == Some(Ty::F32) => Ty::F32,
            None => Ty::F64,
        };
        // Parse in the target format so rounding happens once (5.8).
        let value = if ty == Ty::F32 { text.parse::<f32>().map(f64::from) } else { text.parse::<f64>() };
        let value = value.unwrap_or(f64::INFINITY);
        if value.is_infinite() {
            let message = format!("literal `{text}` does not fit in {}", self.display(ty));
            self.diags.error("E1102", span, message);
        }
        Self::mk(hir::ExprKind::Float(if negative { -value } else { value }), ty, span)
    }

    /// A comparison asserted, with each side bound first so that a failure
    /// can show both (15.8): the bindings, the condition on them and what
    /// the failure adds. Sides that cannot be displayed are left alone.
    pub(super) fn compared(
        &mut self,
        cx: &mut FnCx,
        condition: hir::Expr,
    ) -> (Vec<hir::Stmt>, hir::Expr, Option<Box<hir::Expr>>) {
        let hir::ExprKind::Binary(op, left, right) = &condition.kind else {
            return (Vec::new(), condition, None);
        };
        let comparison = matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge);
        // `a < b` of an `Ordered` type is `compare(a, b) < 0`: its sides are
        // the call's.
        let ordered = match (&left.kind, &right.kind) {
            (hir::ExprKind::Call(Callee::Trait(t, _), _, args), hir::ExprKind::Int(0))
                if Some(*t) == self.ordered && args.len() == 2 =>
            {
                Some((args[0].ty, args[1].ty))
            }
            _ => None,
        };
        let (left_ty, right_ty) = ordered.unwrap_or((left.ty, right.ty));
        let shown = |this: &mut Self, ty: Ty| !this.types.has_error(ty) && this.satisfies(ty, DISPLAY, &[]);
        if !comparison || !shown(self, left_ty) || !shown(self, right_ty) {
            return (Vec::new(), condition, None);
        }
        let hir::ExprKind::Binary(op, left, right) = condition.kind else { unreachable!() };
        let (span, ty) = (condition.span, condition.ty);
        let (zero, call) = (*right, *left);
        let (left, right, rebuild) = match (ordered, call.kind) {
            (Some(_), hir::ExprKind::Call(callee, generics, args)) => {
                let [a, b] = <[hir::Expr; 2]>::try_from(args).ok().expect("two arguments");
                (Box::new(a), Box::new(b), Some((callee, generics, call.ty, call.span)))
            }
            (_, kind) => (Box::new(Self::mk(kind, call.ty, call.span)), Box::new(zero.clone()), None),
        };
        let mut sides = Vec::new();
        let mut reads = Vec::new();
        let mut segments = Vec::new();
        for (label, side) in [("\n  left:  ", *left), ("\n  right: ", *right)] {
            let (side_ty, side_span) = (side.ty, side.span);
            let local = self.hidden_local(cx, side_ty);
            sides.push(hir::Stmt::Let(local, side));
            let read = Self::mk(hir::ExprKind::Local(local), side_ty, side_span);
            reads.push(read.clone());
            segments.push(hir::Segment::Text(label.into()));
            // Quotes show where a string starts and ends.
            let quoted = side_ty == Ty::STRING;
            if quoted {
                segments.push(hir::Segment::Text("\"".into()));
            }
            segments.push(hir::Segment::Value(self.displayed(read)));
            if quoted {
                segments.push(hir::Segment::Text("\"".into()));
            }
        }
        let [left, right] = <[hir::Expr; 2]>::try_from(reads).ok().expect("two sides");
        let (left, right) = match rebuild {
            Some((callee, generics, call_ty, call_span)) => {
                let call = Self::mk(hir::ExprKind::Call(callee, generics, vec![left, right]), call_ty, call_span);
                (call, zero)
            }
            None => (left, right),
        };
        let condition = Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span);
        let detail = Self::mk(hir::ExprKind::Interpolate(segments), Ty::STRING, span);
        (sides, condition, Some(Box::new(detail)))
    }

    /// The field of a template's data that a name which is no binding
    /// reads, if it has one: the closure of a compiled template, or one in
    /// it, sees the fields of its `self` (standard library 14.31).
    pub(super) fn template_field(&self, cx: &FnCx, name: &str) -> Option<String> {
        // The linker names another module's functions after it.
        let bare = name.rsplit('.').next().unwrap_or(name);
        let (_, fields) = self.template_data(cx)?;
        fields.contains(&bare).then(|| bare.to_string())
    }

    /// The type of the data of the template whose code is checked, and
    /// the names of its fields.
    pub(super) fn template_data(&self, cx: &FnCx) -> Option<(Ty, Vec<&str>)> {
        let mut at = Some(cx);
        let data = loop {
            let here = at?;
            if here.template {
                let local = self.lookup(here, "self")?;
                break here.locals[local.0 as usize].ty;
            }
            at = here.parent.as_deref();
        };
        let fields = match self.types.kind(data) {
            TyKind::Adt(id) => match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => fields.iter().map(|f| f.name.as_str()).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        Some((data, fields))
    }

    /// `template.html[T](...)` or `template.text[T](...)` with the code of
    /// its template, which the compiler wrote from its text (standard
    /// library 14.31): a `Template[T]` that holds the closure that writes
    /// it.
    pub(super) fn compiled_template(&mut self, cx: &mut FnCx, args: &[ast::Arg], result: Ty, span: Span) -> hir::Expr {
        let Some(ast::ExprKind::Template(body)) = args.first().map(|a| &a.value.kind) else {
            let message = "a template compiled with the program has its text when the program compiles: write it as a raw string, `r\"...\"`, or read it with `embed.text(\"...\")`; a `template.Set` reads templates while the program runs";
            self.diags.error("E1906", args.first().map_or(span, |a| a.value.span), message);
            return Self::error_expr(span);
        };
        let data = match self.types.kind(result) {
            TyKind::Adt(id) => {
                self.types.adt(*id).instance.as_ref().and_then(|(_, args)| args.first().copied()).unwrap_or(Ty::ERROR)
            }
            _ => Ty::ERROR,
        };
        let render = self.template_closure(cx, body, data, span);
        let ty = self.types.intern(TyKind::Fn(vec![data], Ty::STRING, false, vec![ast::Mode::Value], false));
        let render = self.coerce(render, ty);
        Self::mk(hir::ExprKind::Record(vec![(0, render)]), result, span)
    }

    /// The closure that writes a compiled template for its data, `self`:
    /// checked in a context of its own, which captures nothing.
    pub(super) fn template_closure(&mut self, cx: &mut FnCx, body: &ast::Block, data: Ty, span: Span) -> hir::Expr {
        let param = ast::ClosureParam {
            name: ast::Ident { name: "self".into(), span },
            ty: None,
            mode: ast::Mode::Value,
            variadic: false,
        };
        let expected = self.types.intern(TyKind::Fn(vec![data], Ty::STRING, false, vec![ast::Mode::Value], false));
        let kind = ClosureKind { deferred: false, is_unsafe: false, test: false, infer_error: false };
        self.templating = true;
        self.closure_with(cx, (&[param], None), Some(expected), kind, span, |this, inner, _| {
            let name = ast::Ident { name: crate::templates::OUT.into(), span };
            let out = this.declare(inner, &name, Ty::STRING, true);
            let empty = Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
            let written = this.block(inner, body, Some(Ty::UNIT));
            let written = Self::mk(hir::ExprKind::Block(written), Ty::UNIT, span);
            let tail = Self::mk(hir::ExprKind::Local(out), Ty::STRING, span);
            hir::Block {
                stmts: vec![hir::Stmt::Let(out, empty), hir::Stmt::Expr(written)],
                tail: Some(Box::new(tail)),
                ty: Ty::STRING,
            }
        })
    }

    /// Another template a compiled one writes, with its data, with values
    /// by name, or with the same `self`: a closure of its own, called here.
    /// Values by name are a record of their own, which the included
    /// template reads by those names.
    pub(super) fn template_include(
        &mut self,
        cx: &mut FnCx,
        data: Option<&ast::Expr>,
        named: &[(ast::Ident, ast::Expr)],
        body: &ast::Block,
        span: Span,
    ) -> hir::Expr {
        let data = match data {
            Some(data) => self.expr(cx, data, None),
            None if !named.is_empty() => {
                let values: Vec<hir::Expr> = named.iter().map(|(_, value)| self.expr(cx, value, None)).collect();
                if values.iter().any(|value| value.ty == Ty::ERROR) {
                    return Self::error_expr(span);
                }
                let fields: Vec<Field> = named
                    .iter()
                    .zip(&values)
                    .map(|((name, _), value)| Field {
                        name: name.name.clone(),
                        ty: value.ty,
                        private: false,
                        secret: false,
                    })
                    .collect();
                let shown: Vec<String> =
                    fields.iter().map(|f| format!("{}: {}", f.name, self.types.display(f.ty))).collect();
                let (id, ty) = self.types.declare_adt(format!("{{ {} }}", shown.join(", ")));
                self.types.define_adt(id, AdtKind::Record(fields));
                let values = values.into_iter().enumerate().map(|(index, value)| (index as u32, value)).collect();
                Self::mk(hir::ExprKind::Record(values), ty, span)
            }
            None => self.path(cx, &[ast::Ident { name: "self".into(), span }], None, span),
        };
        let closure = self.template_closure(cx, body, data.ty, span);
        let local = self.hidden_local(cx, closure.ty);
        let read = Self::mk(hir::ExprKind::Local(local), closure.ty, span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), vec![data]), Ty::STRING, span);
        let block =
            hir::Block { stmts: vec![hir::Stmt::Let(local, closure)], tail: Some(Box::new(call)), ty: Ty::STRING };
        Self::mk(hir::ExprKind::Block(block), Ty::STRING, span)
    }

    /// A value a compiled template writes, with its format, and escaped for
    /// its place of HTML: in a script as its JSON, which `Encode` gives; in
    /// any other place, its text, but a `template.Html` in the text of HTML.
    pub(super) fn template_write(
        &mut self,
        cx: &mut FnCx,
        value: &ast::Expr,
        format: Option<&ast::Format>,
        place: Option<u8>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, value, None);
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let place_code = |place: u8| Self::mk(hir::ExprKind::Int(place.into()), Ty::INT, span);
        let function = |this: &Self, name: &str| match this.names.get(name) {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        };
        // Code of a script: the value as JSON.
        if let Some(place @ (6 | 7)) = place {
            let encodes = match self.trait_names.get("serialization.Encode") {
                Some(&encode) => self.satisfies(value.ty, encode, &[]),
                None => false,
            };
            if !encodes {
                let message = format!(
                    "{} does not implement `serialization.Encode`, which a value in a script needs, as its JSON: write one of its fields, or a value of a record, enum or newtype",
                    self.display(value.ty)
                );
                self.diags.error("E1105", value.span, message);
                return Self::error_expr(span);
            }
            let Some(id) = function(self, "template.scriptValue") else { return Self::error_expr(span) };
            let ty = value.ty;
            let call = hir::ExprKind::Call(Callee::Fn(id), vec![ty], vec![value, place_code(place)]);
            return Self::mk(call, Ty::STRING, span);
        }
        if !self.satisfies(value.ty, DISPLAY, &[]) {
            let message = format!(
                "{} does not implement `Display`, so a template cannot write it: write one of its fields, as `{{{{ post.title }}}}`",
                self.display(value.ty)
            );
            self.diags.error("E1105", value.span, message);
            return Self::error_expr(span);
        }
        let trusted =
            matches!(self.type_names.get("template.Html"), Some(&TypeName::Nominal(html)) if html == value.ty);
        let shown = match format {
            Some(format) => self.formatted(value, format),
            None => self.displayed(value),
        };
        let text = Self::mk(hir::ExprKind::Interpolate(vec![hir::Segment::Value(shown)]), Ty::STRING, span);
        match place {
            None => text,
            Some(0) if trusted => text,
            Some(place) => {
                let Some(id) = function(self, "template.escape") else { return Self::error_expr(span) };
                let call = hir::ExprKind::Call(Callee::Fn(id), Vec::new(), vec![text, place_code(place)]);
                Self::mk(call, Ty::STRING, span)
            }
        }
    }

    /// A value to interpolate: a user type or a type parameter through
    /// `Display.display`, any other as it is.
    pub(super) fn displayed(&mut self, value: hir::Expr) -> hir::Expr {
        if matches!(self.types.kind(value.ty), TyKind::Adt(_) | TyKind::Param(_) | TyKind::Opaque(..)) {
            let (ty, span) = (value.ty, value.span);
            let call = hir::ExprKind::Call(Callee::Trait(DISPLAY, 0), vec![ty], vec![value]);
            Self::mk(call, Ty::STRING, span)
        } else {
            value
        }
    }

    /// A value written with the format of its interpolation (language
    /// 19.6), by the prelude's function for an integer, a float or the
    /// text of any other value.
    pub(super) fn formatted(&mut self, value: hir::Expr, format: &ast::Format) -> hir::Expr {
        let (ty, span) = (value.ty, value.span);
        let integer = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Byte) || self.bound_by(ty, self.integer);
        let float = matches!(self.types.kind(ty), TyKind::Float(_));
        // A program's number, as a `math.Decimal`, takes a float's options
        // but scientific notation, on the digits of its `Display`.
        let numeric = !integer
            && !float
            && ty != Ty::STRING
            && !self.types.has_error(ty)
            && self.number.is_some_and(|number| self.satisfies(ty, number, &[]));
        let base = format.spec.kind.filter(|k| "xXob".contains(*k));
        let scientific = format.spec.kind.filter(|k| "eE".contains(*k));
        let refused = [
            (format.spec.plus && !integer && !float && !numeric, "a sign".to_string()),
            (format.spec.zero && !integer && !float && !numeric, "`0`".into()),
            (format.spec.alternate && base.is_none() && integer, "`#` without `x`, `X`, `o` or `b`".into()),
            (format.spec.alternate && !integer, "`#`".into()),
            (
                format.spec.precision.is_some() && integer,
                format!("a precision, `.{}`", format.spec.precision.unwrap_or(0)),
            ),
            (base.is_some() && !integer, format!("the base `{}`", base.unwrap_or('x'))),
            (scientific.is_some() && !float, format!("scientific notation, `{}`", scientific.unwrap_or('e'))),
        ];
        if let Some((_, option)) = refused.iter().find(|(refused, _)| *refused) {
            let takes = if integer {
                "`[[fill]align][+][#][0][width][x|X|o|b]`"
            } else if float {
                "`[[fill]align][+][0][width][.precision][e|E]`"
            } else {
                "`[[fill]align][width][.precision]`"
            };
            let message = format!("a format of {} cannot have {option}: write {takes}", self.display(ty));
            self.diags.error("E1118", format.span, message);
            return self.displayed(value);
        }
        let int = |n: i128| Self::mk(hir::ExprKind::Int(n), Ty::INT, span);
        let flag = |b: bool| Self::mk(hir::ExprKind::Bool(b), Ty::BOOL, span);
        let fill = Self::mk(hir::ExprKind::Char(format.spec.fill), Ty::CHAR, span);
        let align = |default: char| Self::mk(hir::ExprKind::Char(format.spec.align.unwrap_or(default)), Ty::CHAR, span);
        let width = int(format.spec.width.unwrap_or(0).into());
        let precision = int(format.spec.precision.map_or(-1, i128::from));
        let upper = format.spec.kind.is_some_and(|k| k.is_ascii_uppercase());
        let (name, generics, args) = if integer {
            let (value, ty) = match self.types.kind(ty) {
                TyKind::Byte => (Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::INT, span), Ty::INT),
                _ => (value, ty),
            };
            let radix = match base {
                Some('x' | 'X') => 16,
                Some('o') => 8,
                Some('b') => 2,
                _ => 10,
            };
            let (plus, zero, prefixed) = (flag(format.spec.plus), flag(format.spec.zero), flag(format.spec.alternate));
            let args = vec![value, int(radix), flag(upper), prefixed, plus, zero, width, fill, align('>')];
            ("formats.formatInteger", vec![ty], args)
        } else if float {
            let single = flag(ty == Ty::F32);
            let value = match ty == Ty::F32 {
                true => Self::mk(hir::ExprKind::Convert(Box::new(value)), Ty::F64, span),
                false => value,
            };
            let (exponent, plus, zero) = (flag(scientific.is_some()), flag(format.spec.plus), flag(format.spec.zero));
            let args = vec![value, single, precision, exponent, flag(upper), plus, zero, width, fill, align('>')];
            ("formats.formatFloat", Vec::new(), args)
        } else if numeric {
            let shown = Self::mk(
                hir::ExprKind::Interpolate(vec![hir::Segment::Value(self.displayed(value))]),
                Ty::STRING,
                span,
            );
            let (plus, zero) = (flag(format.spec.plus), flag(format.spec.zero));
            ("formats.formatNumeric", Vec::new(), vec![shown, precision, plus, zero, width, fill, align('>')])
        } else {
            let text = match ty == Ty::STRING {
                true => value,
                false => {
                    let shown = hir::ExprKind::Interpolate(vec![hir::Segment::Value(self.displayed(value))]);
                    Self::mk(shown, Ty::STRING, span)
                }
            };
            ("formats.formatText", Vec::new(), vec![text, precision, width, fill, align('<')])
        };
        let Some(&ModuleName::Fn(function)) = self.names.get(name) else { return Self::error_expr(span) };
        Self::mk(hir::ExprKind::Call(Callee::Fn(function), generics, args), Ty::STRING, span)
    }

    /// `sql.Query`, when the program has `std.sql`.
    pub(super) fn query_type(&self) -> Option<Ty> {
        match self.type_names.get("sql.Query") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        }
    }

    /// A string literal where a `sql.Query` is expected: its text is SQL,
    /// each value it interpolates a parameter, a query spliced in, and an
    /// array the list of its elements (standard library 14.37), so a value
    /// never becomes SQL.
    pub(super) fn query_literal(
        &mut self,
        cx: &mut FnCx,
        segments: &[ast::StrSegment],
        query: Ty,
        span: Span,
    ) -> hir::Expr {
        let function = |this: &Self, name: &str| match this.names.get(name) {
            Some(&ModuleName::Fn(id)) => Some(id),
            _ => None,
        };
        let (Some(made), Some(text), Some(nested), Some(bound)) = (
            function(self, "sql.made"),
            function(self, "sql.literalText"),
            function(self, "sql.nested"),
            function(self, "sql.bound"),
        ) else {
            return Self::error_expr(span);
        };
        let piece = self.signatures[text.0 as usize].result;
        let encode = self.trait_names.get("serialization.Encode").copied();
        let mut pieces = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(written) => {
                    let literal = Self::mk(hir::ExprKind::Str(written.clone()), Ty::STRING, span);
                    pieces.push(Self::mk(
                        hir::ExprKind::Call(Callee::Fn(text), Vec::new(), vec![literal]),
                        piece,
                        span,
                    ));
                }
                ast::StrSegment::Expr(e, format) => {
                    if let Some(format) = format {
                        let message = "a value of a query is a parameter, which a format does not change: format it before, or drop the format";
                        self.diags.error("E1118", format.span, message);
                    }
                    let value = self.expr(cx, e, None);
                    if value.ty == query {
                        pieces.push(Self::mk(
                            hir::ExprKind::Call(Callee::Fn(nested), Vec::new(), vec![value]),
                            piece,
                            span,
                        ));
                        continue;
                    }
                    // Bytes are one parameter, not a list of numbers.
                    let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                    let optional = self.types.intern(TyKind::Option(bytes));
                    let binary = match value.ty {
                        ty if ty == bytes => function(self, "sql.boundBytes"),
                        ty if ty == optional => function(self, "sql.boundOptionalBytes"),
                        _ => None,
                    };
                    if let Some(binary) = binary {
                        pieces.push(Self::mk(
                            hir::ExprKind::Call(Callee::Fn(binary), Vec::new(), vec![value]),
                            piece,
                            span,
                        ));
                        continue;
                    }
                    if !self.types.has_error(value.ty)
                        && let Some(encode) = encode
                        && !self.satisfies(value.ty, encode, &[])
                    {
                        let message = format!(
                            "a value a query interpolates is a parameter, and {} is not one: a parameter is a number, a text, bytes, a time, an option or an array of them; interpolate a field, or its JSON as a text",
                            self.display(value.ty)
                        );
                        self.diags.error("E1105", e.span, message);
                        continue;
                    }
                    let ty = value.ty;
                    pieces.push(Self::mk(hir::ExprKind::Call(Callee::Fn(bound), vec![ty], vec![value]), piece, span));
                }
            }
        }
        let array = self.types.intern(TyKind::Array(piece));
        let list = Self::mk(hir::ExprKind::Array(pieces), array, span);
        Self::mk(hir::ExprKind::Call(Callee::Fn(made), Vec::new(), vec![list]), query, span)
    }

    pub(super) fn string(&mut self, cx: &mut FnCx, segments: &[ast::StrSegment], span: Span) -> hir::Expr {
        if let [ast::StrSegment::Text(text)] = segments {
            return Self::mk(hir::ExprKind::Str(text.clone()), Ty::STRING, span);
        }
        let mut out = Vec::new();
        for segment in segments {
            match segment {
                ast::StrSegment::Text(text) => out.push(hir::Segment::Text(text.clone())),
                ast::StrSegment::Expr(e, format) => {
                    // Only a lone name in braces may have meant the braces.
                    let lone = matches!(&e.kind, ast::ExprKind::Path(segments) if segments.len() == 1);
                    let outer = std::mem::replace(&mut self.interpolating, lone);
                    let value = self.expr(cx, e, None);
                    self.interpolating = outer;
                    if !self.satisfies(value.ty, DISPLAY, &[]) {
                        let mut message = format!("{} does not implement `Display`", self.display(value.ty));
                        if let Some(members) = self.types.members(value.ty).map(<[Ty]>::to_vec) {
                            let lacking: Vec<Ty> =
                                members.into_iter().filter(|&m| !self.satisfies(m, DISPLAY, &[])).collect();
                            let lacking: Vec<String> =
                                lacking.into_iter().map(|m| format!("`{}`", self.types.display(m))).collect();
                            message.push_str(&format!(
                                ": {} does not; `match` on its members to format each one",
                                lacking.join(" and ")
                            ));
                        } else if let TyKind::Handle(Handle::Ref, _) = self.types.kind(value.ty) {
                            message.push_str(": an identity has no text; show what it identifies with `.value`");
                        } else if self.iterator_element(value.ty).is_some() {
                            message.push_str(
                                ": it is an iterator, and `collect()` makes an array of its elements, which shows them",
                            );
                            let end = Span::new(value.span.end, value.span.end);
                            self.diags.error("E1105", value.span, message);
                            self.diags.fix("show `collect()`", false, vec![(end, ".collect()".into())]);
                            out.push(hir::Segment::Value(value));
                            continue;
                        } else if matches!(self.types.kind(value.ty), TyKind::Adt(_))
                            && self.owner(value.ty).is_some_and(|o| o.module == self.module)
                        {
                            let name = self.types.display(value.ty);
                            message.push_str(&format!(
                                ": add `impl Display for {name} {{ fn display(self): String {{ ... }} }}`"
                            ));
                        } else if matches!(self.types.kind(value.ty), TyKind::Adt(_)) {
                            // Another module's: only that module may implement it.
                            message.push_str(": another module declares it, so write what to show of it, its fields or a `match` on it");
                        }
                        self.diags.error("E1105", value.span, message);
                        out.push(hir::Segment::Value(value));
                        continue;
                    }
                    let value = match format {
                        Some(format) => self.formatted(value, format),
                        None => self.displayed(value),
                    };
                    out.push(hir::Segment::Value(value));
                }
            }
        }
        Self::mk(hir::ExprKind::Interpolate(out), Ty::STRING, span)
    }

    pub(super) fn path(
        &mut self,
        cx: &mut FnCx,
        segments: &[ast::Ident],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let first = &segments[0];
        let captured = match self.lookup(cx, &first.name) {
            Some(_) => None,
            None => self.capture(cx, &first.name, first.span),
        };
        if let Some(mut value) = captured {
            self.index_local(cx, first);
            for segment in &segments[1..] {
                self.index_expr(value.span, value.ty);
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        if let Some(local) = self.lookup(cx, &first.name) {
            self.index_local(cx, first);
            cx.reads.insert(local);
            let ty = cx.locals[local.0 as usize].ty;
            let mut value = Self::mk(hir::ExprKind::Local(local), ty, first.span);
            for segment in &segments[1..] {
                self.index_expr(value.span, value.ty);
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            if let Some(place) = hir::Place::of(value.clone())
                && let Some((lent, true)) = lent_conflict(cx, &place)
            {
                let lent = self.place_name(cx, lent);
                self.lent_error(&lent, true, value.span);
            }
            return value;
        }
        // In a template, a name that is no binding is a field of its data,
        // before a function or a constant (standard library 14.31).
        if let Some(field) = self.template_field(cx, &first.name) {
            let mut written = vec![
                ast::Ident { name: "self".into(), span: first.span },
                ast::Ident { name: field, span: first.span },
            ];
            written.extend(segments[1..].iter().cloned());
            return self.path(cx, &written, expected, span);
        }
        // The constants of the intrinsic numeric types: `Int.max`, `Float.pi`.
        if let [type_name, constant] = segments
            && matches!(self.names.get(&type_name.name), None | Some(ModuleName::Fn(_)))
            && let Some(ty) = self.intrinsic_owner(&type_name.name)
            && let Some(kind) = self.intrinsic_constant(ty, &constant.name)
        {
            let ty = if matches!(kind, hir::ExprKind::Int(_)) && constant.name == "bits" { Ty::INT } else { ty };
            return Self::mk(kind, ty, span);
        }
        // `T.min`, `T.max` and `T.bits` of a `T: Integer` (18.10).
        if let [type_name, constant] = segments
            && matches!(constant.name.as_str(), "min" | "max" | "bits")
            && self.lookup(cx, &type_name.name).is_none()
            && let Some(ty) = self.type_named(&type_name.name)
            && self.bound_by(ty, self.integer)
        {
            use crate::numbers::NumOp;
            let constant_of = |op: NumOp| Callee::Std(StdFn::Number(op));
            let least = Self::mk(hir::ExprKind::Call(constant_of(NumOp::Least), Vec::new(), Vec::new()), ty, span);
            return match constant.name.as_str() {
                "min" => least,
                "max" => Self::mk(hir::ExprKind::Call(constant_of(NumOp::Greatest), Vec::new(), Vec::new()), ty, span),
                _ => Self::mk(hir::ExprKind::Call(constant_of(NumOp::Width), Vec::new(), vec![least]), Ty::INT, span),
            };
        }
        // `Context.field`: its value where the code runs (11.16).
        if let [name, field, rest @ ..] = segments
            && let Some((ty, context)) = self.context_named(std::slice::from_ref(name))
        {
            let reference = self.types.handle(Handle::Ref, ty);
            // For an editor: the `with` that gave the field its value here.
            if let Some(at) =
                self.record_fields(ty, field.span).and_then(|f| f.iter().position(|(n, _)| *n == field.name))
            {
                let bit = 1u64 << at.min(63);
                let set = cx.withs.iter().rev().find(|&&(c, fields, _)| c == context && fields & bit != 0).map(|w| w.2);
                if let Some(index) = self.index.as_mut() {
                    index.contexts.push((field.span, set));
                }
                if let Some(reads) = self.initial_reads.as_mut() {
                    reads.push((context, at as u32));
                }
            }
            let mut value = Self::mk(hir::ExprKind::ContextRead(context, reference), ty, name.span);
            for segment in std::iter::once(field).chain(rest) {
                let span = value.span.to(segment.span);
                value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
            }
            return value;
        }
        // `Type.operation` as a value: an associated operation (11.7).
        if let [type_name, operation] = segments
            && matches!(self.names.get(&type_name.name), None | Some(ModuleName::Fn(_)))
            && let Some(owner) = self.type_named(&type_name.name)
            && let Some(&id) = self.methods.get(&(owner, operation.name.clone()))
        {
            self.index_refer(type_name, crate::index::Key::Type(type_name.name.clone()));
            self.index_refer(operation, crate::index::Key::Fn(id.0));
            self.check_method_visible(id, operation.span);
            return self.fn_value(id, Vec::new(), expected, span);
        }
        if let Some(resolved) = self.variant_path(segments) {
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            if let Payload::Unit = self.payload(ty, variant) {
                let ty = match self.generic_inference(ty, expected) {
                    Some(inference) => self.finish_inference(&inference, ty, span),
                    None => ty,
                };
                return Self::mk(hir::ExprKind::Variant(variant, Vec::new()), ty, span);
            }
            let message = format!("`{}` needs a payload", Self::path_text(segments));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        match self.names.get(&first.name) {
            Some(&ModuleName::Fn(id)) if Some(id) == self.main_fn() => self.main_called(span),
            Some(&ModuleName::Fn(id)) if segments.len() == 1 => {
                self.index_refer(first, crate::index::Key::Fn(id.0));
                self.fn_value(id, Vec::new(), expected, span)
            }
            Some(&ModuleName::Const(id)) => {
                self.index_refer(first, crate::index::Key::Value(first.name.clone()));
                let mut value = self.const_value(id, first.span);
                for segment in &segments[1..] {
                    let span = value.span.to(segment.span);
                    value = self.field_access(value, FieldRef::Name(&segment.name), segment.span, span);
                }
                value
            }
            Some(&ModuleName::Extern(id)) if segments.len() == 1 => {
                let declared = &self.externs[id as usize];
                let (params, result, variadic) = (declared.params.clone(), declared.result, declared.variadic);
                let ty = self.types.intern(TyKind::CFn(params, result, variadic));
                Self::mk(hir::ExprKind::ExternFn(id), ty, span)
            }
            Some(ModuleName::Extern(_)) => {
                self.diags.error("E1102", span, "a function has no fields");
                Self::error_expr(span)
            }
            Some(ModuleName::Fn(_)) => {
                self.diags.error("E1102", span, "a function has no fields");
                Self::error_expr(span)
            }
            Some(ModuleName::Newtype(_)) => {
                let message = format!("`{}` constructs a value: write `{}(value)`", first.name, first.name);
                self.diags.error("E1102", span, message);
                Self::error_expr(span)
            }
            Some(ModuleName::Module(_)) => {
                let (code, message) = match segments {
                    [module] => ("E1102", format!("`{}` is a module, not a value", module.name)),
                    [module, member] => ("E1001", format!("module `{}` has no value `{}`", module.name, member.name)),
                    [module, member, ..] => {
                        ("E1001", format!("module `{}` has no type `{}`", module.name, member.name))
                    }
                    [] => unreachable!("a path has a segment"),
                };
                self.diags.error(code, span, message);
                Self::error_expr(span)
            }
            None => {
                self.unknown_path(cx, segments, span);
                Self::error_expr(span)
            }
        }
    }

    /// Reports a path whose first name is unknown as a value: a method
    /// called on its type, as `Array.repeat(...)`, or an unknown name.
    pub(super) fn unknown_path(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) {
        let first = &segments[0];
        if segments.len() > 1 && (self.intrinsic_owner(&first.name).is_some() || self.type_named(&first.name).is_some())
        {
            let (owner, member) = (&first.name, &segments[1].name);
            let message = format!(
                "`{owner}` has no function `{member}`; if `{member}` is a method, call it on a value: `value.{member}(...)`"
            );
            self.diags.error("E1001", span, message);
        } else {
            self.unknown_name(cx, &first.name, first.span);
        }
    }

    /// Offers to write the name the last diagnostic says the program may
    /// have meant, in place of `span`.
    pub(super) fn offer_meant(&mut self, span: Span) {
        let Some(last) = self.diags.list.last() else { return };
        let Some(near) = last.message.split("did you mean `").nth(1).and_then(|rest| rest.split('`').next()) else {
            return;
        };
        let near = near.to_string();
        self.diags.fix(format!("write `{near}`"), false, vec![(span, near)]);
    }

    /// Reports `E1001`, pointing habits from other languages at the canonical form.
    pub(super) fn unknown_name(&mut self, cx: &FnCx, name: &str, span: Span) {
        // A module of the program this file does not import.
        let here = self.sources.file_of(span.start).module.clone();
        let module = self
            .module_paths
            .iter()
            .filter(|p| p.first().is_some_and(|first| first != "std") && p.last().is_some_and(|last| last == name))
            .map(|p| p.join("."))
            .find(|path| *path != here);
        let hint = match name {
            "Some" | "Ok" | "Err" => format!("Tondo writes `{}(...)`", name.to_lowercase()),
            "None" | "null" | "nil" | "undefined" | "NULL" | "nullptr" | "Nothing" => {
                "Tondo writes `none`, the `T?` without a value".into()
            }
            "True" | "TRUE" => "Tondo writes `true`".into(),
            "False" | "FALSE" => "Tondo writes `false`".into(),
            "println" | "print" | "printf" | "puts" | "echo" | "printLn" | "log" => {
                "`console.println(text)` writes a line, after `import std.console`".into()
            }
            "range" => "a range is `0..10`, or `0..=10` with its end".into(),
            "len" | "length" | "size" | "count" | "sizeof" | "strlen" => "use `value.length()`".into(),
            "self" => "a method takes `self` as its first parameter: `fn Type.name(self)`".into(),
            "this" => "a method's receiver is `self`, its first parameter: `fn Type.name(self)`".into(),
            "await" => {
                "there is no `await`: call the function, which waits, or `spawn f()` and later `wait()` its `Join`"
                    .into()
            }
            "HashMap" | "Dictionary" | "dict" | "Dict" | "TreeMap" | "BTreeMap" => {
                "a map is `Map[K, V]`: `[:]` is an empty one, `[\"a\": 1]` one with an entry".into()
            }
            "Vec" | "List" | "ArrayList" | "vector" | "Vector" | "list" => {
                "an array is `Array[T]`: `[]` is an empty one, `[1, 2]` one with elements".into()
            }
            "HashSet" | "TreeSet" | "BTreeSet" => "a set is `Set[T]`, as in `Set[1, 2]`".into(),
            "str" | "toString" | "string" => "interpolate the value: `\"{value}\"`".into(),
            "int" | "float" => format!("a conversion is a call of the type: `{}(value)`", capitalized(name)),
            "assert_eq" | "assertEquals" | "assertEqual" | "expect" => {
                "`assert(actual == expected)` shows both sides when it fails".into()
            }
            "sleep" => "`time.sleep(time.milliseconds(10))` waits, after `import std.time`".into(),
            "exit" => "`process.exit(code)` ends the program, after `import std.process`".into(),
            "recover" | "catch_unwind" | "rescue" => {
                "`recover { ... }` stops a panic of its block: `ok` with its value, or `err(p)` with the `Panic`".into()
            }
            _ if crate::api::standard_modules().contains(&format!("std.{name}")) => {
                format!("`{name}` is a standard module: add `import std.{name}` at the top of the file")
            }
            _ if let Some(found) = self.elsewhere.iter().find(|e| e.name == name && e.module == here) => {
                let system = crate::driver::system();
                format!(
                    "`{name}` is in `{}`, for {}: write it for {} too, in a file `*_{}.to` or a common one, as `{}`",
                    found.file,
                    found.systems,
                    system.shown(),
                    system.name(),
                    found.head
                )
            }
            _ if module.is_some() => {
                format!(
                    "`{name}` is the module `{}`: add `import {}` at the top of the file",
                    module.as_deref().unwrap_or_default(),
                    module.as_deref().unwrap_or_default()
                )
            }
            _ if self.script_bindings.iter().any(|bound| bound == name) => match self.scripting {
                true => format!("the script declares `{name}` further down: a binding is seen from its `let` on"),
                false => format!(
                    "`{name}` is a binding of the script, which its functions do not see: pass it to the function as a parameter"
                ),
            },
            _ if self.interpolating => format!(
                "inside a string, `{{{name}}}` interpolates it: write `{{{{{name}}}}}` for the braces themselves"
            ),
            _ if let Some((data, fields)) = self.template_data(cx) => {
                let near =
                    closest(name, fields.iter().copied()).map(|n| format!("did you mean `{n}`? ")).unwrap_or_default();
                let fields: Vec<String> = fields.iter().map(|f| format!("`{f}`")).collect();
                match fields.is_empty() {
                    true => format!("it is no binding, and the template's data, {}, has no fields", self.display(data)),
                    false => format!(
                        "{near}it is no binding, nor a field of the template's data, {}, whose fields are {}",
                        self.display(data),
                        fields.join(", ")
                    ),
                }
            }
            // A declaration of a standard module, written without it.
            _ if !crate::api::declaring(name).is_empty() => {
                let owners = self.declaring(span, name);
                let first = &owners[0];
                let written = format!("{}.{name}", self.imported_as(span, first));
                let imported = self.imported(span, first);
                match owners.len() {
                    1 if imported => format!("`{name}` is declared by `{first}`: write `{written}`"),
                    1 => format!("`{name}` is declared by `{first}`: write `{written}`, after `import {first}`"),
                    count => {
                        let shown: Vec<String> = owners.iter().take(3).map(|o| format!("`{o}`")).collect();
                        let more = if count > 3 { format!(" and {} more", count - 3) } else { String::new() };
                        format!(
                            "`{name}` is declared by {}{more}: write it with its module, as `{written}`",
                            shown.join(", ")
                        )
                    }
                }
            }
            _ => {
                // The bindings in sight and the module's names.
                let mut names: Vec<&str> = self.names.keys().map(String::as_str).collect();
                let mut scope = Some(cx);
                while let Some(found) = scope {
                    names.extend(found.scopes.names());
                    scope = found.parent.as_deref();
                }
                match closest(name, names) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => String::new(),
                }
            }
        };
        let message = match hint.is_empty() {
            true => format!("unknown name `{name}`"),
            false => format!("unknown name `{name}`; {hint}"),
        };
        self.diags.error("E1001", span, message);
        let replacement = match name {
            "Some" | "Ok" | "Err" => Some((true, name.to_lowercase())),
            "None" | "null" | "nil" | "undefined" | "NULL" | "nullptr" | "Nothing" => Some((false, "none".into())),
            "True" | "TRUE" => Some((true, "true".into())),
            "False" | "FALSE" => Some((true, "false".into())),
            "this" => Some((false, "self".into())),
            _ => hint
                .strip_prefix("did you mean `")
                .and_then(|h| h.strip_suffix("`?"))
                .map(|near| (false, near.to_string())),
        };
        let owners = self.declaring(span, name);
        if let Some((safe, replacement)) = replacement {
            self.diags.fix(format!("write `{replacement}`"), safe, vec![(span, replacement)]);
        } else if !owners.is_empty() {
            // Each module that declares it, with its import when the file
            // lacks it; the one meaning there is when only one does.
            for owner in owners.iter().take(6) {
                let written = format!("{}.{name}", self.imported_as(span, owner));
                let mut edits = vec![(span, written.clone())];
                if !self.imported(span, owner) {
                    edits.push((self.import_place(span, owner), format!("import {owner}\n")));
                }
                self.diags.fix(format!("write `{written}`"), owners.len() == 1, edits);
            }
        } else if let Some(path) = module
            .or_else(|| crate::api::standard_modules().contains(&format!("std.{name}")).then(|| format!("std.{name}")))
        {
            let place = self.import_place(span, &path);
            self.diags.fix(format!("add `import {path}`"), true, vec![(place, format!("import {path}\n"))]);
        }
    }
}
