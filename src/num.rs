//! Named numeric conversions, so every lossy cast lives in one place with
//! its reason. Callers say which conversion they mean (`num::i64_f64`), and
//! the edge cases are decided here once:
//!
//! - Integer to float may round above 2^53 (f64) or 2^24 (f32). AiNKI's
//!   counts, seconds and milliseconds stay far below that.
//! - Float to integer drops the fraction toward zero and saturates at the
//!   target's range; NaN becomes 0. Callers round first when they mean it.
//! - Integer to integer clamps to the target's range instead of wrapping.

#[must_use]
#[expect(clippy::cast_precision_loss, reason = "exact below 2^53; counts and timestamps stay far below")]
pub const fn i64_f64(n: i64) -> f64 {
    n as f64
}

#[must_use]
#[expect(clippy::cast_precision_loss, reason = "exact below 2^53; lengths stay far below")]
pub const fn usize_f64(n: usize) -> f64 {
    n as f64
}

#[must_use]
#[expect(clippy::cast_precision_loss, reason = "exact below 2^53; byte sizes stay far below")]
pub const fn u64_f64(n: u64) -> f64 {
    n as f64
}

#[must_use]
#[expect(clippy::cast_precision_loss, reason = "exact below 2^24; used for day counts and seconds of FSRS math")]
pub const fn i64_f32(n: i64) -> f32 {
    n as f32
}

#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "saturating by definition of `as`; NaN becomes 0")]
pub const fn f64_i64(x: f64) -> i64 {
    x as i64
}

#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "FSRS works in f32; the lost digits are far below its precision")]
pub const fn f64_f32(x: f64) -> f32 {
    x as f32
}

/// Clamps to 0..=255 first; NaN becomes 0.
#[must_use]
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to the u8 range first")]
pub const fn f32_u8(x: f32) -> u8 {
    x.clamp(0.0, 255.0) as u8
}

#[must_use]
pub fn usize_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

#[must_use]
pub fn usize_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Negative values clamp to 0.
#[must_use]
pub fn i64_usize(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// Negative values clamp to 0, large ones to `u32::MAX`.
#[must_use]
pub fn i64_u32(n: i64) -> u32 {
    u32::try_from(n.max(0)).unwrap_or(u32::MAX)
}

/// Negative values clamp to 0.
#[must_use]
pub fn i64_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

#[must_use]
pub fn u64_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_clamp_instead_of_wrapping() {
        assert_eq!(f64_i64(f64::NAN), 0);
        assert_eq!(f64_i64(1e300), i64::MAX);
        assert_eq!(f64_i64(-2.7), -2);
        assert_eq!(i64_usize(-5), 0);
        assert_eq!(i64_u32(-1), 0);
        assert_eq!(i64_u32(i64::MAX), u32::MAX);
        assert_eq!(i64_u64(-1), 0);
        assert_eq!(usize_i64(7), 7);
        assert!((i64_f64(3) - 3.0).abs() < f64::EPSILON);
    }
}
