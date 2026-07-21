use std::collections::BTreeMap;

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{ModuleId, PackageGraph};
use crate::source::{FileId, SourceDatabase, Span};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};

use super::{ResolveError, ResolvedEntity, ResolvedName, ResolvedProgram, Visibility};

pub(super) fn validate_public_apis<'a>(
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: &BTreeMap<FileId, &'a Parsed>,
    ordered_files: &[FileId],
    program: &ResolvedProgram,
    diagnostics: &mut Vec<Diagnostic>,
    max_diagnostics: usize,
) -> Result<(), ResolveError> {
    let mut validator = ApiValidator {
        packages,
        sources,
        parsed,
        program,
        diagnostics,
        max_diagnostics,
    };
    for file in ordered_files {
        validator.validate_file(*file)?;
    }
    Ok(())
}

struct ApiValidator<'a> {
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: &'a BTreeMap<FileId, &'a Parsed>,
    program: &'a ResolvedProgram,
    diagnostics: &'a mut Vec<Diagnostic>,
    max_diagnostics: usize,
}

impl ApiValidator<'_> {
    fn validate_file(&mut self, file: FileId) -> Result<(), ResolveError> {
        let module = self.packages.module_for_file(self.sources, file)?;
        let root = self.parsed[&file].cst().root_node();
        for declaration in root.child_nodes() {
            if !is_public(declaration) {
                continue;
            }
            match declaration.kind() {
                SyntaxKind::ConstDecl => {
                    self.validate_direct_types(file, &module, declaration)?;
                }
                SyntaxKind::TypeDecl => {
                    self.validate_generics(file, &module, declaration)?;
                    if let Some(record) = declaration
                        .child_nodes()
                        .find(|child| child.kind() == SyntaxKind::RecordBody)
                    {
                        for field in record
                            .child_nodes()
                            .filter(|child| child.kind() == SyntaxKind::RecordField)
                        {
                            if !is_private_field(field) {
                                self.validate_direct_types(file, &module, field)?;
                            }
                        }
                    } else {
                        self.validate_direct_types(file, &module, declaration)?;
                    }
                }
                SyntaxKind::AliasDecl => {
                    self.validate_generics(file, &module, declaration)?;
                    self.validate_direct_types(file, &module, declaration)?;
                }
                SyntaxKind::EnumDecl => {
                    self.validate_generics(file, &module, declaration)?;
                    for variant in declaration
                        .child_nodes()
                        .filter(|child| child.kind() == SyntaxKind::EnumVariant)
                    {
                        self.validate_type_descendants(file, &module, variant)?;
                    }
                }
                SyntaxKind::TraitDecl => {
                    self.validate_generics(file, &module, declaration)?;
                    for method in declaration
                        .child_nodes()
                        .filter(|child| child.kind() == SyntaxKind::TraitMethod)
                    {
                        self.validate_callable_signature(file, &module, method)?;
                    }
                }
                SyntaxKind::FunctionDecl => {
                    self.validate_callable_signature(file, &module, declaration)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_callable_signature(
        &mut self,
        file: FileId,
        module: &ModuleId,
        callable: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        for child in callable.child_nodes() {
            match child.kind() {
                SyntaxKind::FunctionHead
                | SyntaxKind::GenericParams
                | SyntaxKind::ParameterList
                | SyntaxKind::OutcomeAnnotation => {
                    self.validate_type_descendants(file, module, child)?;
                }
                SyntaxKind::Block => {}
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_generics(
        &mut self,
        file: FileId,
        module: &ModuleId,
        declaration: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        for generic in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
        {
            self.validate_type_descendants(file, module, generic)?;
        }
        Ok(())
    }

    fn validate_direct_types(
        &mut self,
        file: FileId,
        module: &ModuleId,
        node: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        for child in node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::TypeExpr)
        {
            self.validate_type_tree(file, module, child)?;
        }
        Ok(())
    }

    fn validate_type_descendants(
        &mut self,
        file: FileId,
        module: &ModuleId,
        node: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        for child in node.child_nodes() {
            if child.kind() == SyntaxKind::Block {
                continue;
            }
            self.validate_type_tree(file, module, child)?;
        }
        Ok(())
    }

    fn validate_type_tree(
        &mut self,
        file: FileId,
        module: &ModuleId,
        node: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        match node.kind() {
            SyntaxKind::PathType => {
                let path = node
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::TypePath)
                    .expect("a parsed path type contains a type path");
                self.validate_path(file, module, path)?;
                for arguments in path
                    .child_nodes()
                    .filter(|child| child.kind() == SyntaxKind::GenericArgs)
                {
                    self.validate_type_tree(file, module, arguments)?;
                }
                return Ok(());
            }
            SyntaxKind::TypePath => {
                self.validate_path(file, module, node)?;
                for arguments in node
                    .child_nodes()
                    .filter(|child| child.kind() == SyntaxKind::GenericArgs)
                {
                    self.validate_type_tree(file, module, arguments)?;
                }
                return Ok(());
            }
            _ => {}
        }
        for child in node.child_nodes() {
            if child.kind() != SyntaxKind::Block {
                self.validate_type_tree(file, module, child)?;
            }
        }
        Ok(())
    }

    fn validate_path(
        &mut self,
        file: FileId,
        module: &ModuleId,
        path: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        let Some(token) = path
            .child_tokens()
            .find(|token| token.kind() == TokenKind::Identifier)
        else {
            return Ok(());
        };
        let Some(reference) = self.program.reference(file, token.range()) else {
            return Ok(());
        };
        let ResolvedEntity::Name(ResolvedName::Symbol(symbol)) = reference.entity() else {
            return Ok(());
        };
        let declaration = self
            .program
            .symbol(*symbol)
            .expect("resolved references contain valid symbol IDs");
        if declaration.visibility() != Visibility::Private
            || declaration.identity().package() != module.package()
            || declaration.identity().module() != module.path()
        {
            return Ok(());
        }
        self.emit(
            self.sources.span(file, token.range())?,
            format!("public API exposes private type `{}`", declaration.name()),
            declaration.span(),
        )
    }

    fn emit(
        &mut self,
        span: Span,
        message: impl Into<String>,
        private_declaration: Span,
    ) -> Result<(), ResolveError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(ResolveError::DiagnosticLimit {
                file: span.file(),
                offset: span.range().start(),
            });
        }
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1503")?,
            message,
            PrimaryLocation::Source(span),
        )?
        .with_related(Related::new(
            "private type declared here",
            private_declaration,
        )?);
        self.diagnostics.push(diagnostic);
        Ok(())
    }
}

fn is_public(node: SyntaxNodeRef<'_>) -> bool {
    node.child_nodes()
        .any(|child| child.kind() == SyntaxKind::Visibility)
}

fn is_private_field(field: SyntaxNodeRef<'_>) -> bool {
    let Some(name) = field_name_token(field) else {
        return false;
    };
    field
        .child_tokens()
        .any(|token| token.kind() == TokenKind::Priv && token.range() != name.range())
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
