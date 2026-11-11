//! Firmware logic with no ESP-IDF or Slint dependencies, so it can be
//! tested on a computer: see host-tests/ (`scripts/test-host.sh`).

pub mod clock;
pub mod image;
pub mod net;
pub mod screensaver;
pub mod touch;
