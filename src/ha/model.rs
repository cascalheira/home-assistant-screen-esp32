//! Layout and tile state as sent by the `esp32_screen` integration.
//! See docs/protocol.md. Every field has a default so a newer integration
//! adding fields, or an older one missing some, never breaks parsing.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub pages: Vec<Page>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Page {
    pub id: String,
    pub title: String,
    /// "" or an `mdi:` name from `icons::PAGE_ICONS`.
    pub icon: String,
    pub cols: u8,
    pub rows: u8,
    pub tiles: Vec<Tile>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Tile {
    pub id: String,
    pub kind: String,
    pub entity_id: String,
    pub label: String,
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    /// Light tiles: "" (the bulb) or an `mdi:` name from `icons::LIGHT_ICONS`.
    pub icon: String,
    /// Light tiles: "toggle" (or "") turns the light on/off on a tap,
    /// "controls" opens its controls instead.
    pub tap: String,
    /// Light tiles: sliders on the tile: "", "brightness", "color" or
    /// "brightness_color".
    pub inline: String,
}

impl Tile {
    /// A tap opens the controls rather than toggling.
    pub fn tap_opens_controls(&self) -> bool {
        self.kind == "light" && self.tap == "controls"
    }

    pub fn inline_brightness(&self) -> bool {
        self.kind == "light" && self.inline.contains("brightness")
    }

    pub fn inline_color(&self) -> bool {
        self.kind == "light" && self.inline.contains("color")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct TileState {
    pub available: bool,
    pub on: bool,
    pub status: String,
    pub detail: String,
    pub moving: bool,
    pub brightness: i32,
    // f64, not f32: deserializing f32 here trips an instruction-selection bug
    // in the Xtensa LLVM backend ("Cannot select ... PCREL_WRAPPER").
    pub current: Option<f64>,
    pub target: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// Climate humidity in whole percent.
    pub humidity: Option<f64>,
    /// Cover/valve position 0-100, -1 when unknown.
    pub position: i32,
    /// Media player volume 0-100, -1 when not settable.
    pub volume: i32,
    /// Lock/alarm takes a code.
    pub code_required: bool,
    pub mode: String,
    pub modes: Vec<String>,
    pub unit: String,
    /// Light hue 0-359; -1 when it takes no colour.
    pub hue: i32,
    /// Light colour while on, "#rrggbb"; "" when unknown.
    pub rgb: String,
    /// Light colour temperature and its range; -1 when it has none.
    pub kelvin: i32,
    pub kelvin_min: i32,
    pub kelvin_max: i32,
}

impl TileState {
    /// The light has something to adjust beyond on/off.
    pub fn has_light_controls(&self) -> bool {
        self.brightness >= 0 || self.hue >= 0 || self.kelvin_range().is_some()
    }

    pub fn kelvin_range(&self) -> Option<(i32, i32)> {
        (self.kelvin_min > 0 && self.kelvin_max > self.kelvin_min)
            .then_some((self.kelvin_min, self.kelvin_max))
    }

    /// `rgb` as red, green, blue.
    pub fn color(&self) -> Option<(u8, u8, u8)> {
        let hex = self.rgb.strip_prefix('#').filter(|h| h.len() == 6)?;
        let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        Some((channel(0)?, channel(2)?, channel(4)?))
    }
}

impl Default for TileState {
    fn default() -> Self {
        Self {
            available: false,
            on: false,
            status: String::new(),
            detail: String::new(),
            moving: false,
            brightness: -1,
            current: None,
            target: None,
            min: None,
            max: None,
            step: None,
            humidity: None,
            position: -1,
            volume: -1,
            code_required: false,
            mode: String::new(),
            modes: Vec::new(),
            unit: String::new(),
            hue: -1,
            rgb: String::new(),
            kelvin: -1,
            kelvin_min: -1,
            kelvin_max: -1,
        }
    }
}

/// A Home Assistant instance found on the network.
#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    pub url: String,
}

/// The `forecast` event (docs/protocol.md section 7).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Forecast {
    pub unit: String,
    pub days: Vec<ForecastDay>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ForecastDay {
    /// "2026-11-12", in Home Assistant's time zone.
    pub date: String,
    /// 0 = Sunday.
    pub weekday: i32,
    pub condition: String,
    pub high: Option<i64>,
    pub low: Option<i64>,
    /// Chance of precipitation, percent.
    pub precipitation: Option<i64>,
}
