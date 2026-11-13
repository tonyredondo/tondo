//! The functions of `std.cpu.x64` and `std.cpu.arm64` (standard library
//! 14.47): each against a definition written plainly, on random inputs, on
//! this machine's architecture where its processor has the feature, in a
//! release build and in a debug one; its one instruction in `tondo asm`;
//! and where a function may not need a feature (toolchain 7.8).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Numbers that are the same on every run (xorshift).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self) -> [u8; 16] {
        let (low, high) = (self.next().to_le_bytes(), self.next().to_le_bytes());
        std::array::from_fn(|i| if i < 8 { low[i] } else { high[i - 8] })
    }
}

/// CRC-32C of the Castagnoli polynomial, bit by bit, without inverting.
fn crc32c(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 { crc >> 1 ^ 0x82f6_3b78 } else { crc >> 1 };
        }
    }
    crc
}

fn deposit(value: u64, mask: u64) -> u64 {
    let (mut out, mut taken) = (0, 0);
    for bit in (0..64).filter(|bit| mask >> bit & 1 == 1) {
        out |= (value >> taken & 1) << bit;
        taken += 1;
    }
    out
}

fn extract(value: u64, mask: u64) -> u64 {
    let (mut out, mut put) = (0, 0);
    for bit in (0..64).filter(|bit| mask >> bit & 1 == 1) {
        out |= (value >> bit & 1) << put;
        put += 1;
    }
    out
}

/// The product of `a` and `b` without carries, the low half first.
fn carryless(a: u64, b: u64) -> [u64; 2] {
    let product = (0..64).filter(|bit| b >> bit & 1 == 1).fold(0u128, |sum, bit| sum ^ (a as u128) << bit);
    [product as u64, (product >> 64) as u64]
}

/// AES as FIPS 197 defines it, on a state whose byte `r + 4c` is row `r`
/// of column `c`, as the bytes of a vector are.
type State = [u8; 16];

fn times(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 == 1 {
            product ^= a;
        }
        a = (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 };
        b >>= 1;
    }
    product
}

/// The S-box: the inverse in GF(2^8), then the affine map.
fn sbox() -> [u8; 256] {
    std::array::from_fn(|x| {
        let inverse = (0..254).fold(1, |power, _| times(power, x as u8));
        inverse
            ^ inverse.rotate_left(1)
            ^ inverse.rotate_left(2)
            ^ inverse.rotate_left(3)
            ^ inverse.rotate_left(4)
            ^ 0x63
    })
}

fn substituted(state: State, table: &[u8; 256]) -> State {
    state.map(|byte| table[byte as usize])
}

fn shifted(state: State) -> State {
    std::array::from_fn(|i| state[i % 4 + 4 * ((i / 4 + i % 4) % 4)])
}

fn unshifted(state: State) -> State {
    let mut out = [0; 16];
    for (i, &byte) in state.iter().enumerate() {
        out[i % 4 + 4 * ((i / 4 + i % 4) % 4)] = byte;
    }
    out
}

/// Each column times the circulant matrix whose first row is `row`.
fn mixed(state: State, row: [u8; 4]) -> State {
    std::array::from_fn(|i| {
        let (r, c) = (i % 4, i / 4);
        (0..4).fold(0, |sum, k| sum ^ times(row[(k + 4 - r) % 4], state[k + 4 * c]))
    })
}

const MIX: [u8; 4] = [2, 3, 1, 1];
const UNMIX: [u8; 4] = [14, 11, 13, 9];

fn xor(a: State, b: State) -> State {
    std::array::from_fn(|i| a[i] ^ b[i])
}

fn shown(lanes: &[impl std::fmt::Display]) -> String {
    format!("<{}>", lanes.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
}

fn bytes(state: &State) -> String {
    format!("simd.Vector128[Byte].of({})", state.map(|b| b.to_string()).join(", "))
}

/// A program of the machine's architecture that prints what each function
/// of its module gives on random inputs, a group of them for each feature
/// under a guard, and what the definitions give where this processor has
/// the feature.
fn program(arch: &str) -> (String, String) {
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let (sbox, inverse) = {
        let sbox = sbox();
        let mut inverse = [0; 256];
        sbox.iter().enumerate().for_each(|(x, &s)| inverse[s as usize] = x as u8);
        (sbox, inverse)
    };
    let mut code = format!("import std.console\nimport std.cpu\nimport std.cpu.{arch}\nimport std.simd\n");
    let mut expected = String::new();
    let mut groups: Vec<(&str, &str, bool)> = Vec::new();
    let mut group =
        |code: &mut String, name: &'static str, feature: &'static str, has: bool, lines: Vec<(String, String)>| {
            writeln!(code, "\nfn {name}() {{").unwrap();
            for (call, value) in lines {
                writeln!(code, "    console.println(\"{{{call}}}\")").unwrap();
                if has {
                    writeln!(expected, "{value}").unwrap();
                }
            }
            code.push_str("}\n");
            groups.push((name, feature, has));
        };
    let crcs = (0..32)
        .flat_map(|_| {
            let (crc, value) = (random.next() as u32, random.next());
            [
                (format!("{arch}.crc32cByte({crc}, {})", value as u8), crc32c(crc, &[value as u8]).to_string()),
                (format!("{arch}.crc32cInt({crc}, {value})"), crc32c(crc, &value.to_le_bytes()).to_string()),
            ]
        })
        .collect();
    let aes: Vec<(State, State)> = (0..32).map(|_| (random.bytes(), random.bytes())).collect();
    #[cfg(target_arch = "x86_64")]
    {
        group(&mut code, "crcs", "Sse42", std::is_x86_feature_detected!("sse4.2"), crcs);
        let bits = (0..32)
            .flat_map(|_| {
                let (value, mask) = (random.next(), random.next());
                [
                    (format!("x64.depositBits({value}, {mask})"), deposit(value, mask).to_string()),
                    (format!("x64.extractBits({value}, {mask})"), extract(value, mask).to_string()),
                ]
            })
            .collect();
        group(&mut code, "bits", "Bmi2", std::is_x86_feature_detected!("bmi2"), bits);
        let products = (0..32)
            .flat_map(|_| {
                let (a, b) = ([random.next(), random.next()], [random.next(), random.next()]);
                [0, 1, 16, 17].map(|halves: usize| {
                    let call = format!(
                        "x64.carrylessMultiply(simd.Vector128[UInt].of({}, {}), simd.Vector128[UInt].of({}, {}), {halves})",
                        a[0], a[1], b[0], b[1]
                    );
                    (call, shown(&carryless(a[halves & 1], b[halves >> 4])))
                })
            })
            .collect();
        group(&mut code, "products", "Pclmulqdq", std::is_x86_feature_detected!("pclmulqdq"), products);
        let rounds = aes
            .iter()
            .flat_map(|(state, key)| {
                let call = |name: &str| format!("x64.{name}({}, {})", bytes(state), bytes(key));
                let (up, down) = (substituted(shifted(*state), &sbox), substituted(unshifted(*state), &inverse));
                [
                    (call("aesEncrypt"), shown(&xor(mixed(up, MIX), *key))),
                    (call("aesEncryptLast"), shown(&xor(up, *key))),
                    (call("aesDecrypt"), shown(&xor(mixed(down, UNMIX), *key))),
                    (call("aesDecryptLast"), shown(&xor(down, *key))),
                ]
            })
            .collect();
        group(&mut code, "rounds", "Aes", std::is_x86_feature_detected!("aes"), rounds);
    }
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::is_aarch64_feature_detected as detected;
        group(&mut code, "crcs", "Crc32", detected!("crc"), crcs);
        let products = (0..32)
            .map(|_| {
                let (a, b) = (random.next(), random.next());
                (format!("arm64.polynomialMultiply({a}, {b})"), shown(&carryless(a, b)))
            })
            .collect();
        group(&mut code, "products", "Pmull", detected!("pmull"), products);
        let rounds = aes
            .iter()
            .flat_map(|(state, key)| {
                let call = |name: &str| format!("arm64.{name}({}, {})", bytes(state), bytes(key));
                let one = |name: &str| format!("arm64.{name}({})", bytes(state));
                [
                    (call("aesEncrypt"), shown(&substituted(shifted(xor(*state, *key)), &sbox))),
                    (call("aesDecrypt"), shown(&substituted(unshifted(xor(*state, *key)), &inverse))),
                    (one("aesMixColumns"), shown(&mixed(*state, MIX))),
                    (one("aesInverseMixColumns"), shown(&mixed(*state, UNMIX))),
                ]
            })
            .collect();
        group(&mut code, "rounds", "Aes", detected!("aes"), rounds);
    }
    code.push_str("\nfn main() {\n");
    for (name, feature, _) in groups {
        writeln!(code, "    if cpu.has(cpu.Feature.{feature}) {{\n        {name}()\n    }}").unwrap();
    }
    writeln!(code, "    {arch}.pause()\n}}").unwrap();
    (code, expected)
}

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

#[test]
fn each_function_gives_what_its_definition_does() {
    let dir = scratch("instructions-values");
    let (code, expected) = program(arch());
    let file = format!("main_{}.to", arch());
    std::fs::write(dir.join(&file), code).unwrap();
    for build in [&["run"][..], &["run", "--debug"]] {
        let (out, err, status) = tondo(&dir, &[build, &[file.as_str()]].concat());
        assert_eq!(status, 0, "{build:?}: {err}");
        let differs = out.lines().zip(expected.lines()).position(|(got, want)| got != want);
        assert!(
            differs.is_none() && out.lines().count() == expected.lines().count(),
            "{build:?}: line {differs:?} differs:\n{out}\nexpected:\n{expected}"
        );
    }
}

#[test]
fn each_function_is_its_instruction() {
    let dir = scratch("instructions-asm");
    let file = format!("main_{}.to", arch());
    std::fs::write(dir.join(&file), program(arch()).0).unwrap();
    let functions: &[(&str, &[&str])] = if arch() == "x64" {
        &[
            ("crcs", &["crc32"]),
            ("bits", &["pdep", "pext"]),
            ("products", &["pclmulqdq"]),
            ("rounds", &["aesenc ", "aesenclast", "aesdec ", "aesdeclast"]),
            ("main", &["pause"]),
        ]
    } else {
        &[
            ("crcs", &["crc32cb", "crc32cx"]),
            ("products", &["pmull"]),
            ("rounds", &["aese ", "aesd ", "aesmc", "aesimc"]),
            ("main", &["isb"]),
        ]
    };
    for (function, mnemonics) in functions {
        let (out, err, status) = tondo(&dir, &["asm", &file, function]);
        assert_eq!(status, 0, "{err}");
        for mnemonic in *mnemonics {
            assert!(out.contains(mnemonic), "`{function}` has no `{mnemonic}`:\n{out}");
        }
    }
    // A function that needs more stays a call: its code is compiled for
    // features `main` does not take for granted.
    let (out, _, _) = tondo(&dir, &["asm", &file, "main"]);
    assert!(
        out.contains("stays a call of `products`: it needs features of the processor this function does not"),
        "{out}"
    );
}

/// Rule 12 of toolchain 7.8, each place, and rule 11's guards, checked for
/// x64 on any machine.
#[test]
fn where_a_function_may_not_need_a_feature() {
    let dir = scratch("instructions-places");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"places\"\narchitectures = [\"x64\"]\n").unwrap();
    let code = r#"import std.console
import std.cpu
import std.cpu.x64

trait Spreads {
    fn spread(self): UInt
}

type Bits = { value: UInt }

impl Spreads for Bits {
    fn spread(self): UInt {
        x64.depositBits(self.value, 0xF0)
    }
}

fn spread(value: UInt): UInt {
    x64.depositBits(value, 0xF0)
}

fn twice(value: UInt): UInt {
    spread(value) * 2
}

pub extern fn exported(value: UInt): UInt {
    twice(value)
}

fn guarded(value: UInt): UInt {
    if cpu.has(cpu.Feature.Bmi2) and cpu.has(cpu.Feature.Bmi1) {
        return twice(value)
    }
    if not cpu.has(cpu.Feature.Bmi2) {
        return value
    }
    spread(value)
}

fn main() {
    console.println("{x64.depositBits(1, 2)} {twice(1)} {guarded(1)}")
    if cpu.has(cpu.Feature.Bmi2) {
        console.println("{twice(2)}")
    }
    let f = twice
    let values: Array[UInt] = [1, 2]
    console.println("{f(1)} {values.map((x) { x64.extractBits(x, 1) })}")
    _ = spawn twice(3)
    console.println("{Bits { value: 1 }.spread()} {x64.crc32cByte(1, 2)}")
}
"#;
    std::fs::write(dir.join("src/main.to"), code).unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 1, "{err}");
    let errors: Vec<&str> = err.lines().filter(|line| line.starts_with("error[")).collect();
    let bmi2 = "put the call under `if cpu.has(cpu.Feature.Bmi2) { ... }`";
    let alone = "write it in a function of its own, and call that under `if cpu.has(cpu.Feature.Bmi2) { ... }`";
    let wanted = [
        format!(
            "`x64.depositBits` needs `Bmi2` of the processor, and a method that implements `Spreads` cannot need one: {alone}"
        ),
        format!(
            "`x64.depositBits` needs `Bmi2` of the processor, and `main`, or the top level of a script, cannot need one: {alone}"
        ),
        format!(
            "this call needs `Bmi2` of the processor, and a `pub extern fn`, which C calls, cannot need one: `exported` calls `twice`, which calls `spread`, which uses `x64.depositBits`; {bmi2}"
        ),
        format!(
            "this call needs `Bmi2` of the processor, and `main`, or the top level of a script, cannot need one: `main` calls `twice`, which calls `spread`, which uses `x64.depositBits`; {bmi2}"
        ),
        // The `else` of a `not cpu.has(...)` and what follows a `return` are no guards.
        format!(
            "this call needs `Bmi2` of the processor, and `main`, or the top level of a script, cannot need one: `main` calls `guarded`, which calls `spread`, which uses `x64.depositBits`; {bmi2}"
        ),
        format!(
            "this call needs `Bmi2` of the processor, and a function used as a value, as on line 44, cannot need one: `twice` calls `spread`, which uses `x64.depositBits`; {bmi2}"
        ),
        format!(
            "`x64.extractBits` needs `Bmi2` of the processor, and a function used as a value, as on line 46, cannot need one: {alone}"
        ),
        format!(
            "this call needs `Bmi2` of the processor, and what `spawn` starts, as on line 47, cannot need one: `spawn twice` calls `twice`, which calls `spread`, which uses `x64.depositBits`; {bmi2}"
        ),
    ];
    for message in &wanted {
        assert!(errors.iter().any(|line| line.ends_with(message.as_str())), "no error says\n{message}\nin\n{err}");
    }
    // Nothing else: not the guarded calls, and not `crc32cByte`, whose
    // `Sse42` the baseline has.
    assert_eq!(errors.len(), wanted.len(), "{err}");
    // A level that has the feature needs nothing of it.
    std::fs::write(
        dir.join("tondo.toml"),
        "[package]\nname = \"places\"\narchitectures = [\"x64\"]\n\n[build]\ncpu = \"x86-64-v3\"\n",
    )
    .unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 0, "{err}");
}

#[test]
fn a_module_of_one_architecture_and_its_constants() {
    let dir = scratch("instructions-errors");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "import std.cpu.x64\n\nfn main() {\n    x64.pause()\n}\n").unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 1, "{err}");
    let message = "`std.cpu.x64` is of x64 only: import it from a file of x64, as `name_x64.to`, or limit the package with \
                   `architectures = [\"x64\"]` in tondo.toml";
    assert!(err.contains(message), "{err}");
    std::fs::rename(dir.join("src/main.to"), dir.join("src/main_x64.to")).unwrap();
    std::fs::write(dir.join("src/main_arm64.to"), "fn main() {}\n").unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 0, "{err}");
    // A package of each architecture alone is checked on any machine.
    std::fs::remove_file(dir.join("src/main_arm64.to")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\narchitectures = [\"x64\"]\n").unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 0, "{err}");
    std::fs::rename(dir.join("src/main_x64.to"), dir.join("src/main.to")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\narchitectures = [\"arm64\"]\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "import std.cpu.arm64\n\nfn main() {\n    arm64.pause()\n}\n").unwrap();
    let (_, err, status) = tondo(&dir, &["check"]);
    assert_eq!(status, 0, "{err}");
    // `halves` is one of four constants.
    let products = "import std.cpu\nimport std.cpu.x64\nimport std.simd\n\nfn product(halves: Int): simd.Vector128[UInt] {\n    \
                    let zero = simd.Vector128[UInt].zero()\n    \
                    _ = x64.carrylessMultiply(zero, zero, 2)\n    \
                    x64.carrylessMultiply(zero, zero, halves)\n}\n\nfn main() {\n    \
                    if cpu.has(cpu.Feature.Pclmulqdq) {\n        _ = product(1)\n    }\n}\n";
    std::fs::write(dir.join("products_x64.to"), products).unwrap();
    let (_, err, status) = tondo(&dir, &["check", "products_x64.to"]);
    assert_eq!(status, 1, "{err}");
    let message = "error[E1901]: `halves` is a constant written in the call, one of four: 0 takes the low halves of \
                   both, 1 takes the high half of `a` and the low half of `b`, 16 takes the low half of `a` and the high \
                   half of `b`, 17 takes the high halves of both; write the one you mean, as \
                   `x64.carrylessMultiply(a, b, 0)`";
    assert_eq!(err.matches(message).count(), 2, "{err}");
}
