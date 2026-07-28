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
import std.process

fn main() {
    assert(process.args() == ["--flag", "two words", "*", "$HOME"])
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
