#![cfg_attr(not(feature = "dev-console"), allow(dead_code))]
//! Line-based debug console on UART0 (the CH340 USB port).
//!
//! Commands (one per line):
//!   tap X Y [MS] press at (X, Y) and release after MS ms (default 80)
//!   type TEXT    type TEXT on the on-screen keyboard (wizard text pages)
//!   key enter    press the keyboard's enter key
//!   key back     press backspace
//!   shot         dump the frame buffer (RLE + base64) for scripts/devctl.py
//!   status       print Wi-Fi/UI state and free heap
//!   sd           SD card usage and root directory listing
//!   sdformat ERASE   erase the whole SD card (one FAT32 partition), then restart
//!   sdcopyslot   copy the other firmware slot to /sdcard/update.bin (tests SD updates)
//!   saver        start the screensaver now (if enabled in settings)
//!   bench        time five full-screen redraws of the current page
//!   flipshot MS  flip the clock's last digit, wait MS, take a screenshot
//!   page NAME    jump to a page (welcome, networks, home, ...) for inspection
//!   forget       erase Wi-Fi credentials and return to the wizard
//!   reboot       restart the device
//!
//! The console exists for development: it lets a host script drive and
//! inspect the UI without touching the panel.

use std::io::{BufRead, Write};
use std::sync::mpsc::Sender;
use std::thread;

use esp_idf_svc::sys;

use crate::board::{LCD_H_RES, LCD_V_RES};

#[derive(Debug)]
pub enum ConsoleCommand {
    Tap { x: i16, y: i16, ms: u32 },
    Type(String),
    Enter,
    Backspace,
    Status,
    Sd,
    SdFormat,
    SdCopySlot,
    Saver,
    Bench,
    FlipTest,
    Page(String),
    Forget,
}

/// Starts the console thread.
pub fn start(tx: Sender<ConsoleCommand>) {
    unsafe {
        // Route stdin through the UART driver so reads block instead of
        // returning "would block" immediately.
        let err = sys::uart_driver_install(0, 1024, 0, 0, core::ptr::null_mut(), 0);
        if err != sys::ESP_OK {
            log::error!("console: uart_driver_install failed ({err})");
            return;
        }
        sys::uart_vfs_dev_use_driver(0);
    }

    let spawned = thread::Builder::new()
        .name("console".into())
        .stack_size(8 * 1024)
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { continue };
                let line = line.trim();
                let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
                let command = match cmd {
                    "" => continue,
                    "tap" => {
                        let mut it = arg.split_whitespace().filter_map(|v| v.parse::<i32>().ok());
                        match (it.next(), it.next()) {
                            (Some(x), Some(y)) => ConsoleCommand::Tap {
                                x: x as i16,
                                y: y as i16,
                                ms: it.next().unwrap_or(80).clamp(10, 10_000) as u32,
                            },
                            _ => {
                                println!("usage: tap X Y");
                                continue;
                            }
                        }
                    }
                    "type" => ConsoleCommand::Type(arg.to_string()),
                    "key" if arg == "enter" => ConsoleCommand::Enter,
                    "key" if arg == "back" => ConsoleCommand::Backspace,
                    "status" => ConsoleCommand::Status,
                    "sd" => ConsoleCommand::Sd,
                    "sdcopyslot" => ConsoleCommand::SdCopySlot,
                    "sdformat" if arg.trim() == "ERASE" => ConsoleCommand::SdFormat,
                    "sdformat" => {
                        println!("this erases the whole SD card; type: sdformat ERASE");
                        continue;
                    }
                    "saver" => ConsoleCommand::Saver,
                    "bench" => ConsoleCommand::Bench,
                    "flipshot" => {
                        let wait = arg.trim().parse::<u64>().unwrap_or(300);
                        if tx.send(ConsoleCommand::FlipTest).is_err() {
                            return;
                        }
                        thread::sleep(std::time::Duration::from_millis(wait));
                        screenshot(crate::display::front_buffer_addr());
                        continue;
                    }
                    "page" => ConsoleCommand::Page(arg.to_string()),
                    "forget" => ConsoleCommand::Forget,
                    "reboot" => unsafe { sys::esp_restart() },
                    "shot" => {
                        screenshot(crate::display::front_buffer_addr());
                        continue;
                    }
                    _ => {
                        println!("unknown command: {cmd}");
                        continue;
                    }
                };
                if tx.send(command).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("console: could not start thread: {e}");
    }
}

/// Prints the frame buffer as run-length encoded RGB565 (runs of
/// `count:u8, pixel:u16le`), base64 encoded, between marker lines.
fn screenshot(frame_buffer: usize) {
    // SAFETY: the address comes from the RGB panel driver, which owns
    // LCD_H_RES * LCD_V_RES pixels for the program's lifetime. It is the
    // buffer on screen, which nothing draws into until the next swap; a swap
    // during the copy can at worst mix two complete frames.
    let pixels =
        unsafe { core::slice::from_raw_parts(frame_buffer as *const u16, LCD_H_RES * LCD_V_RES) };

    // Runs as (length, pixel); streamed in two passes so the screenshot
    // needs no big buffer (PSRAM may be nearly full, e.g. with a
    // notification picture on screen).
    let runs = || {
        let mut i = 0;
        core::iter::from_fn(move || {
            let value = *pixels.get(i)?;
            let mut run = 1;
            while run < 255 && i + run < pixels.len() && pixels[i + run] == value {
                run += 1;
            }
            i += run;
            Some((run as u8, value))
        })
    };
    let total = runs().count() * 3;

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\nSHOT-BEGIN {LCD_H_RES} {LCD_V_RES} {total}");
    // 30 runs = 90 bytes = one 120-character base64 line without padding.
    let mut line = [0u8; 90];
    let mut len = 0;
    for (run, value) in runs() {
        line[len] = run;
        line[len + 1..len + 3].copy_from_slice(&value.to_le_bytes());
        len += 3;
        if len == line.len() {
            let _ = writeln!(out, "{}", base64(&line));
            len = 0;
        }
    }
    if len > 0 {
        let _ = writeln!(out, "{}", base64(&line[..len]));
    }
    let _ = writeln!(out, "SHOT-END");
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for (k, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if k <= chunk.len() {
                out.push(ALPHABET[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
