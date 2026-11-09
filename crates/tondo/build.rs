//! Compiles the runtime as a staticlib and embeds it in the compiler,
//! compressed, so a single `tondo` binary can link native executables. On
//! Linux it embeds as well what a static executable links (`--static`):
//! the runtime built for musl, and musl's C library, unwinder and start
//! file, which Rust ships for that target.
//!
//! The runtime is built in `<target-dir>/tondo-runtime`, not under
//! `OUT_DIR`, which is another for each way of compiling `tondo` (its
//! tests, clippy, release) and built the runtime again for each; what a
//! build makes is kept there by a key of everything that makes it, which
//! CI keeps between runs.

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime = manifest.join("../tondo-runtime");
    for part in ["src", "data", "build.rs", "Cargo.toml"] {
        println!("cargo:rerun-if-changed={}", runtime.join(part).display());
    }
    // The versions of the runtime's dependencies.
    println!("cargo:rerun-if-changed={}", manifest.join("../../Cargo.lock").display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let target = env::var("TARGET").unwrap();
    let sysroot = Command::new(&rustc).args(["--print", "sysroot"]).output().ok();
    let sysroot = sysroot.map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())).unwrap_or_default();
    let self_contained = |target: &str| sysroot.join(format!("lib/rustlib/{target}/lib/self-contained"));

    // The runtime's source, with the compiler's version, identifies it on
    // every target: a target's runtime that another `tondo` packed works
    // with this one when they agree (toolchain 7.8).
    let mut files = Vec::new();
    let mut pending = vec![runtime.join("src"), runtime.join("data")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { pending.push(path) } else { files.push(path) }
        }
    }
    files.extend([runtime.join("build.rs"), runtime.join("Cargo.toml")]);
    files.sort();
    let mut identity = fnv(0xcbf2_9ce4_8422_2325, env::var("CARGO_PKG_VERSION").unwrap().as_bytes());
    for file in files {
        identity = fnv(identity, file.strip_prefix(&runtime).unwrap().to_string_lossy().as_bytes());
        identity = fnv(identity, &std::fs::read(&file).unwrap());
    }
    std::fs::write(out_dir.join("runtime-identity.txt"), format!("{identity:016x}")).unwrap();

    // What else makes the runtime what it is, besides its source: the
    // versions of its dependencies, the toolchains of Rust and C, and the
    // environment they read.
    let version =
        |program: &str, flag: &str| Command::new(program).arg(flag).output().map(|o| o.stdout).unwrap_or_default();
    let mut inputs = fnv(identity, &std::fs::read(manifest.join("../../Cargo.lock")).unwrap_or_default());
    inputs = fnv(inputs, &version(&rustc, "-vV"));
    inputs = fnv(inputs, &version("cc", "--version"));
    let mut shaping: Vec<(String, String)> = env::vars().filter(|(name, _)| shapes(name)).collect();
    shaping.sort();
    for (name, value) in shaping {
        inputs = fnv(fnv(inputs, name.as_bytes()), value.as_bytes());
    }
    let root = runtime_root(&out_dir);

    let (library, link_args) = build_runtime(&runtime, &root, &target, inputs);
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
                let (library, _) = build_runtime(&runtime, &root, &musl, inputs);
                compress(&std::fs::read(library).unwrap(), &path);
            }
        }
    }

    // FNV-1a of the library and of the static parts identifies the runtime
    // in compiler caches, which keep each part by it.
    let mut hash = fnv(0xcbf2_9ce4_8422_2325, &bytes);
    for (name, _) in parts {
        hash = fnv(hash, &std::fs::read(out_dir.join(name)).unwrap());
    }
    std::fs::write(out_dir.join("runtime-hash.txt"), format!("{hash:016x}")).unwrap();
}

fn fnv(start: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(start, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// The variables of the environment that change what the runtime's build
/// makes: the C compilers and their flags, which the `cc` crate reads,
/// Rust's flags, and Apple's deployment target and SDK.
fn shapes(name: &str) -> bool {
    let prefixes = ["CC", "CFLAGS", "CXX", "AR", "TARGET_", "HOST_", "RUSTFLAGS", "LIBSQLITE3", "MACOSX_", "SDKROOT"];
    prefixes.iter().any(|prefix| name.starts_with(prefix))
}

/// `tondo-runtime` in the target directory: the one above `OUT_DIR` that
/// cargo marks with `CACHEDIR.TAG`, or `OUT_DIR` itself when there is none.
fn runtime_root(out_dir: &Path) -> PathBuf {
    let target_dir = out_dir.ancestors().find(|dir| dir.join("CACHEDIR.TAG").is_file()).unwrap_or(out_dir);
    target_dir.join("tondo-runtime")
}

/// Builds the runtime for `target`, unless a build kept it for the same
/// `inputs`: the archive, and the native libraries it needs as linker
/// arguments.
fn build_runtime(runtime: &Path, root: &Path, target: &str, inputs: u64) -> (PathBuf, String) {
    // `TONDO_RUNTIME_FAST=1`, while working on the toolchain: many codegen
    // units, incrementally, compile a change sooner, with a runtime that
    // runs as the real one but is not measured or shipped.
    println!("cargo:rerun-if-env-changed=TONDO_RUNTIME_FAST");
    let fast = env::var_os("TONDO_RUNTIME_FAST").is_some_and(|value| value == "1");
    // `TONDO_RUNTIME_COVERAGE=1`, to measure what of the runtime the tests
    // of the toolchain run: each program it links writes where its
    // `LLVM_PROFILE_FILE` says (docs/refactor.md, phase 0.1).
    println!("cargo:rerun-if-env-changed=TONDO_RUNTIME_COVERAGE");
    // Only this machine's: the runtimes of other targets, as musl's, have no
    // profiler to link with.
    let coverage = env::var_os("TONDO_RUNTIME_COVERAGE").is_some_and(|value| value == "1")
        && env::var("HOST").is_ok_and(|host| host == target);
    // Its own build directory, so this build does not wait for the one
    // that runs it; and one for each way, so that neither undoes the other.
    let mode = match (coverage, fast) {
        (true, _) => "runtime-coverage",
        (false, true) => "runtime-fast",
        (false, false) => "runtime",
    };
    let target_dir = root.join(mode);
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
        .args(["-C", if fast { "codegen-units=16" } else { "codegen-units=1" }])
        .args(["-C", "llvm-args=-align-all-functions=6"])
        .env("CARGO_PROFILE_RELEASE_INCREMENTAL", fast.to_string())
        .args(["--cfg", "tondo_staticlib"])
        // Counters that live in the file, with `%c` in `LLVM_PROFILE_FILE`:
        // a program a test kills, or that ends with `_exit`, counts too.
        .args(
            coverage
                .then_some(["-C", "instrument-coverage", "-C", "llvm-args=-runtime-counter-relocation"])
                .into_iter()
                .flatten(),
        )
        .args(["--print", "native-static-libs"])
        .current_dir(runtime)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        // Under clippy the runtime is built as under rustc, and is the same.
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
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
    // SQLite asks for the `*64` functions of large files, which musl does
    // not have: its `off_t` is 64 bits already.
    if target.contains("musl") {
        command.env("LIBSQLITE3_FLAGS", "-DSQLITE_DISABLE_LFS");
    }
    let mut key = fnv(inputs, format!("{target} {mode}").as_bytes());
    let mut given: Vec<_> = command.get_envs().collect();
    given.sort();
    for (name, value) in given {
        key = fnv(key, name.as_encoded_bytes());
        key = fnv(key, value.map_or(&b""[..], |value| value.as_encoded_bytes()));
    }
    let kept = root.join("outputs").join(format!("{target}.{mode}.{key:016x}"));
    let (library, arguments) = (kept.join("libtondo_runtime.a"), kept.join("link-args.txt"));
    if let (true, Ok(link_args)) = (library.is_file(), std::fs::read_to_string(&arguments)) {
        return (library, link_args);
    }
    let output = command.output().expect("failed to run cargo for the Tondo runtime");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!("compiling the Tondo runtime for {target} failed:\n{stderr}");
    }
    let mut link_args = stderr
        .lines()
        .find_map(|line| line.split_once("native-static-libs: ").map(|(_, libs)| libs.trim()))
        .unwrap_or("")
        .to_string();
    // What writes the counts as a program ends, which Rust links with a
    // program of its own and a staticlib leaves out.
    if coverage {
        let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let sysroot = Command::new(rustc).args(["--print", "sysroot"]).output().expect("rustc's sysroot");
        let lib = Path::new(String::from_utf8_lossy(&sysroot.stdout).trim()).join(format!("lib/rustlib/{target}/lib"));
        let profiler = std::fs::read_dir(&lib)
            .expect("the libraries of rustc's sysroot")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("libprofiler_builtins"))
            })
            .expect("profiler_builtins in rustc's sysroot");
        link_args = format!("{} -Wl,-u,__llvm_profile_runtime {link_args}", profiler.display());
    }
    keep(&kept, &target_dir.join(target).join("release/libtondo_runtime.a"), &link_args);
    (library, link_args)
}

/// Keeps what a build made in `kept`, written whole or not at all, and lets
/// go of what an earlier key of the same target and way left beside it.
fn keep(kept: &Path, library: &Path, link_args: &str) {
    let (outputs, name) = (kept.parent().unwrap(), kept.file_name().unwrap().to_string_lossy());
    let writing = outputs.join(format!(".{name}.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&writing);
    std::fs::create_dir_all(&writing).unwrap();
    std::fs::copy(library, writing.join("libtondo_runtime.a")).unwrap();
    std::fs::write(writing.join("link-args.txt"), link_args).unwrap();
    // Another build may have kept the same first.
    if std::fs::rename(&writing, kept).is_err() {
        let _ = std::fs::remove_dir_all(&writing);
    }
    let same = &name[..name.rfind('.').unwrap() + 1];
    for entry in std::fs::read_dir(outputs).into_iter().flatten().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if other.starts_with(same) && other != name {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn on_path(program: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn compress(bytes: &[u8], path: &Path) {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(bytes).unwrap();
    std::fs::write(path, encoder.finish().unwrap()).unwrap();
}
