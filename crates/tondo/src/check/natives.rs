//! The methods of sync, time, process, io, files, `Ref`, atomics, channels and handles.

use super::*;

impl<'a> Checker<'a> {
    /// A `std.sync` type (14.4.4) with its type arguments.
    pub(super) fn sync_type(&mut self, name: &ast::Ident, args: &[Ty], span: Span) -> Ty {
        let handle = |this: &mut Self, handle: Handle, state: TyKind| {
            let state = this.types.intern(state);
            this.types.handle(handle, state)
        };
        match (name.name.as_str(), args) {
            ("Mutex", [t]) => self.types.handle(Handle::Mutex, *t),
            ("RwLock", [t]) => self.types.handle(Handle::RwLock, *t),
            ("Atomic", [t]) => self.types.handle(Handle::Atomic, *t),
            ("Once", [t]) => handle(self, Handle::Once, TyKind::Option(*t)),
            ("Array", [t]) => handle(self, Handle::SyncArray, TyKind::Array(*t)),
            ("Stack", [t]) => handle(self, Handle::SyncStack, TyKind::Array(*t)),
            ("Queue", [t]) => handle(self, Handle::SyncQueue, TyKind::Deque(*t)),
            ("Set", [k]) => {
                self.require_key(*k, span);
                handle(self, Handle::SyncSet, TyKind::Set(*k))
            }
            ("Map", [k, v]) => {
                self.require_key(*k, span);
                let map = self.map_type(*k, *v);
                self.types.handle(Handle::SyncMap, map)
            }
            ("CompareExchange", [t]) => {
                let (template, _) = self.compare_exchange.expect("declared with the module");
                self.types.instantiate(template, vec![*t])
            }
            (
                "Mutex" | "RwLock" | "Atomic" | "Once" | "Array" | "Stack" | "Queue" | "Set" | "Map"
                | "CompareExchange",
                _,
            ) => {
                self.diags.error("E1104", span, "wrong number of type arguments");
                Ty::ERROR
            }
            _ => {
                let message = format!("module `sync` has no generic type `{}`", name.name);
                self.diags.error("E1001", name.span, message);
                Ty::ERROR
            }
        }
    }

    /// The methods of `fs.File` (standard library 10.5): a `var` loan of the
    /// file, except `close`, which takes it.
    /// A value a function gives back as its result `expected`: a fallible
    /// function may give the whole `T ! E` it declares, which propagates its
    /// error like `?` (11.2).
    pub(super) fn returned(&mut self, cx: &FnCx, value: hir::Expr, expected: Ty) -> hir::Expr {
        if let Some(error) = cx.error
            && expected == cx.result
            && let TyKind::Result(ok, produced) = *self.types.kind(value.ty)
            && ok == expected
            && produced == error
        {
            let span = value.span;
            return Self::mk(hir::ExprKind::Try(Box::new(value)), expected, span);
        }
        self.coerce(value, expected)
    }

    /// Whether `Type.Variant` names a variant without payload of an enum.
    pub(super) fn unit_variant(&self, path: &[ast::Ident]) -> bool {
        let Some(TypeName::Nominal(ty)) = self.type_names.get(&path[0].name) else { return false };
        let TyKind::Adt(id) = *self.types.kind(*ty) else { return false };
        match &self.types.adt(id).kind {
            AdtKind::Enum(variants) => {
                variants.iter().any(|v| v.name == path[1].name && matches!(v.payload, Payload::Unit))
            }
            _ => false,
        }
    }

    /// A raw operation of a `Pointer[T]`, or `toPointer` of a `UInt64`
    /// (16.12): only inside an `unsafe` region.
    pub(super) fn raw(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        types: Vec<Ty>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let address = Ty::int(IntTy::U64);
        let target = match *self.types.kind(receiver.ty) {
            TyKind::Pointer(target) => Some(target),
            _ => None,
        };
        let known = matches!(
            (target.is_some(), method.name.as_str()),
            (true, "read" | "write" | "offset" | "cast" | "address") | (false, "toPointer" | "toFunction")
        );
        if !known {
            return self.no_method(receiver.ty, method, span);
        }
        if !cx.unsafe_region {
            let message = format!(
                "`{}` is a raw operation: write it inside `unsafe {{ ... }}`, once its preconditions hold",
                method.name
            );
            self.diags.error("E1701", method.span, message);
        }
        if let Some(target) = target
            && matches!(method.name.as_str(), "read" | "write" | "offset")
            && self.types.c_layout(target).is_some_and(|l| l.opaque)
        {
            let message = format!(
                "{} is opaque: C alone knows its size, so only C reads, writes or steps over it",
                self.display(target)
            );
            self.diags.error("E1703", method.span, message);
            return Self::error_expr(span);
        }
        let generic = matches!(method.name.as_str(), "cast" | "toPointer" | "toFunction");
        let ty_arg = match (generic, types.as_slice()) {
            (true, [ty]) => Some(*ty),
            (false, []) => None,
            (true, _) => {
                let example = match method.name.as_str() {
                    "cast" => "pointer.cast[Byte]()",
                    "toPointer" => "address.toPointer[Byte]()",
                    _ => "address.toFunction[extern fn(Int): Int]()",
                };
                let message = format!("`{}` takes the type it points to: `{example}`", method.name);
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            (false, _) => {
                let message = format!("`{}` takes no type arguments", method.name);
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
        };
        let params = match method.name.as_str() {
            "write" => vec![target.unwrap()],
            "offset" => vec![Ty::INT],
            _ => Vec::new(),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let retype = |value: hir::Expr, ty: Ty| Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span);
        let receiver_ty = values[0].ty;
        match method.name.as_str() {
            "read" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Read, values), target.unwrap(), span),
            "write" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Write, values), Ty::UNIT, span),
            "offset" => Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Offset, values), receiver_ty, span),
            "address" => retype(values.remove(0), address),
            // The C function at an address, such as a sentinel `(fn)-1` of C.
            "toFunction" => {
                let function = ty_arg.unwrap();
                if !matches!(self.types.kind(function), TyKind::CFn(..)) && !self.types.has_error(function) {
                    let message = format!(
                        "`toFunction` makes a C function: its type argument is an `extern fn(...)`, not {}",
                        self.display(function)
                    );
                    self.diags.error("E1104", span, message);
                    return Self::error_expr(span);
                }
                retype(values.remove(0), function)
            }
            _ => {
                let pointer = self.types.intern(TyKind::Pointer(ty_arg.unwrap()));
                retype(values.remove(0), pointer)
            }
        }
    }

    /// Whether a value of `ty` may hold a raw pointer.
    pub(super) fn holds_pointer(&self, ty: Ty) -> bool {
        fn walk(types: &Types, ty: Ty, seen: &mut Vec<Ty>) -> bool {
            if seen.contains(&ty) {
                return false;
            }
            seen.push(ty);
            let repr = types.repr(ty);
            match types.kind(repr) {
                TyKind::Pointer(_) => true,
                TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t) => walk(types, *t, seen),
                TyKind::Map(a, b) | TyKind::Result(a, b) => walk(types, *a, seen) || walk(types, *b, seen),
                TyKind::Tuple(items) | TyKind::Union(items) => items.iter().any(|&t| walk(types, t, seen)),
                _ => match (types.components(repr), types.variants(repr)) {
                    (Some(items), _) => items.iter().any(|&t| walk(types, t, seen)),
                    (None, Some(variants)) => variants.concat().iter().any(|&t| walk(types, t, seen)),
                    _ => false,
                },
            }
        }
        walk(&self.types, ty, &mut Vec::new())
    }

    pub(super) fn native_ty(&mut self, ty: hir::NativeTy) -> Ty {
        match ty {
            hir::NativeTy::Int => Ty::INT,
            hir::NativeTy::Float => Ty::F64,
            hir::NativeTy::Bool => Ty::BOOL,
            hir::NativeTy::String => Ty::STRING,
            hir::NativeTy::Unit => Ty::UNIT,
            hir::NativeTy::JsonReader => self.types.handle(Handle::JsonReader, Ty::UNIT),
            hir::NativeTy::JsonWriter => self.types.handle(Handle::JsonWriter, Ty::UNIT),
            hir::NativeTy::IoReader => self.types.handle(Handle::Reader, Ty::UNIT),
            hir::NativeTy::IoWriter => self.types.handle(Handle::Writer, Ty::UNIT),
            hir::NativeTy::BytePointer => self.types.intern(TyKind::Pointer(Ty::BYTE)),
            hir::NativeTy::Bytes => self.types.intern(TyKind::Array(Ty::BYTE)),
            hir::NativeTy::Ints => self.types.intern(TyKind::Array(Ty::INT)),
            hir::NativeTy::Regex => self.types.handle(Handle::Regex, Ty::UNIT),
            hir::NativeTy::ReflectBox => self.types.handle(Handle::ReflectBox, Ty::UNIT),
            hir::NativeTy::Strings => self.types.intern(TyKind::Array(Ty::STRING)),
            hir::NativeTy::Char => Ty::CHAR,
            hir::NativeTy::Object => self.types.handle(Handle::Native, Ty::UNIT),
            hir::NativeTy::Owned => self.types.handle(Handle::Owned, Ty::UNIT),
            hir::NativeTy::Command => self.types.handle(Handle::Command, Ty::UNIT),
            hir::NativeTy::Pipeline => self.types.handle(Handle::Pipeline, Ty::UNIT),
        }
    }

    /// Nanoseconds in a unit of `std.time`, by the name of its constructor
    /// and reader.
    pub(super) fn time_unit(name: &str) -> Option<i128> {
        Some(match name {
            "nanoseconds" => 1,
            "microseconds" => 1_000,
            "milliseconds" => 1_000_000,
            "seconds" => 1_000_000_000,
            "minutes" => 60_000_000_000,
            "hours" => 3_600_000_000_000,
            _ => return None,
        })
    }

    /// A method of a `time.Duration` or `time.Instant` (14.3): arithmetic
    /// on their nanoseconds, which panics with `P0005` when it overflows.
    pub(super) fn time_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let (duration, instant) = (self.duration.unwrap(), self.instant.unwrap());
        let ty = receiver.ty;
        let nanos = |value: hir::Expr| Self::mk(hir::ExprKind::Retype(Box::new(value)), Ty::INT, span);
        let binary = |op: BinOp, left: hir::Expr, right: hir::Expr, ty: Ty| {
            Self::mk(hir::ExprKind::Binary(op, Box::new(left), Box::new(right)), ty, span)
        };
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, span);
        let retype = |value: hir::Expr, ty: Ty| Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span);
        let name = method.name.as_str();
        let param = match (ty == duration, name) {
            (true, "isLessThan") | (false, "add") => Some(duration),
            (true, "multiply") => Some(Ty::INT),
            (false, "since") => Some(instant),
            _ => None,
        };
        let known = param.is_some()
            || (ty == duration && (Self::time_unit(name).is_some() || matches!(name, "isZero" | "isNegative")))
            || (ty == instant && name == "elapsed");
        if !known && ty == duration && matches!(name, "add" | "subtract") {
            let operator = if name == "add" { "+" } else { "-" };
            let message = format!("durations add and subtract with operators: write `a {operator} b`");
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        }
        if !known {
            return self.no_method(ty, method, span);
        }
        let expected = usize::from(param.is_some());
        if args.len() != expected {
            return self.arity_error(span, expected, args.len());
        }
        let argument = param.map(|param| {
            let value = self.expr(cx, &args[0].value, Some(param));
            self.coerce(value, param)
        });
        let own = nanos(receiver);
        match (ty == duration, name, argument) {
            (true, "multiply", Some(factor)) => retype(binary(BinOp::Mul, own, factor, Ty::INT), duration),
            (true, "isLessThan", Some(other)) => binary(BinOp::Lt, own, nanos(other), Ty::BOOL),
            (true, "isZero", None) => binary(BinOp::Eq, own, int(0), Ty::BOOL),
            (true, "isNegative", None) => binary(BinOp::Lt, own, int(0), Ty::BOOL),
            (true, unit, None) => match Self::time_unit(unit).unwrap() {
                1 => own,
                factor => binary(BinOp::Div, own, int(factor), Ty::INT),
            },
            (false, "add", Some(delta)) => retype(binary(BinOp::Add, own, nanos(delta), Ty::INT), instant),
            (false, "since", Some(earlier)) => retype(binary(BinOp::Sub, own, nanos(earlier), Ty::INT), duration),
            (false, "elapsed", None) => {
                let now =
                    Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::TimeNow), Vec::new(), Vec::new()), instant, span);
                // The receiver first, which may be a call that reads the
                // clock or takes long: operands run from the left.
                let ago = binary(BinOp::Sub, own, nanos(now), Ty::INT);
                retype(binary(BinOp::Sub, int(0), ago, Ty::INT), duration)
            }
            _ => unreachable!("checked above"),
        }
    }

    /// A command literal (20.5): a `Command` for one stage and a `Pipeline`
    /// of several, each stage the command of its program and arguments,
    /// with the variables and redirections it names.
    pub(super) fn command_literal(&mut self, cx: &mut FnCx, stages: &[ast::CommandStage], span: Span) -> hir::Expr {
        use crate::lexer::Redirect;
        use hir::ProcessFn as P;
        let command = self.types.handle(Handle::Command, Ty::UNIT);
        let pipeline = self.types.handle(Handle::Pipeline, Ty::UNIT);
        let strings = self.types.intern(TyKind::Array(Ty::STRING));
        let call = |function: P, args: Vec<hir::Expr>, ty: Ty, span: Span| {
            Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Process(function)), Vec::new(), args), ty, span)
        };
        // Arrays of arguments, one after the other.
        let joined = |front: Option<hir::Expr>, next: hir::Expr| match front {
            None => next,
            Some(front) => {
                let span = front.span.to(next.span);
                Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Concat, vec![front, next]), strings, span)
            }
        };
        let mut whole: Option<hir::Expr> = None;
        for stage in stages {
            let mut words = stage.words.iter();
            let Some(ast::CommandWord::One(program)) = words.next() else { return Self::error_expr(span) };
            let program = self.command_word(cx, program);
            let (mut arguments, mut single) = (None, Vec::new());
            for word in words {
                match word {
                    ast::CommandWord::One(word) => single.push(self.command_word(cx, word)),
                    ast::CommandWord::Each(values) => {
                        if !single.is_empty() {
                            let span = values.span;
                            let array = Self::mk(hir::ExprKind::Array(std::mem::take(&mut single)), strings, span);
                            arguments = Some(joined(arguments, array));
                        }
                        let values = self.expr(cx, values, Some(strings));
                        if values.ty != strings && !self.types.has_error(values.ty) {
                            let message = format!(
                                "`{{...values}}` gives an argument for each element of an `Array[String]`, and this is {}: write `{{...values.map((value) {{ \"{{value}}\" }})}}`",
                                self.display(values.ty)
                            );
                            self.diags.error("E1102", values.span, message);
                        }
                        arguments = Some(joined(arguments, values));
                    }
                }
            }
            if !single.is_empty() || arguments.is_none() {
                arguments = Some(joined(arguments, Self::mk(hir::ExprKind::Array(single), strings, stage.span)));
            }
            let arguments = arguments.expect("an array of arguments");
            let mut value = call(P::Command, vec![program, arguments], command, stage.span);
            for (name, word) in &stage.env {
                let name = Self::mk(hir::ExprKind::Str(name.name.clone()), Ty::STRING, name.span);
                let word = self.command_word(cx, word);
                value = call(P::Configure(1), vec![value, name, word], command, stage.span);
            }
            for (redirect, target, at) in &stage.redirects {
                if *redirect == Redirect::ErrorsToOutput {
                    value = call(P::Configure(7), vec![value], command, *at);
                    continue;
                }
                let Some(types) = self.process else {
                    let message =
                        "a redirection of a command names a `process.Input` or `process.Output`: `import std.process`";
                    self.diags.error("E1001", *at, message);
                    continue;
                };
                let Some(target) = target else { continue };
                let path = self.command_word(cx, target);
                let (setting, ty, variant) = match redirect {
                    Redirect::In => (4, types.input, "File"),
                    Redirect::Out => (5, types.output, "File"),
                    Redirect::Append => (5, types.output, "Append"),
                    Redirect::Errors => (6, types.output, "File"),
                    _ => (6, types.output, "Append"),
                };
                let index = match self.types.kind(ty) {
                    TyKind::Adt(id) => match &self.types.adt(*id).kind {
                        AdtKind::Enum(variants) => variants.iter().position(|v| v.name == variant),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(index) = index else { continue };
                let destination = Self::mk(hir::ExprKind::Variant(index as u32, vec![(0, path)]), ty, *at);
                value = call(P::Configure(setting), vec![value, destination], command, *at);
            }
            whole = Some(match whole {
                None => value,
                Some(left) => call(P::Pipe, vec![left, value], pipeline, span),
            });
        }
        let mut whole = whole.unwrap_or_else(|| Self::error_expr(span));
        whole.span = span;
        whole
    }

    /// A word of a command literal: one argument, whose values show with
    /// their `Display`; a collection or an option there is an error that
    /// says to spread it or to take its value.
    fn command_word(&mut self, cx: &mut FnCx, word: &ast::Expr) -> hir::Expr {
        let value = self.expr(cx, word, Some(Ty::STRING));
        if let hir::ExprKind::Interpolate(segments) = &value.kind {
            for segment in segments {
                let hir::Segment::Value(part) = segment else { continue };
                let message = match self.types.kind(part.ty) {
                    TyKind::Array(_) | TyKind::Set(_) | TyKind::Deque(_) => {
                        "this value is several: write `{...values}`, a word of its own, for an argument each"
                    }
                    TyKind::Option(_) | TyKind::Result(..) => {
                        "this value may have nothing: take what it holds first, as `{value ?? \"\"}` or `{value?}`"
                    }
                    _ => continue,
                };
                self.diags.error("E1102", part.span, message);
            }
        }
        value
    }

    /// A method of a `Command`, a `Pipeline` or a `process.Process` (19.2).
    pub(super) fn process_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::ProcessFn as P;
        let Some(types) = self.process else {
            let message = format!("running a {} takes `import std.process`", self.display(receiver.ty));
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        };
        let ty = receiver.ty;
        let failing = |this: &mut Self, ok: Ty| this.types.intern(TyKind::Result(ok, types.error));
        let (s, plan) = (Ty::STRING, handle != Handle::Process);
        let (function, params, result) = match (handle, method.name.as_str()) {
            (Handle::Command, "directory") => (P::Configure(0), vec![s], ty),
            (Handle::Command, "env") => (P::Configure(1), vec![s, s], ty),
            (Handle::Command, "removeEnv") => (P::Configure(2), vec![s], ty),
            (Handle::Command, "clearEnv") => (P::Configure(3), vec![], ty),
            (_, "stdin") if plan => (P::Configure(4), vec![types.input], ty),
            (_, "stdout") if plan => (P::Configure(5), vec![types.output], ty),
            (Handle::Command, "stderr") => (P::Configure(6), vec![types.output], ty),
            (_, "mergeStderr") if plan => (P::Configure(7), vec![], ty),
            (_, "group") if plan => (P::Configure(8), vec![], ty),
            (_, "grace") if plan => {
                let Some(duration) = self.duration else {
                    let message =
                        "`grace` takes a `time.Duration`: `import std.time`, and write `grace(time.seconds(10))`";
                    self.diags.error("E1001", method.span, message);
                    return Self::error_expr(span);
                };
                (P::Configure(9), vec![duration], ty)
            }
            (Handle::Command, "invokedAs") => (P::Configure(10), vec![s], ty),
            (Handle::Command, "exec") => (P::Exec, vec![], self.types.intern(TyKind::Result(Ty::NEVER, types.error))),
            (_, "run") if plan => (P::Run, vec![], failing(self, Ty::UNIT)),
            (_, "text") if plan => (P::ReadText, vec![], failing(self, Ty::STRING)),
            (_, "lines") if plan => {
                let lines = self.types.intern(TyKind::Array(Ty::STRING));
                (P::ReadLines, vec![], failing(self, lines))
            }
            (_, "status") if plan => (P::Status, vec![], failing(self, types.status)),
            (_, "output") if plan => (P::Output, vec![], failing(self, types.result)),
            (_, "check") if plan => (P::Check, vec![], failing(self, types.result)),
            (_, "start") if plan => (P::Start, vec![], failing(self, types.process)),
            (Handle::Process, "stdin") => (P::Stream(0), vec![], self.types.handle(Handle::Writer, Ty::UNIT)),
            (Handle::Process, "stdout") => (P::Stream(1), vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Process, "stderr") => (P::Stream(2), vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Process, "id") => (P::Id, vec![], Ty::INT),
            (Handle::Process, "wait") => (P::Wait, vec![], failing(self, types.status)),
            (Handle::Process, "cancel") => (P::Cancel, vec![], failing(self, types.status)),
            (Handle::Process, "output") => (P::Collect(false), vec![], failing(self, types.result)),
            (Handle::Process, "check") => (P::Collect(true), vec![], failing(self, types.result)),
            (Handle::Process, "detach") => (P::Detach, vec![], Ty::UNIT),
            (Handle::Process, "signal") => (P::Signal, vec![types.signal], Ty::UNIT),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            // A path where a redirection goes: name the variant that takes it.
            if value.ty == Ty::STRING && (ty == types.input || ty == types.output) {
                let message = format!(
                    "expected {}, found `String`: write `{}.File(path)`",
                    self.display(ty),
                    self.types.display(ty)
                );
                self.diags.error("E1102", value.span, message);
                return Self::error_expr(span);
            }
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Process(function)), Vec::new(), values), result, span)
    }

    /// A method of an `io.Writer`, `io.Reader` or `io.Buffer` (10.3): shared
    /// handles, which their methods only read.
    pub(super) fn io_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, this.io_error));
        let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
        use hir::IoFn as I;
        let (function, params, result) = match (handle, method.name.as_str()) {
            (Handle::Writer, "write") => (I::Write, vec![Ty::STRING], failing(self, Ty::UNIT)),
            (Handle::Writer, "writeLine") => (I::WriteLine, vec![Ty::STRING], failing(self, Ty::UNIT)),
            (Handle::Writer, "writeBytes") => (I::WriteBytes, vec![bytes], failing(self, Ty::UNIT)),
            (Handle::Writer, "flush") => (I::Flush, vec![], failing(self, Ty::UNIT)),
            (Handle::Writer | Handle::Reader, "close") => (I::Close, vec![], failing(self, Ty::UNIT)),
            (Handle::Reader, "readLine") => {
                let line = self.types.intern(TyKind::Option(Ty::STRING));
                (I::ReadLine, vec![], failing(self, line))
            }
            (Handle::Reader, "read") => {
                let chunk = self.types.intern(TyKind::Option(bytes));
                (I::Read, vec![Ty::INT], failing(self, chunk))
            }
            (Handle::Reader, "readText") => (I::ReadText, vec![], failing(self, Ty::STRING)),
            (Handle::Reader, "readBytes") => (I::ReadBytes, vec![], failing(self, bytes)),
            (Handle::Buffer, "writer") => (I::BufferWriter, vec![], self.types.handle(Handle::Writer, Ty::UNIT)),
            (Handle::Buffer, "reader") => (I::BufferReader, vec![], self.types.handle(Handle::Reader, Ty::UNIT)),
            (Handle::Buffer, "text") => (I::BufferText, vec![], failing(self, Ty::STRING)),
            (Handle::Buffer, "bytes") => (I::BufferBytes, vec![], bytes),
            (Handle::Buffer, "length") => (I::BufferLength, vec![], Ty::INT),
            (Handle::Buffer, "clear") => (I::BufferClear, vec![], Ty::UNIT),
            (Handle::Buffer | Handle::Reader, "write" | "writeLine" | "writeBytes" | "flush") => {
                let fix =
                    if handle == Handle::Buffer { "write through `buffer.writer()`" } else { "a reader only reads" };
                let message = format!("{} has no method `{}`: {fix}", self.display(receiver.ty), method.name);
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            (Handle::Buffer | Handle::Writer, "readLine" | "read" | "readText") => {
                let fix =
                    if handle == Handle::Buffer { "read through `buffer.reader()`" } else { "a writer only writes" };
                let message = format!("{} has no method `{}`: {fix}", self.display(receiver.ty), method.name);
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            _ => return self.no_method(receiver.ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Io(function)), Vec::new(), values), result, span)
    }

    pub(super) fn file_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let fs = self.fs.expect("declared with the module");
        let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, fs.error));
        let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
        use hir::FsFn as F;
        let (function, params, result) = match method.name.as_str() {
            // The file becomes a stream of `std.io`, the same object.
            "writer" | "reader" if args.is_empty() => {
                let handle = if method.name == "writer" { Handle::Writer } else { Handle::Reader };
                let ty = self.types.handle(handle, Ty::UNIT);
                return Self::mk(hir::ExprKind::Retype(Box::new(receiver)), ty, span);
            }
            "readLine" => {
                let line = self.types.intern(TyKind::Option(Ty::STRING));
                (F::ReadLine, vec![], failing(self, line))
            }
            "read" => {
                let chunk = self.types.intern(TyKind::Option(bytes));
                (F::Read, vec![Ty::INT], failing(self, chunk))
            }
            "readText" => (F::ReadRest, vec![], failing(self, Ty::STRING)),
            "write" => (F::Write, vec![Ty::STRING], failing(self, Ty::UNIT)),
            "writeBytes" => (F::WriteBytesTo, vec![bytes], failing(self, Ty::UNIT)),
            "flush" => (F::Flush, vec![], failing(self, Ty::UNIT)),
            "close" => (F::Close, vec![], failing(self, Ty::UNIT)),
            "lock" => (F::Lock, vec![Ty::BOOL], failing(self, Ty::UNIT)),
            "tryLock" => (F::TryLock, vec![Ty::BOOL], failing(self, Ty::BOOL)),
            _ => return self.no_method(receiver.ty, method, span),
        };
        // `shared` may be left out, for an exclusive lock.
        let locking = matches!(function, F::Lock | F::TryLock);
        if locking && let Some(arg) = args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name != "shared")) {
            let message = format!("`{}` takes `shared`, not `{}`", method.name, arg.name.as_ref().unwrap().name);
            self.diags.error("E1102", arg.value.span, message);
            return Self::error_expr(span);
        }
        if locking && args.is_empty() {
            let receiver = self.file_loan(cx, receiver, method);
            let Some(receiver) = receiver else { return Self::error_expr(span) };
            let unshared = Self::mk(hir::ExprKind::Bool(false), Ty::BOOL, span);
            return Self::mk(
                hir::ExprKind::Call(Callee::Std(StdFn::Fs(function)), Vec::new(), vec![receiver, unshared]),
                result,
                span,
            );
        }
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let receiver = if function == F::Close {
            receiver
        } else {
            let Some(receiver) = self.file_loan(cx, receiver, method) else { return Self::error_expr(span) };
            receiver
        };
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Fs(function)), Vec::new(), values), result, span)
    }

    /// The file a method works on, lent to it.
    pub(super) fn file_loan(&mut self, cx: &mut FnCx, receiver: hir::Expr, method: &ast::Ident) -> Option<hir::Expr> {
        let (ty, receiver_span) = (receiver.ty, receiver.span);
        let operation = format!("`{}`", method.name);
        let place = self.loan_place(cx, receiver, &operation, true)?;
        Some(Self::mk(hir::ExprKind::Loan(place), ty, receiver_span))
    }

    /// `Ref(value)` (16.10): a new identity holding the value, copied when
    /// it is `Copy` and moved otherwise.
    pub(super) fn ref_new(
        &mut self,
        cx: &mut FnCx,
        args: &[ast::Arg],
        explicit: Option<Ty>,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let wanted = explicit.or_else(|| match expected.map(|e| self.types.kind(e)) {
            Some(&TyKind::Handle(Handle::Ref, value)) => Some(value),
            _ => None,
        });
        let value = self.expr(cx, &arg.value, wanted);
        let value = match wanted {
            Some(wanted) => self.coerce(value, wanted),
            None => value,
        };
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let ty = self.types.handle(Handle::Ref, value.ty);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::RefNew, vec![value]), ty, span)
    }

    /// `WeakRef(reference)`: a reference to the same object that does not
    /// keep it alive.
    pub(super) fn weak_new(&mut self, cx: &mut FnCx, args: &[ast::Arg], expected: Option<Ty>, span: Span) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let wanted = match expected.map(|e| self.types.kind(e)) {
            Some(&TyKind::Handle(Handle::Weak, value)) => Some(self.types.handle(Handle::Ref, value)),
            _ => None,
        };
        let reference = self.expr(cx, &arg.value, wanted);
        let value = match *self.types.kind(reference.ty) {
            TyKind::Handle(Handle::Ref, value) => value,
            TyKind::Error => return Self::error_expr(span),
            _ => {
                let message = format!(
                    "`WeakRef` refers to the object of a `Ref`, found {}; make one first: `WeakRef(Ref(value))`",
                    self.display(reference.ty)
                );
                self.diags.error("E1102", reference.span, message);
                return Self::error_expr(span);
            }
        };
        let ty = self.types.handle(Handle::Weak, value);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::WeakNew, vec![reference]), ty, span)
    }

    /// `sync.Map[K, V]()` and the other empty shared collections of
    /// `std.sync`, of these types.
    pub(super) fn sync_empty(&mut self, kind: &ast::Ident, types: Vec<Ty>, args: &[ast::Arg], span: Span) -> hir::Expr {
        if !args.is_empty() {
            return self.arity_error(span, 0, args.len());
        }
        let ty = self.sync_type(kind, &types, span);
        let state = match self.types.kind(ty) {
            TyKind::Handle(_, state) => *state,
            _ => return Self::error_expr(span),
        };
        let empty = match *self.types.kind(state) {
            TyKind::Array(_) | TyKind::Deque(_) => hir::ExprKind::ArrayOp(hir::ArrayOp::New, Vec::new()),
            TyKind::Map(..) | TyKind::Set(_) => hir::ExprKind::MapOp(hir::MapOp::New, Vec::new()),
            TyKind::Option(_) => hir::ExprKind::Variant(NONE, Vec::new()),
            _ => {
                let message = format!("`sync.{0}` takes its initial value: `sync.{0}(value)`", kind.name);
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            }
        };
        let empty = Self::mk(empty, state, span);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![empty]), ty, span)
    }

    /// `sync.Map()` without type arguments: those of the type it is
    /// given to.
    pub(super) fn sync_empty_expected(
        &mut self,
        kind: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let state = expected.and_then(|e| match self.types.kind(e) {
            TyKind::Handle(_, state) => Some(*state),
            _ => None,
        });
        let types = match state.map(|state| self.types.kind(state).clone()) {
            Some(TyKind::Map(key, value)) => vec![key, value],
            Some(TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Option(t)) => vec![t],
            _ => {
                let example = if kind.name == "Map" { "[String, Int]" } else { "[Int]" };
                let message = format!(
                    "`sync.{0}` needs the types it holds: `sync.{0}{example}()`, or give the binding its type",
                    kind.name
                );
                self.diags.error("E1101", span, message);
                return Self::error_expr(span);
            }
        };
        self.sync_empty(kind, types, args, span)
    }

    /// `sync.Mutex(value)` and the other `std.sync` constructors: a new
    /// shared cell holding the value, or a semaphore or barrier.
    pub(super) fn sync_new(
        &mut self,
        cx: &mut FnCx,
        name: &str,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
        let expected_state = expected.and_then(|e| match self.types.kind(e) {
            TyKind::Handle(_, state) => Some(*state),
            _ => None,
        });
        let (op, handle) = match name {
            "Semaphore" => (hir::TaskOp::SemaphoreNew, Handle::Semaphore),
            "Barrier" => (hir::TaskOp::BarrierNew, Handle::Barrier),
            _ => (hir::TaskOp::SyncNew, Handle::Mutex),
        };
        if op != hir::TaskOp::SyncNew {
            let count = self.expr(cx, &arg.value, Some(Ty::INT));
            let count = self.coerce(count, Ty::INT);
            let ty = self.types.handle(handle, Ty::UNIT);
            return Self::mk(hir::ExprKind::TaskOp(op, vec![count]), ty, span);
        }
        // The cell's type is the value's.
        self.inferring += 1;
        let value = self.expr(cx, &arg.value, expected_state);
        self.inferring -= 1;
        let ty = value.ty;
        let (handle, value) = match (name, self.types.kind(ty).clone()) {
            (_, TyKind::Error) => return Self::error_expr(span),
            ("Mutex", _) => (Handle::Mutex, value),
            ("RwLock", _) => (Handle::RwLock, value),
            ("Atomic", _) if self.types.is_equatable(ty) => (Handle::Atomic, value),
            ("Array", TyKind::Array(_)) => (Handle::SyncArray, value),
            ("Stack", TyKind::Array(_)) => (Handle::SyncStack, value),
            ("Queue", TyKind::Deque(_)) => (Handle::SyncQueue, value),
            // An array is a deque that starts at its first element.
            ("Queue", TyKind::Array(element)) => {
                let deque = self.types.intern(TyKind::Deque(element));
                (Handle::SyncQueue, Self::mk(hir::ExprKind::Retype(Box::new(value)), deque, span))
            }
            ("Map", TyKind::Map(..)) => (Handle::SyncMap, value),
            ("Set", TyKind::Set(_)) => (Handle::SyncSet, value),
            ("Atomic", _) => {
                let message = format!("`sync.Atomic` compares its value, but {} is not `Equatable`", self.display(ty));
                self.diags.error("E1105", value.span, message);
                return Self::error_expr(span);
            }
            ("Once", _) => {
                self.diags.error("E1102", span, "`sync.Once` starts empty: `sync.Once[T]()`");
                return Self::error_expr(span);
            }
            ("Array" | "Stack" | "Queue" | "Map" | "Set", _) => {
                let wanted = match name {
                    "Map" => "a map",
                    "Set" => "a set",
                    _ => "an array",
                };
                let message = format!("`sync.{name}` takes {wanted}, found {}", self.display(ty));
                self.diags.error("E1102", value.span, message);
                return Self::error_expr(span);
            }
            _ => {
                let message = format!("module `sync` has no function `{name}`");
                self.diags.error("E1001", span, message);
                return Self::error_expr(span);
            }
        };
        let state = value.ty;
        let ty = self.types.handle(handle, state);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![value]), ty, span)
    }

    /// `sync.Map["one": 1]` and the other literals of shared collections
    /// (5.11): the literal of the value collection, in a new cell.
    pub(super) fn sync_literal(
        &mut self,
        cx: &mut FnCx,
        path: &[ast::Ident],
        literal: &ast::Expr,
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if !matches!(self.names.get(&path[0].name), Some(ModuleName::Module(StdModule::Sync)))
            || self.lookup(cx, &path[0].name).is_some()
        {
            let message =
                format!("`{}` is not `std.sync`; import it to write `sync.{}[...]`", path[0].name, path[1].name);
            self.diags.error("E1001", path[0].span, message);
            return Self::error_expr(span);
        }
        let expected = expected.and_then(|e| match self.types.kind(e) {
            TyKind::Handle(_, state) => Some(*state),
            _ => None,
        });
        let kind = path[1].name.as_str();
        // The literal a set, stack or queue holds is written as an array.
        let wanted = match (kind, expected.map(|e| self.types.kind(e).clone())) {
            ("Set" | "Queue", Some(TyKind::Set(t) | TyKind::Deque(t))) => Some(self.types.intern(TyKind::Array(t))),
            (_, Some(_)) => expected,
            _ => None,
        };
        let value = match (kind, &literal.kind) {
            ("Set", ast::ExprKind::Array(items)) => self.set_literal(cx, items, expected, span),
            ("Set", _) => {
                self.diags.error("E1102", literal.span, "a set literal lists values: `sync.Set[a, b]`");
                return Self::error_expr(span);
            }
            _ => self.expr(cx, literal, wanted),
        };
        let value = match (kind, self.types.kind(value.ty).clone()) {
            (_, TyKind::Error) => return Self::error_expr(span),
            ("Map", TyKind::Map(..)) | ("Array" | "Stack", TyKind::Array(_)) | ("Set", TyKind::Set(_)) => value,
            ("Queue", TyKind::Array(element)) => {
                let deque = self.types.intern(TyKind::Deque(element));
                Self::mk(hir::ExprKind::Retype(Box::new(value)), deque, span)
            }
            _ => {
                let message = format!(
                    "`sync.{kind}[...]` lists {}",
                    if kind == "Map" { "entries: `[key: value]`" } else { "values" }
                );
                self.diags.error("E1102", literal.span, message);
                return Self::error_expr(span);
            }
        };
        let handle = match kind {
            "Array" => Handle::SyncArray,
            "Map" => Handle::SyncMap,
            "Set" => Handle::SyncSet,
            "Stack" => Handle::SyncStack,
            _ => Handle::SyncQueue,
        };
        let ty = self.types.handle(handle, value.ty);
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::SyncNew, vec![value]), ty, span)
    }

    /// A block that evaluates `args`, takes the lock of the state of
    /// `handle`, and ends with what `body` builds; the lock is released
    /// when the block ends, in any way.
    pub(super) fn locked(
        &mut self,
        cx: &mut FnCx,
        handle: hir::Expr,
        mode: hir::LockMode,
        args: Vec<hir::Expr>,
        span: Span,
        body: impl FnOnce(&mut Self, &mut FnCx, &Lock) -> (Vec<hir::Stmt>, hir::Expr),
    ) -> hir::Expr {
        let state = self.types.handle_result(handle.ty);
        let mut stmts = Vec::new();
        let receiver = self.hidden_local(cx, handle.ty);
        let read = Self::mk(hir::ExprKind::Local(receiver), handle.ty, span);
        stmts.push(hir::Stmt::Let(receiver, handle));
        let mut locals = Vec::new();
        for arg in args {
            let local = self.hidden_local(cx, arg.ty);
            locals.push((local, arg.ty));
            stmts.push(hir::Stmt::Let(local, arg));
        }
        let guard_ty = self.types.handle(Handle::Guard, state);
        let guard = self.hidden_local(cx, guard_ty);
        let inner = self.hidden_local(cx, state);
        let exclusive = mode != hir::LockMode::Shared;
        cx.mutable[inner.0 as usize] = exclusive;
        cx.params[inner.0 as usize] = Some(if exclusive { ast::Mode::Var } else { ast::Mode::Ref });
        stmts.push(hir::Stmt::LockSync { guard, inner, handle: read, mode, span });
        let lock = Lock { inner, guard, ty: state, args: locals, span };
        let (more, tail) = body(self, cx, &lock);
        stmts.extend(more);
        let ty = tail.ty;
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty }), ty, span)
    }

    /// The function argument of a `std.sync` operation, which receives
    /// `params` passed as `mode`: the argument and its result type, held
    /// in a local that may change when the function changes its captures.
    pub(super) fn sync_callback(
        &mut self,
        cx: &mut FnCx,
        arg: &ast::Arg,
        params: Vec<Ty>,
        mode: ast::Mode,
    ) -> Option<(hir::Expr, Ty)> {
        let modes = vec![mode; params.len()];
        let expected = self.types.intern(TyKind::Fn(params.clone(), Ty::ERROR, false, modes.clone(), false));
        let value = self.expr(cx, &arg.value, Some(expected));
        if value.ty == Ty::ERROR {
            return None;
        }
        match self.callable(value.ty) {
            // What it writes of its captures stays in its own copy (11.8),
            // which nobody sees once the lock is let go.
            Some(callable) if callable.mutates => {
                let message = "this function changes its captures, which only changes its own copy of them: \
                    give what it computes back instead, as `let seen = lock.update((value) { value })`";
                self.diags.error("E1108", value.span, message);
                None
            }
            Some(callable) if callable.params == params && callable.modes == modes => Some((value, callable.result)),
            _ => {
                let message = match (params.as_slice(), mode) {
                    ([], _) => "expected a function without parameters, such as `() { ... }`".to_string(),
                    ([state], ast::Mode::Var) => format!(
                        "this function receives the state as a `var` loan: write `(value) {{ ... }}`, or `(value: var {}) {{ ... }}`",
                        self.types.display(*state)
                    ),
                    ([state], _) => format!(
                        "this function receives a copy of the state: write `(value) {{ ... }}`, or `(value: {}) {{ ... }}`",
                        self.types.display(*state)
                    ),
                    _ => unreachable!("one parameter at most"),
                };
                self.diags.error("E1102", value.span, message);
                None
            }
        }
    }

    /// A name for the cell an expression reads, the same wherever a function
    /// writes it: a binding, or a field of one.
    pub(super) fn cell(cx: &FnCx, value: &hir::Expr) -> Option<String> {
        match &value.kind {
            hir::ExprKind::Local(local) => Some(cx.locals[local.0 as usize].span.start.to_string()),
            hir::ExprKind::Field(of, field) => Some(format!("{}.{field}", Self::cell(cx, of)?)),
            _ => None,
        }
    }

    pub(super) fn cell_use(&mut self, cell: String, method: &ast::Ident, args: &[ast::Arg], span: Span) {
        let read = matches!(method.name.as_str(), "get" | "load");
        let writes = matches!(method.name.as_str(), "set" | "store" | "swap" | "update");
        let (first, last) = (args.first(), args.last());
        if read || writes {
            let within =
                first.zip(last).map_or(Span::default(), |(a, b)| Span::new(a.value.span.start, b.value.span.end));
            let parameter = match first.map(|arg| &arg.value.kind) {
                Some(ast::ExprKind::Closure(closure)) => closure.params.first().map(|param| param.name.name.clone()),
                _ => None,
            };
            let (method, statement) = (method.name.clone(), Span::default());
            self.cell_uses.push(CellUse { cell, span, read, within, statement, method, parameter });
        }
    }

    /// The edits that make the `sync.Mutex(0)` a binding `name` holds an
    /// `Atomic`, when the function that declares it above `at` only counts with
    /// it: `update` that adds, `get` and `set`.
    pub(super) fn as_counter(&self, name: &str, at: Span) -> Option<Vec<(Span, String)>> {
        let before = self.sources.up_to_function_start(at.start);
        let start = at.start - before.len() as u32;
        let declared = before.find(&format!("let {name} = sync.Mutex("))?;
        let whole = Span::new(start, at.start + self.sources.to_function_end(at.start).len() as u32);
        let text = self.sources.slice(whole);
        let mut edits = Vec::new();
        let open = declared + format!("let {name} = ").len();
        edits.push((
            Span::new(start + open as u32, start + open as u32 + "sync.Mutex".len() as u32),
            "sync.Atomic".to_string(),
        ));
        for used in self.named_in(name, whole) {
            let local = (used.start - start) as usize;
            let after = &text[local + name.len()..];
            if local == declared + "let ".len() {
                continue;
            }
            let call_end = |length: usize| Span::new(used.end, used.end + length as u32);
            if after.starts_with(".get()") {
                edits.push((call_end(".get()".len()), ".load()".to_string()));
            } else if after.starts_with(".set(") {
                edits.push((call_end(".set".len()), ".store".to_string()));
            } else {
                // `.update((n) { n += 2 })`.
                let rest = after.strip_prefix(".update((")?;
                let (parameter, rest) = rest.split_once(") { ")?;
                let (amount, _) = rest.strip_prefix(parameter)?.strip_prefix(" += ")?.split_once(" })")?;
                let length = ".update((".len()
                    + parameter.len()
                    + ") { ".len()
                    + parameter.len()
                    + " += ".len()
                    + amount.len()
                    + " })".len();
                edits.push((call_end(length), format!(".add({amount})")));
            }
        }
        Some(edits)
    }

    /// Where a name is written in a span, as a whole word.
    pub(super) fn named_in(&self, name: &str, within: Span) -> Vec<Span> {
        let text = self.sources.slice(within);
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let found = text.match_indices(name).filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            !before.is_some_and(word) && !after.is_some_and(word)
        });
        found.map(|(at, _)| Span::new(within.start + at as u32, within.start + (at + name.len()) as u32)).collect()
    }

    /// W1016: a function that writes a cell from what it read of it earlier,
    /// as another task may change it in between. Reading the arguments of the
    /// write for the name of the binding is enough: what tells two bindings of
    /// one name apart is not worth building for every program that shares a
    /// value.
    pub(super) fn lost_updates(&mut self) {
        let uses = std::mem::take(&mut self.cell_uses);
        let inside = |within: Span, span: Span| within.start <= span.start && span.end <= within.end;
        let mut found = Vec::new();
        for write in uses.iter().filter(|used| !used.read) {
            let same = uses.iter().filter(|used| used.read && used.cell == write.cell);
            let places = |read: &CellUse| match inside(write.within, read.span) {
                true => vec![read.span],
                false if read.within == Span::default() => Vec::new(),
                false => self.named_in(self.sources.slice(read.within), write.within),
            };
            let source = same.filter(|read| read.span.start < write.span.end).find(|read| !places(read).is_empty());
            found.extend(source.map(|read| (write, read, places(read))));
        }
        let mut told = Vec::new();
        for (write, read, places) in found {
            let fix = self.one_update(write, read, &places);
            told.push((write.span, read.span, fix));
        }
        for (write, read, fix) in told {
            let message = "this writes a value made from one read earlier, and another task may change it in between, \
                           which would be lost: read and write it in one `update`, or an `Atomic` with `compareExchange`";
            self.diags.warning("W1016", write, message);
            self.diags.related(read, "read here");
            if let Some(edits) = fix {
                self.diags.fix("read and write it in one `update`", false, edits);
            }
        }
    }

    /// The edits that make a read and a write of a `Mutex` or a `RwLock` one
    /// `update`: what the value read names becomes the value there. Not when
    /// the value read goes on being used after the write.
    pub(super) fn one_update(&self, write: &CellUse, read: &CellUse, places: &[Span]) -> Option<Vec<(Span, String)>> {
        if !matches!(write.method.as_str(), "set" | "update") {
            return None;
        }
        let text = |span: Span| self.sources.slice(span).to_string();
        // Used after the write, up to where the function ends.
        if read.statement != Span::default() {
            let (name, rest) = (text(read.within), self.sources.to_function_end(write.span.end));
            let word = |c: char| c.is_alphanumeric() || c == '_';
            if rest.split(|c: char| !word(c)).any(|part| part == name) {
                return None;
            }
        }
        let name = write.parameter.clone().unwrap_or("value".to_string());
        let mut edits = Vec::new();
        if write.method == "update" {
            edits.extend(places.iter().map(|&span| (span, name.clone())));
        } else {
            // The argument with the value read in place, and the cell's name.
            let (mut made, mut from) = (String::new(), write.within.start);
            for span in places {
                made.push_str(&text(Span::new(from, span.start)));
                made.push_str(&name);
                from = span.end;
            }
            made.push_str(&text(Span::new(from, write.within.end)));
            let cell = text(write.span);
            let cell = cell.strip_suffix(&format!("{})", text(write.within)))?.strip_suffix(".set(")?;
            edits.push((write.span, format!("{cell}.update(({name}) {{ {name} = {made} }})")));
        }
        // The binding goes, with its line.
        if read.statement != Span::default() {
            let (mut start, mut end) = (read.statement.start, read.statement.end);
            while start > 0 && matches!(text(Span::new(start - 1, start)).as_str(), " " | "\t") {
                start -= 1;
            }
            while matches!(text(Span::new(end, end + 1)).as_str(), " " | "\t" | "\r") {
                end += 1;
            }
            edits.push((Span::new(start, end + 1), String::new()));
        }
        Some(edits)
    }

    /// The operations of the `std.sync` values (14.4.4). Each locks the
    /// shared state for one step, after evaluating its arguments, and works
    /// on it with the operations of the value it holds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sync_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::LockMode::{Exclusive, Shared, Try};
        let ty = receiver.ty;
        let name = method.name.as_str();
        let mk = Self::mk;
        let arity = |this: &mut Self, count: usize| -> bool {
            if args.len() != count {
                this.arity_error(span, count, args.len());
                false
            } else {
                true
            }
        };
        let value_arg = |this: &mut Self, cx: &mut FnCx, index: usize, ty: Ty| {
            let value = this.expr(cx, &args[index].value, Some(ty));
            this.coerce(value, ty)
        };
        let element = match self.types.kind(state) {
            TyKind::Array(t) | TyKind::Deque(t) | TyKind::Option(t) | TyKind::Set(t) => *t,
            _ => state,
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        if handle.collection() {
            return self.shared_method(cx, receiver, handle, state, method, args, span);
        }
        if handle == Handle::Atomic && atomic_scalar(&self.types, state) {
            return self.atomic_method(cx, receiver, state, method, args, span);
        }
        match (handle, name) {
            (Handle::Semaphore | Handle::Permit | Handle::Barrier, _) => {
                let (op, result) = match (handle, name) {
                    (Handle::Semaphore, "acquire") => {
                        (hir::TaskOp::Acquire, self.types.handle(Handle::Permit, Ty::UNIT))
                    }
                    (Handle::Semaphore, "tryAcquire") => {
                        let permit = self.types.handle(Handle::Permit, Ty::UNIT);
                        (hir::TaskOp::TryAcquire, option(self, permit))
                    }
                    (Handle::Permit, "release") => (hir::TaskOp::PermitRelease, Ty::UNIT),
                    (Handle::Barrier, "wait") => (hir::TaskOp::BarrierWait, Ty::BOOL),
                    _ => return self.no_method(ty, method, span),
                };
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                mk(hir::ExprKind::TaskOp(op, vec![receiver]), result, span)
            }
            (Handle::Mutex | Handle::RwLock, "update") | (Handle::Mutex, "tryUpdate") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((change, result)) = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Var) else {
                    return Self::error_expr(span);
                };
                let mode = if name == "tryUpdate" { Try } else { Exclusive };
                let some_result = option(self, result);
                self.locked(cx, receiver, mode, vec![change], span, |_, _, lock| {
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.loan()]), result);
                    if mode == Exclusive {
                        return (Vec::new(), call);
                    }
                    let held = lock.mk(
                        hir::ExprKind::TaskOp(
                            hir::TaskOp::GuardHeld,
                            vec![lock.mk(hir::ExprKind::Local(lock.guard), Ty::UNIT)],
                        ),
                        Ty::BOOL,
                    );
                    let some = lock.mk(hir::ExprKind::Variant(SOME, vec![(0, call)]), some_result);
                    let then = hir::Block { stmts: Vec::new(), tail: Some(Box::new(some)), ty: some_result };
                    let none = lock.mk(hir::ExprKind::Variant(NONE, Vec::new()), some_result);
                    (Vec::new(), lock.mk(hir::ExprKind::If(Box::new(held), then, Some(Box::new(none))), some_result))
                })
            }
            (Handle::Mutex, "updateWhen") => {
                if !arity(self, 2) {
                    return Self::error_expr(span);
                }
                let ready = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Value);
                let change = self.sync_callback(cx, &args[1], vec![state], ast::Mode::Var);
                let (Some((ready, ready_ty)), Some((change, result))) = (ready, change) else {
                    return Self::error_expr(span);
                };
                if ready_ty != Ty::BOOL {
                    let message = format!(
                        "`updateWhen` waits until its first function gives `true`, but it gives {}",
                        self.display(ready_ty)
                    );
                    self.diags.error("E1102", args[0].value.span, message);
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Exclusive, vec![ready, change], span, |_, _, lock| {
                    let check = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.state()]), Ty::BOOL);
                    let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
                    let test = lock.mk(hir::ExprKind::If(Box::new(check), stop, None), Ty::UNIT);
                    let body = hir::Block {
                        stmts: vec![hir::Stmt::Expr(test), hir::Stmt::AwaitChange(lock.guard, lock.span)],
                        tail: None,
                        ty: Ty::UNIT,
                    };
                    let wait = hir::Stmt::Loop { condition: None, body };
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(1)), vec![lock.loan()]), result);
                    (vec![wait], call)
                })
            }
            (Handle::RwLock, "read") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((observe, result)) = self.sync_callback(cx, &args[0], vec![state], ast::Mode::Value) else {
                    return Self::error_expr(span);
                };
                self.locked(cx, receiver, Shared, vec![observe], span, |_, _, lock| {
                    (Vec::new(), lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), vec![lock.state()]), result))
                })
            }
            (Handle::Mutex | Handle::RwLock, "get") | (Handle::Atomic, "load") | (Handle::Once, "get") => {
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Shared, Vec::new(), span, |_, _, lock| (Vec::new(), lock.state()))
            }
            (Handle::Mutex | Handle::RwLock, "set") | (Handle::Atomic, "store") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let value = value_arg(self, cx, 0, state);
                self.locked(cx, receiver, Exclusive, vec![value], span, |_, _, lock| {
                    (vec![lock.assign(Vec::new(), None, lock.arg(0))], lock.mk(hir::ExprKind::Unit, Ty::UNIT))
                })
            }
            (Handle::Atomic, "swap") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let value = value_arg(self, cx, 0, state);
                let old = self.hidden_local(cx, state);
                self.locked(cx, receiver, Exclusive, vec![value], span, |_, _, lock| {
                    let stmts = vec![hir::Stmt::Let(old, lock.state()), lock.assign(Vec::new(), None, lock.arg(0))];
                    (stmts, lock.mk(hir::ExprKind::Local(old), lock.ty))
                })
            }
            (Handle::Atomic, "add") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                if self.types.as_int(state).is_none() {
                    let message = format!("`add` counts in integers, but this atomic holds {}", self.display(state));
                    self.diags.error("E1102", method.span, message);
                    return Self::error_expr(span);
                }
                let delta = value_arg(self, cx, 0, state);
                self.locked(cx, receiver, Exclusive, vec![delta], span, |_, _, lock| {
                    (vec![lock.assign(Vec::new(), Some(BinOp::Add), lock.arg(0))], lock.state())
                })
            }
            (Handle::Atomic, "compareExchange") => {
                if !arity(self, 2) {
                    return Self::error_expr(span);
                }
                let expected = value_arg(self, cx, 0, state);
                let desired = value_arg(self, cx, 1, state);
                let (template, _) = self.compare_exchange.expect("declared with the module");
                let outcome = self.types.instantiate(template, vec![state]);
                self.locked(cx, receiver, Exclusive, vec![expected, desired], span, |_, _, lock| {
                    let same = lock
                        .mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(lock.state()), Box::new(lock.arg(0))), Ty::BOOL);
                    let exchanged = lock.mk(hir::ExprKind::Variant(0, vec![(0, lock.arg(0))]), outcome);
                    let then = hir::Block {
                        stmts: vec![lock.assign(Vec::new(), None, lock.arg(1))],
                        tail: Some(Box::new(exchanged)),
                        ty: outcome,
                    };
                    let mismatch = lock.mk(hir::ExprKind::Variant(1, vec![(0, lock.state())]), outcome);
                    (Vec::new(), lock.mk(hir::ExprKind::If(Box::new(same), then, Some(Box::new(mismatch))), outcome))
                })
            }
            (Handle::Once, "getOrInit") => {
                if !arity(self, 1) {
                    return Self::error_expr(span);
                }
                let Some((init, made)) = self.sync_callback(cx, &args[0], Vec::new(), ast::Mode::Value) else {
                    return Self::error_expr(span);
                };
                let error = match *self.types.kind(made) {
                    _ if made == element => None,
                    TyKind::Result(value, error) if value == element => Some(error),
                    _ => {
                        let message = format!(
                            "`getOrInit` makes the value of this `sync.Once`, of type {}, but this function gives {}",
                            self.display(element),
                            self.display(made)
                        );
                        self.diags.error("E1102", args[0].value.span, message);
                        return Self::error_expr(span);
                    }
                };
                let result = match error {
                    Some(error) => self.types.intern(TyKind::Result(element, error)),
                    None => element,
                };
                let ready = self.hidden_local(cx, element);
                let fresh = self.hidden_local(cx, element);
                let failure = error.map(|e| self.hidden_local(cx, e));
                self.locked(cx, receiver, Exclusive, vec![init], span, |_, _, lock| {
                    let read = |local: LocalId, ty: Ty| lock.mk(hir::ExprKind::Local(local), ty);
                    let wrap = |value: hir::Expr| match error {
                        Some(_) => lock.mk(hir::ExprKind::Variant(OK, vec![(0, value)]), result),
                        None => value,
                    };
                    let bind = |local: LocalId, ty: Ty| hir::Pattern { kind: hir::PatKind::Binding(local), ty, span };
                    let variant = |index: u32, items: Vec<hir::Pattern>, ty: Ty| hir::Pattern {
                        kind: hir::PatKind::Variant(index, items),
                        ty,
                        span,
                    };
                    // Store the value made, then give it.
                    let store = |value: LocalId| {
                        let some = lock.mk(hir::ExprKind::Variant(SOME, vec![(0, read(value, element))]), lock.ty);
                        let block = hir::Block {
                            stmts: vec![lock.assign(Vec::new(), None, some)],
                            tail: Some(Box::new(wrap(read(value, element)))),
                            ty: result,
                        };
                        lock.mk(hir::ExprKind::Block(block), result)
                    };
                    let call = lock.mk(hir::ExprKind::CallValue(Box::new(lock.arg(0)), Vec::new()), made);
                    let make = match failure {
                        None => {
                            let stmts = vec![hir::Stmt::Let(fresh, call)];
                            let tail = store(fresh);
                            lock.mk(
                                hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(tail)), ty: result }),
                                result,
                            )
                        }
                        Some(failed) => {
                            let error_ty = error.unwrap();
                            let arms = vec![
                                hir::Arm {
                                    pattern: variant(OK, vec![bind(fresh, element)], made),
                                    guard: None,
                                    body: store(fresh),
                                },
                                hir::Arm {
                                    pattern: variant(ERR, vec![bind(failed, error_ty)], made),
                                    guard: None,
                                    body: lock
                                        .mk(hir::ExprKind::Variant(ERR, vec![(0, read(failed, error_ty))]), result),
                                },
                            ];
                            lock.mk(hir::ExprKind::Match(Box::new(call), arms), result)
                        }
                    };
                    let arms = vec![
                        hir::Arm {
                            pattern: variant(SOME, vec![bind(ready, element)], lock.ty),
                            guard: None,
                            body: wrap(read(ready, element)),
                        },
                        hir::Arm { pattern: variant(NONE, Vec::new(), lock.ty), guard: None, body: make },
                    ];
                    (Vec::new(), lock.mk(hir::ExprKind::Match(Box::new(lock.state()), arms), result))
                })
            }
            (Handle::Once, "isReady") => {
                if !arity(self, 0) {
                    return Self::error_expr(span);
                }
                self.locked(cx, receiver, Shared, Vec::new(), span, |_, _, lock| {
                    let none = lock.mk(hir::ExprKind::Variant(NONE, Vec::new()), lock.ty);
                    (
                        Vec::new(),
                        lock.mk(hir::ExprKind::Binary(BinOp::Ne, Box::new(lock.state()), Box::new(none)), Ty::BOOL),
                    )
                })
            }
            _ => self.no_method(ty, method, span),
        }
    }

    /// The operations of the shared collections (14.4.4.2), each one call
    /// of the runtime.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shared_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::SharedOp as S;
        let ty = receiver.ty;
        let name = method.name.as_str();
        let (key, entry, element) = match *self.types.kind(state) {
            TyKind::Map(key, value) => (key, value, key),
            TyKind::Set(key) => (key, Ty::UNIT, key),
            TyKind::Array(t) | TyKind::Deque(t) => (Ty::INT, t, t),
            _ => return Self::error_expr(span),
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        let (op, params, result) = match (handle, name) {
            (_, "length" | "isEmpty") => (S::Length, Vec::new(), Ty::INT),
            (_, "snapshot") => (S::Snapshot, Vec::new(), state),
            (Handle::SyncMap, "get") => (S::Get, vec![key], option(self, entry)),
            (Handle::SyncMap | Handle::SyncSet, "contains") => (S::Contains, vec![key], Ty::BOOL),
            (Handle::SyncMap, "insert") => (S::Insert, vec![key, entry], option(self, entry)),
            (Handle::SyncSet, "insert") => (S::Insert, vec![key], Ty::BOOL),
            (Handle::SyncMap, "remove") => (S::Remove, vec![key], option(self, entry)),
            (Handle::SyncSet, "remove") => (S::Remove, vec![key], Ty::BOOL),
            (Handle::SyncArray | Handle::SyncStack | Handle::SyncQueue, "get") => {
                (S::Get, vec![Ty::INT], option(self, element))
            }
            (Handle::SyncStack | Handle::SyncQueue, "push") => (S::Push, vec![element], Ty::UNIT),
            (Handle::SyncStack, "pop") | (Handle::SyncQueue, "popFront") => (S::Pop, Vec::new(), option(self, element)),
            (Handle::SyncArray, "set") => (S::Set, vec![Ty::INT, element], option(self, element)),
            (Handle::SyncArray, "compareExchange") => (S::Exchange, vec![Ty::INT, element, element], Ty::INT),
            (Handle::SyncMap, "compareExchange") => {
                let maybe = option(self, entry);
                (S::Exchange, vec![key, maybe, maybe], Ty::INT)
            }
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        if op == S::Exchange && !self.types.is_equatable(entry) {
            let message = format!("`compareExchange` compares values, but {} is not `Equatable`", self.display(entry));
            self.diags.error("E1105", span, message);
            return Self::error_expr(span);
        }
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, span);
        match op {
            S::Length if name == "isEmpty" => {
                let length = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), Ty::INT, span);
                Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(int(0))), Ty::BOOL, span)
            }
            S::Set => {
                // The replaced value, or `InvalidIndex` outside the array.
                let replaced = option(self, element);
                let result = self.types.intern(TyKind::Result(element, self.collection_error));
                let old = self.hidden_local(cx, element);
                let set = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), replaced, span);
                let pattern = |kind: hir::PatKind, ty: Ty| hir::Pattern { kind, ty, span };
                let read = Self::mk(hir::ExprKind::Local(old), element, span);
                let invalid = Self::mk(hir::ExprKind::Variant(1, Vec::new()), self.collection_error, span);
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(
                            hir::PatKind::Variant(SOME, vec![pattern(hir::PatKind::Binding(old), element)]),
                            replaced,
                        ),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(OK, vec![(0, read)]), result, span),
                    },
                    hir::Arm {
                        pattern: pattern(hir::PatKind::Variant(NONE, Vec::new()), replaced),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), result, span),
                    },
                ];
                Self::mk(hir::ExprKind::Match(Box::new(set), arms), result, span)
            }
            S::Exchange => {
                // The runtime writes the value it saw into `seen` and gives a
                // code; see `tondo_rt_shared_map_exchange` and the array's.
                let seen = self.hidden_local(cx, entry);
                let zero = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Zeroed, Vec::new()), entry, span);
                values.push(Self::mk(hir::ExprKind::Loan(hir::Place { local: seen, path: Vec::new() }), entry, span));
                let code = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), Ty::INT, span);
                let (template, _) = self.compare_exchange.expect("declared with the module");
                let read = Self::mk(hir::ExprKind::Local(seen), entry, span);
                let pattern = |value: Option<i128>| hir::Pattern {
                    kind: value.map_or(hir::PatKind::Wildcard, |v| hir::PatKind::Literal(hir::Literal::Int(v))),
                    ty: Ty::INT,
                    span,
                };
                let (arms, result) = if handle == Handle::SyncMap {
                    let maybe = option(self, entry);
                    let outcome = self.types.instantiate(template, vec![maybe]);
                    let value = |present: bool| match present {
                        true => Self::mk(hir::ExprKind::Variant(SOME, vec![(0, read.clone())]), maybe, span),
                        false => Self::mk(hir::ExprKind::Variant(NONE, Vec::new()), maybe, span),
                    };
                    let arm = |code: Option<i128>, variant: u32, present: bool| hir::Arm {
                        pattern: pattern(code),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(variant, vec![(0, value(present))]), outcome, span),
                    };
                    (
                        vec![arm(Some(0), 1, false), arm(Some(1), 0, false), arm(Some(2), 1, true), arm(None, 0, true)],
                        outcome,
                    )
                } else {
                    let outcome = self.types.instantiate(template, vec![element]);
                    let result = self.types.intern(TyKind::Result(outcome, self.collection_error));
                    let ok = |variant: u32| {
                        let outcome = Self::mk(hir::ExprKind::Variant(variant, vec![(0, read.clone())]), outcome, span);
                        Self::mk(hir::ExprKind::Variant(OK, vec![(0, outcome)]), result, span)
                    };
                    let invalid = Self::mk(hir::ExprKind::Variant(1, Vec::new()), self.collection_error, span);
                    let arms = vec![
                        hir::Arm {
                            pattern: pattern(Some(0)),
                            guard: None,
                            body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), result, span),
                        },
                        hir::Arm { pattern: pattern(Some(1)), guard: None, body: ok(0) },
                        hir::Arm { pattern: pattern(None), guard: None, body: ok(1) },
                    ];
                    (arms, result)
                };
                let decide = Self::mk(hir::ExprKind::Match(Box::new(code), arms), result, span);
                let block =
                    hir::Block { stmts: vec![hir::Stmt::Let(seen, zero)], tail: Some(Box::new(decide)), ty: result };
                Self::mk(hir::ExprKind::Block(block), result, span)
            }
            _ => Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Shared(op), values), result, span),
        }
    }

    /// The operations of a `sync.Atomic` of an integer, `Bool`, `Char` or
    /// `Byte`, done with atomic instructions.
    pub(super) fn atomic_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        state: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::AtomicOp as A;
        let ty = receiver.ty;
        let (op, count, result) = match method.name.as_str() {
            "load" => (A::Load, 0, state),
            "store" => (A::Store, 1, Ty::UNIT),
            "swap" => (A::Swap, 1, state),
            "add" => (A::Add, 1, state),
            "compareExchange" => (A::Exchange, 2, state),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != count {
            return self.arity_error(span, count, args.len());
        }
        if op == A::Add && self.types.as_int(state).is_none() {
            let message = format!("`add` counts in integers, but this atomic holds {}", self.display(state));
            self.diags.error("E1102", method.span, message);
            return Self::error_expr(span);
        }
        let mut values = vec![receiver];
        for arg in args {
            let value = self.expr(cx, &arg.value, Some(state));
            values.push(self.coerce(value, state));
        }
        if op != A::Exchange {
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Atomic(op), values), result, span);
        }
        // `Exchanged(seen)` when the value seen is the one expected.
        let expected = self.hidden_local(cx, state);
        let seen = self.hidden_local(cx, state);
        let read = |local: LocalId| Self::mk(hir::ExprKind::Local(local), state, span);
        let given = values.remove(1);
        values.insert(1, read(expected));
        let exchange = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Atomic(op), values), state, span);
        let (template, _) = self.compare_exchange.expect("declared with the module");
        let outcome = self.types.instantiate(template, vec![state]);
        let same =
            Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(read(seen)), Box::new(read(expected))), Ty::BOOL, span);
        let variant = |index: u32| Self::mk(hir::ExprKind::Variant(index, vec![(0, read(seen))]), outcome, span);
        let then = hir::Block { stmts: Vec::new(), tail: Some(Box::new(variant(0))), ty: outcome };
        let decide = Self::mk(hir::ExprKind::If(Box::new(same), then, Some(Box::new(variant(1)))), outcome, span);
        let stmts = vec![hir::Stmt::Let(expected, given), hir::Stmt::Let(seen, exchange)];
        Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: Some(Box::new(decide)), ty: outcome }), outcome, span)
    }

    /// `channel[T]()`, `channel[T](capacity)` and
    /// `discardWhenAbandoned: true` (11.13): the two ends of a new channel,
    /// unbounded without a capacity. Its operands are whether it discards,
    /// then its capacity if it has one.
    pub(super) fn channel(
        &mut self,
        cx: &mut FnCx,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let [value] = type_args else {
            self.diags.error("E1104", span, "`channel` takes one type argument, the type of its values");
            return Self::error_expr(span);
        };
        let value = self.resolve_type(value);
        if let Some(part) = self.not_send(value) {
            let what = match part == value {
                true => format!("{} is not `Send`", self.display(value)),
                false => format!("{} holds {}, which is not `Send`", self.display(value), self.display(part)),
            };
            let message = format!("a channel carries its values between tasks, but {what}");
            self.diags.error("E1605", span, message);
        }
        let (mut capacity, mut discard) = (None, None);
        for (index, arg) in args.iter().enumerate() {
            let name = arg.name.as_ref().map(|n| n.name.as_str());
            let slot = match name {
                None if index == 0 => &mut capacity,
                Some("capacity") => &mut capacity,
                Some("discardWhenAbandoned") => &mut discard,
                _ => {
                    let message = "`channel` takes its capacity and `discardWhenAbandoned`, as in `channel[T](16)` or `channel[T](16, discardWhenAbandoned: true)`";
                    self.diags.error("E1102", arg.span, message);
                    return Self::error_expr(span);
                }
            };
            if slot.is_some() {
                return self.arity_error(span, 2, args.len());
            }
            let ty = if name == Some("discardWhenAbandoned") { Ty::BOOL } else { Ty::INT };
            let value = self.expr(cx, &arg.value, Some(ty));
            *slot = Some(self.coerce(value, ty));
        }
        let sender = self.types.handle(Handle::Sender, value);
        let receiver = self.types.handle(Handle::Receiver, value);
        let ty = self.types.intern(TyKind::Tuple(vec![sender, receiver]));
        let discard = discard.unwrap_or_else(|| Self::mk(hir::ExprKind::Bool(false), Ty::BOOL, span));
        let operands = std::iter::once(discard).chain(capacity).collect();
        Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Channel, operands), ty, span)
    }

    /// The methods of `Join`, `Group`, `Sender` and `Receiver` (11.11 to
    /// 11.13); `result` is the type of their results or values.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_method(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        handle: Handle,
        result: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::TaskOp as T;
        let ty = receiver.ty;
        let op = match (handle, method.name.as_str()) {
            (Handle::Join, "wait") => T::Wait,
            (Handle::Group, "add") => T::GroupAdd,
            (Handle::Group, "next") => T::GroupNext,
            (Handle::Group, "length" | "isEmpty") => T::GroupLength,
            (Handle::Group, "wait") => T::GroupWait,
            (Handle::Join, "cancel") => T::Cancel,
            (Handle::Join, "detach") => T::Detach,
            (Handle::Group, "cancel") => T::GroupCancel,
            (Handle::Sender, "send") => T::Send,
            (Handle::Sender, "close") => T::Close,
            (Handle::Sender, "copy") => T::CopySender,
            (Handle::Sender | Handle::Receiver, "isClosed") => T::IsClosed,
            (Handle::Sender, "isAbandoned") => T::Abandoned,
            (Handle::Receiver, "receive") => T::Receive,
            (Handle::Weak, "get") => T::WeakGet,
            _ => return self.no_method(ty, method, span),
        };
        let timeout = matches!(args, [arg] if arg.name.as_ref().is_some_and(|n| n.name == "timeout"));
        let op = match op {
            T::Wait if timeout => T::WaitTimeout,
            T::Receive if timeout => T::ReceiveTimeout,
            op => op,
        };
        let params = match op {
            T::GroupAdd => vec![self.types.handle(Handle::Join, result)],
            T::Send => vec![result],
            T::ReceiveTimeout | T::WaitTimeout => match self.duration {
                Some(duration) => vec![duration],
                None => {
                    let message =
                        "a timeout is a `time.Duration`: add `import std.time` and write `time.milliseconds(n)`";
                    self.diags.error("E1102", args[0].span, message);
                    return Self::error_expr(span);
                }
            },
            _ => Vec::new(),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let receiver = match op {
            T::GroupAdd | T::GroupNext => {
                let operation = format!("`{}`", method.name);
                let receiver_span = receiver.span;
                let Some(place) = self.loan_place(cx, receiver, &operation, true) else {
                    return Self::error_expr(span);
                };
                Self::mk(hir::ExprKind::Loan(place), ty, receiver_span)
            }
            _ => receiver,
        };
        let mut values = vec![receiver];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(value, ty));
        }
        let result = match op {
            T::Wait => result,
            T::GroupAdd | T::Send | T::Close | T::Cancel | T::Detach | T::GroupCancel => Ty::UNIT,
            T::GroupNext | T::Receive | T::ReceiveTimeout | T::WaitTimeout => self.types.intern(TyKind::Option(result)),
            T::CopySender => ty,
            T::IsClosed | T::Abandoned => Ty::BOOL,
            T::WeakGet => {
                let reference = self.types.handle(Handle::Ref, result);
                self.types.intern(TyKind::Option(reference))
            }
            T::GroupWait => match *self.types.kind(result) {
                TyKind::Result(value, error) => {
                    let values = self.types.intern(TyKind::Array(value));
                    self.types.intern(TyKind::Result(values, error))
                }
                _ => self.types.intern(TyKind::Array(result)),
            },
            _ => Ty::INT,
        };
        let value = Self::mk(hir::ExprKind::TaskOp(op, values), result, span);
        if method.name == "isEmpty" {
            let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
            return Self::mk(hir::ExprKind::Binary(BinOp::Eq, Box::new(value), Box::new(zero)), Ty::BOOL, span);
        }
        value
    }
}
