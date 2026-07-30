use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, WarningProfile, execute,
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
        self.run_with_limits(ResourceLimits::default())
    }

    pub fn run_with_limits(&self, limits: ResourceLimits) -> Result<FixtureObservation, String> {
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
            BuildTarget::vm_hosted_capabilities(),
            DiagnosticFormat::Json,
            source_form,
            limits,
            PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?,
            sources,
            root,
        )
        .map_err(|error| error.to_string())?
        .with_warning_profiles(read_warning_profiles(&self.sidecar("profiles"))?)
        .with_program_arguments(read_program_arguments(self)?);
        let output = execute(request).map_err(|error| error.to_string())?;
        let codes = output
            .diagnostics()
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect();

        Ok(FixtureObservation {
            status: output.status(),
            exit_code: output.exit_code(),
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

#[derive(Debug, PartialEq, Eq)]
pub struct FixtureObservation {
    status: CompilationStatus,
    exit_code: u8,
    codes: Vec<String>,
    json: String,
    human: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl FixtureObservation {
    fn exit_code(&self) -> i32 {
        i32::from(self.exit_code)
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

fn read_warning_profiles(path: &Path) -> Result<Vec<WarningProfile>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut profiles = BTreeSet::new();
    for name in contents.lines().filter(|line| !line.is_empty()) {
        let profile = match name {
            "core" => WarningProfile::Core,
            _ => {
                return Err(format!(
                    "{} declares unknown warning profile `{name}`",
                    path.display()
                ));
            }
        };
        if !profiles.insert(profile) {
            return Err(format!(
                "{} declares warning profile `{name}` more than once",
                path.display()
            ));
        }
    }
    if profiles.is_empty() {
        return Err(format!(
            "{} must declare at least one warning profile",
            path.display()
        ));
    }
    Ok(profiles.into_iter().collect())
}

fn read_program_arguments(fixture: &Fixture) -> Result<Vec<String>, String> {
    let unix = fixture.sidecar("args-unix");
    let windows = fixture.sidecar("args-windows");
    match (unix.is_file(), windows.is_file()) {
        (false, false) => return Ok(Vec::new()),
        (true, true) => {}
        _ => {
            return Err(format!(
                "{} must declare both `.args-unix` and `.args-windows` sidecars",
                fixture.source.display()
            ));
        }
    }
    if fixture.kind != FixtureKind::Runtime {
        return Err(format!(
            "{} declares platform arguments outside a runtime fixture",
            fixture.source.display()
        ));
    }

    #[cfg(unix)]
    {
        read_argument_lines(&windows)?;
        read_argument_lines(&unix)
    }
    #[cfg(windows)]
    {
        read_argument_lines(&unix)?;
        read_argument_lines(&windows)
    }
    #[cfg(not(any(unix, windows)))]
    {
        read_argument_lines(&unix)?;
        read_argument_lines(&windows)?;
        Err(format!(
            "{} has no argument sidecar for this host platform",
            fixture.source.display()
        ))
    }
}

fn read_argument_lines(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.is_empty() || !bytes.ends_with(b"\n") || bytes.contains(&b'\r') {
        return Err(format!(
            "{} must use non-empty, LF-terminated canonical text",
            path.display()
        ));
    }
    let contents = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    let mut arguments = Vec::new();
    for (index, argument) in contents.split_terminator('\n').enumerate() {
        if argument.is_empty() {
            return Err(format!(
                "{} contains an empty argument on line {}",
                path.display(),
                index + 1
            ));
        }
        if argument.contains('\0') {
            return Err(format!(
                "{} contains a null byte on line {}",
                path.display(),
                index + 1
            ));
        }
        arguments.push(argument.to_owned());
    }
    Ok(arguments)
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
        BuildTarget::vm_hosted_capabilities(),
        DiagnosticFormat::Json,
        SourceForm::Fragment,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).unwrap(),
        sources,
        root,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_fixture(kind: FixtureKind) -> Fixture {
        let directory = std::env::temp_dir().join(format!(
            "tondo-fixture-arguments-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Fixture {
            kind,
            source: directory.join("case.to"),
        }
    }

    fn remove_fixture(fixture: &Fixture) {
        fs::remove_dir_all(fixture.source.parent().unwrap()).unwrap();
    }

    #[test]
    fn fixtures_without_platform_arguments_receive_an_empty_argument_list() {
        let fixture = temporary_fixture(FixtureKind::Runtime);
        assert_eq!(
            read_program_arguments(&fixture).unwrap(),
            Vec::<String>::new()
        );
        remove_fixture(&fixture);
    }

    #[test]
    fn platform_arguments_are_paired_and_select_only_the_current_host() {
        let fixture = temporary_fixture(FixtureKind::Runtime);
        fs::write(fixture.sidecar("args-unix"), b"unix first\nunix second\n").unwrap();
        assert!(
            read_program_arguments(&fixture)
                .unwrap_err()
                .contains("both")
        );
        fs::write(
            fixture.sidecar("args-windows"),
            b"windows first\nwindows second\n",
        )
        .unwrap();

        #[cfg(unix)]
        assert_eq!(
            read_program_arguments(&fixture).unwrap(),
            ["unix first", "unix second"]
        );
        #[cfg(windows)]
        assert_eq!(
            read_program_arguments(&fixture).unwrap(),
            ["windows first", "windows second"]
        );
        remove_fixture(&fixture);
    }

    #[test]
    fn platform_arguments_are_runtime_only_and_canonical() {
        let fixture = temporary_fixture(FixtureKind::CompilePass);
        fs::write(fixture.sidecar("args-unix"), b"unix\n").unwrap();
        fs::write(fixture.sidecar("args-windows"), b"windows\n").unwrap();
        assert!(
            read_program_arguments(&fixture)
                .unwrap_err()
                .contains("outside a runtime fixture")
        );

        let runtime = Fixture {
            kind: FixtureKind::Runtime,
            source: fixture.source.clone(),
        };
        #[cfg(unix)]
        fs::write(runtime.sidecar("args-windows"), b"missing terminator").unwrap();
        #[cfg(windows)]
        fs::write(runtime.sidecar("args-unix"), b"missing terminator").unwrap();
        assert!(
            read_program_arguments(&runtime)
                .unwrap_err()
                .contains("LF-terminated")
        );
        remove_fixture(&fixture);
    }
}
