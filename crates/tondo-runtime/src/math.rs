//! The scalar math of `std.math` and of the prelude's `Float` and `Char`
//! methods (standard library 14.1): IEEE-754 operations, without fast-math.

macro_rules! unary {
    ($($name:ident => $op:expr),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $name(x: f64) -> f64 {
                let op: fn(f64) -> f64 = $op;
                op(x)
            }
        )*
    };
}

unary! {
    tondo_rt_math_sin => f64::sin,
    tondo_rt_math_cos => f64::cos,
    tondo_rt_math_tan => f64::tan,
    tondo_rt_math_asin => f64::asin,
    tondo_rt_math_acos => f64::acos,
    tondo_rt_math_atan => f64::atan,
    tondo_rt_math_exp => f64::exp,
    tondo_rt_math_ln => f64::ln,
    tondo_rt_math_log10 => f64::log10,
    tondo_rt_math_log2 => f64::log2,
    tondo_rt_math_sqrt => f64::sqrt,
    tondo_rt_math_floor => f64::floor,
    tondo_rt_math_ceil => f64::ceil,
    tondo_rt_math_round => f64::round_ties_even,
    tondo_rt_math_round_away => f64::round,
    tondo_rt_math_trunc => f64::trunc,
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_math_hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_math_pow(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_math_fma(a: f64, b: f64, c: f64) -> f64 {
    a.mul_add(b, c)
}

/// 0 for a finite number, 1 for an infinity, 2 for NaN.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_math_class(x: f64) -> i64 {
    if x.is_nan() {
        2
    } else if x.is_infinite() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_code(c: u32) -> i64 {
    c as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_from(code: i64) -> u32 {
    u32::try_from(code).ok().and_then(char::from_u32).map_or(0xFFFD, |c| c as u32)
}

/// The sums of `std`'s `sum` and `math.dot` of floats (language 18.5): value
/// `i` into partial sum `i mod LANES`, then the partial sums by halves, each
/// addition one of the type, never fused with a product. Each partial sum
/// takes its values in order, so the compiler's vectors give the same bits.
macro_rules! fixed_sums {
    ($t:ty, $lanes:literal) => {
        fn reduce(mut partial: [$t; $lanes]) -> $t {
            let mut lanes = $lanes;
            while lanes > 1 {
                lanes /= 2;
                for j in 0..lanes {
                    partial[j] += partial[j + lanes];
                }
            }
            partial[0]
        }

        pub fn sum(values: &[$t]) -> $t {
            let mut partial = [0.0 as $t; $lanes];
            let chunks = values.chunks_exact($lanes);
            let rest = chunks.remainder();
            for chunk in chunks {
                for j in 0..$lanes {
                    partial[j] += chunk[j];
                }
            }
            for (j, &value) in rest.iter().enumerate() {
                partial[j] += value;
            }
            reduce(partial)
        }

        /// `dot` of `a` and the values of `b` from its first by `step`, as
        /// many as `a` has, in the same order; `b` holds them all.
        pub fn dot_by(a: &[$t], b: &[$t], step: usize) -> $t {
            assert!(a.is_empty() || (a.len() - 1) * step < b.len(), "a column within its array");
            // In range, as the assertion says.
            let column = |i: usize| unsafe { *b.get_unchecked(i * step) };
            let mut partial = [0.0 as $t; $lanes];
            let chunks = a.chunks_exact($lanes);
            let rest = chunks.remainder();
            for (index, chunk) in chunks.enumerate() {
                for j in 0..$lanes {
                    partial[j] += chunk[j] * column(index * $lanes + j);
                }
            }
            let whole = a.len() - rest.len();
            for (j, &x) in rest.iter().enumerate() {
                partial[j] += x * column(whole + j);
            }
            reduce(partial)
        }

        /// The arrays have one length: the caller checked it.
        pub fn dot(a: &[$t], b: &[$t]) -> $t {
            let mut partial = [0.0 as $t; $lanes];
            let (chunks, others) = (a.chunks_exact($lanes), b.chunks_exact($lanes));
            let (rest, other_rest) = (chunks.remainder(), others.remainder());
            for (x, y) in chunks.zip(others) {
                for j in 0..$lanes {
                    partial[j] += x[j] * y[j];
                }
            }
            for (j, (&x, &y)) in rest.iter().zip(other_rest).enumerate() {
                partial[j] += x * y;
            }
            reduce(partial)
        }
    };
}

mod sums64 {
    fixed_sums!(f64, 8);
}

mod sums32 {
    fixed_sums!(f32, 16);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_sum_f64(values: *mut crate::ArrayHeader) -> f64 {
    sums64::sum(unsafe { crate::array_items::<f64>(values) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_dot_f64(a: *mut crate::ArrayHeader, b: *mut crate::ArrayHeader) -> f64 {
    sums64::dot(unsafe { crate::array_items::<f64>(a) }, unsafe { crate::array_items::<f64>(b) })
}

/// `math.dot` of `count` values of `a` from `a_start` and of `b` from
/// `b_start` by `b_step`, which a tensor's product takes from a row and a
/// column; the caller checked the ranges.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_dot_at_f64(
    a: *mut crate::ArrayHeader,
    a_start: i64,
    b: *mut crate::ArrayHeader,
    b_start: i64,
    b_step: i64,
    count: i64,
) -> f64 {
    let (a, b) = unsafe { (crate::array_items::<f64>(a), crate::array_items::<f64>(b)) };
    let (a_start, b_start, b_step) = (a_start as usize, b_start as usize, b_step as usize);
    sums64::dot_by(&a[a_start..a_start + count as usize], &b[b_start..], b_step)
}

/// A tensor's `long op short`, element by element, `short` repeated along
/// `long`, whose length is a multiple of its own; with `short_first`,
/// `short op long`. `op` is 0 for `+`, 1 for `-`, 2 for `*` and 3 for `/`:
/// a loop for each, which the compiler makes with vector instructions.
fn cycled<F>(long: &[F], short: &[F], op: i64, short_first: bool) -> *mut crate::ArrayHeader
where
    F: Copy
        + std::ops::Add<Output = F>
        + std::ops::Sub<Output = F>
        + std::ops::Mul<Output = F>
        + std::ops::Div<Output = F>,
{
    let size = std::mem::size_of::<F>();
    let result = crate::arrays::array_alloc(size, long.len(), long.len());
    let out = unsafe { std::slice::from_raw_parts_mut(crate::arrays::element(result, size, 0) as *mut F, long.len()) };
    if short.is_empty() {
        return result;
    }
    fn each<F: Copy>(out: &mut [F], long: &[F], short: &[F], f: impl Fn(F, F) -> F) {
        for (into, from) in out.chunks_exact_mut(short.len()).zip(long.chunks_exact(short.len())) {
            for ((slot, &x), &y) in into.iter_mut().zip(from).zip(short) {
                *slot = f(x, y);
            }
        }
    }
    match (op, short_first) {
        (0, _) => each(out, long, short, |x, y| x + y),
        (1, false) => each(out, long, short, |x, y| x - y),
        (1, true) => each(out, long, short, |x, y| y - x),
        (2, _) => each(out, long, short, |x, y| x * y),
        (_, false) => each(out, long, short, |x, y| x / y),
        (_, true) => each(out, long, short, |x, y| y / x),
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_cycled_f64(
    long: *mut crate::ArrayHeader,
    short: *mut crate::ArrayHeader,
    op: i64,
    short_first: bool,
) -> *mut crate::ArrayHeader {
    cycled(unsafe { crate::array_items::<f64>(long) }, unsafe { crate::array_items::<f64>(short) }, op, short_first)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_cycled_f32(
    long: *mut crate::ArrayHeader,
    short: *mut crate::ArrayHeader,
    op: i64,
    short_first: bool,
) -> *mut crate::ArrayHeader {
    cycled(unsafe { crate::array_items::<f32>(long) }, unsafe { crate::array_items::<f32>(short) }, op, short_first)
}

// A `Float32` goes back as its bits, which a `Float` would change for a NaN.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_dot_at_f32(
    a: *mut crate::ArrayHeader,
    a_start: i64,
    b: *mut crate::ArrayHeader,
    b_start: i64,
    b_step: i64,
    count: i64,
) -> i64 {
    let (a, b) = unsafe { (crate::array_items::<f32>(a), crate::array_items::<f32>(b)) };
    let (a_start, b_start, b_step) = (a_start as usize, b_start as usize, b_step as usize);
    sums32::dot_by(&a[a_start..a_start + count as usize], &b[b_start..], b_step).to_bits() as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_sum_f32(values: *mut crate::ArrayHeader) -> i64 {
    sums32::sum(unsafe { crate::array_items::<f32>(values) }).to_bits() as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_dot_f32(a: *mut crate::ArrayHeader, b: *mut crate::ArrayHeader) -> i64 {
    sums32::dot(unsafe { crate::array_items::<f32>(a) }, unsafe { crate::array_items::<f32>(b) }).to_bits() as i64
}
