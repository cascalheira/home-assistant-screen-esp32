//! Backlight brightness and the optional screensaver clock (plain or flip).
//!
//! The screensaver starts after a period without touches while the dashboard
//! is showing, once the time is known (SNTP, timezone from Home Assistant).
//! The touch that wakes the screen is swallowed so it doesn't also press a
//! tile. Home Assistant can change all of this (docs/protocol.md section 6),
//! and the screen reports its state back. Settings live in NVS (namespace
//! "display").

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
/// Old setting: index into the settings page's options.
const NVS_DELAY_INDEX: &str = "saver_delay";
const NVS_DELAY_SECS: &str = "saver_secs";
const NVS_BRIGHTNESS: &str = "brightness";
const NVS_LANGUAGE: &str = "lang";
const NVS_CLOCK_24H: &str = "clock24";

/// The delays offered on the settings page.
const DELAY_OPTIONS: [u32; 3] = [60, 5 * 60, 15 * 60];
const FLIP_DURATION: Duration = Duration::from_millis(650);
/// Backlight while the screensaver shows (never brighter than the setting).
const BRIGHTNESS_SAVER: u8 = 35;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Mode {
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

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "clock" => Some(Self::Clock),
            "flip" => Some(Self::Flip),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Clock => "clock",
            Self::Flip => "flip",
        }
    }
}

/// What the screen reports to Home Assistant (`esp32_screen/state`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScreenState {
    pub brightness: u8,
    pub mode: Mode,
    pub delay_secs: u32,
    pub active: bool,
}

/// Commands from Home Assistant (docs/protocol.md section 6) and the clock
/// settings (section 7).
#[derive(Clone, Debug)]
pub enum ScreenCommand {
    Brightness(u8),
    SaverMode(Mode),
    SaverDelay(u32),
    Wake,
    /// Wake for a notification: like `Wake`, and a backlight turned off
    /// from Home Assistant comes back on.
    Attention,
    Sleep,
    ClockFormat {
        portuguese: bool,
        h24: bool,
    },
}

pub struct Saver {
    nvs: Option<EspNvs<NvsDefault>>,
    mode: Mode,
    delay_secs: u32,
    /// Backlight 0-100 while awake; 0 = off.
    brightness: u8,
    /// Last brightness above 0, restored by a touch on a dark screen.
    last_brightness: u8,
    portuguese: bool,
    h24: bool,
    active: bool,
    /// The screensaver was started on request (`sleep`) while it is off in
    /// the settings: show the plain clock until woken.
    forced: bool,
    /// Set while the finger that woke the screen is still down.
    swallowing: bool,
    last_input: Instant,
    /// Hours and minutes shown, as `hour * 60 + minute`.
    shown_minute: Option<i32>,
    /// Digit per card; -1 = blank (leading zero of a 12-hour clock).
    digits: [i32; 4],
    prev: [i32; 4],
    /// Flip progress per digit (0-1), driven from `tick`.
    progress: [f32; 4],
    flip_started: [Option<Instant>; 4],
}

impl Saver {
    pub fn new(nvs_partition: EspDefaultNvsPartition, ui: &AppWindow) -> Rc<RefCell<Self>> {
        let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)
            .map_err(|e| log::warn!("display settings unavailable: {e}"))
            .ok();
        let u8_or = |key, default| {
            nvs.as_ref()
                .and_then(|n| n.get_u8(key).ok().flatten())
                .unwrap_or(default)
        };
        let mode = Mode::from_u8(u8_or(NVS_MODE, 0));
        let delay_secs = nvs
            .as_ref()
            .and_then(|n| n.get_u32(NVS_DELAY_SECS).ok().flatten())
            .unwrap_or_else(|| DELAY_OPTIONS[(u8_or(NVS_DELAY_INDEX, 0) as usize).min(2)]);
        let brightness = u8_or(NVS_BRIGHTNESS, 100).min(100);
        let portuguese = u8_or(NVS_LANGUAGE, 0) == 1;
        let h24 = u8_or(NVS_CLOCK_24H, 1) == 1;

        let saver = Rc::new(RefCell::new(Self {
            nvs,
            mode,
            delay_secs,
            brightness,
            last_brightness: brightness.max(1),
            portuguese,
            h24,
            active: false,
            forced: false,
            swallowing: false,
            last_input: Instant::now(),
            shown_minute: None,
            digits: [0; 4],
            prev: [0; 4],
            progress: [1.0; 4],
            flip_started: [None; 4],
        }));
        saver.borrow().show_settings(ui);

        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_mode_picked(move |index| {
            let mut s = s.borrow_mut();
            s.set_mode(Mode::from_u8(index.clamp(0, 2) as u8));
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
            }
        });
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_delay_picked(move |index| {
            let mut s = s.borrow_mut();
            s.set_delay(DELAY_OPTIONS[(index.max(0) as usize).min(DELAY_OPTIONS.len() - 1)]);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
            }
        });
        saver
    }

    fn show_settings(&self, ui: &AppWindow) {
        ui.set_saver_mode(self.mode as i32);
        let index = DELAY_OPTIONS.iter().position(|&d| d == self.delay_secs);
        ui.set_saver_delay(index.map(|i| i as i32).unwrap_or(-1));
    }

    fn save_u8(&mut self, key: &str, value: u8) {
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u8(key, value) {
                log::warn!("saving display setting {key} failed: {e}");
            }
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.save_u8(NVS_MODE, mode as u8);
        self.last_input = Instant::now();
    }

    fn set_delay(&mut self, secs: u32) {
        self.delay_secs = secs.clamp(10, 86_400);
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u32(NVS_DELAY_SECS, self.delay_secs) {
                log::warn!("saving the screensaver delay failed: {e}");
            }
        }
        self.last_input = Instant::now();
    }

    fn backlight_level(&self) -> u8 {
        if self.active {
            self.brightness.min(BRIGHTNESS_SAVER)
        } else {
            self.brightness
        }
    }

    /// Backlight level for the current state (used once the first frame is up).
    pub fn apply_backlight(&self, backlight: &mut Backlight) {
        backlight.set(self.backlight_level());
    }

    pub fn state(&self) -> ScreenState {
        ScreenState {
            brightness: self.brightness,
            mode: self.mode,
            delay_secs: self.delay_secs,
            active: self.active,
        }
    }

    pub fn command(&mut self, command: ScreenCommand, ui: &AppWindow, backlight: &mut Backlight) {
        log::info!("display command: {command:?}");
        match command {
            ScreenCommand::Brightness(value) => {
                self.brightness = value.min(100);
                if self.brightness > 0 {
                    self.last_brightness = self.brightness;
                }
                self.save_u8(NVS_BRIGHTNESS, self.brightness);
                self.apply_backlight(backlight);
            }
            ScreenCommand::SaverMode(mode) => {
                self.set_mode(mode);
                self.show_settings(ui);
            }
            ScreenCommand::SaverDelay(secs) => {
                self.set_delay(secs);
                self.show_settings(ui);
            }
            ScreenCommand::Wake => {
                self.last_input = Instant::now();
                if self.active {
                    self.stop(ui, backlight);
                }
            }
            ScreenCommand::Attention => {
                self.last_input = Instant::now();
                if self.brightness == 0 {
                    self.brightness = self.last_brightness.max(1);
                    self.save_u8(NVS_BRIGHTNESS, self.brightness);
                }
                if self.active {
                    self.stop(ui, backlight);
                }
                self.apply_backlight(backlight);
            }
            ScreenCommand::Sleep => {
                if !self.active {
                    self.forced = self.mode == Mode::Off;
                    self.start(ui, backlight);
                }
            }
            ScreenCommand::ClockFormat { portuguese, h24 } => {
                if (portuguese, h24) != (self.portuguese, self.h24) {
                    self.portuguese = portuguese;
                    self.h24 = h24;
                    self.save_u8(NVS_LANGUAGE, portuguese as u8);
                    self.save_u8(NVS_CLOCK_24H, h24 as u8);
                    if self.active {
                        // Redraw the clock in the new format.
                        self.shown_minute = None;
                    }
                }
            }
        }
    }

    /// Call for every finger-down. Returns true when the touch must not
    /// reach the UI because it woke the screen.
    pub fn on_press(&mut self, ui: &AppWindow, backlight: &mut Backlight, x: f32, y: f32) -> bool {
        self.last_input = Instant::now();
        if self.brightness == 0 {
            // The backlight was turned off from Home Assistant: a touch turns
            // it back on instead of pressing something unseen.
            log::info!("backlight turned on by a touch at {x:.0},{y:.0}");
            self.brightness = self.last_brightness.max(1);
            self.save_u8(NVS_BRIGHTNESS, self.brightness);
            self.swallowing = true;
        }
        if self.active {
            log::info!("woken by a touch at {x:.0},{y:.0}");
            self.stop(ui, backlight);
            self.swallowing = true;
        }
        self.apply_backlight(backlight);
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
        ui.set_sheet_open(false);
        ui.set_saver_flip(self.mode == Mode::Flip);
        // Show the current time without flipping into it.
        let digits = self.digits_of(&now);
        for (i, d) in digits.iter().enumerate() {
            self.digits[i] = *d;
            self.prev[i] = *d;
            self.progress[i] = 1.0;
            self.flip_started[i] = None;
            self.push(ui, i);
        }
        self.shown_minute = Some(now.tm_hour * 60 + now.tm_min);
        ui.set_clock_date(self.date_text(&now).into());
        ui.set_saver_active(true);
        self.active = true;
        self.apply_backlight(backlight);
    }

    fn stop(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        log::info!("screensaver off");
        ui.set_saver_active(false);
        self.active = false;
        self.forced = false;
        self.apply_backlight(backlight);
    }

    /// Call from the UI loop.
    pub fn tick(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        if !self.active {
            let idle = self.last_input.elapsed() >= Duration::from_secs(self.delay_secs as u64);
            if self.mode != Mode::Off
                && idle
                && !self.swallowing
                && ui.get_page() == Page::Dashboard
            {
                self.start(ui, backlight);
            }
            return;
        }
        if self.mode == Mode::Off && !self.forced {
            self.stop(ui, backlight);
            return;
        }
        ui.set_saver_flip(self.mode == Mode::Flip);
        self.animate_flips(ui);
        let Some(now) = local_time() else { return };
        let minute = now.tm_hour * 60 + now.tm_min;
        if self.shown_minute == Some(minute) {
            return;
        }
        self.shown_minute = Some(minute);
        for (i, d) in self.digits_of(&now).iter().enumerate() {
            self.set_digit(ui, i, *d);
        }
        ui.set_clock_date(self.date_text(&now).into());
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Debug aid: flips the last digit to the next value.
    pub fn test_flip(&mut self, ui: &AppWindow) {
        let next = (self.digits[3] + 1) % 10;
        self.set_digit(ui, 3, next);
    }

    fn digits_of(&self, tm: &sys::tm) -> [i32; 4] {
        let hour = if self.h24 {
            tm.tm_hour
        } else {
            match tm.tm_hour % 12 {
                0 => 12,
                h => h,
            }
        };
        let first = if !self.h24 && hour < 10 {
            -1
        } else {
            hour / 10
        };
        [first, hour % 10, tm.tm_min / 10, tm.tm_min % 10]
    }

    fn date_text(&self, tm: &sys::tm) -> String {
        let date = date_text(tm, self.portuguese);
        if self.h24 {
            date
        } else {
            format!("{date} · {}", if tm.tm_hour < 12 { "AM" } else { "PM" })
        }
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

const DAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS_EN: [&str; 12] = [
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
const DAYS_PT: [&str; 7] = [
    "Domingo",
    "Segunda-feira",
    "Terça-feira",
    "Quarta-feira",
    "Quinta-feira",
    "Sexta-feira",
    "Sábado",
];
const MONTHS_PT: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];

/// "Tuesday, 10 November" / "Terça-feira, 10 de novembro".
fn date_text(tm: &sys::tm, portuguese: bool) -> String {
    let (days, months) = if portuguese {
        (DAYS_PT, MONTHS_PT)
    } else {
        (DAYS_EN, MONTHS_EN)
    };
    let day = days.get(tm.tm_wday as usize).copied().unwrap_or("");
    let month = months.get(tm.tm_mon as usize).copied().unwrap_or("");
    if portuguese {
        format!("{day}, {} de {month}", tm.tm_mday)
    } else {
        format!("{day}, {} {month}", tm.tm_mday)
    }
}
