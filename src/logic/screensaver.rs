//! Which screensaver shows: the clock, the weather forecast, or both taking
//! turns.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Clock,
    Forecast,
}

/// The view to show now, or None when the screensaver is off.
///
/// - `clock`, `forecast`: what is turned on.
/// - `forecast_ready`: Home Assistant sent at least one forecast day. With
///   both on, the clock shows alone until then. With only the forecast on,
///   the forecast view shows anyway (it explains what is missing).
/// - `current` and `shown_secs`: the view on screen and for how long.
/// - `rotate_secs`: how long each view stays when both are on.
pub fn view(
    clock: bool,
    forecast: bool,
    forecast_ready: bool,
    current: Option<View>,
    shown_secs: u64,
    rotate_secs: u64,
) -> Option<View> {
    match (clock, forecast) {
        (false, false) => None,
        (true, false) => Some(View::Clock),
        (false, true) => Some(View::Forecast),
        (true, true) if !forecast_ready => Some(View::Clock),
        (true, true) => Some(match current {
            None => View::Clock,
            Some(view) if shown_secs < rotate_secs => view,
            Some(View::Clock) => View::Forecast,
            Some(View::Forecast) => View::Clock,
        }),
    }
}
