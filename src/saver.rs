//! Optional screensaver clock (plain or flip clock).
//!
//! Starts after a period without touches while the dashboard is showing, once
//! the time is known (SNTP, timezone from Home Assistant). The touch that wakes
//! the screen is swallowed so it doesn't also press a tile. Settings live in
//! NVS (namespace "display").

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys;
use slint::ComponentHandle;

use crate::display::Backlight;
use crate::{AppWindow, Page};

const NVS_NAMESPACE: &str = "display";
const NVS_MODE: &str = "saver";
const NVS_DELAY: &str = "saver_delay";

/// Matches the options in the settings page.
const DELAYS: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
];
const FLIP_DURATION: Duration = Duration::from_millis(650);
const BRIGHTNESS_AWAKE: u8 = 100;
const BRIGHTNESS_SAVER: u8 = 35;

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Mode {
    Off = 0,
    Clock = 1,
    Flip = 2,
}

impl Mode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Clock,
            2 => Self::Flip,
            _ => Self::Off,
        }
    }
}

pub struct Saver {
    nvs: Option<EspNvs<NvsDefault>>,
    mode: Mode,
    delay: usize,
    active: bool,
    /// Set while the finger that woke the screen is still down.
    swallowing: bool,
    last_input: Instant,
    /// Hours and minutes shown, as `hour * 60 + minute`.
    shown_minute: Option<i32>,
    digits: [i32; 4],
    prev: [i32; 4],
    /// Flip progress per digit (0-1), driven from `tick`.
    progress: [f32; 4],
    flip_started: [Option<Instant>; 4],
}

impl Saver {
    pub fn new(nvs_partition: EspDefaultNvsPartition, ui: &AppWindow) -> Rc<RefCell<Self>> {
        let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)
            .map_err(|e| log::warn!("screensaver settings unavailable: {e}"))
            .ok();
        let read = |key| {
            nvs.as_ref()
                .and_then(|n| n.get_u8(key).ok().flatten())
                .unwrap_or(0)
        };
        let mode = Mode::from_u8(read(NVS_MODE));
        let delay = (read(NVS_DELAY) as usize).min(DELAYS.len() - 1);

        let saver = Rc::new(RefCell::new(Self {
            nvs,
            mode,
            delay,
            active: false,
            swallowing: false,
            last_input: Instant::now(),
            shown_minute: None,
            digits: [0; 4],
            prev: [0; 4],
            progress: [1.0; 4],
            flip_started: [None; 4],
        }));

        {
            let s = saver.borrow();
            ui.set_saver_mode(s.mode as i32);
            ui.set_saver_delay(s.delay as i32);
        }

        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_mode_picked(move |index| {
            let mut s = s.borrow_mut();
            s.mode = Mode::from_u8(index.clamp(0, 2) as u8);
            let value = s.mode as u8;
            s.save(NVS_MODE, value);
            s.last_input = Instant::now();
            if let Some(ui) = weak.upgrade() {
                ui.set_saver_mode(s.mode as i32);
            }
        });
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_delay_picked(move |index| {
            let mut s = s.borrow_mut();
            s.delay = (index.max(0) as usize).min(DELAYS.len() - 1);
            let value = s.delay as u8;
            s.save(NVS_DELAY, value);
            s.last_input = Instant::now();
            if let Some(ui) = weak.upgrade() {
                ui.set_saver_delay(s.delay as i32);
            }
        });
        saver
    }

    fn save(&mut self, key: &str, value: u8) {
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u8(key, value) {
                log::warn!("saving screensaver setting failed: {e}");
            }
        }
    }

    /// Call for every finger-down. Returns true when the touch must not
    /// reach the UI because it woke the screen.
    pub fn on_press(&mut self, ui: &AppWindow, backlight: &mut Backlight, x: f32, y: f32) -> bool {
        self.last_input = Instant::now();
        if self.active {
            log::info!("woken by a touch at {x:.0},{y:.0}");
            self.stop(ui, backlight);
            self.swallowing = true;
        }
        self.swallowing
    }

    /// Call when the finger lifts. Returns true when the release must be
    /// swallowed as well.
    pub fn on_release(&mut self) -> bool {
        self.last_input = Instant::now();
        std::mem::take(&mut self.swallowing)
    }

    /// Starts the screensaver now if it is enabled and the time is known.
    pub fn start_now(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        if self.mode != Mode::Off {
            self.start(ui, backlight);
        }
    }

    fn start(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        let Some(now) = local_time() else { return };
        log::info!("screensaver on");
        ui.set_climate_open(false);
        ui.set_dimmer_open(false);
        // Show the current time without flipping into it.
        let digits = digits_of(&now);
        for (i, d) in digits.iter().enumerate() {
            self.digits[i] = *d;
            self.prev[i] = *d;
            self.progress[i] = 1.0;
            self.flip_started[i] = None;
            self.push(ui, i);
        }
        self.shown_minute = Some(now.tm_hour * 60 + now.tm_min);
        ui.set_clock_date(date_text(&now).into());
        ui.set_saver_active(true);
        backlight.set(BRIGHTNESS_SAVER);
        self.active = true;
    }

    fn stop(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        log::info!("screensaver off");
        ui.set_saver_active(false);
        backlight.set(BRIGHTNESS_AWAKE);
        self.active = false;
    }

    /// Call from the UI loop.
    pub fn tick(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        if !self.active {
            let idle = self.last_input.elapsed() >= DELAYS[self.delay];
            if self.mode != Mode::Off
                && idle
                && !self.swallowing
                && ui.get_page() == Page::Dashboard
            {
                self.start(ui, backlight);
            }
            return;
        }
        if self.mode == Mode::Off {
            self.stop(ui, backlight);
            return;
        }
        self.animate_flips(ui);
        let Some(now) = local_time() else { return };
        let minute = now.tm_hour * 60 + now.tm_min;
        if self.shown_minute == Some(minute) {
            return;
        }
        self.shown_minute = Some(minute);
        for (i, d) in digits_of(&now).iter().enumerate() {
            self.set_digit(ui, i, *d);
        }
        ui.set_clock_date(date_text(&now).into());
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Debug aid: flips the last digit to the next value.
    pub fn test_flip(&mut self, ui: &AppWindow) {
        let next = (self.digits[3] + 1) % 10;
        self.set_digit(ui, 3, next);
    }

    /// Changes one digit and starts its flip.
    fn set_digit(&mut self, ui: &AppWindow, i: usize, digit: i32) {
        if self.digits[i] == digit {
            return;
        }
        self.prev[i] = self.digits[i];
        self.digits[i] = digit;
        self.progress[i] = 0.0;
        self.flip_started[i] = Some(Instant::now());
        self.push(ui, i);
    }

    /// Advances running flips. Each change of a progress value makes Slint
    /// redraw that card.
    fn animate_flips(&mut self, ui: &AppWindow) {
        for i in 0..4 {
            let Some(start) = self.flip_started[i] else {
                continue;
            };
            let t = start.elapsed().as_secs_f32() / FLIP_DURATION.as_secs_f32();
            if t >= 1.0 {
                self.flip_started[i] = None;
                self.progress[i] = 1.0;
            } else {
                // Ease in: the flap falls slowly at first, then snaps down.
                self.progress[i] = t * t;
            }
            self.push(ui, i);
        }
    }

    /// Sends one card's state to the UI (one property per card, see clock.slint).
    fn push(&self, ui: &AppWindow, i: usize) {
        let (d, prev, p) = (self.digits[i], self.prev[i], self.progress[i]);
        match i {
            0 => {
                ui.set_clock_d0(d);
                ui.set_clock_prev0(prev);
                ui.set_clock_p0(p);
            }
            1 => {
                ui.set_clock_d1(d);
                ui.set_clock_prev1(prev);
                ui.set_clock_p1(p);
            }
            2 => {
                ui.set_clock_d2(d);
                ui.set_clock_prev2(prev);
                ui.set_clock_p2(p);
            }
            _ => {
                ui.set_clock_d3(d);
                ui.set_clock_prev3(prev);
                ui.set_clock_p3(p);
            }
        }
    }

    /// True while the finger that woke the screen is still down.
    pub fn swallowing(&self) -> bool {
        self.swallowing
    }
}

/// Local time, or None while the clock hasn't been set by SNTP yet.
pub fn local_time() -> Option<sys::tm> {
    unsafe {
        let now = sys::time(core::ptr::null_mut());
        let mut tm: sys::tm = core::mem::zeroed();
        sys::localtime_r(&now, &mut tm);
        // Before the first sync the clock starts in 1970.
        (tm.tm_year + 1900 >= 2024).then_some(tm)
    }
}

/// Sets the C library's timezone from a POSIX TZ rule.
pub fn set_timezone(posix_tz: &str) {
    let Ok(value) = std::ffi::CString::new(posix_tz) else {
        return;
    };
    unsafe {
        sys::setenv(c"TZ".as_ptr(), value.as_ptr(), 1);
        sys::tzset();
    }
    log::info!("timezone set to {posix_tz}");
}

fn digits_of(tm: &sys::tm) -> [i32; 4] {
    [
        tm.tm_hour / 10,
        tm.tm_hour % 10,
        tm.tm_min / 10,
        tm.tm_min % 10,
    ]
}

fn date_text(tm: &sys::tm) -> String {
    let day = DAYS.get(tm.tm_wday as usize).copied().unwrap_or("");
    let month = MONTHS.get(tm.tm_mon as usize).copied().unwrap_or("");
    format!("{day}, {} {month}", tm.tm_mday)
}
