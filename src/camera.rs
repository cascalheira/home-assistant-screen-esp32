//! The ESP32-P4 board's camera, used only to wake the screen when someone
//! moves in front of it. Frames are reduced to a 32x18 brightness grid on
//! their own thread and compared there; no picture is kept or sent.
//!
//! The camera streams only while the main loop asks for it (the screensaver
//! shows and waking on motion is on).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use esp_idf_svc::sys::board_p4 as b;

use crate::logic::motion::{MotionDetector, COLS, ROWS};

pub enum Event {
    /// The camera started (true) or not (false).
    Ready(bool),
    Motion,
}

pub struct Camera {
    wanted: Arc<AtomicBool>,
    reset: Arc<AtomicBool>,
    pub events: Receiver<Event>,
}

impl Camera {
    pub fn start() -> Self {
        let wanted = Arc::new(AtomicBool::new(false));
        let reset = Arc::new(AtomicBool::new(false));
        let (tx, events) = channel();
        let (w, r) = (wanted.clone(), reset.clone());
        let spawned = thread::Builder::new()
            .name("camera".into())
            .stack_size(16 * 1024)
            .spawn(move || run(w, r, tx));
        if let Err(e) = spawned {
            log::warn!("camera thread: {e}");
        }
        Self {
            wanted,
            reset,
            events,
        }
    }

    /// Stream and look for motion (true) or rest (false).
    pub fn set_wanted(&self, wanted: bool) {
        self.wanted.store(wanted, Ordering::Relaxed);
    }

    /// The light changed (the backlight, for one): learn the room again.
    pub fn reset(&self) {
        self.reset.store(true, Ordering::Relaxed);
    }
}

fn run(wanted: Arc<AtomicBool>, reset: Arc<AtomicBool>, events: Sender<Event>) {
    let bus = crate::board::CAMERA_BUS.load(Ordering::Acquire);
    let started = !bus.is_null() && unsafe { b::board_p4_camera_init(bus.cast()) } == 0;
    if !started {
        log::warn!("camera: not available");
    }
    let _ = events.send(Event::Ready(started));
    if !started {
        return;
    }
    let mut detector = MotionDetector::new();
    let mut grid = vec![0u8; COLS * ROWS];
    let mut streaming = false;
    // For the log: frames and the most cells changed since the last line.
    let mut frames = 0u32;
    let mut most_changed = 0usize;
    let mut summary_at = std::time::Instant::now();
    loop {
        let want = wanted.load(Ordering::Relaxed);
        if want != streaming {
            let result = unsafe { b::board_p4_camera_stream(want) };
            if result != 0 {
                log::warn!("camera: streaming {want} failed ({result})");
                thread::sleep(Duration::from_secs(5));
                continue;
            }
            log::info!("camera {}", if want { "on" } else { "off" });
            streaming = want;
            detector.reset();
        }
        if !streaming {
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        if reset.swap(false, Ordering::Relaxed) {
            detector.reset();
        }
        let result =
            unsafe { b::board_p4_camera_grid(grid.as_mut_ptr(), COLS as i32, ROWS as i32) };
        if result != 0 {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let moved = detector.feed(&grid);
        frames += 1;
        most_changed = most_changed.max(detector.last_changed);
        if summary_at.elapsed() >= Duration::from_secs(30) {
            log::info!("camera: {frames} frames in 30 s, at most {most_changed} cells changed");
            (frames, most_changed) = (0, 0);
            summary_at = std::time::Instant::now();
        }
        if moved {
            log::info!("camera: motion ({} cells changed)", detector.last_changed);
            if events.send(Event::Motion).is_err() {
                return;
            }
            // One wake-up is enough; then learn the room again.
            detector.reset();
        }
        thread::sleep(Duration::from_millis(150));
    }
}
