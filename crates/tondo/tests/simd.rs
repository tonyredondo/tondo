//! The vectors of `std.simd` (standard library 14.46): every operation of
//! every type of lanes, at each width, against its definition one lane at a
//! time, on random inputs. A program prints each result, a float as its
//! bits, and this test works out the same from the definitions.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

#[derive(Copy, Clone, PartialEq, Debug)]
enum Kind {
    Signed,
    Unsigned,
    Byte,
    Float,
}

/// A type of lanes: its name, its bits, its kind, and the unsigned type of
/// its width, as which a float's lanes show their bits.
#[derive(Copy, Clone, Debug)]
struct Lane {
    name: &'static str,
    bits: u32,
    kind: Kind,
    twin: &'static str,
}

const LANES: [Lane; 11] = [
    Lane { name: "Int8", bits: 8, kind: Kind::Signed, twin: "UInt8" },
    Lane { name: "Int16", bits: 16, kind: Kind::Signed, twin: "UInt16" },
    Lane { name: "Int32", bits: 32, kind: Kind::Signed, twin: "UInt32" },
    Lane { name: "Int", bits: 64, kind: Kind::Signed, twin: "UInt" },
    Lane { name: "UInt8", bits: 8, kind: Kind::Unsigned, twin: "UInt8" },
    Lane { name: "UInt16", bits: 16, kind: Kind::Unsigned, twin: "UInt16" },
    Lane { name: "UInt32", bits: 32, kind: Kind::Unsigned, twin: "UInt32" },
    Lane { name: "UInt", bits: 64, kind: Kind::Unsigned, twin: "UInt" },
    Lane { name: "Byte", bits: 8, kind: Kind::Byte, twin: "Byte" },
    Lane { name: "Float32", bits: 32, kind: Kind::Float, twin: "UInt32" },
    Lane { name: "Float", bits: 64, kind: Kind::Float, twin: "UInt" },
];

/// Numbers that are the same on every run (xorshift).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

impl Lane {
    fn mask(self) -> u64 {
        if self.bits == 64 { u64::MAX } else { (1 << self.bits) - 1 }
    }

    fn arithmetic(self) -> bool {
        self.kind != Kind::Byte
    }

    fn integer(self) -> bool {
        matches!(self.kind, Kind::Signed | Kind::Unsigned)
    }

    fn bitwise(self) -> bool {
        self.kind != Kind::Float
    }

    /// A lane's bits as a signed value.
    fn signed(self, v: u64) -> i64 {
        let shift = 64 - self.bits;
        ((v << shift) as i64) >> shift
    }

    /// Random lanes; a float's are often one of the values at its edges.
    fn values(self, random: &mut Random, count: usize) -> Vec<u64> {
        (0..count)
            .map(|_| {
                let bits = random.next();
                if self.kind != Kind::Float || !bits.is_multiple_of(4) {
                    // A float made of random bits is mostly huge or tiny:
                    // half of them are small numbers.
                    return match (self.kind, bits % 3) {
                        (Kind::Float, 0) => self.float_bits(((bits >> 8) % 2001) as f64 / 8.0 - 125.0),
                        _ => bits & self.mask(),
                    };
                }
                let special = [f64::NAN, -0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY, 0.5, -1.5, 2.5, 1e-310, 3e38];
                self.float_bits(special[(bits >> 8) as usize % special.len()])
            })
            .collect()
    }

    fn float_bits(self, value: f64) -> u64 {
        if self.bits == 32 { (value as f32).to_bits() as u64 } else { value.to_bits() }
    }

    fn is_nan(self, v: u64) -> bool {
        self.kind == Kind::Float
            && if self.bits == 32 { f32::from_bits(v as u32).is_nan() } else { f64::from_bits(v).is_nan() }
    }

    /// A float operation on the lanes' bits.
    fn float(self, a: u64, b: u64, c: u64, op: &str) -> u64 {
        macro_rules! with {
            ($f:ty, $u:ty) => {{
                let (x, y, z) = (<$f>::from_bits(a as $u), <$f>::from_bits(b as $u), <$f>::from_bits(c as $u));
                let r: $f = match op {
                    "add" => x + y,
                    "sub" => x - y,
                    "mul" => x * y,
                    "div" => x / y,
                    "neg" => -x,
                    "abs" => x.abs(),
                    "sqrt" => x.sqrt(),
                    "floor" => x.floor(),
                    "ceil" => x.ceil(),
                    "round" => x.round_ties_even(),
                    "truncate" => x.trunc(),
                    "fma" => x.mul_add(y, z),
                    // NaN when either is, and -0 below +0 (WebAssembly's).
                    "min" | "max" => {
                        if x.is_nan() || y.is_nan() {
                            <$f>::NAN
                        } else if x == y {
                            if (op == "min") == x.is_sign_negative() { x } else { y }
                        } else if (x < y) == (op == "min") {
                            x
                        } else {
                            y
                        }
                    }
                    _ => unreachable!("{op}"),
                };
                r.to_bits() as u64
            }};
        }
        if self.bits == 32 { with!(f32, u32) } else { with!(f64, u64) }
    }

    /// A lane of `op` on lanes `a` and `b`, as bits.
    fn lane(self, op: &str, a: u64, b: u64, c: u64, k: u32) -> u64 {
        if self.kind == Kind::Float && !matches!(op, "and" | "or" | "xor" | "not") {
            return self.float(a, b, c, op);
        }
        let (sa, sb) = (self.signed(a) as i128, self.signed(b) as i128);
        let (ua, ub) = (a as i128, b as i128);
        let signed = self.kind == Kind::Signed;
        let (min, max) = match signed {
            true => (-(1i128 << (self.bits - 1)), (1i128 << (self.bits - 1)) - 1),
            false => (0, self.mask() as i128),
        };
        let (x, y) = if signed { (sa, sb) } else { (ua, ub) };
        let wrap = |v: i128| (v as u64) & self.mask();
        match op {
            "add" => wrap(x + y),
            "sub" => wrap(x - y),
            "mul" => wrap(x.wrapping_mul(y)),
            "neg" => wrap(-x),
            "and" => a & b,
            "or" => a | b,
            "xor" => a ^ b,
            "not" => !a & self.mask(),
            "shl" => wrap(x << k),
            "shr" => wrap(x >> k),
            "min" => wrap(x.min(y)),
            "max" => wrap(x.max(y)),
            "abs" => wrap(x.abs()),
            "saturatingAdd" => wrap((x + y).clamp(min, max)),
            "saturatingSub" => wrap((x - y).clamp(min, max)),
            _ => unreachable!("{op}"),
        }
    }

    /// Whether `a op b` holds, for a comparison.
    fn compare(self, op: &str, a: u64, b: u64) -> bool {
        if self.kind == Kind::Float {
            let (x, y) = match self.bits {
                32 => (f32::from_bits(a as u32) as f64, f32::from_bits(b as u32) as f64),
                _ => (f64::from_bits(a), f64::from_bits(b)),
            };
            return match op {
                "equal" => x == y,
                "notEqual" => x != y,
                "less" => x < y,
                "lessOrEqual" => x <= y,
                "greater" => x > y,
                _ => x >= y,
            };
        }
        let (x, y) = match self.kind {
            Kind::Signed => (self.signed(a) as i128, self.signed(b) as i128),
            _ => (a as i128, b as i128),
        };
        match op {
            "equal" => x == y,
            "notEqual" => x != y,
            "less" => x < y,
            "lessOrEqual" => x <= y,
            "greater" => x > y,
            _ => x >= y,
        }
    }

    /// A value of this type written in Tondo.
    fn literal(self, v: u64) -> String {
        match self.kind {
            Kind::Signed => self.signed(v).to_string(),
            Kind::Float => format!("{}.fromBits({v})", self.name),
            _ => v.to_string(),
        }
    }

    /// A lane as the program prints it: its value, or a float's bits.
    fn shown(self, v: u64) -> String {
        match self.kind {
            Kind::Signed => self.signed(v).to_string(),
            _ => v.to_string(),
        }
    }

    /// How the program prints a vector of this type.
    fn vector(self, expression: &str) -> String {
        match self.kind {
            Kind::Float => format!("({expression}).cast[{}]()", self.twin),
            _ => expression.to_string(),
        }
    }

    /// How the program prints a lane of this type.
    fn scalar(self, expression: &str) -> String {
        match self.kind {
            Kind::Float => format!("({expression}).bits()"),
            _ => expression.to_string(),
        }
    }
}

/// What a lane is converted to by `to[U]`, of the same width.
fn conversions(lane: Lane) -> Vec<Lane> {
    if lane.bits < 32 || lane.kind == Kind::Byte {
        return Vec::new();
    }
    LANES.iter().copied().filter(|other| other.bits == lane.bits && other.kind != lane.kind).collect()
}

/// `to[U]` of one lane.
fn converted(from: Lane, to: Lane, v: u64) -> u64 {
    let float = |v: u64| if from.bits == 32 { f32::from_bits(v as u32) as f64 } else { f64::from_bits(v) };
    match (from.kind, to.kind) {
        (Kind::Float, Kind::Signed) => {
            if to.bits == 32 {
                (float(v) as i32) as u32 as u64
            } else {
                (float(v) as i64) as u64
            }
        }
        (Kind::Float, _) => {
            if to.bits == 32 {
                (float(v) as u32) as u64
            } else {
                float(v) as u64
            }
        }
        (Kind::Signed, Kind::Float) => {
            if to.bits == 32 {
                (from.signed(v) as f32).to_bits() as u64
            } else {
                (from.signed(v) as f64).to_bits()
            }
        }
        (_, Kind::Float) => {
            if to.bits == 32 {
                (v as f32).to_bits() as u64
            } else {
                (v as f64).to_bits()
            }
        }
        // A negative value is zero; an unsigned one past the greatest signed
        // one is it.
        (Kind::Signed, _) => from.signed(v).max(0) as u64,
        _ => v.min((1u64 << (from.bits - 1)) - 1),
    }
}

/// The program for one type of lanes, and what it must print.
fn program(lane: Lane, random: &mut Random) -> (String, String) {
    let mut code = String::from(
        "import std.console\nimport std.simd\n\nfn main() {\n    check128()\n    check256()\n    check512()\n}\n",
    );
    let mut expected = String::new();
    let t = lane.name;
    let show = |values: &[u64]| format!("<{}>", values.iter().map(|&v| lane.shown(v)).collect::<Vec<_>>().join(", "));
    for width in [128u32, 256, 512] {
        let n = (width / lane.bits) as usize;
        let v = format!("simd.Vector{width}[{t}]");
        let _ = writeln!(code, "\nfn check{width}() {{");
        let (a, b, c) = (lane.values(random, n), lane.values(random, n), lane.values(random, n));
        let array = |values: &[u64]| values.iter().map(|&x| lane.literal(x)).collect::<Vec<_>>().join(", ");
        let _ = writeln!(code, "    let a{width}: Array[{t}] = [{}]", array(&a));
        let _ = writeln!(code, "    let b{width}: Array[{t}] = [{}]", array(&b));
        if lane.kind == Kind::Float {
            let _ = writeln!(code, "    let c{width}: Array[{t}] = [{}]", array(&c));
        }
        let _ = writeln!(code, "    let a = {v}.load(ref a{width})");
        let _ = writeln!(code, "    let b = {v}.load(ref b{width})");
        if lane.kind == Kind::Float {
            let _ = writeln!(code, "    let c = {v}.load(ref c{width})");
        }
        macro_rules! line {
            ($name:expr, $printed:expr, $wanted:expr) => {{
                let (name, printed, wanted): (&str, String, String) = ($name, $printed, $wanted);
                let _ = writeln!(code, "    console.println(\"{name} {width}: {{{printed}}}\")");
                let _ = writeln!(expected, "{name} {width}: {wanted}");
            }};
        }
        let each = |f: &dyn Fn(usize) -> u64| (0..n).map(f).collect::<Vec<u64>>();
        line!("load", lane.vector("a"), show(&a));
        let k = (random.next() % lane.bits as u64) as u32;
        let mut binary = vec![];
        if lane.arithmetic() {
            binary.extend([
                ("add", "a + b"),
                ("sub", "a - b"),
                ("mul", "a * b"),
                ("min", "a.min(b)"),
                ("max", "a.max(b)"),
            ]);
        }
        if lane.kind == Kind::Float {
            binary.push(("div", "a / b"));
        }
        if lane.bitwise() {
            binary.extend([("and", "a & b"), ("or", "a | b"), ("xor", "a ^ b")]);
        }
        if lane.integer() {
            binary.extend([("saturatingAdd", "a.saturatingAdd(b)"), ("saturatingSub", "a.saturatingSub(b)")]);
        }
        for (op, expression) in binary {
            line!(op, lane.vector(expression), show(&each(&|i| lane.lane(op, a[i], b[i], c[i], k))));
        }
        let mut unary = vec![];
        if matches!(lane.kind, Kind::Signed | Kind::Float) {
            unary.push(("neg", "-a"));
        }
        if lane.arithmetic() {
            unary.push(("abs", "a.abs()"));
        }
        if lane.bitwise() {
            unary.extend([("not", "~a"), ("shl", "a << SHIFT"), ("shr", "a >> SHIFT")]);
        }
        if lane.kind == Kind::Float {
            unary.extend([
                ("sqrt", "a.sqrt()"),
                ("floor", "a.floor()"),
                ("ceil", "a.ceil()"),
                ("round", "a.round()"),
                ("truncate", "a.truncate()"),
                ("fma", "a.fma(b, c)"),
            ]);
        }
        for (op, expression) in unary {
            let expression = expression.replace("SHIFT", &k.to_string());
            line!(op, lane.vector(&expression), show(&each(&|i| lane.lane(op, a[i], b[i], c[i], k))));
        }
        // A `T` stands for every lane, on either side.
        if lane.arithmetic() {
            let three = lane.number_bits(3);
            let wanted = each(&|i| lane.lane("add", lane.lane("mul", a[i], three, 0, 0), b[i], 0, 0));
            let three = if lane.kind == Kind::Float { "3.0" } else { "3" };
            line!("splat", lane.vector(&format!("{three} * a + b")), show(&wanted));
        }
        for op in ["equal", "notEqual", "less", "lessOrEqual", "greater", "greaterOrEqual"] {
            let bits = (0..n).filter(|&i| lane.compare(op, a[i], b[i])).fold(0u64, |acc, i| acc | 1 << i);
            line!(op, format!("a.{op}(b).bits()"), bits.to_string());
        }
        let less: Vec<bool> = (0..n).map(|i| lane.compare("less", a[i], b[i])).collect();
        let count = less.iter().filter(|&&x| x).count();
        line!("any", "a.less(b).any()".into(), (count > 0).to_string());
        line!("all", "a.less(b).all()".into(), (count == n).to_string());
        line!("count", "a.less(b).count()".into(), count.to_string());
        line!("choose", lane.vector("a.less(b).choose(a, b)"), show(&each(&|i| if less[i] { a[i] } else { b[i] })));
        line!("maskLane", "a.less(b).lane(1)".into(), less[1].to_string());
        if lane.arithmetic() {
            for (op, combine) in [("sum", "add"), ("minLane", "min"), ("maxLane", "max")] {
                let mut lanes = a.clone();
                while lanes.len() > 1 {
                    let half = lanes.len() / 2;
                    lanes = (0..half).map(|j| lane.lane(combine, lanes[j], lanes[j + half], 0, 0)).collect();
                }
                line!(op, lane.scalar(&format!("a.{op}()")), lane.shown(lanes[0]));
            }
        }
        let reversed: Vec<u64> = a.iter().rev().copied().collect();
        line!("reversed", lane.vector("a.reversed()"), show(&reversed));
        line!("lane", lane.scalar(&format!("a.lane({})", n - 1)), lane.shown(a[n - 1]));
        let mut changed = a.clone();
        changed[1] = b[0];
        line!("withLane", lane.vector("a.withLane(1, b.lane(0))"), show(&changed));
        line!("toArray", lane.vector(&format!("{v}.of(...a.toArray())")), show(&a));
        let mut stored = vec![0u64; n + 2];
        stored[1..=n].copy_from_slice(&a);
        let _ = writeln!(code, "    var into{width}: Array[{t}] = [a{width}[0]].repeat({})", n + 2);
        let _ = writeln!(code, "    {v}.zero().store(mut into{width})");
        let _ = writeln!(code, "    {v}.zero().store(mut into{width}, at: 2)");
        let _ = writeln!(code, "    a.store(mut into{width}, at: 1)");
        stored[0] = 0;
        stored[n + 1] = 0;
        line!("store", lane.vector(&format!("{v}.load(ref into{width}, at: 1)")), show(&stored[1..=n]));
        line!("stored", lane.scalar(&format!("into{width}[0]")), "0".into());
        for to in conversions(lane) {
            let wanted: Vec<u64> = a.iter().map(|&x| converted(lane, to, x)).collect();
            let shown = wanted.iter().map(|&x| to.shown(x)).collect::<Vec<_>>().join(", ");
            line!(&format!("to{}", to.name), to.vector(&format!("a.to[{}]()", to.name)), format!("<{shown}>"));
        }
        if width > 128 {
            let half = format!("simd.Vector{}[{t}]", width / 2);
            line!("lower", lane.vector("a.lower()"), show(&a[..n / 2]));
            line!("upper", lane.vector("a.upper()"), show(&a[n / 2..]));
            let joined: Vec<u64> = a[..n / 2].iter().chain(&b[n / 2..]).copied().collect();
            line!("joined", lane.vector(&format!("{v}.joined(a.lower(), b.upper())")), show(&joined));
            let _ = half;
        }
        if width == 128 && lane.bits == 8 {
            let indexes: Vec<u64> = (0..n).map(|_| random.next() % 32).collect();
            let _ = writeln!(
                code,
                "    let indexes = {v}.of({})",
                indexes.iter().map(|&x| lane.literal(x)).collect::<Vec<_>>().join(", ")
            );
            let wanted: Vec<u64> = indexes.iter().map(|&i| if i < 16 { a[i as usize] } else { 0 }).collect();
            line!("swizzle", lane.vector("a.swizzle(indexes)"), show(&wanted));
        }
        line!("lanes", format!("{v}.lanes()"), n.to_string());
        line!("accelerated", format!("{v}.accelerated()"), (width == 128).to_string());
        code.push_str("}\n");
    }
    (code, expected)
}

impl Lane {
    /// The bits of a small whole number of this type.
    fn number_bits(self, value: i64) -> u64 {
        match self.kind {
            Kind::Float => self.float_bits(value as f64),
            _ => (value as u64) & self.mask(),
        }
    }
}

/// Whether two printed results agree: as text, but for a float's lanes, any
/// NaN with any other, as their payloads differ by processor.
fn agree(lane: Lane, printed: &str, wanted: &str) -> bool {
    if printed == wanted {
        return true;
    }
    let numbers = |text: &str| -> Option<Vec<u64>> {
        let inner = text.split_once(": ")?.1.trim_start_matches('<').trim_end_matches('>');
        inner.split(", ").map(|n| n.parse::<u64>().ok()).collect()
    };
    let name = |text: &str| text.split_once(": ").map(|(name, _)| name.to_string());
    if lane.kind != Kind::Float || name(printed) != name(wanted) {
        return false;
    }
    match (numbers(printed), numbers(wanted)) {
        (Some(a), Some(b)) => {
            a.len() == b.len() && a.iter().zip(&b).all(|(&x, &y)| x == y || (lane.is_nan(x) && lane.is_nan(y)))
        }
        _ => false,
    }
}

/// Whether this machine's level does vectors of 256 bits with instructions
/// of that width: an x64 with AVX2 (step 22b).
fn wide_here() -> bool {
    #[cfg(target_arch = "x86_64")]
    return std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma");
    #[cfg(not(target_arch = "x86_64"))]
    return false;
}

/// At the baseline and at this machine's level, where an x64 with AVX2 does
/// vectors of 256 bits with its instructions of 256 bits: the definitions,
/// and the same bits at both levels (rule 15).
#[test]
fn every_operation_of_every_lane_is_its_definition() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("simd");
    std::fs::create_dir_all(&dir).unwrap();
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut failures = Vec::new();
    for lane in LANES {
        let (code, expected) = program(lane, &mut random);
        let file = dir.join(format!("{}.to", lane.name.to_lowercase()));
        std::fs::write(&file, &code).unwrap();
        let mut outputs = Vec::new();
        for (cpu, wide) in [("baseline", false), ("native", wide_here())] {
            let run = Command::new(env!("CARGO_BIN_EXE_tondo"))
                .args(["run", &file.to_string_lossy(), "--cpu", cpu])
                .output()
                .unwrap();
            let printed = String::from_utf8_lossy(&run.stdout).into_owned();
            if !run.status.success() {
                failures.push(format!("{} at {cpu}: {}", lane.name, String::from_utf8_lossy(&run.stderr)));
                continue;
            }
            let expected = if wide {
                expected.replace("accelerated 256: false", "accelerated 256: true")
            } else {
                expected.clone()
            };
            let mut printed_lines = printed.lines();
            for wanted in expected.lines() {
                let got = printed_lines.next().unwrap_or("");
                if !agree(lane, got, wanted) {
                    failures.push(format!(
                        "{} at {cpu} ({}):\n  printed {got}\n  wanted  {wanted}",
                        lane.name,
                        file.display()
                    ));
                }
            }
            outputs
                .push(printed.lines().filter(|line| !line.starts_with("accelerated")).collect::<Vec<_>>().join("\n"));
        }
        if let [baseline, native] = outputs.as_slice()
            && baseline != native
        {
            failures.push(format!("{}: the bits differ between the baseline and this machine's level", lane.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The sum of floats in the order the language fixes (18.5): element `i`
/// into partial sum `i mod lanes`, then the partial sums by halves.
fn fixed_sum(lane: Lane, values: &[u64]) -> u64 {
    let lanes = if lane.bits == 32 { 16 } else { 8 };
    let zero = lane.float_bits(0.0);
    let mut partial = vec![zero; lanes];
    for (i, &value) in values.iter().enumerate() {
        partial[i % lanes] = lane.float(partial[i % lanes], value, 0, "add");
    }
    let mut lanes = lanes;
    while lanes > 1 {
        lanes /= 2;
        for j in 0..lanes {
            partial[j] = lane.float(partial[j], partial[j + lanes], 0, "add");
        }
    }
    partial[0]
}

/// `sum` and `math.dot` of floats (21): their definition, for every length
/// from 0 to 70, at the program's level and at this machine's.
#[test]
fn sums_of_floats_add_in_the_fixed_order() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("simd-sums");
    std::fs::create_dir_all(&dir).unwrap();
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let mut code = String::from("import std.console\nimport std.math\n\nfn main() {\n");
    let mut expected = String::new();
    for lane in [LANES[9], LANES[10]] {
        for length in 0..=70 {
            let (a, b) = (lane.values(&mut random, length), lane.values(&mut random, length));
            let array = |values: &[u64]| values.iter().map(|&x| lane.literal(x)).collect::<Vec<_>>().join(", ");
            let t = lane.name;
            let _ = writeln!(code, "    let a{t}{length}: Array[{t}] = [{}]", array(&a));
            let _ = writeln!(code, "    let b{t}{length}: Array[{t}] = [{}]", array(&b));
            let _ = writeln!(code, "    console.println(\"sum {t} {length}: {{a{t}{length}.sum().bits()}}\")");
            let _ = writeln!(
                code,
                "    console.println(\"dot {t} {length}: {{math.dot(a{t}{length}, b{t}{length}).bits()}}\")"
            );
            let products: Vec<u64> = a.iter().zip(&b).map(|(&x, &y)| lane.float(x, y, 0, "mul")).collect();
            let _ = writeln!(expected, "sum {t} {length}: {}", fixed_sum(lane, &a));
            let _ = writeln!(expected, "dot {t} {length}: {}", fixed_sum(lane, &products));
        }
    }
    code.push_str("}\n");
    let file = dir.join("sums.to");
    std::fs::write(&file, &code).unwrap();
    let mut outputs = Vec::new();
    for cpu in ["baseline", "native"] {
        let run = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["run", &file.to_string_lossy(), "--cpu", cpu])
            .output()
            .unwrap();
        assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
        outputs.push(String::from_utf8_lossy(&run.stdout).into_owned());
    }
    // The same bits at every level (rule 2 of change 21).
    assert_eq!(outputs[0], outputs[1], "a sum differs between the baseline and this machine's level");
    let mut failures = Vec::new();
    for (got, wanted) in outputs[0].lines().zip(expected.lines()) {
        let lane = if wanted.contains("Float32") { LANES[9] } else { LANES[10] };
        let value = |line: &str| line.rsplit(": ").next().and_then(|n| n.parse::<u64>().ok());
        let same = match (value(got), value(wanted)) {
            (Some(x), Some(y)) => x == y || (lane.is_nan(x) && lane.is_nan(y)),
            _ => false,
        };
        if !same {
            failures.push(format!("printed {got}\n  wanted  {wanted}"));
        }
    }
    assert_eq!(outputs[0].lines().count(), expected.lines().count(), "{}", outputs[0]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The arithmetic of arrays of floats (10.6), done with vectors: each
/// element is the scalar operator's, with another array or a number on
/// either side, for every length from 0 to 70.
#[test]
fn arithmetic_of_float_arrays_is_the_scalar_operator() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("simd-arrays");
    std::fs::create_dir_all(&dir).unwrap();
    let mut random = Random(0x6a09_e667_f3bc_c909);
    // A function for each length, which `main` calls in order.
    let (mut code, mut calls) = (String::from("import std.console\n"), String::from("\nfn main() {\n"));
    let mut expected = Vec::new();
    for lane in [LANES[9], LANES[10]] {
        let t = lane.name;
        for length in 0..=70 {
            let _ = writeln!(code, "\nfn check{t}{length}() {{");
            let _ = writeln!(calls, "    check{t}{length}()");
            let (a, b, s) =
                (lane.values(&mut random, length), lane.values(&mut random, length), lane.values(&mut random, 1)[0]);
            let array = |values: &[u64]| values.iter().map(|&x| lane.literal(x)).collect::<Vec<_>>().join(", ");
            let _ = writeln!(code, "    let a{t}{length}: Array[{t}] = [{}]", array(&a));
            let _ = writeln!(code, "    let b{t}{length}: Array[{t}] = [{}]", array(&b));
            let _ = writeln!(code, "    let s{t}{length} = {}", lane.literal(s));
            for (op, symbol) in [("add", "+"), ("sub", "-"), ("mul", "*"), ("div", "/")] {
                let forms = [
                    (
                        format!("a{t}{length} {symbol} b{t}{length}"),
                        (0..length).map(|i| lane.float(a[i], b[i], 0, op)).collect::<Vec<_>>(),
                    ),
                    (
                        format!("a{t}{length} {symbol} s{t}{length}"),
                        (0..length).map(|i| lane.float(a[i], s, 0, op)).collect(),
                    ),
                    (
                        format!("s{t}{length} {symbol} b{t}{length}"),
                        (0..length).map(|i| lane.float(s, b[i], 0, op)).collect(),
                    ),
                ];
                for (expression, wanted) in forms {
                    let _ = writeln!(code, "    console.println(\"{{({expression}).map((x) {{ x.bits() }})}}\")");
                    expected.push((lane, expression, wanted));
                }
            }
            code.push_str("}\n");
        }
    }
    code.push_str(&calls);
    code.push_str("}\n");
    let file = dir.join("arrays.to");
    std::fs::write(&file, &code).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_tondo")).args(["run", &file.to_string_lossy()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let printed = String::from_utf8_lossy(&run.stdout);
    let mut failures = Vec::new();
    for (line, (lane, expression, wanted)) in printed.lines().zip(&expected) {
        let got: Vec<u64> = line
            .trim_matches(|c| c == '[' || c == ']')
            .split(", ")
            .filter(|n| !n.is_empty())
            .map(|n| n.parse().unwrap())
            .collect();
        let same = got.len() == wanted.len()
            && got.iter().zip(wanted).all(|(&x, &y)| x == y || (lane.is_nan(x) && lane.is_nan(y)));
        if !same {
            failures.push(format!("{expression}:\n  printed {got:?}\n  wanted  {wanted:?}"));
        }
    }
    assert_eq!(printed.lines().count(), expected.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
