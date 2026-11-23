//! Backlight brightness and the optional screensaver: a clock (plain or
//! flip), the weather forecast, photos, taking turns; night mode (only a dim
//! or red clock, or the screen off); burn-in protection; waking on motion.
//!
//! The screensaver starts after a period without touches while the dashboard
//! is showing, once the time is known (SNTP, timezone from Home Assistant).
//! The touch that wakes the screen is swallowed so it doesn't also press a
//! tile. Home Assistant can change all of this (docs/protocol.md section 6),
//! and the screen reports its state back. Settings live in NVS (namespace
//! "display").

use crate::logic::i18n::{tr, trf};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys;
use slint::ComponentHandle;

use crate::board::{LCD_H_RES, LCD_V_RES};
use crate::display::Backlight;
use crate::ha::Picture;
use crate::logic::clock;
use crate::logic::image;
use crate::logic::screensaver::{self as views, View, Views};
use crate::{AppWindow, Page};

const NVS_NAMESPACE: &str = "display";
const NVS_MODE: &str = "saver";
/// Old setting: index into the settings page's options.
const NVS_DELAY_INDEX: &str = "saver_delay";
const NVS_DELAY_SECS: &str = "saver_secs";
const NVS_BRIGHTNESS: &str = "brightness";
const NVS_LANGUAGE: &str = "lang";
const NVS_CLOCK_24H: &str = "clock24";
const NVS_WEATHER: &str = "saver_wx";
const NVS_ROTATE: &str = "saver_rot";
const NVS_THEME: &str = "theme";
const NVS_SAVER_THEME: &str = "saver_theme";
/// Colour weather icons on the screensaver (1) or monochrome ones (0).
const NVS_COLOUR_ICONS: &str = "saver_icons";
/// The screen's language: 0 = Home Assistant's setting, 1 English, 2 Portuguese.
const NVS_UI_LANGUAGE: &str = "ui_lang";
const NVS_SUNRISE: &str = "sunrise";
const NVS_SUNSET: &str = "sunset";
const NVS_PHOTOS: &str = "saver_photo";
const NVS_SHIFT: &str = "saver_shift";
const NVS_NIGHT_MODE: &str = "night_mode";
const NVS_NIGHT_HOURS: &str = "night_hours";
const NVS_NIGHT_FROM: &str = "night_from";
const NVS_NIGHT_UNTIL: &str = "night_until";
const NVS_MOTION: &str = "motion";

/// Night mode: what the idle screen does at night.
const NIGHT_OFF: u8 = 0;
const NIGHT_DIM: u8 = 1;
const NIGHT_RED: u8 = 2;
const NIGHT_SCREEN_OFF: u8 = 3;
/// Backlight of the dim and red night clocks (the lowest level).
const NIGHT_LEVEL: u8 = 1;
/// Default night hours (minutes since midnight).
const NIGHT_FROM_DEFAULT: i32 = 23 * 60;
const NIGHT_UNTIL_DEFAULT: i32 = 7 * 60;

/// How long a new photo takes to fade in.
const PHOTO_FADE: Duration = Duration::from_millis(900);
/// Wait this long after a photo failed before asking for another.
const PHOTO_RETRY: Duration = Duration::from_secs(30);
/// An unanswered photo request is given up after this.
const PHOTO_TIMEOUT: Duration = Duration::from_secs(90);
/// PSRAM left for everything else when sizing photos.
const PHOTO_RESERVE: usize = 1024 * 1024;
/// With waking on motion on: the screensaver turns the screen off after this
/// long without motion.
const NO_MOTION_OFF: Duration = Duration::from_secs(10 * 60);

/// The delays offered on the settings page.
const DELAY_OPTIONS: [u32; 3] = [60, 5 * 60, 15 * 60];
/// Seconds between the clock and the forecast when both are on.
const ROTATE_OPTIONS: [u32; 4] = [10, 20, 30, 60];
const ROTATE_DEFAULT: u32 = 20;
const FLIP_DURATION: Duration = Duration::from_millis(650);
/// Backlight while the screensaver shows (never brighter than the setting).
const BRIGHTNESS_SAVER: u8 = 50;

/// The colour theme setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeMode {
    /// Light while the sun is up.
    Auto = 0,
    Light = 1,
    Dark = 2,
}

impl ThemeMode {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Auto,
            1 => Self::Light,
            _ => Self::Dark,
        }
    }
}

/// Before Home Assistant ever sent sunrise and sunset: light from
/// LIGHT_FROM to LIGHT_UNTIL.
const LIGHT_FROM: i32 = 7;
const LIGHT_UNTIL: i32 = 19;

/// Where the next screensaver photo comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PhotoSource {
    HomeAssistant,
    SdCard,
}

/// A photo the screensaver wants (see `Saver::take_photo_request`).
#[derive(Clone, Copy, Debug)]
pub struct PhotoRequest {
    pub source: PhotoSource,
    /// The size to fill (Home Assistant makes exactly this size).
    pub target: (u32, u32),
    /// Most pixels a decoded photo may have.
    pub max_pixels: u64,
}

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
    pub weather: bool,
    pub rotate_secs: u32,
    /// The language the screen shows (reported so tile texts match).
    pub portuguese: bool,
}

/// Commands from Home Assistant (docs/protocol.md section 6) and the clock
/// settings (section 7).
#[derive(Clone, Debug)]
pub enum ScreenCommand {
    Brightness(u8),
    SaverMode(Mode),
    SaverDelay(u32),
    /// Show the forecast as a screensaver.
    SaverWeather(bool),
    /// Seconds per view when the clock and the forecast take turns.
    SaverRotate(u32),
    /// Home Assistant sent a forecast with at least one day (or none).
    ForecastReady(bool),
    /// How many photos Home Assistant has for the screensaver.
    PhotosInHa(u32),
    /// Backlight while the brightness slider moves (not saved).
    PreviewBrightness(u8),
    Theme(ThemeMode),
    /// From Home Assistant's `sun.sun`: whether the sun is up, and sunrise
    /// and sunset in minutes since midnight (None: unknown).
    /// Home Assistant's "Clear PIN" button (handled by the app).
    ClearPin,
    Sun {
        up: Option<bool>,
        rise: Option<i32>,
        set: Option<i32>,
    },
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
    weather: bool,
    rotate_secs: u32,
    forecast_ready: bool,
    theme: ThemeMode,
    sun_up: Option<bool>,
    /// Last sunrise and sunset from Home Assistant (minutes since midnight),
    /// kept in NVS so the theme switches on time while HA is offline.
    sun_times: Option<(i32, i32)>,
    /// The screensaver's theme setting.
    saver_theme: ThemeMode,
    colour_icons: bool,
    /// Language setting (see NVS_UI_LANGUAGE); `portuguese` is HA's.
    ui_language: u8,
    /// The language applied to the UI (None until first applied).
    language_shown: Option<bool>,
    /// Set when the language changed; taken by the main loop.
    language_changed: Option<bool>,
    /// Dark on screen, dark and red on the screensaver (None until first
    /// applied).
    dark_shown: Option<(bool, bool, bool)>,
    /// Photos take turns with the other views.
    photos: bool,
    /// Burn-in protection: move the screensaver a little every minute.
    shift: bool,
    shift_step: u32,
    night_mode: u8,
    /// 0: sunset to sunrise, 1: `night_from` to `night_until`.
    night_hours: u8,
    night_from: i32,
    night_until: i32,
    /// It is night now and night mode is on (updated every few seconds).
    night: bool,
    /// Wake on motion (the camera; boards with one only).
    motion_wake: bool,
    has_camera: bool,
    camera_working: bool,
    last_motion: Instant,
    /// The screensaver turned the screen off: nobody moved for a while.
    nobody_around: bool,
    ha_photos: u32,
    sd_photos: usize,
    /// The next photo, ready to show.
    photo_next: Option<Picture>,
    photo_requested: Option<Instant>,
    photo_failed: Option<Instant>,
    /// A photo is on the UI (shown or kept for the next fade).
    photo_shown: bool,
    photo_since: Instant,
    fade_started: Option<Instant>,
    /// The photo size and pixel budget, worked out at the first request
    /// (None inside: not enough memory for photos).
    photo_size: Option<Option<((u32, u32), u64)>>,
    /// What auto would show, as last told to the settings page.
    theme_detail_shown: String,
    /// Last time `tick` re-checked the theme (auto changes with the clock).
    theme_checked: Instant,
    /// The view on screen while active, and since when.
    view: Option<View>,
    view_since: Instant,
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
        let weather = u8_or(NVS_WEATHER, 0) == 1;
        let theme = ThemeMode::from_u8(u8_or(NVS_THEME, ThemeMode::Dark as u8));
        let saver_theme = ThemeMode::from_u8(u8_or(NVS_SAVER_THEME, ThemeMode::Dark as u8));
        let ui_language = u8_or(NVS_UI_LANGUAGE, 0).min(2);
        let colour_icons = u8_or(NVS_COLOUR_ICONS, 0) == 1;
        let u16_of = |key| nvs.as_ref().and_then(|n| n.get_u16(key).ok().flatten());
        let sun_times = u16_of(NVS_SUNRISE)
            .zip(u16_of(NVS_SUNSET))
            .map(|(r, s)| (r as i32, s as i32))
            .filter(|&(r, s)| r < 1440 && s < 1440);
        let rotate_secs = nvs
            .as_ref()
            .and_then(|n| n.get_u32(NVS_ROTATE).ok().flatten())
            .filter(|s| ROTATE_OPTIONS.contains(s))
            .unwrap_or(ROTATE_DEFAULT);
        let minutes_or = |key, default: i32| {
            u16_of(key)
                .map(|m| m as i32)
                .filter(|m| (0..1440).contains(m))
                .unwrap_or(default)
        };
        let night_from = minutes_or(NVS_NIGHT_FROM, NIGHT_FROM_DEFAULT);
        let night_until = minutes_or(NVS_NIGHT_UNTIL, NIGHT_UNTIL_DEFAULT);
        let photos = u8_or(NVS_PHOTOS, 0) == 1;
        let shift = u8_or(NVS_SHIFT, 0) == 1;
        let night_mode = u8_or(NVS_NIGHT_MODE, NIGHT_OFF).min(NIGHT_SCREEN_OFF);
        let night_hours = u8_or(NVS_NIGHT_HOURS, 1).min(1);
        let motion_wake = u8_or(NVS_MOTION, 0) == 1;

        let saver = Rc::new(RefCell::new(Self {
            nvs,
            mode,
            delay_secs,
            brightness,
            last_brightness: brightness.max(1),
            portuguese,
            h24,
            weather,
            rotate_secs,
            forecast_ready: false,
            theme,
            saver_theme,
            ui_language,
            colour_icons,
            language_shown: None,
            language_changed: None,
            sun_up: None,
            sun_times,
            dark_shown: None,
            photos,
            shift,
            shift_step: 0,
            night_mode,
            night_hours,
            night_from,
            night_until,
            night: false,
            motion_wake,
            has_camera: false,
            camera_working: false,
            last_motion: Instant::now(),
            nobody_around: false,
            ha_photos: 0,
            sd_photos: 0,
            photo_next: None,
            photo_requested: None,
            photo_failed: None,
            photo_shown: false,
            photo_since: Instant::now(),
            fade_started: None,
            photo_size: None,
            theme_detail_shown: String::new(),
            theme_checked: Instant::now(),
            view: None,
            view_since: Instant::now(),
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
        saver.borrow_mut().apply_theme(ui);
        saver.borrow_mut().apply_language(ui);
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_colour_icons_picked(move |index| {
            let mut s = s.borrow_mut();
            s.colour_icons = index == 1;
            let value = s.colour_icons as u8;
            s.save_u8(NVS_COLOUR_ICONS, value);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
            }
        });
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_display_language_picked(move |index| {
            let mut s = s.borrow_mut();
            let language = index.clamp(0, 2) as u8;
            s.ui_language = language;
            s.save_u8(NVS_UI_LANGUAGE, language);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
                s.apply_language(&ui);
            }
        });

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
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_weather_picked(move |index| {
            let mut s = s.borrow_mut();
            s.set_weather(index == 1);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
            }
        });
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_theme_picked(move |index| {
            let mut s = s.borrow_mut();
            let theme = ThemeMode::from_u8(index.clamp(0, 2) as u8);
            s.saver_theme = theme;
            s.save_u8(NVS_SAVER_THEME, theme as u8);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
                s.apply_theme(&ui);
            }
        });
        let weak = ui.as_weak();
        let s = saver.clone();
        ui.on_saver_rotate_picked(move |index| {
            let mut s = s.borrow_mut();
            s.set_rotate(ROTATE_OPTIONS[(index.max(0) as usize).min(ROTATE_OPTIONS.len() - 1)]);
            if let Some(ui) = weak.upgrade() {
                s.show_settings(&ui);
            }
        });
        Self::register_more_callbacks(&saver, ui);
        saver
    }

    fn register_more_callbacks(saver: &Rc<RefCell<Self>>, ui: &AppWindow) {
        // Each setting: change it, save it, show the settings again.
        fn on<F: Fn(&mut Saver, i32) + 'static>(
            saver: &Rc<RefCell<Saver>>,
            ui: &AppWindow,
            register: impl FnOnce(&AppWindow, Box<dyn Fn(i32)>),
            change: F,
        ) {
            let weak = ui.as_weak();
            let s = saver.clone();
            register(
                ui,
                Box::new(move |value| {
                    let mut s = s.borrow_mut();
                    change(&mut s, value);
                    if let Some(ui) = weak.upgrade() {
                        s.update_night();
                        s.show_settings(&ui);
                        s.apply_theme(&ui);
                    }
                }),
            );
        }
        on(
            saver,
            ui,
            |ui, f| ui.on_saver_photos_picked(f),
            |s, i| {
                s.photos = i == 1;
                s.save_u8(NVS_PHOTOS, s.photos as u8);
                s.last_input = Instant::now();
                if !s.photos {
                    // Give the memory back.
                    s.photo_next = None;
                }
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_saver_shift_picked(f),
            |s, i| {
                s.shift = i == 1;
                s.save_u8(NVS_SHIFT, s.shift as u8);
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_night_mode_picked(f),
            |s, i| {
                s.night_mode = i.clamp(0, NIGHT_SCREEN_OFF as i32) as u8;
                s.save_u8(NVS_NIGHT_MODE, s.night_mode);
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_night_hours_picked(f),
            |s, i| {
                s.night_hours = i.clamp(0, 1) as u8;
                s.save_u8(NVS_NIGHT_HOURS, s.night_hours);
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_night_from_step(f),
            |s, d| {
                s.night_from = (s.night_from + d.signum() * 30).rem_euclid(1440);
                s.save_u16(NVS_NIGHT_FROM, s.night_from as u16);
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_night_until_step(f),
            |s, d| {
                s.night_until = (s.night_until + d.signum() * 30).rem_euclid(1440);
                s.save_u16(NVS_NIGHT_UNTIL, s.night_until as u16);
            },
        );
        on(
            saver,
            ui,
            |ui, f| ui.on_motion_wake_picked(f),
            |s, i| {
                s.motion_wake = i == 1;
                s.save_u8(NVS_MOTION, s.motion_wake as u8);
            },
        );
    }

    fn show_settings(&self, ui: &AppWindow) {
        ui.set_saver_mode(self.mode as i32);
        let index = DELAY_OPTIONS.iter().position(|&d| d == self.delay_secs);
        ui.set_saver_delay(index.map(|i| i as i32).unwrap_or(-1));
        ui.set_saver_weather(self.weather);
        let brightness = if self.brightness == 0 {
            self.last_brightness
        } else {
            self.brightness
        };
        ui.set_display_brightness(brightness as i32);
        ui.set_display_brightness_live(brightness as i32);
        ui.set_display_theme(self.theme as i32);
        ui.set_saver_theme(self.saver_theme as i32);
        ui.set_saver_colour_icons(self.colour_icons);
        ui.global::<crate::SaverTheme>()
            .set_colour_icons(self.colour_icons);
        ui.set_display_language(self.ui_language as i32);
        let index = ROTATE_OPTIONS.iter().position(|&s| s == self.rotate_secs);
        ui.set_saver_rotate(index.map(|i| i as i32).unwrap_or(-1));
        ui.set_saver_photos(self.photos);
        ui.set_saver_photos_detail(self.photos_detail().as_str().into());
        ui.set_photo_title(tr("No photos").into());
        ui.set_photo_hint(self.photos_detail().as_str().into());
        ui.set_saver_shift(self.shift);
        ui.set_night_mode(self.night_mode as i32);
        ui.set_night_hours(self.night_hours as i32);
        ui.set_night_from(clock::hhmm(self.night_from).as_str().into());
        ui.set_night_until(clock::hhmm(self.night_until).as_str().into());
        ui.set_night_detail(self.night_detail().as_str().into());
        ui.set_has_camera(self.has_camera);
        ui.set_motion_wake(self.motion_wake);
        ui.set_motion_detail(
            if self.camera_working {
                tr("Wakes the screen when someone moves in front of it. After 10 minutes without anyone around, the screensaver turns the screen off. The camera's pictures never leave the screen and are not stored.")
            } else {
                tr("The camera isn't working.")
            }
            .into(),
        );
    }

    /// Where the photos come from, for the settings page.
    fn photos_detail(&self) -> String {
        if self.photo_size == Some(None) {
            return tr("This screen doesn't have enough free memory for photos.").into();
        }
        if self.ha_photos > 0 {
            trf("{} photos from Home Assistant.", &[&self.ha_photos])
        } else if self.sd_photos > 0 {
            trf("{} photos from the SD card.", &[&self.sd_photos])
        } else {
            tr("No photos yet. Choose a photo folder for this screen in Home Assistant, or put JPEG photos in a folder called photos on the SD card.").into()
        }
    }

    /// When the night is and what happens then, for the settings page.
    fn night_detail(&self) -> String {
        if self.night_mode == NIGHT_OFF {
            return tr("The screensaver looks the same day and night.").into();
        }
        let (from, until) = self.night_window();
        let when = if self.night_hours == 0 && self.sun_times.is_none() {
            trf(
                "From {} to {} until Home Assistant sends sunset and sunrise.",
                &[
                    &clock::hhmm(from) as &dyn std::fmt::Display,
                    &clock::hhmm(until) as &dyn std::fmt::Display,
                ],
            )
        } else {
            trf(
                "From {} to {}.",
                &[
                    &clock::hhmm(from) as &dyn std::fmt::Display,
                    &clock::hhmm(until) as &dyn std::fmt::Display,
                ],
            )
        };
        let what = match self.night_mode {
            NIGHT_DIM => {
                tr("When nobody uses the screen, it shows only the clock, as dim as it goes.")
            }
            NIGHT_RED => tr("When nobody uses the screen, it shows only the clock, in dim red."),
            _ => tr("When nobody uses the screen, it turns off. A touch turns it on."),
        };
        format!("{when} {what}")
    }

    /// The night: (from, until) in minutes since midnight.
    fn night_window(&self) -> (i32, i32) {
        if self.night_hours == 1 {
            return (self.night_from, self.night_until);
        }
        match self.sun_times {
            Some((rise, set)) => (set, rise),
            None => (LIGHT_UNTIL * 60, LIGHT_FROM * 60),
        }
    }

    /// Re-checks whether it is night (night mode on and within its hours).
    fn update_night(&mut self) {
        let (from, until) = self.night_window();
        self.night = self.night_mode != NIGHT_OFF
            && local_time()
                .is_some_and(|t| views::is_night(t.tm_hour * 60 + t.tm_min, from, until));
    }

    fn save_u16(&mut self, key: &str, value: u16) {
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u16(key, value) {
                log::warn!("saving display setting {key} failed: {e}");
            }
        }
    }

    fn set_weather(&mut self, on: bool) {
        self.weather = on;
        self.save_u8(NVS_WEATHER, on as u8);
        self.last_input = Instant::now();
    }

    fn set_rotate(&mut self, secs: u32) {
        if !ROTATE_OPTIONS.contains(&secs) {
            log::warn!("ignoring a screensaver rotation of {secs} s");
            return;
        }
        self.rotate_secs = secs;
        if let Some(nvs) = &mut self.nvs {
            if let Err(e) = nvs.set_u32(NVS_ROTATE, secs) {
                log::warn!("saving the screensaver rotation failed: {e}");
            }
        }
    }

    /// Whether it is day for the auto themes, and what auto does, for the
    /// settings page.
    fn daylight(&self) -> (bool, String) {
        let now = local_time().map(|t| t.tm_hour * 60 + t.tm_min);
        let (sun_up, source) = match (self.sun_times, self.sun_up) {
            (Some((rise, set)), _) => (
                now.map(|now| clock::is_daytime(now, rise, set)),
                trf(
                    "Light from sunrise ({}) to sunset ({}), from Home Assistant",
                    &[
                        &clock::hhmm(rise) as &dyn std::fmt::Display,
                        &clock::hhmm(set) as &dyn std::fmt::Display,
                    ],
                ),
            ),
            (None, Some(up)) => (
                Some(up),
                tr("Light while Home Assistant's sun is up").to_string(),
            ),
            (None, None) => (
                now.map(|now| clock::is_daytime(now, LIGHT_FROM * 60, LIGHT_UNTIL * 60)),
                tr("Light from 07:00 to 19:00 until Home Assistant sends sunrise and sunset")
                    .to_string(),
            ),
        };
        let now = match sun_up {
            Some(true) => tr("Now: light."),
            Some(false) => tr("Now: dark."),
            None => tr("Dark until the time is known."),
        };
        (sun_up.unwrap_or(false), format!("{source}. {now}"))
    }

    /// Switches the theme when it should change.
    fn apply_theme(&mut self, ui: &AppWindow) {
        let (day, detail) = self.daylight();
        let dark_for = |mode| match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::Auto => !day,
        };
        let night = self.active && self.night;
        let shown = (
            dark_for(self.theme),
            night || dark_for(self.saver_theme),
            night && self.night_mode == NIGHT_RED,
        );
        if self.dark_shown != Some(shown) {
            log::info!(
                "{} theme, {} screensaver",
                if shown.0 { "dark" } else { "light" },
                if shown.2 {
                    "red"
                } else if shown.1 {
                    "dark"
                } else {
                    "light"
                }
            );
            ui.global::<crate::Theme>().set_dark(shown.0);
            ui.global::<crate::SaverTheme>().set_dark(shown.1);
            ui.global::<crate::SaverTheme>().set_red(shown.2);
            self.dark_shown = Some(shown);
        }
        if detail != self.theme_detail_shown {
            ui.set_display_theme_detail(detail.as_str().into());
            self.theme_detail_shown = detail;
        }
    }

    /// Portuguese on screen: the screen's own choice, else Home Assistant's.
    fn effective_portuguese(&self) -> bool {
        match self.ui_language {
            1 => false,
            2 => true,
            _ => self.portuguese,
        }
    }

    /// Switches the UI's language when it should change.
    fn apply_language(&mut self, ui: &AppWindow) {
        let pt = self.effective_portuguese();
        if self.language_shown == Some(pt) {
            return;
        }
        log::info!("language: {}", if pt { "Portuguese" } else { "English" });
        crate::logic::i18n::set_portuguese(pt);
        if let Err(e) = slint::select_bundled_translation(if pt { "pt" } else { "en" }) {
            log::warn!("selecting the translation failed: {e:?}");
        }
        self.language_shown = Some(pt);
        self.language_changed = Some(pt);
        self.theme_detail_shown.clear();
        self.apply_theme(ui);
        // Redraw the clock's date in the new language.
        self.shown_minute = None;
    }

    /// The new language, once after it changed (the app redoes its texts).
    pub fn take_language_changed(&mut self) -> Option<bool> {
        self.language_changed.take()
    }

    /// Some screensaver is turned on.
    fn enabled(&self) -> bool {
        self.mode != Mode::Off || self.weather || self.photos
    }

    /// The screensaver should start when idle: one is on, or it is night
    /// and night mode is on.
    fn wanted(&self) -> bool {
        self.enabled() || self.night
    }

    /// The view to show now; None = stop.
    fn wanted_view(&self) -> Option<View> {
        if self.night || self.nobody_around {
            if self.night_mode == NIGHT_SCREEN_OFF || self.nobody_around {
                return Some(View::Blank);
            }
            return Some(View::Clock);
        }
        views::view(
            Views {
                clock: self.mode != Mode::Off || self.forced,
                forecast: self.weather,
                forecast_ready: self.forecast_ready,
                photos: self.photos,
                photo_ready: self.photo_next.is_some()
                    || (self.view == Some(View::Photo) && self.photo_shown),
            },
            self.view,
            self.view_since.elapsed().as_secs(),
            self.rotate_secs as u64,
        )
    }

    fn show_view(&mut self, ui: &AppWindow, view: View) {
        if self.view != Some(view) {
            self.view = Some(view);
            self.view_since = Instant::now();
            ui.set_saver_view(match view {
                View::Clock => 0,
                View::Forecast => 1,
                View::Photo => 2,
                View::Blank => 3,
            });
            if view == View::Photo {
                self.present_photo(ui);
            }
        }
    }

    // ---- Photos

    /// Photo counts: Home Assistant's comes with `ScreenCommand::PhotosInHa`.
    pub fn set_sd_photos(&mut self, count: usize, ui: &AppWindow) {
        self.sd_photos = count;
        self.show_settings(ui);
    }

    /// The next photo to fetch, when one is wanted now.
    pub fn take_photo_request(&mut self) -> Option<PhotoRequest> {
        if !self.photos || self.photo_next.is_some() || self.fade_started.is_some() {
            return None;
        }
        if self
            .photo_requested
            .is_some_and(|at| at.elapsed() < PHOTO_TIMEOUT)
            || self
                .photo_failed
                .is_some_and(|at| at.elapsed() < PHOTO_RETRY)
        {
            return None;
        }
        let source = if self.ha_photos > 0 {
            PhotoSource::HomeAssistant
        } else if self.sd_photos > 0 {
            PhotoSource::SdCard
        } else {
            return None;
        };
        let (target, max_pixels) = (*self.photo_size.get_or_insert_with(|| {
            // Two photos are in memory while one fades into the other.
            let largest = unsafe { sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_SPIRAM) };
            let budget = (largest.saturating_sub(PHOTO_RESERVE) / 3 / 2) as u64;
            let size = image::photo_box((LCD_H_RES as u32, LCD_V_RES as u32), budget)
                .map(|size| (size, budget));
            log::info!("photo size {size:?} ({largest} B of PSRAM free)");
            size
        }))?;
        self.photo_requested = Some(Instant::now());
        Some(PhotoRequest {
            source,
            target,
            max_pixels,
        })
    }

    /// The answer to a photo request.
    pub fn photo_arrived(&mut self, photo: Result<Picture, String>, ui: &AppWindow) {
        self.photo_requested = None;
        match photo {
            Ok(picture) if self.photos => {
                self.photo_failed = None;
                self.photo_next = Some(picture);
                if self.active && self.view == Some(View::Photo) && !self.photo_shown {
                    self.present_photo(ui);
                }
            }
            Ok(_) => {}
            Err(_) => self.photo_failed = Some(Instant::now()),
        }
    }

    /// Shows the next photo, fading in over the current one.
    fn present_photo(&mut self, ui: &AppWindow) {
        let Some(picture) = self.photo_next.take() else {
            return;
        };
        let fill = image::photo_fills(
            picture.width(),
            picture.height(),
            (LCD_H_RES as u32, LCD_V_RES as u32),
        );
        if self.photo_shown {
            ui.set_saver_photo_prev(ui.get_saver_photo());
            ui.set_saver_photo_prev_fill(ui.get_saver_photo_fill());
            ui.set_saver_photo_has_prev(true);
        }
        ui.set_saver_photo(slint::Image::from_rgb8(picture));
        ui.set_saver_photo_fill(fill);
        ui.set_saver_photo_has(true);
        ui.set_saver_photo_fade(0.0);
        self.fade_started = Some(Instant::now());
        self.photo_shown = true;
        self.photo_since = Instant::now();
    }

    fn animate_fade(&mut self, ui: &AppWindow) {
        let Some(start) = self.fade_started else {
            return;
        };
        let t = start.elapsed().as_secs_f32() / PHOTO_FADE.as_secs_f32();
        if t >= 1.0 {
            self.fade_started = None;
            ui.set_saver_photo_fade(1.0);
            // The old photo's memory goes back before the next one is fetched.
            ui.set_saver_photo_has_prev(false);
            ui.set_saver_photo_prev(slint::Image::default());
        } else {
            ui.set_saver_photo_fade(t);
        }
    }

    /// Drops the photos on screen (the screensaver stopped).
    fn clear_photos(&mut self, ui: &AppWindow) {
        self.photo_shown = false;
        self.fade_started = None;
        ui.set_saver_photo_has(false);
        ui.set_saver_photo_has_prev(false);
        ui.set_saver_photo(slint::Image::default());
        ui.set_saver_photo_prev(slint::Image::default());
    }

    // ---- Motion

    /// The board has a camera (`working`: it started).
    #[cfg_attr(not(esp32p4), allow(dead_code))]
    pub fn set_camera(&mut self, working: bool, ui: &AppWindow) {
        self.has_camera = true;
        self.camera_working = working;
        self.show_settings(ui);
    }

    /// Whether the camera should look for motion now.
    #[cfg_attr(not(esp32p4), allow(dead_code))]
    pub fn motion_wanted(&self) -> bool {
        self.has_camera && self.camera_working && self.motion_wake && self.active
    }

    /// The camera saw someone move.
    #[cfg_attr(not(esp32p4), allow(dead_code))]
    pub fn on_motion(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        self.last_motion = Instant::now();
        if self.motion_wanted() {
            log::info!("woken by motion");
            self.last_input = Instant::now();
            self.stop(ui, backlight);
        }
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

    /// The backlight for the current state (0-100).
    pub fn backlight_level(&self) -> u8 {
        if !self.active {
            return self.brightness;
        }
        if self.nobody_around {
            return 0;
        }
        if self.night {
            return match self.night_mode {
                NIGHT_SCREEN_OFF => 0,
                _ => self.brightness.min(NIGHT_LEVEL),
            };
        }
        self.brightness.min(BRIGHTNESS_SAVER)
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
            weather: self.weather,
            rotate_secs: self.rotate_secs,
            portuguese: self.effective_portuguese(),
        }
    }

    pub fn command(&mut self, command: ScreenCommand, ui: &AppWindow, backlight: &mut Backlight) {
        if !matches!(command, ScreenCommand::PreviewBrightness(_)) {
            log::info!("display command: {command:?}");
        }
        match command {
            ScreenCommand::Brightness(value) => {
                self.brightness = value.min(100);
                if self.brightness > 0 {
                    self.last_brightness = self.brightness;
                }
                self.save_u8(NVS_BRIGHTNESS, self.brightness);
                self.apply_backlight(backlight);
                self.show_settings(ui);
            }
            ScreenCommand::SaverMode(mode) => {
                self.set_mode(mode);
                self.show_settings(ui);
            }
            ScreenCommand::SaverDelay(secs) => {
                self.set_delay(secs);
                self.show_settings(ui);
            }
            ScreenCommand::SaverWeather(on) => {
                self.set_weather(on);
                self.show_settings(ui);
            }
            ScreenCommand::SaverRotate(secs) => {
                self.set_rotate(secs);
                self.show_settings(ui);
            }
            ScreenCommand::ForecastReady(ready) => self.forecast_ready = ready,
            ScreenCommand::PhotosInHa(count) => {
                if count != self.ha_photos {
                    self.ha_photos = count;
                    // A different folder: start from its photos.
                    self.photo_next = None;
                    self.photo_failed = None;
                    self.show_settings(ui);
                }
            }
            ScreenCommand::ClearPin => {}
            ScreenCommand::PreviewBrightness(value) => backlight.set(value.clamp(1, 100)),
            ScreenCommand::Theme(theme) => {
                self.theme = theme;
                self.save_u8(NVS_THEME, theme as u8);
                self.show_settings(ui);
                self.apply_theme(ui);
            }
            ScreenCommand::Sun { up, rise, set } => {
                self.sun_up = up;
                if let Some(times) = rise.zip(set) {
                    if self.sun_times != Some(times) {
                        self.sun_times = Some(times);
                        if let Some(nvs) = &mut self.nvs {
                            let saved = nvs
                                .set_u16(NVS_SUNRISE, times.0 as u16)
                                .and_then(|()| nvs.set_u16(NVS_SUNSET, times.1 as u16));
                            if let Err(e) = saved {
                                log::warn!("saving sunrise/sunset failed: {e}");
                            }
                        }
                    }
                }
                self.update_night();
                self.apply_theme(ui);
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
                    self.forced = !self.enabled();
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
                    self.apply_language(ui);
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
        if self.wanted() {
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
        self.view = None;
        self.update_night();
        self.nobody_around = false;
        self.last_motion = Instant::now();
        let Some(view) = self.wanted_view() else {
            return;
        };
        self.active = true;
        self.show_view(ui, view);
        self.apply_shift(ui);
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
        ui.set_clock_time(clock::time_text(now.tm_hour, now.tm_min, self.h24).into());
        ui.set_saver_active(true);
        self.apply_theme(ui);
        self.apply_backlight(backlight);
    }

    fn stop(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        log::info!("screensaver off");
        ui.set_saver_active(false);
        self.active = false;
        self.forced = false;
        self.nobody_around = false;
        self.view = None;
        self.clear_photos(ui);
        self.apply_theme(ui);
        self.apply_backlight(backlight);
    }

    /// Moves the screensaver to the current burn-in step (or back to the
    /// middle when the protection is off).
    fn apply_shift(&self, ui: &AppWindow) {
        let (x, y) = if self.shift {
            views::shift(self.shift_step)
        } else {
            (0, 0)
        };
        ui.set_saver_shift_x(x as f32);
        ui.set_saver_shift_y(y as f32);
    }

    /// Call from the UI loop.
    pub fn tick(&mut self, ui: &AppWindow, backlight: &mut Backlight) {
        if self.theme_checked.elapsed() >= Duration::from_secs(5) {
            self.theme_checked = Instant::now();
            let was_night = self.night;
            self.update_night();
            self.apply_theme(ui);
            if self.night != was_night {
                log::info!("night {}", if self.night { "began" } else { "ended" });
                self.apply_backlight(backlight);
                self.show_settings(ui);
            }
        }
        if !self.active {
            let idle = self.last_input.elapsed() >= Duration::from_secs(self.delay_secs as u64);
            if self.wanted() && idle && !self.swallowing && ui.get_page() == Page::Dashboard {
                self.start(ui, backlight);
            }
            return;
        }
        if self.enabled() {
            // Turned on again after a forced start: no longer forced.
            self.forced = false;
        }
        if self.motion_wanted()
            && !self.nobody_around
            && self.last_motion.elapsed() >= NO_MOTION_OFF
        {
            log::info!("nobody around: screen off");
            self.nobody_around = true;
            self.apply_backlight(backlight);
        }
        let Some(view) = self.wanted_view() else {
            self.stop(ui, backlight);
            return;
        };
        self.show_view(ui, view);
        if view == View::Photo
            && self.photo_next.is_some()
            && self.photo_since.elapsed() >= Duration::from_secs(self.rotate_secs as u64)
        {
            // Photos alone: the next one.
            self.present_photo(ui);
        }
        self.animate_fade(ui);
        ui.set_saver_flip(self.mode == Mode::Flip);
        self.animate_flips(ui);
        let Some(now) = local_time() else { return };
        let minute = now.tm_hour * 60 + now.tm_min;
        if self.shown_minute == Some(minute) {
            return;
        }
        self.shown_minute = Some(minute);
        self.shift_step = self.shift_step.wrapping_add(1);
        self.apply_shift(ui);
        for (i, d) in self.digits_of(&now).iter().enumerate() {
            self.set_digit(ui, i, *d);
        }
        ui.set_clock_date(self.date_text(&now).into());
        ui.set_clock_time(clock::time_text(now.tm_hour, now.tm_min, self.h24).into());
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Time since the last touch.
    pub fn idle(&self) -> Duration {
        self.last_input.elapsed()
    }

    /// The screensaver delay (also when the screensaver is off).
    pub fn delay(&self) -> Duration {
        Duration::from_secs(self.delay_secs as u64)
    }

    /// Debug aid: flips the last digit to the next value.
    pub fn test_flip(&mut self, ui: &AppWindow) {
        let next = (self.digits[3] + 1) % 10;
        self.set_digit(ui, 3, next);
    }

    fn digits_of(&self, tm: &sys::tm) -> [i32; 4] {
        clock::digits(tm.tm_hour, tm.tm_min, self.h24)
    }

    fn date_text(&self, tm: &sys::tm) -> String {
        clock::date_line(
            clock::Date {
                weekday: tm.tm_wday,
                day: tm.tm_mday,
                month: tm.tm_mon,
                hour: tm.tm_hour,
            },
            self.effective_portuguese(),
            self.h24,
        )
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
