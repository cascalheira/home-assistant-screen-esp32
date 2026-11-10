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
        }
    }
}

/// A Home Assistant instance found on the network.
#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    pub url: String,
}
