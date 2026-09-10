//! Bind a checked expansion to the exact request and its final formatted bytes.

use super::{DeriveFrontendError, GeneratedDeriveSource, model_error, provider_origin};
use crate::meta::{
    MetaEnvironment, MetaInput, MetaLimits, MetaOutputSpec, MetaRequest, MetaSnapshot, MetaSource,
    MetaSourceMapEntry, ValidatedTrait,
};
use crate::meta_atomic::{
    AcceptedMetaResult, MetaBuildContext, MetaInvocation, MetaProducerKind, MetaProviderIdentity,
};
use crate::meta_query::MetaQueryDescriptor;
use crate::source::{
    LogicalPath, SourceDatabase, SourceDiagnosticMapping, SourceFile, SourceId, SourceInput, Span,
    TextRange,
};
use crate::syntax::{LexMode, ParseLimits, ParseMode, format_parsed_with_mappings, lex, parse};
use crate::toolchain::{LockedDeriveProvider, ModelRoot};

pub(super) struct Publication<'a> {
    pub environment: &'a MetaEnvironment,
    pub sources: &'a SourceDatabase,
    pub source: &'a SourceFile,
    pub owner: &'a crate::package::PackageId,
    pub target: &'a str,
    pub request_span: Span,
    pub snapshot: &'a MetaSnapshot,
    pub imports: &'a [Vec<u8>],
}

impl Publication<'_> {
    pub fn accept(
        &self,
        output: &MetaSource,
        trait_request: &ValidatedTrait,
        baseline_header: &str,
        written_bounds: &[String],
        locked: &LockedDeriveProvider,
        limits: MetaLimits,
    ) -> Result<
        (
            GeneratedDeriveSource,
            AcceptedMetaResult,
            MetaQueryDescriptor,
        ),
        DeriveFrontendError,
    > {
        let payload = serde_json::to_vec(&serde_json::json!({
            "target": self.target,
            "trait": trait_request.identity(),
            "bounds": written_bounds,
            "baseline_header": baseline_header,
            "source_id": self.source.source_id().as_str(),
            "path": self.source.path().as_str(),
            "span": [self.request_span.range().start(), self.request_span.range().end()],
            "imports": self.imports,
        }))
        .map_err(model_error)?;
        // The placeholder declares the single output slot. Its final path is
        // derived from the invocation hash and never fed back into that hash.
        let request = MetaRequest::new(
            self.snapshot.clone(),
            [MetaInput::new("derive-request", payload).map_err(model_error)?],
            [
                MetaOutputSpec::new("derive/result.to", self.source.module().as_str())
                    .map_err(model_error)?,
            ],
            limits,
        )
        .map_err(model_error)?;
        let compiler = crate::project::bootstrap_standard_hash();
        let invocation = MetaInvocation::new(
            MetaBuildContext {
                compiler: &compiler,
                edition: &self.environment.edition,
                target: &self.environment.target,
                profile: &self.environment.profile,
                capabilities: &self.environment.capabilities,
                features: &self.environment.features,
            },
            MetaProviderIdentity {
                kind: MetaProducerKind::Derive,
                id: "derive:0000000000000000000000000000000000000000000000000000000000000000",
                package: &locked.provider_package,
                hash: &locked.provider_hash,
                entry: &locked.entry,
            },
            self.snapshot.roots().iter().map(|root| ModelRoot {
                package: root.package().into(),
                module: root.module().into(),
            }),
            &request,
        )
        .map_err(model_error)?;
        let path = invocation.derive_output_path();
        let mut bytes = Vec::new();
        for import in self.imports {
            bytes.extend_from_slice(import);
            if !import.ends_with(b"\n") {
                bytes.push(b'\n');
            }
        }
        let prefix = u32::try_from(bytes.len()).map_err(model_error)?;
        let shifted = output
            .mappings()
            .iter()
            .map(|mapping| {
                let start = mapping
                    .generated_start()
                    .checked_add(prefix)
                    .ok_or_else(|| model_error("generated source offset overflow"))?;
                let end = mapping
                    .generated_end()
                    .checked_add(prefix)
                    .ok_or_else(|| model_error("generated source offset overflow"))?;
                MetaSourceMapEntry::new(start, end, mapping.origin()).map_err(model_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        bytes.extend_from_slice(output.bytes());
        let mut database = SourceDatabase::new();
        let file = database
            .add(SourceInput::virtual_file(
                SourceId::new("meta:format").map_err(model_error)?,
                self.source.module().clone(),
                LogicalPath::new(path).map_err(model_error)?,
                bytes.clone(),
            ))
            .map_err(model_error)?;
        let parsed = parse(
            &database,
            file,
            lex(&database, file, LexMode::Module).map_err(model_error)?,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .map_err(model_error)?;
        let formatted =
            format_parsed_with_mappings(&database, file, &parsed).map_err(model_error)?;
        let mappings =
            crate::meta_source_maps::compose(&bytes, &formatted, &shifted).map_err(model_error)?;
        let mut builder = MetaRequest::new(
            self.snapshot.clone(),
            [],
            [MetaOutputSpec::new(path, self.source.module().as_str()).map_err(model_error)?],
            limits,
        )
        .map_err(model_error)?
        .into_source_builder();
        builder
            .add_mapped_source(
                path,
                self.source.module().as_str(),
                formatted.source().bytes().to_vec(),
                mappings,
            )
            .map_err(|error| {
                DeriveFrontendError::Diagnostics(vec![
                    crate::meta_diagnostics::derive_execution_entry(
                        &crate::meta_derive::DeriveExecutionError::Contract(error),
                        Some(self.request_span),
                        [],
                    ),
                ])
            })?;
        let accepted = invocation
            .accept(builder.finish().map_err(model_error)?)
            .map_err(model_error)?;
        let record = &accepted.record().outputs[0];
        let source = &accepted.response().outputs()[0];
        let mappings = source
            .mappings()
            .iter()
            .map(|mapping| {
                Ok(SourceDiagnosticMapping {
                    generated: TextRange::new(mapping.generated_start(), mapping.generated_end())
                        .map_err(model_error)?,
                    origin: provider_origin(self.sources, mapping.origin(), self.request_span)?,
                })
            })
            .collect::<Result<Vec<_>, DeriveFrontendError>>()?;
        let generated = GeneratedDeriveSource {
            owner: self.owner.clone(),
            source_id: SourceId::new(&record.source_id).map_err(model_error)?,
            module: self.source.module().clone(),
            path: record.path.clone(),
            bytes: source.bytes().to_vec(),
            diagnostic_mappings: mappings,
            baseline_header: baseline_header.into(),
            request_span: self.request_span,
        };
        let descriptor = MetaQueryDescriptor::new(
            accepted.identity_hash(),
            Some(self.target),
            crate::meta_derive_bounds::introduced(source.bytes(), baseline_header)
                .map_err(model_error)?
                .into_iter()
                .map(|bound| bound.identity),
        )
        .map_err(model_error)?;
        Ok((generated, accepted, descriptor))
    }
}
