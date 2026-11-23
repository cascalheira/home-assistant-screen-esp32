//! Firmware logic with no ESP-IDF or Slint dependencies, so it can be
//! tested on a computer: see host-tests/ (`scripts/test-host.sh`).

pub mod backlight;
pub mod climate;
pub mod clock;
pub mod i18n;
pub mod image;
// The camera's motion detection (ESP32-P4 board only).
#[cfg_attr(not(any(esp32p4, test)), allow(dead_code))]
pub mod motion;
pub mod net;
pub mod pin;
pub mod screensaver;
pub mod touch;
