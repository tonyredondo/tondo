//! What the checker records for an editor (`crate::index`): each
//! declaration, and each name that refers to one, when it is asked to.

use super::{Checker, FnCx};
use crate::ast;
use crate::index::{Def, DefKind, Key, Member, Shape};
use crate::source::Span;
use crate::types::AdtKind;
use crate::types::{Ty, TyKind};
use std::collections::HashMap;

impl Checker<'_> {
    /// The identifier a name ends with, and the module alias written before
    /// it: the linker joins `models.User` into one name, which its own
    /// module writes `User`.
    fn name_parts(name: &ast::Ident) -> (Option<Span>, Span) {
        let simple = name.name.rsplit('.').next().unwrap_or(&name.name);
        let (span, length) = (name.span, simple.len() as u32);
        if span.end - span.start <= length {
            return (None, span);
        }
        let member = Span::new(span.end - length, span.end);
        (Some(Span::new(span.start, member.start - 1)), member)
    }

    /// Records a declaration, of a binding with its type.
    pub(super) fn index_define(
        &mut self,
        key: Option<Key>,
        kind: DefKind,
        name: &ast::Ident,
        decl: Span,
        ty: Option<Ty>,
    ) -> Option<usize> {
        self.index.as_ref()?;
        let ty = ty.filter(|&ty| ty != Ty::ERROR).map(|ty| self.types.display(ty));
        let (_, name_span) = Self::name_parts(name);
        let simple = name.name.rsplit('.').next().unwrap_or_default().to_string();
        let index = self.index.as_mut()?;
        let def = index.define(key, Def { name: simple, kind, name_span, decl_span: decl, ty: ty.clone() });
        if index.defs[def].ty.is_none() {
            index.defs[def].ty = ty;
        }
        Some(def)
    }

    /// Records that `name` refers to the declaration of `key`, and that the
    /// module alias written before it refers to its module.
    pub(super) fn index_refer(&mut self, name: &ast::Ident, key: Key) {
        let Some(index) = self.index.as_mut() else { return };
        let Some(def) = index.of(&key) else { return };
        let (alias, member) = Self::name_parts(name);
        index.refer(member, def);
        let Some(alias) = alias else { return };
        let file = self.sources.file_of(index.defs[def].decl_span.start);
        let module = Def {
            name: self.sources.slice(alias).to_string(),
            kind: DefKind::Module,
            name_span: Span::default(),
            decl_span: Span::new(file.base, file.base),
            ty: None,
        };
        let module = index.define(Some(Key::Module(file.module.clone())), module);
        index.refer(alias, module);
    }

    /// Records that `name` refers to a binding of the function, or of one
    /// it is nested in.
    pub(super) fn index_local(&mut self, cx: &FnCx, name: &ast::Ident) {
        if self.index.is_none() {
            return;
        }
        if let Some(def) = self.local_def(cx, &name.name)
            && let Some(index) = self.index.as_mut()
        {
            index.refer(name.span, def);
        }
    }

    fn local_def(&self, cx: &FnCx, name: &str) -> Option<usize> {
        match self.lookup(cx, name) {
            Some(local) => cx.local_defs.get(&local.0).copied(),
            None => self.local_def(cx.parent.as_deref()?, name),
        }
    }

    /// The name the program declares a nominal type under.
    pub(super) fn index_type_name(&self, ty: Ty) -> Option<String> {
        if let Some((template, _)) = self.types.instance_of(ty) {
            return Some(self.types.template(template).name.clone());
        }
        match self.types.kind(ty) {
            TyKind::Adt(id) => Some(self.types.adt(*id).name.clone()),
            _ => None,
        }
    }

    /// The declarations of a type and of its fields or variants.
    pub(super) fn index_type(&mut self, item: &ast::Item) {
        if self.index.is_none() {
            return;
        }
        match item {
            ast::Item::Type(decl) => {
                let key = Some(Key::Type(decl.name.name.clone()));
                self.index_define(key, DefKind::Type, &decl.name, decl.span, None);
                if let ast::TypeBody::Record(fields) = &decl.body {
                    for field in fields {
                        let key = Some(Key::Field(decl.name.name.clone(), field.name.name.clone()));
                        let span = field.name.span.to(field.ty.span);
                        self.index_define(key, DefKind::Field, &field.name, span, None);
                    }
                }
            }
            ast::Item::Enum(decl) => {
                let key = Some(Key::Type(decl.name.name.clone()));
                self.index_define(key, DefKind::Enum, &decl.name, decl.span, None);
                for variant in &decl.variants {
                    let key = Some(Key::Variant(decl.name.name.clone(), variant.name.name.clone()));
                    self.index_define(key, DefKind::Variant, &variant.name, variant.name.span, None);
                    if let ast::VariantPayload::Record(fields) = &variant.payload {
                        let owner = format!("{}.{}", decl.name.name, variant.name.name);
                        for field in fields {
                            let key = Some(Key::Field(owner.clone(), field.name.name.clone()));
                            let span = field.name.span.to(field.ty.span);
                            self.index_define(key, DefKind::Field, &field.name, span, None);
                        }
                    }
                }
            }
            ast::Item::Alias(decl) => {
                let key = Some(Key::Type(decl.name.name.clone()));
                self.index_define(key, DefKind::Type, &decl.name, decl.span, None);
            }
            _ => {}
        }
    }

    /// The declarations of the functions with a body, by `FnId`, and of
    /// their parameters.
    pub(super) fn index_functions(&mut self, bodies: &[&ast::FnDecl]) {
        if self.index.is_none() {
            return;
        }
        for (id, f) in bodies.iter().enumerate() {
            let method = self.signatures.get(id).is_some_and(|s| s.receiver.is_some()) || f.owner.is_some();
            let kind = if method { DefKind::Method } else { DefKind::Function };
            self.index_define(Some(Key::Fn(id as u32)), kind, &f.name, f.span, None);
            for param in &f.params {
                let key = Some(Key::Param(id as u32, param.name.name.clone()));
                self.index_define(key, DefKind::Parameter, &param.name, param.span, None);
            }
        }
    }

    /// Records the fields written in a literal or a pattern of `owner`, a
    /// type or `Enum.Variant`, and which of them are shorthands.
    pub(super) fn index_fields<'f>(
        &mut self,
        owner: Option<String>,
        fields: impl Iterator<Item = (&'f ast::Ident, bool)>,
    ) {
        let (Some(owner), Some(_)) = (owner, self.index.as_ref()) else { return };
        for (name, shorthand) in fields {
            self.index_refer(name, Key::Field(owner.clone(), name.name.clone()));
            if shorthand && let Some(index) = self.index.as_mut() {
                index.shorthands.push(name.span);
            }
        }
    }

    /// The declarations of a trait and of its methods.
    pub(super) fn index_trait(&mut self, id: u32, decl: &ast::TraitDecl) {
        if self.index.is_none() {
            return;
        }
        self.index_define(Some(Key::Trait(id)), DefKind::Trait, &decl.name, decl.span, None);
        for method in &decl.methods {
            let key = Some(Key::TraitMethod(id, method.name.name.clone()));
            self.index_define(key, DefKind::Method, &method.name, method.span, None);
        }
    }

    /// An implementation's method is its trait's: calls of either, and its
    /// own name, refer to the trait's method.
    pub(super) fn index_implemented(&mut self, trait_id: u32, id: u32, name: &ast::Ident) {
        let Some(index) = self.index.as_mut() else { return };
        let Some(def) = index.of(&Key::TraitMethod(trait_id, name.name.clone())) else { return };
        index.alias(Key::Fn(id), def);
        index.refer(name.span, def);
        index.implemented.push((def, name.span));
    }

    /// The types of the expressions, with the members of each.
    pub(super) fn index_finish(&mut self) {
        if self.index.is_none() || self.index_exprs.is_empty() {
            return;
        }
        let mut shapes: HashMap<Ty, usize> = HashMap::new();
        let mut types = Vec::new();
        let mut found = Vec::new();
        for (span, ty) in std::mem::take(&mut self.index_exprs) {
            let shape = match shapes.get(&ty) {
                Some(&shape) => shape,
                None => {
                    let shape = Shape { name: self.types.display(ty), members: self.index_members(ty) };
                    found.push(shape);
                    shapes.insert(ty, found.len() - 1);
                    found.len() - 1
                }
            };
            types.push((span, shape));
        }
        if let Some(index) = self.index.as_mut() {
            index.types = types;
            index.shapes = found;
        }
    }

    /// The fields and methods of a value of type `ty`.
    fn index_members(&mut self, ty: Ty) -> Vec<Member> {
        let mut members: Vec<Member> = Vec::new();
        let owner = self.index_type_name(ty);
        let index = self.index.as_ref();
        if let TyKind::Adt(id) = self.types.kind(ty) {
            match &self.types.adt(*id).kind {
                AdtKind::Record(fields) => {
                    for field in fields {
                        let key = owner.clone().map(|o| Key::Field(o, field.name.clone()));
                        members.push(Member {
                            name: field.name.clone(),
                            method: false,
                            ty: Some(self.types.display(field.ty)),
                            def: key.and_then(|key| index?.of(&key)),
                            builtin: None,
                            private: field.private,
                        });
                    }
                }
                AdtKind::Newtype(inner) => members.push(Member {
                    name: "value".into(),
                    method: false,
                    ty: Some(self.types.display(*inner)),
                    def: None,
                    builtin: None,
                    private: false,
                }),
                AdtKind::Enum(_) => {}
            }
        }
        // The methods a value has: those that take `self`, not its type's
        // other functions, as `Builder.new`.
        let key = self.owner_key(ty);
        let mut methods: Vec<(String, u32)> = self
            .methods
            .iter()
            .filter(|((owner, _), id)| {
                *owner == key && self.signatures.get(id.0 as usize).is_some_and(|s| s.receiver.is_some())
            })
            .map(|((_, name), id)| (name.clone(), id.0))
            .collect();
        methods.sort();
        for (name, id) in methods {
            let def = self.index.as_ref().and_then(|index| index.of(&Key::Fn(id)));
            members.push(Member { name, method: true, ty: None, def, builtin: None, private: false });
        }
        // What the runtime implements, as `tondo api` documents it.
        let shown = self.types.display(ty);
        let intrinsic = shown.split('[').next().unwrap_or_default().to_string();
        for name in crate::api::builtin_methods(&intrinsic) {
            let on_values = crate::api::builtin_doc(&intrinsic, name).is_some_and(|(d, _)| crate::api::takes_self(&d));
            if on_values && !members.iter().any(|m| m.name == name) {
                let builtin = Some(intrinsic.clone());
                members.push(Member {
                    name: name.to_string(),
                    method: true,
                    ty: None,
                    def: None,
                    builtin,
                    private: false,
                });
            }
        }
        for implementation in &self.impls {
            if implementation.target != key && implementation.target != ty {
                continue;
            }
            let id = implementation.trait_id;
            for method in &self.traits[id.0 as usize].methods {
                if method.signature.receiver.is_some() && !members.iter().any(|m| m.name == method.name) {
                    let def = self.index.as_ref().and_then(|i| i.of(&Key::TraitMethod(id.0, method.name.clone())));
                    let member = Member {
                        name: method.name.clone(),
                        method: true,
                        ty: None,
                        def,
                        builtin: None,
                        private: false,
                    };
                    members.push(member);
                }
            }
        }
        members
    }

    /// Records the field `name` of a value of type `owner`.
    pub(super) fn index_field(&mut self, owner: Ty, name: &str, span: Span) {
        if self.index.is_none() {
            return;
        }
        if let Some(owner) = self.index_type_name(owner) {
            let field = ast::Ident { name: name.to_string(), span };
            self.index_refer(&field, Key::Field(owner, name.to_string()));
        }
    }
}
