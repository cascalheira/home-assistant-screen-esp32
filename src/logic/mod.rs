//! Firmware logic with no ESP-IDF or Slint dependencies, so it can be
//! tested on a computer: see host-tests/ (`scripts/test-host.sh`).

pub mod backlight;
pub mod climate;
pub mod clock;
pub mod i18n;
pub mod image;
pub mod net;
pub mod pin;
pub mod screensaver;
pub mod touch;
