//! The last layout and tile states, kept on the SD card so the dashboard
//! appears at boot before Home Assistant answers, and stays usable to look
//! at while it is offline.
//!
//! The file holds the raw JSON Home Assistant sent, plus the server it came
//! from: `{"server": "...", "layout": {...}, "states": {"<tile id>": {...}}}`.
//! No credentials. The layout is written when it changes; states at most
//! every few minutes, to spare the card.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

const DIR: &str = "/sdcard/esp32-screen";
const FILE: &str = "/sdcard/esp32-screen/cache.json";
const TEMP: &str = "/sdcard/esp32-screen/cache.tmp";
/// Minimum time between writes for state changes alone.
const STATES_EVERY: Duration = Duration::from_secs(300);
/// States come in a burst after subscribing: wait for it to end before the
/// first write.
const SETTLE: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Cache {
    layout: Option<Value>,
    states: Map<String, Value>,
    /// States changed since the last write.
    dirty: bool,
    changed_at: Option<Instant>,
    saved_at: Option<Instant>,
    /// The file on the card has states for the current layout.
    states_saved: bool,
}

/// What a previous run saved for `server`, if anything.
pub fn load(server: &str) -> Option<(Value, Map<String, Value>)> {
    let text = std::fs::read_to_string(FILE).ok()?;
    let mut saved: Value = serde_json::from_str(&text)
        .map_err(|e| log::warn!("ignoring the cache on the SD card: {e}"))
        .ok()?;
    if saved["server"].as_str() != Some(server) {
        return None;
    }
    let layout = saved["layout"].take();
    let states = match saved["states"].take() {
        Value::Object(states) => states,
        _ => Map::new(),
    };
    layout.is_object().then_some((layout, states))
}

impl Cache {
    /// Starts from what was loaded at boot, so the first write keeps it.
    pub fn new(loaded: Option<&(Value, Map<String, Value>)>) -> Self {
        Self {
            layout: loaded.map(|(layout, _)| layout.clone()),
            states: loaded.map(|(_, states)| states.clone()).unwrap_or_default(),
            ..Default::default()
        }
    }

    /// A layout from Home Assistant: saved right away when it differs. The
    /// states of the old layout are dropped; new ones follow.
    pub fn set_layout(&mut self, server: &str, layout: &Value) {
        if self.layout.as_ref() == Some(layout) {
            return;
        }
        self.layout = Some(layout.clone());
        self.states.clear();
        self.save(server);
    }

    pub fn merge_states(&mut self, tiles: &Map<String, Value>) {
        for (id, state) in tiles {
            if self.states.get(id) != Some(state) {
                self.states.insert(id.clone(), state.clone());
                self.dirty = true;
                self.changed_at = Some(Instant::now());
            }
        }
    }

    /// Writes pending state changes: the first ones for a layout once they
    /// settle, later ones at most every STATES_EVERY.
    pub fn tick(&mut self, server: &str) {
        if !self.dirty || self.changed_at.is_some_and(|at| at.elapsed() < SETTLE) {
            return;
        }
        if !self.states_saved || self.saved_at.is_none_or(|at| at.elapsed() >= STATES_EVERY) {
            self.save(server);
        }
    }

    /// Forgets everything (sign-out): the layout names the user's devices.
    pub fn clear(&mut self) {
        *self = Self::default();
        let _ = std::fs::remove_file(FILE);
    }

    fn save(&mut self, server: &str) {
        let Some(layout) = &self.layout else { return };
        self.dirty = false;
        self.saved_at = Some(Instant::now());
        self.states_saved = !self.states.is_empty();
        let text = json!({ "server": server, "layout": layout, "states": self.states }).to_string();
        // Write a new file and swap it in, so a reset mid-write leaves the
        // old cache intact. FAT can't rename over an existing file.
        let result = std::fs::create_dir_all(DIR)
            .and_then(|()| std::fs::write(TEMP, text.as_bytes()))
            .and_then(|()| match std::fs::remove_file(FILE) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            })
            .and_then(|()| std::fs::rename(TEMP, FILE));
        match result {
            Ok(()) => log::info!("cache saved ({} bytes)", text.len()),
            Err(e) => log::warn!("saving the cache on the SD card failed: {e}"),
        }
    }
}
