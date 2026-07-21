use std::collections::BTreeMap;

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{ModuleId, Name, Namespace, PackageGraph};
use crate::source::{FileId, SourceDatabase, Span};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};

use super::{
    Member, MemberId, MemberKind, MemberName, MemberOwner, ResolveError, ResolvedProgram, SymbolId,
    SymbolKind, Visibility, normalized_name,
};

#[derive(Debug, Clone)]
struct Candidate {
    owner: MemberOwner,
    name: MemberName,
    kind: MemberKind,
    visibility: Visibility,
    span: Span,
    generic_arity: u32,
    synthetic: bool,
}

#[derive(Debug, Clone, Copy)]
struct PendingVariant<'syntax> {
    file: FileId,
    node: SyntaxNodeRef<'syntax>,
    visibility: Visibility,
}

pub(super) fn collect_members<'a>(
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: &BTreeMap<FileId, &'a Parsed>,
    ordered_files: &[FileId],
    program: &mut ResolvedProgram,
    diagnostics: &mut Vec<Diagnostic>,
    max_diagnostics: usize,
) -> Result<(), ResolveError> {
    let mut collector = MemberCollector {
        packages,
        sources,
        parsed,
        program,
        diagnostics,
        max_diagnostics,
    };
    collector.collect(ordered_files)
}

struct MemberCollector<'a> {
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: &'a BTreeMap<FileId, &'a Parsed>,
    program: &'a mut ResolvedProgram,
    diagnostics: &'a mut Vec<Diagnostic>,
    max_diagnostics: usize,
}

impl<'a> MemberCollector<'a> {
    fn collect(&mut self, ordered_files: &[FileId]) -> Result<(), ResolveError> {
        let mut candidates = Vec::new();
        let mut pending_variants = Vec::new();
        for file in ordered_files {
            let module = self.packages.module_for_file(self.sources, *file)?;
            let root = self.parsed[file].cst().root_node();
            for declaration in root.child_nodes() {
                match declaration.kind() {
                    SyntaxKind::TypeDecl => {
                        self.collect_type(*file, &module, declaration, &mut candidates)?
                    }
                    SyntaxKind::EnumDecl => self.collect_enum(
                        *file,
                        &module,
                        declaration,
                        &mut candidates,
                        &mut pending_variants,
                    )?,
                    SyntaxKind::TraitDecl => {
                        self.collect_trait(*file, &module, declaration, &mut candidates)?
                    }
                    SyntaxKind::FunctionDecl => {
                        self.collect_inherent(*file, &module, declaration, &mut candidates)?
                    }
                    SyntaxKind::ImplDecl => self.check_impl_method_names(*file, declaration)?,
                    _ => {}
                }
            }
        }

        self.sort_candidates(&mut candidates);
        let mut variant_ids = BTreeMap::new();
        for candidate in candidates {
            let origin = (candidate.span.file(), candidate.span.range().start());
            let kind = candidate.kind;
            let id = self.install(candidate);
            if kind == MemberKind::EnumVariant {
                variant_ids.insert(origin, id);
            }
        }

        let mut nested = Vec::new();
        for variant in pending_variants {
            let Some(name) = first_identifier(variant.node) else {
                continue;
            };
            let key = (variant.file, name.range().start());
            let Some(owner) = variant_ids.get(&key).copied() else {
                continue;
            };
            let Some(body) = variant
                .node
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::RecordBody)
            else {
                continue;
            };
            for field in body
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::RecordField)
            {
                if let Some(candidate) = self.field_candidate(
                    variant.file,
                    MemberOwner::Variant(owner),
                    MemberKind::VariantField,
                    variant.visibility,
                    false,
                    field,
                )? {
                    nested.push(candidate);
                }
            }
        }
        self.sort_candidates(&mut nested);
        for candidate in nested {
            self.install(candidate);
        }

        self.validate_conflicts()
    }

    fn collect_type(
        &mut self,
        file: FileId,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'a>,
        candidates: &mut Vec<Candidate>,
    ) -> Result<(), ResolveError> {
        let Some(owner) = self.declaration_owner(module, declaration) else {
            return Ok(());
        };
        let visibility = self.symbol_visibility(owner);
        if let Some(body) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::RecordBody)
        {
            for field in body
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::RecordField)
            {
                if let Some(candidate) = self.field_candidate(
                    file,
                    MemberOwner::Type(owner),
                    MemberKind::RecordField,
                    visibility,
                    true,
                    field,
                )? {
                    candidates.push(candidate);
                }
            }
        } else {
            candidates.push(Candidate {
                owner: MemberOwner::Type(owner),
                name: MemberName::new("value").expect("`value` is a valid member name"),
                kind: MemberKind::NewtypeValue,
                visibility,
                span: self
                    .program
                    .symbol(owner)
                    .expect("member owners are valid symbols")
                    .span(),
                generic_arity: 0,
                synthetic: true,
            });
        }
        Ok(())
    }

    fn collect_enum(
        &mut self,
        file: FileId,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'a>,
        candidates: &mut Vec<Candidate>,
        pending: &mut Vec<PendingVariant<'a>>,
    ) -> Result<(), ResolveError> {
        let Some(owner) = self.declaration_owner(module, declaration) else {
            return Ok(());
        };
        let visibility = self.symbol_visibility(owner);
        for variant in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::EnumVariant)
        {
            let Some(token) = first_identifier(variant) else {
                continue;
            };
            let Some(name) = self.member_name(file, token)? else {
                continue;
            };
            candidates.push(Candidate {
                owner: MemberOwner::Type(owner),
                name,
                kind: MemberKind::EnumVariant,
                visibility,
                span: self.sources.span(file, token.range())?,
                generic_arity: 0,
                synthetic: false,
            });
            pending.push(PendingVariant {
                file,
                node: variant,
                visibility,
            });
        }
        Ok(())
    }

    fn collect_trait(
        &mut self,
        file: FileId,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'a>,
        candidates: &mut Vec<Candidate>,
    ) -> Result<(), ResolveError> {
        let Some(owner) = self.declaration_owner(module, declaration) else {
            return Ok(());
        };
        let visibility = self.symbol_visibility(owner);
        for method in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::TraitMethod)
        {
            let Some(token) = first_identifier(method) else {
                continue;
            };
            let Some(name) = self.member_name(file, token)? else {
                continue;
            };
            let receiver = has_receiver(method);
            candidates.push(Candidate {
                owner: MemberOwner::Type(owner),
                name,
                kind: if receiver {
                    MemberKind::TraitMethod
                } else {
                    MemberKind::TraitAssociatedFunction
                },
                visibility,
                span: self.sources.span(file, token.range())?,
                generic_arity: generic_arity(method),
                synthetic: false,
            });
        }
        Ok(())
    }

    fn collect_inherent(
        &mut self,
        file: FileId,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'a>,
        candidates: &mut Vec<Candidate>,
    ) -> Result<(), ResolveError> {
        let Some(head) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::FunctionHead)
        else {
            return Ok(());
        };
        let names = head
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        if names.len() != 2 {
            return Ok(());
        }
        let owner_name = normalized_name(names[0]).expect("method owners are ordinary names");
        let owner = self
            .program
            .module(module)
            .and_then(|resolved| resolved.lookup(Namespace::Type, &owner_name));
        let Some(owner) = owner else {
            if is_intrinsic_type_name(&owner_name)
                || self
                    .program
                    .file(file)
                    .is_some_and(|resolution| resolution.imports().contains_key(&owner_name))
            {
                self.emit(
                    self.sources.span(file, names[0].range())?,
                    "E1504",
                    format!(
                        "module `{}` cannot declare inherent methods for `{owner_name}`",
                        module.path()
                    ),
                    None,
                )?;
            }
            return Ok(());
        };
        let symbol = self
            .program
            .symbol(owner)
            .expect("module symbol tables contain valid IDs");
        if !matches!(symbol.kind(), SymbolKind::Type | SymbolKind::Enum) {
            self.emit(
                self.sources.span(file, names[0].range())?,
                "E1504",
                format!("`{owner_name}` is not a nominal record, newtype, or enum"),
                Some(("owner declared here", symbol.span())),
            )?;
            return Ok(());
        }

        let visibility = declaration_visibility(declaration);
        if visibility == Visibility::Public && symbol.visibility() == Visibility::Private {
            self.emit(
                self.sources.span(file, names[1].range())?,
                "E1503",
                format!("public method exposes private owner `{owner_name}`"),
                Some(("private owner declared here", symbol.span())),
            )?;
        }
        let Some(name) = self.member_name(file, names[1])? else {
            return Ok(());
        };
        let receiver = has_receiver(declaration);
        candidates.push(Candidate {
            owner: MemberOwner::Type(owner),
            name,
            kind: if receiver {
                MemberKind::InherentMethod
            } else {
                MemberKind::AssociatedFunction
            },
            visibility,
            span: self.sources.span(file, names[1].range())?,
            generic_arity: method_generic_arity(head, names[1]),
            synthetic: false,
        });
        Ok(())
    }

    fn check_impl_method_names(
        &mut self,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), ResolveError> {
        let mut seen = BTreeMap::<Name, Span>::new();
        for method in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::ImplementationMethod)
        {
            let Some(token) = first_identifier(method) else {
                continue;
            };
            let name = normalized_name(token).expect("implementation method names are identifiers");
            let span = self.sources.span(file, token.range())?;
            if let Some(previous) = seen.get(&name).copied() {
                self.emit(
                    span,
                    "E1002",
                    format!("implementation method `{name}` is declared more than once"),
                    Some(("first implementation method", previous)),
                )?;
            } else {
                seen.insert(name, span);
            }
        }
        Ok(())
    }

    fn field_candidate(
        &mut self,
        file: FileId,
        owner: MemberOwner,
        kind: MemberKind,
        owner_visibility: Visibility,
        allow_private_modifier: bool,
        field: SyntaxNodeRef<'a>,
    ) -> Result<Option<Candidate>, ResolveError> {
        let Some(name_token) = field_name_token(field) else {
            return Ok(None);
        };
        let Some(name) = self.member_name(file, name_token)? else {
            return Ok(None);
        };
        let private_token = allow_private_modifier
            .then(|| {
                field.child_tokens().find(|token| {
                    token.kind() == TokenKind::Priv && token.range() != name_token.range()
                })
            })
            .flatten();
        if owner_visibility == Visibility::Private
            && let Some(private_token) = private_token
        {
            let owner_span = match owner {
                MemberOwner::Type(symbol) => self
                    .program
                    .symbol(symbol)
                    .expect("member owners are valid symbols")
                    .span(),
                MemberOwner::Variant(_) => {
                    unreachable!("enum variant fields cannot use `priv`")
                }
            };
            self.emit(
                self.sources.span(file, private_token.range())?,
                "E1115",
                "`priv` is redundant on a field of a private type",
                Some(("private type declared here", owner_span)),
            )?;
        }
        let visibility = if owner_visibility == Visibility::Public && private_token.is_none() {
            Visibility::Public
        } else {
            Visibility::Private
        };
        Ok(Some(Candidate {
            owner,
            name,
            kind,
            visibility,
            span: self.sources.span(file, name_token.range())?,
            generic_arity: 0,
            synthetic: false,
        }))
    }

    fn declaration_owner(
        &self,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'_>,
    ) -> Option<SymbolId> {
        let token = first_identifier(declaration)?;
        let name = normalized_name(token).ok()?;
        self.program.module(module)?.lookup(Namespace::Type, &name)
    }

    fn symbol_visibility(&self, symbol: SymbolId) -> Visibility {
        self.program
            .symbol(symbol)
            .expect("member owners are valid symbols")
            .visibility()
    }

    fn member_name(
        &mut self,
        file: FileId,
        token: SyntaxTokenRef<'_>,
    ) -> Result<Option<MemberName>, ResolveError> {
        let spelling = if token.kind() == TokenKind::Identifier {
            token
                .token()
                .normalized_identifier()
                .expect("identifier tokens carry a normalized spelling")
                .to_owned()
        } else {
            let source = self.sources.get(file)?;
            let range = token.range();
            std::str::from_utf8(&source.bytes()[range.start() as usize..range.end() as usize])
                .expect("keywords are valid UTF-8")
                .to_owned()
        };
        match MemberName::new(spelling) {
            Ok(name) => Ok(Some(name)),
            Err(_) => {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1005",
                    "`_` cannot name a field or member",
                    None,
                )?;
                Ok(None)
            }
        }
    }

    fn sort_candidates(&self, candidates: &mut [Candidate]) {
        candidates.sort_by_key(|candidate| {
            let source = self
                .sources
                .get(candidate.span.file())
                .expect("candidate files belong to the source database");
            (
                candidate.owner,
                candidate.name.clone(),
                candidate.kind,
                source.source_id().as_str().to_owned(),
                source.module().as_str().to_owned(),
                source.path().as_str().to_owned(),
                candidate.span.range().start(),
            )
        });
    }

    fn install(&mut self, candidate: Candidate) -> MemberId {
        let id = MemberId(
            u32::try_from(self.program.members.len())
                .expect("member count is bounded by syntax nodes"),
        );
        self.program.members.push(Member {
            id,
            owner: candidate.owner,
            name: candidate.name.clone(),
            kind: candidate.kind,
            visibility: candidate.visibility,
            span: candidate.span,
            generic_arity: candidate.generic_arity,
            synthetic: candidate.synthetic,
        });
        self.program
            .members_by_owner
            .entry((candidate.owner, candidate.name))
            .or_default()
            .push(id);
        id
    }

    fn validate_conflicts(&mut self) -> Result<(), ResolveError> {
        let groups = self
            .program
            .members_by_owner
            .iter()
            .map(|(key, members)| (key.clone(), members.clone()))
            .collect::<Vec<_>>();
        for ((_, name), ids) in groups {
            let members = ids
                .iter()
                .map(|id| {
                    self.program
                        .member(*id)
                        .expect("member indexes contain valid IDs")
                        .clone()
                })
                .collect::<Vec<_>>();
            self.reject_duplicates(&name, &members, |kind| kind.is_field())?;
            self.reject_duplicates(&name, &members, |kind| kind == MemberKind::EnumVariant)?;
            self.reject_duplicates(&name, &members, MemberKind::is_callable)?;

            let field = members.iter().find(|member| member.kind().is_field());
            let receiver_method = members.iter().find(|member| member.kind().is_method());
            if let (Some(field), Some(method)) = (field, receiver_method) {
                self.member_conflict(
                    &name,
                    method.span(),
                    "a receiver method cannot share a name with a field",
                    field.span(),
                )?;
            }
            let variant = members
                .iter()
                .find(|member| member.kind() == MemberKind::EnumVariant);
            let callable = members.iter().find(|member| {
                matches!(
                    member.kind(),
                    MemberKind::InherentMethod | MemberKind::AssociatedFunction
                )
            });
            if let (Some(variant), Some(callable)) = (variant, callable) {
                self.member_conflict(
                    &name,
                    callable.span(),
                    "an inherent operation cannot share a name with an enum variant",
                    variant.span(),
                )?;
            }
        }
        Ok(())
    }

    fn reject_duplicates(
        &mut self,
        name: &MemberName,
        members: &[Member],
        predicate: impl Fn(MemberKind) -> bool,
    ) -> Result<(), ResolveError> {
        let matching = members
            .iter()
            .filter(|member| predicate(member.kind()))
            .collect::<Vec<_>>();
        let Some(first) = matching.first() else {
            return Ok(());
        };
        for duplicate in matching.iter().skip(1) {
            self.member_conflict(
                name,
                duplicate.span(),
                "member is declared more than once for this owner",
                first.span(),
            )?;
        }
        Ok(())
    }

    fn member_conflict(
        &mut self,
        name: &MemberName,
        primary: Span,
        reason: &str,
        previous: Span,
    ) -> Result<(), ResolveError> {
        self.emit(
            primary,
            "E1505",
            format!("member `{name}` conflicts: {reason}"),
            Some(("conflicting member declared here", previous)),
        )
    }

    fn emit(
        &mut self,
        span: Span,
        code: &str,
        message: impl Into<String>,
        related: Option<(&str, Span)>,
    ) -> Result<(), ResolveError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(ResolveError::DiagnosticLimit {
                file: span.file(),
                offset: span.range().start(),
            });
        }
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(span),
        )?;
        if let Some((message, related_span)) = related {
            diagnostic = diagnostic.with_related(Related::new(message, related_span)?);
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }
}

fn first_identifier(node: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    node.descendant_tokens()
        .find(|token| token.kind() == TokenKind::Identifier)
}

fn field_name_token(field: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    let mut candidate = None;
    for token in field.child_tokens() {
        if token.kind() == TokenKind::Colon {
            break;
        }
        if token.kind() == TokenKind::Identifier || token.kind().is_keyword() {
            candidate = Some(token);
        }
    }
    candidate
}

fn declaration_visibility(node: SyntaxNodeRef<'_>) -> Visibility {
    if node
        .child_nodes()
        .any(|child| child.kind() == SyntaxKind::Visibility)
    {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

fn has_receiver(node: SyntaxNodeRef<'_>) -> bool {
    node.child_nodes()
        .find(|child| child.kind() == SyntaxKind::ParameterList)
        .and_then(|parameters| {
            parameters
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::Parameter)
        })
        .is_some_and(|parameter| {
            parameter
                .child_tokens()
                .any(|token| token.kind() == TokenKind::SelfKw)
        })
}

fn generic_arity(node: SyntaxNodeRef<'_>) -> u32 {
    node.child_nodes()
        .filter(|child| child.kind() == SyntaxKind::GenericParams)
        .flat_map(SyntaxNodeRef::child_nodes)
        .filter(|child| child.kind() == SyntaxKind::GenericParam)
        .count()
        .try_into()
        .expect("generic parameter count is bounded by syntax nodes")
}

fn method_generic_arity(head: SyntaxNodeRef<'_>, method_name: SyntaxTokenRef<'_>) -> u32 {
    head.child_nodes()
        .filter(|child| {
            child.kind() == SyntaxKind::GenericParams
                && child.range().start() >= method_name.range().end()
        })
        .flat_map(SyntaxNodeRef::child_nodes)
        .filter(|child| child.kind() == SyntaxKind::GenericParam)
        .count()
        .try_into()
        .expect("generic parameter count is bounded by syntax nodes")
}

fn is_intrinsic_type_name(name: &Name) -> bool {
    matches!(
        name.as_str(),
        "Bool"
            | "Int"
            | "Float"
            | "Byte"
            | "Char"
            | "String"
            | "Unit"
            | "Never"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float32"
            | "Float64"
            | "Option"
            | "Result"
            | "Array"
            | "Map"
            | "Set"
            | "Range"
            | "Iterator"
            | "Ref"
            | "Pointer"
            | "Join"
            | "Command"
            | "Pipeline"
    )
}
