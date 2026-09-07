//! Bounded scalar owner properties shared by fuzzing and fixed regressions.

use tondo_stdlib::format::{self, Builder, FormatError, FormatLimits};
use tondo_stdlib::math::{self, MathError};

pub fn format(input: &[u8]) {
    let mut scalar = [0; 16];
    let length = scalar.len().min(input.len());
    scalar[..length].copy_from_slice(&input[..length]);
    let number = i128::from_le_bytes(scalar);
    let limits = FormatLimits {
        max_bytes: usize::from(input.first().copied().unwrap_or_default()),
    };
    let number_text = number.to_string();
    let expected = |text: String| {
        if text.len() <= limits.max_bytes {
            Ok(text)
        } else {
            Err(FormatError::ResourceLimit)
        }
    };
    assert_eq!(format::format(&number, limits), expected(number_text));
    let text = String::from_utf8_lossy(&input[..input.len().min(1024)]);
    let parts = ["Tondo", text.as_ref(), "🦀"];
    assert_eq!(format::join(&parts, "|", limits), expected(parts.join("|")));

    let prefix = "x".repeat(limits.max_bytes);
    let mut builder = Builder::new(limits);
    builder.append(&prefix).unwrap();
    assert_eq!(builder.append("🦀"), Err(FormatError::ResourceLimit));
    assert_eq!(builder.finish().unwrap(), prefix);
}

pub fn math(input: &[u8]) {
    let mut bits = [0; 8];
    let length = bits.len().min(input.len());
    bits[..length].copy_from_slice(&input[..length]);
    let value = f64::from_bits(u64::from_le_bytes(bits));
    let floor = math::floor(value);
    let ceil = math::ceil(value);
    let rounded = math::round(value);
    let rounded_away = math::round_ties_away(value);
    let truncated = math::truncate(value);
    if value.is_nan() {
        for result in [
            floor,
            ceil,
            rounded,
            rounded_away,
            truncated,
            math::sqrt(value).unwrap(),
        ] {
            assert!(result.is_nan());
        }
    } else if value.is_infinite() {
        for result in [floor, ceil, rounded, rounded_away, truncated] {
            assert_eq!(result.to_bits(), value.to_bits());
        }
        if value.is_sign_negative() {
            assert_eq!(math::sqrt(value), Err(MathError::NonFinite));
        } else {
            assert_eq!(math::sqrt(value), Ok(f64::INFINITY));
        }
    } else {
        assert!(floor <= value && ceil >= value && ceil - floor <= 1.0);
        assert!(floor == ceil || ceil - floor == 1.0);
        for result in [rounded, rounded_away] {
            assert!(result >= floor && result <= ceil);
            assert!((result - value).abs() <= 0.5);
        }
        assert_eq!(
            truncated.to_bits(),
            if value.is_sign_negative() {
                ceil
            } else {
                floor
            }
            .to_bits()
        );
        for result in [floor, ceil, rounded, rounded_away, truncated] {
            assert_eq!(result.fract(), 0.0);
            assert_eq!(math::round(result).to_bits(), result.to_bits());
            assert_eq!(math::round_ties_away(result).to_bits(), result.to_bits());
        }
        if value.abs().fract() == 0.5 {
            assert_eq!(
                rounded.to_bits(),
                // Select the even integer independently of the round primitive.
                if floor % 2.0 == 0.0 { floor } else { ceil }.to_bits()
            );
            assert_eq!(
                rounded_away.to_bits(),
                if value.is_sign_negative() {
                    floor
                } else {
                    ceil
                }
                .to_bits()
            );
        } else {
            assert_eq!(rounded.to_bits(), rounded_away.to_bits());
        }
        if value == 0.0 {
            for result in [
                floor,
                ceil,
                rounded,
                rounded_away,
                truncated,
                math::sqrt(value).unwrap(),
            ] {
                assert_eq!(result.to_bits(), value.to_bits());
            }
        } else if value < 0.0 {
            assert_eq!(math::sqrt(value), Err(MathError::Domain));
        } else {
            let root = math::sqrt(value).unwrap();
            assert!(root.is_finite() && root > 0.0);
            assert!((root - value / root).abs() <= root * (8.0 * f64::EPSILON));
        }
        assert_eq!(math::fma(value, 1.0, -0.0).to_bits(), value.to_bits());
    }

    // Small integer products and sums are exact in Float64. This oracle uses
    // integer arithmetic rather than the same floating-point primitive.
    let a = i64::from(i16::from_le_bytes([bits[0], bits[1]]));
    let b = i64::from(i16::from_le_bytes([bits[2], bits[3]]));
    let c = i64::from(i16::from_le_bytes([bits[4], bits[5]]));
    assert_eq!(math::fma(a as f64, b as f64, c as f64), (a * b + c) as f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_owner_checks_unicode_byte_limits_and_failed_append_atomicity() {
        format(&[]);
        format("Tondo 🦀".as_bytes());
        format(&i128::MIN.to_le_bytes());
        format(&i128::MAX.to_le_bytes());
        for limit in 0..=255 {
            format(&[limit; 32]);
        }
    }

    #[test]
    fn math_owner_checks_nonfinite_signed_zero_ties_and_subnormals() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -0.0,
            1.5,
            -1.5,
            2.5,
            -2.5,
            3.5,
            -3.5,
            0.5,
            -0.5,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            -f64::from_bits(1),
        ] {
            math(&value.to_bits().to_le_bytes());
        }
        for tie in [0.5_f64, 1.5, 2.5, 3.5, -0.5, -1.5, -2.5, -3.5] {
            for value in [tie.next_down(), tie.next_up()] {
                math(&value.to_bits().to_le_bytes());
            }
        }
        let mut state = 0x5a17_c9e3_d408_62bf_u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            math(&state.to_le_bytes());
        }
    }
}
