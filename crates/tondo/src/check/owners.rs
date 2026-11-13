//! Receivers, the owners of methods, and resolving types.

use super::*;

impl<'a> Checker<'a> {
    /// The type of `self` in a method of the owner keyed `key`: a generic
    /// owner applied to the method's binders (11.5).
    pub(super) fn receiver_type(&mut self, key: Ty, binders: &[Ty], f: &ast::FnDecl) -> Ty {
        if key == Ty::ERROR {
            return key;
        }
        let owner = f.owner.as_ref().unwrap();
        // An intrinsic collection of the prelude, over the method's binders.
        let rebuilt = match (self.types.kind(key).clone(), binders) {
            (TyKind::Array(_), [t]) => Some(TyKind::Array(*t)),
            (TyKind::Deque(_), [t]) => Some(TyKind::Deque(*t)),
            (TyKind::Set(_), [t]) => Some(TyKind::Set(*t)),
            (TyKind::Map(..), [k, v]) => Some(TyKind::Map(*k, *v)),
            (TyKind::Option(_), [t]) => Some(TyKind::Option(*t)),
            (TyKind::Result(..), [t, e]) => Some(TyKind::Result(*t, *e)),
            (TyKind::Array(_) | TyKind::Deque(_) | TyKind::Set(_) | TyKind::Option(_), _) => {
                let message = format!(
                    "an operation of `{0}` declares its type parameter: `fn {0}[T].{1}(...)`",
                    owner.name, f.name.name
                );
                self.diags.error("E1104", owner.span, message);
                return Ty::ERROR;
            }
            (TyKind::Map(..) | TyKind::Result(..), _) => {
                let message = format!("an operation of `{0}` declares its two type parameters", owner.name);
                self.diags.error("E1104", owner.span, message);
                return Ty::ERROR;
            }
            _ => None,
        };
        if let Some(kind) = rebuilt {
            return self.types.intern(kind);
        }
        match self.types.instance_of(key) {
            Some((template, _)) => {
                let params: Vec<String> =
                    self.types.template(template).params.iter().map(|&p| self.types.display(p)).collect();
                if binders.len() != params.len() {
                    let message = format!(
                        "an operation of `{0}` declares its type parameters: `fn {0}[{1}].{2}(...)`",
                        owner.name,
                        params.join(", "),
                        f.name.name
                    );
                    self.diags.error("E1104", owner.span, message);
                    return Ty::ERROR;
                }
                self.types.instantiate(template, binders.to_vec())
            }
            None if !binders.is_empty() => {
                let message = format!("`{}` is not generic", owner.name);
                self.diags.error("E1104", owner.span, message);
                Ty::ERROR
            }
            None => key,
        }
    }

    /// The key under which the prelude declares the methods of an intrinsic
    /// type: a collection, option or result over parameters of its own, or
    /// the type itself.
    pub(super) fn intrinsic_owner(&mut self, name: &str) -> Option<Ty> {
        if let Some(&ty) = self.intrinsic_owners.get(name) {
            return Some(ty);
        }
        let param = |this: &mut Self, name: &str, key: bool| {
            let param = this.types.declare_param(name.into());
            this.types.param_mut(param).key = key;
            param
        };
        let ty = match name {
            "Array" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Array(t))
            }
            "Deque" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Deque(t))
            }
            "Set" => {
                let t = param(self, "T", true);
                self.types.intern(TyKind::Set(t))
            }
            "Map" => {
                let (k, v) = (param(self, "K", true), param(self, "V", false));
                self.types.intern(TyKind::Map(k, v))
            }
            "Option" => {
                let t = param(self, "T", false);
                self.types.intern(TyKind::Option(t))
            }
            "Result" => {
                let (t, e) = (param(self, "T", false), param(self, "E", false));
                self.types.intern(TyKind::Result(t, e))
            }
            "String" => Ty::STRING,
            "Char" => Ty::CHAR,
            "Bool" => Ty::BOOL,
            "Byte" => Ty::BYTE,
            name if IntTy::from_name(name).is_some() => Ty::int(IntTy::from_name(name).unwrap()),
            "Float" | "Float64" => self.types.intern(TyKind::Float(crate::types::FloatTy::F64)),
            "Float32" => self.types.intern(TyKind::Float(crate::types::FloatTy::F32)),
            _ => return None,
        };
        self.intrinsic_owners.insert(name.to_string(), ty);
        Some(ty)
    }

    /// The key of the operations of `ty`: a generic type's instance with its
    /// own parameters stands for all of its instances.
    pub(super) fn owner_key(&mut self, ty: Ty) -> Ty {
        let intrinsic = match self.types.kind(ty) {
            TyKind::Array(_) => Some("Array"),
            TyKind::Deque(_) => Some("Deque"),
            TyKind::Set(_) => Some("Set"),
            TyKind::Map(..) => Some("Map"),
            TyKind::Option(_) => Some("Option"),
            TyKind::Result(..) => Some("Result"),
            _ => None,
        };
        if let Some(name) = intrinsic {
            return self.intrinsic_owner(name).expect("an intrinsic owner");
        }
        match self.types.instance_of(ty) {
            Some((template, _)) => {
                let params = self.types.template(template).params.clone();
                self.types.instantiate(template, params)
            }
            None => ty,
        }
    }

    /// The type an inherent method is declared on: a nominal type of this
    /// module (11.5).
    pub(super) fn method_owner(&mut self, owner: &ast::Ident, module: u32) -> Ty {
        if self.type_names.contains_key(&owner.name) {
            self.index_refer(owner, crate::index::Key::Type(owner.name.clone()));
        }
        // The prelude declares the methods of the intrinsic types, and so
        // may a fixture of documentation (Appendix C).
        if (Some(module) == self.prelude || self.fixture(owner.span))
            && let Some(key) = self.intrinsic_owner(&owner.name)
        {
            return key;
        }
        let nominal = match self.type_names.get(&owner.name) {
            Some(TypeName::Nominal(ty)) => Some(Some(*ty)),
            Some(TypeName::Template(template)) => {
                let template = *template;
                let params = self.types.template(template).params.clone();
                Some(Some(self.types.instantiate(template, params)))
            }
            Some(TypeName::Alias(_)) => Some(None),
            None => None,
        };
        match nominal {
            Some(Some(ty)) if self.owner(ty).is_some_and(|o| o.module != module) => {
                let message = format!(
                    "`{0}` is declared in another module, which alone declares its methods: move this method there, or write a function that takes a `{0}`",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            Some(Some(ty)) if !PRELUDE_TYPES.contains(&owner.name.as_str()) => ty,
            // An operation the toolchain has natively.
            _ if self.fixture(owner.span) => Ty::ERROR,
            Some(_) => {
                let message = format!(
                    "methods can only be declared on records, enums and newtypes of this module, not `{}`",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            None if PRELUDE_TYPES.contains(&owner.name.as_str()) || self.type_named(&owner.name).is_some() => {
                let message = format!(
                    "methods cannot be added to the intrinsic type `{}`: write a function that takes one",
                    owner.name
                );
                self.diags.error("E1504", owner.span, message);
                Ty::ERROR
            }
            None => {
                self.diags.error("E1001", owner.span, unknown_type(&owner.name));
                Ty::ERROR
            }
        }
    }

    pub(super) fn declare_method(&mut self, f: &ast::FnDecl, owner: Ty, has_receiver: bool, id: FnId) {
        if owner == Ty::ERROR {
            return;
        }
        let name = &f.name.name;
        let type_name = self.types.display(owner);
        let clash = if self.methods.contains_key(&(owner, name.clone())) {
            Some(format!("`{type_name}` already has an operation named `{name}`"))
        } else if self.types.variants(owner).is_some()
            && let TyKind::Adt(adt) = self.types.kind(owner)
            && let AdtKind::Enum(variants) = &self.types.adt(*adt).kind
            && variants.iter().any(|v| &v.name == name)
        {
            Some(format!("`{type_name}.{name}` is a variant; an operation of `{type_name}` needs another name"))
        } else if has_receiver && self.types.components(owner).is_some_and(|_| self.record_has_field(owner, name)) {
            Some(format!("`{name}` is a field of `{type_name}`; a method with a receiver needs another name"))
        } else {
            None
        };
        // What a fixture of documentation declares that the toolchain has
        // is the toolchain's.
        let native = || f.owner.as_ref().is_some_and(|o| crate::api::builtin_methods(&o.name).contains(&name.as_str()));
        match clash {
            _ if self.fixture(f.span) && (clash.is_some() || native()) => {}
            Some(message) => self.diags.error("E1505", f.name.span, message),
            None => {
                self.methods.insert((owner, name.clone()), id);
                self.method_visibility.insert(id, (f.module, f.public));
            }
        }
    }

    /// Whether `span` is in a fixture of documentation (Appendix C).
    pub(super) fn fixture(&self, span: Span) -> bool {
        self.sources.file_of(span.start).logical.starts_with("<fixture>/")
    }

    pub(super) fn record_has_field(&self, ty: Ty, name: &str) -> bool {
        matches!(self.types.kind(ty), TyKind::Adt(id)
            if matches!(&self.types.adt(*id).kind, AdtKind::Record(fields) if fields.iter().any(|f| f.name == name)))
    }

    pub(super) fn validate_main(&mut self, f: &ast::FnDecl, id: FnId) {
        let signature = &self.signatures[id.0 as usize];
        let problem = if f.public {
            Some("`main` cannot be `pub`")
        } else if f.is_unsafe {
            Some("`main` cannot be `unsafe`")
        } else if !f.params.is_empty() {
            Some("`main` takes no parameters")
        } else if signature.result != Ty::UNIT
            && !matches!(self.types.kind(signature.result), TyKind::Result(ok, _) if *ok == Ty::UNIT)
        {
            Some("`main` returns `Unit` or `!E`")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.diags.error("E1803", f.name.span, problem);
        }
    }

    pub(super) fn resolve_type(&mut self, t: &ast::TypeExpr) -> Ty {
        if let ast::TypeKind::Path { segments, .. } = &t.kind
            && let [name] = segments.as_slice()
            && self.index.is_some()
            && self.type_names.contains_key(&name.name)
            && !self.type_params.iter().any(|(n, _)| *n == name.name)
        {
            self.index_refer(name, crate::index::Key::Type(name.name.clone()));
        }
        match &t.kind {
            ast::TypeKind::Path { segments, args } => {
                if !args.is_empty() {
                    let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
                    if let [module, name] = segments.as_slice()
                        && let Some(ModuleName::Module(StdModule::Sync)) = self.names.get(&module.name)
                    {
                        return self.sync_type(name, &args, t.span);
                    }
                    if let [module, name] = segments.as_slice()
                        && let Some(ModuleName::Module(StdModule::Simd)) = self.names.get(&module.name)
                    {
                        return self.simd_type(name, &args, t.span);
                    }
                    if let [name] = segments.as_slice()
                        && let Some(&TypeName::Template(template)) = self.type_names.get(&name.name)
                    {
                        let expected = self.types.template(template).params.len();
                        if args.len() != expected {
                            let message =
                                format!("`{}` takes {expected} type argument(s), found {}", name.name, args.len());
                            self.diags.error("E1104", t.span, message);
                            return Ty::ERROR;
                        }
                        return self.types.instantiate(template, args);
                    }
                    return match (segments[0].name.as_str(), args.as_slice()) {
                        ("Option", [inner]) if segments.len() == 1 => self.types.intern(TyKind::Option(*inner)),
                        ("Result", [ok, err]) if segments.len() == 1 => self.types.intern(TyKind::Result(*ok, *err)),
                        ("Array", [element]) if segments.len() == 1 => self.types.intern(TyKind::Array(*element)),
                        ("Deque", [element]) if segments.len() == 1 => self.types.intern(TyKind::Deque(*element)),
                        ("Join", [result]) if segments.len() == 1 => self.types.handle(Handle::Join, *result),
                        ("Group", [result]) if segments.len() == 1 => self.types.handle(Handle::Group, *result),
                        ("Sender", [value]) if segments.len() == 1 => self.types.handle(Handle::Sender, *value),
                        ("Receiver", [value]) if segments.len() == 1 => self.types.handle(Handle::Receiver, *value),
                        ("Ref", [value]) if segments.len() == 1 => self.types.handle(Handle::Ref, *value),
                        ("WeakRef", [value]) if segments.len() == 1 => self.types.handle(Handle::Weak, *value),
                        ("Pointer", [target]) if segments.len() == 1 => self.types.intern(TyKind::Pointer(*target)),
                        ("Range", [element]) if segments.len() == 1 => self.range_type(*element, t.span),
                        ("Map", [key, value]) if segments.len() == 1 => {
                            self.require_key(*key, t.span);
                            self.map_type(*key, *value)
                        }
                        ("Set", [element]) if segments.len() == 1 => {
                            self.require_key(*element, t.span);
                            self.types.intern(TyKind::Set(*element))
                        }
                        (
                            "Option" | "Result" | "Array" | "Deque" | "Map" | "Set" | "Join" | "Group" | "Sender"
                            | "Receiver" | "Ref" | "WeakRef" | "Pointer" | "Range",
                            _,
                        ) if segments.len() == 1 => {
                            self.diags.error("E1104", t.span, "wrong number of type arguments");
                            Ty::ERROR
                        }
                        _ => self.unsupported_type(t.span, "generic types"),
                    };
                }
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                if let [name] = names.as_slice()
                    && let Some(&(_, param)) = self.type_params.iter().rev().find(|(n, _)| n == name)
                {
                    return param;
                }
                if let ["Self"] = names.as_slice() {
                    return self.self_type.unwrap_or_else(|| {
                        self.diags.error(
                            "E1001",
                            t.span,
                            "`Self` only exists in a trait, an implementation or a method",
                        );
                        Ty::ERROR
                    });
                }
                // A type of the module goes before a trait of the prelude, as
                // a C union named `Number`.
                if let [name] = names.as_slice()
                    && self.trait_names.contains_key(*name)
                    && !self.type_names.contains_key(*name)
                {
                    let message = format!(
                        "a trait is not a type; take a type parameter bound by it: `fn name[T: {name}](value: T)`"
                    );
                    self.diags.error("E1110", t.span, message);
                    return Ty::ERROR;
                }
                match names.as_slice() {
                    [name] if IntTy::from_name(name).is_some() => Ty::int(IntTy::from_name(name).unwrap()),
                    ["Bool"] => Ty::BOOL,
                    ["String"] => Ty::STRING,
                    ["Unit"] => Ty::UNIT,
                    ["Command"] => self.types.handle(Handle::Command, Ty::UNIT),
                    ["Pipeline"] => self.types.handle(Handle::Pipeline, Ty::UNIT),
                    ["Never"] => Ty::NEVER,
                    [module, name] if self.module_type(module, name).is_some() => {
                        self.module_type(module, name).unwrap()
                    }
                    ["Float" | "Float64"] => Ty::F64,
                    ["Float32"] => Ty::F32,
                    ["Char"] => Ty::CHAR,
                    ["Byte"] => Ty::BYTE,
                    [single] if self.type_names.contains_key(*single) => match self.type_names[*single] {
                        TypeName::Nominal(ty) => ty,
                        TypeName::Alias(index) => self.resolve_alias(index),
                        TypeName::Template(template) => {
                            let params: Vec<String> =
                                self.types.template(template).params.iter().map(|&p| self.types.display(p)).collect();
                            let message = format!("`{single}` takes type arguments: `{single}[{}]`", params.join(", "));
                            self.diags.error("E1104", t.span, message);
                            Ty::ERROR
                        }
                    },
                    _ => {
                        self.diags.error("E1001", t.span, unknown_type(&names.join(".")));
                        Ty::ERROR
                    }
                }
            }
            ast::TypeKind::Result { ok, err } => {
                let ok = ok.as_ref().map_or(Ty::UNIT, |ok| self.resolve_type(ok));
                let err = self.resolve_type(err);
                self.types.intern(TyKind::Result(ok, err))
            }
            ast::TypeKind::Optional(inner) => {
                let inner = self.resolve_type(inner);
                self.types.intern(TyKind::Option(inner))
            }
            ast::TypeKind::Union(members) => {
                let mut types = Vec::new();
                for member in members {
                    let ty = self.resolve_type(member);
                    self.check_union_member(ty, member.span);
                    types.push(ty);
                }
                self.types.union(&types)
            }
            ast::TypeKind::Tuple(items) => {
                let items = items.iter().map(|item| self.resolve_type(item)).collect();
                self.types.intern(TyKind::Tuple(items))
            }
            ast::TypeKind::Opaque(_) => {
                self.diags.error("E0004", t.span, "`impl Bound` only describes the result of a function");
                Ty::ERROR
            }
            ast::TypeKind::NotCopy => {
                self.diags.error("E0004", t.span, "`~Copy` only appears in a list of bounds");
                Ty::ERROR
            }
            ast::TypeKind::Fn { params, modes, variadic, result, is_unsafe: _, is_extern: true } => {
                // The address of a C function: what crosses to C, by value.
                let mut types = Vec::new();
                for param in params {
                    let ty = self.resolve_type(param);
                    self.require_c_type(ty, param.span);
                    types.push(ty);
                }
                if let Some(mode) = modes.iter().find(|&&m| m != ast::Mode::Value) {
                    let message = format!("C takes its arguments by value: remove `{}`", mode.keyword());
                    self.diags.error("E1704", t.span, message);
                }
                // C's `...` has a mode and no type.
                if *variadic && modes.len() == params.len() {
                    let message = "C's variable arguments are a last `...`, without a type";
                    self.diags.error("E1704", t.span, message);
                }
                let result = match result {
                    Some(written) => {
                        let ty = self.resolve_type(written);
                        if let TyKind::Result(value, error) = *self.types.kind(ty) {
                            self.c_failure(value, error, false, written.span);
                        } else if ty != Ty::UNIT && ty != Ty::NEVER {
                            self.require_c_type(ty, written.span);
                        }
                        ty
                    }
                    None => Ty::UNIT,
                };
                self.types.intern(TyKind::CFn(types, result, *variadic))
            }
            ast::TypeKind::Fn { params, modes, variadic, result, is_unsafe, is_extern: false } => {
                if modes.len() > params.len() {
                    let message = "C's `...` ends only an `extern fn(...)`; a Tondo variadic parameter is `...T`";
                    self.diags.error("E1102", t.span, message);
                    return Ty::ERROR;
                }
                let mut types: Vec<Ty> = params.iter().map(|p| self.resolve_type(p)).collect();
                if *variadic && let Some(last) = types.last_mut() {
                    *last = self.types.intern(TyKind::Array(*last));
                }
                let result = result.as_ref().map_or(Ty::UNIT, |r| self.resolve_type(r));
                self.types.intern(TyKind::Fn(types, result, *variadic, modes.clone(), *is_unsafe))
            }
        }
    }

    /// Union members are nominal or intrinsic types written as a path (9.5);
    /// anonymous structural types first get a name with a newtype.
    pub(super) fn check_union_member(&mut self, ty: Ty, span: Span) {
        let anonymous = match self.types.kind(ty) {
            TyKind::Tuple(_) => Some("a tuple"),
            TyKind::Unit => Some("`Unit`"),
            _ => None,
        };
        if let Some(what) = anonymous {
            let message =
                format!("{what} cannot be a union member: wrap it in a newtype, such as `type Pair = (Int, Int)`");
            self.diags.error("E1102", span, message);
        }
    }

    pub(super) fn unsupported_type(&mut self, span: Span, what: &str) -> Ty {
        self.diags.unsupported(span, what);
        Ty::ERROR
    }

    pub(super) fn display(&self, ty: Ty) -> String {
        format!("`{}`", self.types.display(ty))
    }

    // ---- functions -------------------------------------------------------

    /// A context for a body whose declared result is `result`. A `T ! E`
    /// result makes the body fallible: it is written in terms of `T` (9.8).
    pub(super) fn error_count(&self) -> usize {
        let list = &self.diags.list;
        let (seen, errors) = match self.errors_counted.get() {
            (seen, errors) if seen <= list.len() => (seen, errors),
            _ => (0, 0),
        };
        let errors = errors + list[seen..].iter().filter(|d| d.severity == crate::diag::Severity::Error).count();
        self.errors_counted.set((list.len(), errors));
        errors
    }
}
