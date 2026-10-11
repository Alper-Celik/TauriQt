// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Unit conversions for `ops::screen`: Qt answers in device-independent
//! (logical) pixels, tauri's `Monitor` reports physical ones.
//!
//! Same rounding as tao's `to_physical` (`(value * scale).round()`), so the
//! monitors this runtime reports match what the X11 path in tao reports. The
//! monitor structs themselves are built in the runtime crate, which is where the
//! Qt screen info type lives.

/// Logical length → physical pixel (same rounding as tao's `to_physical`).
pub fn to_physical(value: i32, scale_factor: f64) -> i32 {
    (f64::from(value) * scale_factor).round() as i32
}

/// Logical length → physical pixel, clamped to the unsigned range.
pub fn to_physical_u32(value: i32, scale_factor: f64) -> u32 {
    to_physical(value, scale_factor).max(0) as u32
}

/// A reported scale factor, with a missing/zero/absurd one read as 1: a zero
/// ratio would silently collapse every value derived from it.
pub fn scale_factor_or_one(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_lengths_round_to_the_nearest_pixel() {
        assert_eq!(to_physical(3, 1.5), 5);
        assert_eq!(to_physical(4, 1.5), 6);
        assert_eq!(to_physical(-3, 1.5), -5);
        assert_eq!(to_physical(100, 1.0), 100);
    }

    #[test]
    fn unsigned_lengths_clamp_at_zero() {
        assert_eq!(to_physical_u32(-3, 1.0), 0);
        assert_eq!(to_physical_u32(-1, 2.0), 0);
        assert_eq!(to_physical_u32(3, 1.0), 3);
    }

    #[test]
    fn scale_factors_read_as_one_when_absurd() {
        assert_eq!(scale_factor_or_one(2.0), 2.0);
        assert_eq!(scale_factor_or_one(1.25), 1.25);
        assert_eq!(scale_factor_or_one(0.0), 1.0);
        assert_eq!(scale_factor_or_one(-1.0), 1.0);
        assert_eq!(scale_factor_or_one(f64::NAN), 1.0);
        assert_eq!(scale_factor_or_one(f64::INFINITY), 1.0);
    }
}
