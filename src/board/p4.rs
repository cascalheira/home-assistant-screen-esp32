//! The ESP32-P4 board, Guition JC1060P470C: an ESP32-P4 (32 MB PSRAM, 16 MB flash) with a
//! 1024x600 MIPI-DSI panel (JD9165), GT911 touch, an ESP32-C6 for Wi-Fi over
//! SDIO, an ES8311 audio codec, a camera connector, Ethernet and an SD slot.
//!
//! Pins come from the factory firmware's boot log (an Espressif ESP-Brookesia
//! demo built on the ESP32-P4 Function EV board support package): backlight
//! on GPIO23, Wi-Fi SDIO on GPIO14-19 with the C6's reset on GPIO54 (set in
//! sdkconfig.defaults.esp32p4). Touch is on I2C; its pins are found by
//! probing at start-up (see `find_touch`).

// ---- Display (MIPI-DSI, 2 lanes, RGB565) --------------------------------------

pub const LCD_H_RES: usize = 1024;
pub const LCD_V_RES: usize = 600;
/// Physical pixels per UI pixel: the UI is laid out for 800x480, so it is
/// drawn 1.25 times larger (819x480 UI pixels).
pub const UI_SCALE: f32 = 1.25;

// Bus, timings, reset pin and init commands as in Guition's board support
// package for this board (JC1060P470C). The panel needs its own init
// sequence; the generic one in Espressif's esp_lcd_jd9165 leaves it black.
pub const DSI_LANES: u8 = 2;
pub const DSI_LANE_MBPS: f32 = 750.0;
/// As Guition's code sets it. The DPI clock comes from a PLL by an integer
/// divider; ESP-IDF 5.5.6 (unlike 5.5.3) adjusts the line timing to the
/// clock it actually gets.
pub const DPI_CLOCK_MHZ: f32 = 52.0;
pub const LCD_HSYNC_PULSE: u32 = 24;
pub const LCD_HSYNC_BACK_PORCH: u32 = 136;
pub const LCD_HSYNC_FRONT_PORCH: u32 = 160;
pub const LCD_VSYNC_PULSE: u32 = 2;
pub const LCD_VSYNC_BACK_PORCH: u32 = 21;
pub const LCD_VSYNC_FRONT_PORCH: u32 = 12;
/// The MIPI-DSI PHY's supply: on-chip LDO channel 3 at 2.5 V.
pub const DSI_PHY_LDO_CHANNEL: i32 = 3;
pub const DSI_PHY_LDO_MV: i32 = 2500;
/// Panel reset (active low).
pub const LCD_PIN_RESET: i32 = 0;
/// The backlight is on while its pin is high (20 kHz PWM, as the factory
/// firmware drives it).
pub const BACKLIGHT_ACTIVE_LOW: bool = false;

/// The panel's init sequence: (command, parameters, delay in ms after it),
/// from Guition's JC1060P470C board support package (P4 v3 demo, V3.8).
pub const LCD_INIT_COMMANDS: &[(u8, &[u8], u32)] = &[
    (0x30, &[0x00], 0),
    (0xF7, &[0x49, 0x61, 0x02, 0x00], 0),
    (0x30, &[0x01], 0),
    (0x04, &[0x0C], 0),
    (0x05, &[0x08], 0),
    (0x0B, &[0x11], 0),
    (0x20, &[0x04], 0),
    (0x1F, &[0x05], 0),
    (0x23, &[0x38], 0),
    (0x28, &[0x18], 0),
    (0x29, &[0x29], 0),
    (0x2A, &[0x01], 0),
    (0x2B, &[0x29], 0),
    (0x2C, &[0x01], 0),
    (0x30, &[0x02], 0),
    (0x00, &[0x05], 0),
    (0x01, &[0x22], 0),
    (0x02, &[0x08], 0),
    (0x03, &[0x12], 0),
    (0x04, &[0x16], 0),
    (0x05, &[0x64], 0),
    (0x06, &[0x00], 0),
    (0x07, &[0x00], 0),
    (0x08, &[0x78], 0),
    (0x09, &[0x00], 0),
    (0x0A, &[0x04], 0),
    (
        0x0B,
        &[
            0x16, 0x17, 0x0B, 0x0D, 0x0D, 0x0D, 0x11, 0x10, 0x07, 0x07, 0x09,
        ],
        0,
    ),
    (
        0x0C,
        &[
            0x09, 0x1E, 0x1E, 0x1C, 0x1C, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D,
        ],
        0,
    ),
    (
        0x0D,
        &[
            0x0A, 0x05, 0x0B, 0x0D, 0x0D, 0x0D, 0x11, 0x10, 0x06, 0x06, 0x08,
        ],
        0,
    ),
    (
        0x0E,
        &[
            0x08, 0x1F, 0x1F, 0x1D, 0x1D, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D,
        ],
        0,
    ),
    (
        0x0F,
        &[
            0x0A, 0x05, 0x0D, 0x0B, 0x0D, 0x0D, 0x11, 0x10, 0x1D, 0x1D, 0x1F,
        ],
        0,
    ),
    (
        0x10,
        &[
            0x1F, 0x08, 0x08, 0x06, 0x06, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D,
        ],
        0,
    ),
    (
        0x11,
        &[
            0x16, 0x17, 0x0D, 0x0B, 0x0D, 0x0D, 0x11, 0x10, 0x1C, 0x1C, 0x1E,
        ],
        0,
    ),
    (
        0x12,
        &[
            0x1E, 0x09, 0x09, 0x07, 0x07, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D, 0x0D,
        ],
        0,
    ),
    (0x13, &[0x00, 0x00, 0x00, 0x00], 0),
    (0x14, &[0x00, 0x00, 0x41, 0x41], 0),
    (0x15, &[0x00, 0x00, 0x00, 0x00], 0),
    (0x17, &[0x00], 0),
    (0x18, &[0x85], 0),
    (0x19, &[0x06, 0x09], 0),
    (0x1A, &[0x05, 0x08], 0),
    (0x1B, &[0x0A, 0x04], 0),
    (0x26, &[0x00], 0),
    (0x27, &[0x00], 0),
    (0x30, &[0x06], 0),
    (
        0x12,
        &[
            0x3F, 0x25, 0x27, 0x35, 0x1D, 0x1B, 0x1B, 0x1A, 0x18, 0x0A, 0x2A, 0x21, 0x19, 0x30,
        ],
        0,
    ),
    (
        0x13,
        &[
            0x3F, 0x26, 0x27, 0x35, 0x1E, 0x1C, 0x1C, 0x1A, 0x18, 0x0B, 0x2A, 0x21, 0x19, 0x30,
        ],
        0,
    ),
    (0x30, &[0x0A], 0),
    (0x02, &[0x4F], 0),
    (0x0B, &[0x40], 0),
    (0x30, &[0x0D], 0),
    (0x0D, &[0x04], 0),
    (0x10, &[0x05], 0),
    (0x11, &[0x0C], 0),
    (0x12, &[0x05], 0),
    (0x13, &[0x0C], 0),
    (0x30, &[0x00], 0),
    (0x3A, &[0x55], 0),
    (0x11, &[0x00], 120),
    (0x29, &[0x00], 20),
];

// ---- Touch (GT911 on I2C) ----------------------------------------------------
// The I2C pins are not in the factory log. These pairs are tried in order;
// the first is the ESP32-P4 Function EV board's.
const TOUCH_I2C_CANDIDATES: [(i32, i32); 4] = [(7, 8), (8, 7), (31, 32), (32, 31)];

// ---- Buttons -------------------------------------------------------------------
// BOOT button: GPIO35, active low.

use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver, I2C0};
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
            .frequency(20.kHz().into())
            .resolution(Resolution::Bits10),
    )?;
    let channel = LedcDriver::new(p.ledc.channel0, &timer, p.pins.gpio23)?;
    let mut backlight = Backlight::new(channel, timer);
    backlight.set(0);

    let display = super::on_core0(Display::new)?;
    let touch = find_touch()?;
    let boot_button = PinDriver::input(p.pins.gpio35, Pull::Up)?;
    let sd_card = match crate::sd::SdCard::mount_sdmmc() {
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

/// Tries the candidate I2C pin pairs until a GT911 answers, logging what
/// each bus holds (the audio codec at 0x18 also tells the bus apart).
fn find_touch() -> anyhow::Result<Touch> {
    for (sda, scl) in TOUCH_I2C_CANDIDATES {
        // SAFETY: I2C0 and these pins are used by nothing else; each try's
        // driver is dropped (releasing them) before the next.
        let i2c = unsafe {
            I2cDriver::new(
                I2C0::steal(),
                AnyIOPin::steal(sda as u8),
                AnyIOPin::steal(scl as u8),
                &I2cConfig::new().baudrate(400.kHz().into()),
            )
        };
        let Ok(mut i2c) = i2c else { continue };
        let found: Vec<u8> = (0x08u8..0x78)
            .filter(|&addr| i2c.write(addr, &[], 5).is_ok())
            .collect();
        log::info!("I2C SDA={sda} SCL={scl}: devices {found:02X?}");
        if found.contains(&0x5D) || found.contains(&0x14) {
            log::info!("touch controller on SDA={sda} SCL={scl}");
            return Ok(Touch::new(i2c, None)?);
        }
    }
    log::error!("no GT911 found on the candidate I2C pins");
    // A touch-less screen still shows the dashboard.
    let i2c = unsafe {
        I2cDriver::new(
            I2C0::steal(),
            AnyIOPin::steal(TOUCH_I2C_CANDIDATES[0].0 as u8),
            AnyIOPin::steal(TOUCH_I2C_CANDIDATES[0].1 as u8),
            &I2cConfig::new().baudrate(400.kHz().into()),
        )?
    };
    Ok(Touch::new(i2c, None)?)
}
