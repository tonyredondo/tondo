//! Calls: generic, method, language, trait and dot calls, function values, arguments.

use super::*;

impl<'a> Checker<'a> {
    /// `value.method[T](...)`: a method's own type parameters, given; its
    /// type's come from the value.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn generic_method_call(
        &mut self,
        cx: &mut FnCx,
        receiver: &ast::Expr,
        method: &ast::Ident,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, receiver, None);
        let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
        if value.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        let key = self.owner_key(value.ty);
        let Some(&id) = self.methods.get(&(key, method.name.clone())) else {
            let message = format!(
                "{} has no method `{}` with type parameters of its own: call it as `value.{}(...)`",
                self.display(value.ty),
                method.name,
                method.name
            );
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        };
        self.index_refer(method, crate::index::Key::Fn(id.0));
        self.check_method_visible(id, method.span);
        let signature = self.signatures[id.0 as usize].clone();
        // The owner's own parameters come first: a generic type's, or an
        // intrinsic one's, as `T` of `Array[T]`.
        let owner = match self.types.instance_of(key) {
            Some((template, _)) => self.types.template(template).params.len(),
            None => match self.types.kind(value.ty) {
                TyKind::Array(_) | TyKind::Deque(_) | TyKind::Set(_) | TyKind::Option(_) => 1,
                TyKind::Map(..) | TyKind::Result(..) => 2,
                _ => 0,
            },
        };
        let own = &signature.generics[owner.min(signature.generics.len())..];
        if own.len() != types.len() {
            let message = match own.len() {
                0 => format!("`{0}` takes no type arguments: call it as `value.{0}(...)`", method.name),
                n => format!("`{}` takes {n} type argument(s), and {} are given", method.name, types.len()),
            };
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        }
        let prebound = own.iter().copied().zip(types).collect();
        self.dot_call(cx, Callee::Fn(id), signature, prebound, value, method, args, expected, span)
    }

    /// Type arguments given to what takes none: a path of an unknown name,
    /// as a module not imported, says so; any other, that it takes none.
    pub(super) fn not_generic(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) -> hir::Expr {
        let first = segments[0].name.as_str();
        if self.lookup(cx, first).is_some()
            || self.visible_capture(cx, first)
            || self.names.contains_key(first)
            || self.intrinsic_owner(first).is_some()
            || self.type_named(first).is_some()
        {
            let path = segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".");
            let message = format!("`{path}` takes no type arguments: call it as `{path}(...)`");
            self.diags.error("E1104", span, message);
        } else {
            self.unknown_name(cx, first, segments[0].span);
        }
        Self::error_expr(span)
    }

    /// `Array[T]()` and `Array.withCapacity[T](capacity)`.
    pub(super) fn generic_call(
        &mut self,
        cx: &mut FnCx,
        base: &ast::Expr,
        type_args: &[ast::TypeExpr],
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        // `value.cast[U]()` of any expression, such as a call's result.
        if let ast::ExprKind::Field(receiver, method) = &base.kind
            && matches!(method.name.as_str(), "cast" | "toPointer" | "toFunction")
        {
            let receiver = self.expr(cx, receiver, None);
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.raw(cx, receiver, method, types, args, span);
        }
        if let ast::ExprKind::Field(receiver, method) = &base.kind {
            return self.generic_method_call(cx, receiver, method, type_args, args, expected, span);
        }
        let ast::ExprKind::Path(segments) = &base.kind else {
            return self.unsupported_expr(span, "generic calls");
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        // `pointer.cast[U]()`, `address.toPointer[T]()` and their qualified
        // forms `Pointer.cast[U](pointer)` and `UInt64.toPointer[T](address)`.
        // `record.field.cast[U]()`: a receiver of several segments.
        if names.len() > 2
            && matches!(names[names.len() - 1], "cast" | "toPointer" | "toFunction")
            && (self.lookup(cx, names[0]).is_some()
                || self.visible_capture(cx, names[0])
                || matches!(self.names.get(names[0]), Some(ModuleName::Const(_))))
        {
            let (method, prefix) = segments.split_last().unwrap();
            let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
            let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
            let receiver = self.expr(cx, &receiver, None);
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.raw(cx, receiver, method, types, args, span);
        }
        if let [first, method @ ("cast" | "toPointer" | "toFunction")] = names.as_slice() {
            let raw = match *first {
                "Pointer" | "UInt" | "UInt64" if self.lookup(cx, first).is_none() => None,
                _ if self.lookup(cx, first).is_some() || self.visible_capture(cx, first) => {
                    let receiver =
                        ast::Expr { kind: ast::ExprKind::Path(vec![segments[0].clone()]), span: segments[0].span };
                    Some(self.expr(cx, &receiver, None))
                }
                _ => return self.not_generic(cx, segments, span),
            };
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let method = ast::Ident { name: (*method).to_string(), span: segments[1].span };
            return match raw {
                Some(receiver) => self.raw(cx, receiver, &method, types, args, span),
                None => {
                    let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                    let receiver = self.expr(cx, &first.value, None);
                    self.raw(cx, receiver, &method, types, rest, span)
                }
            };
        }
        // `value.method[T](...)` of a local value, as `value.field.method[T](...)`.
        if names.len() >= 2
            && (self.lookup(cx, names[0]).is_some()
                || self.visible_capture(cx, names[0])
                || matches!(self.names.get(names[0]), Some(ModuleName::Const(_))))
        {
            let (method, prefix) = segments.split_last().unwrap();
            let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
            let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
            return self.generic_method_call(cx, &receiver, method, type_args, args, expected, span);
        }
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
            && !self.type_params.iter().any(|(name, _)| name == trait_name)
        {
            let explicit: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            self.index_refer(&segments[0], crate::index::Key::Trait(trait_id.0));
            return self.trait_call(cx, trait_id, &segments[1], explicit, args, expected, span);
        }
        if let [name] = names.as_slice()
            && self.lookup(cx, name).is_none()
            && let Some(ModuleName::Fn(id)) = self.names.get(*name).copied()
        {
            let explicit = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let signature = self.signatures[id.0 as usize].clone();
            let target = CallTarget { explicit, ..CallTarget::new(Callee::Fn(id), signature, expected) };
            return self.call_signature(cx, target, None, args, span);
        }
        if let ["channel"] = names.as_slice()
            && self.lookup(cx, "channel").is_none()
        {
            return self.channel(cx, type_args, args, span);
        }
        if let ["Ref"] = names.as_slice()
            && self.lookup(cx, "Ref").is_none()
        {
            let [value] = type_args else {
                self.diags.error("E1104", span, "`Ref` takes one type argument: `Ref[T](value)`");
                return Self::error_expr(span);
            };
            let value = self.resolve_type(value);
            return self.ref_new(cx, args, Some(value), expected, span);
        }
        // `memory.take[T](pointer)`: the value, moved out of the memory.
        if let [module, "take"] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalMemory)) = self.names.get(*module)
        {
            let ([ty], [arg]) = (type_args, args) else {
                self.diags.error("E1104", span, "`take` takes one type argument and a pointer");
                return Self::error_expr(span);
            };
            let ty = self.resolve_type(ty);
            let pointer = self.types.intern(TyKind::Pointer(ty));
            let value = self.expr(cx, &arg.value, Some(pointer));
            let value = self.coerce(value, pointer);
            if !cx.unsafe_region {
                self.diags.error("E1701", span, "`take` is a raw operation: write it inside `unsafe { ... }`");
            }
            return Self::mk(hir::ExprKind::Pointer(hir::PointerOp::Take, vec![value]), ty, span);
        }
        // `memory.sizeOf[T]()` and `memory.alignOf[T]()`, which `std.ffi` exposes.
        if let [module, query @ ("sizeOf" | "alignOf")] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalMemory)) = self.names.get(*module)
        {
            let [ty] = type_args else {
                self.diags.error("E1104", span, format!("`{query}` takes one type argument"));
                return Self::error_expr(span);
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let ty = self.resolve_type(ty);
            if self.types.c_layout(ty).is_some_and(|l| l.opaque) {
                let message = format!("{} is opaque: C alone knows its size", self.display(ty));
                self.diags.error("E1703", span, message);
            }
            return Self::mk(hir::ExprKind::Layout(ty, *query == "alignOf"), Ty::INT, span);
        }
        // `native.unbox[T](box)`, `native.is[T](box)` and `native.typeName[T]()`
        // of `std.internal.reflect`, which `std.reflect` is written on.
        if let [module, op @ ("unbox" | "is" | "typeName" | "identity" | "capabilities")] = names.as_slice()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::InternalReflect)) = self.names.get(*module)
        {
            let boxed = self.types.handle(Handle::ReflectBox, Ty::UNIT);
            if let Some((op, result)) = match *op {
                "typeName" => Some((hir::ReflectOp::Name, Ty::STRING)),
                "identity" => Some((hir::ReflectOp::Identity, Ty::INT)),
                "capabilities" => Some((hir::ReflectOp::Capabilities, Ty::INT)),
                _ => None,
            } {
                let [ty] = type_args else {
                    self.diags.error("E1104", span, "this operation takes one type argument");
                    return Self::error_expr(span);
                };
                let ty = self.resolve_type(ty);
                return Self::mk(hir::ExprKind::Reflect(op, ty, Vec::new()), result, span);
            }
            let [arg] = args else {
                return self.arity_error(span, 1, args.len());
            };
            let [ty] = type_args else {
                self.diags.error("E1104", span, format!("`{op}` takes one type argument"));
                return Self::error_expr(span);
            };
            let ty = self.resolve_type(ty);
            let value = self.expr(cx, &arg.value, Some(boxed));
            let value = self.coerce(value, boxed);
            let (op, result) =
                if *op == "unbox" { (hir::ReflectOp::Unbox, ty) } else { (hir::ReflectOp::Is, Ty::BOOL) };
            return Self::mk(hir::ExprKind::Reflect(op, ty, vec![value]), result, span);
        }
        if let Some(retired) = self.retired_new(cx, segments, span) {
            return retired;
        }
        if let [module, _] = names.as_slice()
            && args.is_empty()
            && self.lookup(cx, module).is_none()
            && let Some(ModuleName::Module(StdModule::Sync)) = self.names.get(*module)
        {
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            return self.sync_empty(&segments[1], types, args, span);
        }
        if let ["Group"] = names.as_slice() {
            let [result] = type_args else {
                self.diags.error("E1104", span, "`Group` takes one type argument, the tasks' result");
                return Self::error_expr(span);
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let result = self.resolve_type(result);
            let ty = self.types.handle(Handle::Group, result);
            return Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::GroupNew, Vec::new()), ty, span);
        }
        if let ["Map" | "Set"] = names.as_slice() {
            let types: Vec<Ty> = type_args.iter().map(|t| self.resolve_type(t)).collect();
            let ty = match (names[0], types.as_slice()) {
                ("Map", [key, value]) => {
                    self.require_key(*key, span);
                    self.map_type(*key, *value)
                }
                ("Set", [element]) => {
                    self.require_key(*element, span);
                    self.types.intern(TyKind::Set(*element))
                }
                _ => {
                    self.diags.error("E1104", span, "wrong number of type arguments");
                    return Self::error_expr(span);
                }
            };
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            return Self::mk(hir::ExprKind::MapOp(hir::MapOp::New, Vec::new()), ty, span);
        }
        let (op, params) = match names.as_slice() {
            ["Array" | "Deque"] => (hir::ArrayOp::New, Vec::new()),
            ["Array", "withCapacity"] => (hir::ArrayOp::WithCapacity, vec![Ty::INT]),
            _ => return self.not_generic(cx, segments, span),
        };
        let [element] = type_args else {
            self.diags.error("E1104", span, format!("`{}` takes one type argument", names[0]));
            return Self::error_expr(span);
        };
        let element = self.resolve_type(element);
        let array = if names[0] == "Deque" {
            self.types.intern(TyKind::Deque(element))
        } else {
            self.types.intern(TyKind::Array(element))
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let values = args
            .iter()
            .zip(params)
            .map(|(a, t)| {
                let v = self.expr(cx, &a.value, Some(t));
                self.coerce(v, t)
            })
            .collect();
        let ty = match op {
            hir::ArrayOp::New => array,
            _ => self.types.intern(TyKind::Result(array, self.collection_error)),
        };
        Self::mk(hir::ExprKind::ArrayOp(op, values), ty, span)
    }

    /// `receiver.method(args)` on an intrinsic collection (std.collections).
    pub(super) fn method_call(
        &mut self,
        cx: &mut FnCx,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let value = self.expr(cx, receiver, None);
        self.method_of(cx, value, method, args, expected, span)
    }

    /// `Array.concat(a, b)`: a method the language gives, with its receiver
    /// written first and the loan of one that changes it in sight (11.5).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn language_call(
        &mut self,
        cx: &mut FnCx,
        owner: Ty,
        receiver: &ast::Arg,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let changes = MUTATIONS.contains(&method.name.as_str());
        let value = self.expr(cx, &receiver.value, None);
        if self.owner_key(value.ty) != owner && !self.types.has_error(value.ty) {
            let message = format!("expected {}, found {}", self.display(owner), self.display(value.ty));
            self.diags.error("E1102", value.span, message);
            return Self::error_expr(span);
        }
        match (changes, receiver.mode) {
            (true, ast::Mode::Var) | (false, ast::Mode::Value) => {}
            (true, _) => {
                let text = self.sources.file_of(value.span.start).slice(value.span);
                let message = format!("`{}` changes its receiver: write `var {text}`", method.name);
                self.diags.error("E1407", receiver.span, message);
            }
            (false, mode) => {
                let message = format!("the receiver `self` is already a shared loan: remove `{}`", mode.keyword());
                self.diags.error("E1407", receiver.span, message);
            }
        }
        self.method_of(cx, value, method, args, expected, span)
    }

    /// `method` called on a value already checked.
    pub(super) fn method_of(
        &mut self,
        cx: &mut FnCx,
        value: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = value.ty;
        if self.is_http_route(ty, method, args) {
            return self.http_route(cx, value, method, args, expected, span);
        }
        // The named operations of a number (18.9), and of any integer on a
        // `T: Integer` (18.10).
        let entry = match self.number_kind(ty) {
            Some(kind) => crate::numbers::find(kind, &method.name, true),
            None if self.bound_by(ty, self.integer) => crate::numbers::find_integer(&method.name, true),
            None => None,
        };
        if let Some(entry) = entry {
            let signature = self.number_signature(ty, entry);
            let callee = Callee::Std(StdFn::Number(entry.op));
            return self.dot_call(cx, callee, signature, HashMap::new(), value, method, args, expected, span);
        }
        let key = self.owner_key(ty);
        if let Some(&id) = self.methods.get(&(key, method.name.clone())) {
            self.index_refer(method, crate::index::Key::Fn(id.0));
            self.check_method_visible(id, method.span);
            let signature = self.signatures[id.0 as usize].clone();
            return self.dot_call(cx, Callee::Fn(id), signature, HashMap::new(), value, method, args, expected, span);
        }
        if self.is_abstract(ty) {
            // A type parameter has the methods of the traits it is bound by (12.7).
            let bounds = self.bounds_of(ty);
            let found: Vec<(TraitId, Vec<Ty>, usize)> = bounds
                .iter()
                .filter_map(|(t, a)| {
                    let index = self.traits[t.0 as usize].methods.iter().position(|m| m.name == method.name)?;
                    Some((*t, a.clone(), index))
                })
                .collect();
            return match found.as_slice() {
                [(trait_id, trait_args, index)] => {
                    self.trait_dot_call(cx, *trait_id, trait_args.clone(), *index, value, method, args, expected, span)
                }
                [] => self.no_method(ty, method, span),
                _ => {
                    let names: Vec<String> = found
                        .iter()
                        .map(|(t, ..)| format!("`{}.{}(value)`", self.traits[t.0 as usize].name, method.name))
                        .collect();
                    let message =
                        format!("`{}` comes from more than one bound; write {}", method.name, names.join(" or "));
                    self.diags.error("E1004", method.span, message);
                    Self::error_expr(span)
                }
            };
        }
        // Inside an `impl`, the implemented trait's methods on `Self` (11.5).
        if let Some((trait_id, trait_args)) = self.implementing.clone()
            && Some(ty) == self.self_type
            && let Some(index) = self.traits[trait_id.0 as usize].methods.iter().position(|m| m.name == method.name)
        {
            return self.trait_dot_call(cx, trait_id, trait_args, index, value, method, args, expected, span);
        }
        // `Iterator`'s methods on any type implementing it (11.5).
        if !self.is_abstract(ty)
            && let Some(index) = self.traits[ITERATOR.0 as usize].methods.iter().position(|m| m.name == method.name)
            && let Some(element) = self.iterator_element(ty)
        {
            return self.trait_dot_call(cx, ITERATOR, vec![element], index, value, method, args, expected, span);
        }
        // A trait's method on a concrete value, when one trait gives it (12.7);
        // those of the type itself and of the language go first.
        if !self.is_abstract(ty) && !self.language_method(ty, &method.name) {
            let givers = self.traits_with(ty, &method.name);
            match givers.as_slice() {
                [] => {}
                [giver] => {
                    let index = self.traits[giver.0 as usize].methods.iter().position(|m| m.name == method.name);
                    let index = index.expect("a trait that has the method");
                    return self.trait_dot_call(cx, *giver, Vec::new(), index, value, method, args, expected, span);
                }
                _ => {
                    let name = |t: &TraitId| format!("`{}.{}(value)`", self.traits[t.0 as usize].name, method.name);
                    let names: Vec<String> = givers.iter().map(name).collect();
                    let message =
                        format!("`{}` comes from more than one trait; write {}", method.name, names.join(" or "));
                    self.diags.error("E1004", method.span, message);
                    return Self::error_expr(span);
                }
            }
        }
        // `DateTime`'s methods, written in Tondo (14.3.4): the functions of
        // `std.time`'s companions that take a `DateTime` first.
        if Some(ty) == self.datetime
            && let Some(id) = Self::companions(StdModule::Time).iter().find_map(|companion| {
                match self.names.get(&format!("{companion}.{}", method.name)) {
                    Some(&ModuleName::Fn(id)) if self.signatures[id.0 as usize].params.first() == Some(&ty) => Some(id),
                    _ => None,
                }
            })
        {
            let signature = self.signatures[id.0 as usize].clone();
            let target = CallTarget::new(Callee::Fn(id), signature, expected);
            return self.call_signature(cx, target, Some(value), args, span);
        }
        if Some(ty) == self.duration || Some(ty) == self.instant {
            return self.time_method(cx, value, method, args, span);
        }
        if matches!(self.types.kind(ty), TyKind::Pointer(_))
            || (ty == Ty::int(IntTy::U64) && matches!(method.name.as_str(), "toPointer" | "toFunction"))
        {
            return self.raw(cx, value, method, Vec::new(), args, span);
        }
        if let Some(types) = self.process
            && ty == types.result
            && method.name == "text"
        {
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let stdout = self.field_access(value, FieldRef::Name("stdout"), method.span, span);
            let result = self.types.intern(TyKind::Result(Ty::STRING, types.error));
            let callee = Callee::Std(StdFn::Process(hir::ProcessFn::Text));
            return Self::mk(hir::ExprKind::Call(callee, Vec::new(), vec![stdout]), result, span);
        }
        if matches!(self.types.kind(ty), TyKind::Adt(_)) {
            // A field holding a function is called (11.5).
            if self.record_has_field(ty, &method.name) {
                let field = self.field_access(value, FieldRef::Name(&method.name), method.span, span);
                if self.callable(field.ty).is_some() {
                    return self.call_value(cx, field, args, None, span);
                }
                return self.no_method(ty, method, span);
            }
            return self.no_method(ty, method, span);
        }
        if MUTATIONS.contains(&method.name.as_str())
            && matches!(self.types.kind(ty), TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_))
        {
            return self.mutation_call(cx, value, method, args, span);
        }
        if ty == Ty::STRING {
            return self.string_method(cx, value, method, args, span);
        }
        if let TyKind::Handle(handle, state) = *self.types.kind(ty) {
            if handle == Handle::File {
                return self.file_method(cx, value, method, args, span);
            }
            if matches!(handle, Handle::Writer | Handle::Reader | Handle::Buffer) {
                return self.io_method(cx, value, handle, method, args, span);
            }
            if matches!(handle, Handle::Command | Handle::Pipeline | Handle::Process) {
                return self.process_method(cx, value, handle, method, args, span);
            }
            if matches!(handle, Handle::JsonReader | Handle::JsonWriter | Handle::Regex) {
                let receiver = match handle {
                    Handle::JsonReader => hir::NativeTy::JsonReader,
                    Handle::JsonWriter => hir::NativeTy::JsonWriter,
                    _ => hir::NativeTy::Regex,
                };
                let found = hir::NATIVES.iter().position(|n| n.receiver == Some(receiver) && n.name == method.name);
                let Some(index) = found else { return self.no_method(ty, method, span) };
                let native = &hir::NATIVES[index];
                if args.len() != native.params.len() {
                    return self.arity_error(span, native.params.len(), args.len());
                }
                let mut values = vec![value];
                for (arg, &param) in args.iter().zip(native.params) {
                    let param = self.native_ty(param);
                    let argument = self.expr(cx, &arg.value, Some(param));
                    values.push(self.coerce(argument, param));
                }
                let result = self.native_ty(native.result);
                let callee = Callee::Std(StdFn::Native(index as u16));
                return Self::mk(hir::ExprKind::Call(callee, Vec::new(), values), result, span);
            }
            if handle == Handle::Clock {
                return self.clock_method(cx, value, method, args, span);
            }
            // A cancelled task's `defer` ends at a wait: a lock another holds.
            if cx.deferred && matches!(handle, Handle::Mutex | Handle::RwLock) && method.name != "tryUpdate" {
                let message = "in a cancelled task this `defer` ends here if another task holds the lock, and what it \
                               does is lost: use an `Atomic`, or send to a channel with room, which need no wait";
                self.diags.warning("W1017", span, message);
                let receiver = Span::new(span.start, method.span.start.saturating_sub(1));
                if let Some(edits) = self.as_counter(self.sources.slice(receiver), span) {
                    self.diags.fix("make it a counter: an `Atomic`, which needs no lock", false, edits);
                }
            }
            if handle.locked() || matches!(handle, Handle::Semaphore | Handle::Permit | Handle::Barrier) {
                let cell = Self::cell(cx, &value)
                    .filter(|_| matches!(handle, Handle::Mutex | Handle::RwLock | Handle::Atomic));
                let checked = self.sync_method(cx, value, handle, state, method, args, span);
                if let Some(cell) = cell {
                    self.cell_use(cell, method, args, span);
                }
                return checked;
            }
            return self.handle_method(cx, value, handle, state, method, args, span);
        }
        let kind = self.types.kind(ty).clone();
        let (element, sequence) = match kind {
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => {
                (element, !matches!(kind, TyKind::Set(_)))
            }
            TyKind::Map(key, _) => (key, false),
            TyKind::Error => return Self::error_expr(span),
            _ => return self.no_method(ty, method, span),
        };
        if let Some(cursor) = match (method.name.as_str(), &kind) {
            ("entries", TyKind::Map(..)) => Some(CursorKind::Entries),
            ("keys", TyKind::Map(..)) => Some(CursorKind::Keys),
            ("values", TyKind::Map(..) | TyKind::Set(_)) => Some(CursorKind::Values),
            _ => None,
        } {
            // A lazy cursor over the collection, in insertion order (10.2.3).
            if !args.is_empty() {
                return self.arity_error(span, 0, args.len());
            }
            let (set, args) = match kind {
                TyKind::Set(element) => (true, vec![element]),
                TyKind::Map(key, value) => (false, vec![key, value]),
                _ => unreachable!("a map or set"),
            };
            let template = self.cursor_templates[&(cursor, set)];
            let cursor = self.types.instantiate(template, args);
            let start = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
            return Self::mk(hir::ExprKind::Record(vec![(0, value), (1, start)]), cursor, span);
        }
        match method.name.as_str() {
            "length" | "isEmpty" => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                let length = Self::mk(hir::ExprKind::Length(Box::new(value)), Ty::INT, span);
                if method.name == "length" {
                    return length;
                }
                let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                let kind = hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(zero));
                return Self::mk(kind, Ty::BOOL, span);
            }
            "get" | "contains" if !sequence => {
                let result = match kind {
                    TyKind::Map(_, entry) if method.name == "get" => self.types.intern(TyKind::Option(entry)),
                    _ if method.name == "contains" => Ty::BOOL,
                    _ => return self.no_method(ty, method, span),
                };
                let op = if method.name == "get" { hir::MapOp::Get } else { hir::MapOp::Contains };
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let arg = self.expr(cx, &arg.value, Some(element));
                let arg = self.coerce(arg, element);
                return Self::mk(hir::ExprKind::MapOp(op, vec![value, arg]), result, span);
            }
            _ => {}
        }
        let is_array = matches!(kind, TyKind::Array(_));
        let (op, params, result) = match method.name.as_str() {
            "get" if sequence => (hir::ArrayOp::Get, vec![Ty::INT], self.types.intern(TyKind::Option(element))),
            "slice" if is_array => {
                let result = self.types.intern(TyKind::Result(ty, self.collection_error));
                (hir::ArrayOp::SliceChecked, vec![Ty::INT, Ty::INT], result)
            }
            "concat" if is_array => (hir::ArrayOp::Concat, vec![ty], ty),
            "repeat" if is_array => (hir::ArrayOp::Repeat, vec![Ty::INT], ty),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![value];
        for (arg, ty) in args.iter().zip(params) {
            let arg = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(arg, ty));
        }
        Self::mk(hir::ExprKind::ArrayOp(op, values), result, span)
    }

    /// The intrinsic methods of `String` (19.2, 19.6).
    pub(super) fn string_method(
        &mut self,
        cx: &mut FnCx,
        text: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let char_option = self.types.intern(TyKind::Option(Ty::CHAR));
        let int_option = self.types.intern(TyKind::Option(Ty::INT));
        let checked = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
        use hir::StrOp as S;
        let (op, params, result) = match method.name.as_str() {
            "length" | "byteLength" | "isEmpty" => (S::ByteLength, vec![], Ty::INT),
            "get" => (S::Get, vec![Ty::INT], char_option),
            "concat" => (S::Concat, vec![Ty::STRING], Ty::STRING),
            "contains" => (S::Contains, vec![Ty::STRING], Ty::BOOL),
            "startsWith" => (S::StartsWith, vec![Ty::STRING], Ty::BOOL),
            "endsWith" => (S::EndsWith, vec![Ty::STRING], Ty::BOOL),
            "find" => (S::Find, vec![Ty::STRING], int_option),
            "replace" => (S::Replace, vec![Ty::STRING, Ty::STRING], Ty::STRING),
            "toLowerAscii" => (S::LowerAscii, vec![], Ty::STRING),
            "toUpperAscii" => (S::UpperAscii, vec![], Ty::STRING),
            "slice" => (S::SliceChecked, vec![Ty::INT, Ty::INT], checked),
            "bytes" => (S::Bytes, vec![], self.types.intern(TyKind::Array(Ty::BYTE))),
            _ => return self.no_method(Ty::STRING, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let mut values = vec![text];
        for (arg, ty) in args.iter().zip(params) {
            let arg = self.expr(cx, &arg.value, Some(ty));
            values.push(self.coerce(arg, ty));
        }
        let kind = match method.name.as_str() {
            "length" => hir::ExprKind::Length(Box::new(values.remove(0))),
            "isEmpty" => {
                let length = Self::mk(hir::ExprKind::StrOp(S::ByteLength, values), Ty::INT, span);
                let zero = Self::mk(hir::ExprKind::Int(0), Ty::INT, span);
                let kind = hir::ExprKind::Binary(BinOp::Eq, Box::new(length), Box::new(zero));
                return Self::mk(kind, Ty::BOOL, span);
            }
            _ => hir::ExprKind::StrOp(op, values),
        };
        Self::mk(kind, result, span)
    }

    /// The generic function a name refers to, when it is not a binding.
    pub(super) fn specialization(&self, cx: &FnCx, e: &ast::Expr) -> Option<FnId> {
        let ast::ExprKind::Path(segments) = &e.kind else { return None };
        let [name] = segments.as_slice() else { return None };
        if self.lookup(cx, &name.name).is_some() || self.visible_capture(cx, &name.name) {
            return None;
        }
        match self.names.get(&name.name) {
            Some(&ModuleName::Fn(id)) if !self.signatures[id.0 as usize].generics.is_empty() => Some(id),
            _ => None,
        }
    }

    /// An expression written where a type is meant, as a type.
    pub(super) fn type_of_expr(e: &ast::Expr) -> Option<ast::TypeExpr> {
        let kind = match &e.kind {
            ast::ExprKind::Path(segments) => ast::TypeKind::Path { segments: segments.clone(), args: Vec::new() },
            ast::ExprKind::Index { base, index } => {
                let ast::TypeKind::Path { segments, .. } = Self::type_of_expr(base)?.kind else { return None };
                ast::TypeKind::Path { segments, args: vec![Self::type_of_expr(index)?] }
            }
            _ => return None,
        };
        Some(ast::TypeExpr { kind, span: e.span })
    }

    /// A named function as a `fn(...)` value (11.7). A generic function is
    /// specialized by explicit arguments or by the expected function type.
    pub(super) fn fn_value(&mut self, id: FnId, explicit: Vec<Ty>, expected: Option<Ty>, span: Span) -> hir::Expr {
        self.repeatable(id, span);
        let signature = self.signatures[id.0 as usize].clone();
        if signature.receiver.is_some() {
            let message = format!(
                "a method is not a value; write a closure that calls it, such as `(x) {{ value.{}(x) }}`",
                signature.name.rsplit('.').next().unwrap_or_default()
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let mut inference = Inference::new(signature.generics.clone());
        if !explicit.is_empty() {
            if explicit.len() != signature.generics.len() {
                let message =
                    format!("expected {} type argument(s), found {}", signature.generics.len(), explicit.len());
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            inference.map.extend(signature.generics.iter().copied().zip(explicit));
        }
        let generic = self.types.intern(TyKind::Fn(
            signature.params.clone(),
            signature.result,
            signature.variadic,
            signature.modes.clone(),
            signature.is_unsafe,
        ));
        if let Some(expected) = expected {
            self.unify(&mut inference, generic, expected);
        }
        let Some(ty) = self.inferred(&inference, generic) else {
            let message = format!(
                "`{}` is generic: specialize it, as in `{}[Int]`, or give the expected `fn(...)` type",
                signature.name, signature.name
            );
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        };
        let type_args = signature.generics.iter().map(|g| inference.map[g]).collect();
        Self::mk(hir::ExprKind::FnValue(id, type_args), ty, span)
    }

    /// `value(args)` for a `fn(...)`, a closure or a type parameter with a
    /// call bound (11.7, 11.8). Arguments are positional.
    pub(super) fn call_value(
        &mut self,
        cx: &mut FnCx,
        callee: hir::Expr,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        if callee.ty == Ty::ERROR {
            return Self::error_expr(span);
        }
        if let TyKind::CFn(params, result, variadic) = self.types.kind(callee.ty).clone() {
            return self.c_value_call(cx, callee, (params, result, variadic), args, expected, span);
        }
        let Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe }) = self.callable(callee.ty)
        else {
            let message = format!("{} cannot be called", self.display(callee.ty));
            self.diags.error("E1102", callee.span, message);
            return Self::error_expr(span);
        };
        if is_unsafe && !cx.unsafe_region {
            let message =
                "this function value is `unsafe`: call it inside `unsafe { ... }`, once its preconditions hold";
            self.diags.error("E1701", span, message);
        }
        if mutates && self.shared_place(cx, &callee) && !self.types.is_copy(callee.ty) {
            let text = self.sources.file_of(callee.span.start).slice(callee.span);
            let message = match once {
                true => format!("`{text}` is a shared loan, and calling it would consume it: take it by value"),
                false => format!("`{text}` is a shared loan, and calling it can change it: take it as `mut`"),
            };
            self.diags.error("E1407", callee.span, message);
            return Self::error_expr(span);
        }
        if let Some(arg) = args.iter().find(|a| a.name.is_some()) {
            self.diags.error("E1102", arg.span, "a call through a function value takes positional arguments");
            return Self::error_expr(span);
        }
        let names = (0..params.len()).map(|i| format!("argument{}", i + 1)).collect();
        let signature = Signature {
            params,
            names,
            receiver: None,
            modes,
            variadic,
            result,
            generics: Vec::new(),
            name: "this function".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        // A placeholder callee; the checked arguments are what matters.
        let target = CallTarget::new(Callee::Fn(FnId(u32::MAX)), signature, expected);
        let call = self.call_signature(cx, target, None, args, span);
        let hir::ExprKind::Call(_, _, values) = call.kind else { return call };
        // A closure is called through a place it may change; a `var` binding
        // keeps the changes, and anything else is copied first (11.8).
        let direct = matches!(self.types.kind(callee.ty), TyKind::Fn(..));
        let keep = !direct && self.var_place(cx, &callee) && mutates || !direct && !mutates && self.is_place(&callee);
        if direct || keep {
            return Self::mk(hir::ExprKind::CallValue(Box::new(callee), values), call.ty, span);
        }
        let local = self.hidden_local(cx, callee.ty);
        let (ty, callee_span, result) = (callee.ty, callee.span, call.ty);
        let read = Self::mk(hir::ExprKind::Local(local), ty, callee_span);
        let call = Self::mk(hir::ExprKind::CallValue(Box::new(read), values), result, span);
        let block = hir::Block { stmts: vec![hir::Stmt::Let(local, callee)], tail: Some(Box::new(call)), ty: result };
        Self::mk(hir::ExprKind::Block(block), result, span)
    }

    /// Whether a value expression reads a place.
    pub(super) fn is_place(&self, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(_) => true,
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.is_place(base)
            }
            _ => false,
        }
    }

    /// Whether a value expression reads a place of a `var` binding.
    pub(super) fn var_place(&self, cx: &FnCx, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(local) => cx.mutable[local.0 as usize] && Some(*local) != cx.env,
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.var_place(cx, base)
            }
            _ => false,
        }
    }

    /// Whether `value` is reached through a `ref` parameter.
    pub(super) fn shared_place(&self, cx: &FnCx, value: &hir::Expr) -> bool {
        match &value.kind {
            hir::ExprKind::Local(local) => {
                cx.params[local.0 as usize] == Some(ast::Mode::Ref) && Some(*local) != cx.env
            }
            hir::ExprKind::Field(base, _) | hir::ExprKind::Retype(base) | hir::ExprKind::Index(base, _) => {
                self.shared_place(cx, base)
            }
            _ => false,
        }
    }

    /// `App()`, which is `App()`: a type is built by calling it (11.5). A
    /// type of the program may have a `new` of its own.
    pub(super) fn retired_new(&mut self, cx: &FnCx, segments: &[ast::Ident], span: Span) -> Option<hir::Expr> {
        let [prefix @ .., owner, last] = segments else { return None };
        if last.name != "new" || self.lookup(cx, &segments[0].name).is_some() {
            return None;
        }
        let ty = match prefix {
            [] => self.type_named(&owner.name),
            [module] => self.module_type(&module.name, &owner.name),
            _ => None,
        };
        if let Some(ty) = ty {
            let key = self.owner_key(ty);
            if self.methods.contains_key(&(key, "new".to_string())) {
                return None;
            }
        }
        let qualified = !prefix.is_empty() || owner.name.contains('.');
        if !qualified && !matches!(owner.name.as_str(), "Array" | "Deque" | "Group" | "Map" | "Set") {
            return None;
        }
        let name = Self::path_text(&segments[..segments.len() - 1]);
        let message = format!("a type is built by calling it: `{name}(...)`, without `.new`");
        self.diags.error("E1001", last.span, message);
        let dot = Span::new(owner.span.end, last.span.end);
        self.diags.fix(format!("write `{name}(...)`"), true, vec![(dot, String::new())]);
        Some(Self::error_expr(span))
    }

    /// `value.method(args)` for method `index` of a trait with `Self` set to
    /// the receiver's type.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trait_dot_call(
        &mut self,
        cx: &mut FnCx,
        trait_id: TraitId,
        trait_args: Vec<Ty>,
        index: usize,
        value: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        self.index_refer(method, crate::index::Key::TraitMethod(trait_id.0, method.name.clone()));
        // The prelude's methods of an iterator copy it (10.17).
        if trait_id == ITERATOR && index > 0 && !self.types.is_copy(value.ty) && !self.types.has_error(value.ty) {
            let message = format!(
                "`{}` copies the iterator, and {} is not `Copy`: go through it with `for`",
                method.name,
                self.display(value.ty)
            );
            self.diags.error("E1406", method.span, message);
            return Self::error_expr(span);
        }
        let def = &self.traits[trait_id.0 as usize];
        let signature = def.methods[index].signature.clone();
        let mut prebound = HashMap::from([(def.self_param, value.ty)]);
        prebound.extend(def.params.iter().copied().zip(trait_args));
        let callee = Callee::Trait(trait_id, index as u32);
        self.dot_call(cx, callee, signature, prebound, value, method, args, expected, span)
    }

    /// `Trait.method(...)`, with `Self` given first in `explicit` or taken
    /// from the receiver argument (12.7).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trait_call(
        &mut self,
        cx: &mut FnCx,
        trait_id: TraitId,
        method: &ast::Ident,
        explicit: Vec<Ty>,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        self.index_refer(method, crate::index::Key::TraitMethod(trait_id.0, method.name.clone()));
        let def = &self.traits[trait_id.0 as usize];
        let Some(index) = def.methods.iter().position(|m| m.name == method.name) else {
            let message = format!("trait `{}` has no method `{}`", def.name, method.name);
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        };
        let signature = def.methods[index].signature.clone();
        // `Trait.op[Self, Own...]`: the trait's parameters come from its bounds.
        let mut prebound = HashMap::new();
        if let Some((&self_ty, own)) = explicit.split_first() {
            prebound.insert(def.self_param, self_ty);
            let own_params = &signature.generics[1 + def.params.len()..];
            prebound.extend(own_params.iter().copied().zip(own.iter().copied()));
        }
        let target =
            CallTarget { prebound, ..CallTarget::new(Callee::Trait(trait_id, index as u32), signature, expected) };
        self.call_signature(cx, target, None, args, span)
    }

    /// The `for` loop over a cursor of type `ty` yielding `element` (10.17):
    /// the loop owns a copy and calls `Iterator.next` until `none`.
    pub(super) fn for_cursor(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
    ) -> hir::Stmt {
        let (ty, span) = (source.ty, source.span);
        let cursor = self.hidden_local(cx, ty);
        cx.mutable[cursor.0 as usize] = true;
        let option = self.types.intern(TyKind::Option(element));
        let loan = Self::mk(hir::ExprKind::Loan(hir::Place { local: cursor, path: Vec::new() }), ty, span);
        let next =
            Self::mk(hir::ExprKind::Call(Callee::Trait(ITERATOR, 0), vec![ty, element], vec![loan]), option, span);
        self.until_none(cx, pattern, element, body, (cursor, source), next)
    }

    /// `for value in receiver` (11.13): receives until the channel is
    /// closed and empty.
    pub(super) fn for_receiver(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        source: hir::Expr,
        element: Ty,
        body: &ast::Block,
    ) -> hir::Stmt {
        let (ty, span) = (source.ty, source.span);
        let receiver = self.hidden_local(cx, ty);
        let option = self.types.intern(TyKind::Option(element));
        let read = Self::mk(hir::ExprKind::Local(receiver), ty, span);
        let next = Self::mk(hir::ExprKind::TaskOp(hir::TaskOp::Receive, vec![read]), option, span);
        self.until_none(cx, pattern, element, body, (receiver, source), next)
    }

    /// A loop that binds `pattern` to each value `next` gives until `none`,
    /// after storing `source` in a hidden local.
    pub(super) fn until_none(
        &mut self,
        cx: &mut FnCx,
        pattern: &ast::Pattern,
        element: Ty,
        body: &ast::Block,
        (cursor, source): (LocalId, hir::Expr),
        next: hir::Expr,
    ) -> hir::Stmt {
        let span = source.span;
        let option = next.ty;
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
        let pattern_span = pattern.span;
        let some = hir::Pattern { kind: hir::PatKind::Variant(SOME, vec![pattern]), ty: option, span: pattern_span };
        let none = hir::Pattern { kind: hir::PatKind::Variant(NONE, Vec::new()), ty: option, span: pattern_span };
        let stop = hir::Block { stmts: vec![hir::Stmt::Break], tail: None, ty: Ty::NEVER };
        let arms = vec![
            hir::Arm { pattern: some, guard: None, body: Self::mk(hir::ExprKind::Block(body), Ty::UNIT, span) },
            hir::Arm { pattern: none, guard: None, body: Self::mk(hir::ExprKind::Block(stop), Ty::NEVER, span) },
        ];
        let step = Self::mk(hir::ExprKind::Match(Box::new(next), arms), Ty::UNIT, span);
        let body = hir::Block { stmts: vec![hir::Stmt::Expr(step)], tail: None, ty: Ty::UNIT };
        let stmts = vec![hir::Stmt::Let(cursor, source), hir::Stmt::Loop { condition: None, body }];
        hir::Stmt::Expr(Self::mk(hir::ExprKind::Block(hir::Block { stmts, tail: None, ty: Ty::UNIT }), Ty::UNIT, span))
    }

    /// The element type of a cursor: a type implementing `Iterator[T]` or a
    /// type parameter bound by it.
    pub(super) fn iterator_element(&mut self, ty: Ty) -> Option<Ty> {
        if self.is_abstract(ty) {
            return self.bounds_of(ty).into_iter().find(|(t, _)| *t == ITERATOR).map(|(_, args)| args[0]);
        }
        let candidates: Vec<(Ty, Ty, Vec<Ty>)> = self
            .impls
            .iter()
            .filter(|i| i.trait_id == ITERATOR)
            .map(|i| (i.target, i.trait_args[0], i.generics.clone()))
            .collect();
        for (target, element, generics) in candidates {
            let mut map = HashMap::new();
            if self.types.matches(target, ty, &generics, &mut map) {
                return Some(self.types.subst(element, &map));
            }
        }
        None
    }

    /// A method that changes a collection's extent or contents; the
    /// receiver must be a `var` place (10.5).
    pub(super) fn mutation_call(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        use hir::Mutation as M;
        let ty = receiver.ty;
        let operation = format!("`{}`", method.name);
        let Some(hir::Place { local, path }) = self.loan_place(cx, receiver, &operation, true) else {
            return Self::error_expr(span);
        };
        let option = |this: &mut Self, t: Ty| this.types.intern(TyKind::Option(t));
        let (mutation, params, result) = match (method.name.as_str(), self.types.kind(ty).clone()) {
            ("push", TyKind::Array(e) | TyKind::Deque(e)) => (M::Push, vec![e], Ty::UNIT),
            ("pushFront", TyKind::Deque(e)) => (M::PushFront, vec![e], Ty::UNIT),
            ("pop", TyKind::Array(e) | TyKind::Deque(e)) => (M::Pop, vec![], option(self, e)),
            ("popFront", TyKind::Deque(e)) => (M::PopFront, vec![], option(self, e)),
            ("insert", TyKind::Array(e)) => (M::InsertAt, vec![Ty::INT, e], Ty::UNIT),
            ("removeAt", TyKind::Array(e)) => (M::RemoveAt, vec![Ty::INT], option(self, e)),
            ("clear", TyKind::Array(_) | TyKind::Deque(_) | TyKind::Map(..) | TyKind::Set(_)) => {
                (M::Clear, vec![], Ty::UNIT)
            }
            ("insert", TyKind::Map(k, v)) => (M::Insert, vec![k, v], option(self, v)),
            ("remove", TyKind::Map(k, v)) => (M::Remove, vec![k], option(self, v)),
            ("insert", TyKind::Set(k)) => (M::Insert, vec![k], Ty::BOOL),
            ("remove", TyKind::Set(k)) => (M::Remove, vec![k], Ty::BOOL),
            (_, TyKind::Error) => return Self::error_expr(span),
            _ => return self.no_method(ty, method, span),
        };
        if args.len() != params.len() {
            return self.arity_error(span, params.len(), args.len());
        }
        let values = args
            .iter()
            .zip(params)
            .map(|(a, t)| {
                let v = self.expr(cx, &a.value, Some(t));
                self.coerce(v, t)
            })
            .collect();
        Self::mk(hir::ExprKind::Mutate(mutation, hir::Place { local, path }, values), result, span)
    }

    /// `value.method(args)` for an inherent method (11.5): the receiver is
    /// borrowed, or loaned for `mut self` and `var self`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dot_call(
        &mut self,
        cx: &mut FnCx,
        callee: Callee,
        signature: Signature,
        prebound: HashMap<Ty, Ty>,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = receiver.ty;
        let receiver = match signature.receiver {
            None => {
                let owner = self.types.display(ty);
                let message = format!(
                    "`{}` is an operation of `{owner}` without a receiver: call it as `{owner}.{}(...)`",
                    method.name, method.name
                );
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            Some(mode @ (ast::Mode::Mut | ast::Mode::Var)) => {
                let receiver_span = receiver.span;
                let operation = format!("`{}`", method.name);
                match self.loan_place(cx, receiver, &operation, mode == ast::Mode::Var) {
                    Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, receiver_span),
                    None => return Self::error_expr(span),
                }
            }
            Some(_) => receiver,
        };
        let target = CallTarget { prebound, ..CallTarget::new(callee, signature, expected) };
        self.call_signature(cx, target, Some(receiver), args, span)
    }

    pub(super) fn no_method(&mut self, ty: Ty, method: &ast::Ident, span: Span) -> hir::Expr {
        let mut message = format!("{} has no method `{}`", self.display(ty), method.name);
        if let Some(hint) = self.method_hint(ty, &method.name) {
            message = format!("{message}: {hint}");
        } else if let Some(trait_name) = self.implemented_trait_with(ty, &method.name) {
            // Implementations do not add methods to dot-calls on concrete types (12.7).
            message = format!(
                "{message}: `{0}` comes from the trait `{trait_name}`; call `{trait_name}.{0}(value, ...)`",
                method.name
            );
        } else if let Some(element) = self.iterator_element(ty)
            && let Some(hint) = self.collected_hint(element, &method.name)
        {
            // An iterator's adapters are lazy: an array has the rest.
            let message = format!("{message}: it is an iterator, and {hint}");
            self.diags.error("E1001", method.span, message);
            let call =
                if method.name == "toArray" { "collect".to_string() } else { format!("collect().{}", method.name) };
            self.diags.fix(format!("call `{call}`"), false, vec![(method.span, call)]);
            return Self::error_expr(span);
        } else if let Some(near) = self.closest_method(ty, &method.name) {
            message = format!("{message}; did you mean `{near}`?");
            self.diags.error("E1001", method.span, message);
            self.diags.fix(format!("call `{near}`"), false, vec![(method.span, near)]);
            return Self::error_expr(span);
        } else if let Some(shown) = self.api_name(ty) {
            message = format!("{message}: `tondo api {shown}` shows the methods it has");
        }
        self.diags.error("E1001", method.span, message);
        Self::error_expr(span)
    }

    /// What to write for `name` on an iterator of `element`, when an array
    /// has it, or it would make one: `collect()` first.
    fn collected_hint(&mut self, element: Ty, name: &str) -> Option<String> {
        if matches!(name, "toArray" | "toList" | "toVec" | "array" | "list") {
            return Some("`collect()` makes an array of its elements".into());
        }
        let array = self.types.intern(TyKind::Array(element));
        let key = self.owner_key(array);
        let of_array = self.methods.keys().any(|(owner, method)| *owner == key && method == name)
            || crate::api::builtin_methods("Array").contains(&name);
        of_array.then(|| format!("`collect()` makes an array of its elements, which has `{name}`"))
    }

    /// The name `tondo api` shows a type of the prelude or of a standard
    /// module by, with its methods: `String`, `Option`, `http.Response`;
    /// `none` for a type of the program.
    pub(super) fn api_name(&mut self, ty: Ty) -> Option<String> {
        let shown = match self.types.kind(ty) {
            TyKind::Option(_) => "Option".to_string(),
            TyKind::Result(..) => "Result".to_string(),
            _ => self.types.display(ty).split('[').next().unwrap_or_default().to_string(),
        };
        let known = match shown.split_once('.') {
            Some((module, _)) => {
                module == "prelude" || crate::api::standard_modules().contains(&format!("std.{module}"))
            }
            None => crate::api::standard("prelude").and_then(|api| crate::api::declared(&api, &shown)).is_some(),
        };
        known.then_some(shown)
    }

    /// The method of `ty` whose name looks like a typo of `name`: one the
    /// prelude or the program declares, or one the compiler implements.
    pub(super) fn closest_method(&mut self, ty: Ty, name: &str) -> Option<String> {
        let key = self.owner_key(ty);
        let owner = match self.types.kind(ty) {
            TyKind::String => "String".to_string(),
            TyKind::Array(_) => "Array".to_string(),
            TyKind::Deque(_) => "Deque".to_string(),
            TyKind::Map(..) => "Map".to_string(),
            TyKind::Set(_) => "Set".to_string(),
            _ => self.types.display(ty),
        };
        let declared = self.methods.keys().filter(|(k, _)| *k == key).map(|(_, n)| n.clone());
        let names: Vec<String> =
            declared.chain(crate::api::builtin_methods(&owner).iter().map(|name| name.to_string())).collect();
        closest(name, names.iter().map(String::as_str)).map(String::from)
    }

    /// A trait with a method named `name` that `ty` implements, if any.
    pub(super) fn implemented_trait_with(&mut self, ty: Ty, name: &str) -> Option<String> {
        let found = (0..self.traits.len() as u32).map(TraitId).filter(|t| self.traits[t.0 as usize].params.is_empty());
        let found: Vec<TraitId> =
            found.filter(|t| self.traits[t.0 as usize].methods.iter().any(|m| m.name == name)).collect();
        found.into_iter().find(|&t| self.satisfies(ty, t, &[])).map(|t| self.traits[t.0 as usize].name.clone())
    }

    /// The traits without parameters that give `ty` a method `name`.
    pub(super) fn traits_with(&mut self, ty: Ty, name: &str) -> Vec<TraitId> {
        let candidates: Vec<TraitId> = (0..self.traits.len() as u32)
            .map(TraitId)
            .filter(|t| self.traits[t.0 as usize].params.is_empty())
            .filter(|t| self.traits[t.0 as usize].methods.iter().any(|m| m.name == name))
            .collect();
        candidates.into_iter().filter(|&t| self.satisfies(ty, t, &[])).collect()
    }

    /// Whether the language gives a type of its own a method `name`. Of the
    /// types it has no list of, all it knows.
    pub(super) fn language_method(&self, ty: Ty, name: &str) -> bool {
        let owner = match self.types.kind(ty) {
            TyKind::Adt(_) | TyKind::Tuple(_) | TyKind::Bool | TyKind::Char | TyKind::Byte => return false,
            TyKind::Int(_) | TyKind::Float(_) => return false,
            TyKind::String => "String",
            TyKind::Array(_) => "Array",
            TyKind::Deque(_) => "Deque",
            TyKind::Map(..) => "Map",
            TyKind::Set(_) => "Set",
            _ => return true,
        };
        crate::api::builtin_methods(owner).contains(&name)
    }

    /// The canonical spelling of a method that other languages name
    /// differently (10.2.3).
    pub(super) fn method_hint(&self, ty: Ty, name: &str) -> Option<&'static str> {
        let hint = match (self.types.kind(ty), name) {
            (TyKind::Option(_), "unwrap" | "get" | "value" | "orElseThrow") => {
                "use `expect(message)`, which panics with the message on `none`; or `unwrapOr(fallback)`, `?` or `match`"
            }
            (TyKind::Option(_) | TyKind::Result(..), "unwrap_or" | "getOrElse" | "orElseGet" | "valueOr") => {
                "use `unwrapOr(fallback)`"
            }
            (TyKind::Option(_), "isPresent" | "is_some" | "hasValue") => "use `isSome()`",
            (TyKind::Option(_), "is_none" | "isEmpty" | "isAbsent") => "use `isNone()`",
            (TyKind::Result(..), "unwrap" | "get" | "value") => {
                "use `expect(message)`, which panics with the message on an error; or `unwrapOr(fallback)`, `?` or `match`"
            }
            (TyKind::Result(..), "is_ok" | "isSuccess") => "use `isOk()`",
            (TyKind::Result(..), "is_err" | "isFailure" | "isError") => "use `isErr()`",
            (TyKind::Result(..), "map_err" | "mapError") => "use `mapErr(transform)`",
            (TyKind::String, "toInt" | "parseInt" | "toInteger" | "toNumber" | "parse" | "atoi" | "toLong") => {
                "use `Int.parse(text)`, which gives `Int ! TextError`, or `Float.parse(text)`"
            }
            (TyKind::String, "toFloat" | "toDouble" | "parseFloat") => "use `Float.parse(text)`",
            (TyKind::String, "toLowerCase" | "lower" | "to_lowercase" | "downcase" | "lowercased") => "use `toLower()`",
            (TyKind::String, "toUpperCase" | "upper" | "to_uppercase" | "upcase" | "uppercased") => "use `toUpper()`",
            (TyKind::String, "strip" | "trimmed" | "trimSpace") => "use `trim()`, `trimStart()` or `trimEnd()`",
            (TyKind::String, "trimChars" | "trimMatches" | "trim_matches" | "trimFunc") => {
                "use `trim(characters)`, `trimStart(characters)` or `trimEnd(characters)`"
            }
            (TyKind::String, "cut" | "partition" | "split_once" | "splitAt") => {
                "use `splitOnce(separator)`, which gives `(String, String)?`"
            }
            (TyKind::String, "splitN" | "splitn") => "use `split(separator, limit: count)`",
            (TyKind::String, "indexOf" | "index" | "firstIndex") => "use `find(text)`, which gives `Int?`",
            (TyKind::String, "includes" | "has") => "use `contains(text)`",
            (TyKind::String, "starts_with" | "hasPrefix") => "use `startsWith(prefix)`",
            (TyKind::String, "ends_with" | "hasSuffix") => "use `endsWith(suffix)`",
            (TyKind::String, "format") => "interpolate the values: `\"{first} and {second}\"`",
            (TyKind::String, "equalsIgnoreCase" | "equalFold" | "eq_ignore_ascii_case" | "caseInsensitiveEquals") => {
                "use `equalsIgnoringCase(other)`"
            }
            (_, "toString" | "to_string" | "str" | "toStr" | "description") => "interpolate it: `\"{value}\"`",
            (_, "equals" | "eq" | "isEqual") => "compare with `==`",
            (_, "clone" | "copy" | "dup" | "deepCopy") => "values are copied on assignment: `let copy = value`",
            (TyKind::Array(_), "forEach" | "for_each" | "each") => "use `for value in array { ... }`",
            (TyKind::Array(_), "sort_by" | "sortWith" | "sorted_by") => "use `sortBy(key)`, or `sortedWith(compare)`",
            (TyKind::Array(_), "addAll" | "append_all" | "push_all") => "use `extend(other)`",
            (TyKind::Map(..), "forEach" | "each" | "items" | "iter") => "use `for (key, value) in map { ... }`",
            (TyKind::Map(..), "getOrDefault" | "get_or_default" | "get_or") => "use `getOr(key, fallback)`",
            (_, "size" | "count" | "len") => "use `length()`",
            (_, "empty") => "use `isEmpty()`",
            (TyKind::String, "charAt" | "at") => "use `text[index]`, or `get(index)` for a `Char?`",
            (TyKind::String, "substring" | "substr" | "slice") => "use `text[start:end]`",
            (TyKind::Array(_) | TyKind::Deque(_), "append" | "add" | "enqueue" | "pushBack") => "use `push(value)`",
            (TyKind::Array(_) | TyKind::Deque(_), "peek" | "last" | "back") => "use `get(-1)`, which returns `T?`",
            (TyKind::Array(_) | TyKind::Deque(_), "first" | "front") => "use `get(0)`, which returns `T?`",
            (TyKind::Array(_) | TyKind::Deque(_), "popBack") => "use `pop()`",
            (TyKind::Deque(_), "dequeue" | "shift") => "use `popFront()`",
            (TyKind::Deque(_), "unshift" | "prepend") => "use `pushFront(value)`",
            (TyKind::Array(_), "dequeue" | "shift" | "popFront") => {
                "use `removeAt(0)`, or a `Deque` for a queue: `Deque[T]()` has `popFront()`"
            }
            (TyKind::Array(_), "unshift" | "prepend" | "pushFront") => {
                "use `insert(0, value)`, or a `Deque` for a queue: `Deque[T]()` has `pushFront(value)`"
            }
            (TyKind::Array(_), "remove") => "use `removeAt(index)`",
            (TyKind::Array(_), "indexOf" | "includes") => "use `value in array`",
            (TyKind::Deque(_), "insert" | "removeAt") => "a `Deque` only changes at its ends",
            (TyKind::Map(..), "put" | "set") => "use `insert(key, value)` or `map[key] = value`",
            (TyKind::Map(..), "containsKey" | "has") => "use `contains(key)`",
            (TyKind::Map(..) | TyKind::Set(_), "delete") => "use `remove(key)`",
            (TyKind::Set(_), "add") => "use `insert(value)`",
            (TyKind::Set(_), "has" | "includes") => "use `contains(value)`",
            _ => return None,
        };
        Some(hint)
    }

    pub(super) fn call(
        &mut self,
        cx: &mut FnCx,
        callee: &ast::Expr,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        match &callee.kind {
            ast::ExprKind::Field(receiver, method) => {
                return self.method_call(cx, receiver, method, args, expected, span);
            }
            ast::ExprKind::Path(segments)
                if segments.len() > 1
                    && (self.lookup(cx, &segments[0].name).is_some()
                        || self.visible_capture(cx, &segments[0].name)
                        || self.template_field(cx, &segments[0].name).is_some()
                        || matches!(self.names.get(&segments[0].name), Some(ModuleName::Const(_)))) =>
            {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Request.user.unwrapOr("")`: a method of a context's field.
            ast::ExprKind::Path(segments) if segments.len() > 2 && self.is_context(&segments[0].name) => {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix.last().unwrap().span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Float.nan.isNaN()`: a method of an intrinsic constant.
            ast::ExprKind::Path(segments)
                if segments.len() == 3
                    && !self.names.contains_key(&segments[0].name)
                    && self.lookup(cx, &segments[0].name).is_none()
                    && self
                        .intrinsic_owner(&segments[0].name)
                        .is_some_and(|ty| self.intrinsic_constant(ty, &segments[1].name).is_some()) =>
            {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix[1].span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            // `Color.Red.describe()`: a method of a variant without payload.
            ast::ExprKind::Path(segments) if segments.len() == 3 && self.unit_variant(&segments[..2]) => {
                let (method, prefix) = segments.split_last().unwrap();
                let receiver_span = prefix[0].span.to(prefix[1].span);
                let receiver = ast::Expr { kind: ast::ExprKind::Path(prefix.to_vec()), span: receiver_span };
                return self.method_call(cx, &receiver, method, args, expected, span);
            }
            ast::ExprKind::Generic { base, args: type_args } => {
                return self.generic_call(cx, base, type_args, args, expected, span);
            }
            // `call().cast[U]()`: written after a call, the type argument
            // parses as an index.
            ast::ExprKind::Index { base, index } if matches!(&base.kind, ast::ExprKind::Field(_, m) if matches!(m.name.as_str(), "cast" | "toPointer" | "toFunction")) => {
                if let Some(ty) = Self::type_of_expr(index) {
                    return self.generic_call(cx, base, &[ty], args, expected, span);
                }
            }
            _ => {}
        }
        let ast::ExprKind::Path(segments) = &callee.kind else {
            // Any other expression is a value to call (11.7).
            let value = self.expr(cx, callee, None);
            return self.call_value(cx, value, args, expected, span);
        };
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if self.lookup(cx, names[0]).is_some() || self.visible_capture(cx, names[0]) {
            let value = self.expr(cx, callee, None);
            return self.call_value(cx, value, args, expected, span);
        }
        match names.as_slice() {
            ["Pointer", operation @ ("read" | "write" | "offset" | "address")] => {
                // The qualified form takes the pointer first (16.12).
                let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                let receiver = self.expr(cx, &first.value, None);
                let method = ast::Ident { name: (*operation).to_string(), span: segments[1].span };
                if !matches!(self.types.kind(receiver.ty), TyKind::Pointer(_)) && !self.types.has_error(receiver.ty) {
                    let message = format!("expected a `Pointer[T]`, found {}", self.display(receiver.ty));
                    self.diags.error("E1102", receiver.span, message);
                    return Self::error_expr(span);
                }
                return self.raw(cx, receiver, &method, Vec::new(), rest, span);
            }
            ["String", "empty"] => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                return Self::mk(hir::ExprKind::Str(String::new()), Ty::STRING, span);
            }
            ["String", "fromBytes"] => {
                // `ok(text)`, or `InvalidData` when the bytes are not UTF-8.
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                let value = self.expr(cx, &arg.value, Some(bytes));
                let value = self.coerce(value, bytes);
                let maybe = self.types.intern(TyKind::Option(Ty::STRING));
                let ty = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
                let decoded = Self::mk(hir::ExprKind::StrOp(hir::StrOp::FromBytes, vec![value]), maybe, span);
                let text = self.hidden_local(cx, Ty::STRING);
                let pattern = |kind: hir::PatKind, ty: Ty| hir::Pattern { kind, ty, span };
                let read = Self::mk(hir::ExprKind::Local(text), Ty::STRING, span);
                let invalid = Self::mk(hir::ExprKind::Variant(3, Vec::new()), self.text_error, span);
                let arms = vec![
                    hir::Arm {
                        pattern: pattern(
                            hir::PatKind::Variant(SOME, vec![pattern(hir::PatKind::Binding(text), Ty::STRING)]),
                            maybe,
                        ),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(OK, vec![(0, read)]), ty, span),
                    },
                    hir::Arm {
                        pattern: pattern(hir::PatKind::Variant(NONE, Vec::new()), maybe),
                        guard: None,
                        body: Self::mk(hir::ExprKind::Variant(ERR, vec![(0, invalid)]), ty, span),
                    },
                ];
                return Self::mk(hir::ExprKind::Match(Box::new(decoded), arms), ty, span);
            }
            ["String", "fromChars"] => {
                let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                let chars = self.types.intern(TyKind::Array(Ty::CHAR));
                let value = self.expr(cx, &arg.value, Some(chars));
                let value = self.coerce(value, chars);
                // Building the text cannot fail short of resource exhaustion.
                let text = Self::mk(hir::ExprKind::StrOp(hir::StrOp::FromChars, vec![value]), Ty::STRING, span);
                let ty = self.types.intern(TyKind::Result(Ty::STRING, self.text_error));
                return Self::mk(hir::ExprKind::Variant(OK, vec![(0, text)]), ty, span);
            }
            _ => {}
        }
        if let Some(retired) = self.retired_new(cx, segments, span) {
            return retired;
        }
        if let ["Array", "withCapacity"] = names.as_slice() {
            let message = "`Array.withCapacity` needs its element type: `Array.withCapacity[T](...)`";
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        if let [kind @ ("Array" | "Deque" | "Group")] = names.as_slice() {
            self.diags.error("E1101", span, format!("`{kind}` needs its element type: `{kind}[T]()`"));
            return Self::error_expr(span);
        }
        if let [kind @ ("Map" | "Set")] = names.as_slice() {
            let types = if *kind == "Map" { "K, V" } else { "T" };
            self.diags.error("E1101", span, format!("`{kind}` needs what it holds: `{kind}[{types}]()`"));
            return Self::error_expr(span);
        }
        if let ["Ref"] = names.as_slice()
            && self.lookup(cx, "Ref").is_none()
        {
            return self.ref_new(cx, args, None, expected, span);
        }
        if let ["WeakRef"] = names.as_slice()
            && self.lookup(cx, "WeakRef").is_none()
        {
            return self.weak_new(cx, args, expected, span);
        }
        if let ["channel"] = names.as_slice() {
            let message = "`channel` needs the type of its values: `channel[T]()`, or `channel[T](capacity)`";
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        // `Trait.method(value, ...)` calls an implementation explicitly (12.7).
        if let [trait_name, _] = names.as_slice()
            && let Some(&trait_id) = self.trait_names.get(*trait_name)
            && !self.names.contains_key(*trait_name)
            && !self.type_params.iter().any(|(name, _)| name == trait_name)
        {
            self.index_refer(&segments[0], crate::index::Key::Trait(trait_id.0));
            return self.trait_call(cx, trait_id, &segments[1], Vec::new(), args, expected, span);
        }
        // A method of a type of the language, with its receiver written first (11.5).
        if let [type_name, operation] = names.as_slice()
            && !self.names.contains_key(*type_name)
            && let Some(owner) = self.intrinsic_owner(type_name)
            && self.number_kind(owner).is_none()
        {
            if let Some(&id) = self.methods.get(&(owner, operation.to_string()))
                && self.signatures[id.0 as usize].receiver.is_some()
            {
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                return self.call_signature(cx, target, None, args, span);
            }
            if crate::api::builtin_methods(type_name).contains(operation) {
                let [first, rest @ ..] = args else { return self.arity_error(span, 1, 0) };
                return self.language_call(cx, owner, first, &segments[1], rest, expected, span);
            }
        }
        // `Type.operation(...)`: an associated operation, or a method with
        // its receiver written first (11.5). Operations and variants never
        // share a name.
        if let [type_name, operation] = names.as_slice()
            && matches!(self.names.get(*type_name), None | Some(ModuleName::Fn(_)))
            && let Some(owner) = self.type_named(type_name)
        {
            let integer = self.bound_by(owner, self.integer);
            if let Some(kind) = self.number_kind(owner).or(integer.then_some(crate::numbers::Kind::Int(IntTy::I64))) {
                let find = |receiver: bool| match integer {
                    true => crate::numbers::find_integer(operation, receiver),
                    false => crate::numbers::find(kind, operation, receiver),
                };
                if let Some(entry) = find(false) {
                    return self.number_function(cx, owner, entry, args, expected, span);
                }
                // A method with its receiver written first: `Int.abs(x)` (11.5).
                if let Some(entry) = find(true)
                    && let Some((first, rest)) = args.split_first()
                {
                    let receiver = self.expr(cx, &first.value, Some(owner));
                    let receiver = self.coerce(receiver, owner);
                    let signature = self.number_signature(owner, entry);
                    let callee = Callee::Std(StdFn::Number(entry.op));
                    let target = CallTarget::new(callee, signature, expected);
                    return self.call_signature(cx, target, Some(receiver), rest, span);
                }
            }
            if let Some(id) = self.methods.get(&(owner, operation.to_string())).copied() {
                self.index_refer(&segments[0], crate::index::Key::Type(segments[0].name.clone()));
                self.index_refer(&segments[1], crate::index::Key::Fn(id.0));
                self.check_method_visible(id, span);
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                return self.call_signature(cx, target, None, args, span);
            }
            if self.types.variants(owner).is_none()
                || matches!(self.types.kind(owner), TyKind::Option(_) | TyKind::Result(..))
            {
                let message = format!("`{type_name}` has no operation named `{operation}`");
                self.diags.error("E1001", segments[1].span, message);
                return Self::error_expr(span);
            }
        }
        if let Some(resolved) = self.variant_path(segments) {
            if let Some(arg) = args.iter().find(|a| a.name.is_some() || a.mode != ast::Mode::Value || a.spread) {
                return self.unsupported_expr(arg.span, "this argument form in a variant constructor");
            }
            let Some((ty, variant)) = resolved else { return Self::error_expr(span) };
            let Payload::Tuple(types) = self.payload(ty, variant) else {
                let message = format!("`{}` does not take positional values", Self::path_text(segments));
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            };
            if types.len() != args.len() {
                return self.arity_error(span, types.len(), args.len());
            }
            // A generic enum's arguments come from the expected type and the
            // payload; values that need context wait for the others.
            let mut inference = self.generic_inference(ty, expected);
            let mut checked: Vec<Option<hir::Expr>> = (0..args.len()).map(|_| None).collect();
            for pass in 0..2 {
                for (index, (arg, &payload)) in args.iter().zip(&types).enumerate() {
                    if checked[index].is_some() {
                        continue;
                    }
                    let expected = match &inference {
                        Some(inference) => self.inferred(inference, payload),
                        None => Some(payload),
                    };
                    if pass == 0 && expected.is_none() && self.needs_context(&arg.value) {
                        continue;
                    }
                    let value = self.expr(cx, &arg.value, expected);
                    if let Some(inference) = &mut inference {
                        self.unify(inference, payload, value.ty);
                    }
                    checked[index] = Some(value);
                }
            }
            let values: Vec<hir::Expr> = checked.into_iter().map(Option::unwrap).collect();
            let (ty, map) = match inference {
                Some(inference) => (self.finish_inference(&inference, ty, span), inference.map),
                None => (ty, HashMap::new()),
            };
            let values = values
                .into_iter()
                .zip(types)
                .enumerate()
                .map(|(index, (value, payload))| {
                    let payload = self.types.subst(payload, &map);
                    (index as u32, self.coerce(value, payload))
                })
                .collect();
            return Self::mk(hir::ExprKind::Variant(variant, values), ty, span);
        }
        match (names.as_slice(), self.names.get(names[0]).copied()) {
            ([_, member], Some(ModuleName::Module(module))) if let Some(id) = self.companion_fn(module, member) => {
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            (["panic"], _) => self.panic_call(cx, args, span),
            (["assert"], _) => {
                let Some((condition, parts)) = args.split_first() else {
                    return self.arity_error(span, 1, 0);
                };
                let source = self.sources.slice(condition.value.span).to_string();
                let written = matches!(condition.value.kind, ast::ExprKind::Binary(..));
                let condition = self.expr(cx, &condition.value, Some(Ty::BOOL));
                let condition = self.coerce(condition, Ty::BOOL);
                let (sides, condition, detail) = match written {
                    true => self.compared(cx, condition),
                    false => (Vec::new(), condition, None),
                };
                let parts = parts
                    .iter()
                    .map(|p| {
                        let part = self.expr(cx, &p.value, Some(Ty::STRING));
                        self.coerce(part, Ty::STRING)
                    })
                    .collect();
                let condition = Box::new(condition);
                let assert = Self::mk(hir::ExprKind::Assert { condition, parts, source, detail }, Ty::UNIT, span);
                if sides.is_empty() {
                    return assert;
                }
                let block = hir::Block { stmts: sides, tail: Some(Box::new(assert)), ty: Ty::UNIT };
                Self::mk(hir::ExprKind::Block(block), Ty::UNIT, span)
            }
            ([_], Some(ModuleName::Newtype(ty))) => {
                self.index_refer(&segments[0], crate::index::Key::Type(segments[0].name.clone()));
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let TyKind::Adt(id) = self.types.kind(ty) else { unreachable!() };
                let AdtKind::Newtype(inner) = self.types.adt(*id).kind else { unreachable!() };
                let value = self.expr(cx, &arg.value, Some(inner));
                let value = self.coerce(value, inner);
                Self::mk(hir::ExprKind::Retype(Box::new(value)), ty, span)
            }
            ([_], Some(ModuleName::Fn(id))) if Some(id) == self.main_fn() => self.main_called(span),
            ([_], Some(ModuleName::Fn(id))) => {
                self.index_refer(&segments[0], crate::index::Key::Fn(id.0));
                let signature = self.signatures[id.0 as usize].clone();
                let target = CallTarget::new(Callee::Fn(id), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_], Some(ModuleName::Extern(id))) => self.extern_call(cx, id, args, expected, span),
            ([_, member], Some(ModuleName::Module(StdModule::Console))) => {
                // `print` and `println` only append to the output buffer and
                // cannot fail; delivery errors surface through `flush` (19.1).
                let writer = self.types.handle(Handle::Writer, Ty::UNIT);
                let reader = self.types.handle(Handle::Reader, Ty::UNIT);
                let (function, params, result) = match *member {
                    "print" => (StdFn::ConsolePrint, vec![Ty::STRING], Ty::UNIT),
                    "println" => (StdFn::ConsolePrintln, vec![Ty::STRING], Ty::UNIT),
                    "eprint" => (StdFn::ConsoleEprint, vec![Ty::STRING], Ty::UNIT),
                    "eprintln" => (StdFn::ConsoleEprintln, vec![Ty::STRING], Ty::UNIT),
                    "flush" => {
                        let failing = self.types.intern(TyKind::Result(Ty::UNIT, self.io_error));
                        (StdFn::ConsoleFlush, Vec::new(), failing)
                    }
                    "readLine" => {
                        let line = self.types.intern(TyKind::Option(Ty::STRING));
                        (StdFn::ConsoleReadLine, Vec::new(), line)
                    }
                    "stdout" => (StdFn::ConsoleStream(0), Vec::new(), writer),
                    "stderr" => (StdFn::ConsoleStream(1), Vec::new(), writer),
                    "stdin" => (StdFn::ConsoleStream(2), Vec::new(), reader),
                    "setOutput" => (StdFn::ConsoleSet(0), vec![writer], Ty::UNIT),
                    "setError" => (StdFn::ConsoleSet(1), vec![writer], Ty::UNIT),
                    "setInput" => (StdFn::ConsoleSet(2), vec![reader], Ty::UNIT),
                    _ => {
                        let message = format!("module `console` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Testing))) => {
                let (function, param) = match *member {
                    // An assertion that fails, or a skip: both end the test
                    // or suite where they are called.
                    "failNow" | "skip" => {
                        let [arg] = args else {
                            return self.arity_error(span, 1, args.len());
                        };
                        let message = self.expr(cx, &arg.value, Some(Ty::STRING));
                        let message = Box::new(self.coerce(message, Ty::STRING));
                        // Cleanup runs to its end: a `skip` there is a panic (testing 7.4).
                        let code = match (*member, cx.deferred) {
                            ("failNow", _) => hir::FAIL_NOW,
                            (_, true) => hir::SKIP_IN_CLEANUP,
                            _ => hir::SKIP,
                        };
                        return Self::mk(hir::ExprKind::Panic(message, code), Ty::NEVER, span);
                    }
                    "withVirtualTime" => return self.with_virtual_time(cx, args, span),
                    "bench" => return self.bench(cx, args, span),
                    "fuzz" => return self.fuzz(cx, args, span),
                    "keep" => {
                        let [arg] = args else {
                            return self.arity_error(span, 1, args.len());
                        };
                        let value = self.expr(cx, &arg.value, None);
                        let call = hir::ExprKind::Call(Callee::Std(StdFn::Keep), vec![value.ty], vec![value]);
                        return Self::mk(call, Ty::UNIT, span);
                    }
                    "log" => (StdFn::TestLog, vec![Ty::STRING]),
                    "tags" => (StdFn::TestTags, vec![self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING))]),
                    "attach" => {
                        (StdFn::TestAttach, vec![Ty::STRING, Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))])
                    }
                    "snapshot" => return self.snapshot(cx, args, span),
                    _ => {
                        let message = format!("module `testing` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(param, Ty::UNIT);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member @ ("offer" | "tryOffer")], Some(ModuleName::Module(StdModule::InternalChannel))) => {
                let [sender, value] = args else { return self.arity_error(span, 2, args.len()) };
                let sender = self.expr(cx, &sender.value, None);
                let TyKind::Handle(Handle::Sender, item) = *self.types.kind(sender.ty) else {
                    let message = format!("`{member}` sends through a `Sender`, found {}", self.display(sender.ty));
                    self.diags.error("E1102", sender.span, message);
                    return Self::error_expr(span);
                };
                let value = self.expr(cx, &value.value, Some(item));
                let value = self.coerce(value, item);
                let op = if *member == "offer" { hir::TaskOp::Offer } else { hir::TaskOp::TryOffer };
                Self::mk(hir::ExprKind::TaskOp(op, vec![sender, value]), Ty::INT, span)
            }
            (
                [_, member],
                Some(ModuleName::Module(
                    module @ (StdModule::InternalJson
                    | StdModule::InternalMemory
                    | StdModule::InternalRegex
                    | StdModule::InternalReflect
                    | StdModule::InternalText
                    | StdModule::InternalMath
                    | StdModule::InternalMeta
                    | StdModule::InternalCodec
                    | StdModule::InternalHttp
                    | StdModule::InternalToml
                    | StdModule::InternalYaml
                    | StdModule::InternalMsgpack
                    | StdModule::InternalProtobuf
                    | StdModule::InternalGrpc
                    | StdModule::InternalCbor
                    | StdModule::InternalNet
                    | StdModule::InternalFuzz
                    | StdModule::InternalModel
                    | StdModule::InternalPanic
                    | StdModule::InternalTime
                    | StdModule::InternalContext
                    | StdModule::InternalFs
                    | StdModule::InternalEnv
                    | StdModule::InternalProcess
                    | StdModule::InternalSql
                    | StdModule::InternalImage),
                )),
            ) => {
                // `built()`: what `env.build()` reports, read now and kept as
                // a constant (standard library 14.3.6).
                if module == StdModule::InternalEnv && *member == "built" {
                    let described = self.built.as_ref().map(crate::built::describe).unwrap_or_default();
                    return Self::mk(hir::ExprKind::Str(described), Ty::STRING, span);
                }
                // `joined(values, separator)`: the elements' `Display`, with
                // the separator between, into one text, as `Array.join`.
                if module == StdModule::InternalText && *member == "joined" {
                    let [values, separator] = args else { return self.arity_error(span, 2, args.len()) };
                    let values = self.expr(cx, &values.value, None);
                    let separator = self.expr(cx, &separator.value, Some(Ty::STRING));
                    let separator = self.coerce(separator, Ty::STRING);
                    let joined = hir::ExprKind::ArrayOp(hir::ArrayOp::Join, vec![values, separator]);
                    return Self::mk(joined, Ty::STRING, span);
                }
                // `nativeOrder(values)` and `sortedNatively(values)`: what
                // the runtime sorts, as `Array.sorted` does.
                if module == StdModule::InternalText && matches!(*member, "nativeOrder" | "sortedNatively") {
                    let [values] = args else { return self.arity_error(span, 1, args.len()) };
                    let values = self.expr(cx, &values.value, None);
                    let (op, ty) = match *member {
                        "nativeOrder" => (hir::ArrayOp::NativeOrder, Ty::BOOL),
                        _ => (hir::ArrayOp::SortedNatively, values.ty),
                    };
                    return Self::mk(hir::ExprKind::ArrayOp(op, vec![values]), ty, span);
                }
                if module == StdModule::InternalReflect && matches!(*member, "box" | "describe") {
                    let [arg] = args else {
                        return self.arity_error(span, 1, args.len());
                    };
                    let value = self.expr(cx, &arg.value, None);
                    let (op, result) = match *member {
                        "box" => (hir::ReflectOp::Box, self.types.handle(Handle::ReflectBox, Ty::UNIT)),
                        _ => (hir::ReflectOp::Describe, Ty::STRING),
                    };
                    return Self::mk(hir::ExprKind::Reflect(op, value.ty, vec![value]), result, span);
                }
                let module = match module {
                    StdModule::InternalJson => "json",
                    StdModule::InternalMemory => "memory",
                    StdModule::InternalRegex => "regex",
                    StdModule::InternalText => "text",
                    StdModule::InternalMath => "math",
                    StdModule::InternalMeta => "meta",
                    StdModule::InternalCodec => "codec",
                    StdModule::InternalHttp => "http",
                    StdModule::InternalToml => "toml",
                    StdModule::InternalYaml => "yaml",
                    StdModule::InternalMsgpack => "msgpack",
                    StdModule::InternalProtobuf => "protobuf",
                    StdModule::InternalGrpc => "grpc",
                    StdModule::InternalCbor => "cbor",
                    StdModule::InternalNet => "net",
                    StdModule::InternalFuzz => "fuzz",
                    StdModule::InternalModel => "model",
                    StdModule::InternalPanic => "panic",
                    StdModule::InternalTime => "time",
                    StdModule::InternalContext => "context",
                    StdModule::InternalFs => "fs",
                    StdModule::InternalEnv => "env",
                    StdModule::InternalProcess => "process",
                    StdModule::InternalSql => "sql",
                    StdModule::InternalImage => "image",
                    _ => "reflect",
                };
                let found =
                    hir::NATIVES.iter().position(|n| n.receiver.is_none() && n.module == module && n.name == *member);
                let Some(index) = found else {
                    let message = format!("module `{}` has no function `{member}`", segments[0].name);
                    self.diags.error("E1001", segments[1].span, message);
                    return Self::error_expr(span);
                };
                let native = &hir::NATIVES[index];
                let params = native.params.iter().map(|&t| self.native_ty(t)).collect();
                let signature = Signature::simple(params, self.native_ty(native.result));
                let target = CallTarget::new(Callee::Std(StdFn::Native(index as u16)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Process))) => {
                use hir::ProcessFn as P;
                let command = self.types.handle(Handle::Command, Ty::UNIT);
                let s = Ty::STRING;
                let mut signature = match *member {
                    "command" => {
                        let arguments = self.types.intern(TyKind::Array(s));
                        let mut signature = Signature::simple(vec![s, arguments], command);
                        signature.names = vec!["program".into(), "arguments".into()];
                        signature.variadic = true;
                        signature
                    }
                    "shell" => Signature::simple(vec![s], command),
                    "exit" => Signature::simple(vec![Ty::INT], Ty::NEVER),
                    "signals" => {
                        let signal = self.process.expect("declared with the module").signal;
                        let wanted = self.types.intern(TyKind::Array(signal));
                        let mut signature =
                            Signature::simple(vec![wanted], self.types.handle(Handle::Receiver, signal));
                        signature.names = vec!["wanted".into()];
                        signature.variadic = true;
                        signature
                    }
                    _ => {
                        let message =
                            format!("module `process` has no function `{member}`{}", std_alias("process", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let function = match *member {
                    "command" => P::Command,
                    "shell" => P::Shell,
                    "signals" => P::Signals,
                    _ => P::Exit,
                };
                if function == P::Shell {
                    signature.names = vec!["text".into()];
                }
                let target = CallTarget::new(Callee::Std(StdFn::Process(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Env))) => {
                use hir::EnvFn as E;
                let s = Ty::STRING;
                let names = self.types.intern(TyKind::Array(s));
                let map = self.types.intern(TyKind::Map(s, s));
                let (function, params, result) = match *member {
                    "args" => (E::Args, vec![], names),
                    "get" => (E::Get, vec![("name", s)], self.types.intern(TyKind::Option(s))),
                    "vars" => (E::Vars, vec![], map),
                    "set" => (E::Set, vec![("name", s), ("value", s)], Ty::UNIT),
                    "setAll" => (E::SetAll, vec![("values", map)], Ty::UNIT),
                    "remove" => (E::Remove, vec![("name", s)], Ty::UNIT),
                    "currentDirectory" => (E::CurrentDirectory, vec![], s),
                    "setCurrentDirectory" => {
                        let error = self.env_error.expect("declared with the module");
                        (E::SetCurrentDirectory, vec![("path", s)], self.types.intern(TyKind::Result(Ty::UNIT, error)))
                    }
                    "syncNative" => (E::SyncNative, vec![], Ty::UNIT),
                    "hostname" => (E::Hostname, vec![], s),
                    "processId" => (E::ProcessId, vec![], Ty::INT),
                    "executable" => (E::Executable, vec![], s),
                    "invokedAs" => (E::InvokedAs, vec![], s),
                    _ => {
                        let message = format!("module `env` has no function `{member}`{}", std_alias("env", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let (names, params): (Vec<&str>, Vec<Ty>) = params.into_iter().unzip();
                let mut signature = Signature::simple(params, result);
                signature.names = names.into_iter().map(String::from).collect();
                let target = CallTarget::new(Callee::Std(StdFn::Env(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Io))) => {
                use hir::IoFn as I;
                let (function, params, result) = match *member {
                    "reader" => (I::Reader, vec![Ty::STRING], self.types.handle(Handle::Reader, Ty::UNIT)),
                    "discard" => (I::Discard, Vec::new(), self.types.handle(Handle::Writer, Ty::UNIT)),
                    "Buffer" => (I::BufferNew, Vec::new(), self.types.handle(Handle::Buffer, Ty::UNIT)),
                    _ => {
                        let message = format!("module `io` has no function `{member}`");
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(StdFn::Io(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Sync))) if args.is_empty() => {
                self.sync_empty_expected(&segments[1], args, expected, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Sync))) => self.sync_new(cx, member, args, expected, span),
            ([_, member], Some(ModuleName::Module(StdModule::Fs))) => {
                let fs = self.fs.expect("declared with the module");
                let failing = |this: &mut Self, ty: Ty| this.types.intern(TyKind::Result(ty, fs.error));
                let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
                let names = self.types.intern(TyKind::Array(Ty::STRING));
                let s = Ty::STRING;
                use hir::FsFn as F;
                let (function, params, result) = match *member {
                    "readText" => (F::ReadText, vec![("path", s)], failing(self, s)),
                    "readBytes" => (F::ReadBytes, vec![("path", s)], failing(self, bytes)),
                    "writeText" => (F::WriteText, vec![("path", s), ("text", s)], failing(self, Ty::UNIT)),
                    "writeBytes" => (F::WriteBytes, vec![("path", s), ("data", bytes)], failing(self, Ty::UNIT)),
                    "appendText" => (F::AppendText, vec![("path", s), ("text", s)], failing(self, Ty::UNIT)),
                    "exists" => (F::Exists, vec![("path", s)], Ty::BOOL),
                    "list" => (F::List, vec![("path", s)], failing(self, names)),
                    "createDirectory" => (F::CreateDirectory, vec![("path", s)], failing(self, Ty::UNIT)),
                    "rename" => (F::Rename, vec![("from", s), ("to", s)], failing(self, Ty::UNIT)),
                    "copy" => (F::Copy, vec![("from", s), ("to", s)], failing(self, Ty::UNIT)),
                    "remove" => (F::Remove, vec![("path", s), ("recursive", Ty::BOOL)], failing(self, Ty::UNIT)),
                    "open" => (F::Open, vec![("path", s), ("mode", fs.mode)], failing(self, fs.file)),
                    _ => {
                        let message = format!("module `fs` has no function `{member}`{}", std_alias("fs", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let (names, params): (Vec<&str>, Vec<Ty>) = params.into_iter().unzip();
                let mut signature = Signature::simple(params, result);
                signature.names = names.into_iter().map(String::from).collect();
                if function == F::Remove {
                    let recursive = Self::mk(hir::ExprKind::Bool(false), Ty::BOOL, span);
                    signature.defaults = vec![None, Some(recursive)];
                }
                let target = CallTarget::new(Callee::Std(StdFn::Fs(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Path))) => {
                let part = self.types.intern(TyKind::Option(Ty::STRING));
                use hir::PathFn as P;
                let (function, params, result) = match *member {
                    "join" => (P::Join, vec![Ty::STRING, Ty::STRING], Ty::STRING),
                    "parent" => (P::Parent, vec![Ty::STRING], part),
                    "fileName" => (P::FileName, vec![Ty::STRING], part),
                    "extension" => (P::Extension, vec![Ty::STRING], part),
                    "isAbsolute" => (P::IsAbsolute, vec![Ty::STRING], Ty::BOOL),
                    _ => {
                        let message = format!("module `path` has no function `{member}`{}", std_alias("path", member));
                        self.diags.error("E1001", segments[1].span, message);
                        return Self::error_expr(span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(StdFn::Path(function)), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([_, member], Some(ModuleName::Module(StdModule::Time))) => {
                let (duration, instant, datetime) =
                    (self.duration.unwrap(), self.instant.unwrap(), self.datetime.unwrap());
                let (function, params, result) = match *member {
                    "sleep" => (StdFn::TimeSleep, vec![duration], Ty::UNIT),
                    "now" => (StdFn::TimeNow, vec![], instant),
                    "unixTime" => (StdFn::TimeUnix, vec![], duration),
                    "utcNow" => (StdFn::TimeUtcNow, vec![], datetime),
                    "utc" => (StdFn::TimeUtc, vec![duration], datetime),
                    _ => {
                        let Some(factor) = Self::time_unit(member) else {
                            let message =
                                format!("module `time` has no function `{member}`{}", std_alias("time", member));
                            self.diags.error("E1001", segments[1].span, message);
                            return Self::error_expr(span);
                        };
                        // A count in nanoseconds; one that does not fit
                        // panics with `P0005` (14.3.1).
                        let [arg] = args else { return self.arity_error(span, 1, args.len()) };
                        let count = self.expr(cx, &arg.value, Some(Ty::INT));
                        let count = self.coerce(count, Ty::INT);
                        let factor = Self::mk(hir::ExprKind::Int(factor), Ty::INT, span);
                        let nanos = Self::mk(
                            hir::ExprKind::Binary(BinOp::Mul, Box::new(count), Box::new(factor)),
                            Ty::INT,
                            span,
                        );
                        return Self::mk(hir::ExprKind::Retype(Box::new(nanos)), duration, span);
                    }
                };
                let signature = Signature::simple(params, result);
                let target = CallTarget::new(Callee::Std(function), signature, expected);
                self.call_signature(cx, target, None, args, span)
            }
            ([name], None) if Self::numeric_type(name).is_some() => {
                let to = Self::numeric_type(name).unwrap();
                let [arg] = args else {
                    return self.arity_error(span, 1, args.len());
                };
                let value = self.expr(cx, &arg.value, None);
                if value.ty == to {
                    let message = format!("the value is already `{name}`: remove `{name}(...)` around it");
                    self.diags.warning("W1007", span, message);
                }
                match self.conversion_checked(value.ty, to) {
                    _ if value.ty == Ty::ERROR => Self::error_expr(span),
                    Some(false) => Self::mk(hir::ExprKind::Convert(Box::new(value)), to, span),
                    Some(true) => {
                        let ty = self.types.intern(TyKind::Result(to, self.conversion_error));
                        Self::mk(hir::ExprKind::Convert(Box::new(value)), ty, span)
                    }
                    None => {
                        let message = format!("there is no conversion from {} to `{name}`", self.display(value.ty));
                        self.diags.error("E1103", span, message);
                        Self::error_expr(span)
                    }
                }
            }
            (_, None) => {
                self.unknown_path(cx, segments, span);
                // The arguments' unknown names too, as other modules not
                // imported, and the bindings they use; not what follows from
                // a call of nothing.
                let before = self.diags.list.len();
                for arg in args.iter().filter(|a| !matches!(a.value.kind, ast::ExprKind::Closure(_))) {
                    self.expr(cx, &arg.value, None);
                }
                let mut index = 0;
                self.diags.list.retain(|d| {
                    index += 1;
                    index <= before || d.code == Some("E1001")
                });
                self.errors_counted.set((0, 0));
                Self::error_expr(span)
            }
            _ => self.unsupported_expr(callee.span, "calling this expression"),
        }
    }

    pub(super) fn arity_error(&mut self, span: Span, expected: usize, found: usize) -> hir::Expr {
        let noun = if expected == 1 { "argument" } else { "arguments" };
        let message = format!("expected {expected} {noun}, found {found}");
        self.diags.error("E1102", span, message);
        Self::error_expr(span)
    }

    /// A call of `callee` with `signature`. A dot-call passes its receiver
    /// already checked; otherwise every parameter comes from `args`, which
    /// may be named, variadic or spread (11.3). Arguments are evaluated in
    /// the order they are written.
    /// Whether `generic` is in `ty` as the error of a result, and whether it is
    /// anywhere else.
    pub(super) fn where_mentioned(&mut self, ty: Ty, generic: Ty) -> (bool, bool) {
        let both = |a: (bool, bool), b: (bool, bool)| (a.0 || b.0, a.1 || b.1);
        match self.types.kind(ty).clone() {
            TyKind::Result(ok, error) if error == generic => (true, self.where_mentioned(ok, generic).1),
            TyKind::Result(ok, error) => {
                let ok = self.where_mentioned(ok, generic);
                let error = self.where_mentioned(error, generic);
                both(ok, error)
            }
            TyKind::Fn(params, result, ..) => {
                let mut found = self.where_mentioned(result, generic);
                for param in params {
                    let inner = self.where_mentioned(param, generic);
                    found = both(found, inner);
                }
                found
            }
            TyKind::Option(inner) | TyKind::Array(inner) | TyKind::Deque(inner) | TyKind::Set(inner) => {
                self.where_mentioned(inner, generic)
            }
            _ if ty == generic => (false, true),
            _ => {
                let marked = self.types.subst(ty, &HashMap::from([(generic, Ty::UNIT)]));
                (false, marked != ty)
            }
        }
    }

    pub(super) fn call_signature(
        &mut self,
        cx: &mut FnCx,
        target: CallTarget,
        receiver: Option<hir::Expr>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if let Callee::Fn(id) = target.callee {
            match (self.measuring, self.calling) {
                (Some(test), _) => _ = self.test_calls.entry(test).or_default().insert(id),
                (None, Some(caller)) => _ = self.calls.entry(caller).or_default().insert(id),
                (None, None) => {}
            }
        }
        let CallTarget { callee, signature, explicit, prebound, expected } = target;
        if let Callee::Fn(id) = callee {
            self.repeatable(id, span);
            self.check_time_literals(id, &signature, receiver.is_some() as usize, args);
            self.check_template_literals(id, args);
            self.check_regex_literal(id, args);
            self.note_model_name(id, args);
            if self.is_std_fn(id, "cli", "cli.markup")
                && let Some(arg) = args.first()
                && let ast::ExprKind::Str(segments) = &arg.value.kind
                && let Some(problem) = markup_problem(segments)
            {
                self.diags.error("E1907", arg.value.span, problem);
            }
        }
        let signature = &signature;
        if signature.is_unsafe && !cx.unsafe_region {
            let message = if signature.foreign {
                format!(
                    "`{}` is a C function: call it inside `unsafe {{ ... }}`, once its preconditions hold",
                    signature.name
                )
            } else {
                format!(
                    "`{}` is `unsafe`: call it inside `unsafe {{ ... }}`, once its preconditions hold",
                    signature.name
                )
            };
            self.diags.error("E1701", span, message);
        }
        let first = receiver.is_some() as usize;
        let count = signature.params.len() - first;
        let fixed = count - signature.variadic as usize;
        // Which argument fills each parameter, then the variadic elements.
        let mut slots: Vec<Option<usize>> = vec![None; fixed];
        let (mut pack, mut spread) = (Vec::new(), None);
        let mut named = false;
        for (index, arg) in args.iter().enumerate() {
            let position = match &arg.name {
                None if named => {
                    self.diags.error("E1102", arg.span, "positional arguments must come before named ones");
                    return Self::error_expr(span);
                }
                None => index,
                Some(name) => {
                    named = true;
                    match signature.names[first..].iter().position(|n| *n == name.name) {
                        Some(position) => {
                            if let Callee::Fn(id) = callee {
                                self.index_refer(name, crate::index::Key::Param(id.0, name.name.clone()));
                            }
                            position
                        }
                        None if signature.names[first..].iter().all(String::is_empty) => {
                            let message = "a standard function takes positional arguments; remove the name";
                            self.diags.error("E1102", name.span, message);
                            return Self::error_expr(span);
                        }
                        None => {
                            let known: Vec<String> =
                                signature.names[first..].iter().map(|n| format!("`{n}`")).collect();
                            let message = format!(
                                "no parameter is named `{}`; the parameters are {}",
                                name.name,
                                known.join(", ")
                            );
                            self.diags.error("E1001", name.span, message);
                            return Self::error_expr(span);
                        }
                    }
                }
            };
            if position >= fixed && signature.variadic {
                // Single values may come first; one spread goes last (11.3).
                if spread.is_some() {
                    let message = "a spread `...values` is the last argument, and a call has one at most";
                    self.diags.error("E1102", arg.span, message);
                    return Self::error_expr(span);
                }
                if arg.name.is_some() && !arg.spread {
                    let name = &signature.names[first + fixed];
                    let message = format!("a named variadic argument is an array spread: `{name}: ...[a, b]`");
                    self.diags.error("E1102", arg.span, message);
                    return Self::error_expr(span);
                }
                if arg.spread {
                    spread = Some(index)
                } else {
                    pack.push(index)
                }
                continue;
            }
            if arg.spread {
                self.diags.error("E1102", arg.span, "`...` expands an array only into a variadic parameter");
                return Self::error_expr(span);
            }
            if position >= fixed {
                return self.arity_error(span, fixed, args.len());
            }
            if slots[position].replace(index).is_some() {
                let message = format!("parameter `{}` is given twice", signature.names[first + position]);
                self.diags.error("E1102", arg.span, message);
                return Self::error_expr(span);
            }
        }
        let omitted = |position: usize| slots[position].is_none() && !signature.has_default(first + position);
        if let Some(missing) = (0..fixed).find(|&position| omitted(position)) {
            let message = match signature.names[first + missing].as_str() {
                // A standard function's parameters have no names.
                "" => format!(
                    "missing argument {}, of type {}",
                    missing + 1,
                    self.display(signature.params[first + missing])
                ),
                name => {
                    format!("missing argument `{name}`, of type {}", self.display(signature.params[first + missing]))
                }
            };
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        // Type arguments come from explicit arguments, then from the values.
        let mut inference = Inference::new(signature.generics.clone());
        inference.map.extend(prebound);
        if !explicit.is_empty() {
            if explicit.len() != signature.generics.len() {
                let message =
                    format!("expected {} type argument(s), found {}", signature.generics.len(), explicit.len());
                self.diags.error("E1104", span, message);
                return Self::error_expr(span);
            }
            inference.map.extend(signature.generics.iter().copied().zip(explicit));
        }
        if let Some(receiver) = &receiver {
            self.unify(&mut inference, signature.params[0], receiver.ty);
        }
        let element = signature.variadic.then(|| match self.types.kind(signature.params[signature.params.len() - 1]) {
            TyKind::Array(element) => *element,
            _ => Ty::ERROR,
        });
        let parameter_of = |index: usize| match slots.iter().position(|&s| s == Some(index)) {
            Some(position) => first + position,
            None => signature.params.len() - 1,
        };
        let type_of = |index: usize| match (slots.contains(&Some(index)), spread == Some(index)) {
            (true, _) | (false, true) => signature.params[parameter_of(index)],
            (false, false) => element.unwrap_or(Ty::ERROR),
        };
        // Variadic elements are values; every other argument has its parameter's mode.
        let mode_of = |index: usize| {
            if slots.contains(&Some(index)) { signature.modes[parameter_of(index)] } else { ast::Mode::Value }
        };
        // Check the arguments in the order they are written; an unsuffixed
        // literal waits until the others have fixed its type.
        let mut values: Vec<Option<hir::Expr>> = (0..args.len()).map(|_| None).collect();
        for pass in 0..2 {
            // What the call is expected to give fixes what the arguments
            // that wait for context, such as closures, may not.
            if pass == 1
                && let Some(expected) = expected
                && values.iter().any(Option::is_none)
            {
                self.unify(&mut inference, signature.result, expected);
            }
            for (index, arg) in args.iter().enumerate() {
                if values[index].is_some() {
                    continue;
                }
                let ty = type_of(index);
                // Any value becomes a `reflect.Value` where one is a parameter (27.10).
                let dynamic = self.reflect_value_ty() == Some(ty);
                // A partly known `fn(A): B` waits, for an argument that needs
                // context, until the other arguments and the expected result
                // have fixed what they can.
                let waits = pass == 0 && self.needs_context(&arg.value);
                let expected = match dynamic {
                    true => None,
                    false => self
                        .inferred(&inference, ty)
                        .or_else(|| self.call_expectation(&inference, ty))
                        .or_else(|| {
                            let annotated = matches!(&arg.value.kind, ast::ExprKind::Closure(c) if c.params.iter().all(|p| p.ty.is_some()));
                            if waits { None } else { self.fn_expectation(&inference, ty, annotated) }
                        }),
                };
                if pass == 0 && expected.is_none() && self.needs_context(&arg.value) {
                    continue;
                }
                let mode = mode_of(index);
                let total = if arg.mode == ast::Mode::Value { self.total_as_fallible(cx, arg, ty) } else { None };
                let value = if let Some(value) = total {
                    value
                } else {
                    self.argument_value(
                        cx,
                        arg,
                        expected,
                        mode,
                        parameter_of(index) < first + usize::from(signature.receiver.is_some()) && first == 0,
                    )
                };
                let value = if dynamic && value.ty != ty && !arg.spread { self.reflect_value(value) } else { value };
                self.unify(&mut inference, ty, value.ty);
                // A closure for a `fn(A): B` parameter fixes what `B` mentions.
                if matches!(self.types.kind(ty), TyKind::Fn(..))
                    && let Some(Callable { params, result, variadic, modes, is_unsafe, .. }) = self.callable(value.ty)
                {
                    let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                    self.unify(&mut inference, ty, found);
                }
                // A callable bound to `F: Call[fn(A): B]` also fixes `A` and `B`.
                if let Some(&(_, bound)) = self.call_bounds.get(&ty)
                    && let Some(Callable { params, result, variadic, modes, is_unsafe, .. }) = self.callable(value.ty)
                {
                    let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                    self.unify(&mut inference, bound, found);
                }
                values[index] = Some(value);
            }
        }
        if let Some(expected) = expected {
            self.unify(&mut inference, signature.result, expected);
            // `call(...)?` where its success type is expected, which may be
            // an option when the call gives a result.
            match (self.types.kind(signature.result).clone(), self.types.kind(expected)) {
                (TyKind::Result(..), TyKind::Result(..))
                | (TyKind::Option(_), TyKind::Result(..) | TyKind::Option(_)) => {}
                (TyKind::Result(success, _) | TyKind::Option(success), _) => {
                    self.unify(&mut inference, success, expected)
                }
                _ => {}
            }
        }
        self.infer_from_bounds(&mut inference);
        // An error nothing fixes is that of what does not fail (12.1).
        for &generic in &signature.generics {
            if let std::collections::hash_map::Entry::Vacant(slot) = inference.map.entry(generic) {
                let types: Vec<Ty> = signature.params.iter().copied().chain([signature.result]).collect();
                let (error, elsewhere) = types.iter().fold((false, false), |(error, elsewhere), &ty| {
                    let (e, o) = self.where_mentioned(ty, generic);
                    (error || e, elsewhere || o)
                });
                if error && !elsewhere {
                    slot.insert(Ty::NEVER);
                }
            }
        }
        let unknown: Vec<String> = signature
            .generics
            .iter()
            .filter(|g| !inference.map.contains_key(g))
            .map(|&g| format!("`{}`", self.types.display(g)))
            .collect();
        if !unknown.is_empty() {
            let message = format!(
                "cannot infer {} from the arguments; write the type arguments: `name[...](...)`",
                unknown.join(", ")
            );
            self.diags.error("E1101", span, message);
            return Self::error_expr(span);
        }
        let c_arguments = std::mem::replace(&mut self.c_arguments, signature.foreign);
        for (index, value) in values.iter_mut().enumerate() {
            let ty = self.types.subst(type_of(index), &inference.map);
            let raw = value.take().unwrap();
            *value = Some(self.finish_argument(cx, raw, ty, mode_of(index)));
        }
        self.c_arguments = c_arguments;
        // Arguments with whether they are shared `ref` loans; a plain
        // `self` receiver is one.
        let lent = values.iter().enumerate().filter_map(|(i, v)| v.as_ref().map(|v| (v, mode_of(i) == ast::Mode::Ref)));
        let receiver_shared = signature.modes.first() == Some(&ast::Mode::Ref);
        self.check_loan_overlap(lent.chain(receiver.iter().map(|r| (r, receiver_shared))), span);
        let type_args: Vec<Ty> = signature.generics.iter().map(|g| inference.map[g]).collect();
        // The arguments must meet the bounds of the parameters they replace.
        for (&param, &actual) in signature.generics.iter().zip(&type_args) {
            if let Some(missing) = self.bound_holds(param, actual, &inference.map, 0) {
                // The traits the compiler implements by itself say why it did not.
                let automatic = matches!(
                    missing.as_str(),
                    "`reflect.Reflect`" | "`serialization.Encode`" | "`serialization.Decode`"
                );
                let lacking = automatic.then(|| self.lacking_field(actual, param, &inference.map)).flatten();
                // A type named like the prelude's has its error already.
                let misnamed = |ty: Ty| self.misnamed.contains(&self.types.display(ty));
                if misnamed(actual) || lacking.as_ref().is_some_and(|(_, ty)| misnamed(*ty)) {
                    return Self::error_expr(span);
                }
                let why = match lacking {
                    Some((field, ty)) => format!(
                        "; the compiler implements it for records whose fields all implement it, and its field `{field}` is {}, which does not",
                        self.display(ty)
                    ),
                    None if automatic => {
                        "; the compiler implements it for records, enums and newtypes whose parts all implement it"
                            .into()
                    }
                    None => String::new(),
                };
                let message = format!(
                    "{} does not implement {missing}, which `{}` requires of `{}`{why}",
                    self.display(actual),
                    signature.name,
                    self.types.display(param),
                );
                self.diags.error("E1105", span, message);
                return Self::error_expr(span);
            }
        }
        if let Callee::Trait(trait_id, _) = callee {
            let def = &self.traits[trait_id.0 as usize];
            let self_ty = inference.map[&def.self_param];
            let trait_args: Vec<Ty> = def.params.iter().map(|p| inference.map[p]).collect();
            if !self.satisfies(self_ty, trait_id, &trait_args) {
                let trait_name = self.traits[trait_id.0 as usize].name.clone();
                // Only the module of the trait or of the type may implement it.
                let own = self.owner(self_ty).is_some_and(|o| o.module == self.module)
                    || self.trait_modules.get(&trait_id) == Some(&self.module);
                let message = if own {
                    format!(
                        "{} does not implement `{trait_name}`: add `impl {trait_name} for {}`",
                        self.display(self_ty),
                        self.types.display(self_ty)
                    )
                } else {
                    format!(
                        "{} does not implement `{trait_name}`, and only its module or the trait's can implement it: wrap it in a newtype of this module, `type Name = {}`, and implement it for that",
                        self.display(self_ty),
                        self.types.display(self_ty)
                    )
                };
                self.diags.error("E1105", span, message);
                return Self::error_expr(span);
            }
        }
        // The values a literal markup interpolates are text, whatever
        // brackets they hold (standard library 14.32).
        if let Callee::Fn(id) = callee
            && self.is_std_fn(id, "cli", "cli.markup")
            && let Some(Some(text)) = values.first_mut()
        {
            self.markup_values(text);
        }
        if matches!(signature.name.as_str(), "reflect.get" | "reflect.set" | "reflect.call" | "reflect.callMut") {
            self.check_reflection(&signature.name, &type_args, args);
        }
        if signature.name == "reflect.annotationsOf" {
            let result = self.types.subst(signature.result, &inference.map);
            return self.annotations_of(type_args[0], args, result, span);
        }
        if signature.name == "protobuf.knownMessages" {
            let result = self.types.subst(signature.result, &inference.map);
            return self.known_messages(result, span);
        }
        if let Callee::Fn(id) = callee
            && signature.receiver.is_none()
            && let Some(kind) = signature.name.rsplit('.').next().and_then(crate::embed::Kind::of)
            && let Some(scope) = self.fn_scopes.get(id.0 as usize)
            && self.module_paths.get(scope.module as usize).is_some_and(|p| p == &["std", "embed"])
        {
            let result = self.types.subst(signature.result, &inference.map);
            return self.embedded(kind, args, result, span);
        }
        if let Callee::Fn(id) = callee
            && signature.receiver.is_none()
            && matches!(signature.name.as_str(), "template.html" | "template.text")
            && let Some(scope) = self.fn_scopes.get(id.0 as usize)
            && self.module_paths.get(scope.module as usize).is_some_and(|p| p == &["std", "template"])
        {
            let result = self.types.subst(signature.result, &inference.map);
            return self.compiled_template(cx, args, result, span);
        }
        // `format.apply` writes an integer or a float as one, as an
        // interpolation does (standard library 10.6).
        if let Callee::Fn(id) = callee
            && self.is_std_fn(id, "format", "format.apply")
            && let &[Some(value), Some(spec)] = slots.as_slice()
            && let Some(typed) = self.typed_apply(type_args[0])
        {
            let (value, spec) = (values[value].take().unwrap(), values[spec].take().unwrap());
            let result = self.types.subst(signature.result, &inference.map);
            return self.apply_typed(typed, value, spec, result, span);
        }
        let result = self.types.subst(signature.result, &inference.map);
        let mut ordered: Vec<(Option<usize>, hir::Expr)> = Vec::new();
        if let Some(receiver) = receiver {
            ordered.push((None, receiver));
        }
        for (position, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(index) => ordered.push((Some(index), values[index].take().unwrap())),
                None => {
                    let mut value =
                        signature.defaults[first + position].clone().expect("an omitted argument has a default");
                    self.instantiate(&mut value, &inference.map);
                    value.span = span;
                    ordered.push((None, value));
                }
            }
        }
        if let Some(array) = signature.variadic.then(|| signature.params[signature.params.len() - 1]) {
            let array = self.types.subst(array, &inference.map);
            let items: Vec<hir::Expr> = pack.iter().map(|&i| values[i].take().unwrap()).collect();
            let value = match spread {
                Some(index) if items.is_empty() => values[index].take().unwrap(),
                // The single values, then the spread array's.
                Some(index) => {
                    let rest = values[index].take().unwrap();
                    let element = match self.types.kind(array) {
                        TyKind::Array(element) => *element,
                        _ => Ty::ERROR,
                    };
                    if !self.types.is_copy(element) {
                        let message = format!(
                            "a spread of {} values, which are not `Copy`, is the only variadic argument: put the single values into its array first",
                            self.display(element)
                        );
                        self.diags.error("E1102", rest.span, message);
                        return Self::error_expr(span);
                    }
                    let front = Self::mk(hir::ExprKind::Array(items), array, span);
                    Self::mk(hir::ExprKind::ArrayOp(hir::ArrayOp::Concat, vec![front, rest]), array, span)
                }
                None => Self::mk(hir::ExprKind::Array(items), array, span),
            };
            ordered.push((pack.first().copied().or(spread), value));
        }
        // Named arguments out of declaration order are evaluated into
        // temporaries first, in the order they are written.
        let in_order = ordered.iter().filter_map(|(i, _)| *i).collect::<Vec<_>>().is_sorted();
        if in_order {
            let args = ordered.into_iter().map(|(_, value)| value).collect();
            return Self::mk(hir::ExprKind::Call(callee, type_args, args), result, span);
        }
        let mut stmts = Vec::new();
        let mut args = Vec::new();
        for (index, value) in &mut ordered {
            let mut value = std::mem::replace(value, Self::error_expr(span));
            match (index, &mut value.kind) {
                (Some(index), hir::ExprKind::Region(_, bounds)) => {
                    // A region is lent where the call is; its bounds are
                    // evaluated where they are written.
                    for bound in bounds.iter_mut().flatten() {
                        let local = self.hidden_local(cx, bound.ty);
                        let read = Self::mk(hir::ExprKind::Local(local), bound.ty, bound.span);
                        stmts.push((*index, hir::Stmt::Let(local, std::mem::replace(&mut **bound, read))));
                    }
                    args.push(value);
                }
                (Some(index), kind) if !matches!(kind, hir::ExprKind::Loan(_)) => {
                    let local = self.hidden_local(cx, value.ty);
                    args.push(Self::mk(hir::ExprKind::Local(local), value.ty, value.span));
                    stmts.push((*index, hir::Stmt::Let(local, value)));
                }
                _ => args.push(value),
            }
        }
        stmts.sort_by_key(|(index, _)| *index);
        let call = Self::mk(hir::ExprKind::Call(callee, type_args, args), result, span);
        let block =
            hir::Block { stmts: stmts.into_iter().map(|(_, s)| s).collect(), tail: Some(Box::new(call)), ty: result };
        Self::mk(hir::ExprKind::Block(block), result, span)
    }

    /// One argument as written, before its parameter type is final; a
    /// `mut self` or `var self` receiver in a qualified call is a loan.
    /// One argument as written for a parameter passed as `mode`. A plain
    /// `self` receiver written first in a qualified call takes no keyword.
    /// A function that does not fail, named where one that may is expected:
    /// a closure that calls it, whose value is `ok` (12.1).
    pub(super) fn total_as_fallible(&mut self, cx: &mut FnCx, arg: &ast::Arg, param: Ty) -> Option<hir::Expr> {
        if let TyKind::Fn(params, result, false, ..) = self.types.kind(param).clone()
            && matches!(self.types.kind(result), TyKind::Result(..))
            && let ast::ExprKind::Path(segments) = &arg.value.kind
            && let [name] = segments.as_slice()
            && self.lookup(cx, &name.name).is_none()
            && let Some(ModuleName::Fn(id)) = self.names.get(&name.name).copied()
            && let Some(signature) = self.signatures.get(id.0 as usize)
            && signature.params.len() == params.len()
            && signature.receiver.is_none()
            && signature.generics.is_empty()
            && !matches!(self.types.kind(signature.result), TyKind::Result(..))
        {
            // What the function takes is known, though the call may not know
            // it yet.
            // Its error, if the call gives one; one it leaves open is `Never`.
            let TyKind::Result(_, error) = self.types.kind(result).clone() else { unreachable!("matched") };
            let error = if matches!(self.types.kind(error), TyKind::Param(..)) { Ty::NEVER } else { error };
            let gives = self.types.intern(TyKind::Result(signature.result, error));
            let modes = signature.modes.clone();
            let expected = self.types.intern(TyKind::Fn(signature.params.clone(), gives, false, modes, false));
            let span = arg.value.span;
            let names: Vec<ast::Ident> =
                (0..params.len()).map(|at| ast::Ident { name: format!("#{at}"), span }).collect();
            let call = ast::Expr {
                kind: ast::ExprKind::Call {
                    callee: Box::new(arg.value.clone()),
                    args: names
                        .iter()
                        .map(|name| ast::Arg {
                            name: None,
                            value: ast::Expr { kind: ast::ExprKind::Path(vec![name.clone()]), span },
                            mode: ast::Mode::Value,
                            spread: false,
                            span,
                        })
                        .collect(),
                },
                span,
            };
            let params = names
                .into_iter()
                .map(|name| ast::ClosureParam { name, ty: None, mode: ast::Mode::Value, variadic: false })
                .collect();
            let body = ast::Block { stmts: Vec::new(), tail: Some(Box::new(call)), span };
            let closure = ast::Closure { is_unsafe: false, params, result: None, body };
            let wrapped = ast::Expr { kind: ast::ExprKind::Closure(Box::new(closure)), span };
            return Some(self.expr(cx, &wrapped, Some(expected)));
        }
        None
    }

    pub(super) fn argument_value(
        &mut self,
        cx: &mut FnCx,
        arg: &ast::Arg,
        expected: Option<Ty>,
        mode: ast::Mode,
        receiver: bool,
    ) -> hir::Expr {
        let written = if receiver && mode == ast::Mode::Ref { ast::Mode::Value } else { mode };
        if arg.mode != written {
            let message = match (written, receiver) {
                (ast::Mode::Value, true) => {
                    format!("the receiver `self` is already a shared loan: remove `{}`", arg.mode.keyword())
                }
                (ast::Mode::Value, false) => format!("this parameter takes a value: remove `{}`", arg.mode.keyword()),
                (mode, _) => {
                    let text = self.sources.file_of(arg.value.span.start).slice(arg.value.span);
                    format!("this parameter is a `{0}` loan: write `{0} {text}`", mode.keyword())
                }
            };
            let code = if receiver { "E1407" } else { "E1102" };
            self.diags.error(code, arg.span, message);
        }
        self.expr(cx, &arg.value, expected)
    }

    /// An argument coerced to its final parameter type, or the loan of its
    /// place for a `mut self` or `var self` receiver.
    pub(super) fn finish_argument(&mut self, cx: &mut FnCx, value: hir::Expr, ty: Ty, mode: ast::Mode) -> hir::Expr {
        let value = self.coerce(value, ty);
        if !matches!(mode, ast::Mode::Mut | ast::Mode::Var) || matches!(value.kind, hir::ExprKind::Loan(_)) {
            return value;
        }
        if let hir::ExprKind::Slice(..) = value.kind {
            return self.region(cx, value, mode);
        }
        let span = value.span;
        let operation = format!("passing it as `{}`", mode.keyword());
        match self.loan_place(cx, value, &operation, mode == ast::Mode::Var) {
            Some(place) => Self::mk(hir::ExprKind::Loan(place), ty, span),
            None => Self::error_expr(span),
        }
    }

    /// `mut values[start:end:step]`: a region of an array lent to a `mut`
    /// parameter, which changes its elements but not its length (16.4).
    pub(super) fn region(&mut self, cx: &mut FnCx, value: hir::Expr, mode: ast::Mode) -> hir::Expr {
        let (span, ty) = (value.span, value.ty);
        let hir::ExprKind::Slice(base, bounds) = value.kind else { unreachable!("called on a slice") };
        if !matches!(self.types.kind(base.ty), TyKind::Array(_)) {
            let message = format!("only a region of an array can be lent, not of {}", self.display(base.ty));
            self.diags.error("E1411", span, message);
            return Self::error_expr(span);
        }
        if mode == ast::Mode::Var {
            let message = format!(
                "a `var` loan takes a whole place, and a region keeps its length: take the parameter as `mut {}`",
                self.types.display(ty)
            );
            self.diags.error("E1411", span, message);
            return Self::error_expr(span);
        }
        match self.loan_place(cx, *base, "lending a region of it as `mut`", false) {
            Some(place) => Self::mk(hir::ExprKind::Region(place, bounds), ty, span),
            None => Self::error_expr(span),
        }
    }

    /// The `var` place a value expression denotes, for an operation that
    /// changes it (11.6); reports values that are not such a place.
    /// `structural` operations may change the extent of the root (`var`);
    /// the others keep it (`mut`, 16.3).
    pub(super) fn loan_place(
        &mut self,
        cx: &mut FnCx,
        value: hir::Expr,
        operation: &str,
        structural: bool,
    ) -> Option<hir::Place> {
        let span = value.span;
        let mut steps = Vec::new();
        let mut current = value;
        let local = loop {
            match current.kind {
                hir::ExprKind::Local(local) => break local,
                hir::ExprKind::Field(base, index) => {
                    steps.push(hir::PlaceStep::Field(index));
                    current = *base;
                }
                hir::ExprKind::Retype(base) => current = *base,
                hir::ExprKind::Index(base, index) => {
                    steps.push(hir::PlaceStep::Index(*index));
                    current = *base;
                }
                hir::ExprKind::Error => return None,
                hir::ExprKind::RefValue(_) => {
                    self.diags.error("E1411", span, format!("{operation} would change it, but {REF_UNCHANGED}"));
                    return None;
                }
                _ => {
                    let message =
                        format!("{operation} changes its receiver, which must be a `var` binding or part of one");
                    self.diags.error("E1411", span, message);
                    return None;
                }
            }
        };
        steps.reverse();
        let place = hir::Place { local, path: steps };
        if let Some((lent, false)) = lent_conflict(cx, &place) {
            let lent = self.place_name(cx, lent);
            self.lent_error(&lent, false, span);
        }
        let hir::Place { local, path: steps } = place;
        if Some(local) == cx.env
            && let Some(hir::PlaceStep::Field(index)) = steps.first()
        {
            // Changing a capture changes the closure's own copy (11.8).
            let capture = &cx.captures[*index as usize];
            if !capture.mutable {
                let message = format!(
                    "{operation} changes `{0}`, which is not declared with `var`: declare it as `var {0}`",
                    capture.name
                );
                self.diags.error("E1411", span, message);
            }
            cx.mutates = true;
            return Some(hir::Place { local, path: steps });
        }
        let name = cx.locals[local.0 as usize].name.clone();
        let ty = self.display(cx.locals[local.0 as usize].ty);
        let binding = cx.loan_bindings.contains(&local);
        match cx.params[local.0 as usize] {
            Some(ast::Mode::Mut) if structural && steps.is_empty() && binding => {
                let message = format!(
                    "{operation} can change the length of `{name}`, a `mut` binding that keeps its extent; bind it as `var {name}`"
                );
                self.diags.error("E1411", span, message);
            }
            Some(ast::Mode::Mut) if structural && steps.is_empty() => {
                let message = format!(
                    "{operation} can change the length of `{name}`, a `mut` loan that keeps its extent; take it as `{name}: var {}`",
                    ty.trim_matches('`')
                );
                self.diags.error("E1411", span, message);
            }
            Some(ast::Mode::Mut | ast::Mode::Var) => {}
            Some(_) if binding => {
                let message = format!(
                    "{operation} changes `{name}`, a `ref` binding that only observes its part; borrow it with `mut {name}` in a `for` header"
                );
                self.diags.error("E1411", span, message);
            }
            Some(mode) if !cx.mutable[local.0 as usize] => {
                let what = if mode == ast::Mode::Ref { "a `ref` loan" } else { "a parameter" };
                let message = format!(
                    "{operation} changes `{name}`, which is {what}; take it as `{name}: {} {}` to change the caller's value",
                    if structural { "var" } else { "mut" },
                    ty.trim_matches('`')
                );
                self.diags.error("E1411", span, message);
            }
            _ if !cx.mutable[local.0 as usize] => {
                let message = format!(
                    "{operation} changes `{name}`, which is not declared with `var`: declare it as `var {name}`"
                );
                self.diags.error("E1411", span, message);
            }
            _ => {}
        }
        Some(hir::Place { local, path: steps })
    }
}
