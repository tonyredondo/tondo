//! The processor (toolchain 7.8): a program checks at its start that it
//! has what the level it was built for needs.

use super::{Str, str_bytes, str_new};
use std::io::Write;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

/// The level the program was built for and the features it needs, a line
/// each, as its compiled code hands them over.
static NEEDED: AtomicPtr<Str> = AtomicPtr::new(std::ptr::null_mut());

/// What the processor has of `cpu.Feature`, a bit for each variant in its
/// order, found at the start.
static FEATURES: AtomicU64 = AtomicU64::new(0);

/// Before any of the program's code: what the processor lacks of what its
/// level needs ends the program, with a panic's status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_cpu(needed: *mut Str) {
    NEEDED.store(needed, Ordering::Relaxed);
    let found = features().iter().enumerate().fold(0, |mask, (bit, &has)| mask | (u64::from(has) << bit));
    FEATURES.store(found, Ordering::Relaxed);
    let text = String::from_utf8_lossy(unsafe { str_bytes(needed) }).into_owned();
    let mut lines = text.lines();
    let level = lines.next().unwrap_or_default();
    let lacking: Vec<&str> = lines.filter(|name| !detected(name)).map(shown).collect();
    let Some((last, first)) = lacking.split_last() else { return };
    let lacked = match first {
        [] => last.to_string(),
        first => format!("{} and {last}", first.join(", ")),
    };
    let fix = match level {
        "baseline" | "x86-64-v2" => ", which every program of tondo needs",
        _ => ": build it with --cpu baseline",
    };
    let _ =
        writeln!(std::io::stderr(), "fatal: this program was built for {level} and this processor lacks {lacked}{fix}");
    std::process::exit(crate::PANIC_EXIT_CODE);
}

/// `cpu.has(feature)` for the variant `feature` of `cpu.Feature`, when
/// the level does not say.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_cpu_has(feature: i64) -> bool {
    FEATURES.load(Ordering::Relaxed) >> feature & 1 == 1
}

/// What the processor has of each variant of `cpu.Feature`, in its order
/// (standard library 14.44).
fn features() -> [bool; 23] {
    #[cfg(target_arch = "x86_64")]
    {
        use std::is_x86_feature_detected as has;
        [
            has!("sse3"),
            has!("ssse3"),
            has!("sse4.1"),
            has!("sse4.2"),
            has!("popcnt"),
            has!("avx"),
            has!("avx2"),
            has!("fma"),
            has!("bmi1"),
            has!("bmi2"),
            has!("lzcnt"),
            has!("avx512f"),
            has!("avx512dq"),
            has!("avx512vl"),
            has!("avx512bw"),
            has!("pclmulqdq"),
            has!("sha"),
            false,
            false,
            false,
            false,
            false,
            has!("aes"),
        ]
    }
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::is_aarch64_feature_detected as has;
        let mut found = [false; 23];
        found[17..].copy_from_slice(&[
            has!("neon"),
            has!("lse"),
            has!("crc"),
            has!("pmull"),
            has!("sha2"),
            has!("aes"),
        ]);
        found
    }
}

/// `env.build().cpu`: the level the program was built for.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_cpu() -> *mut Str {
    let needed = NEEDED.load(Ordering::Relaxed);
    let text = if needed.is_null() { &[][..] } else { unsafe { str_bytes(needed) } };
    str_new(text.split(|&b| b == b'\n').next().unwrap_or_default())
}

/// Whether the processor has the feature Rust's detection calls `name`.
fn detected(name: &str) -> bool {
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

/// A feature as its makers write it.
fn shown(name: &str) -> &str {
    match name {
        "sse3" => "SSE3",
        "ssse3" => "SSSE3",
        "sse4.1" => "SSE4.1",
        "sse4.2" => "SSE4.2",
        "popcnt" => "POPCNT",
        "cmpxchg16b" => "CMPXCHG16B",
        "avx" => "AVX",
        "avx2" => "AVX2",
        "fma" => "FMA",
        "bmi1" => "BMI1",
        "bmi2" => "BMI2",
        "lzcnt" => "LZCNT",
        "avx512f" => "AVX-512F",
        "avx512dq" => "AVX-512DQ",
        "avx512vl" => "AVX-512VL",
        "avx512bitalg" => "AVX-512 BITALG",
        "avx512vbmi" => "AVX-512 VBMI",
        "avx512vnni" => "AVX-512 VNNI",
        "avxvnni" => "AVX-VNNI",
        "lse" => "LSE",
        "paca" => "pointer authentication",
        "fp16" => "FP16",
        "dotprod" => "DotProd",
        "i8mm" => "I8MM",
        name => name,
    }
}
