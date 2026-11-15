//! Interfaces of C (language 16.16): a trait with `@ffi.Interface`, whose
//! methods are the entries of a table of functions, in the order written,
//! after those of the interface it requires; `interfaces.rs` writes the
//! object that calls them for one that holds to these rules.

use super::*;

impl<'a> Checker<'a> {
    /// Whether a trait has `@ffi.Interface` before it.
    pub(super) fn is_interface(&self, decl: &ast::TraitDecl) -> bool {
        self.interface_use(decl).is_some()
    }

    fn interface_use<'d>(&self, decl: &'d ast::TraitDecl) -> Option<&'d ast::AnnotationUse> {
        decl.uses.iter().find(|annotation| match annotation.path.as_slice() {
            [name] => matches!(self.type_names.get(&name.name), Some(&TypeName::Nominal(ty)) if self.types.display(ty) == "ffi.Interface"),
            _ => false,
        })
    }

    /// What every interface may be (16.16, rule 1): no type parameters, at
    /// most one interface it requires, its identity a GUID, which one that
    /// reaches `ffi.Unknown` has, and methods that take `self` and values
    /// that cross to C, and give one, nothing, or `ffi.HResult` as a
    /// failure.
    pub(super) fn check_interfaces(&mut self, items: &'a [ast::Item]) {
        let found: Vec<(TraitId, &ast::TraitDecl)> = items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Trait(decl) if self.is_interface(decl) => {
                    self.trait_names.get(&decl.name.name).map(|&id| (id, decl))
                }
                _ => None,
            })
            .collect();
        let interfaces: HashSet<TraitId> = found.iter().map(|(id, _)| *id).collect();
        for (id, decl) in found {
            self.check_interface(id, decl, &interfaces);
        }
    }

    fn check_interface(&mut self, id: TraitId, decl: &ast::TraitDecl, interfaces: &HashSet<TraitId>) {
        let name = decl.name.name.rsplit('.').next().unwrap_or(&decl.name.name).to_string();
        if !decl.generics.is_empty() {
            let message =
                format!("the interface `{name}` has no type parameters: its table is C's, with the types it writes");
            self.diags.error("E1707", decl.name.span, message);
            return;
        }
        let requires = self.traits[id.0 as usize].requires.clone();
        match requires.as_slice() {
            [] => {}
            [(base, _)] if interfaces.contains(base) => {}
            [(base, _)] => {
                let base = self.traits[base.0 as usize].name.clone();
                let message = format!(
                    "the interface `{name}` requires `{base}`, which is not an interface: the one it requires has `@ffi.Interface` too, and its methods come first in the table"
                );
                self.diags.error("E1707", decl.name.span, message);
            }
            _ => {
                let message = format!(
                    "the interface `{name}` requires more than one trait: it requires one interface at most, whose methods come first in its table"
                );
                self.diags.error("E1707", decl.name.span, message);
            }
        }
        // Its identity, a GUID, which `queryInterface` asks for: one that
        // reaches `ffi.Unknown` has one.
        let written = self.interface_use(decl).and_then(|annotation| {
            annotation.args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == "id")).map(|a| &a.value)
        });
        let id_text = written.and_then(|value| match &value.kind {
            ast::ExprKind::Str(segments) => match segments.as_slice() {
                [ast::StrSegment::Text(text)] => Some(text.clone()),
                [] => Some(String::new()),
                _ => None,
            },
            _ => None,
        });
        match (written, id_text.as_deref()) {
            (Some(value), Some(text)) if !text.is_empty() && crate::interfaces::guid(text).is_none() => {
                let message = format!(
                    "`{text}` is not a GUID: an interface's identity is written `176FBED1-A55C-4796-98CA-A9DA0EF883E7`, as its header gives it"
                );
                self.diags.error("E1708", value.span, message);
            }
            (_, None | Some("")) if self.reaches_unknown(id) => {
                let message = format!(
                    "the interface `{name}` requires `ffi.Unknown`, so `queryInterface` asks for it by its identity: write it, `@ffi.Interface(id: \"...\")`, as its header gives it"
                );
                self.diags.error("E1707", decl.name.span, message);
            }
            _ => {}
        }
        let methods: Vec<Signature> = self.traits[id.0 as usize].methods.iter().map(|m| m.signature.clone()).collect();
        for (signature, f) in methods.iter().zip(&decl.methods) {
            let method = &f.name.name;
            let what = |wrong: String| format!("the method `{method}` of the interface `{name}` {wrong}");
            let receiver = f.params.first();
            if receiver.is_none_or(|r| r.ty.is_some() || r.mode != ast::Mode::Ref) {
                let message = what("takes `self`, the object that C passes first to each of its functions".into());
                self.diags.error("E1707", f.name.span, message);
                continue;
            }
            if !f.generics.is_empty() {
                self.diags.error("E1707", f.name.span, what("has type parameters, which C has none of".into()));
                continue;
            }
            if let Some(param) = f.params.iter().skip(1).find(|p| p.mode != ast::Mode::Value || p.variadic) {
                let message =
                    what(format!("takes `{}` as C cannot pass it: C takes its arguments by value", param.name.name));
                self.diags.error("E1707", param.span, message);
                continue;
            }
            let crossing = signature.params.iter().skip(1).copied().zip(f.params.iter().skip(1));
            let mut wrong = None;
            for (ty, param) in crossing {
                if let Some(reason) = self.c_type_error(ty) {
                    wrong =
                        Some((param.span, format!("takes {}, which does not cross to C: {reason}", self.display(ty))));
                    break;
                }
            }
            let result = signature.result;
            let gives = match *self.types.kind(result) {
                TyKind::Result(_, error) if !self.is_hresult(error) => Some(format!(
                    "fails with {}: a method of an interface fails with `ffi.HResult`, as `!ffi.HResult` or `T ! ffi.HResult`",
                    self.display(error)
                )),
                TyKind::Result(value, _) if value != Ty::UNIT => self
                    .c_type_error(value)
                    .map(|reason| format!("gives {}, which does not cross to C: {reason}", self.display(value))),
                TyKind::Result(..) => None,
                _ if result == Ty::UNIT => None,
                _ => self
                    .c_type_error(result)
                    .map(|reason| format!("gives {}, which does not cross to C: {reason}", self.display(result))),
            };
            let span = f.result.as_ref().map_or(f.name.span, |r| r.span);
            if let Some((span, message)) = wrong.or(gives.map(|message| (span, message))) {
                self.diags.error("E1707", span, what(message));
            }
        }
    }

    /// Whether an interface requires `ffi.Unknown`, itself or through the
    /// one it requires.
    fn reaches_unknown(&self, id: TraitId) -> bool {
        let mut at = Some(id);
        let mut steps = 0;
        while let Some(trait_id) = at {
            if self.traits[trait_id.0 as usize].name == "ffi.Unknown" {
                return true;
            }
            at = self.traits[trait_id.0 as usize].requires.first().map(|(base, _)| *base);
            steps += 1;
            if steps > 64 {
                return false;
            }
        }
        false
    }

    /// Whether `ty` is `std.ffi.HResult`.
    pub(super) fn is_hresult(&self, ty: Ty) -> bool {
        self.types.display(ty) == "ffi.HResult"
    }
}
