//! Targets (toolchain 7.8): `--target` builds a static executable for Linux
//! on x86-64 or arm64; `tondo target pack` writes the runtime of this
//! machine's target, which a `tondo` elsewhere builds for it with. Where
//! `TONDO_TARGETS` has the other architecture's pack, its programs build
//! and run here too, under qemu.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

const PROGRAM: &str = "import std.console\n\nfn main() {\n    console.println(\"built for {6 * 7}\")\n}\n";

fn tondo(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap()
}

#[test]
fn programs_build_and_run_for_targets() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("targets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("built.to"), PROGRAM).unwrap();
    let (machine, other) = match cfg!(target_arch = "aarch64") {
        true => ("aarch64-linux", "x86_64-linux"),
        false => ("x86_64-linux", "aarch64-linux"),
    };

    // This machine's target: static, with no loader.
    let built = tondo(&dir, &["build", "built.to", "--target", machine, "--output", "built"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let bytes = std::fs::read(dir.join("built")).unwrap();
    assert!(!bytes.windows(9).any(|w| w == b"ld-linux-"), "a static executable names no loader");
    let run = Command::new(dir.join("built")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "built for 42\n");

    // Its pack.
    let packed = tondo(&dir, &["target", "pack", "packs"]);
    assert!(packed.status.success(), "{}", String::from_utf8_lossy(&packed.stderr));
    for file in ["runtime.a", "libunwind.a", "libc.a", "rcrt1.o", "identity"] {
        assert!(dir.join("packs").join(machine).join(file).is_file(), "{file}");
    }

    // The other architecture, without its pack and with it.
    let missing = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "built.to", "--target", other, "--output", "other"])
        .current_dir(&dir)
        .env("TONDO_TARGETS", dir.join("packs"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&missing.stderr).contains("tondo target pack"));
    if std::env::var_os("TONDO_TARGETS").is_some() {
        let ran = tondo(&dir, &["run", "built.to", "--target", other]);
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "built for 42\n", "{}", String::from_utf8_lossy(&ran.stderr));
    }
}

/// A program checks at its start that the processor has what its level
/// needs (toolchain 7.8): under qemu, told to be a processor without AVX2,
/// one built for x86-64-v3 ends with the line that says so. Where nothing
/// runs x86-64 code under qemu here, there is nothing to tell.
#[test]
fn a_program_checks_its_processor() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("levels");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("built.to"), PROGRAM).unwrap();
    let qemu = ["qemu-x86_64", "qemu-x86_64-static"].into_iter().find_map(|name| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path).map(|dir| dir.join(name)).find(|path| path.is_file())
    });
    // This machine's x86-64 code, or another's, which binfmt hands to qemu.
    let emulated = cfg!(target_arch = "aarch64") && std::env::var_os("TONDO_TARGETS").is_some();
    if qemu.is_none() && !emulated {
        return;
    }
    let mut built = Command::new(env!("CARGO_BIN_EXE_tondo"));
    built.args(["build", "built.to", "--cpu", "x86-64-v3", "--output", "v3"]).current_dir(&dir);
    if emulated {
        built.args(["--target", "x86_64-linux"]);
    }
    let built = built.output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let mut run = match &qemu {
        Some(qemu) if !emulated => Command::new(qemu),
        _ => Command::new(dir.join("v3")),
    };
    if !emulated {
        run.arg(dir.join("v3"));
    }
    let ran = run.env("QEMU_CPU", "Nehalem").output().unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(101), "{said}");
    assert_eq!(
        said,
        "fatal: this program was built for x86-64-v3 and this processor lacks AVX, AVX2, FMA, BMI1, BMI2 and LZCNT: build it with --cpu baseline\n"
    );
    assert!(ran.stdout.is_empty());
}
