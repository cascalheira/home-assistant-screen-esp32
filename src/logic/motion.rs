//! Motion detection on a coarse brightness grid from the camera.
//!
//! Each frame is compared with a slowly adapting background. Changes of
//! whole rows' brightness are taken out first: the camera's auto exposure, a
//! lamp, the screen dimming, and the moving bands mains-powered lights make
//! on a rolling-shutter camera. What is left must change enough cells, in
//! three frames in a row, to count as someone moving.

pub const COLS: usize = 32;
pub const ROWS: usize = 18;
const CELLS: usize = COLS * ROWS;
/// A cell changed when its brightness moved more than this (0-255).
const CELL_DELTA: i32 = 18;
/// Cells that must change: about 2% of the picture.
const MIN_CELLS: usize = 12;
/// Frames used to learn the background after a (re)start (the camera's
/// auto exposure settles meanwhile).
const SETTLE_FRAMES: u32 = 15;
/// The background follows the picture by 1/ADAPT of the difference per frame.
const ADAPT: i32 = 8;

/// Frames in a row with changes that make motion.
const HITS: u8 = 3;

pub struct MotionDetector {
    /// Background brightness, times 16.
    background: Vec<i32>,
    frames: u32,
    hits: u8,
    /// Cells changed in the last frame (for logging).
    pub last_changed: usize,
}

impl Default for MotionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionDetector {
    pub fn new() -> Self {
        Self {
            background: vec![0; CELLS],
            frames: 0,
            hits: 0,
            last_changed: 0,
        }
    }

    /// Learn the background again (the camera restarted, the light changed).
    pub fn reset(&mut self) {
        self.frames = 0;
        self.hits = 0;
    }

    /// Feeds a frame's grid (`COLS` x `ROWS`). True when someone moved.
    pub fn feed(&mut self, grid: &[u8]) -> bool {
        if grid.len() != CELLS {
            return false;
        }
        self.frames = self.frames.saturating_add(1);
        if self.frames <= SETTLE_FRAMES {
            for (b, &g) in self.background.iter_mut().zip(grid) {
                *b = g as i32 * 16;
            }
            return false;
        }
        let mut changed = 0;
        for (bg_row, row) in self.background.chunks_mut(COLS).zip(grid.chunks(COLS)) {
            // The row's own brightness change, by its median cell (a person
            // covering part of the row hardly moves the median).
            let mut diffs: Vec<i32> = bg_row
                .iter()
                .zip(row)
                .map(|(b, &g)| g as i32 * 16 - b)
                .collect();
            diffs.sort_unstable();
            let shift = diffs[COLS / 2];
            for (b, &g) in bg_row.iter_mut().zip(row) {
                let now = g as i32 * 16;
                if (now - *b - shift).abs() > CELL_DELTA * 16 {
                    changed += 1;
                }
                *b += (now - *b) / ADAPT;
            }
        }
        self.last_changed = changed;
        if changed >= MIN_CELLS {
            self.hits = self.hits.saturating_add(1);
        } else {
            self.hits = 0;
        }
        self.hits >= HITS
    }
}
