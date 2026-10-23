//! Every example of `tondo guide` compiles, and every example of a mistake
//! fails with the error it names.

use std::path::Path;
use std::process::Command;

/// The examples of a topic: their info string and their code.
fn examples(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.strip_prefix("```") else { continue };
        let code: Vec<&str> = lines.by_ref().take_while(|l| !l.starts_with("```")).collect();
        if info.starts_with("tondo") {
            out.push((info.to_string(), code.join("\n") + "\n"));
        }
    }
    out
}

#[test]
fn examples_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("guide");
    let _ = std::fs::remove_dir_all(&dir);
    let mut jobs = Vec::new();
    for (topic, text) in tondo::guide::TOPICS {
        for (index, (info, code)) in examples(text).into_iter().enumerate() {
            let case = dir.join(format!("{topic}-{index}"));
            std::fs::create_dir_all(&case).unwrap();
            std::fs::write(case.join("main.to"), &code).unwrap();
            jobs.push((format!("{topic} example {index} ({info})"), info, case, code));
        }
    }
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(8)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for (name, info, case, code) in chunk {
                        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                            .args(["check", "main.to"])
                            .current_dir(case)
                            .output()
                            .unwrap();
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let expected = info.strip_prefix("tondo error ").map(str::trim);
                        let ok = match expected {
                            None => output.status.success(),
                            Some(code) => !output.status.success() && stderr.contains(&format!("error[{code}]")),
                        };
                        if !ok {
                            failures.push(format!("{name}:\n{code}\n{stderr}"));
                        }
                    }
                    failures
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
