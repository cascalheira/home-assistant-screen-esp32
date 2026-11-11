//! Text on climate tiles.

/// "18.8°" with one decimal, "19°" when whole.
pub fn current(value: f64) -> String {
    let tenths = (value * 10.0).round() as i64;
    if tenths % 10 == 0 {
        format!("{}°", tenths / 10)
    } else {
        format!("{:.1}°", tenths as f64 / 10.0)
    }
}

/// The target, with decimals only when the thermostat steps by less than 1.
pub fn target(value: f64, step: Option<f64>) -> String {
    if step.unwrap_or(0.5) < 1.0 {
        current(value)
    } else {
        format!("{}°", value.round() as i64)
    }
}
