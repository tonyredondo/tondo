//! Immutable ordinary-source providers shared by generation and derivation.

use crate::driver::CompilationRequest;
use crate::meta_derive::{DeriveProviderCompiler, DeriveProviderRequest};
use crate::meta_generate::{GeneratorProviderCompiler, GeneratorProviderRequest};
use crate::meta_vm::{MetaEntryKind, MetaVmArtifact, MetaVmError};

/// Source providers selected by the closed project plan. Keys retain the full
/// runtime trait identity; source spelling and import aliases are not keys.
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceDeriveProviders {
    entries: std::collections::BTreeMap<(String, String, String), SourceDeriveRegistration>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceDeriveRegistration {
    pub locked: crate::toolchain::LockedDeriveProvider,
    pub provider: std::sync::Arc<SourceMetaProvider>,
}

impl SourceDeriveRegistration {
    pub fn identity(&self) -> String {
        format!(
            "{}::{}::{}=>{}::{}@{}",
            self.locked.trait_package,
            self.locked.trait_module,
            self.locked.trait_name,
            self.locked.provider_package,
            self.locked.entry,
            self.locked.provider_hash
        )
    }
}

impl SourceDeriveProviders {
    pub fn insert(&mut self, registration: SourceDeriveRegistration) -> Result<(), String> {
        let locked = &registration.locked;
        if registration.provider.artifact.entry_kind() != Some(MetaEntryKind::Derive)
            || registration
                .provider
                .hash()
                .map_err(|error| error.to_string())?
                != locked.provider_hash
        {
            return Err(
                "locked derive executable hash or ABI differs from the compiled provider".into(),
            );
        }
        let key = (
            locked.trait_package.clone(),
            locked.trait_module.clone(),
            locked.trait_name.clone(),
        );
        match self.entries.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(registration);
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                Err("duplicate source derive provider".into())
            }
        }
    }

    pub fn get(&self, symbol: &crate::resolve::Symbol) -> Option<&SourceDeriveRegistration> {
        self.entries.get(&(
            symbol.identity().package().as_str().into(),
            symbol.identity().module().as_str().into(),
            symbol.name().as_str().into(),
        ))
    }
}

#[derive(Debug)]
pub struct SourceMetaProvider {
    artifact: MetaVmArtifact,
}

impl SourceMetaProvider {
    pub fn hash(&self) -> Result<String, MetaVmError> {
        self.artifact.hash()
    }

    /// The caller supplies an already closed source graph whose bytes have been
    /// checked against the package lock. Compilation runs once; each invocation
    /// receives a cloned immutable artifact and starts a fresh VM.
    pub fn compile(
        request: CompilationRequest,
        entry: &str,
        kind: MetaEntryKind,
    ) -> Result<Self, MetaVmError> {
        Ok(Self {
            artifact: MetaVmArtifact::compile_entry(request, entry, kind)?,
        })
    }
}

impl GeneratorProviderCompiler for SourceMetaProvider {
    fn compile(&self, _request: GeneratorProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
        if self.artifact.entry_kind() != Some(MetaEntryKind::Generate) {
            return Err("derive entry cannot serve a generator".into());
        }
        Ok(self.artifact.clone())
    }
}

impl DeriveProviderCompiler for SourceMetaProvider {
    fn compile(&self, _request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
        if self.artifact.entry_kind() != Some(MetaEntryKind::Derive) {
            return Err("generator entry cannot serve a derive request".into());
        }
        Ok(self.artifact.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::*;
    use crate::meta_generate::{
        GeneratorExecution, GeneratorExecutionError, GeneratorProviderRegistry,
        execute_generator_plan,
    };
    use crate::meta_test_support::source_request;
    use crate::toolchain::{Limits, LockedGenerator, Output};

    fn derive(
        source: &str,
    ) -> Result<crate::meta_derive::DeriveExecution, crate::meta_derive::DeriveExecutionError> {
        use crate::meta_derive::{DeriveProviderRegistry, execute_derive_plan};
        use crate::source::{
            LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, TextRange,
        };
        let target = "type User = { secret: Int }\nderive Display for User\n";
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:app").unwrap(),
                ModulePath::new("app").unwrap(),
                LogicalPath::new("src/app.to").unwrap(),
                target.as_bytes(),
            ))
            .unwrap();
        let split = target.find("derive").unwrap() as u32;
        let request_span = sources
            .span(file, TextRange::new(split, target.len() as u32).unwrap())
            .unwrap();
        let target_span = MetaSpan::new(file.index(), 0, split).unwrap();
        let snapshot = MetaSnapshot::new(
            crate::meta::MetaEnvironment::meta(),
            [MetaRoot::new("workspace:app@1", "app").unwrap()],
            [MetaModule::new("app", None::<String>).unwrap()],
            [MetaDeclaration::new(
                "User",
                "app",
                MetaVisibility::Private,
                [],
                [],
                target_span,
                None::<String>,
                MetaDeclarationKind::record([MetaField::new(
                    "secret",
                    "Int",
                    MetaVisibility::Private,
                    0,
                    target_span,
                    None::<String>,
                )
                .unwrap()]),
            )
            .unwrap()],
        )
        .unwrap();
        let mut context = DeriveContext::new("app");
        context.add_target(DeriveTarget::new(
            "User",
            "app",
            Vec::<String>::new(),
            DeriveTargetKind::Record,
        ));
        context.add_trait("Display");
        context.add_provider(DeriveProvider::new(
            "Display",
            "custom.display",
            Vec::<String>::new(),
        ));
        let plan = validate_derive_requests(
            &[
                DeriveRequest::new("app", "User", Vec::<String>::new(), ["Display"])
                    .with_span(request_span),
            ],
            &context,
        )
        .unwrap();
        let provider = SourceMetaProvider::compile(
            source_request(source),
            "provider.expandDerive",
            MetaEntryKind::Derive,
        )
        .unwrap();
        let mut registry = DeriveProviderRegistry::default();
        registry
            .insert("custom.display", Arc::new(provider))
            .unwrap();
        execute_derive_plan(
            &plan,
            snapshot,
            MetaLimits::new(100_000, 1024 * 1024, 1024).unwrap(),
            &registry,
        )
    }

    const DERIVE_SOURCE: &str = r#"
import std.meta
pub fn expandDerive(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    assert(request.target() == "User")
    assert(request.module() == "app")
    assert(request.traitIdentity() == "Display")
    assert(request.bounds().length() == 0)
    let declaration = request.snapshot().declarations[0]
    assert(declaration.visibility == meta.Visibility.Private)
    match declaration.kind {
        meta.DeclarationKind.Record(fields) => {
            assert(fields[0].name == "secret")
            assert(fields[0].visibility == meta.Visibility.Private)
        }
        _ => panic("expected target record")
    }
    let origin = request.span()
    let source = "impl  {request.traitIdentity()} for {request.target()} {{\nfn display(self):String{{\"{request.target()}\"}}\n}}\n"
    ok(meta.DeriveResponse {
        source,
        diagnostics: [meta.Diagnostic { severity: meta.DiagnosticSeverity.Note, message: "derived display", origin: some(origin) }],
        mappings: [meta.SourceMap { generatedStart: 0u32, generatedEnd: 4u32, originFile: origin.file, originStart: origin.start, originEnd: origin.end }]
    })
}
"#;

    #[test]
    fn meta_public_source_derive_compiles_complete_impl_and_preserves_request_mapping() {
        let execution = derive(DERIVE_SOURCE).unwrap();
        let output = &execution.response().outputs()[0];
        assert_eq!(&output.bytes()[..4], b"impl");
        assert_eq!(output.mappings()[0].generated_start(), 0);
        assert_eq!(output.mappings()[0].generated_end(), 4);
        assert_eq!(
            execution.response().diagnostics()[0].origin,
            Some(output.mappings()[0].origin())
        );
        let source = format!(
            "type User = {{ secret: Int }}\n{}\npub fn run(): String {{\n    let value = User {{ secret: 7 }}\n    \"{{value}}\"\n}}\n",
            std::str::from_utf8(output.bytes()).unwrap()
        );
        let artifact = MetaVmArtifact::compile(source_request(&source), "provider.run").unwrap();
        let result = artifact
            .load(crate::meta_vm::MetaVmLimits::default())
            .unwrap()
            .run()
            .unwrap();
        assert_eq!(
            result.outcome,
            tondo_vm::runtime::VmOutcome::Returned(tondo_vm::runtime::RuntimeValue::String(
                "User".into()
            ))
        );
        assert_eq!(derive(DERIVE_SOURCE).unwrap(), execution);
    }

    #[test]
    fn meta_public_derive_builder_enforces_one_output_and_matching_finish() {
        let source = r#"
import std.meta
pub fn expandDerive(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    var builder = request.sourceBuilder()
    let outputs = builder.outputs()
    assert(outputs.length() == 1)
    assert(outputs[0].module == request.module())
    assert(match builder.finishDerive() {
        err(meta.Error.MissingOutput(path)) => path == outputs[0].path
        _ => false
    })
    assert(match builder.finish() {
        err(meta.Error.Provider(_)) => true
        _ => false
    })
    assert(match builder.add("unexpected.to", "") {
        err(meta.Error.UnknownOutput(_)) => true
        _ => false
    })
    builder.add(outputs[0].path, "impl Display for User {{\nfn display(self): String {{\n\"built\"\n}}\n}}\n")?
    assert(match builder.add(outputs[0].path, "") {
        err(meta.Error.DuplicateOutput(_)) => true
        _ => false
    })
    builder.finishDerive()
}
"#;
        let execution = derive(source).unwrap();
        assert_eq!(execution.response().outputs().len(), 1);
        assert!(
            std::str::from_utf8(execution.response().outputs()[0].bytes())
                .unwrap()
                .contains("\"built\"")
        );
        let result = generator(
            "let result = request.sourceBuilder().finishDerive()\nassert(match result {\nerr(meta.Error.Provider(_)) => true\n_ => false\n})\nvar builder = request.sourceBuilder()\nlet path = builder.outputs()[0].path\nbuilder.add(path, \"pub type Value = Int\\n\")?\nbuilder.finish()",
            8192,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn meta_public_source_derive_rejects_foreign_target_and_unmappable_ranges() {
        use crate::meta_derive::DeriveExecutionError;
        let foreign = DERIVE_SOURCE.replace("for {request.target()}", "for Other");
        assert!(matches!(
            derive(&foreign),
            Err(DeriveExecutionError::InvalidProviderBody)
        ));
        let unmappable = DERIVE_SOURCE.replace(
            "generatedStart: 0u32, generatedEnd: 4u32",
            "generatedStart: 5u32, generatedEnd: 6u32",
        );
        assert!(matches!(
            derive(&unmappable),
            Err(DeriveExecutionError::InvalidProviderBody)
        ));
        let invalid_origin = DERIVE_SOURCE.replace("originEnd: origin.end", "originEnd: 9999u32");
        assert!(matches!(
            derive(&invalid_origin),
            Err(DeriveExecutionError::ProviderVm {
                source: MetaVmError::StructuredOutput(_),
                ..
            })
        ));
    }
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn generator(
        body: &str,
        output_limit: u64,
    ) -> Result<GeneratorExecution, GeneratorExecutionError> {
        let source = format!(
            "import std.meta\npub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {{\n{body}\n}}\n"
        );
        let provider = SourceMetaProvider::compile(
            source_request(&source),
            "provider.generate",
            MetaEntryKind::Generate,
        )
        .unwrap();
        let generator = LockedGenerator {
            id: "ordinary".into(),
            owner_package: "workspace:app@1".into(),
            provider_package: "workspace:provider@1".into(),
            entry: "provider.generate".into(),
            meta_model: META_MODEL.into(),
            provider_hash: crate::artifact::sha256(source.as_bytes()),
            inputs: vec![],
            model_roots: vec![],
            outputs: vec![Output {
                logical_path: "generated/result.to".into(),
                module: "app".into(),
            }],
            limits: Limits {
                steps: 100_000,
                memory_bytes: 1024 * 1024,
                output_bytes: output_limit,
            },
        };
        let mut registry = GeneratorProviderRegistry::default();
        registry.insert_for(&generator, Arc::new(provider)).unwrap();
        execute_generator_plan(
            &[generator],
            &[],
            &BTreeMap::new(),
            &BTreeMap::from([(
                "ordinary".into(),
                MetaSnapshot::new(crate::meta::MetaEnvironment::meta(), [], [], []).unwrap(),
            )]),
            &registry,
        )
    }

    #[test]
    fn meta_public_source_generator_plan_preserves_diagnostics_and_formats_atomically() {
        let body = r#"assert(request.snapshot().declarations.length() == 0)
ok(meta.GenerateResponse {
    outputs: [request.outputs()[0].path: "pub fn value():Int{{7}}\n"],
    diagnostics: [meta.Diagnostic { severity: meta.DiagnosticSeverity.Warning, message: "generated value", origin: none }]
})"#;
        let first = generator(body, 1024).unwrap();
        let second = generator(body, 1024).unwrap();
        assert_eq!(first, second);
        let response = first.results()[0].response();
        assert_eq!(
            response.outputs()[0].bytes(),
            b"pub fn value(): Int {\n    7\n}\n"
        );
        assert_eq!(response.diagnostics()[0].message, "generated value");
        assert_eq!(
            MetaResponse::decode(&response.canonical_bytes().unwrap()).unwrap(),
            *response
        );
    }

    #[test]
    fn meta_public_source_generator_plan_rejects_outputs_errors_and_nested_generation() {
        for (body, expected) in [
            (
                r#"ok(meta.GenerateResponse { outputs: [:], diagnostics: [] })"#,
                "missing declared meta output",
            ),
            (
                r#"ok(meta.GenerateResponse { outputs: ["generated/extra.to": "fn value() {{}}\n"], diagnostics: [] })"#,
                "undeclared meta output",
            ),
            (
                r#"err(meta.Error.Provider("cannot generate"))"#,
                "cannot generate",
            ),
            (
                r#"ok(meta.GenerateResponse { outputs: [:], diagnostics: [meta.Diagnostic { severity: meta.DiagnosticSeverity.Error, message: "bad model", origin: none }] })"#,
                "bad model",
            ),
            (
                r#"ok(meta.GenerateResponse { outputs: [:], diagnostics: [meta.Diagnostic { severity: meta.DiagnosticSeverity.Warning, message: "wrong origin", origin: some(meta.Span { file: 0u32, start: 0u32, end: 1u32 }) }] })"#,
                "outside the authorized snapshot",
            ),
            (
                r#"ok(meta.GenerateResponse { outputs: [request.outputs()[0].path: "fn broken(\n"], diagnostics: [] })"#,
                "invalid Tondo source",
            ),
            (
                r#"ok(meta.GenerateResponse { outputs: [request.outputs()[0].path: "type Item = {{ value: Int }}\nderive Display for Item\n"], diagnostics: [] })"#,
                "invalid Tondo source",
            ),
            (
                r#"var builder = request.sourceBuilder()
builder.add(request.outputs()[0].path, "fn value() {{}}\n")?
builder.add(request.outputs()[0].path, "fn value() {{}}\n")?
builder.finish()"#,
                "duplicate meta output",
            ),
        ] {
            let error = generator(body, 1024).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        let limited = generator(
            r#"var builder = request.sourceBuilder()
builder.add(request.outputs()[0].path, "é")?
builder.finish()"#,
            1,
        )
        .unwrap_err();
        assert!(matches!(
            limited,
            GeneratorExecutionError::ProviderVm {
                source: MetaVmError::ReportedOutputLimit { limit: 1 },
                ..
            }
        ));
    }
}
