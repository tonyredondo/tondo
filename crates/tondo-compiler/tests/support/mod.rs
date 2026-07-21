use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    Spec,
    CompilePass,
    CompileFail,
    Runtime,
}

impl FixtureKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::CompilePass => "compile-pass",
            Self::CompileFail => "compile-fail",
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    pub kind: FixtureKind,
    pub source: PathBuf,
}

impl Fixture {
    pub fn sidecar(&self, extension: &str) -> PathBuf {
        self.source.with_extension(extension)
    }

    pub fn run(&self) -> Result<FixtureObservation, String> {
        let bytes = fs::read(&self.source).map_err(|error| error.to_string())?;
        let logical_path = self
            .source
            .strip_prefix(workspace_test_root())
            .map_err(|error| error.to_string())?
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| "fixture path is not valid UTF-8".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let operation = if self.kind == FixtureKind::Runtime {
            Operation::Run
        } else {
            Operation::Check
        };
        let source_form = match self.kind {
            FixtureKind::Spec => SourceForm::Fragment,
            FixtureKind::Runtime => SourceForm::Script,
            FixtureKind::CompilePass | FixtureKind::CompileFail => SourceForm::Module,
        };
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:fixture").map_err(|error| error.to_string())?,
                ModulePath::new("test").map_err(|error| error.to_string())?,
                LogicalPath::new(logical_path).map_err(|error| error.to_string())?,
                bytes,
            ))
            .map_err(|error| error.to_string())?;
        let request = CompilationRequest::new(
            operation,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            source_form,
            ResourceLimits::default(),
            PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?,
            sources,
            root,
        )
        .map_err(|error| error.to_string())?;
        let output = execute(request).map_err(|error| error.to_string())?;
        let codes = output
            .diagnostics()
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect();

        Ok(FixtureObservation {
            status: output.status(),
            codes,
            json: output
                .diagnostics()
                .json_lines()
                .map_err(|error| error.to_string())?,
            human: output.diagnostics().human(),
            stdout: output.stdout().to_vec(),
            stderr: Vec::new(),
        })
    }

    pub fn assert_matches(&self, observation: &FixtureObservation) -> Result<(), String> {
        match self.kind {
            FixtureKind::CompilePass => {
                if observation.status != CompilationStatus::Success {
                    return Err(format!("{} was rejected", self.source.display()));
                }
                if self.sidecar("codes").exists() {
                    let expected = required_codes(&self.sidecar("codes"))?;
                    if observation.codes != expected {
                        return Err(format!(
                            "{} produced {:?}, expected {:?}",
                            self.source.display(),
                            observation.codes,
                            expected
                        ));
                    }
                } else if !observation.codes.is_empty() {
                    return Err(format!(
                        "{} produced undeclared diagnostics {:?}",
                        self.source.display(),
                        observation.codes
                    ));
                }
            }
            FixtureKind::CompileFail => {
                if observation.status != CompilationStatus::Rejected {
                    return Err(format!("{} was accepted", self.source.display()));
                }
                let expected = required_codes(&self.sidecar("codes"))?;
                if observation.codes != expected {
                    return Err(format!(
                        "{} produced {:?}, expected {:?}",
                        self.source.display(),
                        observation.codes,
                        expected
                    ));
                }
            }
            FixtureKind::Spec => {
                let expected = required_codes(&self.sidecar("codes"))?;
                if observation.codes != expected {
                    return Err(format!(
                        "{} produced {:?}, expected {:?}",
                        self.source.display(),
                        observation.codes,
                        expected
                    ));
                }
            }
            FixtureKind::Runtime => {
                let expected_exit = fs::read_to_string(self.sidecar("exit"))
                    .map_err(|error| error.to_string())?
                    .trim()
                    .parse::<i32>()
                    .map_err(|error| error.to_string())?;
                if observation.exit_code() != expected_exit {
                    return Err(format!(
                        "{} exited {}, expected {expected_exit}",
                        self.source.display(),
                        observation.exit_code()
                    ));
                }
                if self.sidecar("codes").exists() {
                    let expected = required_codes(&self.sidecar("codes"))?;
                    if observation.codes != expected {
                        return Err(format!(
                            "{} produced {:?}, expected {:?}",
                            self.source.display(),
                            observation.codes,
                            expected
                        ));
                    }
                } else if !observation.codes.is_empty() {
                    return Err(format!(
                        "{} produced undeclared diagnostics {:?}",
                        self.source.display(),
                        observation.codes
                    ));
                }
            }
        }

        compare_optional_text(&self.sidecar("jsonl"), &observation.json)?;
        compare_optional_text(&self.sidecar("stderr"), &observation.human)?;
        compare_optional_bytes(&self.sidecar("stdout"), &observation.stdout)?;
        compare_optional_bytes(&self.sidecar("runtime-stderr"), &observation.stderr)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct FixtureObservation {
    status: CompilationStatus,
    codes: Vec<String>,
    json: String,
    human: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl FixtureObservation {
    fn exit_code(&self) -> i32 {
        match self.status {
            CompilationStatus::Success => 0,
            CompilationStatus::Rejected => 1,
        }
    }
}

fn required_codes(path: &Path) -> Result<Vec<String>, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    Ok(contents
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn compare_optional_text(path: &Path, actual: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let expected = fs::read_to_string(path).map_err(|error| error.to_string())?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("snapshot mismatch for {}", path.display()))
    }
}

fn compare_optional_bytes(path: &Path, actual: &[u8]) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let expected = fs::read(path).map_err(|error| error.to_string())?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("snapshot mismatch for {}", path.display()))
    }
}

pub fn workspace_test_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests")
}

pub fn discover(kind: FixtureKind) -> io::Result<Vec<Fixture>> {
    let root = workspace_test_root().join(kind.directory());
    let mut sources = Vec::new();
    collect_tondo_sources(&root, &mut sources)?;
    sources.sort();
    Ok(sources
        .into_iter()
        .map(|source| Fixture { kind, source })
        .collect())
}

fn collect_tondo_sources(directory: &Path, sources: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_tondo_sources(&path, sources)?;
        } else if path.extension().is_some_and(|extension| extension == "to") {
            sources.push(path);
        }
    }
    Ok(())
}

pub fn inline_request(operation: Operation, source_name: &str, bytes: &[u8]) -> CompilationRequest {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:inline-test").unwrap(),
            ModulePath::new("test").unwrap(),
            LogicalPath::new(source_name).unwrap(),
            bytes,
        ))
        .unwrap();
    CompilationRequest::new(
        operation,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BTreeSet::new(),
        DiagnosticFormat::Json,
        SourceForm::Fragment,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).unwrap(),
        sources,
        root,
    )
    .unwrap()
}
