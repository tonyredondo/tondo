use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, WarningProfile, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::project::ProjectPlan;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
};

const EXIT_DIAGNOSTIC: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_INTERNAL: u8 = 3;

type PreparedCompilation = (CompilationRequest, Option<Arc<[u8]>>);

const USAGE: &str = "\
Tondo bootstrap toolchain

Usage:
  tondo <command> [--diagnostic-format <human|json>] [--warnings core] <source.to>
  tondo <check|run> [--diagnostic-format <human|json>] [--warnings core] --manifest <tondo.json>
  tondo run [--diagnostic-format <human|json>] [--warnings core] <source.to> -- [argument ...]

Commands:
  fmt      Format one Tondo source file
  check    Analyze one Tondo source file
  run      Compile and run one Tondo script

Options:
  --diagnostic-format <human|json>  Select diagnostic output
  --warnings <core>                 Enable a closed warning profile
  --check                           Verify formatting without writing output (fmt only)
  --manifest <path>                 Build a closed project manifest (check/run only)
  --lockfile <path>                 Use this lockfile (default: tondo.lock.json)
  --emit-interface <path>           Write the canonical compiled interface on success
  --emit-artifact <path>            Write canonical build metadata on success
  -- [argument ...]                 Pass UTF-8 arguments to a run script
  -h, --help                        Show this help
  -V, --version                     Show version information";

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("tondo: {error}");
            ExitCode::from(EXIT_INTERNAL)
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    match arguments.as_slice() {
        [argument] if argument == "-h" || argument == "--help" => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        [argument] if argument == "-V" || argument == "--version" => {
            println!(
                "tondo {} (language {}, backend {})",
                env!("CARGO_PKG_VERSION"),
                tondo_compiler::LANGUAGE_EDITION,
                tondo_vm::BACKEND_NAME,
            );
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    let invocation = match parse_invocation(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("tondo: {message}\n\n{USAGE}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let (request, original_source) = match compilation_request(&invocation) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("tondo: {message}");
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let request = request
        .with_warning_profiles(invocation.warning_profiles.iter().copied())
        .with_program_arguments(invocation.program_arguments.clone());
    let output = execute(request).map_err(|error| error.to_string())?;

    let format_check_failed = invocation.format_check
        && output.status() == CompilationStatus::Success
        && original_source
            .as_deref()
            .is_some_and(|bytes| output.stdout() != bytes);
    if !invocation.format_check {
        io::stdout()
            .write_all(output.stdout())
            .map_err(|error| format!("cannot write command output: {error}"))?;
    }
    emit_products(&invocation, &output)?;

    let rendered = match invocation.diagnostic_format {
        DiagnosticFormat::Human => output.diagnostics().human(),
        DiagnosticFormat::Json => output
            .diagnostics()
            .json_lines()
            .map_err(|error| error.to_string())?,
    };
    eprint!("{rendered}");

    Ok(if format_check_failed {
        ExitCode::from(EXIT_DIAGNOSTIC)
    } else {
        ExitCode::from(output.exit_code())
    })
}

#[derive(Debug)]
struct Invocation {
    operation: Operation,
    source_form: SourceForm,
    diagnostic_format: DiagnosticFormat,
    warning_profiles: BTreeSet<WarningProfile>,
    format_check: bool,
    source: Option<PathBuf>,
    manifest: Option<PathBuf>,
    lockfile: Option<PathBuf>,
    emit_interface: Option<PathBuf>,
    emit_artifact: Option<PathBuf>,
    program_arguments: Vec<String>,
}

fn parse_invocation(arguments: &[OsString]) -> Result<Invocation, String> {
    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Err("a UTF-8 command is required".into());
    };
    let (operation, source_form) = match command {
        "fmt" => (Operation::Format, SourceForm::Module),
        "check" => (Operation::Check, SourceForm::Module),
        "run" => (Operation::Run, SourceForm::Script),
        _ => return Err(format!("unknown command `{command}`")),
    };

    let mut diagnostic_format = DiagnosticFormat::Human;
    let mut warning_profiles = BTreeSet::new();
    let mut format_check = false;
    let mut source: Option<PathBuf> = None;
    let mut manifest: Option<PathBuf> = None;
    let mut lockfile: Option<PathBuf> = None;
    let mut emit_interface: Option<PathBuf> = None;
    let mut emit_artifact: Option<PathBuf> = None;
    let mut program_arguments = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            if operation != Operation::Run {
                return Err("program arguments are only valid with `tondo run`".into());
            }
            if source.is_none() && manifest.is_none() {
                return Err("the source file or manifest must appear before `--`".into());
            }
            program_arguments = arguments[index + 1..]
                .iter()
                .map(|argument| {
                    argument
                        .clone()
                        .into_string()
                        .map_err(|_| "program arguments must be valid UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            break;
        } else if argument == "--diagnostic-format" {
            index += 1;
            let Some(value) = arguments.get(index).and_then(|value| value.to_str()) else {
                return Err("`--diagnostic-format` requires `human` or `json`".into());
            };
            diagnostic_format = parse_diagnostic_format(value)?;
        } else if argument == "--check" {
            if operation != Operation::Format {
                return Err("`--check` is only valid with `tondo fmt`".into());
            }
            format_check = true;
        } else if argument == "--warnings" {
            index += 1;
            let Some(value) = arguments.get(index).and_then(|value| value.to_str()) else {
                return Err("`--warnings` requires `core`".into());
            };
            warning_profiles.insert(parse_warning_profile(value)?);
        } else if argument == "--manifest" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--manifest` requires a path".into());
            };
            if manifest.replace(PathBuf::from(value)).is_some() {
                return Err("`--manifest` may appear only once".into());
            }
        } else if argument == "--lockfile" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--lockfile` requires a path".into());
            };
            if lockfile.replace(PathBuf::from(value)).is_some() {
                return Err("`--lockfile` may appear only once".into());
            }
        } else if argument == "--emit-interface" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--emit-interface` requires a path".into());
            };
            if emit_interface.replace(PathBuf::from(value)).is_some() {
                return Err("`--emit-interface` may appear only once".into());
            }
        } else if argument == "--emit-artifact" {
            index += 1;
            let Some(value) = arguments.get(index) else {
                return Err("`--emit-artifact` requires a path".into());
            };
            if emit_artifact.replace(PathBuf::from(value)).is_some() {
                return Err("`--emit-artifact` may appear only once".into());
            }
        } else if let Some(argument) = argument.to_str() {
            if let Some(value) = argument.strip_prefix("--diagnostic-format=") {
                diagnostic_format = parse_diagnostic_format(value)?;
            } else if let Some(value) = argument.strip_prefix("--warnings=") {
                warning_profiles.insert(parse_warning_profile(value)?);
            } else if argument.starts_with('-') {
                return Err(format!("unknown option `{argument}`"));
            } else if source.replace(PathBuf::from(argument)).is_some() {
                return Err("bootstrap commands accept exactly one source file".into());
            }
        } else if source.replace(PathBuf::from(argument)).is_some() {
            return Err("bootstrap commands accept exactly one source file".into());
        }
        index += 1;
    }

    if source.is_some() && manifest.is_some() {
        return Err("choose either one source file or `--manifest`, not both".into());
    }
    if source.is_none() && manifest.is_none() {
        return Err("a source file is required (or use `--manifest` for a project)".into());
    }
    if operation == Operation::Format && manifest.is_some() {
        return Err("`tondo fmt` accepts a source file, not a project manifest".into());
    }
    if operation == Operation::Format && (emit_interface.is_some() || emit_artifact.is_some()) {
        return Err("build products are only available from `check` or `run`".into());
    }
    if operation == Operation::Format && !warning_profiles.is_empty() {
        return Err("warning profiles are only available from `check` or `run`".into());
    }
    if lockfile.is_some() && manifest.is_none() {
        return Err("`--lockfile` requires `--manifest`".into());
    }
    if let Some(source) = &source {
        validate_source_extension(source)?;
        if source.file_name().and_then(OsStr::to_str).is_none() {
            return Err("source filename is not valid UTF-8".into());
        }
    }
    if let Some(manifest_path) = &manifest {
        let resolved_lockfile = lockfile.get_or_insert_with(|| {
            manifest_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join("tondo.lock.json")
        });
        for output in [&emit_interface, &emit_artifact].into_iter().flatten() {
            if paths_refer_to_same_location(output, manifest_path)
                || paths_refer_to_same_location(output, resolved_lockfile)
            {
                return Err(
                    "an emitted product must not overwrite the manifest or lockfile".into(),
                );
            }
        }
    }
    if let (Some(interface), Some(artifact)) = (&emit_interface, &emit_artifact)
        && paths_refer_to_same_location(interface, artifact)
    {
        return Err("interface and artifact outputs require distinct paths".into());
    }
    if let Some(source_path) = &source {
        for output in [&emit_interface, &emit_artifact].into_iter().flatten() {
            if paths_refer_to_same_location(output, source_path) {
                return Err("an emitted product must not overwrite the source file".into());
            }
        }
    }
    Ok(Invocation {
        operation,
        source_form,
        diagnostic_format,
        warning_profiles,
        format_check,
        source,
        manifest,
        lockfile,
        emit_interface,
        emit_artifact,
        program_arguments,
    })
}

fn compilation_request(invocation: &Invocation) -> Result<PreparedCompilation, String> {
    if let Some(manifest_path) = &invocation.manifest {
        let lockfile_path = invocation
            .lockfile
            .as_ref()
            .expect("parse_invocation resolves the default lockfile");
        let manifest = read_input(manifest_path, "manifest")?;
        let lockfile = read_input(lockfile_path, "lockfile")?;
        let plan = ProjectPlan::parse(&manifest, &lockfile).map_err(|error| error.to_string())?;
        let base = manifest_path.parent().unwrap_or_else(|| Path::new(""));
        let mut supplied = BTreeMap::new();
        for input in plan.required_inputs() {
            let physical = base.join(input.path());
            reject_product_input_collision(invocation, &physical, input.path())?;
            let bytes = read_input(
                &physical,
                &format!("{} input `{}`", input.kind().as_str(), input.path()),
            )?;
            supplied.insert(input.path().to_owned(), Arc::<[u8]>::from(bytes));
        }
        let request = plan
            .resolve(&supplied)
            .map_err(|error| error.to_string())?
            .into_compilation_request(
                invocation.operation,
                invocation.diagnostic_format,
                ResourceLimits::default(),
            )
            .map_err(|error| error.to_string())?;
        return Ok((request, None));
    }

    let source = invocation
        .source
        .as_ref()
        .expect("parse_invocation requires a source or manifest");
    let bytes = Arc::<[u8]>::from(read_input(source, "source")?);
    let file_name = source
        .file_name()
        .and_then(OsStr::to_str)
        .expect("parse_invocation validated the UTF-8 source filename");
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::new(
            SourceId::new("root:cli").map_err(|error| error.to_string())?,
            ModulePath::new("main").map_err(|error| error.to_string())?,
            LogicalPath::new(file_name).map_err(|error| error.to_string())?,
            SourceOrigin::Physical,
            bytes.clone(),
        ))
        .map_err(|error| error.to_string())?;
    let request = CompilationRequest::new(
        invocation.operation,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BuildTarget::vm_hosted_capabilities(),
        invocation.diagnostic_format,
        invocation.source_form,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?,
        sources,
        root,
    )
    .map_err(|error| error.to_string())?;
    Ok((request, Some(bytes)))
}

fn reject_product_input_collision(
    invocation: &Invocation,
    physical_input: &Path,
    logical_input: &str,
) -> Result<(), String> {
    for output in [&invocation.emit_interface, &invocation.emit_artifact]
        .into_iter()
        .flatten()
    {
        if paths_refer_to_same_location(output, physical_input) {
            return Err(format!(
                "an emitted product must not overwrite project input `{logical_input}`"
            ));
        }
    }
    Ok(())
}

fn paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (fs::canonicalize(left), fs::canonicalize(right)) {
        return left == right;
    }
    match (
        normalized_absolute_path(left),
        normalized_absolute_path(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn normalized_absolute_path(path: &Path) -> Option<PathBuf> {
    let path = std::path::absolute(path).ok()?;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Some(normalized)
}

fn read_input(path: &Path, description: &str) -> Result<Vec<u8>, String> {
    fs::read(path)
        .map_err(|error| format!("cannot read {description} `{}`: {error}", path.display()))
}

fn emit_products(
    invocation: &Invocation,
    output: &tondo_compiler::driver::CompilationOutput,
) -> Result<(), String> {
    if output.status() != CompilationStatus::Success {
        return Ok(());
    }
    if let Some(path) = &invocation.emit_interface {
        let interface = output
            .interface()
            .ok_or_else(|| "successful compilation produced no interface".to_owned())?;
        let bytes = interface.encode().map_err(|error| error.to_string())?;
        fs::write(path, bytes)
            .map_err(|error| format!("cannot write interface `{}`: {error}", path.display()))?;
    }
    if let Some(path) = &invocation.emit_artifact {
        let artifact = output
            .artifact()
            .ok_or_else(|| "successful compilation produced no build artifact".to_owned())?;
        let bytes = artifact.encode().map_err(|error| error.to_string())?;
        fs::write(path, bytes)
            .map_err(|error| format!("cannot write artifact `{}`: {error}", path.display()))?;
    }
    Ok(())
}

fn parse_diagnostic_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "json" => Ok(DiagnosticFormat::Json),
        _ => Err(format!(
            "unknown diagnostic format `{value}`; expected `human` or `json`"
        )),
    }
}

fn parse_warning_profile(value: &str) -> Result<WarningProfile, String> {
    match value {
        "core" => Ok(WarningProfile::Core),
        _ => Err(format!(
            "unknown warning profile `{value}`; expected `core`"
        )),
    }
}

fn validate_source_extension(path: &Path) -> Result<(), String> {
    if path.extension() == Some(OsStr::new("to")) {
        Ok(())
    } else {
        Err("source file must use the `.to` extension".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation_error(values: &[&str]) -> String {
        parse_invocation(&arguments(values)).unwrap_err()
    }

    #[test]
    fn parses_json_diagnostics_in_either_option_form() {
        for arguments in [
            vec!["check", "--diagnostic-format", "json", "main.to"],
            vec!["check", "--diagnostic-format=json", "main.to"],
        ] {
            let arguments = arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            let invocation = parse_invocation(&arguments).unwrap();
            assert_eq!(invocation.diagnostic_format, DiagnosticFormat::Json);
        }
    }

    #[test]
    fn rejects_multiple_sources() {
        let arguments = ["check", "one.to", "two.to"].map(OsString::from).to_vec();
        assert!(parse_invocation(&arguments).is_err());
    }

    #[test]
    fn format_check_flag_is_scoped_to_the_formatter() {
        let format = ["fmt", "--check", "main.to"].map(OsString::from).to_vec();
        assert!(parse_invocation(&format).unwrap().format_check);

        for command in ["check", "run"] {
            let arguments = [command, "--check", "main.to"].map(OsString::from).to_vec();
            assert!(parse_invocation(&arguments).is_err());
        }
    }

    #[test]
    fn run_preserves_arguments_after_separator() {
        let arguments = ["run", "main.to", "--", "--flag", "two words"]
            .map(OsString::from)
            .to_vec();
        let invocation = parse_invocation(&arguments).unwrap();
        assert_eq!(invocation.program_arguments, ["--flag", "two words"]);
    }

    #[test]
    fn non_run_commands_reject_program_arguments() {
        for command in ["fmt", "check"] {
            let arguments = [command, "main.to", "--", "argument"]
                .map(OsString::from)
                .to_vec();
            assert!(parse_invocation(&arguments).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_non_utf8_program_arguments() {
        use std::os::unix::ffi::OsStringExt;

        let arguments = vec![
            OsString::from("run"),
            OsString::from("main.to"),
            OsString::from("--"),
            OsString::from_vec(vec![0xff]),
        ];
        assert!(
            parse_invocation(&arguments)
                .unwrap_err()
                .contains("valid UTF-8")
        );
    }

    #[test]
    fn project_products_cannot_overwrite_a_declared_input() {
        let arguments = [
            "check",
            "--manifest",
            "project/tondo.json",
            "--emit-interface",
            "project/src/main.to",
        ]
        .map(OsString::from)
        .to_vec();
        let invocation = parse_invocation(&arguments).unwrap();
        assert!(matches!(
            reject_product_input_collision(
                &invocation,
                Path::new("project/src/main.to"),
                "src/main.to"
            ),
            Err(message) if message.contains("must not overwrite project input")
        ));
        assert!(paths_refer_to_same_location(
            Path::new("project/build/../src/main.to"),
            Path::new("project/src/main.to")
        ));
        assert!(paths_refer_to_same_location(
            Path::new("project/out/../interface.ti"),
            Path::new("project/interface.ti")
        ));
    }

    #[test]
    fn invocation_rejects_every_ambiguous_or_incomplete_cli_shape() {
        let invalid = [
            (&[][..], "UTF-8 command"),
            (&["unknown", "main.to"], "unknown command"),
            (&["run", "--"], "must appear before `--`"),
            (
                &["check", "--diagnostic-format"],
                "`--diagnostic-format` requires",
            ),
            (
                &["check", "--diagnostic-format", "xml", "main.to"],
                "unknown diagnostic format",
            ),
            (&["check", "--warnings"], "`--warnings` requires"),
            (
                &["check", "--warnings", "all", "main.to"],
                "unknown warning profile",
            ),
            (&["check", "--manifest"], "`--manifest` requires"),
            (
                &["check", "--manifest", "one.json", "--manifest", "two.json"],
                "`--manifest` may appear only once",
            ),
            (&["check", "--lockfile"], "`--lockfile` requires"),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--lockfile",
                    "one.lock",
                    "--lockfile",
                    "two.lock",
                ],
                "`--lockfile` may appear only once",
            ),
            (
                &["check", "--emit-interface"],
                "`--emit-interface` requires",
            ),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-interface",
                    "one.ti",
                    "--emit-interface",
                    "two.ti",
                ],
                "`--emit-interface` may appear only once",
            ),
            (&["check", "--emit-artifact"], "`--emit-artifact` requires"),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-artifact",
                    "one.ta",
                    "--emit-artifact",
                    "two.ta",
                ],
                "`--emit-artifact` may appear only once",
            ),
            (&["check", "--unknown", "main.to"], "unknown option"),
            (
                &["check", "main.to", "--manifest", "tondo.json"],
                "choose either",
            ),
            (
                &["fmt", "--manifest", "tondo.json"],
                "accepts a source file",
            ),
            (
                &["fmt", "main.to", "--emit-interface", "main.ti"],
                "build products",
            ),
            (&["fmt", "--warnings=core", "main.to"], "warning profiles"),
            (
                &["check", "--lockfile", "tondo.lock.json", "main.to"],
                "requires `--manifest`",
            ),
            (&["check", "main.tondo"], "`.to` extension"),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--emit-interface",
                    "tondo.json",
                ],
                "must not overwrite the manifest or lockfile",
            ),
            (
                &[
                    "check",
                    "--manifest",
                    "tondo.json",
                    "--lockfile",
                    "custom.lock",
                    "--emit-artifact",
                    "custom.lock",
                ],
                "must not overwrite the manifest or lockfile",
            ),
            (
                &[
                    "check",
                    "main.to",
                    "--emit-interface",
                    "product",
                    "--emit-artifact",
                    "product",
                ],
                "distinct paths",
            ),
            (
                &["check", "main.to", "--emit-artifact", "main.to"],
                "must not overwrite the source file",
            ),
        ];

        for (values, expected) in invalid {
            let error = invocation_error(values);
            assert!(
                error.contains(expected),
                "`{values:?}` returned unexpected error: {error}"
            );
        }

        for values in [
            &["check", "--diagnostic-format", "human", "main.to"][..],
            &["check", "--warnings", "core", "main.to"][..],
            &["check", "--warnings=core", "main.to"][..],
        ] {
            parse_invocation(&arguments(values)).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_commands_and_source_names_are_rejected_at_their_boundaries() {
        use std::os::unix::ffi::OsStringExt;

        assert!(
            parse_invocation(&[OsString::from_vec(vec![0xff])])
                .unwrap_err()
                .contains("UTF-8 command")
        );
        let invalid_name = OsString::from_vec(vec![0xff, b'.', b't', b'o']);
        assert!(
            parse_invocation(&[OsString::from("check"), invalid_name.clone()])
                .unwrap_err()
                .contains("filename is not valid UTF-8")
        );
        assert!(
            parse_invocation(&[
                OsString::from("check"),
                OsString::from("main.to"),
                invalid_name,
            ])
            .unwrap_err()
            .contains("exactly one source file")
        );
    }
}
