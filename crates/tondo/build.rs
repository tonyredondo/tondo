//! Compiles the runtime as a staticlib and embeds it in the compiler,
//! compressed, so a single `tondo` binary can link native executables. On
//! Linux it embeds as well what a static executable links (`--static`):
//! the runtime built for musl, and musl's C library, unwinder and start
//! file, which Rust ships for that target.

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime = manifest.join("../tondo-runtime");
    println!("cargo:rerun-if-changed={}", runtime.join("src").display());
    println!("cargo:rerun-if-changed={}", runtime.join("Cargo.toml").display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let target = env::var("TARGET").unwrap();
    let sysroot = Command::new(&rustc).args(["--print", "sysroot"]).output().ok();
    let sysroot = sysroot.map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())).unwrap_or_default();
    let self_contained = |target: &str| sysroot.join(format!("lib/rustlib/{target}/lib/self-contained"));

    let (library, link_args) = build_runtime(&runtime, &out_dir, &target);
    std::fs::write(out_dir.join("runtime-link-args.txt"), link_args).unwrap();
    let bytes = std::fs::read(&library).unwrap();
    compress(&bytes, &out_dir.join("libtondo_runtime.a.gz"));

    // With musl, Rust's runtime needs libunwind, which Rust ships for the
    // target and Alpine does not install: the compiler carries it.
    let unwind = self_contained(&target).join("libunwind.a");
    match unwind.is_file() && target.contains("musl") {
        true => compress(&std::fs::read(&unwind).unwrap(), &out_dir.join("libunwind.a.gz")),
        false => std::fs::write(out_dir.join("libunwind.a.gz"), b"").unwrap(),
    }

    // What a static executable links.
    let musl = target.replace("-gnu", "-musl");
    let parts = [
        ("libtondo_runtime-static.a.gz", None),
        ("libc-static.a.gz", Some("libc.a")),
        ("libunwind-static.a.gz", Some("libunwind.a")),
        ("rcrt1-static.o.gz", Some("rcrt1.o")),
    ];
    let available = target.contains("linux") && self_contained(&musl).join("libc.a").is_file();
    for (name, file) in parts {
        let path = out_dir.join(name);
        match (available, file) {
            (false, _) => std::fs::write(&path, b"").unwrap(),
            (true, Some(file)) => compress(&std::fs::read(self_contained(&musl).join(file)).unwrap(), &path),
            (true, None) if musl == target => compress(&bytes, &path),
            (true, None) => {
                let (library, _) = build_runtime(&runtime, &out_dir, &musl);
                compress(&std::fs::read(library).unwrap(), &path);
            }
        }
    }

    // FNV-1a of the library identifies the runtime in compiler caches.
    let hash = fnv(0xcbf2_9ce4_8422_2325, &bytes);
    std::fs::write(out_dir.join("runtime-hash.txt"), format!("{hash:016x}")).unwrap();

    // The runtime's source, with the compiler's version, identifies it on
    // every target: a target's runtime that another `tondo` packed works
    // with this one when they agree (toolchain 7.8).
    let mut files = Vec::new();
    let mut pending = vec![runtime.join("src")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { pending.push(path) } else { files.push(path) }
        }
    }
    files.push(runtime.join("Cargo.toml"));
    files.sort();
    let mut identity = fnv(0xcbf2_9ce4_8422_2325, env::var("CARGO_PKG_VERSION").unwrap().as_bytes());
    for file in files {
        identity = fnv(identity, file.strip_prefix(&runtime).unwrap().to_string_lossy().as_bytes());
        identity = fnv(identity, &std::fs::read(&file).unwrap());
    }
    std::fs::write(out_dir.join("runtime-identity.txt"), format!("{identity:016x}")).unwrap();
}

fn fnv(start: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(start, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Builds the runtime for `target`: the archive, and the native libraries
/// it needs as linker arguments.
fn build_runtime(runtime: &Path, out_dir: &Path, target: &str) -> (PathBuf, String) {
    // Its own build directory, so this build does not wait for the one
    // that runs it.
    let target_dir = out_dir.join("runtime");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(&cargo);
    command
        .args(["rustc", "--package", "tondo-runtime", "--lib", "--release", "--crate-type", "staticlib"])
        .args(["--target", target, "--target-dir"])
        .arg(&target_dir)
        // Frame pointers let a panic walk the stack through the runtime; one
        // codegen unit inlines across the whole runtime, so adding a module
        // does not change how another's hot paths are compiled; and each
        // function starts a cache line, so that changing one does not move
        // the others' code across lines.
        .args(["--", "-C", "panic=abort", "-C", "debuginfo=0", "-C", "force-frame-pointers=yes"])
        .args(["-C", "codegen-units=1", "-C", "llvm-args=-align-all-functions=6"])
        .args(["--cfg", "tondo_staticlib"])
        .args(["--print", "native-static-libs"])
        .current_dir(runtime)
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    // The C parts of the runtime's dependencies, for musl on a system
    // without a musl compiler: the system's, without glibc's fortified
    // functions, nor the C23 names its headers give functions, as
    // `__isoc23_strtol`, which musl does not have.
    let key = target.replace('-', "_");
    let compiler = format!("{}-linux-musl-gcc", target.split('-').next().unwrap_or_default());
    if target.contains("musl")
        && env::var_os(format!("CC_{key}")).is_none()
        && !on_path(&compiler)
        && !on_path("musl-gcc")
    {
        command.env(format!("CC_{key}"), "cc").env(format!("CFLAGS_{key}"), "-U_FORTIFY_SOURCE -std=gnu11");
    }
    let output = command.output().expect("failed to run cargo for the Tondo runtime");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!("compiling the Tondo runtime for {target} failed:\n{stderr}");
    }
    let link_args = stderr
        .lines()
        .find_map(|line| line.split_once("native-static-libs: ").map(|(_, libs)| libs.trim()))
        .unwrap_or("")
        .to_string();
    (target_dir.join(target).join("release/libtondo_runtime.a"), link_args)
}

fn on_path(program: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn compress(bytes: &[u8], path: &Path) {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(bytes).unwrap();
    std::fs::write(path, encoder.finish().unwrap()).unwrap();
}
