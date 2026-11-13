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

    /// The system it runs: Linux, the only one tondo builds for from
    /// another machine.
    pub fn system(self) -> crate::project::System {
        crate::project::System::Linux
    }

    /// The target of language 6.8 it is: which files a build for it has.
    pub fn project(self) -> crate::project::Target {
        let arch = match self {
            Target::X86_64 => crate::project::Arch::X64,
            Target::Aarch64 => crate::project::Arch::Arm64,
        };
        crate::project::Target { system: self.system(), arch }
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

/// The features of a processor the code generator has a flag for: the
/// flag, and the name Rust's detection gives the feature, which the runtime
/// checks at the start (toolchain 7.8).
const FLAGS: [(crate::project::Arch, &str, &str); 26] = {
    use crate::project::Arch::{Arm64, X64};
    [
        (X64, "has_sse3", "sse3"),
        (X64, "has_ssse3", "ssse3"),
        (X64, "has_sse41", "sse4.1"),
        (X64, "has_sse42", "sse4.2"),
        (X64, "has_popcnt", "popcnt"),
        (X64, "has_cmpxchg16b", "cmpxchg16b"),
        (X64, "has_avx", "avx"),
        (X64, "has_avx2", "avx2"),
        (X64, "has_fma", "fma"),
        (X64, "has_bmi1", "bmi1"),
        (X64, "has_bmi2", "bmi2"),
        (X64, "has_lzcnt", "lzcnt"),
        (X64, "has_avx512f", "avx512f"),
        (X64, "has_avx512dq", "avx512dq"),
        (X64, "has_avx512vl", "avx512vl"),
        (X64, "has_avx512bitalg", "avx512bitalg"),
        (X64, "has_avx512vbmi", "avx512vbmi"),
        (X64, "has_avx512vnni", "avx512vnni"),
        (X64, "has_avx_vnni", "avxvnni"),
        (X64, "has_aes", "aes"),
        (X64, "has_pclmulqdq", "pclmulqdq"),
        (Arm64, "has_lse", "lse"),
        (Arm64, "has_pauth", "paca"),
        (Arm64, "has_fp16", "fp16"),
        (Arm64, "has_dotprod", "dotprod"),
        (Arm64, "has_i8mm", "i8mm"),
    ]
};

/// The variants of `cpu.Feature`, in order (standard library 14.44): the
/// architecture of each, none for both, and its generator flag, if it has
/// one; `neon` stands for what every arm64 has.
const FEATURES: [(Option<crate::project::Arch>, Option<&str>); 23] = {
    use crate::project::Arch::{Arm64, X64};
    [
        (Some(X64), Some("has_sse3")),
        (Some(X64), Some("has_ssse3")),
        (Some(X64), Some("has_sse41")),
        (Some(X64), Some("has_sse42")),
        (Some(X64), Some("has_popcnt")),
        (Some(X64), Some("has_avx")),
        (Some(X64), Some("has_avx2")),
        (Some(X64), Some("has_fma")),
        (Some(X64), Some("has_bmi1")),
        (Some(X64), Some("has_bmi2")),
        (Some(X64), Some("has_lzcnt")),
        (Some(X64), Some("has_avx512f")),
        (Some(X64), Some("has_avx512dq")),
        (Some(X64), Some("has_avx512vl")),
        (Some(X64), None),
        (Some(X64), Some("has_pclmulqdq")),
        (Some(X64), None),
        (Some(Arm64), Some("neon")),
        (Some(Arm64), Some("has_lse")),
        (Some(Arm64), None),
        (Some(Arm64), None),
        (Some(Arm64), None),
        (None, None),
    ]
};

/// The names of the variants of `cpu.Feature`, in order.
pub const FEATURE_NAMES: [&str; 23] = [
    "Sse3",
    "Ssse3",
    "Sse41",
    "Sse42",
    "Popcnt",
    "Avx",
    "Avx2",
    "Fma",
    "Bmi1",
    "Bmi2",
    "Lzcnt",
    "Avx512F",
    "Avx512Dq",
    "Avx512Vl",
    "Avx512Bw",
    "Pclmulqdq",
    "Sha",
    "Neon",
    "Lse",
    "Crc32",
    "Pmull",
    "Sha2",
    "Aes",
];

/// The levels of x64: each the flags of `FLAGS` it adds to the one before.
const X64_LEVELS: [(&str, usize); 4] = [("baseline", 6), ("x86-64-v2", 6), ("x86-64-v3", 12), ("x86-64-v4", 15)];

/// The level of processor a build is for (toolchain 7.8): which features
/// of its architecture the code may use, as `--cpu` and `[build] cpu`
/// name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub name: String,
    pub arch: crate::project::Arch,
    /// The generator's flags it turns on.
    pub flags: Vec<&'static str>,
}

impl Cpu {
    /// The level `name` for `arch`; `foreign` when the build is for
    /// another machine than this one, which `native` is not.
    pub fn parse(name: &str, arch: crate::project::Arch, foreign: Option<&str>) -> Result<Cpu, String> {
        use crate::project::Arch;
        let levels = |arch: Arch| match arch {
            Arch::X64 => "`baseline`, `x86-64-v2`, `x86-64-v3`, `x86-64-v4` and `native`",
            Arch::Arm64 => "`baseline` and `native`",
        };
        let of_arch = |wanted: Arch| FLAGS.iter().filter(move |(arch, ..)| *arch == wanted);
        let flags = match (name, arch) {
            ("native", _) => {
                if let Some(target) = foreign {
                    return Err(format!(
                        "`native` is the processor of the machine that builds, and this build is for {target}: name a level, as `baseline`"
                    ));
                }
                of_arch(arch).filter(|(_, _, detected)| detected_here(detected)).map(|(_, flag, _)| *flag).collect()
            }
            ("baseline", Arch::Arm64) => Vec::new(),
            (name, Arch::X64) if let Some((_, count)) = X64_LEVELS.iter().find(|(level, _)| *level == name) => {
                of_arch(Arch::X64).take(*count).map(|(_, flag, _)| *flag).collect()
            }
            (name, Arch::Arm64) if X64_LEVELS.iter().any(|(level, _)| *level == name) => {
                return Err(format!(
                    "`{name}` is a level of x64, and this build is for arm64, which has {}",
                    levels(Arch::Arm64)
                ));
            }
            (name, arch) => {
                return Err(format!("unknown processor level `{name}`: {} has {}", arch.name(), levels(arch)));
            }
        };
        Ok(Cpu { name: name.to_string(), arch, flags })
    }

    /// Whether a processor of the level has the variant `feature` of
    /// `cpu.Feature`, when the level says: one of the other architecture is
    /// not there, one the level turns on is, and NEON is on every arm64.
    pub fn has(&self, feature: u32) -> Option<bool> {
        let (arch, _) = *FEATURES.get(feature as usize)?;
        if arch.is_some_and(|arch| arch != self.arch) {
            return Some(false);
        }
        flag_of(feature, self.arch).is_some_and(|flag| flag == "neon" || self.flags.contains(&flag)).then_some(true)
    }

    /// The generator's flags of a function that needs the features
    /// `needs`, a bit each, beyond the level (toolchain 7.8): the level's,
    /// with the flag of each feature it needs that has one.
    pub fn settings_with(&self, needs: u32) -> impl Iterator<Item = (&'static str, bool)> + '_ {
        let needed: Vec<&str> = (0..FEATURES.len() as u32)
            .filter(|feature| needs & (1 << feature) != 0)
            .filter_map(|feature| flag_of(feature, self.arch))
            .collect();
        self.settings().map(move |(flag, on)| (flag, on || needed.contains(&flag)))
    }

    /// Every flag of its architecture's features, and whether it is on: the
    /// level sets them all, and nothing else of the generator's settings.
    pub fn settings(&self) -> impl Iterator<Item = (&'static str, bool)> + '_ {
        FLAGS.iter().filter(|(arch, ..)| *arch == self.arch).map(|(_, flag, _)| (*flag, self.flags.contains(flag)))
    }

    /// What the runtime checks at the start: the level's name, and the name
    /// of each feature it needs, a line each.
    pub fn needed(&self) -> String {
        let names = FLAGS.iter().filter(|(_, flag, _)| self.flags.contains(flag)).map(|(.., detected)| *detected);
        std::iter::once(self.name.as_str()).chain(names).collect::<Vec<_>>().join("\n")
    }
}

/// The generator's flag of the variant `feature` of `cpu.Feature` on
/// `arch`, if it has one: `Aes` is of both architectures, and has one on
/// x64 only.
fn flag_of(feature: u32, arch: crate::project::Arch) -> Option<&'static str> {
    match (FEATURE_NAMES.get(feature as usize), arch) {
        (Some(&"Aes"), crate::project::Arch::X64) => Some("has_aes"),
        _ => FEATURES.get(feature as usize)?.1,
    }
}

/// Whether this machine's processor has the feature Rust's detection
/// calls `name`.
fn detected_here(name: &str) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        match name {
            "sse3" => std::is_x86_feature_detected!("sse3"),
            "ssse3" => std::is_x86_feature_detected!("ssse3"),
            "sse4.1" => std::is_x86_feature_detected!("sse4.1"),
            "sse4.2" => std::is_x86_feature_detected!("sse4.2"),
            "popcnt" => std::is_x86_feature_detected!("popcnt"),
            "cmpxchg16b" => std::is_x86_feature_detected!("cmpxchg16b"),
            "avx" => std::is_x86_feature_detected!("avx"),
            "avx2" => std::is_x86_feature_detected!("avx2"),
            "fma" => std::is_x86_feature_detected!("fma"),
            "bmi1" => std::is_x86_feature_detected!("bmi1"),
            "bmi2" => std::is_x86_feature_detected!("bmi2"),
            "lzcnt" => std::is_x86_feature_detected!("lzcnt"),
            "avx512f" => std::is_x86_feature_detected!("avx512f"),
            "avx512dq" => std::is_x86_feature_detected!("avx512dq"),
            "avx512vl" => std::is_x86_feature_detected!("avx512vl"),
            "avx512bitalg" => std::is_x86_feature_detected!("avx512bitalg"),
            "avx512vbmi" => std::is_x86_feature_detected!("avx512vbmi"),
            "avx512vnni" => std::is_x86_feature_detected!("avx512vnni"),
            "avxvnni" => std::is_x86_feature_detected!("avxvnni"),
            "aes" => std::is_x86_feature_detected!("aes"),
            "pclmulqdq" => std::is_x86_feature_detected!("pclmulqdq"),
            _ => false,
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        match name {
            "lse" => std::arch::is_aarch64_feature_detected!("lse"),
            "paca" => std::arch::is_aarch64_feature_detected!("paca"),
            "fp16" => std::arch::is_aarch64_feature_detected!("fp16"),
            "dotprod" => std::arch::is_aarch64_feature_detected!("dotprod"),
            "i8mm" => std::arch::is_aarch64_feature_detected!("i8mm"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cpu;
    use crate::project::Arch;

    #[test]
    fn levels_turn_on_their_features() {
        let flags = |name, arch| Cpu::parse(name, arch, None).unwrap().flags;
        assert_eq!(flags("baseline", Arch::X64), flags("x86-64-v2", Arch::X64));
        assert_eq!(flags("baseline", Arch::X64).len(), 6);
        assert!(flags("x86-64-v3", Arch::X64).ends_with(&["has_bmi1", "has_bmi2", "has_lzcnt"]));
        assert!(flags("x86-64-v4", Arch::X64).ends_with(&["has_avx512f", "has_avx512dq", "has_avx512vl"]));
        assert!(flags("baseline", Arch::Arm64).is_empty());
        // Every feature of its architecture is set, on or off, and no other.
        let v3 = Cpu::parse("x86-64-v3", Arch::X64, None).unwrap();
        let settings: Vec<_> = v3.settings().collect();
        assert_eq!(settings.len(), 21);
        assert!(settings.contains(&("has_avx2", true)) && settings.contains(&("has_avx512f", false)));
        assert_eq!(v3.needed().lines().next(), Some("x86-64-v3"));
        assert!(v3.needed().lines().any(|line| line == "avx2"));
        // A function that needs `Bmi2` and `Aes` beyond the baseline gets
        // their flags, and only those; on arm64 they have none.
        let baseline = Cpu::parse("baseline", Arch::X64, None).unwrap();
        let feature = |name| 1 << super::FEATURE_NAMES.iter().position(|n| *n == name).unwrap();
        let on =
            |cpu: &Cpu, needs| cpu.settings_with(needs).filter(|(_, on)| *on).map(|(flag, _)| flag).collect::<Vec<_>>();
        let more = on(&baseline, feature("Bmi2") | feature("Aes"));
        assert_eq!(more.len(), baseline.flags.len() + 2, "{more:?}");
        assert!(more.contains(&"has_bmi2") && more.contains(&"has_aes"));
        let arm = Cpu::parse("baseline", Arch::Arm64, None).unwrap();
        assert!(on(&arm, feature("Aes") | feature("Crc32")).is_empty());
    }

    #[test]
    fn a_level_is_one_of_its_architecture() {
        let error = |name, arch, foreign| Cpu::parse(name, arch, foreign).unwrap_err();
        assert_eq!(
            error("fast", Arch::X64, None),
            "unknown processor level `fast`: x64 has `baseline`, `x86-64-v2`, `x86-64-v3`, `x86-64-v4` and `native`"
        );
        assert_eq!(
            error("x86-64-v3", Arch::Arm64, None),
            "`x86-64-v3` is a level of x64, and this build is for arm64, which has `baseline` and `native`"
        );
        assert_eq!(
            error("native", Arch::Arm64, Some("aarch64-linux")),
            "`native` is the processor of the machine that builds, and this build is for aarch64-linux: name a level, as `baseline`"
        );
        assert_eq!(Cpu::parse("native", Arch::host(), None).unwrap().name, "native");
    }
}
