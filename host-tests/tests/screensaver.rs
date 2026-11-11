//! Choosing between the clock and the forecast.

use esp32_screen_host_tests::logic::screensaver::{view, View};

const CLOCK: Option<View> = Some(View::Clock);
const FORECAST: Option<View> = Some(View::Forecast);

#[test]
fn single_views() {
    assert_eq!(view(false, false, true, None, 0, 20), None);
    assert_eq!(view(true, false, true, FORECAST, 0, 20), CLOCK);
    // The forecast alone shows even without data (it says what is missing).
    assert_eq!(view(false, true, false, None, 0, 20), FORECAST);
}

#[test]
fn both_take_turns() {
    assert_eq!(view(true, true, true, None, 0, 20), CLOCK, "the clock comes first");
    assert_eq!(view(true, true, true, CLOCK, 19, 20), CLOCK);
    assert_eq!(view(true, true, true, CLOCK, 20, 20), FORECAST);
    assert_eq!(view(true, true, true, FORECAST, 5, 20), FORECAST);
    assert_eq!(view(true, true, true, FORECAST, 20, 20), CLOCK);
}

#[test]
fn both_without_forecast_shows_the_clock() {
    assert_eq!(view(true, true, false, None, 0, 20), CLOCK);
    assert_eq!(view(true, true, false, FORECAST, 0, 20), CLOCK);
}
