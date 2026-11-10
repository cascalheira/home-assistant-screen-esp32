//! Goodix GT911 capacitive touch controller on I2C.

use std::thread;
use std::time::Duration;

use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::gpio::{Output, OutputPin, PinDriver};
use esp_idf_svc::hal::i2c::I2cDriver;
use esp_idf_svc::sys::EspError;

use crate::board::{LCD_H_RES, LCD_V_RES};

/// Bounded so a wedged bus cannot freeze the UI loop.
const I2C_TIMEOUT: u32 = TickType::new_millis(50).0;

const REG_CONFIG_X_MAX: u16 = 0x8048; // x_lo, x_hi, y_lo, y_hi
const REG_PRODUCT_ID: u16 = 0x8140; // 4 ASCII bytes, e.g. "911\0"
const REG_STATUS: u16 = 0x814E; // bit 7: sample ready, bits 0-3: touch count
const REG_POINT_1: u16 = 0x8150; // x_lo, x_hi, y_lo, y_hi, size_lo, size_hi

/// What the controller reported since the last poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    /// No new sample yet; keep the previous state.
    None,
    Pressed {
        x: i16,
        y: i16,
    },
    Released,
}

pub struct Touch {
    i2c: I2cDriver<'static>,
    addr: u8,
    max_x: u32,
    max_y: u32,
    _reset: PinDriver<'static, Output>,
}

impl Touch {
    /// Resets the controller (selecting I2C address 0x5D) and probes it.
    pub fn new(
        i2c: I2cDriver<'static>,
        int_pin: impl OutputPin + 'static,
        reset_pin: impl OutputPin + 'static,
    ) -> Result<Self, EspError> {
        let mut reset = PinDriver::output(reset_pin)?;
        {
            // INT held low while RST rises selects address 0x5D. Afterwards
            // INT is released (the driver is dropped and the pin floats) so
            // the controller can drive it.
            let mut int = PinDriver::output(int_pin)?;
            int.set_low()?;
            reset.set_low()?;
            thread::sleep(Duration::from_millis(10));
            reset.set_high()?;
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(60));

        let mut touch = Self {
            i2c,
            addr: 0,
            max_x: LCD_H_RES as u32,
            max_y: LCD_V_RES as u32,
            _reset: reset,
        };

        let mut id = [0u8; 4];
        for addr in [0x5D, 0x14] {
            touch.addr = addr;
            if touch.read_reg(REG_PRODUCT_ID, &mut id).is_ok() {
                break;
            }
            touch.addr = 0;
        }
        if touch.addr == 0 {
            log::error!("GT911 touch controller not found on I2C");
            return Ok(touch);
        }

        let mut res = [0u8; 4];
        if touch.read_reg(REG_CONFIG_X_MAX, &mut res).is_ok() {
            let (x, y) = (
                u16::from_le_bytes([res[0], res[1]]),
                u16::from_le_bytes([res[2], res[3]]),
            );
            if x > 0 && y > 0 {
                touch.max_x = x as u32;
                touch.max_y = y as u32;
            }
        }
        let id = core::str::from_utf8(&id)
            .unwrap_or("?")
            .trim_end_matches('\0');
        log::info!(
            "GT{id} touch at 0x{:02X}, {}x{}",
            touch.addr,
            touch.max_x,
            touch.max_y
        );
        Ok(touch)
    }

    pub fn is_present(&self) -> bool {
        self.addr != 0
    }

    /// Polls the controller. The GT911 publishes a new sample about every
    /// 10 ms; in between it reports `Sample::None`.
    pub fn poll(&mut self) -> Sample {
        if self.addr == 0 {
            return Sample::None;
        }
        let mut status = [0u8; 1];
        if self.read_reg(REG_STATUS, &mut status).is_err() || status[0] & 0x80 == 0 {
            return Sample::None;
        }
        let count = status[0] & 0x0F;
        let mut sample = Sample::Released;
        if (1..=5).contains(&count) {
            let mut p = [0u8; 4];
            if self.read_reg(REG_POINT_1, &mut p).is_ok() {
                let raw_x = u16::from_le_bytes([p[0], p[1]]) as u32;
                let raw_y = u16::from_le_bytes([p[2], p[3]]) as u32;
                let x = (raw_x * LCD_H_RES as u32 / self.max_x).min(LCD_H_RES as u32 - 1);
                let y = (raw_y * LCD_V_RES as u32 / self.max_y).min(LCD_V_RES as u32 - 1);
                sample = Sample::Pressed {
                    x: x as i16,
                    y: y as i16,
                };
            } else {
                sample = Sample::None;
            }
        }
        // Acknowledge the sample so the controller produces the next one.
        let _ = self.write_reg(REG_STATUS, 0);
        sample
    }

    fn read_reg(&mut self, reg: u16, buf: &mut [u8]) -> Result<(), EspError> {
        self.i2c
            .write_read(self.addr, &reg.to_be_bytes(), buf, I2C_TIMEOUT)
    }

    fn write_reg(&mut self, reg: u16, value: u8) -> Result<(), EspError> {
        let [hi, lo] = reg.to_be_bytes();
        self.i2c.write(self.addr, &[hi, lo, value], I2C_TIMEOUT)
    }
}
