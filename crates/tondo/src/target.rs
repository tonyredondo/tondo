//! Targets (toolchain 7.8): Linux on x86-64 or on arm64, whose executables
//! `--target` builds static, with musl. The runtime of the machine's own
//! comes with the compiler; another's in a pack, which a `tondo` for that
//! target writes with `tondo target pack` and `TONDO_TARGETS` names the
//! directory of. A program for another architecture runs under qemu's
//! user emulation.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    X86_64,
    Aarch64,
}

/// The files of a pack, in the order a static executable links them: the
/// runtime, Rust's unwinder, musl's C library and its start file.
pub const PACK: [&str; 4] = ["runtime.a", "libunwind.a", "libc.a", "rcrt1.o"];

impl Target {
    pub fn parse(name: &str) -> Result<Target, String> {
        match name {
            "x86_64-linux" => Ok(Target::X86_64),
            "aarch64-linux" => Ok(Target::Aarch64),
            _ => Err(format!("unknown target `{name}`: tondo builds for x86_64-linux and aarch64-linux")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::X86_64 => "x86_64-linux",
            Target::Aarch64 => "aarch64-linux",
        }
    }

    /// Cranelift's name for its code.
    pub fn triple(self) -> &'static str {
        match self {
            Target::X86_64 => "x86_64-unknown-linux-musl",
            Target::Aarch64 => "aarch64-unknown-linux-musl",
        }
    }

    fn arch(self) -> &'static str {
        match self {
            Target::X86_64 => "x86_64",
            Target::Aarch64 => "aarch64",
        }
    }

    /// Linux on this machine's architecture, if it is one.
    pub fn machine() -> Option<Target> {
        match (cfg!(target_os = "linux"), cfg!(target_arch = "aarch64"), cfg!(target_arch = "x86_64")) {
            (true, true, _) => Some(Target::Aarch64),
            (true, _, true) => Some(Target::X86_64),
            _ => None,
        }
    }

    /// The machine's own, whose runtime the compiler carries.
    pub fn native(self) -> bool {
        Target::machine() == Some(self)
    }

    /// The pack of this target for a compiler whose runtime is `identity`:
    /// in a directory `TONDO_TARGETS` names.
    pub fn pack(self, identity: &str) -> Result<Vec<PathBuf>, String> {
        let dirs = std::env::var_os("TONDO_TARGETS").unwrap_or_default();
        for dir in std::env::split_paths(&dirs) {
            let pack = dir.join(self.name());
            let Ok(found) = std::fs::read_to_string(pack.join("identity")) else { continue };
            if found.trim() != identity {
                return Err(format!(
                    "the pack {} is for another version of tondo: write it again with `tondo target pack` of this version for {}",
                    pack.display(),
                    self.name()
                ));
            }
            return Ok(PACK.iter().map(|file| pack.join(file)).collect());
        }
        Err(format!(
            "building for {0} needs its runtime, which this tondo does not carry: write it with `tondo target pack <dir>` of a tondo for {0}, and set TONDO_TARGETS to <dir>",
            self.name()
        ))
    }

    /// What runs one of its executables here: nothing on its own machine, or
    /// where the kernel hands it to qemu (`binfmt_misc`); qemu's user
    /// emulation where it is installed.
    pub fn launcher(self) -> Result<Option<PathBuf>, String> {
        if self.native() {
            return Ok(None);
        }
        if !cfg!(target_os = "linux") {
            return Err(format!(
                "a program for {0} runs on Linux: build it with `tondo build --target {0}` and run it there",
                self.name()
            ));
        }
        let names = [format!("qemu-{}", self.arch()), format!("qemu-{}-static", self.arch())];
        let path = std::env::var_os("PATH").unwrap_or_default();
        for dir in std::env::split_paths(&path) {
            if let Some(found) = names.iter().map(|name| dir.join(name)).find(|p| p.is_file()) {
                return Ok(Some(found));
            }
        }
        let registered = Path::new("/proc/sys/fs/binfmt_misc").join(format!("qemu-{}", self.arch()));
        if std::fs::read_to_string(registered).is_ok_and(|text| text.starts_with("enabled")) {
            return Ok(None);
        }
        Err(format!(
            "a program for {} runs here under qemu's user emulation: install qemu-user, which has qemu-{}",
            self.name(),
            self.arch()
        ))
    }
}
