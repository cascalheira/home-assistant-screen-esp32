//! 800x480 RGB panel driven by ESP-IDF's `esp_lcd` RGB driver.
//!
//! Double buffered: Slint renders into a back buffer while the panel shows
//! the front one; [`Display::present`] hands the back buffer to the driver,
//! which switches at the end of the frame being sent, so the panel never
//! shows a half-drawn frame. Both buffers live in PSRAM. Bounce buffers in
//! internal RAM sit between them and the DMA: the CPU copies each chunk
//! through the cache, so no cache flushing is needed after rendering, and
//! the picture stays stable while Wi-Fi competes for PSRAM.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::ledc::{LedcDriver, LedcTimerDriver};
use esp_idf_svc::sys::{self, esp, EspError};
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board::*;

/// Incremented by the driver's ISR each time it starts reading a frame
/// buffer from the top, i.e. when a requested buffer switch has happened.
static FRAMES_STARTED: AtomicU32 = AtomicU32::new(0);
/// Address of the buffer on screen, for the console's screenshots.
static FRONT_BUFFER: AtomicUsize = AtomicUsize::new(0);
/// Swaps that gave up waiting for the panel (should stay 0).
static SWAP_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// Runs in the LCD ISR, so it must live in IRAM (CONFIG_LCD_RGB_ISR_IRAM_SAFE).
#[link_section = ".iram1.esp32_screen_frame_done"]
unsafe extern "C" fn on_frame_buffer_switched(
    _panel: sys::esp_lcd_panel_handle_t,
    _event: *const sys::esp_lcd_rgb_panel_event_data_t,
    _ctx: *mut c_void,
) -> bool {
    FRAMES_STARTED.fetch_add(1, Ordering::Release);
    false
}

pub struct Display {
    panel: sys::esp_lcd_panel_handle_t,
    buffers: [*mut Rgb565Pixel; 2],
    /// Index of the buffer Slint draws into next.
    back: usize,
}

// SAFETY: the handle and buffer pointers belong to the RGB panel driver,
// which is thread-safe for the calls made here (draw_bitmap). The Display is
// created on a helper thread pinned to core 0 (so the panel's ISR lands there)
// and then moved, once, to the UI task, which is its only user afterwards.
unsafe impl Send for Display {}

impl Display {
    pub fn new() -> Result<Self, EspError> {
        let mut cfg = sys::esp_lcd_rgb_panel_config_t {
            clk_src: sys::soc_periph_lcd_clk_src_t_LCD_CLK_SRC_DEFAULT,
            data_width: 16,
            bits_per_pixel: 16,
            num_fbs: 2,
            bounce_buffer_size_px: LCD_H_RES * 10,
            hsync_gpio_num: LCD_PIN_HSYNC,
            vsync_gpio_num: LCD_PIN_VSYNC,
            de_gpio_num: LCD_PIN_DE,
            pclk_gpio_num: LCD_PIN_PCLK,
            disp_gpio_num: -1,
            data_gpio_nums: LCD_PIN_DATA,
            ..Default::default()
        };
        cfg.__bindgen_anon_1.dma_burst_size = 64;
        cfg.flags.set_fb_in_psram(1);

        let t = &mut cfg.timings;
        t.pclk_hz = LCD_PCLK_HZ;
        t.h_res = LCD_H_RES as u32;
        t.v_res = LCD_V_RES as u32;
        t.hsync_pulse_width = LCD_HSYNC_PULSE;
        t.hsync_back_porch = LCD_HSYNC_BACK_PORCH;
        t.hsync_front_porch = LCD_HSYNC_FRONT_PORCH;
        t.vsync_pulse_width = LCD_VSYNC_PULSE;
        t.vsync_back_porch = LCD_VSYNC_BACK_PORCH;
        t.vsync_front_porch = LCD_VSYNC_FRONT_PORCH;
        t.flags.set_hsync_idle_low(1);
        t.flags.set_vsync_idle_low(1);
        t.flags.set_pclk_active_neg(LCD_PCLK_ACTIVE_NEG as u32);

        let callbacks = sys::esp_lcd_rgb_panel_event_callbacks_t {
            __bindgen_anon_1: sys::esp_lcd_rgb_panel_event_callbacks_t__bindgen_ty_1 {
                on_frame_buf_complete: Some(on_frame_buffer_switched),
            },
            ..Default::default()
        };

        let mut panel: sys::esp_lcd_panel_handle_t = ptr::null_mut();
        let mut fb0: *mut c_void = ptr::null_mut();
        let mut fb1: *mut c_void = ptr::null_mut();
        unsafe {
            esp!(sys::esp_lcd_new_rgb_panel(&cfg, &mut panel))?;
            esp!(sys::esp_lcd_rgb_panel_register_event_callbacks(
                panel,
                &callbacks,
                ptr::null_mut()
            ))?;
            esp!(sys::esp_lcd_panel_reset(panel))?;
            esp!(sys::esp_lcd_panel_init(panel))?;
            esp!(sys::esp_lcd_rgb_panel_get_frame_buffer(
                panel, 2, &mut fb0, &mut fb1
            ))?;
        }
        log::info!(
            "RGB panel up: {LCD_H_RES}x{LCD_V_RES} at {} MHz, double buffered",
            LCD_PCLK_HZ / 1_000_000
        );
        // The driver starts out showing buffer 0.
        FRONT_BUFFER.store(fb0 as usize, Ordering::Relaxed);
        Ok(Self {
            panel,
            buffers: [fb0.cast(), fb1.cast()],
            back: 1,
        })
    }

    /// The buffer to draw the next frame into, row-major, stride
    /// `LCD_H_RES`. It holds the frame from two `present` calls ago.
    pub fn back_buffer(&mut self) -> &mut [Rgb565Pixel] {
        // SAFETY: the driver allocated LCD_H_RES * LCD_V_RES RGB565 pixels
        // per buffer and keeps them for the program's lifetime. `present`
        // only flips `back` once the panel has stopped reading this buffer,
        // so nothing else touches it while Rust holds the slice.
        unsafe { core::slice::from_raw_parts_mut(self.buffers[self.back], LCD_H_RES * LCD_V_RES) }
    }

    /// Puts the back buffer on screen and waits until the panel has switched
    /// to it, so the old front buffer can be drawn into next.
    pub fn present(&mut self) {
        let buffer = self.buffers[self.back];
        unsafe {
            // A pointer inside one of the driver's own buffers makes it switch
            // to that buffer at the next frame instead of copying.
            sys::esp_lcd_panel_draw_bitmap(
                self.panel,
                0,
                0,
                LCD_H_RES as i32,
                LCD_V_RES as i32,
                buffer as *const c_void,
            );
        }
        // Read the counter after the request: the next increment is a frame
        // that started from the new buffer. (A frame finishing in between
        // only costs one extra frame of waiting.)
        let seen = FRAMES_STARTED.load(Ordering::Acquire);
        let deadline = Instant::now() + Duration::from_millis(100);
        while FRAMES_STARTED.load(Ordering::Acquire) == seen {
            if Instant::now() >= deadline {
                SWAP_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        FRONT_BUFFER.store(buffer as usize, Ordering::Relaxed);
        self.back ^= 1;
    }
}

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
pub fn front_buffer_addr() -> usize {
    FRONT_BUFFER.load(Ordering::Relaxed)
}

/// PWM-dimmable backlight on GPIO2.
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

    /// Brightness in percent, 0-100.
    pub fn set(&mut self, percent: u8) {
        let max = self.channel.get_max_duty();
        let duty = max * percent.min(100) as u32 / 100;
        if let Err(e) = self.channel.set_duty(duty) {
            log::warn!("backlight: {e}");
        }
    }
}
