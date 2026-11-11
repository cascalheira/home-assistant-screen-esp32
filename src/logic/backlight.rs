//! From the brightness people pick (1-100) to the backlight's PWM duty.
//!
//! The eye sees light roughly on a square-law scale, and an LED backlight
//! at a few percent duty is already nearly black. So the lowest setting maps
//! to MIN_PERMILLE, still readable in a dark room, and the rest follows a
//! square curve, which makes equal slider steps look like equal changes.

/// Duty at brightness 1, in tenths of a percent.
pub const MIN_PERMILLE: u32 = 60;

/// PWM duty in tenths of a percent (0-1000) for a brightness of 0-100.
/// 0 is off.
pub fn duty_permille(percent: u8) -> u32 {
    let p = percent.min(100) as u32;
    if p == 0 {
        return 0;
    }
    MIN_PERMILLE + (1000 - MIN_PERMILLE) * p * p / 10_000
}
