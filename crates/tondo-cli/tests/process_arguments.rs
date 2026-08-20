use std::fs;
use std::process::Command;

#[test]
fn run_forwards_only_arguments_after_the_separator() {
    let source =
        std::env::temp_dir().join(format!("tondo-process-arguments-{}.to", std::process::id()));
    fs::write(
        &source,
        br#"
import std.console
import std.env

fn text(value: env.Value): String {
    match value.asText() {
        some(argumentText) => argumentText
        none => panic("argument is not UTF-8")
    }
}

fn main(): !env.EnvError {
    let arguments = env.snapshot()?.arguments()
    assert(text(arguments[0]) == "--flag")
    assert(text(arguments[1]) == "two words")
    assert(text(arguments[2]) == "*")
    assert(text(arguments[3]) == "$HOME")
    console.print("cli-args-ok\n")
}
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("run")
        .arg(&source)
        .arg("--")
        .args(["--flag", "two words", "*", "$HOME"])
        .output()
        .unwrap();
    let _ = fs::remove_file(source);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"cli-args-ok\n");
    assert!(output.stderr.is_empty());
}
