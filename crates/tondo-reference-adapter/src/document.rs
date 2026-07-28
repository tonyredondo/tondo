use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};
use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, HostProfile, Operation,
    ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::{Edition, PackageAlias, PackageGraph, PackageId, PackageNode};
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use tondo_compiler::syntax::{LexMode, ParseLimits, ParseMode, format_parsed, lex, parse};
use tondo_compiler::syntax::{SyntaxKind, TokenKind};
use tondo_conformance::decode_hex;
use tondo_conformance::protocol::{
    AdapterRequest, CompilationState, DocCategory, Observation, WireDocumentFenceAction,
};

pub(crate) fn observe_document_fence(
    request: &AdapterRequest,
    action: &WireDocumentFenceAction,
) -> Result<Observation, String> {
    let fixture_manifest = decode_hex(&action.fixture_manifest_hex)?;
    let actual_fixture_hash = tondo_conformance::sha256(&fixture_manifest);
    if actual_fixture_hash != action.fixture_manifest_sha256 {
        return Err(format!(
            "fixture manifest hash `{actual_fixture_hash}` does not match `{}`",
            action.fixture_manifest_sha256
        ));
    }
    crate::document::fixture::validate_fixture_manifest(
        &fixture_manifest,
        &action.fixture_manifest_sha256,
    )?;
    let source = decode_hex(&action.source_hex)?;
    match action.category {
        DocCategory::Syntax => observe_syntax(action, &source),
        DocCategory::Pseudocode => Err("pseudocode must not reach the adapter".into()),
        DocCategory::Fragment | DocCategory::Script | DocCategory::CompileFail => {
            observe_typed(request, action, &source)
        }
    }
}

fn observe_syntax(action: &WireDocumentFenceAction, source: &[u8]) -> Result<Observation, String> {
    let mut production = None;
    let mut formatted = None;
    let mut parse_ok = false;
    let mut actual_codes = Vec::new();
    for (name, mode) in [
        ("module_program", ParseMode::Module),
        ("syntax_sequence", ParseMode::SyntaxSequence),
        ("standalone_block", ParseMode::StandaloneBlock),
    ] {
        let (sources, root) = one_source(action, source)?;
        let lexed = lex(&sources, root, LexMode::Fragment).map_err(|error| error.to_string())?;
        let parsed = parse(&sources, root, lexed, mode, ParseLimits::default())
            .map_err(|error| error.to_string())?;
        let codes = parsed
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str().to_owned())
            .collect::<Vec<_>>();
        if codes.is_empty() {
            parse_ok = true;
            production = Some(vec![name.to_owned()]);
            formatted = Some(
                format_parsed(&sources, root, &parsed)
                    .map_err(|error| error.to_string())?
                    .into_bytes(),
            );
            break;
        }
        if actual_codes.is_empty() {
            actual_codes = codes;
        }
    }
    actual_codes.sort();
    actual_codes.dedup();
    let formatted_sha256 = formatted.as_deref().map(tondo_conformance::sha256);
    let record = record(
        action,
        production,
        tondo_conformance::sha256(source),
        formatted_sha256,
        Some(parse_ok),
        None,
        actual_codes,
    );
    let mut observation = Observation::empty();
    observation.compilation = if parse_ok {
        CompilationState::Success
    } else {
        CompilationState::Rejected
    };
    observation.exit_code = i32::from(!parse_ok);
    observation.data = record;
    Ok(observation)
}

fn observe_typed(
    request: &AdapterRequest,
    action: &WireDocumentFenceAction,
    source: &[u8],
) -> Result<Observation, String> {
    let fixture = action
        .fixture
        .as_deref()
        .ok_or_else(|| "typed documentation fence has no fixture".to_owned())?;
    let original_source_form = if action.category == DocCategory::Script {
        SourceForm::Script
    } else {
        SourceForm::Fragment
    };
    let lex_mode = if original_source_form == SourceForm::Script {
        LexMode::Script
    } else {
        LexMode::Fragment
    };
    let parse_mode = if original_source_form == SourceForm::Script {
        ParseMode::Script
    } else {
        ParseMode::Fragment
    };
    let (parse_sources, parse_root) = one_source(action, source)?;
    let lexed = lex(&parse_sources, parse_root, lex_mode).map_err(|error| error.to_string())?;
    let parsed = parse(
        &parse_sources,
        parse_root,
        lexed,
        parse_mode,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let parse_ok = parsed.diagnostics().is_empty();
    let formatted = if parse_ok {
        Some(
            format_parsed(&parse_sources, parse_root, &parsed)
                .map_err(|error| error.to_string())?
                .into_bytes(),
        )
    } else {
        None
    };
    let (has_main, has_script_statements) = fragment_shape(&parsed);
    let source_form =
        if action.category == DocCategory::Script || has_script_statements || !has_main {
            SourceForm::Script
        } else {
            SourceForm::Module
        };
    // A declaration-only fragment still has a hygienic implicit body. `Check`
    // never executes it; the unit expression merely makes that body observable
    // to the existing script lowering. An explicit main is compiled as a
    // module unless the fence also contains statements, in which case ordinary
    // E1802 mixing rules remain active.
    let synthetic_entry =
        action.category != DocCategory::Script && !has_main && !has_script_statements;
    let combined = crate::document::fixture::inject(fixture, source, synthetic_entry)?;

    let (sources, root, packages) = fixture_sources(action, &combined)?;
    let capabilities = request
        .target
        .capabilities
        .iter()
        .map(|capability| {
            tondo_compiler::driver::CapabilityName::new(capability.clone())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let output = execute(
        CompilationRequest::new(
            Operation::Check,
            tondo_compiler::package::Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            capabilities,
            DiagnosticFormat::Json,
            source_form,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .map_err(|error| error.to_string())?
        .with_documentation_fixture(),
    )
    .map_err(|error| error.to_string())?;
    let mut actual_codes = output
        .diagnostics()
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .filter(|code| code.starts_with('E') || code.starts_with('W'))
        .collect::<Vec<_>>();
    actual_codes.sort();
    actual_codes.dedup();
    let actual_errors = actual_codes
        .iter()
        .filter(|code| code.starts_with('E'))
        .cloned()
        .collect::<Vec<_>>();
    let typecheck_ok = output.status() == CompilationStatus::Success;
    let accepted = if action.category == DocCategory::CompileFail {
        actual_errors == action.expected_codes
    } else {
        typecheck_ok
    };
    let record = record(
        action,
        Some(vec![
            "script_program".into(),
            if action.category == DocCategory::Script {
                "script_program".into()
            } else {
                "fragment_wrapper".into()
            },
        ]),
        tondo_conformance::sha256(source),
        formatted.as_deref().map(tondo_conformance::sha256),
        Some(parse_ok),
        Some(typecheck_ok),
        actual_codes,
    );
    let mut observation = Observation::empty();
    observation.compilation = if accepted {
        CompilationState::Success
    } else {
        CompilationState::Rejected
    };
    observation.exit_code = i32::from(!accepted);
    observation.diagnostics = crate::normative_diagnostics(output.diagnostics())?;
    observation.data = record;
    Ok(observation)
}

fn fragment_shape(parsed: &tondo_compiler::syntax::Parsed) -> (bool, bool) {
    let mut has_main = false;
    let mut has_script_statements = false;
    for child in parsed.cst().root_node().child_nodes() {
        if matches!(
            child.kind(),
            SyntaxKind::BindingDecl
                | SyntaxKind::Assignment
                | SyntaxKind::ReturnStmt
                | SyntaxKind::FailStmt
                | SyntaxKind::BreakStmt
                | SyntaxKind::ContinueStmt
                | SyntaxKind::DeferStmt
                | SyntaxKind::ForStmt
                | SyntaxKind::ExpressionStmt
                | SyntaxKind::TailExpression
        ) {
            has_script_statements = true;
        }
        if child.kind() == SyntaxKind::FunctionDecl
            && child
                .child_nodes()
                .find(|node| node.kind() == SyntaxKind::FunctionHead)
                .and_then(|head| {
                    head.child_tokens()
                        .find(|token| token.kind() == TokenKind::Identifier)
                })
                .and_then(|token| token.token().normalized_identifier())
                == Some("main")
        {
            has_main = true;
        }
    }
    (has_main, has_script_statements)
}

fn one_source(
    action: &WireDocumentFenceAction,
    source: &[u8],
) -> Result<(SourceDatabase, tondo_compiler::source::FileId), String> {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("doc:{}:{}", action.file, action.fence_byte))
                .map_err(|error| error.to_string())?,
            ModulePath::new("spec").map_err(|error| error.to_string())?,
            LogicalPath::new(format!("fence-{}.to", action.fence_byte))
                .map_err(|error| error.to_string())?,
            Arc::<[u8]>::from(source.to_vec()),
        ))
        .map_err(|error| error.to_string())?;
    Ok((sources, root))
}

fn fixture_sources(
    action: &WireDocumentFenceAction,
    source: &[u8],
) -> Result<(SourceDatabase, tondo_compiler::source::FileId, PackageGraph), String> {
    let mut sources = SourceDatabase::new();
    let root_source = SourceId::new(format!("doc:{}:{}", action.file, action.fence_byte))
        .map_err(|error| error.to_string())?;
    let root_module = ModulePath::new("spec").map_err(|error| error.to_string())?;
    let root = sources
        .add(SourceInput::virtual_file(
            root_source.clone(),
            root_module.clone(),
            LogicalPath::new(format!("fence-{}.to", action.fence_byte))
                .map_err(|error| error.to_string())?,
            Arc::<[u8]>::from(source.to_vec()),
        ))
        .map_err(|error| error.to_string())?;
    let standard_source =
        SourceId::new("toolchain:std:0.1-bootstrap").map_err(|error| error.to_string())?;
    for (module, path, contents) in [
        ("fs", "fixture-fs.to", FIXTURE_FS),
        ("json", "fixture-json.to", FIXTURE_JSON),
    ] {
        sources
            .add(SourceInput::virtual_file(
                standard_source.clone(),
                ModulePath::new(module).map_err(|error| error.to_string())?,
                LogicalPath::new(path).map_err(|error| error.to_string())?,
                Arc::<[u8]>::from(contents.as_bytes()),
            ))
            .map_err(|error| error.to_string())?;
    }
    let root_package = PackageId::new(format!("doc-fixture:{}", action.fence_byte))
        .map_err(|error| error.to_string())?;
    let standard_package =
        PackageId::new("toolchain:std:0.1-bootstrap").map_err(|error| error.to_string())?;
    let packages = PackageGraph::new(
        root_package.clone(),
        standard_package.clone(),
        [
            PackageNode::new(
                root_package,
                root_source,
                PackageAlias::new("main").map_err(|error| error.to_string())?,
                Edition::V0_1,
                [root_module],
                [],
            )
            .map_err(|error| error.to_string())?,
            PackageNode::new(
                standard_package,
                standard_source,
                PackageAlias::new("tondoStd").map_err(|error| error.to_string())?,
                Edition::V0_1,
                ["console", "process", "fs", "json"]
                    .into_iter()
                    .map(ModulePath::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?,
                [],
            )
            .map_err(|error| error.to_string())?,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok((sources, root, packages))
}

const FIXTURE_FS: &str = "\
pub type Path = String\n\
pub type IoError = Unit\n\
pub alias Bytes = Array[Byte]\n\
pub fn read(fixturePath: Path): Bytes ! IoError {\n\
    panic(\"documentation fixture\")\n\
}\n";

const FIXTURE_JSON: &str = "\
pub type DecodeError = Unit\n";

fn record(
    action: &WireDocumentFenceAction,
    production: Option<Vec<String>>,
    source_sha256: String,
    formatted_sha256: Option<String>,
    parse_ok: Option<bool>,
    typecheck_ok: Option<bool>,
    actual_codes: Vec<String>,
) -> Value {
    json!({
        "file": action.file,
        "fence_byte": action.fence_byte,
        "category": match action.category {
            DocCategory::Syntax => "syntax",
            DocCategory::Fragment => "fragment",
            DocCategory::Script => "script",
            DocCategory::CompileFail => "compile-fail",
            DocCategory::Pseudocode => "pseudocode"
        },
        "edition": "0.1",
        "fixture": action.fixture,
        "fixture_sha256": action.fixture.as_ref().map(|_| action.fixture_manifest_sha256.clone()),
        "production": production,
        "source_sha256": source_sha256,
        "formatted_sha256": formatted_sha256,
        "parse_ok": parse_ok,
        "typecheck_ok": typecheck_ok,
        "expected_codes": action.expected_codes,
        "actual_codes": actual_codes
    })
}

mod fixture {
    use std::collections::BTreeSet;

    const EXPECTED_HASH: &str = "1b6ab9f853b7ef4b94b4b9aaff6297e20556f81e8d99c322bed03854453d76c2";

    pub(super) fn validate_fixture_manifest(bytes: &[u8], hash: &str) -> Result<(), String> {
        if hash != EXPECTED_HASH {
            return Err(format!(
                "unsupported fixture manifest hash `{hash}`, expected `{EXPECTED_HASH}`"
            ));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("fixture manifest is not UTF-8: {error}"))?;
        if !text.starts_with("tondo-fixture-manifest 0.1\n")
            || !text.ends_with("end\n")
            || text.contains('\r')
        {
            return Err("fixture manifest is not canonically framed".into());
        }
        let mut fixtures = BTreeSet::new();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("fixture ") {
                let name = name.split_ascii_whitespace().next().unwrap_or_default();
                if !fixtures.insert(name) {
                    return Err(format!("duplicate fixture `{name}`"));
                }
            }
        }
        for required in [
            "spec.core",
            "spec.cursor",
            "spec.resource",
            "spec.domain",
            "spec.settings",
            "spec.user",
            "spec.console",
            "spec.jobs",
            "spec.application",
            "spec.async_page",
            "spec.process",
        ] {
            if !fixtures.contains(required) {
                return Err(format!("fixture manifest omits `{required}`"));
            }
        }
        Ok(())
    }

    pub(super) fn inject(
        name: &str,
        source: &[u8],
        synthetic_entry: bool,
    ) -> Result<Vec<u8>, String> {
        let declarations = declarations(name)?;
        let text = std::str::from_utf8(source)
            .map_err(|error| format!("documentation source is not UTF-8: {error}"))?;
        let mut combined = String::with_capacity(
            text.len()
                .saturating_add(declarations.len())
                .saturating_add(2),
        );
        combined.push_str(synthetic_imports(name)?);
        combined.push_str(text);
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(declarations);
        if synthetic_entry {
            combined.push_str("()\n");
        }
        Ok(combined.into_bytes())
    }

    fn synthetic_imports(name: &str) -> Result<&'static str, String> {
        match name {
            "spec.settings" => Ok("import std.fs\n"),
            "spec.console" | "spec.async_page" => Ok("import std.console\n"),
            "spec.0_1" | "spec.core" | "spec.cursor" | "spec.resource" | "spec.domain"
            | "spec.user" | "spec.jobs" | "spec.application" | "spec.process" => Ok(""),
            _ => Err(format!("unknown documentation fixture `{name}`")),
        }
    }

    fn declarations(name: &str) -> Result<&'static str, String> {
        match name {
            "spec.0_1" | "spec.core" => Ok(CORE),
            "spec.cursor" => Ok(CURSOR),
            "spec.resource" => Ok(RESOURCE),
            "spec.domain" => Ok(DOMAIN),
            "spec.settings" => Ok(SETTINGS),
            "spec.user" => Ok(USER),
            "spec.console" => Ok(CONSOLE),
            "spec.jobs" => Ok(JOBS),
            "spec.application" => Ok(APPLICATION),
            "spec.async_page" => Ok(ASYNC_PAGE),
            "spec.process" => Ok(PROCESS),
            _ => Err(format!("unknown documentation fixture `{name}`")),
        }
    }

    // These declarations are private doc-runner source. They deliberately use
    // ordinary Tondo types and signatures; no declaration becomes part of the
    // prelude or a normal package graph.
    const CORE: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const CURSOR: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
type Query = Unit\n\
type Row = Unit\n\
type DatabaseError = Unit\n\
type Database = Unit\n\
type RowCursor = { handle: Join[Unit, Never] }\n\
fn Database.openRows(_: Query): RowCursor ! DatabaseError {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn RowCursor.close(fixtureCursor: RowCursor) {\n\
    panic(\"documentation fixture\")\n\
}\n\
impl Iterator[Row] for RowCursor {\n\
    fn next(mut self): Row? {\n\
        panic(\"documentation fixture\")\n\
    }\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const RESOURCE: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
fn acquire(): Resource ! AcquireError {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const DOMAIN: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n\
fn decodeUser(_: Bytes): User ! json.DecodeError {\n\
    panic(\"documentation fixture\")\n\
}\n";
    const SETTINGS: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n\
alias Path = fs.Path\n\
alias IoError = fs.IoError\n\
type DecodeError = Unit\n\
type Settings = Unit\n\
fn decodeSettings(_: Bytes): Settings ! DecodeError {\n\
    Settings(())\n\
}\n";
    const USER: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
type UserId = Int\n\
type User = { id: UserId, name: String, email: String? }\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const CONSOLE: &str = CORE;
    const JOBS: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
type Job = { cancelled: Bool }\n\
type JobError = Unit\n\
type Deque[T: Discard] = { items: Array[T] }\n\
fn Deque[T].isEmpty(self): Bool {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Deque[T].popFront(var self): T? {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn run(_: Job): !JobError {\n\
    ()\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const APPLICATION: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
type Path = String\n\
type ArgsError = Unit\n\
type ConfigError = Unit\n\
type RuntimeError = Unit\n\
type Config = Unit\n\
type Options = { configPath: Path }\n\
fn parseArgs(_: Array[String]): Options ! ArgsError {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn loadConfig(_: Path): Config ! ConfigError {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn run(_: Config): !RuntimeError {\n\
    ()\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const ASYNC_PAGE: &str = "\
alias Bytes = Array[Byte]\n\
type Utf8Error = Unit\n\
type Status = Int\n\
type AcquireError = Unit\n\
type Resource = { handle: Join[Unit, Never] }\n\
fn Resource.release(fixtureResource: Resource) {\n\
    panic(\"documentation fixture\")\n\
}\n\
fn Resource.status(self): Status {\n\
    Status(0)\n\
}\n\
type UserId = Int\n\
type User = Unit\n\
type Posts = Unit\n\
type ApiError = Unit\n\
type Page = { user: User, posts: Posts }\n\
async fn fetchUser(_: UserId): User ! ApiError {\n\
    panic(\"documentation fixture\")\n\
}\n\
async fn fetchPosts(_: UserId): Posts ! ApiError {\n\
    panic(\"documentation fixture\")\n\
}\n\
impl Display for Page {\n\
    fn display(self): String {\n\
        \"documentation fixture\"\n\
    }\n\
}\n\
fn consume[T: Discard](_: T) {\n\
}\n";
    const PROCESS: &str = CORE;
}
