//! Compile a provider's ordinary source through the shared checked frontend.

use super::*;
use crate::bytecode::{BytecodeLoweringLimits, lower_entry_to_bytecode};
use crate::driver::{CompilationRequest, CompilationStatus, Operation};
use crate::hir::HirCallableId;
use crate::mir::{MirLoweringLimits, lower_to_mir};
use crate::resolve::{SymbolKind, Visibility};

impl MetaVmArtifact {
    /// Compile an ordinary source entry with the exact public companion ABI.
    pub fn compile_entry(
        request: CompilationRequest,
        entry: &str,
        kind: MetaEntryKind,
    ) -> Result<Self, MetaVmError> {
        let request = request
            .with_meta_companion()
            .map_err(|error| MetaVmError::Compilation(error.to_string()))?;
        let mut artifact = Self::compile(request, entry)?;
        let program = &artifact.program;
        let callable = program
            .callables
            .iter()
            .find(|call| call.implementation == Some(artifact.entry))
            .ok_or_else(|| MetaVmError::Compilation("provider entry is missing".into()))?;
        let nominal = |ty, name: &str| {
            let package = crate::std_meta::STD_META_PACKAGE;
            let expected = format!("@{}:{package}::meta::type::{name}", package.len());
            matches!(program.ty(ty).map(|ty| &ty.kind), Some(BytecodeTypeKind::Nominal { identity, arguments, .. })
                if *identity == expected && arguments.is_empty())
        };
        let (input, output) = match kind {
            MetaEntryKind::Generate => ("GenerateRequest", "GenerateResponse"),
            MetaEntryKind::Derive => ("DeriveRequest", "DeriveResponse"),
        };
        let valid_input = matches!(callable.parameters.as_slice(), [parameter]
            if !parameter.receiver && parameter.mode == tondo_vm::bytecode::BytecodeParameterMode::Value
                && nominal(parameter.ty, input));
        let valid_output = matches!(program.ty(callable.outcome).map(|ty| &ty.kind),
            Some(BytecodeTypeKind::Result { success, error }) if nominal(*success, output) && nominal(*error, "Error"));
        if !valid_input || !valid_output || callable.generic_arity != 0 {
            return Err(MetaVmError::Compilation(format!(
                "provider entry must accept meta.{input} and return meta.{output} ! meta.Error"
            )));
        }
        artifact.entry_kind = Some(kind);
        Ok(artifact)
    }

    /// Compile a public, concrete callable from an already closed meta package
    /// graph. This performs no source discovery, host execution or generation.
    /// The calling generator/derive adapter validates its exact request and
    /// response contract before executing the artifact.
    pub fn compile(request: CompilationRequest, entry: &str) -> Result<Self, MetaVmError> {
        let fail = |message: String| MetaVmError::Compilation(message);
        if request.operation() != Operation::Check
            || request.target().name() != "tondo-meta"
            || request.profile() != HostProfile::Meta
            || !request.capabilities().is_empty()
        {
            return Err(fail(
                "provider compilation requires a meta check request with no capabilities".into(),
            ));
        }
        let package = request.packages().root().clone();
        let root_module = request
            .sources()
            .get(request.root())
            .map_err(|e| fail(e.to_string()))?
            .module()
            .as_str()
            .to_owned();
        let (module, name) = entry.rsplit_once('.').unwrap_or((&root_module, entry));
        crate::source::ModulePath::new(module).map_err(|e| fail(e.to_string()))?;
        crate::package::Name::new(name).map_err(|e| fail(e.to_string()))?;
        let limits = request.limits();
        let output = crate::driver::execute(request).map_err(|e| fail(e.to_string()))?;
        if output.status() != CompilationStatus::Success {
            return Err(fail(format!(
                "provider source was rejected: {:?}",
                output.diagnostics()
            )));
        }
        let semantic = output
            .semantic_model()
            .ok_or_else(|| fail("provider semantic model is missing".into()))?;
        if !semantic.expression_check_complete() {
            return Err(fail("provider expression checking is incomplete".into()));
        }
        let resolved = semantic.resolved();
        let hir = semantic
            .hir()
            .ok_or_else(|| fail("provider typed program is missing".into()))?;
        let symbol = resolved
            .symbols()
            .find(|symbol| {
                symbol.identity().package() == &package
                    && symbol.identity().module().as_str() == module
                    && symbol.name().as_str() == name
                    && symbol.kind() == SymbolKind::Function
            })
            .ok_or_else(|| fail(format!("provider entry `{entry}` is missing")))?;
        if symbol.visibility() != Visibility::Public {
            return Err(fail(format!("provider entry `{entry}` is private")));
        }
        let identity = symbol.identity().canonical_name();
        let mir = lower_to_mir(
            resolved,
            hir,
            MirLoweringLimits {
                max_functions: limits.max_mir_functions,
                max_blocks_per_function: limits.max_mir_blocks_per_function,
                max_locals_per_function: limits.max_mir_locals_per_function,
                max_statements_per_function: limits.max_mir_statements_per_function,
                max_verification_steps: limits.max_mir_verification_steps,
            },
        )
        .map_err(|e| fail(e.to_string()))?;
        let program = lower_entry_to_bytecode(
            resolved,
            hir,
            &mir,
            BytecodeLoweringLimits {
                max_types: limits.max_bytecode_types,
                max_nominals: limits.max_bytecode_nominals,
                max_callables: limits.max_bytecode_callables,
                max_constants: limits.max_bytecode_constants,
                max_functions: limits.max_bytecode_functions,
                max_slots_per_function: limits.max_bytecode_slots_per_function,
                max_blocks_per_function: limits.max_bytecode_blocks_per_function,
                max_instructions_per_function: limits.max_bytecode_instructions_per_function,
                max_spans_per_function: limits.max_bytecode_spans_per_function,
                max_generic_instantiations: limits.max_generic_instantiations,
                max_verification_steps: limits.max_bytecode_verification_steps,
            },
            HirCallableId::Symbol(symbol.id()),
        )
        .map_err(|e| fail(e.to_string()))?;
        validate_program(&program)?;
        let entry = program
            .callables
            .iter()
            .find(|callable| callable.name == identity)
            .and_then(|callable| callable.implementation)
            .ok_or_else(|| fail("provider entry has no concrete bytecode body".into()))?;
        Ok(Self::new(program, entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{DiagnosticFormat, ResourceLimits, SourceForm};
    use crate::package::{Edition, PackageGraph};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};

    fn request(source: &str) -> CompilationRequest {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:meta-provider").unwrap(),
                ModulePath::new("provider").unwrap(),
                LogicalPath::new("src/provider.to").unwrap(),
                source.as_bytes(),
            ))
            .unwrap();
        let packages = PackageGraph::loose(&sources, root).unwrap();
        CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::tondo_meta(),
            HostProfile::Meta,
            Default::default(),
            DiagnosticFormat::Human,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap()
    }

    #[test]
    fn meta_source_provider_compiles_and_consumes_an_ordinary_request() {
        let source = r#"
pub type Request = { prefix: String, count: Int }
fn append[T](value: T, transform: fn(T): String): String { transform(value) }
pub fn generate(request: Request): String {
    var text = ""
    for _ in 0..request.count {
        text = "{text}{append(request.prefix, (value) { "{value}!" })}"
    }

    text
}
fn unused(): Int { 99 }
"#;
        let artifact = MetaVmArtifact::compile(request(source), "provider.generate").unwrap();
        assert!(
            !artifact
                .program
                .callables
                .iter()
                .any(|call| call.name.ends_with("::unused"))
        );
        let program = artifact.load(MetaVmLimits::default()).unwrap();
        for _ in 0..2 {
            let result = program
                .run_with_request(
                    RuntimeValue::Record {
                        name: "Request".into(),
                        values: vec![RuntimeValue::String("x".into()), RuntimeValue::Integer(3)],
                    },
                    outcome_payload_bytes,
                )
                .unwrap();
            assert_eq!(
                result.outcome,
                VmOutcome::Returned(RuntimeValue::String("x!x!x!".into()))
            );
        }
    }

    #[test]
    fn meta_public_source_generator_consumes_companion_request_and_builds_typed_output() {
        use crate::meta::*;
        use crate::std_meta::source_api;
        let source = r#"
import std.meta
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let input = request.input("schema")?
    assert(input.bytes[0] == Byte(42u8))
    assert(input.hash != "")
    assert(request.inputs().length() == 1)
    let snapshot = request.snapshot()
    assert(snapshot.format == "tondo-meta-model-0.1/1")
    assert(snapshot.roots.length() == 1)
    assert(snapshot.declarations.length() == 1)
    let declaration = snapshot.declarations[0]
    assert(declaration.identity == "Item")
    match declaration.kind {
        meta.DeclarationKind.Record(fields) => {
            assert(fields[0].typeRef.identity() == "String")
            assert(fields[0].docs == some("field docs"))
        }
        _ => panic("expected record")
    }
    var builder = request.sourceBuilder()
    builder.add(request.outputs()[0].path, "pub fn café(): String {{ \"🙂\" }}\n")?
    builder.finish()
}
"#;
        let artifact = MetaVmArtifact::compile_entry(
            request(source),
            "provider.generate",
            MetaEntryKind::Generate,
        )
        .unwrap();
        let span = MetaSpan::new(0, 0, 100).unwrap();
        let model = MetaSnapshot::new(
            crate::meta::MetaEnvironment::meta(),
            [MetaRoot::new("root:app", "app").unwrap()],
            [MetaModule::new("app", None::<String>).unwrap()],
            [MetaDeclaration::new(
                "Item",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span,
                None::<String>,
                MetaDeclarationKind::record([MetaField::new(
                    "name",
                    "String",
                    MetaVisibility::Public,
                    0,
                    span,
                    Some("field docs"),
                )
                .unwrap()]),
            )
            .unwrap()],
        )
        .unwrap();
        let limits = MetaLimits::new(100_000, 1024 * 1024, 1024).unwrap();
        let request = MetaRequest::new(
            model,
            [MetaInput::new("schema", [42]).unwrap()],
            [MetaOutputSpec::new("generated/item.to", "app").unwrap()],
            limits,
        )
        .unwrap();
        let bounded = artifact
            .clone()
            .load(MetaVmLimits {
                max_output_bytes: 89,
                ..MetaVmLimits::for_request(limits)
            })
            .unwrap();
        assert!(matches!(
            bounded.run_with_request(source_api::generate_request(&request), |outcome| {
                source_api::measure_generate_output(outcome, &request)
            }),
            Err(MetaVmError::OutputLimit {
                limit: 89,
                actual: 90
            })
        ));
        let program = artifact.load(MetaVmLimits::for_request(limits)).unwrap();
        let mut prior = None;
        for _ in 0..2 {
            let execution = program
                .run_with_request(source_api::generate_request(&request), |outcome| {
                    source_api::measure_generate_output(outcome, &request)
                })
                .unwrap();
            let response = source_api::generate_response(&execution.outcome, &request).unwrap();
            assert_eq!(
                response.outputs()[0].bytes(),
                "pub fn café(): String { \"🙂\" }\n".as_bytes()
            );
            // The canonical response includes the path, escaped source, and
            // empty diagnostics array, not just the 34 source bytes.
            assert_eq!(execution.counters.output_bytes, 90);
            let current = (response, execution.counters);
            if let Some(previous) = &prior {
                assert_eq!(previous, &current);
            }
            prior = Some(current);
        }
    }

    #[test]
    fn meta_public_source_entry_requires_exact_companion_signature_and_private_requests() {
        for (source, expected) in [
            (
                "pub type GenerateRequest = {}\npub fn generate(request: GenerateRequest): String { \"wrong\" }",
                "must accept meta.GenerateRequest",
            ),
            (
                "import std.meta\npub fn generate(request: meta.GenerateRequest): meta.GenerateResponse { meta.GenerateResponse { outputs: [:], diagnostics: [] } }",
                "must accept meta.GenerateRequest",
            ),
            (
                "import std.meta\npub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error { request.outputSpecs\nerr(meta.Error.Provider(\"wrong\")) }",
                "private",
            ),
        ] {
            let error = MetaVmArtifact::compile_entry(
                request(source),
                "provider.generate",
                MetaEntryKind::Generate,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn meta_source_provider_rejects_invalid_entry_and_nested_derivation() {
        for (source, entry, expected) in [
            ("pub fn generate(): Int { 1 }", "provider.absent", "missing"),
            ("fn generate(): Int { 1 }", "provider.generate", "private"),
            (
                "pub fn generate[T](value: T): T { value }",
                "provider.generate",
                "concrete",
            ),
            (
                "pub fn generate(): Int { 1 }",
                "provider.unknown.generate",
                "missing",
            ),
            (
                "type Item = { value: Int }\nderive Display for Item\npub fn generate(): Int { 1 }",
                "provider.generate",
                "E2109",
            ),
        ] {
            let error = MetaVmArtifact::compile(request(source), entry)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
}
