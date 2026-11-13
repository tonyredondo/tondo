//! Contracts of `std.plugin` (standard library 14.48): the text whose
//! SHA-256 is a contract's fingerprint, which `plugin.contractText` gives,
//! and what a contract may hold.

use super::*;
use crate::types::{AdtKind, Payload};

impl Checker<'_> {
    /// Whether a trait has `@plugin.Contract` before it.
    pub(super) fn is_contract(&self, decl: &ast::TraitDecl) -> bool {
        decl.uses.iter().any(|annotation| match annotation.path.as_slice() {
            [name] => matches!(self.type_names.get(&name.name), Some(&TypeName::Nominal(ty)) if self.types.display(ty) == "plugin.Contract"),
            _ => false,
        })
    }

    /// What a contract may hold (standard library 14.48, rule 2): each
    /// method takes `self`, has no type parameters, and gives `T !
    /// plugin.Error` or `T ! (E | plugin.Error)`, of types that are written
    /// and read; the trait has no type parameters and requires no trait.
    pub(super) fn check_contract(&mut self, id: TraitId, decl: &ast::TraitDecl) {
        let name = decl.name.name.rsplit('.').next().unwrap_or(&decl.name.name).to_string();
        if !decl.generics.is_empty() || !decl.requires.is_empty() {
            let message = format!(
                "the contract `{name}` has no type parameters and requires no other trait: a plugin's methods are its own, of the types written"
            );
            self.diags.error("E1119", decl.name.span, message);
            return;
        }
        let (encode, decode) = (
            self.trait_names.get("serialization.Encode").copied(),
            self.trait_names.get("serialization.Decode").copied(),
        );
        let plugin_error = match self.type_names.get("plugin.Error") {
            Some(&TypeName::Nominal(ty)) => Some(ty),
            _ => None,
        };
        let methods: Vec<(String, Signature)> =
            self.traits[id.0 as usize].methods.iter().map(|m| (m.name.clone(), m.signature.clone())).collect();
        for ((method, signature), f) in methods.iter().zip(&decl.methods) {
            let what = |wrong: String| format!("the method `{method}` of the contract `{name}` {wrong}");
            let receiver = f.params.first();
            if receiver.is_none_or(|r| r.ty.is_some()) {
                self.diags.error("E1119", f.name.span, what("takes `self`, as every method of a plugin".into()));
                continue;
            }
            if let Some(mode @ (ast::Mode::Mut | ast::Mode::Var)) = receiver.map(|r| r.mode) {
                let written = if mode == ast::Mode::Mut { "mut self" } else { "var self" };
                let message = what(format!("takes `{written}`, and a plugin is not changed by a call: take `self`"));
                self.diags.error("E1119", f.name.span, message);
                continue;
            }
            if !f.generics.is_empty() {
                self.diags.error(
                    "E1119",
                    f.name.span,
                    what("has type parameters, which do not cross to a plugin".into()),
                );
                continue;
            }
            let TyKind::Result(value, error) = *self.types.kind(signature.result) else {
                let message = what("gives no `T ! plugin.Error`: a call of a plugin can fail, so write `T ! plugin.Error`, or `T ! (E | plugin.Error)` with an error of its own".into());
                self.diags.error("E1119", f.name.span, message);
                continue;
            };
            let members = match self.types.kind(error).clone() {
                TyKind::Union(members) => members,
                _ => vec![error],
            };
            if plugin_error.is_none_or(|e| !members.contains(&e)) {
                let message = what(format!(
                    "fails with {}, without `plugin.Error`: write `T ! plugin.Error`, or `T ! (E | plugin.Error)`",
                    self.display(error)
                ));
                self.diags.error("E1119", f.name.span, message);
                continue;
            }
            let crossing = signature
                .params
                .iter()
                .skip(1)
                .copied()
                .chain([value])
                .chain(members.into_iter().filter(|&m| Some(m) != plugin_error));
            for ty in crossing.collect::<Vec<_>>() {
                let written = encode.is_some_and(|t| self.satisfies(ty, t, &[]))
                    && decode.is_some_and(|t| self.satisfies(ty, t, &[]));
                if !written && ty != Ty::UNIT {
                    let message = what(format!(
                        "takes or gives {}, which is not written and read: what crosses to a plugin implements `serialization.Encode` and `serialization.Decode`",
                        self.display(ty)
                    ));
                    self.diags.error("E1119", f.name.span, message);
                    break;
                }
            }
        }
    }

    /// The contract named `name` written out in full: its name, each method
    /// with the types of its parameters and of its result, and every record,
    /// enum and newtype those reach, with their fields, variants and types,
    /// in the order they are first met. Two builds of one contract write one
    /// text; any difference in what crosses writes another.
    pub(super) fn contract_text(&self, name: &str) -> Option<String> {
        let &id = self.trait_names.get(name)?;
        let contract = &self.traits[id.0 as usize];
        let mut out = format!("contract {}\n", contract.name);
        let mut met: Vec<Ty> = Vec::new();
        for method in &contract.methods {
            let signature = &method.signature;
            let params: Vec<String> =
                signature.params.iter().skip(1).map(|&ty| self.contract_type(ty, &mut met)).collect();
            let result = self.contract_type(signature.result, &mut met);
            out.push_str(&format!("fn {}({}): {result}\n", method.name, params.join(", ")));
        }
        let mut at = 0;
        while at < met.len() {
            let ty = met[at];
            at += 1;
            let TyKind::Adt(adt) = *self.types.kind(ty) else { continue };
            let shown = self.types.display(ty);
            let body = match &self.types.adt(adt).kind {
                AdtKind::Record(fields) => self.fields_written(fields, &mut met),
                AdtKind::Newtype(inner) => format!("({})", self.contract_type(*inner, &mut met)),
                AdtKind::Enum(variants) => {
                    let variants: Vec<String> = variants
                        .iter()
                        .map(|variant| match &variant.payload {
                            Payload::Unit => variant.name.clone(),
                            Payload::Tuple(types) => {
                                let types: Vec<String> =
                                    types.iter().map(|&t| self.contract_type(t, &mut met)).collect();
                                format!("{}({})", variant.name, types.join(", "))
                            }
                            Payload::Record(fields) => {
                                format!("{} {}", variant.name, self.fields_written(fields, &mut met))
                            }
                        })
                        .collect();
                    format!("enum {{ {} }}", variants.join(", "))
                }
            };
            out.push_str(&format!("type {shown} = {body}\n"));
        }
        Some(out)
    }

    fn fields_written(&self, fields: &[crate::types::Field], met: &mut Vec<Ty>) -> String {
        let fields: Vec<String> =
            fields.iter().map(|f| format!("{}: {}", f.name, self.contract_type(f.ty, met))).collect();
        format!("{{ {} }}", fields.join(", "))
    }

    /// A type as the contract's text writes it, by its name, keeping the
    /// records, enums and newtypes in it to write out after.
    fn contract_type(&self, ty: Ty, met: &mut Vec<Ty>) -> String {
        self.reached_types(ty, met);
        self.types.display(ty)
    }

    fn reached_types(&self, ty: Ty, met: &mut Vec<Ty>) {
        match self.types.kind(ty).clone() {
            TyKind::Adt(_) => {
                if !met.contains(&ty) {
                    met.push(ty);
                }
            }
            TyKind::Array(inner) | TyKind::Option(inner) | TyKind::Set(inner) | TyKind::Deque(inner) => {
                self.reached_types(inner, met)
            }
            TyKind::Map(key, value) | TyKind::Result(key, value) => {
                self.reached_types(key, met);
                self.reached_types(value, met);
            }
            TyKind::Tuple(members) | TyKind::Union(members) => {
                members.into_iter().for_each(|member| self.reached_types(member, met))
            }
            _ => {}
        }
    }
}
