//! Choosing between the clock, the forecast and photos; night hours; the
//! burn-in shift.

use esp32_screen_host_tests::logic::screensaver::{is_night, shift, view, View, Views};

const CLOCK: Option<View> = Some(View::Clock);
const FORECAST: Option<View> = Some(View::Forecast);
const PHOTO: Option<View> = Some(View::Photo);

fn on(clock: bool, forecast: bool, forecast_ready: bool) -> Views {
    Views {
        clock,
        forecast,
        forecast_ready,
        ..Default::default()
    }
}

#[test]
fn single_views() {
    assert_eq!(view(on(false, false, true), None, 0, 20), None);
    assert_eq!(view(on(true, false, true), FORECAST, 0, 20), CLOCK);
    // The forecast alone shows even without data (it says what is missing).
    assert_eq!(view(on(false, true, false), None, 0, 20), FORECAST);
    let photos = Views {
        photos: true,
        ..Default::default()
    };
    assert_eq!(view(photos, None, 0, 20), PHOTO);
}

#[test]
fn both_take_turns() {
    let v = on(true, true, true);
    assert_eq!(view(v, None, 0, 20), CLOCK, "the clock comes first");
    assert_eq!(view(v, CLOCK, 19, 20), CLOCK);
    assert_eq!(view(v, CLOCK, 20, 20), FORECAST);
    assert_eq!(view(v, FORECAST, 5, 20), FORECAST);
    assert_eq!(view(v, FORECAST, 20, 20), CLOCK);
}

#[test]
fn both_without_forecast_shows_the_clock() {
    assert_eq!(view(on(true, true, false), None, 0, 20), CLOCK);
    assert_eq!(view(on(true, true, false), FORECAST, 0, 20), CLOCK);
}

#[test]
fn three_take_turns_and_skip_what_is_not_ready() {
    let mut v = Views {
        clock: true,
        forecast: true,
        forecast_ready: true,
        photos: true,
        photo_ready: true,
    };
    assert_eq!(view(v, CLOCK, 20, 20), FORECAST);
    assert_eq!(view(v, FORECAST, 20, 20), PHOTO);
    assert_eq!(view(v, PHOTO, 20, 20), CLOCK);
    v.photo_ready = false;
    assert_eq!(view(v, FORECAST, 20, 20), CLOCK);
    assert_eq!(
        view(v, PHOTO, 0, 20),
        CLOCK,
        "a photo view without a photo ends"
    );
    v.forecast = false;
    v.photo_ready = true;
    assert_eq!(view(v, CLOCK, 20, 20), PHOTO);
    assert_eq!(view(v, PHOTO, 20, 20), CLOCK);
}

#[test]
fn forecast_and_photos_without_data_show_the_forecast() {
    let v = Views {
        forecast: true,
        photos: true,
        ..Default::default()
    };
    assert_eq!(view(v, None, 0, 20), FORECAST);
}

#[test]
fn night_hours() {
    assert!(is_night(23 * 60, 22 * 60, 7 * 60));
    assert!(is_night(3 * 60, 22 * 60, 7 * 60));
    assert!(!is_night(7 * 60, 22 * 60, 7 * 60));
    assert!(!is_night(12 * 60, 22 * 60, 7 * 60));
    assert!(is_night(90, 60, 6 * 60), "a night after midnight");
    assert!(!is_night(0, 60, 6 * 60));
    assert!(!is_night(100, 300, 300), "an empty night");
}

#[test]
fn shift_walks_one_cell_at_a_time_and_stays_small() {
    let mut seen = std::collections::BTreeSet::new();
    for step in 0..200 {
        let (x, y) = shift(step);
        let (nx, ny) = shift(step + 1);
        assert!(x.abs() <= 8 && y.abs() <= 8);
        assert!((nx - x).abs() + (ny - y).abs() <= 4, "step {step} jumps");
        seen.insert((x, y));
    }
    assert_eq!(seen.len(), 25, "every grid position is used");
}
