//! Screensaver photos from the SD card: JPEGs in a `photos` folder at the
//! card's top level (any letter case), shown in a shuffled order.
//!
//! Decoding runs on its own thread: a camera photo takes a second or more.
//! Large photos are reduced while decoding (1/2 to 1/8) so they still fill
//! the screen, and turned upright from their EXIF orientation.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use crate::ha::Picture;
use crate::logic::image;

/// Bigger files are skipped (the whole file is read into memory).
const MAX_FILE: u64 = 12 * 1024 * 1024;

pub struct SdPhotos {
    count: usize,
    requests: Sender<Request>,
    pub results: Receiver<Result<Picture, String>>,
}

struct Request {
    /// The box the photo should fill.
    target: (u32, u32),
    /// Most pixels the decoded photo may have.
    max_pixels: u64,
}

impl SdPhotos {
    /// Looks for photos on the card (mounted at `root`) and starts the
    /// decoding thread. None when there are none.
    pub fn start(root: &str) -> Option<Self> {
        let files = find(root);
        if files.is_empty() {
            return None;
        }
        log::info!("{} photos on the SD card", files.len());
        let count = files.len();
        let (requests, rx) = channel::<Request>();
        let (tx, results) = channel();
        let spawned = thread::Builder::new()
            .name("photos".into())
            .stack_size(12 * 1024)
            .spawn(move || run(files, rx, tx));
        if let Err(e) = spawned {
            log::warn!("photo thread: {e}");
            return None;
        }
        Some(Self {
            count,
            requests,
            results,
        })
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Asks for the next photo; the answer comes on `results`.
    pub fn request(&self, target: (u32, u32), max_pixels: u64) {
        let _ = self.requests.send(Request { target, max_pixels });
    }
}

/// JPEG files in the card's photos folder (and its subfolders).
fn find(root: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let folder = entries.flatten().find(|e| {
        e.file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("photos")
            && e.file_type().is_ok_and(|t| t.is_dir())
    });
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = folder.map(|f| f.path()).into_iter().collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if !name.starts_with('.') && (name.ends_with(".jpg") || name.ends_with(".jpeg"))
            {
                files.push(path);
            }
            if files.len() >= 5000 {
                return files;
            }
        }
    }
    files
}

fn run(
    mut files: Vec<PathBuf>,
    requests: Receiver<Request>,
    results: Sender<Result<Picture, String>>,
) {
    let mut seed = unsafe { esp_idf_svc::sys::esp_random() } | 1;
    let mut next = files.len();
    while let Ok(request) = requests.recv() {
        // Try a few files: some may be unreadable (progressive JPEGs).
        let mut result = Err("no readable photo".to_string());
        for _ in 0..files.len().min(8) {
            if next >= files.len() {
                shuffle(&mut files, &mut seed);
                next = 0;
            }
            let path = &files[next];
            next += 1;
            match load(path, &request) {
                Ok(picture) => {
                    result = Ok(picture);
                    break;
                }
                Err(e) => {
                    log::warn!("photo {}: {e:#}", path.display());
                    result = Err(format!("{e:#}"));
                }
            }
        }
        if results.send(result).is_err() {
            return;
        }
    }
}

fn shuffle(files: &mut [PathBuf], seed: &mut u32) {
    for i in (1..files.len()).rev() {
        // xorshift32
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        files.swap(i, *seed as usize % (i + 1));
    }
}

fn load(path: &PathBuf, request: &Request) -> anyhow::Result<Picture> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FILE {
        anyhow::bail!("file too large ({size} bytes)");
    }
    let mut data = std::fs::read(path)?;
    let orientation = image::jpeg_orientation(&data);
    let (w, h) = crate::ha::jpeg_size(&mut data)
        .ok_or_else(|| anyhow::anyhow!("not a JPEG the screen can read (progressive?)"))?;
    // A photo stored sideways fills the box turned.
    let target = if matches!(orientation, 5..=8) {
        (request.target.1, request.target.0)
    } else {
        request.target
    };
    let scale = image::jpeg_scale(w, h, target, request.max_pixels)
        .ok_or_else(|| anyhow::anyhow!("too large ({w}x{h})"))?;
    let picture = crate::ha::decode_jpeg_scaled(data, scale, request.max_pixels as usize)?;
    if matches!(orientation, 3 | 6 | 8) {
        let (pw, ph) = (picture.width() as usize, picture.height() as usize);
        let (pixels, nw, nh) = image::rotate_rgb(picture.as_bytes(), pw, ph, orientation);
        drop(picture);
        return Ok(Picture::clone_from_slice(&pixels, nw as u32, nh as u32));
    }
    Ok(picture)
}
