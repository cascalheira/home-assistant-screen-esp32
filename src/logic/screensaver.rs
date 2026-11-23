//! Which screensaver shows (the clock, the forecast, photos, taking turns),
//! night hours, and the slow shift that keeps the clock from burning in.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Clock,
    Forecast,
    Photo,
    /// Nothing (night mode "screen off"); never chosen by `view`.
    Blank,
}

/// What is turned on and what has data.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Views {
    pub clock: bool,
    pub forecast: bool,
    /// Home Assistant sent at least one forecast day.
    pub forecast_ready: bool,
    pub photos: bool,
    /// A photo is ready to show.
    pub photo_ready: bool,
}

/// The view to show now, or None when the screensaver is off.
///
/// Views take turns in the order clock, forecast, photo, each staying
/// `rotate_secs`. A view without data is skipped while another one can show;
/// a view turned on alone shows anyway (it says what is missing).
/// `current` and `shown_secs`: the view on screen and for how long.
pub fn view(v: Views, current: Option<View>, shown_secs: u64, rotate_secs: u64) -> Option<View> {
    let on = [
        (View::Clock, v.clock, true),
        (View::Forecast, v.forecast, v.forecast_ready),
        (View::Photo, v.photos, v.photo_ready),
    ];
    let enabled: Vec<View> = on.iter().filter(|o| o.1).map(|o| o.0).collect();
    let ready: Vec<View> = on.iter().filter(|o| o.1 && o.2).map(|o| o.0).collect();
    let candidates = match (enabled.len(), ready.is_empty()) {
        (0, _) => return None,
        (1, _) => return Some(enabled[0]),
        // Several on, none with data: the first one explains.
        (_, true) => return Some(enabled[0]),
        _ => ready,
    };
    match current {
        Some(view) if candidates.contains(&view) && shown_secs < rotate_secs => Some(view),
        Some(view) => {
            // The next one in order after the current view.
            let order = [View::Clock, View::Forecast, View::Photo];
            let at = order.iter().position(|&o| o == view).unwrap_or(0);
            (1..=3)
                .map(|step| order[(at + step) % 3])
                .find(|o| candidates.contains(o))
        }
        None => Some(candidates[0]),
    }
}

/// Whether `now` (minutes since midnight) is within the night from `from`
/// to `until` (minutes since midnight; the night may cross midnight).
pub fn is_night(now: i32, from: i32, until: i32) -> bool {
    if from == until {
        return false;
    }
    if from < until {
        (from..until).contains(&now)
    } else {
        now >= from || now < until
    }
}

/// Offset of the screensaver's contents (UI pixels) at step `step`, one
/// step a minute: a slow walk over a 5x5 grid, 4 px apart, that never jumps
/// by more than one cell, so each pixel's light changes gradually.
pub fn shift(step: u32) -> (i32, i32) {
    // Boustrophedon over the grid, then back: 0..25 forward, 25..50 back.
    const N: u32 = 5;
    const STEP_PX: i32 = 4;
    let cycle = 2 * N * N;
    let s = step % cycle;
    let i = if s < N * N { s } else { cycle - 1 - s };
    let row = i / N;
    let col = if row % 2 == 0 { i % N } else { N - 1 - i % N };
    let centre = (N / 2) as i32;
    (
        (col as i32 - centre) * STEP_PX,
        (row as i32 - centre) * STEP_PX,
    )
}
