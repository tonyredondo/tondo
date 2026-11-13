//! Signatures, traits, implementations, bounds and inference from bounds.

use super::*;

impl<'a> Checker<'a> {
    /// A default value for a call whose type arguments are `map`.
    pub(super) fn instantiate(&mut self, value: &mut hir::Expr, map: &HashMap<Ty, Ty>) {
        hir::instantiate_constant(&mut self.types, value, map);
    }

    pub(super) fn signature(&mut self, f: &ast::FnDecl, owner: Option<Ty>, outer: FnScope) -> Signature {
        let outer_count = outer.types.len();
        self.type_params.extend(outer.types.iter().cloned());
        let saved_self = std::mem::replace(&mut self.self_type, outer.self_type);
        let binders = self.declare_generics(&f.owner_generics, true, f.module);
        self.enter_generics(&f.owner_generics, &binders);
        let owner = match owner {
            Some(key) => Some(self.receiver_type(key, &binders, f)),
            None => self.self_type,
        };
        if f.owner.is_some() {
            // `Self` names the owner in an inherent method (12.3).
            self.self_type = owner;
        }
        let own = self.declare_generics(&f.generics, true, f.module);
        self.enter_generics(&f.generics, &own);
        let (mut params, mut names, mut receiver, mut variadic) = (Vec::new(), Vec::new(), None, false);
        for (index, param) in f.params.iter().enumerate() {
            let ty = match &param.ty {
                None => {
                    receiver = Some(param.mode);
                    owner.unwrap_or_else(|| {
                        let message = format!(
                            "only a method has a receiver: declare it on its type, as in `fn Type.{}(self, ...)`",
                            f.name.name
                        );
                        self.diags.error("E1002", param.span, message);
                        Ty::ERROR
                    })
                }
                Some(ty) => {
                    let ty = self.resolve_type(ty);
                    if param.variadic {
                        if index + 1 != f.params.len() {
                            self.diags.error("E1102", param.span, "only the last parameter can be variadic");
                        }
                        variadic = true;
                        self.types.intern(TyKind::Array(ty))
                    } else {
                        ty
                    }
                }
            };
            params.push(ty);
            names.push(param.name.name.clone());
        }
        let all_generics: Vec<Ty> = self.type_params.iter().map(|&(_, t)| t).collect();
        let result = match &f.result {
            Some(result) => self.result_type(result, f, all_generics),
            None => Ty::UNIT,
        };
        if f.public {
            self.check_public_api(&params, result, f);
        }
        let scope = FnScope {
            module: f.module,
            types: self.type_params.clone(),
            self_type: self.self_type,
            implementing: outer.implementing.clone(),
        };
        self.leave_generics(&f.generics);
        self.leave_generics(&f.owner_generics);
        self.type_params.truncate(self.type_params.len() - outer_count);
        self.self_type = saved_self;
        let outer_generics: Vec<Ty> = outer.types.iter().map(|&(_, t)| t).collect();
        let generics = outer_generics.into_iter().chain(binders).chain(own).collect();
        let name = match &f.owner {
            Some(owner) => format!("{}.{}", owner.name, f.name.name),
            None => f.name.name.clone(),
        };
        self.fn_scopes.push(scope);
        let modes = f.params.iter().map(|p| p.mode).collect();
        Signature {
            params,
            names,
            receiver,
            modes,
            variadic,
            result,
            generics,
            name,
            defaults: Vec::new(),
            is_unsafe: f.is_unsafe,
            foreign: false,
        }
    }

    /// Declares the traits of the module: names and parameters first, so
    /// that bounds can mention any of them, then their bounds (12.3).
    /// Names the traits, before the types are declared, whose generic
    /// parameters a trait may bound.
    pub(super) fn name_traits(&mut self, traits: &[&ast::TraitDecl]) {
        for decl in traits {
            let name = &decl.name;
            if self.trait_names.contains_key(&name.name) {
                let message = format!("`{}` is already declared", name.name);
                self.diags.error("E1002", name.span, message);
                self.first_declared(name);
            } else {
                self.declared_at.insert(name.name.clone(), name.span);
            }
            let self_param = self.types.declare_param("Self".into());
            let params = decl.generics.iter().map(|g| self.types.declare_param(g.name.name.clone())).collect();
            let id = TraitId(self.traits.len() as u32);
            self.trait_modules.insert(id, decl.module);
            // The prelude's traits are everywhere, by their own name.
            let shown = match name.name.strip_prefix("prelude.") {
                Some(bare) if Some(decl.module) == self.prelude => {
                    self.trait_names.insert(bare.to_string(), id);
                    bare.to_string()
                }
                _ => name.name.clone(),
            };
            self.traits.push(TraitDef { name: shown, self_param, params, methods: Vec::new(), requires: Vec::new() });
            self.trait_names.insert(name.name.clone(), id);
        }
        // The prelude's arithmetic (18.10).
        self.additive = self.trait_names.get("Additive").copied();
        self.number = self.trait_names.get("Number").copied();
        self.integer = self.trait_names.get("Integer").copied();
        self.ordered = self.trait_names.get("Ordered").copied();
    }

    /// Declares the methods of the traits `name_traits` named.
    pub(super) fn declare_traits(&mut self, traits: &[&ast::TraitDecl]) {
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
            self.index_trait(id.0, decl);
            let (self_param, params) =
                (self.traits[id.0 as usize].self_param, self.traits[id.0 as usize].params.clone());
            self.enter_generics(&decl.generics, &params);
            for (generic, &param) in decl.generics.iter().zip(&params) {
                for bound in &generic.bounds {
                    self.apply_bound(param, bound);
                }
            }
            let requires: Vec<(TraitId, Vec<Ty>)> = decl.requires.iter().filter_map(|r| self.trait_ref(r)).collect();
            self.traits[id.0 as usize].requires = requires;
            self.leave_generics(&decl.generics);
            // Inside the trait, `Self` implements it.
            self.bounds.entry(self_param).or_default().push((id, params));
        }
        // A trait cannot require itself, even through others (12.3).
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
            let required = self.with_required(self.traits[id.0 as usize].requires.clone());
            if required.iter().any(|(t, _)| *t == id) {
                let message = format!("`{}` requires itself, through the traits it requires", decl.name.name);
                self.diags.error("E1112", decl.name.span, message);
                self.traits[id.0 as usize].requires.clear();
            }
        }
    }

    /// Bounds with the traits they require, and those they require (12.3).
    pub(super) fn with_required(&mut self, mut bounds: Vec<(TraitId, Vec<Ty>)>) -> Vec<(TraitId, Vec<Ty>)> {
        let mut at = 0;
        while at < bounds.len() && bounds.len() < 64 {
            let (trait_id, args) = bounds[at].clone();
            let def = &self.traits[trait_id.0 as usize];
            let map: HashMap<Ty, Ty> = def.params.iter().copied().zip(args).collect();
            for (required, required_args) in def.requires.clone() {
                let required_args: Vec<Ty> = required_args.iter().map(|&a| self.types.subst(a, &map)).collect();
                if !bounds.contains(&(required, required_args.clone())) {
                    bounds.push((required, required_args));
                }
            }
            at += 1;
        }
        bounds
    }

    /// Whether a language type implements a trait of the prelude's
    /// arithmetic, which the compiler gives it (18.10): every number is
    /// `Additive` and `Number`, every integer `Integer`, and a duration
    /// `Additive`. `None` for other traits and types.
    pub(super) fn arithmetic_impl(&self, ty: Ty, trait_id: TraitId) -> Option<bool> {
        let number = matches!(self.types.kind(ty), TyKind::Int(_) | TyKind::Float(_));
        if Some(trait_id) == self.integer {
            return Some(matches!(self.types.kind(ty), TyKind::Int(_)));
        }
        if Some(trait_id) == self.number && number {
            return Some(true);
        }
        if Some(trait_id) == self.additive && (number || Some(ty) == self.duration) {
            return Some(true);
        }
        None
    }

    /// The trait method by which a type takes an arithmetic operator
    /// (18.10): `plus` of `Additive` for `+` on an amount or a `T: Number`.
    /// `None` for the language's numbers, a duration and a `T: Integer`,
    /// which compute in place.
    pub(super) fn operator_trait(&mut self, op: BinOp, ty: Ty) -> Option<(TraitId, u32)> {
        let (trait_id, name) = match op {
            BinOp::Add => (self.additive?, "plus"),
            BinOp::Sub => (self.additive?, "minus"),
            BinOp::Mul => (self.number?, "times"),
            BinOp::Div => (self.number?, "dividedBy"),
            _ => return None,
        };
        let intrinsic = matches!(
            self.types.kind(ty),
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Error | TyKind::Never | TyKind::Array(_)
        );
        if intrinsic || Some(ty) == self.duration || self.bound_by(ty, self.integer) {
            return None;
        }
        if !self.satisfies(ty, trait_id, &[]) {
            return None;
        }
        let index = self.traits[trait_id.0 as usize].methods.iter().position(|m| m.name == name)?;
        Some((trait_id, index as u32))
    }

    /// `Ordered.compare` for a type the relational operators reach through
    /// it (17.8): one of the program's, or a parameter bound by `Ordered`.
    pub(super) fn ordering(&mut self, ty: Ty) -> Option<(TraitId, u32)> {
        let ordered = self.ordered?;
        if *self.types.kind(ty) == TyKind::Bool || !self.satisfies(ty, ordered, &[]) {
            return None;
        }
        let index = self.traits[ordered.0 as usize].methods.iter().position(|m| m.name == "compare")?;
        Some((ordered, index as u32))
    }

    /// Whether a type parameter or opaque result is bound by `trait_id`,
    /// itself or through what its bounds require.
    pub(super) fn bound_by(&mut self, ty: Ty, trait_id: Option<TraitId>) -> bool {
        let Some(trait_id) = trait_id else { return false };
        self.is_abstract(ty) && self.bounds_of(ty).iter().any(|(t, _)| *t == trait_id)
    }

    /// The signatures of each trait's methods, and the bodies of defaults.
    pub(super) fn trait_methods<'d>(&mut self, traits: &[&'d ast::TraitDecl], bodies: &mut Vec<&'d ast::FnDecl>) {
        for (index, decl) in traits.iter().enumerate() {
            let id = TraitId(index as u32 + PRELUDE_TRAITS);
            let def = &self.traits[id.0 as usize];
            let mut types = vec![("Self".to_string(), def.self_param)];
            types.extend(decl.generics.iter().map(|g| g.name.name.clone()).zip(def.params.iter().copied()));
            let scope = FnScope { module: decl.module, types, self_type: Some(def.self_param), implementing: None };
            let mut seen = HashSet::new();
            for f in &decl.methods {
                if !seen.insert(f.name.name.as_str()) {
                    let message = format!("`{}` declares `{}` more than once", decl.name.name, f.name.name);
                    self.diags.error("E1505", f.name.span, message);
                    continue;
                }
                let mut signature = self.signature(f, None, scope.clone());
                signature.name = format!("{}.{}", decl.name.name, f.name.name);
                self.no_defaults(f);
                let default = match &f.body {
                    Some(_) => {
                        let fn_id = FnId(bodies.len() as u32);
                        self.signatures.push(signature.clone());
                        bodies.push(f);
                        Some(fn_id)
                    }
                    None => {
                        self.fn_scopes.pop();
                        None
                    }
                };
                let method = TraitMethod { name: f.name.name.clone(), signature, default, span: f.name.span };
                self.traits[id.0 as usize].methods.push(method);
            }
        }
    }

    /// The methods the prelude gives every iterator (10.17): defaults of the
    /// intrinsic `Iterator`, whose `self` they copy, so their receivers are
    /// `Copy`.
    pub(super) fn iterator_defaults<'d>(&mut self, decl: &'d ast::TraitDecl, bodies: &mut Vec<&'d ast::FnDecl>) {
        let def = &self.traits[ITERATOR.0 as usize];
        let (self_param, element) = (def.self_param, def.params[0]);
        self.types.param_mut(self_param).copy = true;
        self.types.param_mut(element).copy = true;
        let Some(generic) = decl.generics.first() else { return };
        let types = vec![("Self".to_string(), self_param), (generic.name.name.clone(), element)];
        let scope = FnScope { module: decl.module, types, self_type: Some(self_param), implementing: None };
        for f in &decl.methods {
            if f.name.name == "next" || f.body.is_none() {
                continue;
            }
            let mut signature = self.signature(f, None, scope.clone());
            signature.name = format!("Iterator.{}", f.name.name);
            let id = FnId(bodies.len() as u32);
            self.signatures.push(signature.clone());
            bodies.push(f);
            let method = TraitMethod { name: f.name.name.clone(), signature, default: Some(id), span: f.name.span };
            self.traits[ITERATOR.0 as usize].methods.push(method);
        }
    }

    /// The trait a bound or implementation names, with its arguments.
    pub(super) fn trait_ref(&mut self, t: &ast::TypeExpr) -> Option<(TraitId, Vec<Ty>)> {
        let ast::TypeKind::Path { segments, args } = &t.kind else {
            self.diags.error("E1105", t.span, "expected a trait");
            return None;
        };
        let [name] = segments.as_slice() else {
            self.diags.error("E1105", t.span, "expected a trait");
            return None;
        };
        let Some(&id) = self.trait_names.get(&name.name) else {
            let message = format!("unknown trait `{}`", name.name);
            self.diags.error("E1001", name.span, message);
            return None;
        };
        self.index_refer(name, crate::index::Key::Trait(id.0));
        let args: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
        let expected = self.traits[id.0 as usize].params.len();
        if args.len() != expected {
            let message = format!("`{}` takes {expected} type argument(s), found {}", name.name, args.len());
            self.diags.error("E1104", t.span, message);
            return None;
        }
        Some((id, args))
    }

    /// The error the compiler declares for a standard module it implements.
    fn module_error(&self, module: &str) -> Option<Ty> {
        match module {
            "fs" => self.fs.as_ref().map(|fs| fs.error),
            "time" => self.time_error,
            "env" => self.env_error,
            "process" => self.process.as_ref().map(|process| process.error),
            "text" => Some(self.text_error),
            "io" => Some(self.io_error),
            _ => None,
        }
    }

    /// Checks an implementation's header and registers its methods (12.4).
    pub(super) fn implementation<'d>(&mut self, decl: &'d ast::ImplDecl, bodies: &mut Vec<&'d ast::FnDecl>) {
        let binders = self.declare_generics(&decl.generics, true, decl.module);
        self.enter_generics(&decl.generics, &binders);
        let resolved = self.trait_ref(&decl.trait_ref);
        let target = self.resolve_type(&decl.target);
        self.leave_generics(&decl.generics);
        let Some((trait_id, trait_args)) = resolved else { return };
        if target == Ty::ERROR {
            return;
        }
        let trait_name = self.traits[trait_id.0 as usize].name.clone();
        let target_name = self.types.display(target);
        // `Integer` is the language's own integers (18.10).
        if Some(trait_id) == self.integer && Some(decl.module) != self.prelude {
            let message = format!(
                "`Integer` is the language's own integers, and a program does not implement it: implement `Number` for `{target_name}`, which gives it `+`, `-`, `*`, `/` and literals"
            );
            self.diags.error("E1114", decl.trait_ref.span, message);
            return;
        }
        // Coherence (12.6): the module owns the trait or the target's nominal type.
        let local_trait = self.trait_modules.get(&trait_id) == Some(&decl.module);
        // A standard module written in Tondo that completes one the compiler
        // implements owns the error the compiler declares for it, as `files`
        // that of `fs`, whatever name the program imports the module by.
        let completes = || match self.module_paths.get(decl.module as usize).map(Vec::as_slice) {
            Some([std, companion]) if std == "std" => {
                crate::stdlib::completed(companion).is_some_and(|module| self.module_error(module) == Some(target))
            }
            _ => false,
        };
        let local_target = self.owner(target).is_some_and(|o| o.module == decl.module)
            || (self.datetime_module == Some(decl.module)
                && [self.datetime, self.duration, self.instant].contains(&Some(target)))
            || (self.pacing_module == Some(decl.module) && matches!(self.types.kind(target), TyKind::Handle(..)))
            || completes();
        if !local_trait && !local_target {
            let message = format!(
                "`impl {trait_name} for {target_name}` implements a trait for a type, neither of which this module declares; implement it in the module of one of them, or wrap the type in a newtype, such as `type Name = {target_name}`"
            );
            self.diags.error("E1114", decl.target.span, message);
            return;
        }
        let mut mentioned = Vec::new();
        self.types.params_in(target, &mut mentioned);
        trait_args.iter().for_each(|&a| self.types.params_in(a, &mut mentioned));
        for (generic, binder) in decl.generics.iter().zip(&binders) {
            if !mentioned.contains(binder) {
                let message =
                    format!("`{}` must appear in the trait or the target of the implementation", generic.name.name);
                self.diags.error("E1114", generic.name.span, message);
            }
        }
        // A type has at most one `Iterator`, whatever it yields (10.17).
        if trait_id == ITERATOR {
            let earlier: Vec<(Ty, Vec<Ty>, Span)> = self
                .impls
                .iter()
                .filter(|i| i.trait_id == ITERATOR)
                .map(|i| (i.target, i.generics.clone(), i.span))
                .collect();
            for (other, generics, at) in earlier {
                let vars: Vec<Ty> = binders.iter().chain(&generics).copied().collect();
                if self.types.unify_both(target, other, &vars, &mut HashMap::new()) {
                    let message =
                        format!("`{target_name}` already implements `Iterator`; a type yields one element type");
                    self.diags.error("E1113", decl.span, message);
                    self.diags.related(at, "the `Iterator` it has");
                    return;
                }
            }
        }
        // Only those of the same head may overlap it.
        let earlier: Vec<(Vec<Ty>, Ty, Vec<Ty>, Span)> = self
            .impl_candidates(trait_id, target)
            .into_iter()
            .map(|i| {
                let other = &self.impls[i];
                (other.trait_args.clone(), other.target, other.generics.clone(), other.span)
            })
            .collect();
        for (other_args, other_target, other_generics, at) in earlier {
            if self.headers_overlap((&trait_args, target, &binders), (&other_args, other_target, &other_generics)) {
                let message = format!(
                    "`impl {trait_name} for {target_name}` overlaps an earlier implementation for `{}`",
                    self.types.display(other_target)
                );
                self.diags.error("E1111", decl.span, message);
                self.diags.related(at, "the implementation it overlaps");
                return;
            }
        }
        // Each method must match the trait's signature with `Self` and the
        // trait's parameters substituted.
        let def = &self.traits[trait_id.0 as usize];
        let mut map: HashMap<Ty, Ty> = HashMap::from([(def.self_param, target)]);
        map.extend(def.params.iter().copied().zip(trait_args.iter().copied()));
        let declared: Vec<(String, Signature, bool)> =
            def.methods.iter().map(|m| (m.name.clone(), m.signature.clone(), m.default.is_some())).collect();
        let declared_at: Vec<Span> = def.methods.iter().map(|m| m.span).collect();
        let mut first: Vec<Span> = vec![Span::default(); declared.len()];
        let mut types: Vec<(String, Ty)> =
            decl.generics.iter().map(|g| g.name.name.clone()).zip(binders.iter().copied()).collect();
        types.dedup();
        let scope = FnScope {
            module: decl.module,
            types,
            self_type: Some(target),
            implementing: Some((trait_id, trait_args.clone())),
        };
        let mut methods = vec![None; declared.len()];
        for f in &decl.methods {
            let Some(index) = declared.iter().position(|(name, ..)| *name == f.name.name) else {
                let message = format!(
                    "`{trait_name}` has no method `{}`; an operation of `{target_name}` alone is an inherent method `fn {target_name}.{}(...)`",
                    f.name.name, f.name.name
                );
                self.diags.error("E1114", f.name.span, message);
                continue;
            };
            if methods[index].is_some() {
                let message = format!("`{}` is implemented more than once", f.name.name);
                self.diags.error("E1114", f.name.span, message);
                self.diags.related(first[index], format!("`{}` is first implemented here", f.name.name));
                continue;
            }
            first[index] = f.name.span;
            let id = FnId(bodies.len() as u32);
            let mut signature = self.signature(f, None, scope.clone());
            signature.name = format!("{trait_name}.{}", f.name.name);
            self.no_defaults(f);
            let expected = &declared[index].1;
            if let Some(problem) = self.signature_mismatch(expected, &signature, &map, binders.len()) {
                // An `unsafe` contract that differs is its own error (16.12).
                let code = if expected.is_unsafe != signature.is_unsafe { "E1702" } else { "E1114" };
                let message = format!("`{}` does not match `{trait_name}`: {problem}", f.name.name);
                self.diags.error(code, f.name.span, message);
                self.diags.related(declared_at[index], format!("`{trait_name}` declares `{}` here", f.name.name));
            }
            self.signatures.push(signature);
            self.index_implemented(trait_id.0, id.0, &f.name);
            bodies.push(f);
            methods[index] = Some(id);
        }
        let mut stubs: Vec<(Span, String)> = Vec::new();
        for ((name, signature, has_default), provided) in declared.iter().zip(&methods) {
            if provided.is_none() && !has_default {
                let message = format!("`impl {trait_name} for {target_name}` is missing `{name}`");
                self.diags.error("E1114", decl.span, message);
                if let Some(edit) = self.method_stub(decl.span, name, signature, &map) {
                    self.diags.fix(format!("write `{name}`"), false, vec![edit.clone()]);
                    stubs.push(edit);
                }
            }
        }
        // They all go in the same place.
        if let [(at, _), _, ..] = stubs[..] {
            let every: String = stubs.iter().map(|(_, stub)| stub.as_str()).collect();
            self.diags.fix(format!("write the {} methods missing", stubs.len()), false, vec![(at, every)]);
        }
        let name =
            self.index.is_some().then(|| self.index_type_name(target).unwrap_or_else(|| self.types.display(target)));
        if let (Some(index), Some(name)) = (self.index.as_mut(), name) {
            let declared = index.of(&crate::index::Key::Trait(trait_id.0));
            index.impls.push((declared, decl.span, name));
        }
        self.impls.push(ImplDef {
            intrinsic: false,
            span: decl.span,
            trait_id,
            trait_args,
            target,
            generics: binders,
            methods,
        });
    }

    /// The edit that writes a trait's method into an `impl` before its
    /// closing brace, with the trait's signature for this type and a body
    /// that says it is not written yet.
    pub(super) fn method_stub(
        &mut self,
        within: Span,
        name: &str,
        signature: &Signature,
        map: &HashMap<Ty, Ty>,
    ) -> Option<(Span, String)> {
        let shown = |checker: &mut Self, ty: Ty| {
            let ty = checker.types.subst(ty, map);
            checker.types.display(ty)
        };
        let own: Vec<Ty> = signature.generics.iter().copied().filter(|g| !map.contains_key(g)).collect();
        let mut generics = Vec::new();
        for generic in own {
            let param = self.types.param(generic).map(|p| p.name.clone()).unwrap_or_default();
            let bounds: Vec<String> = self
                .bounds
                .get(&generic)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(bound, args)| {
                    let trait_name = self.traits[bound.0 as usize].name.clone();
                    match args.is_empty() {
                        true => trait_name,
                        false => {
                            let args: Vec<String> = args.iter().map(|&a| shown(self, a)).collect();
                            format!("{trait_name}[{}]", args.join(", "))
                        }
                    }
                })
                .collect();
            generics.push(if bounds.is_empty() { param } else { format!("{param}: {}", bounds.join(" + ")) });
        }
        let mut params = Vec::new();
        for (index, (&ty, written)) in signature.params.iter().zip(&signature.names).enumerate() {
            let mode = signature.modes.get(index).copied().unwrap_or(ast::Mode::Value);
            if index == 0 && signature.receiver.is_some() {
                params.push(match signature.receiver {
                    Some(ast::Mode::Mut) => "mut self".to_string(),
                    Some(ast::Mode::Var) => "var self".to_string(),
                    _ => "self".to_string(),
                });
                continue;
            }
            let last = index + 1 == signature.params.len();
            let ty = match (signature.variadic && last, self.types.kind(ty).clone()) {
                (true, TyKind::Array(element)) => format!("...{}", shown(self, element)),
                _ => shown(self, ty),
            };
            let mode = match mode {
                ast::Mode::Value => String::new(),
                mode => format!("{} ", mode.keyword()),
            };
            params.push(format!("{written}: {mode}{ty}"));
        }
        let returned = self.types.subst(signature.result, map);
        let result = match self.types.kind(returned).clone() {
            TyKind::Unit => String::new(),
            TyKind::Result(Ty::UNIT, error) => format!(": !{}", shown(self, error)),
            _ => format!(": {}", shown(self, signature.result)),
        };
        let generics = if generics.is_empty() { String::new() } else { format!("[{}]", generics.join(", ")) };
        // At the `impl`'s indentation and one more, before its last brace.
        let file = self.sources.file_of(within.start);
        let text = &file.text[(within.start - file.base) as usize..(within.end - file.base) as usize];
        if !text.ends_with('}') {
            return None;
        }
        let close = within.end - 1;
        let line = file.text[..(within.start - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
        let indent: String = file.text[line..].chars().take_while(|c| *c == ' ').collect();
        let method = format!(
            "{indent}    fn {name}{generics}({}){result} {{\n{indent}        panic(\"`{name}` is not written yet\")\n{indent}    }}\n",
            params.join(", ")
        );
        let last = file.text[..(close - file.base) as usize].rfind('\n').map_or(0, |at| at + 1);
        let before = &file.text[last..(close - file.base) as usize];
        // A blank line after the methods the `impl` has.
        let open = within.start + text.find('{')? as u32 + 1;
        let methods = !file.text[(open - file.base) as usize..last.max((open - file.base) as usize)].trim().is_empty();
        let method = if methods { format!("\n{method}") } else { method };
        Some(match before.trim().is_empty() {
            true => (Span::new(file.base + last as u32, file.base + last as u32), method),
            false => (Span::new(close, close), format!("\n{method}{indent}")),
        })
    }

    /// Each implementation's target implements the traits its trait
    /// requires (12.3).
    pub(super) fn check_required(&mut self) {
        for index in 0..self.impls.len() {
            let (trait_id, trait_args, target, span) = {
                let i = &self.impls[index];
                (i.trait_id, i.trait_args.clone(), i.target, i.span)
            };
            if self.impls[index].intrinsic {
                continue;
            }
            let def = &self.traits[trait_id.0 as usize];
            let map: HashMap<Ty, Ty> = def.params.iter().copied().zip(trait_args).collect();
            for (required, args) in def.requires.clone() {
                let args: Vec<Ty> = args.iter().map(|&a| self.types.subst(a, &map)).collect();
                if self.satisfies(target, required, &args) {
                    continue;
                }
                let (name, required_name) =
                    (self.traits[trait_id.0 as usize].name.clone(), self.traits[required.0 as usize].name.clone());
                let target = self.types.display(target);
                let message = format!(
                    "`impl {name} for {target}` needs `impl {required_name} for {target}` too: `{name}` requires `{required_name}`"
                );
                self.diags.error("E1105", span, message);
            }
        }
    }

    /// The size-change termination check of trait resolution (12.6). Each
    /// generic implementation leads from the query of its header to each
    /// trait bound of its binders, with a matrix relating their arguments
    /// (`<` strictly smaller, `=` identical, `?` unknown). Every idempotent
    /// composition that returns to the same trait must shrink some argument.
    pub(super) fn check_termination(&mut self) {
        let mut edges: Vec<(TraitId, TraitId, Matrix, Span)> = Vec::new();
        for implementation in self.impls.iter().filter(|i| !i.intrinsic) {
            let mut source = implementation.trait_args.clone();
            source.push(implementation.target);
            for &binder in &implementation.generics {
                for (trait_id, args) in self.bounds.get(&binder).cloned().unwrap_or_default() {
                    let mut target = args;
                    target.push(binder);
                    let matrix = target
                        .iter()
                        .map(|&to| source.iter().map(|&from| self.size_relation(to, from)).collect())
                        .collect();
                    edges.push((implementation.trait_id, trait_id, matrix, implementation.span));
                }
            }
        }
        // Saturate the compositions of paths, remembering the traits passed.
        let mut known: HashMap<(TraitId, TraitId, Matrix), (Vec<TraitId>, Span)> = HashMap::new();
        let mut work: Vec<(TraitId, TraitId, Matrix, Vec<TraitId>, Span)> =
            edges.iter().map(|(from, to, m, span)| (*from, *to, m.clone(), vec![*from, *to], *span)).collect();
        while let Some((from, to, matrix, path, span)) = work.pop() {
            if known.contains_key(&(from, to, matrix.clone())) {
                continue;
            }
            for (next_from, next_to, next, _) in &edges {
                if *next_from == to {
                    let mut longer = path.clone();
                    longer.push(*next_to);
                    work.push((from, *next_to, compose(next, &matrix), longer, span));
                }
            }
            known.insert((from, to, matrix), (path, span));
        }
        // Report each cycle once, in a stable order.
        let mut cycles: Vec<(&Vec<TraitId>, &Matrix, Span)> = known
            .iter()
            .filter(|((from, to, matrix), _)| {
                let shrinks = (0..matrix.len()).any(|i| matrix[i][i] == Size::Less);
                from == to && compose(matrix, matrix) == *matrix && !shrinks
            })
            .map(|((_, _, matrix), (path, span))| (path, matrix, *span))
            .collect();
        cycles.sort_by_key(|(path, _, span)| (path.len(), path.iter().map(|t| t.0).collect::<Vec<_>>(), span.start));
        let mut reported = HashSet::new();
        for (path, matrix, span) in cycles {
            if path.iter().any(|t| reported.contains(t)) {
                continue;
            }
            reported.extend(path.iter().copied());
            let names: Vec<String> = path.iter().map(|t| format!("`{}`", self.traits[t.0 as usize].name)).collect();
            let shown: Vec<String> = matrix
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|c| match c {
                            Size::Less => "<",
                            Size::Equal => "=",
                            Size::Unknown => "?",
                        })
                        .collect()
                })
                .collect();
            let message = format!(
                "resolving {} may never end: no type argument gets smaller along the way (matrix [{}]); bound a part of the target instead, as in `impl[T: Trait] Trait for Box[T]`",
                names.join(" -> "),
                shown.join("; ")
            );
            self.diags.error("E1112", span, message);
        }
    }

    /// How the term `to` relates in size to `from` (12.6).
    pub(super) fn size_relation(&self, to: Ty, from: Ty) -> Size {
        if to == from {
            Size::Equal
        } else if self.strict_subterm(to, from) {
            Size::Less
        } else {
            Size::Unknown
        }
    }

    /// Whether `part` is reached by descending at least once through the
    /// arguments of `whole`'s type constructors.
    pub(super) fn strict_subterm(&self, part: Ty, whole: Ty) -> bool {
        let children: Vec<Ty> = match self.types.kind(whole) {
            TyKind::Tuple(items) | TyKind::Union(items) | TyKind::Opaque(_, items) => items.clone(),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) | TyKind::Handle(_, t) => vec![*t],
            TyKind::Result(a, b) | TyKind::Map(a, b) => vec![*a, *b],
            TyKind::Fn(params, result, ..) => params.iter().copied().chain([*result]).collect(),
            TyKind::Adt(_) => self.types.instance_of(whole).map_or(Vec::new(), |(_, args)| args.to_vec()),
            _ => Vec::new(),
        };
        children.into_iter().any(|child| child == part || self.strict_subterm(part, child))
    }

    /// How an implementation's method differs from the trait's, if at all.
    pub(super) fn signature_mismatch(
        &mut self,
        expected: &Signature,
        found: &Signature,
        map: &HashMap<Ty, Ty>,
        binders: usize,
    ) -> Option<String> {
        // `map` binds `Self` and the trait's parameters, which come first.
        let own_expected = &expected.generics[map.len()..];
        let own_found = &found.generics[binders..];
        if own_expected.len() != own_found.len() {
            return Some(format!("{} type parameter(s) are expected", own_expected.len()));
        }
        let mut map = map.clone();
        map.extend(own_expected.iter().copied().zip(own_found.iter().copied()));
        if expected.is_unsafe != found.is_unsafe {
            return Some(if expected.is_unsafe {
                "the trait declares it `unsafe fn`".into()
            } else {
                "the trait declares it without `unsafe`".into()
            });
        }
        if expected.receiver != found.receiver {
            return Some("a different receiver".into());
        }
        if let Some(index) =
            (0..expected.modes.len().min(found.modes.len())).find(|&i| expected.modes[i] != found.modes[i])
        {
            let mode = match expected.modes[index] {
                ast::Mode::Value => "by value".to_string(),
                mode => format!("as a `{}` loan", mode.keyword()),
            };
            return Some(format!("parameter `{}` is passed {mode}", expected.names[index]));
        }
        if expected.params.len() != found.params.len() || expected.variadic != found.variadic {
            return Some(format!("{} parameter(s) are expected", expected.params.len()));
        }
        for (index, (&want, &got)) in expected.params.iter().zip(&found.params).enumerate() {
            let want = self.types.subst(want, &map);
            if want != got && !self.types.has_error(got) {
                let name = &expected.names[index];
                return Some(format!("parameter `{name}` must be {}", self.display(want)));
            }
        }
        let result = self.types.subst(expected.result, &map);
        if result != found.result && !self.types.has_error(found.result) {
            return Some(format!("the result must be {}", self.display(result)));
        }
        None
    }

    /// Whether two implementation headers can describe the same pair of
    /// trait arguments and target (12.6).
    pub(super) fn headers_overlap(&mut self, a: (&[Ty], Ty, &[Ty]), b: (&[Ty], Ty, &[Ty])) -> bool {
        let vars: Vec<Ty> = a.2.iter().chain(b.2).copied().collect();
        let mut map = HashMap::new();
        let left: Vec<Ty> = a.0.iter().copied().chain([a.1]).collect();
        let right: Vec<Ty> = b.0.iter().copied().chain([b.1]).collect();
        left.iter().zip(&right).all(|(&x, &y)| self.types.unify_both(x, y, &vars, &mut map))
    }

    /// Whether `ty` implements `trait_id` with `args` (12.5): by a bound of a
    /// type parameter, intrinsically (`Display` of built-in values), or by an
    /// implementation whose bounds hold.
    pub(super) fn satisfies(&mut self, ty: Ty, trait_id: TraitId, args: &[Ty]) -> bool {
        self.satisfies_depth(ty, trait_id, args, 0)
    }

    pub(super) fn satisfies_depth(&mut self, ty: Ty, trait_id: TraitId, args: &[Ty], depth: usize) -> bool {
        if depth > 32 || self.types.has_error(ty) || ty == Ty::NEVER {
            return true;
        }
        if self.is_abstract(ty) {
            return self.bounds_of(ty).iter().any(|(t, a)| *t == trait_id && a == args);
        }
        if let Some(holds) = self.arithmetic_impl(ty, trait_id) {
            return holds;
        }
        if trait_id == DISPLAY {
            let parts: Option<Vec<Ty>> = match self.types.kind(ty) {
                TyKind::Int(_)
                | TyKind::Float(_)
                | TyKind::Char
                | TyKind::Byte
                | TyKind::Bool
                | TyKind::String
                | TyKind::Vector(..)
                | TyKind::Mask(..) => Some(Vec::new()),
                TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::Option(element) => {
                    Some(vec![*element])
                }
                TyKind::Map(key, value) | TyKind::Result(key, value) => Some(vec![*key, *value]),
                // A union shows the member it holds.
                TyKind::Tuple(items) | TyKind::Union(items) => Some(items.clone()),
                _ => None,
            };
            if let Some(parts) = parts {
                return parts.into_iter().all(|part| self.satisfies_depth(part, DISPLAY, &[], depth + 1));
            }
            // Standard library errors display a human description (7.4);
            // durations and dates their usual forms.
            if self.types.shown.contains_key(&ty) {
                return true;
            }
            // An enum whose variants hold nothing shows the variant's name
            // (19.6): the compiler's own, and a generic one, which have no
            // `impl` written for them.
            if let TyKind::Adt(id) = self.types.kind(ty)
                && let AdtKind::Enum(variants) = &self.types.adt(*id).kind
                && !variants.is_empty()
                && variants.iter().all(|v| matches!(v.payload, Payload::Unit))
            {
                return true;
            }
        }
        // A union is written as the member it holds, and read as the first
        // member that reads the value: the compiler writes both (standard
        // library 14.6).
        if let TyKind::Union(members) = self.types.kind(ty)
            && matches!(self.traits[trait_id.0 as usize].name.as_str(), "serialization.Encode" | "serialization.Decode")
        {
            let members = members.clone();
            return members.into_iter().all(|member| self.satisfies_depth(member, trait_id, args, depth + 1));
        }
        for index in self.impl_candidates(trait_id, ty) {
            let (target, trait_args, generics) =
                (self.impls[index].target, self.impls[index].trait_args.clone(), self.impls[index].generics.clone());
            let mut map = HashMap::new();
            let matched = self.types.matches(target, ty, &generics, &mut map)
                && trait_args.iter().zip(args).all(|(&p, &a)| self.types.matches(p, a, &generics, &mut map));
            if !matched {
                continue;
            }
            return generics.iter().all(|g| {
                let actual = map.get(g).copied().unwrap_or(Ty::ERROR);
                self.bound_holds(*g, actual, &map, depth + 1).is_none()
            });
        }
        // A record or an enum without an `impl Display` shows itself when
        // each of its parts does (9.4); a part that holds it again shows as
        // it does.
        if trait_id == DISPLAY
            && let Some(parts) = self.displayed_parts(ty)
        {
            if !self.showing.insert(ty) {
                return true;
            }
            let shown = parts.into_iter().all(|(_, part, _)| self.satisfies_depth(part, DISPLAY, &[], depth + 1));
            self.showing.remove(&ty);
            return shown;
        }
        false
    }

    /// What a record or an enum declared in a module shows of itself when
    /// it has no `impl Display` (9.4): each field but a `@secret` one, and
    /// the data of each variant, with how to name it and whether it is
    /// `priv`; `None` for another type, and for an iterator, which
    /// `collect()` shows.
    pub(super) fn displayed_parts(&mut self, ty: Ty) -> Option<Vec<(String, Ty, bool)>> {
        let TyKind::Adt(id) = *self.types.kind(ty) else { return None };
        let adt = self.types.adt(id);
        adt.module?;
        if adt.secret {
            return Some(Vec::new());
        }
        let parts = match &adt.kind {
            AdtKind::Record(fields) => fields
                .iter()
                .filter(|f| !f.secret)
                .map(|f| (format!("its field `{}`", f.name), f.ty, f.private))
                .collect(),
            AdtKind::Enum(variants) => variants
                .iter()
                .flat_map(|v| match &v.payload {
                    Payload::Unit => Vec::new(),
                    Payload::Tuple(items) => {
                        items.iter().map(|&item| (format!("the data of `{}`", v.name), item, false)).collect()
                    }
                    Payload::Record(fields) => fields
                        .iter()
                        .filter(|f| !f.secret)
                        .map(|f| (format!("the field `{}` of `{}`", f.name, v.name), f.ty, f.private))
                        .collect(),
                })
                .collect(),
            AdtKind::Newtype(_) => return None,
        };
        if self.iterator_element(ty).is_some() {
            return None;
        }
        Some(parts)
    }

    /// The first part of a record or an enum that has no `Display`, which
    /// is why it has none: as `its field `run`, of type `fn()``, and how to
    /// give it one when the part is a type parameter; `None` when it is a
    /// `priv` field of another module's type, which is no business here.
    pub(super) fn undisplayed_part(&mut self, ty: Ty) -> Option<String> {
        let parts = self.displayed_parts(ty)?;
        let (name, part, private) = parts.into_iter().find(|&(_, part, _)| !self.satisfies(part, DISPLAY, &[]))?;
        if private && self.owner(ty).is_none_or(|o| o.module != self.module) {
            return None;
        }
        let shown = self.types.display(part);
        Some(match self.types.kind(part) {
            TyKind::Param(_) => format!("{name}, of type `{shown}`, has none: declare `{shown}: Display`"),
            _ => format!("{name}, of type `{shown}`, has none"),
        })
    }

    /// The `impl`s of a trait that may be for `ty`, in the order they were
    /// declared: those for a type of the same head, and those for a type
    /// parameter; all of the trait's when `ty` is one.
    pub(super) fn impl_candidates(&mut self, trait_id: TraitId, ty: Ty) -> Vec<usize> {
        while self.heads_indexed < self.impls.len() {
            let index = self.heads_indexed;
            let key = (self.impls[index].trait_id, self.head(self.impls[index].target));
            self.impl_heads.entry(key).or_default().push(index);
            self.heads_indexed += 1;
        }
        let Some(head) = self.head(ty) else {
            return (0..self.impls.len()).filter(|&i| self.impls[i].trait_id == trait_id).collect();
        };
        let empty = Vec::new();
        let same = self.impl_heads.get(&(trait_id, Some(head))).unwrap_or(&empty);
        let any = self.impl_heads.get(&(trait_id, None)).unwrap_or(&empty);
        let mut candidates: Vec<usize> = same.iter().chain(any).copied().collect();
        candidates.sort_unstable();
        candidates
    }

    /// What a type is at its outermost: a nominal type, by its template for
    /// an instance of a generic one, or a kind of type; `None` for a type
    /// parameter, which any type may be.
    pub(super) fn head(&self, ty: Ty) -> Option<Head> {
        match self.types.kind(ty) {
            TyKind::Param(_) => None,
            TyKind::Adt(id) => Some(match &self.types.adt(*id).instance {
                Some((template, _)) => Head::Template(*template),
                None => Head::Adt(*id),
            }),
            kind => Some(Head::Kind(std::mem::discriminant(kind))),
        }
    }

    /// The first bound of the type parameter `param` that `actual` does not
    /// meet, as text for a diagnostic.
    /// The first field of a record that misses a bound of `param`, which is
    /// why the compiler did not implement it for the record.
    pub(super) fn lacking_field(&mut self, record: Ty, param: Ty, map: &HashMap<Ty, Ty>) -> Option<(String, Ty)> {
        let TyKind::Adt(id) = self.types.kind(record) else { return None };
        let AdtKind::Record(fields) = &self.types.adt(*id).kind else { return None };
        let fields: Vec<(String, Ty)> = fields.iter().map(|f| (f.name.clone(), f.ty)).collect();
        fields.into_iter().find(|&(_, ty)| self.bound_holds(param, ty, map, 0).is_some())
    }

    pub(super) fn bound_holds(&mut self, param: Ty, actual: Ty, map: &HashMap<Ty, Ty>, depth: usize) -> Option<String> {
        let def = self.types.param(param)?;
        let (equatable, key, copy, send, share) = (def.equatable, def.key, def.copy, def.send, def.share);
        if copy && !self.types.is_copy(actual) {
            return Some("`Copy` (type parameters are `Copy` unless declared `~Copy`)".into());
        }
        if send && !self.types.is_send(actual) {
            return Some("`Send`".into());
        }
        if share && !self.types.is_share(actual) {
            return Some("`Share`".into());
        }
        if key && !self.types.is_key(actual) {
            return Some("`Key`".into());
        }
        if equatable && !self.types.is_equatable(actual) {
            return Some("`Equatable`".into());
        }
        if let Some((protocol, signature)) = self.call_bounds.get(&param).copied() {
            let signature = self.types.subst(signature, map);
            if let Some(problem) = self.callable_as(actual, protocol, signature) {
                return Some(problem);
            }
        }
        let bounds = self.bounds.get(&param).cloned().unwrap_or_default();
        for (trait_id, args) in bounds {
            let args: Vec<Ty> = args.iter().map(|&a| self.types.subst(a, map)).collect();
            if !self.satisfies_depth(actual, trait_id, &args, depth) {
                return Some(format!("`{}`", self.traits[trait_id.0 as usize].name));
            }
        }
        None
    }

    /// Infers parameters that only appear in trait bounds of others, such
    /// as `T` in `I: Iterator[T]`, from the implementation of the type
    /// inferred for the bounded parameter when only one can apply.
    pub(super) fn infer_from_bounds(&mut self, inference: &mut Inference) {
        for var in inference.vars.clone() {
            let Some(&actual) = inference.map.get(&var) else { continue };
            for (trait_id, args) in self.bounds.get(&var).cloned().unwrap_or_default() {
                if args.iter().all(|&a| self.inferred(inference, a).is_some()) {
                    continue;
                }
                let found: Vec<Vec<Ty>> = if self.is_abstract(actual) {
                    self.bounds_of(actual).into_iter().filter(|(t, _)| *t == trait_id).map(|(_, a)| a).collect()
                } else {
                    let candidates: Vec<(Ty, Vec<Ty>, Vec<Ty>)> = self
                        .impls
                        .iter()
                        .filter(|i| i.trait_id == trait_id)
                        .map(|i| (i.target, i.trait_args.clone(), i.generics.clone()))
                        .collect();
                    candidates
                        .into_iter()
                        .filter_map(|(target, trait_args, generics)| {
                            let mut map = HashMap::new();
                            self.types
                                .matches(target, actual, &generics, &mut map)
                                .then(|| trait_args.iter().map(|&a| self.types.subst(a, &map)).collect())
                        })
                        .collect()
                };
                if let [values] = found.as_slice() {
                    for (&arg, &value) in args.iter().zip(values) {
                        self.unify(inference, arg, value);
                    }
                }
            }
        }
    }

    /// The result a closure is checked against: what inference knows, or
    /// `T ! {error}` when only `T` is known, so that the closure infers its
    /// errors, none when it cannot fail; `{error}` when nothing is known.
    pub(super) fn expected_result(&mut self, inference: &Inference, result: Ty) -> Ty {
        if let Some(result) = self.inferred(inference, result) {
            return result;
        }
        match self.types.kind(result).clone() {
            TyKind::Result(success, _) => match self.inferred(inference, success) {
                Some(success) => self.types.intern(TyKind::Result(success, Ty::ERROR)),
                None => Ty::ERROR,
            },
            _ => Ty::ERROR,
        }
    }

    /// What a closure passed for the type parameter `ty` with a call bound
    /// should look like: the bound's signature with the arguments inferred
    /// so far. An unknown result is left to the closure (`{error}`).
    pub(super) fn call_expectation(&mut self, inference: &Inference, ty: Ty) -> Option<Ty> {
        let &(_, bound) = self.call_bounds.get(&ty)?;
        let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(bound).clone() else {
            return None;
        };
        let params = params.into_iter().map(|p| self.inferred(inference, p)).collect::<Option<Vec<Ty>>>()?;
        let result = self.expected_result(inference, result);
        Some(self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe)))
    }

    /// What a closure for a `fn(A): B` parameter is checked against when
    /// its parameters are known and its result is not yet: `B` comes from
    /// the closure's body.
    pub(super) fn fn_expectation(&mut self, inference: &Inference, ty: Ty, annotated: bool) -> Option<Ty> {
        let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(ty).clone() else {
            return None;
        };
        // A closure that writes its parameters' types needs only the result.
        let params = params
            .into_iter()
            .map(|p| self.inferred(inference, p).or(annotated.then_some(Ty::ERROR)))
            .collect::<Option<Vec<Ty>>>()?;
        let result = self.expected_result(inference, result);
        Some(self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe)))
    }

    /// Why `ty` cannot be called as `signature` with the protocol a call
    /// bound demands, if it cannot (11.8).
    pub(super) fn callable_as(&mut self, ty: Ty, protocol: Protocol, signature: Ty) -> Option<String> {
        let wanted = format!("`{protocol:?}[{}]`", self.types.display(signature));
        match self.callable(ty) {
            Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe }) => {
                let found = self.types.intern(TyKind::Fn(params, result, variadic, modes, is_unsafe));
                if found != signature {
                    Some(format!("{wanted} (its signature is `{}`)", self.types.display(found)))
                } else if once && protocol != Protocol::CallOnce {
                    Some(format!("{wanted} (it gives a capture away, so it is only `CallOnce`)"))
                } else if mutates && protocol == Protocol::Call {
                    Some(format!("{wanted} (it changes its captures, so it is only `CallMut`)"))
                } else {
                    None
                }
            }
            None if self.types.has_error(ty) => None,
            None => Some(wanted),
        }
    }

    /// How a value of `ty` is called, if it can be.
    pub(super) fn callable(&mut self, ty: Ty) -> Option<Callable> {
        if let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(ty).clone() {
            return Some(Callable { params, result, variadic, mutates: false, once: false, modes, is_unsafe });
        }
        if let Some((protocol, signature)) = self.call_bound_of(ty) {
            let TyKind::Fn(params, result, variadic, modes, is_unsafe) = self.types.kind(signature).clone() else {
                unreachable!()
            };
            let (mutates, once) = (protocol != Protocol::Call, protocol == Protocol::CallOnce);
            return Some(Callable { params, result, variadic, mutates, once, modes, is_unsafe });
        }
        let (_, _, info) = self.types.closure(ty)?;
        Some(Callable {
            params: info.params,
            result: info.result,
            variadic: info.variadic,
            mutates: info.mutates,
            once: info.once,
            modes: info.modes,
            is_unsafe: info.is_unsafe,
        })
    }

    /// A function's result type, where `impl A + B` (12.8) declares an
    /// opaque type with those bounds over the function's type parameters.
    pub(super) fn result_type(&mut self, result: &ast::TypeExpr, f: &ast::FnDecl, generics: Vec<Ty>) -> Ty {
        let (bounds, error) = match &result.kind {
            ast::TypeKind::Opaque(bounds) => (bounds, None),
            ast::TypeKind::Result { ok: Some(ok), err } if matches!(ok.kind, ast::TypeKind::Opaque(_)) => {
                let ast::TypeKind::Opaque(bounds) = &ok.kind else { unreachable!() };
                (bounds, Some(err))
            }
            _ => return self.resolve_type(result),
        };
        if f.owner.is_none() && self.self_type.is_some() {
            self.diags.error(
                "E0004",
                result.span,
                "trait methods and their implementations cannot return `impl Bound`",
            );
            return Ty::ERROR;
        }
        let shown: Vec<String> = bounds.iter().map(|b| self.bound_text(b)).collect();
        let name = format!("impl {}", shown.join(" + "));
        // Like a type parameter, an opaque result is `Copy` unless it says
        // `~Copy`.
        let carrier = self.types.declare_param(name.clone());
        self.types.param_mut(carrier).copy = true;
        for bound in bounds {
            self.apply_bound(carrier, bound);
        }
        let opaque = self.types.declare_opaque(name, generics, carrier);
        match error {
            Some(error) => {
                let error = self.resolve_type(error);
                self.types.intern(TyKind::Result(opaque, error))
            }
            None => opaque,
        }
    }

    /// Records the concrete type an opaque result hides: every normal path
    /// returns it and it meets the published bounds (12.8).
    pub(super) fn record_witness(&mut self, cx: &FnCx, opaque: Ty, body: &hir::Block, f: &ast::FnDecl) {
        let span = f.result.as_ref().map_or(f.name.span, |r| r.span);
        let returned = cx.result;
        let witness = match (returned, body.ty) {
            (Ty::ERROR, tail) => tail,
            (returned, Ty::NEVER) => returned,
            (returned, tail) if returned == tail || self.types.has_error(tail) => returned,
            (returned, tail) => {
                let message = format!(
                    "every path of an `impl` result returns one type, but this returns {} and {}",
                    self.display(returned),
                    self.display(tail)
                );
                self.diags.error("E1117", span, message);
                returned
            }
        };
        if witness == Ty::NEVER {
            self.diags.error("E1117", span, "an `impl` result needs a path that returns a value; use `Never` instead");
        }
        let carrier = self.types.opaque(opaque).unwrap().0.carrier;
        if !self.types.has_error(witness)
            && witness != Ty::NEVER
            && let Some(missing) = self.bound_holds(carrier, witness, &HashMap::new(), 0)
        {
            let message =
                format!("{} does not implement {missing}, which the `impl` result publishes", self.display(witness));
            self.diags.error("E1117", span, message);
        }
        self.types.set_witness(opaque, witness);
    }

    /// The source text of a bound, for the name of an opaque type.
    pub(super) fn bound_text(&self, bound: &ast::TypeExpr) -> String {
        self.sources.file_of(bound.span.start).slice(bound.span).to_string()
    }

    /// The traits bounding a type parameter or an opaque result, with
    /// arguments for the opaque result's type arguments.
    pub(super) fn bounds_of(&mut self, ty: Ty) -> Vec<(TraitId, Vec<Ty>)> {
        if let Some((def, args)) = self.types.opaque(ty) {
            let map: HashMap<Ty, Ty> = def.generics.iter().copied().zip(args.iter().copied()).collect();
            let bounds = self.bounds.get(&def.carrier).cloned().unwrap_or_default();
            return bounds
                .into_iter()
                .map(|(t, a)| (t, a.into_iter().map(|x| self.types.subst(x, &map)).collect()))
                .collect();
        }
        let direct = self.bounds.get(&ty).cloned().unwrap_or_default();
        self.with_required(direct)
    }

    /// The call bound of a type parameter or an opaque result.
    pub(super) fn call_bound_of(&mut self, ty: Ty) -> Option<(Protocol, Ty)> {
        if let Some((def, args)) = self.types.opaque(ty) {
            let map: HashMap<Ty, Ty> = def.generics.iter().copied().zip(args.iter().copied()).collect();
            let (protocol, signature) = self.call_bounds.get(&def.carrier).copied()?;
            return Some((protocol, self.types.subst(signature, &map)));
        }
        self.call_bounds.get(&ty).copied()
    }

    /// Whether only bounds describe `ty`: a type parameter or an opaque result.
    pub(super) fn is_abstract(&self, ty: Ty) -> bool {
        self.types.param(ty).is_some() || self.types.opaque(ty).is_some()
    }
}
