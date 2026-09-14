//! Source-bound scalar execution without the evaluation runtime. The C file
//! only enters compiler-produced functions and prints their observed result.

use super::*;

pub(super) fn run(
    options: &Options,
    probe: &ProbeReport,
    probe_bytes: &[u8],
) -> Result<(), String> {
    let cc = options
        .cc
        .as_ref()
        .ok_or("--source-scalars requires --cc")?;
    if options.target != "x86_64-unknown-linux-gnu"
        || cranelift_isa()?.triple().to_string() != options.target
    {
        return Err("source scalar execution requires the x86_64 GNU Linux host target".into());
    }
    if options.std_core_probe.is_some() || options.aot_performance_output.is_some() {
        return Err(
            "source scalar execution does not generate promotion or performance reports".into(),
        );
    }
    if probe.format != "tondo-native-mir-probe/1" || probe.fixtures.len() != 1 {
        return Err(
            "source scalar execution requires exactly one compiler-produced fixture".into(),
        );
    }
    let fixture = &probe.fixtures[0];
    let program = fixture
        .mir
        .as_ref()
        .and_then(|mir| mir.backend.as_ref())
        .ok_or("source scalar fixture has no MIR")?;
    validate_backend_program(program)?;
    if fixture.status != "passed"
        || program.functions.iter().any(|function| {
            !function.supported
                && !matches!(function.generics, Some(MirBackendGenerics::Template { .. }))
        })
    {
        return Err(
            "source scalar execution requires every concrete fixture function to be supported"
                .into(),
        );
    }
    let source_path = Path::new(&fixture.fixture);
    if source_path.is_absolute()
        || source_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("source scalar fixture must use a logical relative path".into());
    }
    let source =
        fs::read(source_path).map_err(|error| format!("cannot read source fixture: {error}"))?;
    let source_hash = sha256_bytes(&source);
    let debug = program
        .debug
        .as_ref()
        .ok_or("source fixture has no debug metadata")?;
    if fixture.fixture_sha256 != source_hash
        || debug.sources.len() != 1
        || debug.sources[0].logical_path != fixture.fixture
        || debug.sources[0].content_sha256 != source_hash
        || debug.sources[0].length as usize != source.len()
    {
        return Err(
            "source scalar fixture or debug identity differs from actual source bytes".into(),
        );
    }

    let object = options.temp_dir.join("source-scalars.o");
    emit_cranelift_object(cranelift_isa()?, program, &object)?;
    let llvm = if options.llvm.as_os_str().is_empty() {
        None
    } else {
        compile_llvm(
            &options.llvm,
            &options.target,
            &options.temp_dir,
            fixture,
            program,
        )?;
        Some((
            options
                .temp_dir
                .join(format!("{}.o", safe_stem(&fixture.fixture))),
            command_version(&options.llvm)?,
        ))
    };
    let mut observations = Vec::new();
    let mut llvm_observations = Vec::new();
    for function in &program.functions {
        if matches!(function.generics, Some(MirBackendGenerics::Template { .. })) {
            continue;
        }
        if function.return_type != "Int" || function.parameter_types.iter().any(|ty| ty != "Int") {
            continue;
        }
        for (case, arguments) in scalar_case_arguments_for_function(function)
            .into_iter()
            .enumerate()
        {
            let vm = fixture
                .vm_scalar
                .iter()
                .find(|observation| {
                    observation.function_ordinal == function.ordinal
                        && observation.arguments == arguments
                })
                .ok_or("source scalar case lacks an independent hosted VM observation")?;
            let oracle = evaluate_scalar_program(program, function.ordinal, &arguments);
            let expected = match oracle {
                Ok(value)
                    if vm.status == "returned"
                        && vm.result == Some(value)
                        && vm.diagnostics.is_empty() =>
                {
                    Some(value)
                }
                Err(_) if vm.status == "panicked" && vm.result.is_none() => None,
                observed => return Err(format!(
                    "normalized MIR and hosted VM observations disagree for function {} with {:?}: MIR {:?}, VM {} {:?}",
                    function.ordinal, arguments, observed, vm.status, vm.result,
                )),
            };
            let stem = format!("source-{}-{case}", function.ordinal);
            let harness = harness_source(function, &arguments);
            let observed = observe(options, cc, &object, &stem, &harness, expected)?;
            if let Some((object, _)) = &llvm {
                let result = observe(
                    options,
                    cc,
                    object,
                    &format!("{stem}-llvm"),
                    &harness,
                    expected,
                )?;
                llvm_observations.push(serde_json::json!({
                    "function_ordinal": function.ordinal,
                    "arguments": arguments,
                    "native_status": if result.is_some() { "returned" } else { "trapped" },
                    "native_result": result,
                }));
            }
            observations.push(serde_json::json!({
                "function_ordinal": function.ordinal,
                "arguments": arguments,
                "vm_status": vm.status,
                "vm_result": vm.result,
                "native_status": if observed.is_some() { "returned" } else { "trapped" },
                "native_result": observed,
            }));
        }
    }
    if observations.is_empty() {
        return Err("source scalar execution observed no cases".into());
    }
    let mut report = serde_json::json!({
        "format": "tondo-native-source-scalars/1",
        "backend": "cranelift",
        "cranelift_version": CRANELIFT_VERSION,
        "target": options.target,
        "boundary": "source-driven-runtime-free-scalar-functions",
        "n1_claim": false,
        "production_runtime_linked": false,
        "fixture": fixture.fixture,
        "fixture_sha256": source_hash,
        "probe_sha256": sha256_bytes(probe_bytes),
        "observations": observations,
    });
    if let Some((_, version)) = llvm {
        report["llvm_comparison"] = serde_json::json!({
            "version": version,
            "observations": llvm_observations,
        });
    }
    fs::write(
        &options.output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| format!("cannot write source scalar report: {error}"))
}

fn observe(
    options: &Options,
    cc: &Path,
    object: &Path,
    stem: &str,
    harness_source: &str,
    expected: Option<i64>,
) -> Result<Option<i64>, String> {
    let harness = options.temp_dir.join(format!("{stem}.c"));
    let binary = options.temp_dir.join(format!("{stem}.bin"));
    fs::write(&harness, harness_source)
        .map_err(|error| format!("cannot write scalar entry harness: {error}"))?;
    // Linking fails if code generation still needs any runtime symbol.
    link_native_runner(cc, &harness, object, &binary)?;
    let (status, output) = execute_case(&binary)?;
    if let Some(expected) = expected {
        if !status.success() {
            return Err(format!(
                "source scalar case {stem} unexpectedly failed: {status}"
            ));
        }
        let value = std::str::from_utf8(&output)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .ok_or("native scalar output is not an integer")?;
        if value != expected {
            return Err(format!(
                "source scalar case {stem} differs: native {value}, VM {expected}"
            ));
        }
        Ok(Some(value))
    } else if is_arithmetic_trap(status) && output.is_empty() {
        Ok(None)
    } else {
        Err(format!(
            "source scalar case {stem} did not produce the expected arithmetic trap: {status}"
        ))
    }
}

fn harness_source(function: &MirBackendFunction, arguments: &[i64]) -> String {
    let parameters = if arguments.is_empty() {
        "void".into()
    } else {
        vec!["int64_t"; arguments.len()].join(", ")
    };
    let arguments = arguments
        .iter()
        .map(|value| {
            if *value == i64::MIN {
                "(-INT64_C(9223372036854775807) - 1)".into()
            } else {
                format!("INT64_C({value})")
            }
        })
        .collect::<Vec<String>>()
        .join(", ");
    format!(
        "#include <stdint.h>\n#include <stdio.h>\n\
         extern int64_t tondo_probe_{ordinal}({parameters});\n\
         int64_t tondo_explicit_panic(void) {{ __builtin_trap(); }}\n\
         int main(void) {{ int64_t result = tondo_probe_{ordinal}({arguments});\n\
         printf(\"%lld\\n\", (long long)result); return 0; }}\n",
        ordinal = function.ordinal,
    )
}

fn execute_case(binary: &Path) -> Result<(std::process::ExitStatus, Vec<u8>), String> {
    let mut child = Command::new(binary)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot execute source scalar case: {error}"))?;
    let started = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for source scalar case: {error}"))?
        {
            Some(status) => {
                let mut bytes = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or("source scalar output pipe is missing")?
                    .take(64)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                return Ok((status, bytes));
            }
            None if started.elapsed() >= MAX_NATIVE_CASE_RUNTIME => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("source scalar case exceeded its runtime budget".into());
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn is_arithmetic_trap(status: std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(4)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}
