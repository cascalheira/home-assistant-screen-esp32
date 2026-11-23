//! Motion detection on the camera's brightness grid.

use esp32_screen_host_tests::logic::motion::{MotionDetector, COLS, ROWS};

/// A room: a gradient, with a little noise from `seed`.
fn room(seed: u32, brightness: i32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..COLS * ROWS)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = (s % 7) as i32 - 3;
            (60 + (i % COLS) as i32 * 3 + brightness + noise).clamp(0, 255) as u8
        })
        .collect()
}

/// Someone: a dark block over columns `x..x+6`, rows 4..16.
fn with_person(mut grid: Vec<u8>, x: usize) -> Vec<u8> {
    for row in 4..16 {
        for col in x..(x + 6).min(COLS) {
            grid[row * COLS + col] = 20;
        }
    }
    grid
}

fn settled() -> MotionDetector {
    let mut d = MotionDetector::new();
    for i in 0..16 {
        assert!(!d.feed(&room(i, 0)));
    }
    d
}

#[test]
fn a_still_room_is_not_motion() {
    let mut d = settled();
    for i in 100..300 {
        assert!(!d.feed(&room(i, 0)), "frame {i}");
    }
}

#[test]
fn brightness_changes_are_not_motion() {
    let mut d = settled();
    // Auto exposure or a lamp: the whole picture brighter, then darker.
    for (i, b) in [10, 25, 40, 40, 20, 0, -20].iter().enumerate() {
        assert!(!d.feed(&room(200 + i as u32, *b)), "brightness {b}");
    }
}

#[test]
fn someone_walking_in_is_motion_after_three_frames() {
    let mut d = settled();
    assert!(
        !d.feed(&with_person(room(50, 0), 2)),
        "one frame is not enough"
    );
    assert!(!d.feed(&with_person(room(51, 0), 4)), "nor two");
    assert!(d.feed(&with_person(room(52, 0), 6)));
}

#[test]
fn moving_light_bands_are_not_motion() {
    // Mains-powered light on a rolling shutter: bright and dark rows that
    // move a little every frame.
    let mut d = settled();
    for frame in 0..40usize {
        let mut grid = room(300 + frame as u32, 0);
        for (i, cell) in grid.iter_mut().enumerate() {
            let row = i / COLS;
            let band = if (row + frame) % 4 < 2 { 35 } else { -35 };
            *cell = (*cell as i32 + band).clamp(0, 255) as u8;
        }
        assert!(!d.feed(&grid), "frame {frame}");
    }
}

#[test]
fn a_single_odd_frame_is_not_motion() {
    let mut d = settled();
    assert!(!d.feed(&with_person(room(60, 0), 10)));
    assert!(!d.feed(&room(61, 0)));
    assert!(!d.feed(&room(62, 0)));
}

#[test]
fn reset_learns_the_room_again() {
    let mut d = settled();
    d.reset();
    // A different room right after a reset is the new background.
    for i in 0..15 {
        assert!(!d.feed(&with_person(room(70 + i, 30), 12)));
    }
    assert!(!d.feed(&with_person(room(90, 30), 12)));
    assert!(!d.feed(&with_person(room(91, 30), 12)));
}

#[test]
fn a_small_change_like_camera_noise_is_not_motion() {
    // 20 scattered cells changing every frame: about what a still room
    // showed on the real camera.
    let mut d = settled();
    for frame in 0..20u32 {
        let mut grid = room(400 + frame, 0);
        for k in 0..20usize {
            let i = (k * 29 + frame as usize * 7) % grid.len();
            grid[i] = grid[i].saturating_add(40);
        }
        assert!(!d.feed(&grid), "frame {frame}");
    }
}

#[test]
fn a_wrong_size_grid_is_ignored() {
    let mut d = settled();
    assert!(!d.feed(&[0; 10]));
}
