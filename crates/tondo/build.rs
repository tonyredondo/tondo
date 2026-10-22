//! Compiles the runtime as a staticlib and embeds it in the compiler, so a
//! single `tondo` binary can link native executables.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime = manifest.join("../tondo-runtime");
    println!("cargo:rerun-if-changed={}", runtime.join("src").display());
    println!("cargo:rerun-if-changed={}", runtime.join("Cargo.toml").display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let target = env::var("TARGET").unwrap();
    // Its own build directory, so this build does not wait for the one
    // that runs it.
    let target_dir = out_dir.join("runtime");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(&cargo)
        .args(["rustc", "--package", "tondo-runtime", "--lib", "--release", "--crate-type", "staticlib"])
        .args(["--target", &target, "--target-dir"])
        .arg(&target_dir)
        .args(["--", "-C", "panic=abort", "-C", "debuginfo=0", "--cfg", "tondo_staticlib"])
        .args(["--print", "native-static-libs"])
        .current_dir(&runtime)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("failed to run cargo for the Tondo runtime");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!("compiling the Tondo runtime failed:\n{stderr}");
    }
    let library = out_dir.join("libtondo_runtime.a");
    std::fs::copy(target_dir.join(&target).join("release/libtondo_runtime.a"), &library).unwrap();
    let link_args = stderr
        .lines()
        .find_map(|line| line.split_once("native-static-libs: ").map(|(_, libs)| libs.trim()))
        .unwrap_or("");
    std::fs::write(out_dir.join("runtime-link-args.txt"), link_args).unwrap();

    // With musl, Rust's runtime needs libunwind, which Rust ships for the
    // target and Alpine does not install: the compiler carries it.
    let unwind = out_dir.join("libunwind.a");
    let sysroot = Command::new(&rustc).args(["--print", "sysroot"]).output().ok();
    let shipped = sysroot.map(|o| {
        PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())
            .join(format!("lib/rustlib/{target}/lib/self-contained/libunwind.a"))
    });
    match shipped.filter(|path| target.contains("musl") && path.is_file()) {
        Some(path) => std::fs::copy(path, &unwind).map(|_| ()).unwrap(),
        None => std::fs::write(&unwind, b"").unwrap(),
    }

    // FNV-1a of the library identifies the runtime in compiler caches.
    let bytes = std::fs::read(&library).unwrap();
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    std::fs::write(out_dir.join("runtime-hash.txt"), format!("{hash:016x}")).unwrap();
}
