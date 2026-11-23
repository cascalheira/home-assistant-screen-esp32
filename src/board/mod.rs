//! The hardware, per board: pins, panel size and bringing it all up.
//!
//! The board is chosen at build time by the chip (`BOARD=s3|p4` in
//! scripts/cargo-esp.sh):
//! - `s3`: ESP32-S3, 800x480 parallel RGB panel, GT911 touch, SD over SPI.
//! - `p4`: ESP32-P4, 1024x600 MIPI-DSI panel (JD9165), GT911 touch, Wi-Fi
//!   through an ESP32-C6 over SDIO.

use esp_idf_svc::hal::gpio::{Input, PinDriver};
use esp_idf_svc::hal::modem::Modem;

use crate::display::{Backlight, Display};
use crate::touch::Touch;

#[cfg(esp32p4)]
mod p4;
#[cfg(esp32s3)]
mod s3;
#[cfg(esp32p4)]
pub use p4::*;
#[cfg(esp32s3)]
pub use s3::*;

/// The board's hardware, ready to use.
pub struct Hardware {
    pub display: Display,
    pub backlight: Backlight,
    pub touch: Touch,
    pub boot_button: PinDriver<'static, Input>,
    pub sd_card: Option<crate::sd::SdCard>,
    pub modem: Modem<'static>,
}

/// Runs `f` on a helper thread pinned to core 0, so interrupts it sets up
/// (the panel's) land there, away from the UI task on core 1.
fn on_core0<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, esp_idf_svc::sys::EspError> + Send + 'static,
) -> anyhow::Result<T> {
    use esp_idf_svc::hal::cpu::Core;
    use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
    ThreadSpawnConfiguration {
        name: Some(c"lcd-init"),
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    let created = std::thread::spawn(f).join();
    ThreadSpawnConfiguration::default().set()?;
    Ok(created.map_err(|_| anyhow::anyhow!("display init thread panicked"))??)
}
