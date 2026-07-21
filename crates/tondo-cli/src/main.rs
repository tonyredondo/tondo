use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{
    LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
};

const EXIT_DIAGNOSTIC: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_INTERNAL: u8 = 3;

const USAGE: &str = "\
Tondo bootstrap toolchain

Usage:
  tondo <command> [--diagnostic-format <human|json>] <source.to>

Commands:
  fmt      Format one Tondo source file
  check    Analyze one Tondo source file
  run      Compile and run one Tondo script

Options:
  --diagnostic-format <human|json>  Select diagnostic output
  --check                           Verify formatting without writing output (fmt only)
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
    let bytes = match fs::read(&invocation.source) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "tondo: cannot read `{}`: {error}",
                invocation.source.display()
            );
            return Ok(ExitCode::from(EXIT_USAGE));
        }
    };
    let bytes = Arc::<[u8]>::from(bytes);
    let file_name = invocation
        .source
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
    let output = execute(request).map_err(|error| error.to_string())?;

    let format_check_failed = invocation.format_check
        && output.status() == CompilationStatus::Success
        && output.stdout() != bytes.as_ref();
    if !invocation.format_check {
        io::stdout()
            .write_all(output.stdout())
            .map_err(|error| format!("cannot write command output: {error}"))?;
    }

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
    format_check: bool,
    source: PathBuf,
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
    let mut format_check = false;
    let mut source: Option<PathBuf> = None;
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--diagnostic-format" {
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
        } else if let Some(argument) = argument.to_str() {
            if let Some(value) = argument.strip_prefix("--diagnostic-format=") {
                diagnostic_format = parse_diagnostic_format(value)?;
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

    let source = source.ok_or_else(|| "a source file is required".to_owned())?;
    validate_source_extension(&source)?;
    if source.file_name().and_then(OsStr::to_str).is_none() {
        return Err("source filename is not valid UTF-8".into());
    }
    Ok(Invocation {
        operation,
        source_form,
        diagnostic_format,
        format_check,
        source,
    })
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
}
