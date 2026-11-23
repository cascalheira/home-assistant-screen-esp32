//! The panel: a double-buffered frame buffer the UI renders into, and the
//! backlight. `rgb` drives the ESP32-S3 board's parallel RGB panel, `dsi`
//! the ESP32-P4 board's MIPI-DSI panel; both have the same interface.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use esp_idf_svc::hal::ledc::{LedcDriver, LedcTimerDriver};
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board::LCD_H_RES;

#[cfg(esp32p4)]
mod dsi;
#[cfg(esp32s3)]
mod rgb;
#[cfg(esp32p4)]
pub use dsi::Display;
#[cfg(esp32s3)]
pub use rgb::Display;

/// Incremented by the driver's interrupt each time it starts reading a frame
/// buffer from the top, i.e. when a requested buffer switch has happened.
static FRAMES_STARTED: AtomicU32 = AtomicU32::new(0);
/// Address of the buffer on screen, for the console's screenshots.
static FRONT_BUFFER: AtomicUsize = AtomicUsize::new(0);
/// Swaps that gave up waiting for the panel (should stay 0).
static SWAP_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// (frames started, swap timeouts), for the debug console.
pub fn frame_stats() -> (u32, u32) {
    (
        FRAMES_STARTED.load(Ordering::Relaxed),
        SWAP_TIMEOUTS.load(Ordering::Relaxed),
    )
}

/// Feeds Slint's line-by-line renderer: each line is drawn into a small
/// buffer in internal RAM and then copied to the back buffer in PSRAM.
/// Blending directly in PSRAM is several times slower.
pub struct LineWriter<'a> {
    pub target: &'a mut [Rgb565Pixel],
    pub line: &'a mut [Rgb565Pixel],
}

impl slint::platform::software_renderer::LineBufferProvider for LineWriter<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Rgb565Pixel]),
    ) {
        let start = line * LCD_H_RES;
        // No need to read the old pixels first: the line renderer fills the
        // range with the window background before drawing.
        render_fn(&mut self.line[range.clone()]);
        self.target[start + range.start..start + range.end].copy_from_slice(&self.line[range]);
    }
}

/// Address of the buffer currently on screen (for screenshots).
#[cfg_attr(not(feature = "dev-console"), allow(dead_code))]
pub fn front_buffer_addr() -> usize {
    FRONT_BUFFER.load(Ordering::Relaxed)
}

/// PWM-dimmable backlight (LEDC).
pub struct Backlight {
    channel: LedcDriver<'static>,
    // Kept alive for the channel's sake.
    _timer: LedcTimerDriver<'static, esp_idf_svc::hal::ledc::LowSpeed>,
}

impl Backlight {
    pub fn new(
        channel: LedcDriver<'static>,
        timer: LedcTimerDriver<'static, esp_idf_svc::hal::ledc::LowSpeed>,
    ) -> Self {
        Self {
            channel,
            _timer: timer,
        }
    }

    /// Brightness 0-100 as people pick it (see `logic::backlight`).
    pub fn set(&mut self, percent: u8) {
        let max = self.channel.get_max_duty();
        let mut duty = max * crate::logic::backlight::duty_permille(percent) / 1000;
        if crate::board::BACKLIGHT_ACTIVE_LOW {
            duty = max - duty;
        }
        if let Err(e) = self.channel.set_duty(duty) {
            log::warn!("backlight: {e}");
        }
    }
}
