//! ESP32-P4 board: 1024x600 MIPI-DSI panel (JD9165) through ESP-IDF's DPI
//! driver.
//!
//! Double buffered like the S3 board's RGB panel: the driver allocates two
//! frame buffers in PSRAM and streams one to the panel; drawing a buffer
//! that is one of its own makes it write back the cache and switch to it at
//! the next frame (no copy). Slint renders into the other one.
//!
//! The JD9165 driver's types come from its own bindings module
//! (`sys::board_p4`), so the panel is set up through that module; the
//! generic panel calls (reset, init, draw) are in the main bindings.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use esp_idf_svc::sys::board_p4 as b;
use esp_idf_svc::sys::{self, esp, EspError};
use slint::platform::software_renderer::Rgb565Pixel;

use super::{FRAMES_STARTED, FRONT_BUFFER, SWAP_TIMEOUTS};
use crate::board::*;

/// The panel has moved on to the buffer most recently drawn: the other one
/// may be drawn into.
unsafe extern "C" fn on_frame_buffer_switched(
    _panel: b::esp_lcd_panel_handle_t,
    _event: *mut b::esp_lcd_dpi_panel_event_data_t,
    _ctx: *mut c_void,
) -> bool {
    FRAMES_STARTED.fetch_add(1, Ordering::Release);
    false
}

/// The panel's command channel, for `panel_off` (also from the restart
/// handler, which has no `Display`).
static PANEL_IO: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// Turns the panel off properly: display off, then sleep. Without the video
/// stream (restarting, flashing) an LCD left on holds a one-sided charge on
/// its liquid crystal, which shows as flickering edges for a while after it
/// starts again. Blocks about 150 ms.
pub fn panel_off() {
    let io = PANEL_IO.load(Ordering::Acquire);
    if io.is_null() {
        return;
    }
    unsafe {
        // The backlight first, so nothing shows while the panel goes off.
        sys::ledc_stop(sys::ledc_mode_t_LEDC_LOW_SPEED_MODE, 0, 0);
        sys::esp_lcd_panel_io_tx_param(io.cast(), 0x28, ptr::null(), 0); // DISPOFF
        sys::esp_rom_delay_us(20_000);
        sys::esp_lcd_panel_io_tx_param(io.cast(), 0x10, ptr::null(), 0); // SLPIN
        sys::esp_rom_delay_us(120_000);
    }
}

/// Runs on every software restart (firmware updates, the console).
unsafe extern "C" fn on_shutdown() {
    panel_off();
}

pub struct Display {
    panel: b::esp_lcd_panel_handle_t,
    buffers: [*mut Rgb565Pixel; 2],
    /// Index of the buffer Slint draws into next.
    back: usize,
}

// SAFETY: as for the RGB panel (see rgb.rs): created on a helper thread,
// then moved once to the UI task, its only user.
unsafe impl Send for Display {}

impl Display {
    pub fn new() -> Result<Self, EspError> {
        unsafe {
            // Power the MIPI-DSI PHY.
            let ldo_config = b::esp_ldo_channel_config_t {
                chan_id: DSI_PHY_LDO_CHANNEL,
                voltage_mv: DSI_PHY_LDO_MV,
                ..Default::default()
            };
            let mut ldo: b::esp_ldo_channel_handle_t = ptr::null_mut();
            esp!(b::esp_ldo_acquire_channel(&ldo_config, &mut ldo))?;

            let bus_config = b::esp_lcd_dsi_bus_config_t {
                bus_id: 0,
                num_data_lanes: DSI_LANES,
                phy_clk_src:
                    b::soc_periph_mipi_dsi_phy_pllref_clk_src_t_MIPI_DSI_PHY_PLLREF_CLK_SRC_DEFAULT,
                lane_bit_rate_mbps: DSI_LANE_MBPS,
                ..Default::default()
            };
            let mut bus: b::esp_lcd_dsi_bus_handle_t = ptr::null_mut();
            esp!(b::esp_lcd_new_dsi_bus(&bus_config, &mut bus))?;

            let io_config = b::esp_lcd_dbi_io_config_t {
                virtual_channel: 0,
                lcd_cmd_bits: 8,
                lcd_param_bits: 8,
            };
            let mut io: b::esp_lcd_panel_io_handle_t = ptr::null_mut();
            esp!(b::esp_lcd_new_panel_io_dbi(bus, &io_config, &mut io))?;

            let mut dpi_config = b::esp_lcd_dpi_panel_config_t {
                virtual_channel: 0,
                dpi_clk_src: b::soc_periph_mipi_dsi_dpi_clk_src_t_MIPI_DSI_DPI_CLK_SRC_DEFAULT,
                dpi_clock_freq_mhz: DPI_CLOCK_MHZ,
                pixel_format: b::lcd_color_rgb_pixel_format_t_LCD_COLOR_PIXEL_FORMAT_RGB565,
                num_fbs: 2,
                ..Default::default()
            };
            let t = &mut dpi_config.video_timing;
            t.h_size = LCD_H_RES as u32;
            t.v_size = LCD_V_RES as u32;
            t.hsync_pulse_width = LCD_HSYNC_PULSE;
            t.hsync_back_porch = LCD_HSYNC_BACK_PORCH;
            t.hsync_front_porch = LCD_HSYNC_FRONT_PORCH;
            t.vsync_pulse_width = LCD_VSYNC_PULSE;
            t.vsync_back_porch = LCD_VSYNC_BACK_PORCH;
            t.vsync_front_porch = LCD_VSYNC_FRONT_PORCH;

            let init_cmds: Vec<b::jd9165_lcd_init_cmd_t> = LCD_INIT_COMMANDS
                .iter()
                .map(|(cmd, data, delay)| b::jd9165_lcd_init_cmd_t {
                    cmd: *cmd as i32,
                    data: data.as_ptr().cast(),
                    data_bytes: data.len(),
                    delay_ms: *delay,
                })
                .collect();
            let vendor = b::jd9165_vendor_config_t {
                init_cmds: init_cmds.as_ptr(),
                init_cmds_size: init_cmds.len() as u16,
                mipi_config: b::jd9165_vendor_config_t__bindgen_ty_1 {
                    dsi_bus: bus,
                    dpi_config: &dpi_config,
                },
            };
            let mut dev_config = b::esp_lcd_panel_dev_config_t {
                reset_gpio_num: LCD_PIN_RESET,
                bits_per_pixel: 16,
                vendor_config: (&vendor as *const b::jd9165_vendor_config_t)
                    .cast_mut()
                    .cast(),
                ..Default::default()
            };
            dev_config.__bindgen_anon_1.rgb_ele_order =
                b::lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_RGB;

            let mut panel: b::esp_lcd_panel_handle_t = ptr::null_mut();
            esp!(b::esp_lcd_new_panel_jd9165(io, &dev_config, &mut panel))?;
            esp!(sys::esp_lcd_panel_reset(panel.cast()))?;
            esp!(sys::esp_lcd_panel_init(panel.cast()))?;
            PANEL_IO.store(io.cast(), Ordering::Release);
            esp!(sys::esp_register_shutdown_handler(Some(on_shutdown)))?;

            let callbacks = b::esp_lcd_dpi_panel_event_callbacks_t {
                __bindgen_anon_1: b::esp_lcd_dpi_panel_event_callbacks_t__bindgen_ty_1 {
                    on_frame_buf_complete: Some(on_frame_buffer_switched),
                },
                ..Default::default()
            };
            esp!(b::esp_lcd_dpi_panel_register_event_callbacks(
                panel,
                &callbacks,
                ptr::null_mut()
            ))?;

            let mut fb0: *mut c_void = ptr::null_mut();
            let mut fb1: *mut c_void = ptr::null_mut();
            esp!(b::esp_lcd_dpi_panel_get_frame_buffer(
                panel, 2, &mut fb0, &mut fb1
            ))?;
            log::info!(
                "MIPI-DSI panel up: {LCD_H_RES}x{LCD_V_RES}, {DSI_LANES} lanes at {DSI_LANE_MBPS} Mbps, double buffered"
            );
            // The driver starts out showing buffer 0.
            FRONT_BUFFER.store(fb0 as usize, Ordering::Relaxed);
            Ok(Self {
                panel,
                buffers: [fb0.cast(), fb1.cast()],
                back: 1,
            })
        }
    }

    /// Bring-up aid: shows the DSI controller's own test pattern (0 = off,
    /// back to the frame buffer).
    pub fn test_pattern(&mut self, kind: u32) {
        let result = unsafe { b::esp_lcd_dpi_panel_set_pattern(self.panel, kind) };
        println!("pattern {kind}: {result}");
    }

    /// The buffer to draw the next frame into, row-major, stride `LCD_H_RES`.
    pub fn back_buffer(&mut self) -> &mut [Rgb565Pixel] {
        // SAFETY: see rgb.rs; the DPI driver keeps its buffers for good.
        unsafe { core::slice::from_raw_parts_mut(self.buffers[self.back], LCD_H_RES * LCD_V_RES) }
    }

    /// Puts the back buffer on screen and waits until the panel has switched
    /// to it.
    pub fn present(&mut self) {
        let buffer = self.buffers[self.back];
        unsafe {
            // One of the driver's own buffers: it writes back the cache and
            // switches to it at the next frame instead of copying.
            sys::esp_lcd_panel_draw_bitmap(
                self.panel.cast(),
                0,
                0,
                LCD_H_RES as i32,
                LCD_V_RES as i32,
                buffer as *const c_void,
            );
        }
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
