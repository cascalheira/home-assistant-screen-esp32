//! The ESP32-S3 board: an ESP32-S3 (N16R8) with an
//! 800x480 parallel RGB panel, GT911 touch and an SD slot (Sunton
//! ESP32-8048S043 class).
//!
//! These values were recovered from the factory firmware that shipped on the
//! board, not from a datasheet. GPIOs that are claimed through
//! `Peripherals` (touch, backlight, BOOT button) are wired up in `main.rs`;
//! the constants here document them and cover the pins that go straight to
//! ESP-IDF's C drivers.

// ---- Display (16-bit parallel RGB565) ---------------------------------------

pub const LCD_H_RES: usize = 800;
pub const LCD_V_RES: usize = 480;
/// Physical pixels per UI pixel: the UI is laid out for 800x480.
pub const UI_SCALE: f32 = 1.0;

/// 160 MHz / 10. The factory firmware asked for 15.5 MHz.
pub const LCD_PCLK_HZ: u32 = 16_000_000;

pub const LCD_PIN_DE: i32 = 40;
pub const LCD_PIN_VSYNC: i32 = 41;
pub const LCD_PIN_HSYNC: i32 = 39;
pub const LCD_PIN_PCLK: i32 = 42;

/// Data lines in bus order D0..D15 = B0-B4, G0-G5, R0-R4, so a
/// little-endian RGB565 pixel maps straight onto the bus.
pub const LCD_PIN_DATA: [i32; 16] = [
    8, 3, 46, 9, 1, // B0-B4
    5, 6, 7, 15, 16, 4, // G0-G5
    45, 48, 47, 21, 14, // R0-R4
];

pub const LCD_HSYNC_PULSE: u32 = 4;
pub const LCD_HSYNC_BACK_PORCH: u32 = 8;
pub const LCD_HSYNC_FRONT_PORCH: u32 = 8;
pub const LCD_VSYNC_PULSE: u32 = 4;
pub const LCD_VSYNC_BACK_PORCH: u32 = 8;
pub const LCD_VSYNC_FRONT_PORCH: u32 = 8;
pub const LCD_PCLK_ACTIVE_NEG: bool = true;

/// Backlight enable / PWM: GPIO2.
pub const _LCD_PIN_BACKLIGHT: i32 = 2;
/// The backlight is on while its pin is high.
pub const BACKLIGHT_ACTIVE_LOW: bool = false;

// ---- Touch (GT911 on I2C) ----------------------------------------------------
// GPIO19/20 are the ESP32-S3's native USB pins. This board routes them to the
// touch controller instead, so native USB must stay unused (see
// sdkconfig.defaults). SDA=19, SCL=20, INT=18, RST=38.

// ---- SD card (SPI, not used yet) --------------------------------------------
// CS=10, MOSI=11, SCK=12, MISO=13.

// ---- Buttons -----------------------------------------------------------------
// BOOT button: GPIO0, active low.

use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver, Resolution};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::FromValueType;

use super::Hardware;
use crate::display::{Backlight, Display};
use crate::touch::Touch;

/// Brings up the board's hardware.
pub fn init(p: Peripherals) -> anyhow::Result<Hardware> {
    // Backlight stays off until the first frame is on the panel.
    let timer = LedcTimerDriver::new(
        p.ledc.timer0,
        &TimerConfig::new()
            .frequency(5.kHz().into())
            .resolution(Resolution::Bits10),
    )?;
    let channel = LedcDriver::new(p.ledc.channel0, &timer, p.pins.gpio2)?;
    let mut backlight = Backlight::new(channel, timer);
    backlight.set(0);

    // The panel's interrupt (which copies every frame from PSRAM into the
    // bounce buffers) runs on the core that creates the panel: core 0, next
    // to Wi-Fi, while the UI task runs on core 1.
    let display = super::on_core0(Display::new)?;

    let i2c = I2cDriver::new(
        p.i2c0,
        p.pins.gpio19,
        p.pins.gpio20,
        &I2cConfig::new().baudrate(400.kHz().into()),
    )?;
    let touch = Touch::new(i2c, Some((p.pins.gpio18.into(), p.pins.gpio38.into())))?;
    let boot_button = PinDriver::input(p.pins.gpio0, Pull::Up)?;

    let sd_card = match crate::sd::SdCard::mount(
        p.spi2,
        p.pins.gpio12,
        p.pins.gpio11,
        p.pins.gpio13,
        p.pins.gpio10,
    ) {
        Ok(card) => Some(card),
        Err(e) => {
            log::info!("no SD card: {e:#}");
            None
        }
    };
    Ok(Hardware {
        display,
        backlight,
        touch,
        boot_button,
        sd_card,
        modem: p.modem,
    })
}
