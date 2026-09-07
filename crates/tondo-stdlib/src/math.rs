//! Deterministic scalar math kernels.
//!
//! These wrappers make the language-level error policy explicit while leaving
//! the actual IEEE-754 operations to the target's correctly rounded primitive.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    Domain,
    NonFinite,
    ResourceLimit,
}

pub fn floor(value: f64) -> f64 {
    value.floor()
}

pub fn ceil(value: f64) -> f64 {
    value.ceil()
}

/// Round to the nearest integer, choosing the even integer at an exact tie.
/// Preserve signed zero and infinities; a NaN input produces NaN.
pub fn round(value: f64) -> f64 {
    value.round_ties_even()
}

/// Round to the nearest integer, choosing the integer farther from zero at a tie.
/// Preserve signed zero and infinities; a NaN input produces NaN.
pub fn round_ties_away(value: f64) -> f64 {
    value.round()
}

pub fn truncate(value: f64) -> f64 {
    value.trunc()
}

pub fn sqrt(value: f64) -> Result<f64, MathError> {
    if value.is_nan() {
        return Ok(f64::NAN);
    }
    if value == f64::NEG_INFINITY {
        return Err(MathError::NonFinite);
    }
    if value < 0.0 {
        return Err(MathError::Domain);
    }
    Ok(value.sqrt())
}

pub fn fma(a: f64, b: f64, c: f64) -> f64 {
    a.mul_add(b, c)
}

pub fn abs(value: f64) -> f64 {
    value.abs()
}

pub fn min(a: f64, b: f64) -> f64 {
    a.min(b)
}

pub fn max(a: f64, b: f64) -> f64 {
    a.max(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_kernels_follow_ieee_values() {
        assert_eq!(floor(1.9), 1.0);
        assert_eq!(ceil(-1.1), -1.0);
        assert_eq!(round(1.5), 2.0);
        assert_eq!(truncate(-1.9), -1.0);
        assert!(floor(f64::NAN).is_nan());
        assert_eq!(floor(f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn rounding_modes_choose_explicit_ties_and_agree_beside_them() {
        for (value, even, away) in [
            (0.5_f64, 0.0_f64, 1.0_f64),
            (1.5, 2.0, 2.0),
            (2.5, 2.0, 3.0),
            (3.5, 4.0, 4.0),
            (-0.5, -0.0, -1.0),
            (-1.5, -2.0, -2.0),
            (-2.5, -2.0, -3.0),
            (-3.5, -4.0, -4.0),
        ] {
            assert_eq!(round(value).to_bits(), even.to_bits(), "{value}");
            assert_eq!(round_ties_away(value).to_bits(), away.to_bits(), "{value}");
            for operation in [round, round_ties_away] {
                assert_eq!(
                    operation(value.next_down()).to_bits(),
                    value.floor().to_bits()
                );
                assert_eq!(operation(value.next_up()).to_bits(), value.ceil().to_bits());
            }
        }
    }

    #[test]
    fn round_preserves_special_values_and_handles_subnormals_and_large_integers() {
        for value in [
            0.0_f64,
            -0.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            4_503_599_627_370_496.0,
            4_503_599_627_370_497.0,
        ] {
            for operation in [round, round_ties_away] {
                assert_eq!(operation(value).to_bits(), value.to_bits());
            }
        }
        for value in [f64::from_bits(1), f64::MIN_POSITIVE] {
            for operation in [round, round_ties_away] {
                assert_eq!(operation(value).to_bits(), 0.0_f64.to_bits());
                assert_eq!(operation(-value).to_bits(), (-0.0_f64).to_bits());
            }
        }
        assert!(round(f64::NAN).is_nan());
        assert!(round_ties_away(f64::NAN).is_nan());
    }

    #[test]
    fn sqrt_distinguishes_domain_and_nonfinite_inputs() {
        assert_eq!(sqrt(9.0), Ok(3.0));
        assert_eq!(sqrt(-1.0), Err(MathError::Domain));
        assert_eq!(sqrt(f64::NEG_INFINITY), Err(MathError::NonFinite));
        assert!(sqrt(f64::NAN).unwrap().is_nan());
        assert_eq!(sqrt(f64::INFINITY), Ok(f64::INFINITY));
    }

    #[test]
    fn fused_and_extrema_kernels_preserve_special_values() {
        assert_eq!(fma(2.0, 3.0, 4.0), 10.0);
        assert_eq!(abs(-3.5), 3.5);
        assert_eq!(min(-0.0, 0.0), -0.0);
        assert_eq!(max(-0.0, 0.0), 0.0);
        assert_eq!(min(f64::NAN, 1.0), 1.0);
        assert_eq!(max(f64::NEG_INFINITY, 2.0), 2.0);
    }

    #[test]
    fn scalar_kernels_cover_signed_zero_ties_and_nonfinite_boundaries() {
        assert_eq!(ceil(-1.5), -1.0);
        assert_eq!(round(-1.5), -2.0);
        assert_eq!(truncate(-0.75), -0.0);
        assert_eq!(abs(-0.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(floor(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert_eq!(ceil(f64::INFINITY), f64::INFINITY);
        assert!(round(f64::NAN).is_nan());
        assert!(truncate(f64::NAN).is_nan());
        assert!(abs(f64::NAN).is_nan());
        assert!(fma(f64::INFINITY, 0.0, 1.0).is_nan());
        assert_eq!(min(1.0, f64::NAN), 1.0);
        assert_eq!(max(1.0, f64::NAN), 1.0);
    }
}
