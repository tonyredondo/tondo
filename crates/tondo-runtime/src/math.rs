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

// A `Float32` goes back as its bits, which a `Float` would change for a NaN.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_sum_f32(values: *mut crate::ArrayHeader) -> i64 {
    sums32::sum(unsafe { crate::array_items::<f32>(values) }).to_bits() as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_math_dot_f32(a: *mut crate::ArrayHeader, b: *mut crate::ArrayHeader) -> i64 {
    sums32::dot(unsafe { crate::array_items::<f32>(a) }, unsafe { crate::array_items::<f32>(b) }).to_bits() as i64
}
